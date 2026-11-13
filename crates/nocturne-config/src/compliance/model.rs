//! What a compliance policy is, and what evaluating one produces.
//!
//! A policy is a named list of rules with a scope. A rule asks one question
//! of a stored configuration and is answered from the text alone — no device
//! is touched, so a run is instant and works with the network unreachable.
//!
//! The shapes here are the contract the UI and the assistant are written
//! against (`docs/dev/features/config.md`).

use nocturne_app::model::Device;
use serde::{Deserialize, Serialize};

/// How bad it is when a rule fails.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    pub const ALL: [Severity; 5] = [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Info,
    ];

    pub fn parse(text: &str) -> Option<Severity> {
        Severity::ALL
            .into_iter()
            .find(|severity| severity.as_str() == text)
    }
}

/// What a rule concluded about one device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    #[default]
    Fail,
    /// The rule has nothing to judge on this device (no section it is about).
    NotApplicable,
    /// The device has no stored configuration: nothing is known either way.
    NotEvaluated,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pass => "pass",
            Status::Fail => "fail",
            Status::NotApplicable => "not_applicable",
            Status::NotEvaluated => "not_evaluated",
        }
    }

    pub fn parse(text: &str) -> Option<Status> {
        [
            Status::Pass,
            Status::Fail,
            Status::NotApplicable,
            Status::NotEvaluated,
        ]
        .into_iter()
        .find(|status| status.as_str() == text)
    }
}

/// Which devices a policy is about. An empty list means "any"; a device must
/// match every list that is not empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    /// Device type ids. A device also matches a profile its own profile extends.
    pub profiles: Vec<String>,
    /// Operating system families as device types report them (`ios`, `routeros`);
    /// compared without regard to case.
    pub os: Vec<String>,
    pub roles: Vec<String>,
    pub sites: Vec<String>,
    pub tags: Vec<String>,
}

fn matches(wanted: &[String], have: &str) -> bool {
    wanted.is_empty() || wanted.iter().any(|one| one.eq_ignore_ascii_case(have))
}

impl Scope {
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
            && self.os.is_empty()
            && self.roles.is_empty()
            && self.sites.is_empty()
            && self.tags.is_empty()
    }

    /// Whether a device is in scope. `profile_lineage` is the device's device
    /// type and the ones it extends; `os` is what that device type reports.
    pub fn covers(&self, device: &Device, profile_lineage: &[String], os: &str) -> bool {
        let profile_ok = self.profiles.is_empty()
            || profile_lineage
                .iter()
                .any(|profile| matches(&self.profiles, profile));
        profile_ok
            && matches(&self.os, os)
            && matches(&self.roles, &device.role)
            && matches(&self.sites, &device.site)
            && (self.tags.is_empty() || device.tags.iter().any(|tag| matches(&self.tags, tag)))
    }
}

/// One question asked of a configuration.
///
/// Lines are compared after whitespace normalisation (runs of blanks become
/// one, leading and trailing blanks go). `line` entries of [`Check::Section`]
/// and [`Check::SectionWhen`] are matched both as a literal line and as a
/// regular expression — whichever hits first — so a rule can say
/// `transport input ssh` or `^transport input ssh$`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Check {
    /// A line at the top level of the configuration is exactly this.
    Present { line: String },
    /// No top-level line is exactly this.
    Absent { line: String },
    /// Some line matches the pattern. Lines are offered to the pattern
    /// indented one space per level, so `^ntp ` only matches a global line.
    Matches { pattern: String },
    /// No line matches the pattern.
    NotMatches { pattern: String },
    /// Sections whose header matches `parent`. With `each`, every one of them
    /// must satisfy `must`/`must_not`; without it, at least one must.
    Section {
        parent: String,
        #[serde(default)]
        each: bool,
        #[serde(default)]
        must: Vec<String>,
        #[serde(default)]
        must_not: Vec<String>,
    },
    /// Only the sections matching `parent` that contain a line matching
    /// `when`; every one of them must satisfy `must`/`must_not`.
    SectionWhen {
        parent: String,
        when: String,
        #[serde(default)]
        must: Vec<String>,
        #[serde(default)]
        must_not: Vec<String>,
    },
    /// A golden block: every line of `config`, in its block structure, must be
    /// present on the device.
    Snippet { config: String },
}

impl Check {
    /// The name the UI shows for the kind of check.
    pub fn kind(&self) -> &'static str {
        match self {
            Check::Present { .. } => "present",
            Check::Absent { .. } => "absent",
            Check::Matches { .. } => "matches",
            Check::NotMatches { .. } => "not_matches",
            Check::Section { .. } => "section",
            Check::SectionWhen { .. } => "section_when",
            Check::Snippet { .. } => "snippet",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    /// Why this matters, and what the rule can and cannot tell from the text.
    pub rationale: String,
    pub check: Check,
    /// Configuration lines that would fix it. `{{ device.name }}`,
    /// `{{ device.address }}`, `{{ device.site }}`, `{{ device.role }}` and —
    /// for a section rule — `{{ section }}` are substituted per device.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<Vec<String>>,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            id: String::new(),
            title: String::new(),
            severity: Severity::default(),
            rationale: String::new(),
            check: Check::Matches {
                pattern: String::new(),
            },
            remediation: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    /// Shipped with Nocturne: read-only, but it can be disabled or duplicated.
    pub builtin: bool,
    pub scope: Scope,
    pub rules: Vec<Rule>,
    /// When a user policy was last saved; 0 for a built-in.
    pub updated_at: i64,
}

/// One line of a configuration quoted as evidence. Line numbers are those of
/// the stored text, so they still point at the right line when it is shown
/// redacted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceLine {
    pub line_no: usize,
    pub line: String,
}

/// What one rule concluded about one device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Finding {
    pub policy_id: String,
    pub policy_name: String,
    pub rule_id: String,
    pub title: String,
    pub severity: Severity,
    pub device_id: String,
    pub device_name: String,
    pub status: Status,
    /// Configuration lines that justify the verdict — always redacted.
    pub evidence: Vec<EvidenceLine>,
    /// What the device should have and has not.
    pub missing: Vec<String>,
    /// One sentence for the person reading the result.
    pub detail: String,
    /// The configuration the verdict was reached from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<i64>,
    /// When that configuration was last fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
    /// The rule carries remediation lines, so `compliance.remediate` can act on it.
    pub remediable: bool,
    /// Section headers a section rule found wanting; `compliance.remediate`
    /// applies the remediation inside each of them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<String>,
}

impl Default for Finding {
    fn default() -> Self {
        Finding {
            policy_id: String::new(),
            policy_name: String::new(),
            rule_id: String::new(),
            title: String::new(),
            severity: Severity::default(),
            device_id: String::new(),
            device_name: String::new(),
            status: Status::NotEvaluated,
            evidence: Vec::new(),
            missing: Vec::new(),
            detail: String::new(),
            version_id: None,
            fetched_at: None,
            remediable: false,
            sections: Vec::new(),
        }
    }
}
