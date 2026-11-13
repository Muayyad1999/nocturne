//! Putting finds into the inventory — preview first, then import.
//!
//! Adding is the one thing discovery does that changes anything, so it is
//! shaped like the CSV import on the devices screen: `discovery.preview`
//! says exactly what would be created, with the evidence for every field and
//! a reason for every row it would skip, and `discovery.add` then does it.
//! Neither ever logs in anywhere: a device is created with the transport its
//! open ports suggest and the SNMP credential that already answered, and the
//! first login is the user's own doing.

use std::collections::HashSet;

use nocturne_app::inventory::{self, DeviceInput};
use nocturne_app::model::{Device, MonitorSpec, Transport, collections::DEVICES};
use nocturne_app::{App, AppError, AppResult, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db;
use crate::model::{Evidence, Found};

/// What the user wants every added device to have.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub site: Option<String>,
    pub role: Option<String>,
    pub tags: Option<Vec<String>>,
    /// The CLI credential to attach. Discovery does not test it — that is a login.
    pub credential_id: Option<String>,
    /// Override the transport the find suggests (`ssh`, `telnet`, `none`).
    pub transport: Option<String>,
    /// Switch monitoring on for the added devices.
    pub monitor: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct AddParams {
    pub scan_id: String,
    /// Addresses from that scan. Empty means every find that is new.
    pub addresses: Vec<String>,
    pub defaults: Defaults,
}

/// One device that would be created.
#[derive(Debug, Serialize)]
pub struct PreviewRow {
    pub address: String,
    pub name: String,
    pub profile: String,
    pub transport: String,
    pub site: String,
    pub role: String,
    pub tags: Vec<String>,
    pub monitor_snmp: bool,
    pub snmp_credential_id: Option<String>,
    pub credential_id: Option<String>,
    /// `new`, or `skip` with a reason.
    pub status: String,
    pub reason: Option<String>,
    /// Why each field says what it says.
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Serialize)]
pub struct Preview {
    pub scan_id: String,
    pub rows: Vec<PreviewRow>,
    pub new: usize,
    pub skipped: usize,
}

