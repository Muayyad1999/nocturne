//! What an insight is, as the UI and the assistant see it.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub const ALL: [Severity; 5] = [
        Severity::Info,
        Severity::Low,
        Severity::Medium,
        Severity::High,
        Severity::Critical,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// The order stored in the table: 0 (info) … 4 (critical).
    pub fn rank(self) -> i64 {
        self as i64
    }

    pub fn from_rank(rank: i64) -> Severity {
        match rank {
            i64::MIN..=0 => Severity::Info,
            1 => Severity::Low,
            2 => Severity::Medium,
            3 => Severity::High,
            _ => Severity::Critical,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Physical,
    L2,
    L3,
    Config,
    Security,
    Capacity,
    Hygiene,
}

impl Category {
    pub const ALL: [Category; 7] = [
        Category::Physical,
        Category::L2,
        Category::L3,
        Category::Config,
        Category::Security,
        Category::Capacity,
        Category::Hygiene,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Physical => "physical",
            Category::L2 => "l2",
            Category::L3 => "l3",
            Category::Config => "config",
            Category::Security => "security",
            Category::Capacity => "capacity",
            Category::Hygiene => "hygiene",
        }
    }

    pub fn parse(text: &str) -> Option<Category> {
        Category::ALL
            .into_iter()
            .find(|category| category.as_str() == text)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Dismissed,
    Resolved,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Dismissed => "dismissed",
            State::Resolved => "resolved",
        }
    }

    pub fn parse(text: &str) -> Option<State> {
        [State::Open, State::Dismissed, State::Resolved]
            .into_iter()
            .find(|state| state.as_str() == text)
    }
}

/// One fact a finding was decided from: a counter, a configuration line, a timestamp.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub label: String,
    pub value: String,
}

/// What to do about a finding.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    /// Configuration-mode lines for the insight's device, exactly as they
    /// would be typed; `insights.fix` hands them to the change pipeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_lines: Option<Vec<String>>,
    /// Read-only commands that confirm the finding on the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
}

impl Suggestion {
    pub fn text(text: impl Into<String>) -> Suggestion {
        Suggestion {
            text: text.into(),
            ..Suggestion::default()
        }
    }

    pub fn lines<I, S>(mut self, lines: I) -> Suggestion
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let lines: Vec<String> = lines.into_iter().map(Into::into).collect();
        self.config_lines = (!lines.is_empty()).then_some(lines);
        self
    }

    pub fn commands<I, S>(mut self, commands: I) -> Suggestion
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let commands: Vec<String> = commands.into_iter().map(Into::into).collect();
        self.commands = (!commands.is_empty()).then_some(commands);
        self
    }
}

/// Another place the same problem shows: the far end of a link, the other
/// holder of an address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Related {
    pub device_id: String,
    pub subject: String,
    /// The related device's name, so a list can show it without a lookup.
    #[serde(default)]
    pub device: String,
}

/// A problem found on the network, with what it was decided from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Insight {
    /// Stable across scans: derived from the check, the device and the subject.
    pub id: String,
    pub check: String,
    pub category: Category,
    pub severity: Severity,
    pub device_id: String,
    pub device: String,
    /// What on the device it is about: `Gi0/3`, `VLAN 20`, `10.0.0.5`.
    pub subject: String,
    pub title: String,
    pub detail: String,
    pub evidence: Vec<Evidence>,
    pub suggestion: Option<Suggestion>,
    pub related: Vec<Related>,
    pub state: State,
    pub first_seen: i64,
    pub last_seen: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
    /// When a dismissal ends; absent while dismissed means "for good".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_until: Option<i64>,
    pub lab: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_note: Option<String>,
    /// How many separate times this problem has appeared (1 = first time).
    pub occurrences: u32,
}

