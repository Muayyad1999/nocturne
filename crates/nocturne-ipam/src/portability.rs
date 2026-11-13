//! CSV in and out, with the same preview-then-commit shape the device import
//! in the core has.
//!
//! An export is the *plan*: records, not sightings. Asking for the observed
//! addresses as well is a separate switch, and the rows then say which they
//! are, because a spreadsheet that mixes the two is how the mess starts.

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::model::{
    AddressRecord, DhcpRange, ObservationKind, Prefix, Role, Settings, Source, Status, VlanRecord,
};
use crate::net;
use crate::prefixes::{self, Entry};

/// What one line of an import would become.
#[derive(Debug, Serialize)]
pub struct ImportRow {
    pub line: usize,
    /// `new`, `update` or `invalid`.
    pub status: String,
    pub problem: Option<String>,
    /// The parsed row, for the preview table.
    pub value: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub kind: String,
    pub dry_run: bool,
    pub rows: Vec<ImportRow>,
    pub new: usize,
    pub update: usize,
    pub invalid: usize,
    pub created: usize,
    pub updated: usize,
}

#[derive(Debug, Default, Deserialize)]
struct PrefixCsv {
    #[serde(alias = "prefix", alias = "network")]
    cidr: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    site: String,
    #[serde(default)]
    vlan_id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    gateway: String,
    #[serde(default)]
    dhcp_start: String,
    #[serde(default)]
    dhcp_end: String,
    #[serde(default)]
    tags: String,
}

#[derive(Debug, Default, Deserialize)]
struct AddressCsv {
    #[serde(alias = "address")]
    ip: String,
    #[serde(default, alias = "prefix")]
    prefix_cidr: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    device: String,
    #[serde(default)]
    interface: String,
    #[serde(default)]
    mac: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    site: String,
}

#[derive(Debug, Default, Deserialize)]
struct VlanCsv {
    #[serde(alias = "vlan", alias = "vlan_id")]
    vid: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    site: String,
    #[serde(default)]
    description: String,
    #[serde(default, alias = "prefixes")]
    prefix_cidrs: String,
}