#[derive(Debug, Serialize)]
pub struct Skipped {
    pub address: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct Added {
    pub created: Vec<String>,
    pub skipped: Vec<Skipped>,
}

/// Work out what would be created, without creating anything.
pub fn preview(app: &App, params: &AddParams) -> AppResult<Preview> {
    let hosts = chosen(app, params)?;
    let existing: Vec<Device> = app.store().list(DEVICES)?;
    let mut taken: HashSet<String> = existing
        .iter()
        .map(|device| device.name.to_lowercase())
        .collect();
    let mut rows = Vec::new();
    for found in hosts {
        rows.push(row(&found, &params.defaults, &mut taken));
    }
    Ok(Preview {
        scan_id: params.scan_id.clone(),
        new: rows.iter().filter(|row| row.status == "new").count(),
        skipped: rows.iter().filter(|row| row.status != "new").count(),
        rows,
    })
}

/// Create the devices. Every row the preview would skip is skipped here too,
/// with the same reason.
pub fn add(app: &App, params: &AddParams) -> AppResult<Added> {
    let preview = preview(app, params)?;
    let mut added = Added {
        created: Vec::new(),
        skipped: Vec::new(),
    };
    for row in preview.rows {
        if row.status != "new" {
            added.skipped.push(Skipped {
                address: row.address,
                reason: row.reason.unwrap_or_else(|| "skipped".into()),
            });
            continue;
        }
        let input = DeviceInput {
            name: Some(row.name.clone()),
            address: Some(row.address.clone()),
            profile: Some(row.profile.clone()),
            transport: Some(transport_of(&row.transport)),
            credential_id: row.credential_id.clone(),
            site: Some(row.site.clone()),
            role: Some(row.role.clone()),
            tags: Some(row.tags.clone()),
            notes: Some(notes_for(&row)),
            monitor: Some(MonitorSpec {
                enabled: true,
                icmp: true,
                snmp: row.monitor_snmp,
                snmp_credential_id: row.snmp_credential_id.clone(),
                ..MonitorSpec::default()
            }),
            ..DeviceInput::default()
        };
        match inventory::create(app, input) {
            Ok(created) => added.created.push(created.device.id),
            Err(error) => added.skipped.push(Skipped {
                address: row.address,
                reason: error.message,
            }),
        }
    }
    if !added.created.is_empty() {
        app.audit(
            "discovery.add",
            None,
            format!("Added {} device(s) found by discovery", added.created.len()),
            json!({"scan_id": params.scan_id, "device_ids": added.created}),
        );
        app.publish(
            "discovery.added",
            &json!({"scan_id": params.scan_id, "device_ids": added.created}),
        );
    }
    Ok(added)
}

/// The finds this call is about: the addresses asked for, or every new find.
fn chosen(app: &App, params: &AddParams) -> AppResult<Vec<Found>> {
    let mut hosts = db::hosts(app, &params.scan_id)?;
    if hosts.is_empty() {
        // Distinguish "no such scan" from "that scan found nothing".
        db::get_scan(app, &params.scan_id)?;
    }
    // The inventory and the ignore list may have moved since the scan ran, and
    // what is skipped must follow them rather than the scan's own memory.
    crate::scan::refresh(app, &mut hosts)?;
    if params.addresses.is_empty() {
        hosts.retain(|host| host.device_id.is_none() && !host.ignored);
        return Ok(hosts);
    }
    let mut chosen = Vec::new();
    for address in &params.addresses {
        let wanted = address.trim();
        match hosts.iter().find(|host| host.address == wanted) {
            Some(host) => chosen.push(host.clone()),
            None => {
                return Err(AppError::not_found(format!(
                    "`{wanted}` among the finds of scan `{}`",
                    params.scan_id
                )));
            }
        }
    }
    Ok(chosen)
}

fn row(found: &Found, defaults: &Defaults, taken: &mut HashSet<String>) -> PreviewRow {
    let mut evidence = found.evidence.clone();
    let site = defaults
        .site
        .clone()
        .filter(|site| !site.is_empty())
        .unwrap_or_else(|| found.suggested.site.clone());
    let role = defaults
        .role
        .clone()
        .filter(|role| !role.is_empty())
        .unwrap_or_else(|| found.suggested.role.clone());
    let transport = defaults
        .transport
        .clone()
        .filter(|transport| !transport.is_empty())
        .unwrap_or_else(|| found.suggested.transport.clone());
    if defaults.transport.is_some() {
        evidence.push(Evidence::new(
            "suggested.transport",
            "user",
            format!("the import asked for `{transport}` for every device"),
        ));
    }

    let mut name = found.suggested.name.clone();
    if !taken.insert(name.to_lowercase()) {
        // The inventory requires unique names; the address is what tells two
        // devices of the same name apart, so it goes in the name.
        name = format!("{name} ({})", found.address);
        evidence.push(Evidence::new(
            "suggested.name",
            "inventory",
            format!(
                "a device named `{}` already exists, so the address was appended",
                found.suggested.name
            ),
        ));
        taken.insert(name.to_lowercase());
    }

    let (status, reason) = if let Some(device) = &found.device_name {
        (
            "skip",
            Some(format!("already in the inventory as `{device}`")),
        )
    } else if found.ignored {
        ("skip", Some("on the ignore list".to_string()))
    } else {
        ("new", None)
    };

    let snmp_credential_id = found
        .snmp
        .as_ref()
        .map(|snmp| snmp.credential_id.clone())
        .filter(|id| !id.is_empty());
    PreviewRow {
        address: found.address.clone(),
        name,
        profile: found.suggested.profile.clone(),
        transport,
        site,
        role,
        tags: defaults.tags.clone().unwrap_or_default(),
        monitor_snmp: snmp_credential_id.is_some(),
        snmp_credential_id,
        credential_id: defaults.credential_id.clone(),
        status: status.to_string(),
        reason,
        evidence,
    }
}

fn transport_of(name: &str) -> Transport {
    match name {
        "ssh" => Transport::Ssh,
        "telnet" => Transport::Telnet,
        "serial" => Transport::Serial,
        _ => Transport::None,
    }
}

/// What the device's notes say about where it came from. A person opening it
/// in six months should be able to see that it was found, not typed in.
fn notes_for(row: &PreviewRow) -> String {
    let mut lines = vec!["Added by discovery.".to_string()];
    for evidence in &row.evidence {
        lines.push(format!(
            "- {} ({}): {}",
            evidence.field, evidence.source, evidence.detail
        ));
    }
    lines.join("\n")
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct IgnoreParams {
    pub addresses: Vec<String>,
    pub note: String,
}

pub fn register(router: &mut Router) {
    router.add(
        "discovery.preview",
        |app: App, params: AddParams| async move { preview(&app, &params) },
    );
    router.add("discovery.add", |app: App, params: AddParams| async move {
        add(&app, &params)
    });
    router.add(
        "discovery.ignore",
        |app: App, params: IgnoreParams| async move {
            let stored = db::ignore(&app, &params.addresses, &params.note)?;
            app.audit(
                "discovery.ignore",
                None,
                format!("Ignored {} address(es) in discovery", stored.len()),
                json!({"addresses": stored}),
            );
            app.publish("discovery.ignored_changed", &json!({"ignored": stored}));
            Ok(json!({"ignored": stored}))
        },
    );
    router.add(
        "discovery.unignore",
        |app: App, params: IgnoreParams| async move {
            let removed = db::unignore(&app, &params.addresses)?;
            app.audit(
                "discovery.unignore",
                None,
                format!(
                    "Stopped ignoring {} address(es) in discovery",
                    removed.len()
                ),
                json!({"addresses": removed}),
            );
            app.publish("discovery.ignored_changed", &json!({"unignored": removed}));
            Ok(json!({"unignored": removed}))
        },
    );
    router.add(
        "discovery.ignored",
        |app: App, _: nocturne_app::NoParams| async move { db::ignored(&app) },
    );
}