/// What a check reports. The ledger turns it into an [`Insight`] (or updates
/// the one that is already there).
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub check: &'static str,
    pub device_id: String,
    pub subject: String,
    /// `None` = the check's default severity.
    pub severity: Option<Severity>,
    pub title: String,
    pub detail: String,
    pub evidence: Vec<Evidence>,
    pub suggestion: Option<Suggestion>,
    pub related: Vec<Related>,
}

impl Finding {
    pub fn new(
        check: &'static str,
        device_id: &str,
        subject: impl Into<String>,
        title: impl Into<String>,
    ) -> Finding {
        Finding {
            check,
            device_id: device_id.to_string(),
            subject: subject.into(),
            severity: None,
            title: title.into(),
            detail: String::new(),
            evidence: Vec::new(),
            suggestion: None,
            related: Vec::new(),
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Finding {
        self.detail = detail.into();
        self
    }

    pub fn severity(mut self, severity: Severity) -> Finding {
        self.severity = Some(severity);
        self
    }

    pub fn evidence(mut self, label: impl Into<String>, value: impl Into<String>) -> Finding {
        self.evidence.push(Evidence {
            label: label.into(),
            value: value.into(),
        });
        self
    }

    pub fn suggest(mut self, suggestion: Suggestion) -> Finding {
        self.suggestion = Some(suggestion);
        self
    }

    pub fn related(mut self, device_id: &str, device: &str, subject: impl Into<String>) -> Finding {
        self.related.push(Related {
            device_id: device_id.to_string(),
            subject: subject.into(),
            device: device.to_string(),
        });
        self
    }

    pub fn id(&self) -> String {
        insight_id(self.check, &self.device_id, &self.subject)
    }
}

/// The id of the insight for `subject` of `check` on a device. The same
/// problem hashes to the same id in every scan, which is what lets it keep
/// its history, its dismissal and its alert.
pub fn insight_id(check: &str, device_id: &str, subject: &str) -> String {
    let mut hasher = Sha256::new();
    for part in [check, device_id, subject.trim()] {
        hasher.update(part.as_bytes());
        // A separator that cannot occur in any part keeps ("ab", "c") apart from ("a", "bc").
        hasher.update([0x1f]);
    }
    hex::encode(&hasher.finalize()[..10])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_distinct() {
        let id = insight_id("duplex_mismatch", "dev1", "Gi0/3");
        assert_eq!(id.len(), 20);
        assert_eq!(id, insight_id("duplex_mismatch", "dev1", " Gi0/3 "));
        assert_ne!(id, insight_id("duplex_mismatch", "dev1", "Gi0/4"));
        assert_ne!(id, insight_id("speed_mismatch", "dev1", "Gi0/3"));
        assert_ne!(id, insight_id("duplex_mismatch", "dev2", "Gi0/3"));
        // Parts do not run into each other.
        assert_ne!(insight_id("a", "bc", "d"), insight_id("ab", "c", "d"));
        // Pinned: ids must not change between releases, or every dismissal is lost.
        assert_eq!(id, "99f6704f0eaa509fa022");
    }

    #[test]
    fn severities_order_and_round_trip() {
        assert!(Severity::Critical > Severity::High && Severity::Low > Severity::Info);
        for severity in Severity::ALL {
            assert_eq!(Severity::from_rank(severity.rank()), severity);
            assert_eq!(
                serde_json::to_value(severity).unwrap(),
                serde_json::json!(severity.as_str())
            );
        }
        assert_eq!(Category::parse("l2"), Some(Category::L2));
        assert_eq!(State::parse("gone"), None);
    }

    #[test]
    fn optional_parts_are_left_out_of_the_wire_form() {
        let suggestion = Suggestion::text("Check the cable").commands(["show interfaces Gi0/1"]);
        let json = serde_json::to_value(&suggestion).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"text": "Check the cable", "commands": ["show interfaces Gi0/1"]})
        );
        let none: [&str; 0] = [];
        assert_eq!(Suggestion::text("x").lines(none).config_lines, None);
    }
}
