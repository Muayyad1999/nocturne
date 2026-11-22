//! Notes and runbooks: what the last engineer learned at 3am.
//!
//! Markdown against a device, a site, a tag or an incident. The body is
//! stored exactly as written and never rendered here. A `[[device name]]` in
//! it is resolved to a device id when the note is saved, so that the note
//! turns up on that device's page afterwards — and so that a note naming a
//! device that does not exist says so instead of pretending the link works.

use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, AppError, AppResult};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::{Note, NoteHead, NoteLink};
use crate::records::{self, Kind, Query, Record};
use crate::refs::Devices;
use crate::text;

const WHAT: &str = "note";

/// `[[…]]`, with no nesting and nothing spanning a line.
fn link_pattern() -> &'static Regex {
    static PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"\[\[([^\[\]\r\n]{1,200})\]\]").expect("a valid pattern"))
}

/// The `[[device name]]` links in a body, resolved against the inventory.
/// A name that matches nothing keeps its text and gets no id: the note says
/// what it says, and the reader can see the link did not resolve.
pub fn resolve_links(body: &str, devices: &Devices) -> Vec<NoteLink> {
    let mut links: Vec<NoteLink> = Vec::new();
    for found in link_pattern().captures_iter(body) {
        let text = found[1].trim().to_string();
        if text.is_empty() || links.iter().any(|link| link.text == text) {
            continue;
        }
        let wanted = text.to_lowercase();
        let device_id = devices
            .rows()
            .iter()
            .find(|row| {
                row.device.name.to_lowercase() == wanted
                    || row.device.id == text
                    || row.device.address.to_lowercase() == wanted
            })
            .map(|row| row.device.id.clone());
        links.push(NoteLink { text, device_id });
    }
    links
}

fn record(note: &Note) -> AppResult<Record> {
    Ok(Record {
        id: note.id.clone(),
        sort: note.updated_at,
        facet: String::new(),
        head: serde_json::to_string(&note.head())?,
        search: note.search_text(),
        body: serde_json::to_string(note)?,
        links: note.links_to(),
    })
}

