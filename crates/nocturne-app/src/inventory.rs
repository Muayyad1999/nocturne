//! The device inventory.
//!
//! Devices are added by hand, by discovery, from a lab, or imported from what
//! teams already have: a CSV sheet, an OpenSSH client config, or an Ansible
//! inventory. Whatever the source, they pass through the same validation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::{App, AppBuilder, NoParams};
use crate::error::{AppError, AppResult};
use crate::model::collections::{CREDENTIALS, DEVICE_STATE, DEVICES};
use crate::model::{Credential, Device, DeviceRow, DeviceState, LabRef, MonitorSpec, Transport};
use crate::util::{matches_query, new_id, now_ms};

/// Fields the UI may set on a device. Absent = leave unchanged (on update) or
/// default (on create).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceInput {
    pub name: Option<String>,
    pub address: Option<String>,
    pub profile: Option<String>,
    pub transport: Option<Transport>,
    /// `0` clears the port back to the transport's default.
    pub port: Option<u32>,
    /// Empty string clears the credential.
    pub credential_id: Option<String>,
    pub jump_device_id: Option<String>,
    pub site: Option<String>,
    pub role: Option<String>,
    pub tags: Option<Vec<String>>,
    pub notes: Option<String>,
    pub monitor: Option<MonitorSpec>,
    pub lab: Option<LabRef>,
    pub protected: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DeviceFilter {
    /// Free text matched against name, address, site, role, tags, profile, model and serial.
    pub query: Option<String>,
    pub site: Option<String>,
    pub role: Option<String>,
    pub tag: Option<String>,
    pub profile: Option<String>,
    /// `up`, `down` or `unknown`.
    pub status: Option<String>,
    /// `true` only lab devices, `false` only real ones.
    pub lab: Option<bool>,
}

pub fn state_of(app: &App, device_id: &str) -> AppResult<DeviceState> {
    Ok(app
        .store()
        .get::<DeviceState>(DEVICE_STATE, device_id)?
        .unwrap_or_else(|| DeviceState {
            device_id: device_id.to_string(),
            ..DeviceState::default()
        }))
}

/// Change a device's state record and tell the UI.
pub fn update_state(
    app: &App,
    device_id: &str,
    change: impl FnOnce(&mut DeviceState),
) -> AppResult<DeviceState> {
    let mut state = state_of(app, device_id)?;
    change(&mut state);
    app.store().put(DEVICE_STATE, device_id, &state)?;
    app.publish("devices.state", &state);
    Ok(state)
}

pub fn get(app: &App, id: &str) -> AppResult<Device> {
    app.store().require(DEVICES, id, "device")
}

/// Find a device by id, or failing that by name (case-insensitive) or address.
pub fn find(app: &App, reference: &str) -> AppResult<Device> {
    if let Some(device) = app.store().get::<Device>(DEVICES, reference)? {
        return Ok(device);
    }
    let wanted = reference.trim();
    let devices: Vec<Device> = app.store().list(DEVICES)?;
    devices
        .iter()
        .find(|device| device.name.eq_ignore_ascii_case(wanted))
        .or_else(|| {
            devices
                .iter()
                .find(|device| device.address.eq_ignore_ascii_case(wanted))
        })
        .cloned()
        .ok_or_else(|| AppError::not_found(format!("device `{reference}`")))
}

pub fn row(app: &App, device: Device) -> AppResult<DeviceRow> {
    let state = state_of(app, &device.id)?;
    Ok(DeviceRow { device, state })
}

pub fn list(app: &App, filter: &DeviceFilter) -> AppResult<Vec<DeviceRow>> {
    let states: BTreeMap<String, DeviceState> = app
        .store()
        .list::<DeviceState>(DEVICE_STATE)?
        .into_iter()
        .map(|state| (state.device_id.clone(), state))
        .collect();
    let mut rows: Vec<DeviceRow> = app
        .store()
        .list::<Device>(DEVICES)?
        .into_iter()
        .map(|device| {
            let state = states
                .get(&device.id)
                .cloned()
                .unwrap_or_else(|| DeviceState {
                    device_id: device.id.clone(),
                    ..DeviceState::default()
                });
            DeviceRow { device, state }
        })
        .filter(|row| matches(row, filter))
        .collect();
    rows.sort_by_key(|a| natural_key(&a.device.name));
    Ok(rows)
}

fn matches(row: &DeviceRow, filter: &DeviceFilter) -> bool {
    let device = &row.device;
    let same = |wanted: &Option<String>, actual: &str| {
        wanted
            .as_deref()
            .is_none_or(|wanted| wanted.eq_ignore_ascii_case(actual))
    };
    if !same(&filter.site, &device.site)
        || !same(&filter.role, &device.role)
        || !same(&filter.profile, &device.profile)
    {
        return false;
    }
    if let Some(tag) = &filter.tag
        && !device
            .tags
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(tag))
    {
        return false;
    }
    if let Some(lab) = filter.lab
        && device.lab.is_some() != lab
    {
        return false;
    }
    if let Some(status) = &filter.status {
        let actual = match row.state.reachable {
            Some(true) => "up",
            Some(false) => "down",
            None => "unknown",
        };
        if !status.eq_ignore_ascii_case(actual) {
            return false;
        }
    }
    match filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        None => true,
        Some(query) => {
            let facts = row.state.facts.as_ref();
            let haystack = format!(
                "{} {} {} {} {} {} {} {} {}",
                device.name,
                device.address,
                device.site,
                device.role,
                device.tags.join(" "),
                device.profile,
                device.notes,
                facts
                    .map(|facts| format!("{} {} {}", facts.model, facts.serial, facts.os_version))
                    .unwrap_or_default(),
                facts
                    .map(|facts| facts.hostname.as_str())
                    .unwrap_or_default(),
            );
            matches_query(&haystack, query)
        }
    }
}

/// Sort key that puts `sw2` before `sw10`.
fn natural_key(name: &str) -> Vec<(String, u64)> {
    let mut key = Vec::new();
    let mut text = String::new();
    let mut digits = String::new();
    for ch in name.to_lowercase().chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            if !digits.is_empty() {
                key.push((
                    std::mem::take(&mut text),
                    digits.parse().unwrap_or(u64::MAX),
                ));
                digits.clear();
            }
            text.push(ch);
        }
    }
    key.push((text, digits.parse().unwrap_or(0)));
    key
}

pub fn create(app: &App, input: DeviceInput) -> AppResult<DeviceRow> {
    let now = now_ms();
    let device = Device {
        id: new_id(),
        created_at: now,
        ..Device::default()
    };
    let device = save(app, device, input, None)?;
    app.audit(
        "device.create",
        Some(&device.id),
        format!("Added device `{}`", device.name),
        json!({"address": device.address}),
    );
    app.publish(
        "devices.changed",
        &json!({"ids": [device.id], "change": "created"}),
    );
    row(app, device)
}