fn tags_of(text: &str) -> Vec<String> {
    text.split([';', '|'])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

fn role_of(text: &str) -> Option<Role> {
    serde_json::from_value(serde_json::Value::String(text.trim().to_ascii_lowercase())).ok()
}

fn status_of(text: &str) -> AppResult<Status> {
    match text.trim().to_ascii_lowercase().as_str() {
        "" | "active" => Ok(Status::Active),
        "reserved" => Ok(Status::Reserved),
        "dhcp" => Ok(Status::Dhcp),
        other => Err(AppError::invalid(format!(
            "`{other}` is not a status a record can have (active, reserved, dhcp)"
        ))),
    }
}

/// Parse and, unless `dry_run`, commit.
pub fn import(
    app: &App,
    kind: &str,
    text: &str,
    dry_run: bool,
    settings: &Settings,
) -> AppResult<ImportResult> {
    let mut result = ImportResult {
        kind: kind.to_string(),
        dry_run,
        rows: Vec::new(),
        new: 0,
        update: 0,
        invalid: 0,
        created: 0,
        updated: 0,
    };
    match kind {
        "prefixes" => import_prefixes(app, text, dry_run, settings, &mut result)?,
        "addresses" => import_addresses(app, text, dry_run, &mut result)?,
        "vlans" => import_vlans(app, text, dry_run, &mut result)?,
        other => {
            return Err(AppError::invalid(format!(
                "`{other}` cannot be imported (prefixes, addresses, vlans)"
            )));
        }
    }
    result.new = result.rows.iter().filter(|row| row.status == "new").count();
    result.update = result
        .rows
        .iter()
        .filter(|row| row.status == "update")
        .count();
    result.invalid = result
        .rows
        .iter()
        .filter(|row| row.status == "invalid")
        .count();
    Ok(result)
}

fn reader(text: &str) -> csv::Reader<&[u8]> {
    csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(text.as_bytes())
}

fn import_prefixes(
    app: &App,
    text: &str,
    dry_run: bool,
    settings: &Settings,
    result: &mut ImportResult,
) -> AppResult<()> {
    let entries = prefixes::load(app)?;
    let mut reader = reader(text);
    for (index, record) in reader.deserialize::<PrefixCsv>().enumerate() {
        let line = index + 2;
        let row = match record {
            Ok(row) => row,
            Err(err) => {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(err.to_string()),
                    value: serde_json::Value::Null,
                });
                continue;
            }
        };
        let site = (!row.site.trim().is_empty()).then(|| row.site.trim().to_string());
        let parsed = match net::strict_net(&row.cidr) {
            Ok((cidr, _)) => cidr,
            Err(err) => {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(err.message),
                    value: serde_json::json!({"cidr": row.cidr}),
                });
                continue;
            }
        };
        let existing = entries
            .iter()
            .find(|entry| entry.prefix.cidr == parsed && entry.prefix.site == site)
            .map(|entry| entry.prefix.id.clone());
        let vlan_id = match row.vlan_id.trim() {
            "" => None,
            text => match text.parse::<u16>() {
                Ok(vid) if (1..=4094).contains(&vid) => Some(vid),
                _ => {
                    result.rows.push(ImportRow {
                        line,
                        status: "invalid".into(),
                        problem: Some(format!("`{text}` is not a VLAN id (1 to 4094)")),
                        value: serde_json::json!({"cidr": parsed}),
                    });
                    continue;
                }
            },
        };
        let dhcp = match (row.dhcp_start.trim(), row.dhcp_end.trim()) {
            ("", "") => None,
            (start, end) => Some(DhcpRange {
                start: start.to_string(),
                end: end.to_string(),
            }),
        };
        let input = prefixes::PrefixInput {
            id: existing.clone(),
            cidr: Some(parsed.clone()),
            name: Some(row.name.clone()),
            description: Some(row.description.clone()),
            site: Some(row.site.clone()),
            vlan_id,
            role: role_of(&row.role),
            gateway: Some(row.gateway.clone()),
            dhcp,
            tags: Some(tags_of(&row.tags)),
        };
        let value = serde_json::json!({
            "cidr": parsed,
            "name": row.name,
            "site": site,
            "vlan_id": vlan_id,
            "role": row.role,
            "gateway": row.gateway,
        });
        if dry_run {
            // Validate without writing, so the preview tells the truth.
            if let Err(err) = validate_prefix(&input, &entries) {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(err.message),
                    value,
                });
                continue;
            }
            result.rows.push(ImportRow {
                line,
                status: if existing.is_some() { "update" } else { "new" }.into(),
                problem: None,
                value,
            });
            continue;
        }
        match prefixes::save(app, input, settings) {
            Ok(saved) => {
                if saved.created {
                    result.created += 1;
                } else {
                    result.updated += 1;
                }
                result.rows.push(ImportRow {
                    line,
                    status: if saved.created { "new" } else { "update" }.into(),
                    problem: (!saved.warnings.is_empty()).then(|| saved.warnings.join("; ")),
                    value,
                });
            }
            Err(err) => result.rows.push(ImportRow {
                line,
                status: "invalid".into(),
                problem: Some(err.message),
                value,
            }),
        }
    }
    Ok(())
}

/// The checks a preview can make without touching anything.
fn validate_prefix(input: &prefixes::PrefixInput, entries: &[Entry]) -> AppResult<()> {
    let cidr = input
        .cidr
        .clone()
        .ok_or_else(|| AppError::invalid("a prefix needs a CIDR block"))?;
    let (_, block) = net::strict_net(&cidr)?;
    if let Some(gateway) = input
        .gateway
        .as_ref()
        .filter(|text| !text.trim().is_empty())
    {
        let address = net::addr(gateway)?;
        if !block.contains_addr(&address) {
            return Err(AppError::invalid(format!(
                "the gateway {} is not inside {cidr}",
                address.text
            )));
        }
    }
    if let Some(dhcp) = &input.dhcp {
        let start = net::addr(&dhcp.start)?;
        let end = net::addr(&dhcp.end)?;
        if start.value > end.value {
            return Err(AppError::invalid(format!(
                "the DHCP range {} to {} runs backwards",
                start.text, end.text
            )));
        }
        if !block.contains_addr(&start) || !block.contains_addr(&end) {
            return Err(AppError::invalid(format!(
                "the DHCP range {} to {} is not inside {cidr}",
                start.text, end.text
            )));
        }
    }
    let _ = entries;
    Ok(())
}

