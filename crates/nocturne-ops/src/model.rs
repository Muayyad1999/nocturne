//! The records this feature owns: incidents with their timelines, notes, and
//! the saved copies of handovers and reports.

use serde::{Deserialize, Serialize};

use crate::refs::{Period, Ref};
use crate::sources::Unavailable;

// ---------------------------------------------------------------- incidents

/// How much of the network an incident is about. `sev1` is "the campus is
/// down", `sev4` "somebody should look at this eventually".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Sev1,
    Sev2,
    Sev3,
    Sev4,
}

impl Severity {
    pub const ALL: [Severity; 4] = [
        Severity::Sev1,
        Severity::Sev2,
        Severity::Sev3,
        Severity::Sev4,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Sev1 => "sev1",
            Severity::Sev2 => "sev2",
            Severity::Sev3 => "sev3",
            Severity::Sev4 => "sev4",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Happening, nobody has stopped the bleeding yet.
    Open,
    /// The users are served again; the cause is still there.
    Mitigated,
    /// Done, with a cause written down.
    Resolved,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Mitigated => "mitigated",
            State::Resolved => "resolved",
        }
    }

    pub fn is_open(self) -> bool {
        self != State::Resolved
    }
}

/// Where a timeline entry came from, so that the timeline is evidence rather
/// than a story. Every entry has one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// What produced the entry: `operator` (a person typed it), the RPC
    /// method of the feature it was read from (`alerts.get`), or the core
    /// table it was read from (`store.configs`, `store.logs`).
    pub from: String,
    /// Which record and which of its fields the entry's time and text come
    /// from, e.g. ``alert `a1`: started_at``. Enough to go back and check.
    pub evidence: String,
    /// When ops read it. A `gathered_at` far from `ts` means the entry was
    /// added to the timeline long after the thing itself happened.
    pub gathered_at: i64,
}

impl Provenance {
    /// Something a person typed through the RPC surface.
    pub fn operator(what: &str, at: i64) -> Provenance {
        Provenance {
            from: "operator".into(),
            evidence: what.to_string(),
            gathered_at: at,
        }
    }

    pub fn read(from: &str, evidence: impl Into<String>, at: i64) -> Provenance {
        Provenance {
            from: from.to_string(),
            evidence: evidence.into(),
            gathered_at: at,
        }
    }
}

/// What a timeline entry is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// Written by a person.
    Note,
    /// An alert fired, was acknowledged or resolved.
    Alert,
    /// A change was applied, failed or rolled back.
    Change,
    /// A command was run against a device (from the audit trail).
    Command,
    /// The incident itself moved: opened, acknowledged, assigned, resolved.
    Status,
    /// A log entry, including the audit trail and configuration movement.
    Log,
    /// A source that should have contributed entries could not be read. Not
    /// in the specification's list of kinds, and deliberately added: a
    /// timeline with a hole in it has to be able to say where the hole is.
    Gap,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Note => "note",
            EntryKind::Alert => "alert",
            EntryKind::Change => "change",
            EntryKind::Command => "command",
            EntryKind::Status => "status",
            EntryKind::Log => "log",
            EntryKind::Gap => "gap",
        }
    }
}

/// One line of an incident's timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Derived from what the entry is about (`alert:a1:fired`), so that
    /// gathering the same fact twice does not produce two entries.
    pub id: String,
    /// When the thing happened — not when it was read.
    pub ts: i64,
    pub kind: EntryKind,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "ref")]
    pub reference: Option<Ref>,
    /// Who did it, as the record says: a person, `nocturne` for something the
    /// platform did by itself, or empty when the source does not say.
    #[serde(default)]
    pub author: String,
    pub source: Provenance,
}