pub fn update(app: &App, id: &str, input: DeviceInput) -> AppResult<DeviceRow> {
    let device = get(app, id)?;
    let device = save(app, device, input, None)?;
    app.audit(
        "device.update",
        Some(&device.id),
        format!("Updated device `{}`", device.name),
        serde_json::Value::Null,
    );
    app.publish(
        "devices.changed",
        &json!({"ids": [device.id], "change": "updated"}),
    );
    row(app, device)
}

/// Validate and store. `others` lets bulk operations pass the device list once.
fn save(
    app: &App,
    mut device: Device,
    input: DeviceInput,
    others: Option<&[Device]>,
) -> AppResult<Device> {
    if let Some(name) = input.name {
        device.name = name.trim().to_string();
    }
    if let Some(address) = input.address {
        device.address = address.trim().to_string();
    }
    if let Some(profile) = input.profile {
        device.profile = profile.trim().to_string();
    }
    if let Some(transport) = input.transport {
        device.transport = transport;
    }
    if let Some(port) = input.port {
        device.port = (port != 0).then_some(port);
    }
    if let Some(credential) = input.credential_id {
        device.credential_id = Some(credential).filter(|id| !id.is_empty());
    }
    if let Some(jump) = input.jump_device_id {
        device.jump_device_id = Some(jump).filter(|id| !id.is_empty());
    }
    if let Some(site) = input.site {
        device.site = site.trim().to_string();
    }
    if let Some(role) = input.role {
        device.role = role.trim().to_lowercase();
    }
    if let Some(tags) = input.tags {
        device.tags = clean_tags(tags);
    }
    if let Some(notes) = input.notes {
        device.notes = notes;
    }
    if let Some(monitor) = input.monitor {
        device.monitor = monitor;
    }
    if let Some(lab) = input.lab {
        device.lab = Some(lab).filter(|lab| !lab.lab_id.is_empty());
    }
    if let Some(protected) = input.protected {
        device.protected = protected;
    }

    if device.name.is_empty() {
        // A name is a convenience; fall back to the address rather than refusing.
        device.name = device.address.clone();
    }
    if device.name.is_empty() {
        return Err(AppError::invalid("a device needs a name or an address"));
    }
    if device.lab.is_some() {
        device.transport = Transport::Sim;
    }
    if device.address.is_empty() && !matches!(device.transport, Transport::Sim) {
        return Err(AppError::invalid(format!(
            "`{}` needs an address (hostname or IP)",
            device.name
        )));
    }
    if device.address.chars().any(char::is_whitespace) {
        return Err(AppError::invalid(format!(
            "the address `{}` contains spaces",
            device.address
        )));
    }
    if let Some(port) = device.port {
        let is_serial = device.transport == Transport::Serial;
        if !is_serial && port > 65_535 {
            return Err(AppError::invalid(format!(
                "{port} is not a valid TCP port (1–65535)"
            )));
        }
    }
    device.monitor.interval_s = device.monitor.interval_s.clamp(5, 86_400);

    let loaded;
    let others = match others {
        Some(others) => others,
        None => {
            loaded = app.store().list::<Device>(DEVICES)?;
            &loaded
        }
    };
    if others
        .iter()
        .any(|other| other.id != device.id && other.name.eq_ignore_ascii_case(&device.name))
    {
        return Err(AppError::conflict(format!(
            "a device named `{}` already exists",
            device.name
        )));
    }
    if let Some(credential) = &device.credential_id
        && app
            .store()
            .get::<Credential>(CREDENTIALS, credential)?
            .is_none()
    {
        return Err(AppError::invalid(
            "the selected credential no longer exists",
        ));
    }
    if let Some(jump) = &device.jump_device_id {
        if jump == &device.id {
            return Err(AppError::invalid("a device cannot be its own jump host"));
        }
        // Follow the chain to refuse loops and absurd depths.
        let mut hop = Some(jump.clone());
        let mut depth = 0;
        while let Some(current) = hop {
            depth += 1;
            if current == device.id || depth > 5 {
                return Err(AppError::invalid(
                    "the jump-host chain loops or is more than five hops long",
                ));
            }
            hop = others
                .iter()
                .find(|other| other.id == current)
                .ok_or_else(|| AppError::invalid("the selected jump host no longer exists"))?
                .jump_device_id
                .clone();
        }
    }

    device.updated_at = now_ms();
    app.store().put(DEVICES, &device.id, &device)?;
    Ok(device)
}

fn clean_tags(tags: Vec<String>) -> Vec<String> {
    let mut cleaned: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim().trim_start_matches('#').to_string();
        if !tag.is_empty()
            && !cleaned
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&tag))
        {
            cleaned.push(tag);
        }
    }
    cleaned
}