pub fn load(app: &App, id: &str) -> AppResult<Note> {
    let body = records::body(app.store(), Kind::Note, id)?
        .ok_or_else(|| AppError::not_found(format!("{WHAT} `{id}`")))?;
    serde_json::from_str(&body).map_err(AppError::from)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListParams {
    pub device_id: Option<String>,
    pub site: Option<String>,
    pub tag: Option<String>,
    pub incident_id: Option<String>,
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

pub fn list(app: &App, params: &ListParams) -> AppResult<Value> {
    let mut links: Vec<String> = Vec::new();
    if let Some(device_id) = &params.device_id {
        links.push(format!("device:{device_id}"));
    }
    if let Some(site) = &params.site {
        links.push(format!("site:{}", site.to_lowercase()));
    }
    if let Some(tag) = &params.tag {
        links.push(format!("tag:{}", tag.to_lowercase()));
    }
    if let Some(incident_id) = &params.incident_id {
        links.push(format!("incident:{incident_id}"));
    }
    let words = params
        .query
        .as_deref()
        .map(records::words)
        .unwrap_or_default();
    let page = records::page(
        app.store(),
        Kind::Note,
        &Query {
            links,
            words: words.clone(),
            cursor: params.cursor.clone(),
            limit: params.limit.unwrap_or(50).clamp(1, 500),
            ..Query::default()
        },
    )?;
    let mut notes: Vec<Value> = Vec::new();
    for head in &page.heads {
        let head: NoteHead = serde_json::from_str(head)?;
        let mut value = serde_json::to_value(&head)?;
        if !words.is_empty() {
            // The piece of the note the words were found in, so a search
            // result can be read without opening it.
            let note = load(app, &head.id)?;
            value["excerpt"] = json!(text::excerpt(&note.body, &words, 240));
        }
        notes.push(value);
    }
    Ok(json!({"notes": notes, "next_cursor": page.next_cursor}))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    /// Left out for a new note.
    pub id: Option<String>,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub device_ids: Vec<String>,
    pub sites: Vec<String>,
    pub incident_ids: Vec<String>,
    pub author: String,
}

pub fn save(app: &App, params: SaveParams) -> AppResult<Value> {
    if params.title.trim().is_empty() {
        return Err(AppError::invalid("a note needs a title"));
    }
    text::prose("a note's body", &params.body)?;
    let now = now_ms();
    let existing = match &params.id {
        Some(id) => Some(load(app, id)?),
        None => None,
    };
    let devices = Devices::load(app)?;
    let links = resolve_links(&params.body, &devices);
    let mut device_ids = params.device_ids.clone();
    // A device named in the text is a device the note is about.
    for link in &links {
        if let Some(id) = &link.device_id
            && !device_ids.contains(id)
        {
            device_ids.push(id.clone());
        }
    }
    device_ids.sort();
    device_ids.dedup();
    let mut tags: Vec<String> = params
        .tags
        .iter()
        .map(|tag| tag.trim().trim_start_matches('#').to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect();
    tags.sort();
    tags.dedup();
    let mut sites: Vec<String> = params
        .sites
        .iter()
        .map(|site| site.trim().to_string())
        .filter(|site| !site.is_empty())
        .collect();
    sites.sort();
    sites.dedup();

    let note = Note {
        id: existing
            .as_ref()
            .map(|note| note.id.clone())
            .unwrap_or_else(new_id),
        title: params.title.trim().to_string(),
        body: params.body.clone(),
        tags,
        device_ids,
        sites,
        incident_ids: params.incident_ids.clone(),
        links,
        author: match (params.author.as_str(), &existing) {
            ("", Some(note)) => note.author.clone(),
            (author, _) => author.to_string(),
        },
        created_at: existing.as_ref().map_or(now, |note| note.created_at),
        updated_at: now,
    };
    records::save(app.store(), Kind::Note, &record(&note)?)?;
    app.publish("notes.changed", &note.head());
    app.audit(
        if existing.is_some() {
            "notes.update"
        } else {
            "notes.create"
        },
        note.device_ids.first().map(String::as_str),
        format!(
            "{} note `{}`",
            if existing.is_some() {
                "Edited"
            } else {
                "Wrote"
            },
            note.title
        ),
        json!({
            "note_id": note.id,
            "device_ids": note.device_ids,
            "sites": note.sites,
            "tags": note.tags,
            "unresolved_links": note
                .links
                .iter()
                .filter(|link| link.device_id.is_none())
                .map(|link| link.text.clone())
                .collect::<Vec<_>>(),
        }),
    );
    Ok(serde_json::to_value(&note)?)
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

pub fn delete(app: &App, id: &str) -> AppResult<Value> {
    let note = load(app, id)?;
    let removed = records::delete(app.store(), Kind::Note, id)?;
    app.publish("notes.changed", &json!({"id": id, "deleted": true}));
    app.audit(
        "notes.delete",
        note.device_ids.first().map(String::as_str),
        format!("Deleted note `{}`", note.title),
        json!({"note_id": id}),
    );
    Ok(json!({"deleted": removed}))
}

/// A note with its links resolved against the inventory as it is now, so a
/// link to a device that has since been renamed or deleted is visible.
fn detailed(app: &App, note: &Note) -> AppResult<Value> {
    let devices = Devices::load(app)?;
    let mut value = serde_json::to_value(note)?;
    value["links"] = serde_json::to_value(
        note.links
            .iter()
            .map(|link| {
                json!({
                    "text": link.text,
                    "device_id": link.device_id,
                    "device": link
                        .device_id
                        .as_ref()
                        .map(|id| devices.reference(id)),
                    "resolved": link.device_id.is_some(),
                })
            })
            .collect::<Vec<_>>(),
    )?;
    value["devices"] = serde_json::to_value(
        note.device_ids
            .iter()
            .map(|id| devices.reference(id))
            .collect::<Vec<_>>(),
    )?;
    Ok(value)
}

/// Notes matching a free-text query, for the global search.
pub fn search(app: &App, words: &[String], limit: usize) -> AppResult<Vec<Value>> {
    let page = records::page(
        app.store(),
        Kind::Note,
        &Query {
            words: words.to_vec(),
            limit,
            ..Query::default()
        },
    )?;
    // The same row `notes.list` returns, plus where the words were found, so
    // a search result and a note list are one shape for the UI.
    let mut found = Vec::new();
    for head in &page.heads {
        let head: NoteHead = serde_json::from_str(head)?;
        let note = load(app, &head.id)?;
        let mut value = serde_json::to_value(&head)?;
        value["excerpt"] = json!(text::excerpt(&note.body, words, 240));
        found.push(value);
    }
    Ok(found)
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add("notes.list", |app: App, params: ListParams| async move {
        list(&app, &params)
    });
    router.add("notes.get", |app: App, params: IdParams| async move {
        detailed(&app, &load(&app, &params.id)?)
    });
    router.add("notes.save", |app: App, params: SaveParams| async move {
        save(&app, params)
    });
    router.add("notes.delete", |app: App, params: IdParams| async move {
        delete(&app, &params.id)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::model::{Device, DeviceRow, DeviceState};
    use pretty_assertions::assert_eq;

    fn devices() -> Devices {
        let row = |name: &str, address: &str| DeviceRow {
            device: Device {
                id: format!("id-{name}"),
                name: name.into(),
                address: address.into(),
                ..Device::default()
            },
            state: DeviceState::default(),
        };
        Devices::from_rows(vec![
            row("EDGE-RTR1", "10.0.0.1"),
            row("core-sw1", "10.0.0.2"),
        ])
    }

    #[test]
    fn device_links_resolve_by_name_address_or_id() {
        let body = "To fail over, shut the uplink on [[EDGE-RTR1]] and watch [[CORE-SW1]].\n\
                    Console is on [[10.0.0.1]]; the old box [[ACC-SW9]] is gone.\n\
                    Mentioning [[EDGE-RTR1]] again adds nothing.";
        let links = resolve_links(body, &devices());
        assert_eq!(
            links
                .iter()
                .map(|link| (link.text.as_str(), link.device_id.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("EDGE-RTR1", Some("id-EDGE-RTR1")),
                ("CORE-SW1", Some("id-core-sw1")),
                ("10.0.0.1", Some("id-EDGE-RTR1")),
                // A name the inventory does not have keeps its text and
                // stays unresolved rather than being dropped or guessed.
                ("ACC-SW9", None),
            ]
        );
    }

    #[test]
    fn a_link_cannot_span_lines_or_nest() {
        let devices = devices();
        assert!(resolve_links("[[\nEDGE-RTR1\n]]", &devices).is_empty());
        assert!(resolve_links("[[]]", &devices).is_empty());
        // Extra brackets around a link still name the device inside them.
        assert_eq!(
            resolve_links("[[[EDGE-RTR1]]]", &devices)[0]
                .device_id
                .as_deref(),
            Some("id-EDGE-RTR1")
        );
        // One pair of brackets is not a link.
        assert!(resolve_links("[EDGE-RTR1]", &devices).is_empty());
        // A very long name is not matched, so a pathological body cannot
        // produce a huge link text.
        let long = format!("[[{}]]", "x".repeat(400));
        assert!(resolve_links(&long, &devices).is_empty());
    }
}