fn import_addresses(
    app: &App,
    text: &str,
    dry_run: bool,
    result: &mut ImportResult,
) -> AppResult<()> {
    let entries = prefixes::load(app)?;
    let devices = crate::collected::devices(app)?;
    let mut reader = reader(text);
    for (index, record) in reader.deserialize::<AddressCsv>().enumerate() {
        let line = index + 2;
        let row = match record {
            Ok(row) => row,
            Err(err) => {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(err.to_string()),
                    value: serde_json::Value::Null,
                });
                continue;
            }
        };
        let address = match net::addr(&row.ip) {
            Ok(address) => address,
            Err(err) => {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(err.message),
                    value: serde_json::json!({"ip": row.ip}),
                });
                continue;
            }
        };
        let status = match status_of(&row.status) {
            Ok(status) => status,
            Err(err) => {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(err.message),
                    value: serde_json::json!({"ip": address.text}),
                });
                continue;
            }
        };
        // A named prefix must exist; without one the containing prefix is used.
        let prefix = if row.prefix_cidr.trim().is_empty() {
            prefixes::containing(&entries, &scope_of(&row.site), &address)
                .map(|entry| entry.prefix.id.clone())
        } else {
            match net::strict_net(&row.prefix_cidr) {
                Ok((cidr, _)) => match entries.iter().find(|entry| entry.prefix.cidr == cidr) {
                    Some(entry) => Some(entry.prefix.id.clone()),
                    None => {
                        result.rows.push(ImportRow {
                            line,
                            status: "invalid".into(),
                            problem: Some(format!("no prefix `{cidr}` is on record")),
                            value: serde_json::json!({"ip": address.text, "prefix": cidr}),
                        });
                        continue;
                    }
                },
                Err(err) => {
                    result.rows.push(ImportRow {
                        line,
                        status: "invalid".into(),
                        problem: Some(err.message),
                        value: serde_json::json!({"ip": address.text}),
                    });
                    continue;
                }
            }
        };
        let device_id = match row.device.trim() {
            "" => None,
            name => match devices
                .iter()
                .find(|device| device.name.eq_ignore_ascii_case(name))
            {
                Some(device) => Some(device.id.clone()),
                None => {
                    result.rows.push(ImportRow {
                        line,
                        status: "invalid".into(),
                        problem: Some(format!("no device called `{name}` is in the inventory")),
                        value: serde_json::json!({"ip": address.text, "device": name}),
                    });
                    continue;
                }
            },
        };
        let scope = prefix
            .as_ref()
            .and_then(|id| entries.iter().find(|entry| &entry.prefix.id == id))
            .map(|entry| entry.scope.clone())
            .unwrap_or_else(|| scope_of(&row.site));
        let existing = db::record(app.store(), &scope, &address.text)?;
        let value = serde_json::json!({
            "ip": address.text,
            "status": status,
            "hostname": row.hostname,
            "device": row.device,
            "interface": row.interface,
            "prefix_id": prefix,
        });
        if dry_run {
            result.rows.push(ImportRow {
                line,
                status: if existing.is_some() { "update" } else { "new" }.into(),
                problem: prefix
                    .is_none()
                    .then(|| format!("no prefix on record contains {}", address.text)),
                value,
            });
            continue;
        }
        let record = AddressRecord {
            ip: address.text.clone(),
            scope,
            prefix_id: prefix,
            status,
            hostname: row.hostname.trim().to_string(),
            description: row.description.clone(),
            device_id,
            interface: (!row.interface.trim().is_empty()).then(|| row.interface.trim().to_string()),
            mac: (!row.mac.trim().is_empty()).then(|| row.mac.trim().to_string()),
            tags: tags_of(&row.tags),
            // An import that touches a record it did not create keeps the day
            // that record was first written.
            created_at: existing
                .as_ref()
                .map(|record| record.created_at)
                .unwrap_or_else(now_ms),
            updated_at: now_ms(),
        };
        db::put_record(app.store(), &record)?;
        if existing.is_some() {
            result.updated += 1;
        } else {
            result.created += 1;
        }
        result.rows.push(ImportRow {
            line,
            status: if existing.is_some() { "update" } else { "new" }.into(),
            problem: None,
            value,
        });
    }
    Ok(())
}

fn scope_of(site: &str) -> String {
    if site.starts_with("lab:") {
        site.to_string()
    } else {
        String::new()
    }
}