/// Delete devices and everything recorded about them.
pub fn delete(app: &App, ids: &[String]) -> AppResult<usize> {
    let devices: Vec<Device> = app.store().list(DEVICES)?;
    let mut removed = Vec::new();
    for id in ids {
        let Some(device) = devices.iter().find(|device| &device.id == id) else {
            continue;
        };
        if let Some(dependant) = devices
            .iter()
            .find(|other| other.jump_device_id.as_deref() == Some(id) && !ids.contains(&other.id))
        {
            return Err(AppError::conflict(format!(
                "`{}` is the jump host for `{}`; change that device first",
                device.name, dependant.name
            )));
        }
        removed.push(device.clone());
    }
    for device in &removed {
        app.store().delete(DEVICES, &device.id)?;
        app.store().delete(DEVICE_STATE, &device.id)?;
        app.store().samples_forget_device(&device.id)?;
        app.store().config_delete_device(&device.id)?;
        crate::collect::forget(app, &device.id)?;
        app.audit(
            "device.delete",
            Some(&device.id),
            format!("Deleted device `{}`", device.name),
            serde_json::Value::Null,
        );
    }
    if !removed.is_empty() {
        let ids: Vec<&str> = removed.iter().map(|device| device.id.as_str()).collect();
        app.publish("devices.changed", &json!({"ids": ids, "change": "deleted"}));
    }
    Ok(removed.len())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct BulkPatch {
    pub site: Option<String>,
    pub role: Option<String>,
    pub profile: Option<String>,
    pub credential_id: Option<String>,
    pub protected: Option<bool>,
    pub monitor_enabled: Option<bool>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
}

pub fn bulk_update(app: &App, ids: &[String], patch: BulkPatch) -> AppResult<usize> {
    let devices: Vec<Device> = app.store().list(DEVICES)?;
    let mut changed = Vec::new();
    for id in ids {
        let Some(device) = devices.iter().find(|device| &device.id == id).cloned() else {
            continue;
        };
        let mut tags = device.tags.clone();
        tags.retain(|tag| {
            !patch
                .remove_tags
                .iter()
                .any(|removed| removed.eq_ignore_ascii_case(tag))
        });
        tags.extend(patch.add_tags.iter().cloned());
        let monitor = patch.monitor_enabled.map(|enabled| MonitorSpec {
            enabled,
            ..device.monitor.clone()
        });
        let input = DeviceInput {
            site: patch.site.clone(),
            role: patch.role.clone(),
            profile: patch.profile.clone(),
            credential_id: patch.credential_id.clone(),
            protected: patch.protected,
            monitor,
            tags: Some(tags),
            ..DeviceInput::default()
        };
        save(app, device, input, Some(&devices))?;
        changed.push(id.clone());
    }
    if !changed.is_empty() {
        app.audit(
            "device.bulk_update",
            None,
            format!("Updated {} devices", changed.len()),
            json!({"ids": changed}),
        );
        app.publish(
            "devices.changed",
            &json!({"ids": changed, "change": "updated"}),
        );
    }
    Ok(changed.len())
}

/// Counts per site, role, tag and profile, for filter menus.
#[derive(Debug, Default, Serialize)]
pub struct Facets {
    pub total: usize,
    pub lab: usize,
    pub sites: Vec<(String, usize)>,
    pub roles: Vec<(String, usize)>,
    pub tags: Vec<(String, usize)>,
    pub profiles: Vec<(String, usize)>,
}

pub fn facets(app: &App) -> AppResult<Facets> {
    let devices: Vec<Device> = app.store().list(DEVICES)?;
    let mut sites = BTreeMap::new();
    let mut roles = BTreeMap::new();
    let mut tags = BTreeMap::new();
    let mut profiles = BTreeMap::new();
    for device in &devices {
        for (map, value) in [
            (&mut sites, &device.site),
            (&mut roles, &device.role),
            (&mut profiles, &device.profile),
        ] {
            if !value.is_empty() {
                *map.entry(value.clone()).or_insert(0usize) += 1;
            }
        }
        for tag in &device.tags {
            *tags.entry(tag.clone()).or_insert(0usize) += 1;
        }
    }
    Ok(Facets {
        total: devices.len(),
        lab: devices.iter().filter(|device| device.lab.is_some()).count(),
        sites: sites.into_iter().collect(),
        roles: roles.into_iter().collect(),
        tags: tags.into_iter().collect(),
        profiles: profiles.into_iter().collect(),
    })
}

// ------------------------------------------------------------------- import

/// One device parsed from an import, before it is committed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportRow {
    pub input: DeviceInput,
    /// Name of the credential to attach, resolved at commit time.
    #[serde(default)]
    pub credential_name: Option<String>,
    /// `new`, `update` (a device with this name exists) or `invalid`.
    pub status: String,
    #[serde(default)]
    pub problem: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct ImportPreview {
    pub format: String,
    pub rows: Vec<ImportRow>,
    pub new: usize,
    pub update: usize,
    pub invalid: usize,
}

/// Parse an import without changing anything.
pub fn import_preview(app: &App, format: &str, text: &str) -> AppResult<ImportPreview> {
    let format = match format.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => detect_format(text),
        other => other.to_string(),
    };
    let mut rows = match format.as_str() {
        "csv" => parse_csv(text)?,
        "ssh_config" => parse_ssh_config(text),
        "ansible" => parse_ansible(text)?,
        other => {
            return Err(AppError::invalid(format!(
                "unknown import format `{other}` (csv, ssh_config or ansible)"
            )));
        }
    };
    let existing: Vec<Device> = app.store().list(DEVICES)?;
    let mut seen: Vec<String> = Vec::new();
    for row in &mut rows {
        let name = row
            .input
            .name
            .clone()
            .or_else(|| row.input.address.clone())
            .unwrap_or_default();
        let address = row.input.address.clone().unwrap_or_default();
        if name.trim().is_empty() {
            row.status = "invalid".into();
            row.problem = Some("no name or address".into());
        } else if address.trim().is_empty() && row.input.transport != Some(Transport::Sim) {
            row.status = "invalid".into();
            row.problem = Some("no address".into());
        } else if seen
            .iter()
            .any(|earlier| earlier.eq_ignore_ascii_case(&name))
        {
            row.status = "invalid".into();
            row.problem = Some("appears more than once in this import".into());
        } else {
            row.status = if existing
                .iter()
                .any(|device| device.name.eq_ignore_ascii_case(&name))
            {
                "update"
            } else {
                "new"
            }
            .into();
            seen.push(name);
        }
    }
    let count = |status: &str| rows.iter().filter(|row| row.status == status).count();
    Ok(ImportPreview {
        new: count("new"),
        update: count("update"),
        invalid: count("invalid"),
        format,
        rows,
    })
}

#[derive(Debug, Default, Serialize)]
pub struct ImportResult {
    pub created: usize,
    pub updated: usize,
    pub skipped: Vec<String>,
}

/// Commit an import: creates new devices and (optionally) updates existing ones by name.
pub fn import_apply(
    app: &App,
    format: &str,
    text: &str,
    update_existing: bool,
) -> AppResult<ImportResult> {
    let preview = import_preview(app, format, text)?;
    let credentials: Vec<Credential> = app.store().list(CREDENTIALS)?;
    let mut result = ImportResult::default();
    let mut touched = Vec::new();
    for row in preview.rows {
        let label = row
            .input
            .name
            .clone()
            .or_else(|| row.input.address.clone())
            .unwrap_or_else(|| "(unnamed)".into());
        if row.status == "invalid" {
            result
                .skipped
                .push(format!("{label}: {}", row.problem.unwrap_or_default()));
            continue;
        }
        let mut input = row.input;
        if let Some(name) = &row.credential_name {
            match credentials
                .iter()
                .find(|credential| credential.name.eq_ignore_ascii_case(name))
            {
                Some(credential) => input.credential_id = Some(credential.id.clone()),
                None => result.skipped.push(format!(
                    "{label}: credential `{name}` not found (imported without one)"
                )),
            }
        }
        let devices: Vec<Device> = app.store().list(DEVICES)?;
        let existing = devices
            .iter()
            .find(|device| device.name.eq_ignore_ascii_case(&label))
            .cloned();
        let outcome = match existing {
            Some(_) if !update_existing => {
                result.skipped.push(format!("{label}: already exists"));
                continue;
            }
            Some(device) => save(app, device, input, Some(&devices)).map(|device| (device, false)),
            None => save(
                app,
                Device {
                    id: new_id(),
                    created_at: now_ms(),
                    ..Device::default()
                },
                input,
                Some(&devices),
            )
            .map(|device| (device, true)),
        };
        match outcome {
            Ok((device, created)) => {
                if created {
                    result.created += 1;
                } else {
                    result.updated += 1;
                }
                touched.push(device.id);
            }
            Err(err) => result.skipped.push(format!("{label}: {}", err.message)),
        }
    }
    if !touched.is_empty() {
        app.audit(
            "device.import",
            None,
            format!(
                "Imported {} devices ({} new, {} updated)",
                touched.len(),
                result.created,
                result.updated
            ),
            json!({"format": preview.format}),
        );
        app.publish(
            "devices.changed",
            &json!({"ids": touched, "change": "imported"}),
        );
    }
    Ok(result)
}

