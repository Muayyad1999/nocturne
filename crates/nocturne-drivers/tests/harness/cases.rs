//! The shapes of the YAML test files. Unknown keys are rejected, so a typo
//! in a test file is reported instead of quietly testing nothing.

use indexmap::IndexMap;
use nocturne_drivers::collect::ProblemCode;
use nocturne_drivers::model::Kind;
use nocturne_drivers::session::Mode;
use serde::Deserialize;
use serde_json::Value;

/// `prompts.yml`
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Prompts {
    /// Prompts of the normal (privileged) mode.
    pub exec: Vec<String>,
    /// Prompts of the unprivileged mode.
    pub user: Vec<String>,
    /// Prompts inside configuration mode.
    pub config: Vec<String>,
    /// Lines of ordinary output that must not be taken for a prompt.
    pub not_prompts: Vec<String>,
    /// Prompt → the hostname the driver must read from it.
    pub hosts: IndexMap<String, String>,
}

/// `errors.yml`
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Errors {
    pub exec: ErrorSamples,
    pub config: ErrorSamples,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ErrorSamples {
    /// Output of commands the device refused.
    pub rejected: Vec<Rejected>,
    /// Output of commands that worked, however alarming it looks.
    pub clean: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Rejected {
    Output(String),
    Detailed {
        output: String,
        /// The line that must be reported as the error.
        error: String,
    },
}

/// `policy.yml`
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct PolicyCases {
    pub read_only: Vec<String>,
    pub change: Vec<String>,
    pub dangerous: Vec<String>,
}

/// `detect.yml`
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct DetectCases {
    pub banner: Evidence<String>,
    pub snmp: Evidence<Snmp>,
    /// Probe command → what the device prints. Commands that are not listed are rejected.
    pub probe: Evidence<IndexMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Evidence<T> {
    /// Must select this profile.
    #[serde(rename = "match")]
    pub matching: Vec<T>,
    /// Must not select this profile.
    pub reject: Vec<T>,
}

impl<T> Default for Evidence<T> {
    fn default() -> Self {
        Evidence {
            matching: Vec::new(),
            reject: Vec::new(),
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Snmp {
    pub sysdescr: String,
    pub sysobjectid: String,
}

/// Where the files of a capture come from. It decides what the capture
/// proves: only a recording says anything about the real platform.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// Printed by a device of this platform and recorded: the ntc-templates
    /// corpus, or a device somebody ran the command on.
    #[default]
    Recorded,
    /// A sample printed in the vendor's own documentation, copied as it
    /// stands there. It shows the format the vendor describes, but it has
    /// been through an editor and a typesetter and nobody can say which
    /// release printed it, so it is not a recording.
    Documented,
    /// Written by hand from a documented format, or printed by something
    /// that is not the platform (Nocturne's simulator).
    Synthetic,
}

/// `captures/<case>/expect.yml`
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    /// `recorded` unless stated.
    pub origin: Option<Origin>,
    /// The older spelling of `origin: synthetic`.
    pub synthetic: Option<bool>,
    pub kinds: IndexMap<Kind, KindExpect>,
    /// Problems the collection must report (none unless listed).
    #[serde(default)]
    pub problems: Vec<ExpectedProblem>,
}

impl Expect {
    /// What the case says its files are. Saying it twice is refused, so that
    /// the two statements cannot come to disagree.
    pub fn origin(&self) -> Result<Origin, &'static str> {
        match (self.origin, self.synthetic) {
            (Some(_), Some(_)) => Err(
                "says both `origin` and `synthetic`: state where the files come from once, with `origin: recorded | documented | synthetic`",
            ),
            (Some(origin), None) => Ok(origin),
            (None, Some(true)) => Ok(Origin::Synthetic),
            (None, Some(false) | None) => Ok(Origin::Recorded),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KindExpect {
    /// The exact number of rows. May be left out for `facts` and `health`, which are one row.
    /// `0` is a read that worked and found nothing, and then `rows` is empty.
    pub count: Option<usize>,
    /// Rows that must be there. Each is found by its first field; every field listed must match exactly.
    #[serde(default)]
    pub rows: Vec<IndexMap<String, Value>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedProblem {
    pub kind: Kind,
    pub code: ProblemCode,
    /// Text the problem's message must contain.
    #[serde(default)]
    pub contains: String,
}

/// `session/<case>.yml`
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transcript {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub credentials: Login,
    /// Whether the driver may enter privileged mode (default yes).
    pub enable: Option<bool>,
    /// Whether the driver runs the profile's `cli.init` commands (default yes).
    pub init: Option<bool>,
    /// What the device prints before anything is sent to it.
    #[serde(default)]
    pub greeting: String,
    /// What the driver must send, in order, and what the device prints back.
    pub dialogue: Vec<Exchange>,
    /// What the test does with the session.
    pub actions: Vec<Action>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Login {
    pub username: Option<String>,
    pub password: Option<String>,
    pub enable_secret: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Exchange {
    /// A line the driver must send (without the line ending).
    pub recv: Option<String>,
    /// A key the driver must send as is (a pager's space bar).
    pub key: Option<String>,
    /// What the device prints in reply, echo and next prompt included.
    #[serde(default)]
    pub send: String,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Action {
    /// Log in. Implied before the first other action.
    pub start: Option<Start>,
    /// Run a command in exec mode.
    pub exec: Option<String>,
    /// The exact output `exec` must return.
    pub output: Option<String>,
    /// The error line `exec` must report (none unless given), or text the
    /// failure of `start` / `configure` / `save` must contain.
    pub error: Option<String>,
    /// Apply configuration lines.
    pub configure: Option<Vec<String>>,
    /// Index of the line `configure` must stop at (it must succeed unless given).
    pub failed_at: Option<usize>,
    /// Save the configuration.
    pub save: Option<bool>,
    /// Fetch the `running` or `startup` configuration.
    pub fetch_config: Option<String>,
    /// The exact cleaned configuration `fetch_config` must return.
    pub config: Option<String>,
    /// The mode the session must be in afterwards.
    pub mode: Option<Mode>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Start {
    pub hostname: Option<String>,
    pub prompt: Option<String>,
    /// Text the login screen must contain.
    pub login_screen: Option<String>,
    /// How many warnings the login must leave (none unless given).
    pub warnings: usize,
}