fn import_vlans(app: &App, text: &str, dry_run: bool, result: &mut ImportResult) -> AppResult<()> {
    let entries = prefixes::load(app)?;
    let existing: Vec<VlanRecord> = app.store().list(db::VLANS)?;
    let mut reader = reader(text);
    for (index, record) in reader.deserialize::<VlanCsv>().enumerate() {
        let line = index + 2;
        let row = match record {
            Ok(row) => row,
            Err(err) => {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(err.to_string()),
                    value: serde_json::Value::Null,
                });
                continue;
            }
        };
        let vid = match row.vid.trim().parse::<u16>() {
            Ok(vid) if (1..=4094).contains(&vid) => vid,
            _ => {
                result.rows.push(ImportRow {
                    line,
                    status: "invalid".into(),
                    problem: Some(format!("`{}` is not a VLAN id (1 to 4094)", row.vid)),
                    value: serde_json::Value::Null,
                });
                continue;
            }
        };
        let site = (!row.site.trim().is_empty()).then(|| row.site.trim().to_string());
        let mut prefix_ids = Vec::new();
        let mut problem = None;
        for cidr in row.prefix_cidrs.split([';', '|']).map(str::trim) {
            if cidr.is_empty() {
                continue;
            }
            match entries.iter().find(|entry| entry.prefix.cidr == cidr) {
                Some(entry) => prefix_ids.push(entry.prefix.id.clone()),
                None => problem = Some(format!("no prefix `{cidr}` is on record")),
            }
        }
        let found = existing
            .iter()
            .find(|record| record.vid == vid && record.site == site);
        let value = serde_json::json!({
            "vid": vid,
            "name": row.name,
            "site": site,
            "prefix_ids": prefix_ids,
        });
        if dry_run {
            result.rows.push(ImportRow {
                line,
                status: if found.is_some() { "update" } else { "new" }.into(),
                problem,
                value,
            });
            continue;
        }
        let input = crate::vlans::VlanInput {
            id: found.map(|record| record.id.clone()),
            vid: Some(vid),
            name: Some(row.name.clone()),
            site: Some(row.site.clone()),
            description: Some(row.description.clone()),
            prefix_ids: Some(prefix_ids),
        };
        match crate::vlans::save(app, input) {
            Ok(_) => {
                if found.is_some() {
                    result.updated += 1;
                } else {
                    result.created += 1;
                }
                result.rows.push(ImportRow {
                    line,
                    status: if found.is_some() { "update" } else { "new" }.into(),
                    problem,
                    value,
                });
            }
            Err(err) => result.rows.push(ImportRow {
                line,
                status: "invalid".into(),
                problem: Some(err.message),
                value,
            }),
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ export

/// `csv::Error` is a foreign type and so is [`AppError`], so the conversion
/// cannot be a `From` impl; it is this function instead.
fn csv_error(err: csv::Error) -> AppError {
    AppError::internal(format!("the CSV could not be written: {err}"))
}

fn finish(writer: csv::Writer<Vec<u8>>) -> AppResult<String> {
    let bytes = writer
        .into_inner()
        .map_err(|err| AppError::internal(format!("the CSV could not be written: {err}")))?;
    String::from_utf8(bytes)
        .map_err(|err| AppError::internal(format!("the CSV is not valid UTF-8: {err}")))
}

pub fn export(app: &App, kind: &str, include_observed: bool) -> AppResult<String> {
    match kind {
        "prefixes" => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            writer
                .write_record([
                    "cidr",
                    "name",
                    "description",
                    "site",
                    "vlan_id",
                    "role",
                    "gateway",
                    "dhcp_start",
                    "dhcp_end",
                    "tags",
                    "source",
                ])
                .map_err(csv_error)?;
            for entry in prefixes::load(app)? {
                let prefix: Prefix = entry.prefix;
                writer
                    .write_record([
                        prefix.cidr.as_str(),
                        prefix.name.as_str(),
                        prefix.description.as_str(),
                        prefix.site.as_deref().unwrap_or_default(),
                        &prefix
                            .vlan_id
                            .map(|vid| vid.to_string())
                            .unwrap_or_default(),
                        &prefix
                            .role
                            .map(|role| {
                                serde_json::to_value(role)
                                    .ok()
                                    .and_then(|value| value.as_str().map(str::to_string))
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default(),
                        prefix.gateway.as_deref().unwrap_or_default(),
                        prefix
                            .dhcp
                            .as_ref()
                            .map(|dhcp| dhcp.start.as_str())
                            .unwrap_or_default(),
                        prefix
                            .dhcp
                            .as_ref()
                            .map(|dhcp| dhcp.end.as_str())
                            .unwrap_or_default(),
                        &prefix.tags.join(";"),
                        if prefix.source == Source::Manual {
                            "manual"
                        } else {
                            "discovered"
                        },
                    ])
                    .map_err(csv_error)?;
            }
            finish(writer)
        }
        "addresses" => {
            let entries = prefixes::load(app)?;
            let mut writer = csv::Writer::from_writer(Vec::new());
            writer
                .write_record([
                    "ip",
                    "prefix",
                    "status",
                    "hostname",
                    "description",
                    "device",
                    "interface",
                    "mac",
                    "tags",
                    "site",
                    "source",
                    "last_seen",
                ])
                .map_err(csv_error)?;
            let devices = crate::collected::devices(app)?;
            let name_of = |id: &Option<String>| -> String {
                id.as_ref()
                    .and_then(|id| devices.iter().find(|device| &device.id == id))
                    .map(|device| device.name.clone())
                    .unwrap_or_default()
            };
            for record in db::all_records(app.store())? {
                let prefix = record
                    .prefix_id
                    .as_ref()
                    .and_then(|id| entries.iter().find(|entry| &entry.prefix.id == id))
                    .map(|entry| entry.prefix.cidr.clone())
                    .unwrap_or_default();
                let site = record
                    .prefix_id
                    .as_ref()
                    .and_then(|id| entries.iter().find(|entry| &entry.prefix.id == id))
                    .and_then(|entry| entry.prefix.site.clone())
                    .unwrap_or_default();
                writer
                    .write_record([
                        record.ip.as_str(),
                        prefix.as_str(),
                        serde_json::to_value(record.status)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_string))
                            .unwrap_or_default()
                            .as_str(),
                        record.hostname.as_str(),
                        record.description.as_str(),
                        name_of(&record.device_id).as_str(),
                        record.interface.as_deref().unwrap_or_default(),
                        record.mac.as_deref().unwrap_or_default(),
                        record.tags.join(";").as_str(),
                        site.as_str(),
                        "manual",
                        "",
                    ])
                    .map_err(csv_error)?;
            }
            if include_observed {
                for observation in db::all_observations(app.store())? {
                    if db::record(app.store(), &observation.scope, &observation.ip)?.is_some() {
                        continue;
                    }
                    let address = net::addr(&observation.ip)?;
                    let entry = prefixes::containing(&entries, &observation.scope, &address);
                    writer
                        .write_record([
                            observation.ip.as_str(),
                            entry
                                .map(|entry| entry.prefix.cidr.clone())
                                .unwrap_or_default()
                                .as_str(),
                            "active",
                            "",
                            "",
                            observation.device.as_str(),
                            observation.interface.as_str(),
                            observation.mac.as_deref().unwrap_or_default(),
                            "",
                            entry
                                .and_then(|entry| entry.prefix.site.clone())
                                .unwrap_or_default()
                                .as_str(),
                            match observation.kind {
                                ObservationKind::Interface => "interface",
                                ObservationKind::Arp => "arp",
                            },
                            observation.last_seen.to_string().as_str(),
                        ])
                        .map_err(csv_error)?;
                }
            }
            finish(writer)
        }
        "vlans" => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            writer
                .write_record(["vid", "name", "site", "description", "prefixes", "source"])
                .map_err(csv_error)?;
            for row in crate::vlans::rows(app, None, None)? {
                if !row.documented {
                    continue;
                }
                writer
                    .write_record([
                        row.vlan.vid.to_string().as_str(),
                        row.vlan.name.as_str(),
                        row.vlan.site.as_deref().unwrap_or_default(),
                        row.vlan.description.as_str(),
                        row.prefixes
                            .iter()
                            .map(|prefix| prefix.cidr.clone())
                            .collect::<Vec<_>>()
                            .join(";")
                            .as_str(),
                        if row.vlan.source == Source::Manual {
                            "manual"
                        } else {
                            "discovered"
                        },
                    ])
                    .map_err(csv_error)?;
            }
            finish(writer)
        }
        other => Err(AppError::invalid(format!(
            "`{other}` cannot be exported (prefixes, addresses, vlans)"
        ))),
    }
}