fn detect_format(text: &str) -> String {
    let trimmed = text.trim_start();
    let first = trimmed
        .lines()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if trimmed
        .lines()
        .any(|line| line.trim_start().to_ascii_lowercase().starts_with("host "))
        && !first.contains(',')
    {
        "ssh_config".into()
    } else if first.starts_with('[')
        || first.starts_with("all:")
        || first.starts_with("---")
        || trimmed.contains("ansible_host")
        || trimmed.contains("hosts:")
    {
        "ansible".into()
    } else {
        "csv".into()
    }
}

fn parse_transport(text: &str) -> Option<Transport> {
    match text.trim().to_ascii_lowercase().as_str() {
        "ssh" | "ssh2" | "network_cli" | "ansible.netcommon.network_cli" => Some(Transport::Ssh),
        "telnet" => Some(Transport::Telnet),
        "serial" | "console" => Some(Transport::Serial),
        "none" | "monitor" | "snmp" | "icmp" => Some(Transport::None),
        _ => None,
    }
}

fn parse_csv(text: &str) -> AppResult<Vec<ImportRow>> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());
    let headers: Vec<String> = reader
        .headers()
        .map_err(|err| AppError::invalid(format!("the CSV header could not be read: {err}")))?
        .iter()
        .map(|header| {
            header
                .trim()
                .trim_start_matches('\u{feff}')
                .to_ascii_lowercase()
                .replace([' ', '-'], "_")
        })
        .collect();
    let column = |names: &[&str]| {
        headers
            .iter()
            .position(|header| names.contains(&header.as_str()))
    };
    let name = column(&["name", "hostname", "device", "device_name"]);
    let address = column(&[
        "address",
        "host",
        "ip",
        "ip_address",
        "mgmt_ip",
        "management_ip",
        "fqdn",
    ]);
    if name.is_none() && address.is_none() {
        return Err(AppError::invalid(
            "the CSV needs a header row with at least a `name` or `address` column (others: profile, transport, port, site, role, tags, credential, notes)",
        ));
    }
    let profile = column(&["profile", "platform", "type", "device_type", "os"]);
    let transport = column(&["transport", "protocol", "method"]);
    let port = column(&["port"]);
    let site = column(&["site", "location"]);
    let role = column(&["role"]);
    let tags = column(&["tags", "groups", "labels"]);
    let credential = column(&["credential", "credentials", "login"]);
    let notes = column(&["notes", "description", "comment"]);

    let mut rows = Vec::new();
    for record in reader.records() {
        let record =
            record.map_err(|err| AppError::invalid(format!("the CSV could not be read: {err}")))?;
        let cell = |index: Option<usize>| {
            index
                .and_then(|index| record.get(index))
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
        };
        if record.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        rows.push(ImportRow {
            input: DeviceInput {
                name: cell(name).map(str::to_string),
                address: cell(address).map(str::to_string),
                profile: cell(profile).map(profile_from_alias),
                transport: cell(transport).and_then(parse_transport),
                port: cell(port).and_then(|value| value.parse().ok()),
                site: cell(site).map(str::to_string),
                role: cell(role).map(str::to_string),
                tags: cell(tags)
                    .map(|value| value.split([';', ',', '|']).map(str::to_string).collect()),
                notes: cell(notes).map(str::to_string),
                ..DeviceInput::default()
            },
            credential_name: cell(credential).map(str::to_string),
            status: String::new(),
            problem: None,
        });
    }
    Ok(rows)
}

/// OpenSSH client configuration: one device per concrete `Host` entry.
fn parse_ssh_config(text: &str) -> Vec<ImportRow> {
    let mut rows: Vec<ImportRow> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (keyword, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((keyword, value)) => (
                keyword.to_ascii_lowercase(),
                value
                    .trim()
                    .trim_start_matches('=')
                    .trim()
                    .trim_matches('"'),
            ),
            None => continue,
        };
        match keyword.as_str() {
            "host" => {
                current.clear();
                for alias in value
                    .split_whitespace()
                    .filter(|alias| !alias.contains(['*', '?', '!']))
                {
                    current.push(rows.len());
                    rows.push(ImportRow {
                        input: DeviceInput {
                            name: Some(alias.to_string()),
                            address: Some(alias.to_string()),
                            transport: Some(Transport::Ssh),
                            ..DeviceInput::default()
                        },
                        credential_name: None,
                        status: String::new(),
                        problem: None,
                    });
                }
            }
            "match" => current.clear(),
            "hostname" => {
                for index in &current {
                    rows[*index].input.address = Some(value.to_string());
                }
            }
            "port" => {
                for index in &current {
                    rows[*index].input.port = value.parse().ok();
                }
            }
            "user" => {
                for index in &current {
                    rows[*index].input.notes =
                        Some(format!("SSH user in the imported config: {value}"));
                }
            }
            _ => {}
        }
    }
    rows
}

/// `ansible_network_os` (and friends) ↔ Nocturne profile ids.
const ANSIBLE_OS: &[(&str, &str)] = &[
    ("cisco.ios.ios", "cisco_ios"),
    ("cisco.nxos.nxos", "cisco_nxos"),
    ("cisco.iosxr.iosxr", "cisco_xr"),
    ("cisco.asa.asa", "cisco_asa"),
    ("arista.eos.eos", "arista_eos"),
    ("junipernetworks.junos.junos", "juniper_junos"),
    ("community.routeros.routeros", "mikrotik_routeros"),
    ("vyos.vyos.vyos", "vyos"),
    ("community.network.ce", "huawei_vrp"),
    ("community.network.comware", "hp_comware"),
    ("arubanetworks.aoscx.aoscx", "aruba_aoscx"),
    ("fortinet.fortios.fortios", "fortinet"),
    ("paloaltonetworks.panos.panos", "paloalto_panos"),
    ("dellemc.os10.os10", "dell_os10"),
    ("community.network.edgeos", "ubiquiti_edgerouter"),
    ("community.network.exos", "extreme_exos"),
];

/// Accept an Ansible network OS, a Netmiko device type or a profile id.
fn profile_from_alias(value: &str) -> String {
    let lowered = value.trim().to_ascii_lowercase();
    if let Some((_, profile)) = ANSIBLE_OS
        .iter()
        .find(|(os, _)| *os == lowered || os.rsplit('.').next() == Some(lowered.as_str()))
    {
        return (*profile).to_string();
    }
    match lowered.as_str() {
        "ios" | "cisco" | "cisco_xe" | "iosxe" | "ios-xe" => "cisco_ios".into(),
        "nxos" | "nx-os" => "cisco_nxos".into(),
        "iosxr" | "ios-xr" | "cisco_xr" => "cisco_xr".into(),
        "asa" => "cisco_asa".into(),
        "eos" | "arista" => "arista_eos".into(),
        "junos" | "juniper" => "juniper_junos".into(),
        "routeros" | "mikrotik" | "ros" => "mikrotik_routeros".into(),
        "vrp" | "huawei" => "huawei_vrp".into(),
        "comware" | "h3c" => "hp_comware".into(),
        "procurve" | "aruba_os" | "aruba_osswitch" => "hp_procurve".into(),
        "aoscx" | "aos-cx" => "aruba_aoscx".into(),
        "fortios" | "fortigate" => "fortinet".into(),
        "panos" | "pan-os" | "paloalto" => "paloalto_panos".into(),
        "auto" | "autodetect" => String::new(),
        _ => lowered,
    }
}