impl Entry {
    /// Timelines are ordered by time, and by id within one millisecond so
    /// that two facts stamped alike keep a stable order.
    pub fn order(&self) -> (i64, &str) {
        (self.ts, &self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FollowUp {
    pub text: String,
    #[serde(default)]
    pub done: bool,
}

/// Something that went wrong, and what is known about it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Incident {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    pub state: State,
    /// When the trouble started — taken from the alert when the incident was
    /// opened from one, otherwise when it was opened.
    pub started_at: i64,
    pub mitigated_at: Option<i64>,
    pub resolved_at: Option<i64>,
    /// When somebody said they were looking at it.
    pub acked_at: Option<i64>,
    pub acked_by: String,
    /// Who is on it. Free text: Nocturne has no user directory.
    pub assignee: String,
    pub device_ids: Vec<String>,
    pub alert_ids: Vec<String>,
    pub change_ids: Vec<String>,
    pub summary: String,
    pub impact: String,
    pub root_cause: String,
    pub follow_ups: Vec<FollowUp>,
    pub timeline: Vec<Entry>,
    /// Who opened it.
    pub opened_by: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for Incident {
    fn default() -> Incident {
        Incident {
            id: String::new(),
            title: String::new(),
            severity: Severity::Sev3,
            state: State::Open,
            started_at: 0,
            mitigated_at: None,
            resolved_at: None,
            acked_at: None,
            acked_by: String::new(),
            assignee: String::new(),
            device_ids: Vec::new(),
            alert_ids: Vec::new(),
            change_ids: Vec::new(),
            summary: String::new(),
            impact: String::new(),
            root_cause: String::new(),
            follow_ups: Vec::new(),
            timeline: Vec::new(),
            opened_by: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl Incident {
    /// The window the incident covers: from when it started to when it was
    /// resolved, or to `now` while it is open.
    pub fn window(&self, now: i64) -> Period {
        Period {
            since: self.started_at,
            until: self.resolved_at.unwrap_or(now).max(self.started_at) + 1,
        }
    }

    /// Put the timeline in order and drop entries that repeat an id.
    pub fn sort_timeline(&mut self) {
        self.timeline
            .sort_by(|a, b| a.order().cmp(&b.order()).then_with(|| a.ts.cmp(&b.ts)));
        let mut seen = std::collections::HashSet::new();
        self.timeline.retain(|entry| seen.insert(entry.id.clone()));
    }

    /// Add entries that are not in the timeline yet; returns how many were new.
    pub fn absorb(&mut self, entries: Vec<Entry>) -> usize {
        let known: std::collections::HashSet<String> =
            self.timeline.iter().map(|entry| entry.id.clone()).collect();
        let mut added = 0;
        for entry in entries {
            if !known.contains(&entry.id) {
                self.timeline.push(entry);
                added += 1;
            }
        }
        self.sort_timeline();
        added
    }

    /// What a list shows, without its timeline or its prose.
    pub fn head(&self) -> IncidentHead {
        IncidentHead {
            id: self.id.clone(),
            title: self.title.clone(),
            severity: self.severity,
            state: self.state,
            started_at: self.started_at,
            mitigated_at: self.mitigated_at,
            resolved_at: self.resolved_at,
            acked_at: self.acked_at,
            assignee: self.assignee.clone(),
            device_ids: self.device_ids.clone(),
            devices: Vec::new(),
            alert_ids: self.alert_ids.clone(),
            change_ids: self.change_ids.clone(),
            summary: self.summary.clone(),
            entries: self.timeline.len(),
            open_follow_ups: self
                .follow_ups
                .iter()
                .filter(|follow_up| !follow_up.done)
                .count(),
            // A timeline with a hole in it is worth seeing in the list.
            gaps: self
                .timeline
                .iter()
                .filter(|entry| entry.kind == EntryKind::Gap)
                .count(),
            updated_at: self.updated_at,
        }
    }

    /// The text a search matches against.
    pub fn search_text(&self) -> String {
        let mut text = format!(
            "{} {} {} {} {} {}",
            self.title,
            self.summary,
            self.impact,
            self.root_cause,
            self.assignee,
            self.severity.as_str()
        );
        for entry in &self.timeline {
            text.push(' ');
            text.push_str(&entry.text);
        }
        text.to_lowercase()
    }

    /// What the incident refers to, for the link table.
    pub fn links(&self) -> Vec<String> {
        let mut links: Vec<String> = Vec::new();
        for id in &self.device_ids {
            links.push(format!("device:{id}"));
        }
        for id in &self.alert_ids {
            links.push(format!("alert:{id}"));
        }
        for id in &self.change_ids {
            links.push(format!("change:{id}"));
        }
        links.sort();
        links.dedup();
        links
    }
}

/// An incident as a list shows it. `devices` is filled from the inventory
/// when the list is read, so a renamed device is not shown under its old name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IncidentHead {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    pub state: State,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mitigated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked_at: Option<i64>,
    #[serde(default)]
    pub assignee: String,
    pub device_ids: Vec<String>,
    /// The devices, as the inventory has them now.
    #[serde(default)]
    pub devices: Vec<crate::refs::DeviceRef>,
    #[serde(default)]
    pub alert_ids: Vec<String>,
    #[serde(default)]
    pub change_ids: Vec<String>,
    #[serde(default)]
    pub summary: String,
    /// How many timeline entries it has.
    pub entries: usize,
    pub open_follow_ups: usize,
    /// How many of the timeline's entries are a source that could not be read.
    #[serde(default)]
    pub gaps: usize,
    pub updated_at: i64,
}

// -------------------------------------------------------------------- notes

/// A `[[device name]]` in a note's text, resolved when it is saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteLink {
    /// The text inside the brackets.
    pub text: String,
    /// The device it names, when the inventory has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

/// What the last engineer learned at 3am.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Note {
    pub id: String,
    pub title: String,
    /// Markdown, as written. Never rendered by the backend.
    pub body: String,
    pub tags: Vec<String>,
    pub device_ids: Vec<String>,
    pub sites: Vec<String>,
    pub incident_ids: Vec<String>,
    /// `[[device name]]` links found in the body.
    pub links: Vec<NoteLink>,
    pub author: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Note {
    pub fn head(&self) -> NoteHead {
        NoteHead {
            id: self.id.clone(),
            title: self.title.clone(),
            tags: self.tags.clone(),
            device_ids: self.device_ids.clone(),
            sites: self.sites.clone(),
            incident_ids: self.incident_ids.clone(),
            author: self.author.clone(),
            lines: self.body.lines().count(),
            updated_at: self.updated_at,
        }
    }

    pub fn search_text(&self) -> String {
        format!(
            "{} {} {} {} {}",
            self.title,
            self.body,
            self.tags.join(" "),
            self.sites.join(" "),
            self.author
        )
        .to_lowercase()
    }

    pub fn links_to(&self) -> Vec<String> {
        let mut links: Vec<String> = Vec::new();
        for id in &self.device_ids {
            links.push(format!("device:{id}"));
        }
        for site in &self.sites {
            links.push(format!("site:{}", site.to_lowercase()));
        }
        for id in &self.incident_ids {
            links.push(format!("incident:{id}"));
        }
        for tag in &self.tags {
            links.push(format!("tag:{}", tag.to_lowercase()));
        }
        links.sort();
        links.dedup();
        links
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoteHead {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub device_ids: Vec<String>,
    pub sites: Vec<String>,
    pub incident_ids: Vec<String>,
    pub author: String,
    /// How long the note is, so a list can show "12 lines" without the body.
    pub lines: usize,
    pub updated_at: i64,
}

// ----------------------------------------------------------- saved documents

/// A saved shift handover: the data sections exactly as the server generated
/// them, plus whatever prose the outgoing engineer wrote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedHandover {
    pub id: String,
    pub period: Period,
    /// The generated document (`handover.generate`'s result).
    pub data: serde_json::Value,
    /// The human part. The only field a client or a model may write.
    pub notes: String,
    pub author: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for SavedHandover {
    fn default() -> SavedHandover {
        SavedHandover {
            id: String::new(),
            period: Period { since: 0, until: 0 },
            data: serde_json::Value::Null,
            notes: String::new(),
            author: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl SavedHandover {
    pub fn head(&self) -> HandoverHead {
        let count = |section: &str, field: &str| -> Option<u64> {
            self.data
                .pointer(&format!("/{section}/{field}"))
                .and_then(|value| match value {
                    serde_json::Value::Array(items) => Some(items.len() as u64),
                    other => other.as_u64(),
                })
        };
        HandoverHead {
            id: self.id.clone(),
            period: self.period,
            author: self.author.clone(),
            incidents_opened: count("incidents", "opened"),
            incidents_open: count("incidents", "open"),
            alerts_fired: count("alerts", "fired"),
            changes_applied: count("changes", "applied"),
            unavailable: self
                .data
                .get("unavailable")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default(),
            has_notes: !self.notes.trim().is_empty(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    pub fn search_text(&self) -> String {
        self.notes.to_lowercase()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HandoverHead {
    pub id: String,
    pub period: Period,
    pub author: String,
    /// `null` where the section itself was unavailable — not zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incidents_opened: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incidents_open: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerts_fired: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes_applied: Option<u64>,
    #[serde(default)]
    pub unavailable: Vec<Unavailable>,
    pub has_notes: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A report kept after it was generated.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedReport {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub format: String,
    /// The rendered report.
    pub content: String,
    /// How many rows of data it has (not lines of text).
    pub rows: usize,
    pub period: Option<Period>,
    pub filter: Option<serde_json::Value>,
    pub notes: String,
    pub author: String,
    pub unavailable: Vec<Unavailable>,
    pub generated_at: i64,
    pub created_at: i64,
}

impl SavedReport {
    pub fn head(&self) -> ReportHead {
        ReportHead {
            id: self.id.clone(),
            kind: self.kind.clone(),
            title: self.title.clone(),
            format: self.format.clone(),
            rows: self.rows,
            period: self.period,
            bytes: self.content.len(),
            notes: self.notes.clone(),
            author: self.author.clone(),
            unavailable: self.unavailable.clone(),
            generated_at: self.generated_at,
        }
    }

    pub fn search_text(&self) -> String {
        format!("{} {} {}", self.title, self.kind, self.notes).to_lowercase()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReportHead {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub format: String,
    pub rows: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<Period>,
    /// The size of the stored text, so a list need not carry it.
    pub bytes: usize,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub unavailable: Vec<Unavailable>,
    pub generated_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn entry(id: &str, ts: i64, kind: EntryKind) -> Entry {
        Entry {
            id: id.into(),
            ts,
            kind,
            text: format!("text of {id}"),
            reference: None,
            author: String::new(),
            source: Provenance::operator("typed", ts),
        }
    }

    #[test]
    fn absorbing_entries_is_idempotent_and_keeps_them_in_order() {
        let mut incident = Incident {
            id: "i1".into(),
            started_at: 100,
            ..Incident::default()
        };
        assert_eq!(
            incident.absorb(vec![
                entry("alert:a1:resolved", 900, EntryKind::Alert),
                entry("alert:a1:fired", 100, EntryKind::Alert),
            ]),
            2
        );
        // The same facts read again add nothing and move nothing.
        assert_eq!(
            incident.absorb(vec![
                entry("alert:a1:fired", 100, EntryKind::Alert),
                entry("note:n1", 400, EntryKind::Note),
            ]),
            1
        );
        assert_eq!(
            incident
                .timeline
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["alert:a1:fired", "note:n1", "alert:a1:resolved"]
        );
        // Two facts stamped the same millisecond keep a stable order by id.
        incident.absorb(vec![
            entry("change:c2:applied", 400, EntryKind::Change),
            entry("change:c1:applied", 400, EntryKind::Change),
        ]);
        assert_eq!(
            incident
                .timeline
                .iter()
                .filter(|entry| entry.ts == 400)
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["change:c1:applied", "change:c2:applied", "note:n1"]
        );
    }

    #[test]
    fn a_head_counts_what_a_list_shows_without_the_body() {
        let mut incident = Incident {
            id: "i1".into(),
            title: "Core switch down".into(),
            severity: Severity::Sev1,
            started_at: 100,
            device_ids: vec!["d1".into()],
            alert_ids: vec!["a1".into()],
            follow_ups: vec![
                FollowUp {
                    text: "replace the optic".into(),
                    done: false,
                },
                FollowUp {
                    text: "tell the users".into(),
                    done: true,
                },
            ],
            ..Incident::default()
        };
        incident.absorb(vec![
            entry("note:n1", 200, EntryKind::Note),
            entry("gap:changes:300", 300, EntryKind::Gap),
        ]);
        let head = incident.head();
        assert_eq!((head.entries, head.open_follow_ups, head.gaps), (2, 1, 1));
        assert_eq!(incident.links(), ["alert:a1", "device:d1"]);
        assert!(incident.search_text().contains("core switch down"));
        assert!(
            incident.search_text().contains("text of note:n1"),
            "the timeline is searchable"
        );
    }

    #[test]
    fn an_incidents_window_runs_to_now_while_it_is_open() {
        let incident = Incident {
            started_at: 1_000,
            ..Incident::default()
        };
        assert_eq!(incident.window(5_000).until, 5_001);
        let resolved = Incident {
            started_at: 1_000,
            resolved_at: Some(2_000),
            state: State::Resolved,
            ..Incident::default()
        };
        assert_eq!(
            resolved.window(9_000),
            Period {
                since: 1_000,
                until: 2_001
            }
        );
        // A clock that went backwards must not make an empty window.
        let odd = Incident {
            started_at: 9_000,
            resolved_at: Some(1_000),
            ..Incident::default()
        };
        assert!(odd.window(0).until > odd.window(0).since);
    }
}