fn ansible_os_for(profile: &str) -> Option<&'static str> {
    ANSIBLE_OS
        .iter()
        .find(|(_, candidate)| *candidate == profile)
        .map(|(os, _)| *os)
}

#[derive(Default, Clone)]
struct AnsibleHost {
    vars: BTreeMap<String, String>,
    groups: Vec<String>,
}

fn parse_ansible(text: &str) -> AppResult<Vec<ImportRow>> {
    let trimmed = text.trim_start();
    let looks_yaml = trimmed.starts_with("---")
        || trimmed.lines().next().is_some_and(|line| {
            line.trim_end().ends_with(':') && !line.trim_start().starts_with('[')
        });
    let (hosts, group_vars) = if looks_yaml {
        parse_ansible_yaml(text)?
    } else {
        parse_ansible_ini(text)
    };

    let mut rows = Vec::new();
    for (name, host) in hosts {
        // Host variables win over group variables; later groups over earlier ones.
        let lookup = |key: &str| -> Option<String> {
            host.vars.get(key).cloned().or_else(|| {
                host.groups
                    .iter()
                    .rev()
                    .chain(std::iter::once(&"all".to_string()))
                    .find_map(|group| {
                        group_vars
                            .get(group)
                            .and_then(|vars| vars.get(key))
                            .cloned()
                    })
            })
        };
        let connection = lookup("ansible_connection");
        let tags: Vec<String> = host
            .groups
            .iter()
            .filter(|group| *group != "all" && *group != "ungrouped")
            .cloned()
            .collect();
        rows.push(ImportRow {
            input: DeviceInput {
                address: Some(lookup("ansible_host").unwrap_or_else(|| name.clone())),
                name: Some(name),
                profile: lookup("ansible_network_os").map(|os| profile_from_alias(&os)),
                transport: connection
                    .as_deref()
                    .and_then(parse_transport)
                    .or(Some(Transport::Ssh)),
                port: lookup("ansible_port").and_then(|port| port.parse().ok()),
                tags: Some(tags),
                notes: lookup("ansible_user")
                    .map(|user| format!("Ansible user in the imported inventory: {user}")),
                ..DeviceInput::default()
            },
            credential_name: None,
            status: String::new(),
            problem: None,
        });
    }
    Ok(rows)
}

type GroupVars = BTreeMap<String, BTreeMap<String, String>>;

fn parse_ansible_ini(text: &str) -> (Vec<(String, AnsibleHost)>, GroupVars) {
    let mut hosts: Vec<(String, AnsibleHost)> = Vec::new();
    let mut group_vars: GroupVars = BTreeMap::new();
    let mut children: Vec<(String, String)> = Vec::new();
    let mut section = ("ungrouped".to_string(), "hosts");

    for line in text.lines() {
        let line = line.split(['#', ';']).next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            section = match header.split_once(':') {
                Some((group, "vars")) => (group.to_string(), "vars"),
                Some((group, "children")) => (group.to_string(), "children"),
                _ => (header.to_string(), "hosts"),
            };
            continue;
        }
        match section.1 {
            "vars" => {
                if let Some((key, value)) = line.split_once('=') {
                    group_vars
                        .entry(section.0.clone())
                        .or_default()
                        .insert(key.trim().to_string(), unquote(value));
                }
            }
            "children" => children.push((section.0.clone(), line.to_string())),
            _ => {
                let mut words = line.split_whitespace();
                let Some(name) = words.next() else { continue };
                let vars: BTreeMap<String, String> = words
                    .filter_map(|word| word.split_once('='))
                    .map(|(key, value)| (key.to_string(), unquote(value)))
                    .collect();
                match hosts.iter_mut().find(|(existing, _)| existing == name) {
                    Some((_, host)) => {
                        host.vars.extend(vars);
                        host.groups.push(section.0.clone());
                    }
                    None => hosts.push((
                        name.to_string(),
                        AnsibleHost {
                            vars,
                            groups: vec![section.0.clone()],
                        },
                    )),
                }
            }
        }
    }
    // A host in a child group is also in the parent group.
    for _ in 0..8 {
        for (parent, child) in &children {
            for (_, host) in &mut hosts {
                if host.groups.contains(child) && !host.groups.contains(parent) {
                    host.groups.insert(0, parent.clone());
                }
            }
        }
    }
    (hosts, group_vars)
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

fn parse_ansible_yaml(text: &str) -> AppResult<(Vec<(String, AnsibleHost)>, GroupVars)> {
    let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)
        .map_err(|err| AppError::invalid(format!("the inventory is not valid YAML: {err}")))?;
    let mut hosts: Vec<(String, AnsibleHost)> = Vec::new();
    let mut group_vars: GroupVars = BTreeMap::new();

    fn scalar(value: &serde_yaml_ng::Value) -> Option<String> {
        match value {
            serde_yaml_ng::Value::String(text) => Some(text.clone()),
            serde_yaml_ng::Value::Number(number) => Some(number.to_string()),
            serde_yaml_ng::Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        }
    }

    fn walk(
        name: &str,
        group: &serde_yaml_ng::Value,
        lineage: &[String],
        hosts: &mut Vec<(String, AnsibleHost)>,
        group_vars: &mut GroupVars,
        depth: usize,
    ) {
        if depth > 16 {
            return;
        }
        let mut lineage = lineage.to_vec();
        lineage.push(name.to_string());
        if let Some(vars) = group.get("vars").and_then(serde_yaml_ng::Value::as_mapping) {
            let entry = group_vars.entry(name.to_string()).or_default();
            for (key, value) in vars {
                if let (Some(key), Some(value)) = (key.as_str(), scalar(value)) {
                    entry.insert(key.to_string(), value);
                }
            }
        }
        if let Some(members) = group
            .get("hosts")
            .and_then(serde_yaml_ng::Value::as_mapping)
        {
            for (host_name, host_vars) in members {
                let Some(host_name) = host_name.as_str() else {
                    continue;
                };
                let vars: BTreeMap<String, String> = host_vars
                    .as_mapping()
                    .map(|mapping| {
                        mapping
                            .iter()
                            .filter_map(|(key, value)| {
                                Some((key.as_str()?.to_string(), scalar(value)?))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                match hosts.iter_mut().find(|(existing, _)| existing == host_name) {
                    Some((_, host)) => {
                        host.vars.extend(vars);
                        for group in &lineage {
                            if !host.groups.contains(group) {
                                host.groups.push(group.clone());
                            }
                        }
                    }
                    None => hosts.push((
                        host_name.to_string(),
                        AnsibleHost {
                            vars,
                            groups: lineage.clone(),
                        },
                    )),
                }
            }
        }
        if let Some(children) = group
            .get("children")
            .and_then(serde_yaml_ng::Value::as_mapping)
        {
            for (child_name, child) in children {
                if let Some(child_name) = child_name.as_str() {
                    walk(child_name, child, &lineage, hosts, group_vars, depth + 1);
                }
            }
        }
    }

    let Some(top) = document.as_mapping() else {
        return Err(AppError::invalid(
            "the inventory YAML must be a mapping of groups",
        ));
    };
    for (name, group) in top {
        if let Some(name) = name.as_str() {
            walk(name, group, &[], &mut hosts, &mut group_vars, 0);
        }
    }
    Ok((hosts, group_vars))
}

// ------------------------------------------------------------------- export

pub fn export(app: &App, format: &str, ids: Option<&[String]>) -> AppResult<String> {
    let mut devices: Vec<Device> = app.store().list(DEVICES)?;
    if let Some(ids) = ids {
        devices.retain(|device| ids.contains(&device.id));
    }
    devices.retain(|device| device.lab.is_none());
    devices.sort_by_key(|a| natural_key(&a.name));
    let credentials: Vec<Credential> = app.store().list(CREDENTIALS)?;

    match format.trim().to_ascii_lowercase().as_str() {
        "csv" => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            let io = |err: csv::Error| AppError::internal(format!("could not write CSV: {err}"));
            writer
                .write_record([
                    "name",
                    "address",
                    "profile",
                    "transport",
                    "port",
                    "site",
                    "role",
                    "tags",
                    "credential",
                    "notes",
                ])
                .map_err(io)?;
            for device in &devices {
                let transport = serde_json::to_value(device.transport)?
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                writer
                    .write_record([
                        device.name.as_str(),
                        device.address.as_str(),
                        device.profile.as_str(),
                        transport.as_str(),
                        &device.port.map(|port| port.to_string()).unwrap_or_default(),
                        device.site.as_str(),
                        device.role.as_str(),
                        &device.tags.join(";"),
                        credential_of(&credentials, device)
                            .map(|credential| credential.name.as_str())
                            .unwrap_or_default(),
                        device.notes.as_str(),
                    ])
                    .map_err(io)?;
            }
            String::from_utf8(
                writer
                    .into_inner()
                    .map_err(|err| AppError::internal(err.to_string()))?,
            )
            .map_err(|err| AppError::internal(err.to_string()))
        }
        "ansible" => Ok(export_ansible(&devices, &credentials)),
        other => Err(AppError::invalid(format!(
            "unknown export format `{other}` (csv or ansible)"
        ))),
    }
}

fn credential_of<'a>(credentials: &'a [Credential], device: &Device) -> Option<&'a Credential> {
    let id = device.credential_id.as_ref()?;
    credentials.iter().find(|credential| &credential.id == id)
}

/// An Ansible YAML inventory: one group per site and per role, passwords left out on purpose.
fn export_ansible(devices: &[Device], credentials: &[Credential]) -> String {
    let group_name = |prefix: &str, value: &str| -> String {
        let cleaned: String = value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();
        format!("{prefix}_{}", cleaned.trim_matches('_'))
    };
    let quote = |value: &str| format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    let mut groups: BTreeMap<String, Vec<&Device>> = BTreeMap::new();
    for device in devices {
        let mut placed = false;
        if !device.site.is_empty() {
            groups
                .entry(group_name("site", &device.site))
                .or_default()
                .push(device);
            placed = true;
        }
        if !device.role.is_empty() {
            groups
                .entry(group_name("role", &device.role))
                .or_default()
                .push(device);
            placed = true;
        }
        if !placed {
            groups.entry("ungrouped".into()).or_default().push(device);
        }
    }

    let mut out = String::from(
        "# Generated by Nocturne. Passwords are not exported: supply them with Ansible Vault or --ask-pass.\nall:\n  hosts:\n",
    );
    for device in devices
        .iter()
        .filter(|device| matches!(device.transport, Transport::Ssh | Transport::Telnet))
    {
        out.push_str(&format!(
            "    {}:\n      ansible_host: {}\n",
            quote(&device.name),
            quote(&device.address)
        ));
        if let Some(port) = device.port {
            out.push_str(&format!("      ansible_port: {port}\n"));
        }
        if let Some(os) = ansible_os_for(&device.profile) {
            out.push_str(&format!("      ansible_network_os: {os}\n      ansible_connection: ansible.netcommon.network_cli\n"));
        }
        if let Some(credential) =
            credential_of(credentials, device).filter(|credential| !credential.username.is_empty())
        {
            out.push_str(&format!(
                "      ansible_user: {}\n",
                quote(&credential.username)
            ));
            if credential.has_enable_secret {
                out.push_str("      ansible_become: true\n      ansible_become_method: enable\n");
            }
        }
    }
    out.push_str("  children:\n");
    for (group, members) in groups {
        out.push_str(&format!("    {group}:\n      hosts:\n"));
        for device in members
            .iter()
            .filter(|device| matches!(device.transport, Transport::Ssh | Transport::Telnet))
        {
            out.push_str(&format!("        {}: {{}}\n", quote(&device.name)));
        }
    }
    out
}

// ---------------------------------------------------------------------- rpc

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct IdsParams {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct UpdateParams {
    id: String,
    #[serde(flatten)]
    input: DeviceInput,
}

#[derive(Deserialize)]
struct BulkParams {
    ids: Vec<String>,
    #[serde(flatten)]
    patch: BulkPatch,
}

#[derive(Deserialize)]
struct ImportParams {
    #[serde(default)]
    format: String,
    text: String,
    #[serde(default)]
    update_existing: bool,
}

#[derive(Deserialize)]
struct ExportParams {
    format: String,
    #[serde(default)]
    ids: Option<Vec<String>>,
}

#[derive(Serialize)]
struct Count {
    count: usize,
}

#[derive(Serialize)]
struct Exported {
    text: String,
}

pub fn init(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add(
        "devices.list",
        |app: App, filter: DeviceFilter| async move { list(&app, &filter) },
    );
    router.add("devices.get", |app: App, params: IdParams| async move {
        row(&app, find(&app, &params.id)?)
    });
    router.add(
        "devices.create",
        |app: App, input: DeviceInput| async move { create(&app, input) },
    );
    router.add(
        "devices.update",
        |app: App, params: UpdateParams| async move { update(&app, &params.id, params.input) },
    );
    router.add("devices.delete", |app: App, params: IdsParams| async move {
        Ok(Count {
            count: delete(&app, &params.ids)?,
        })
    });
    router.add(
        "devices.bulk_update",
        |app: App, params: BulkParams| async move {
            Ok(Count {
                count: bulk_update(&app, &params.ids, params.patch)?,
            })
        },
    );
    router.add("devices.facets", |app: App, _: NoParams| async move {
        facets(&app)
    });
    router.add(
        "devices.import_preview",
        |app: App, params: ImportParams| async move {
            import_preview(&app, &params.format, &params.text)
        },
    );
    router.add(
        "devices.import",
        |app: App, params: ImportParams| async move {
            import_apply(&app, &params.format, &params.text, params.update_existing)
        },
    );
    router.add(
        "devices.export",
        |app: App, params: ExportParams| async move {
            Ok(Exported {
                text: export(&app, &params.format, params.ids.as_deref())?,
            })
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;
    use serde_json::{Value, json};

    fn app() -> App {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        crate::credentials::init(&mut builder);
        init(&mut builder);
        builder.build()
    }

    async fn add(app: &App, name: &str, address: &str, extra: Value) -> Value {
        let mut params = json!({"name": name, "address": address});
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        app.rpc("devices.create", params).await.unwrap()
    }

    #[tokio::test]
    async fn create_list_filter_update_delete() {
        let app = app();
        let mut events = app.subscribe();
        let core = add(&app, "CORE-SW1", "10.0.0.1", json!({"site": "HQ", "role": "Core", "tags": ["#critical", "critical", "l3"], "profile": "cisco_ios"})).await;
        assert_eq!(core["role"], "core");
        assert_eq!(core["tags"], json!(["critical", "l3"]));
        assert_eq!(core["transport"], "ssh");
        assert_eq!(core["monitor"]["interval_s"], 60);
        assert_eq!(core["state"]["reachable"], Value::Null);
        add(
            &app,
            "acc-sw10",
            "10.0.1.10",
            json!({"site": "HQ", "role": "access"}),
        )
        .await;
        add(
            &app,
            "acc-sw2",
            "10.0.1.2",
            json!({"site": "Branch", "role": "access"}),
        )
        .await;

        // Skip the audit events; find the change announcement.
        let mut saw_created = false;
        while let Ok(event) = events.try_recv() {
            saw_created |= event.topic == "devices.changed" && event.payload["change"] == "created";
        }
        assert!(saw_created);

        let all = app.rpc("devices.list", Value::Null).await.unwrap();
        let names: Vec<&str> = all
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["acc-sw2", "acc-sw10", "CORE-SW1"],
            "natural, case-insensitive order"
        );

        let hq = app
            .rpc("devices.list", json!({"site": "hq"}))
            .await
            .unwrap();
        assert_eq!(hq.as_array().unwrap().len(), 2);
        let query = app
            .rpc("devices.list", json!({"query": "10.0.1 access"}))
            .await
            .unwrap();
        assert_eq!(query.as_array().unwrap().len(), 2);
        let tagged = app.rpc("devices.list", json!({"tag": "L3"})).await.unwrap();
        assert_eq!(tagged.as_array().unwrap().len(), 1);
        let unknown = app
            .rpc("devices.list", json!({"status": "unknown"}))
            .await
            .unwrap();
        assert_eq!(unknown.as_array().unwrap().len(), 3);

        let id = core["id"].as_str().unwrap();
        let updated = app
            .rpc(
                "devices.update",
                json!({"id": id, "address": "10.0.0.2", "port": 2222, "protected": true}),
            )
            .await
            .unwrap();
        assert_eq!(
            (
                updated["address"].as_str(),
                updated["port"].as_u64(),
                updated["site"].as_str()
            ),
            (Some("10.0.0.2"), Some(2222), Some("HQ"))
        );
        assert_eq!(updated["protected"], true);
        // Lookup by name works wherever an id is accepted.
        assert_eq!(
            app.rpc("devices.get", json!({"id": "core-sw1"}))
                .await
                .unwrap()["id"],
            core["id"]
        );

        update_state(&app, id, |state| state.reachable = Some(false)).unwrap();
        let down = app
            .rpc("devices.list", json!({"status": "down"}))
            .await
            .unwrap();
        assert_eq!(down.as_array().unwrap().len(), 1);

        let facets = app.rpc("devices.facets", Value::Null).await.unwrap();
        assert_eq!(facets["total"], 3);
        assert_eq!(facets["sites"], json!([["Branch", 1], ["HQ", 2]]));

        assert_eq!(
            app.rpc("devices.delete", json!({"ids": [id, "missing"]}))
                .await
                .unwrap()["count"],
            1
        );
        assert_eq!(
            app.rpc("devices.get", json!({"id": id}))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[tokio::test]
    async fn validation_rules() {
        let app = app();
        add(&app, "sw1", "10.0.0.1", json!({})).await;
        assert_eq!(
            app.rpc(
                "devices.create",
                json!({"name": "SW1", "address": "10.0.0.9"})
            )
            .await
            .unwrap_err()
            .code,
            "conflict"
        );
        assert_eq!(
            app.rpc("devices.create", json!({"name": "x"}))
                .await
                .unwrap_err()
                .code,
            "invalid"
        );
        assert_eq!(
            app.rpc("devices.create", json!({})).await.unwrap_err().code,
            "invalid"
        );
        assert!(
            app.rpc(
                "devices.create",
                json!({"address": "10.0.0.5", "port": 70000})
            )
            .await
            .unwrap_err()
            .message
            .contains("70000")
        );
        assert!(
            app.rpc(
                "devices.create",
                json!({"address": "10.0.0.5", "credential_id": "gone"})
            )
            .await
            .unwrap_err()
            .message
            .contains("credential")
        );
        // A device without a name takes its address.
        let unnamed = app
            .rpc("devices.create", json!({"address": "10.0.0.7"}))
            .await
            .unwrap();
        assert_eq!(unnamed["name"], "10.0.0.7");
        // Monitor-only devices need an address too, lab devices do not.
        let lab = app
            .rpc(
                "devices.create",
                json!({"name": "R1 (lab)", "lab": {"lab_id": "lab1", "node": "R1"}}),
            )
            .await
            .unwrap();
        assert_eq!(lab["transport"], "sim");
    }

    #[tokio::test]
    async fn jump_hosts_cannot_loop_or_be_orphaned() {
        let app = app();
        let bastion = add(&app, "bastion", "198.51.100.1", json!({})).await;
        let inner = add(
            &app,
            "inner",
            "10.0.0.1",
            json!({"jump_device_id": bastion["id"]}),
        )
        .await;
        let looped = app
            .rpc(
                "devices.update",
                json!({"id": bastion["id"], "jump_device_id": inner["id"]}),
            )
            .await
            .unwrap_err();
        assert!(looped.message.contains("loops"));
        let refused = app
            .rpc("devices.delete", json!({"ids": [bastion["id"]]}))
            .await
            .unwrap_err();
        assert_eq!(refused.code, "conflict");
        assert!(refused.message.contains("inner"));
        // Deleting both together is fine.
        assert_eq!(
            app.rpc(
                "devices.delete",
                json!({"ids": [bastion["id"], inner["id"]]})
            )
            .await
            .unwrap()["count"],
            2
        );
    }

    #[tokio::test]
    async fn bulk_update_changes_many_devices() {
        let app = app();
        let a = add(&app, "a", "10.0.0.1", json!({"tags": ["old", "keep"]})).await;
        let b = add(&app, "b", "10.0.0.2", json!({})).await;
        let count = app
            .rpc("devices.bulk_update", json!({"ids": [a["id"], b["id"]], "site": "DC1", "add_tags": ["new"], "remove_tags": ["OLD"], "monitor_enabled": false}))
            .await
            .unwrap();
        assert_eq!(count["count"], 2);
        let a = app
            .rpc("devices.get", json!({"id": a["id"]}))
            .await
            .unwrap();
        assert_eq!(
            (
                a["site"].as_str(),
                &a["tags"],
                a["monitor"]["enabled"].as_bool()
            ),
            (Some("DC1"), &json!(["keep", "new"]), Some(false))
        );
    }

    #[tokio::test]
    async fn csv_import_previews_then_commits() {
        let app = app();
        app.rpc(
            "credentials.create",
            json!({"name": "Campus", "username": "admin"}),
        )
        .await
        .unwrap();
        add(&app, "sw1", "10.0.0.1", json!({})).await;
        let csv = "\u{feff}Hostname,IP Address,Platform,Site,Tags,Credential,Port\n\
                   sw1,10.0.0.11,ios,HQ,core;l3,Campus,\n\
                   sw2,10.0.0.2,cisco.nxos.nxos,HQ,,Nope,2222\n\
                   ,,,,,,\n\
                   sw3,,eos,HQ,,,\n\
                   sw2,10.0.0.3,,,,,\n";
        let preview = app
            .rpc("devices.import_preview", json!({"text": csv}))
            .await
            .unwrap();
        assert_eq!(preview["format"], "csv");
        assert_eq!(
            (
                preview["new"].as_u64(),
                preview["update"].as_u64(),
                preview["invalid"].as_u64()
            ),
            (Some(1), Some(1), Some(2))
        );
        assert_eq!(preview["rows"][0]["input"]["profile"], "cisco_ios");
        assert_eq!(preview["rows"][1]["input"]["profile"], "cisco_nxos");

        let result = app
            .rpc(
                "devices.import",
                json!({"text": csv, "update_existing": true}),
            )
            .await
            .unwrap();
        assert_eq!(
            (result["created"].as_u64(), result["updated"].as_u64()),
            (Some(1), Some(1))
        );
        assert_eq!(result["skipped"].as_array().unwrap().len(), 3, "{result}");
        let sw1 = app.rpc("devices.get", json!({"id": "sw1"})).await.unwrap();
        assert_eq!(
            (sw1["address"].as_str(), sw1["site"].as_str()),
            (Some("10.0.0.11"), Some("HQ"))
        );
        assert!(sw1["credential_id"].is_string());
        assert_eq!(
            app.rpc("devices.get", json!({"id": "sw2"})).await.unwrap()["port"],
            2222
        );
    }

    #[tokio::test]
    async fn imports_ssh_config_and_ansible_inventories() {
        let app = app();
        let ssh = "Host *\n  ServerAliveInterval 30\n\nHost core-rtr edge-rtr\n  User netops\n  Port 2222\n\nHost fw1\n  HostName 192.0.2.10\n";
        let preview = import_preview(&app, "auto", ssh).unwrap();
        assert_eq!(preview.format, "ssh_config");
        assert_eq!(preview.rows.len(), 3);
        assert_eq!(preview.rows[0].input.port, Some(2222));
        assert_eq!(preview.rows[2].input.address.as_deref(), Some("192.0.2.10"));

        let ini = "[switches]\nsw1 ansible_host=10.0.0.1\nsw2 ansible_host=10.0.0.2 ansible_port=2022\n\n[routers]\nr1\n\n[campus:children]\nswitches\nrouters\n\n[switches:vars]\nansible_network_os=cisco.ios.ios\n\n[campus:vars]\nansible_user=netops\n";
        let preview = import_preview(&app, "auto", ini).unwrap();
        assert_eq!(preview.format, "ansible");
        let sw2 = &preview.rows[1].input;
        assert_eq!(
            (sw2.address.as_deref(), sw2.port, sw2.profile.as_deref()),
            (Some("10.0.0.2"), Some(2022), Some("cisco_ios"))
        );
        assert_eq!(
            sw2.tags.as_ref().unwrap(),
            &vec!["campus".to_string(), "switches".to_string()]
        );
        assert_eq!(preview.rows[2].input.address.as_deref(), Some("r1"));
        assert!(
            preview.rows[2]
                .input
                .notes
                .as_ref()
                .unwrap()
                .contains("netops")
        );

        let yaml = "all:\n  vars:\n    ansible_connection: ansible.netcommon.network_cli\n  children:\n    edge:\n      vars:\n        ansible_network_os: community.routeros.routeros\n      hosts:\n        mt1:\n          ansible_host: 192.168.88.1\n        mt2: {}\n";
        let preview = import_preview(&app, "ansible", yaml).unwrap();
        assert_eq!(preview.rows.len(), 2);
        assert_eq!(
            preview.rows[0].input.profile.as_deref(),
            Some("mikrotik_routeros")
        );
        assert_eq!(preview.rows[0].input.transport, Some(Transport::Ssh));
        assert_eq!(preview.rows[1].input.address.as_deref(), Some("mt2"));
    }

    #[tokio::test]
    async fn exports_csv_and_ansible_without_secrets() {
        let app = app();
        let credential = app
            .rpc("credentials.create", json!({"name": "Campus", "username": "netops", "password": "TopSecret!", "enable_secret": "En"}))
            .await
            .unwrap();
        add(&app, "CORE \"A\"", "10.0.0.1", json!({"site": "HQ Main", "role": "core", "profile": "cisco_ios", "credential_id": credential["id"], "tags": ["a", "b"]})).await;
        add(&app, "probe", "10.0.0.50", json!({"transport": "none"})).await;

        let csv = app
            .rpc("devices.export", json!({"format": "csv"}))
            .await
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(csv.starts_with("name,address,profile"));
        assert!(
            csv.contains("\"CORE \"\"A\"\"\",10.0.0.1,cisco_ios,ssh,,HQ Main,core,a;b,Campus,")
        );
        // Round trip: the export can be imported again.
        assert_eq!(import_preview(&app, "csv", &csv).unwrap().update, 2);

        let ansible = app
            .rpc("devices.export", json!({"format": "ansible"}))
            .await
            .unwrap()["text"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            ansible.contains("ansible_network_os: cisco.ios.ios")
                && ansible.contains("ansible_user: \"netops\"")
        );
        assert!(ansible.contains("site_hq_main:") && ansible.contains("role_core:"));
        assert!(ansible.contains("ansible_become_method: enable"));
        assert!(
            !ansible.contains("TopSecret") && !ansible.contains("probe"),
            "{ansible}"
        );
        assert!(
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&ansible).is_ok(),
            "export must be valid YAML"
        );
    }
}
