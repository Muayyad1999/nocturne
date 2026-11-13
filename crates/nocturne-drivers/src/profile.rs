//! Device profiles: a platform described as data.
//!
//! A profile is a YAML document that tells Nocturne how to talk to one kind
//! of device — what its prompts look like, how to turn paging off, how to
//! enter configuration mode and save, which commands are safe, and how to map
//! its `show` output onto the common [`model`](crate::model). Supporting a new
//! device type means writing (or asking the AI to draft) one of these files;
//! no code changes, no rebuild.
//!
//! Profiles can `extends:` another profile; the child is merged over the
//! parent key by key (maps merge, lists and scalars replace). Unknown keys are
//! rejected so that a typo is reported instead of silently ignored.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value as Yaml;

use crate::expr::Expressions;
use crate::hier::Syntax;
use crate::model::Kind;
use crate::policy::Policy;

static BUILTIN: include_dir::Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/profiles");

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// Unique key, e.g. `cisco_ios`. Ids starting with `_` are abstract bases.
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// Operating system family reported in facts ("IOS", "RouterOS", …).
    pub os: String,
    pub description: String,
    pub extends: Option<String>,
    /// Platform key in the ntc-templates index, for `parser: {ntc: auto}`.
    pub ntc_platform: Option<String>,
    pub detect: Detect,
    pub cli: Cli,
    pub config: ConfigSpec,
    pub policy: PolicySpec,
    pub collect: IndexMap<Kind, Vec<CollectStep>>,
    pub snmp: SnmpSpec,
}

/// How to recognise the platform.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Detect {
    /// Patterns for the SSH identification string, login banner or first screen.
    pub banner: Vec<String>,
    /// Patterns for SNMP `sysDescr`.
    pub sysdescr: Vec<String>,
    /// `sysObjectID` prefixes, e.g. `1.3.6.1.4.1.9.`.
    pub sysobjectid: Vec<String>,
    /// Commands to try on an unidentified session, with the pattern their output must match.
    pub probes: Vec<Probe>,
    /// Higher wins when several profiles match.
    pub priority: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Probe {
    pub command: String,
    pub matches: String,
}

/// Interactive session behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cli {
    /// Matches a complete prompt line. A named group `host` lets the driver
    /// pin the prompt to this device's hostname.
    pub prompt: String,
    /// Matches the prompt of the unprivileged mode (needs `enable`).
    pub user_prompt: Option<String>,
    /// Matches the prompt inside configuration mode.
    pub config_prompt: Option<String>,
    pub enable: Option<Enable>,
    pub login: Login,
    /// Sent once after login (turn paging off, widen the terminal).
    pub init: Vec<String>,
    pub pager: Option<Pager>,
    /// What "Enter" is on this platform.
    pub newline: String,
    pub exit: String,
    /// Output patterns that mean an exec command was rejected.
    pub errors: Vec<String>,
    /// Interactive questions to answer automatically in exec mode.
    pub answers: Vec<Answer>,
    /// Seconds without output before a command is considered hung.
    pub timeout_s: u64,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            prompt: r"^(?P<host>[\w.\-:/@~\[\]]{1,64})\s?[>#$%]\s?$".into(),
            user_prompt: None,
            config_prompt: None,
            enable: None,
            login: Login::default(),
            init: Vec::new(),
            pager: None,
            newline: "\n".into(),
            exit: "exit".into(),
            errors: Vec::new(),
            answers: Vec::new(),
            timeout_s: 30,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Enable {
    pub command: String,
    pub password_prompt: String,
}

impl Default for Enable {
    fn default() -> Self {
        Enable {
            command: "enable".into(),
            password_prompt: r"(?i)password:\s*$".into(),
        }
    }
}

/// Prompts seen when the transport does not authenticate for us (Telnet, console).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Login {
    pub username_prompt: String,
    pub password_prompt: String,
    pub failure: String,
    /// Appended to the username (RouterOS uses `+ct511w4098h` to disable colours and paging).
    pub username_suffix: String,
}

impl Default for Login {
    fn default() -> Self {
        Login {
            username_prompt: r"(?i)(?:user ?name|login|user):\s*$".into(),
            password_prompt: r"(?i)password:\s*$".into(),
            failure: r"(?i)(?:authentication fail|login (?:invalid|incorrect|failed)|access denied|bad passwords?|permission denied)"
                .into(),
            username_suffix: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pager {
    /// Matches the "more" prompt on the line being drawn.
    pub pattern: String,
    /// What to send to continue.
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Answer {
    pub prompt: String,
    pub answer: String,
}

/// How configuration is read and written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigSpec {
    pub syntax: Syntax,
    /// Commands that enter configuration mode. Empty = the platform has no config mode.
    pub enter: Vec<String>,
    pub exit: Vec<String>,
    /// Commands that activate a candidate configuration (Junos `commit`).
    pub commit: Vec<String>,
    /// Commands that discard a candidate configuration after a failure.
    pub abort: Vec<String>,
    /// Commands that persist the running configuration.
    pub save: Vec<String>,
    /// Command that prints the running configuration.
    pub running: String,
    pub startup: Option<String>,
    /// Lines to drop from a fetched configuration (timestamps, byte counts)
    /// so that two fetches of an unchanged device compare equal.
    pub ignore: Vec<String>,
    /// Output patterns that mean a configuration line was rejected.
    pub errors: Vec<String>,
    pub answers: Vec<Answer>,
    pub guard: Option<Guard>,
    /// Extra secret patterns for redaction (named group `secret`).
    pub secrets: Vec<String>,
}

impl Default for ConfigSpec {
    fn default() -> Self {
        ConfigSpec {
            syntax: Syntax::Flat,
            enter: Vec::new(),
            exit: Vec::new(),
            commit: Vec::new(),
            abort: Vec::new(),
            save: Vec::new(),
            running: String::new(),
            startup: None,
            ignore: Vec::new(),
            errors: Vec::new(),
            answers: Vec::new(),
            guard: None,
            secrets: Vec::new(),
        }
    }
}

/// A dead-man switch: something the device will do by itself unless told
/// not to, so a change that cuts management access undoes itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Guard {
    /// Human name of the mechanism ("reload timer", "commit confirmed").
    pub name: String,
    /// Exec commands that arm it; `{minutes}` is substituted.
    pub arm: Vec<String>,
    /// Exec commands that cancel it once the change is verified.
    pub disarm: Vec<String>,
    /// What happens if it fires, in words shown to the user.
    pub effect: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicySpec {
    pub read_only: Vec<String>,
    pub dangerous: Vec<String>,
}

/// One command to run and how to turn its output into model rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectStep {
    pub command: String,
    pub parser: Parser,
    /// Model field → expression over the parsed row.
    pub map: IndexMap<String, String>,
    /// Keep a row only when this expression is truthy.
    pub when: Option<String>,
    /// A failing optional step is skipped instead of failing the collection.
    pub optional: bool,
    /// Merge this step's rows into earlier rows that have the same value in this model field.
    pub merge_on: Option<String>,
}

/// Written in YAML as a bare word (`kv`, `colon`, `raw`) or a one-key map
/// (`{ntc: cisco_ios_show_version}`, `{regex: '…'}`, `{json: interfaces.*}`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Parser {
    /// A bundled ntc-templates template by name, or `auto` to resolve the command through the index.
    Ntc(String),
    /// A TextFSM template written inline.
    Textfsm(String),
    /// A regular expression; each match is a row of its named groups.
    Regex(String),
    /// RouterOS-style `key=value` listing.
    Kv,
    /// `key: value` property sheet → one row.
    Colon,
    /// JSON output; the value is the path to the rows (`interfaces.*`).
    Json(String),
    /// The whole output as one row with a single `output` field.
    #[default]
    Raw,
}

impl Serialize for Parser {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let (key, value) = match self {
            Parser::Kv => return serializer.serialize_str("kv"),
            Parser::Colon => return serializer.serialize_str("colon"),
            Parser::Raw => return serializer.serialize_str("raw"),
            Parser::Ntc(name) => ("ntc", name),
            Parser::Textfsm(template) => ("textfsm", template),
            Parser::Regex(pattern) => ("regex", pattern),
            Parser::Json(path) => ("json", path),
        };
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(key, value)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Parser {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Word(String),
            Map(IndexMap<String, String>),
        }
        const HELP: &str = "expected `kv`, `colon`, `raw`, or a one-key map with `ntc`, `textfsm`, `regex` or `json`";
        match Written::deserialize(deserializer)? {
            Written::Word(word) => match word.as_str() {
                "kv" => Ok(Parser::Kv),
                "colon" => Ok(Parser::Colon),
                "raw" => Ok(Parser::Raw),
                "json" => Ok(Parser::Json(String::new())),
                other => Err(D::Error::custom(format!(
                    "unknown parser `{other}`: {HELP}"
                ))),
            },
            Written::Map(map) => {
                let mut entries = map.into_iter();
                match (entries.next(), entries.next()) {
                    (Some((key, value)), None) => match key.as_str() {
                        "ntc" => Ok(Parser::Ntc(value)),
                        "textfsm" => Ok(Parser::Textfsm(value)),
                        "regex" => Ok(Parser::Regex(value)),
                        "json" => Ok(Parser::Json(value)),
                        other => Err(D::Error::custom(format!(
                            "unknown parser `{other}`: {HELP}"
                        ))),
                    },
                    _ => Err(D::Error::custom(format!(
                        "a parser takes exactly one key: {HELP}"
                    ))),
                }
            }
        }
    }
}

/// Where to find health data over SNMP. Each entry is tried in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SnmpSpec {
    /// OIDs (scalars or columns) holding a CPU percentage.
    pub cpu: Vec<String>,
    pub memory_used: Vec<String>,
    pub memory_free: Vec<String>,
    pub memory_total: Vec<String>,
    /// OIDs holding a temperature in °C.
    pub temperature: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Builtin,
    User,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
#[error("profile `{profile}`: {}", .problems.join("; "))]
pub struct ProfileError {
    pub profile: String,
    pub problems: Vec<String>,
}

/// A profile with everything compiled and checked.
#[allow(dead_code)] // some compiled fields are only read by the session driver
pub struct Platform {
    pub profile: Profile,
    pub origin: Origin,
    /// The YAML this platform was loaded from (before inheritance).
    pub source: String,
    pub policy: Policy,
    pub(crate) prompt: Regex,
    pub(crate) user_prompt: Option<Regex>,
    pub(crate) config_prompt: Option<Regex>,
    pub(crate) enable_password: Option<Regex>,
    pub(crate) username_prompt: Regex,
    pub(crate) password_prompt: Regex,
    pub(crate) login_failure: Regex,
    pub(crate) pager: Option<Regex>,
    pub(crate) exec_errors: Vec<Regex>,
    pub(crate) config_errors: Vec<Regex>,
    pub(crate) exec_answers: Vec<(Regex, String)>,
    pub(crate) config_answers: Vec<(Regex, String)>,
    pub(crate) config_ignore: Vec<Regex>,
    banner: Vec<Regex>,
    sysdescr: Vec<Regex>,
}

impl std::fmt::Debug for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Platform")
            .field("id", &self.profile.id)
            .field("origin", &self.origin)
            .finish()
    }
}

impl Platform {
    pub fn id(&self) -> &str {
        &self.profile.id
    }

    pub fn is_abstract(&self) -> bool {
        self.profile.id.starts_with('_')
    }

    /// Whether a prompt line belongs to configuration mode.
    pub fn is_config_prompt(&self, line: &str) -> bool {
        self.config_prompt
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(line))
    }

    pub fn is_user_prompt(&self, line: &str) -> bool {
        !self.is_config_prompt(line)
            && self
                .user_prompt
                .as_ref()
                .is_some_and(|pattern| pattern.is_match(line))
    }

    /// The first line of `output` that matches one of the platform's error patterns.
    pub fn find_error<'a>(&self, output: &'a str, config_mode: bool) -> Option<&'a str> {
        let patterns = if config_mode && !self.config_errors.is_empty() {
            &self.config_errors
        } else {
            &self.exec_errors
        };
        output
            .lines()
            .find(|line| patterns.iter().any(|pattern| pattern.is_match(line)))
            .map(str::trim)
    }

    /// Drop volatile lines from a fetched configuration.
    pub fn clean_config(&self, text: &str) -> String {
        let mut cleaned = String::with_capacity(text.len());
        for line in text.lines() {
            if self
                .config_ignore
                .iter()
                .any(|pattern| pattern.is_match(line))
            {
                continue;
            }
            cleaned.push_str(line.trim_end());
            cleaned.push('\n');
        }
        // Leading/trailing blank lines are display artefacts.
        cleaned.trim_matches('\n').to_string() + "\n"
    }

    fn compile(
        profile: Profile,
        origin: Origin,
        source: String,
        expressions: &Expressions,
    ) -> Result<Platform, ProfileError> {
        let mut problems: Vec<String> = Vec::new();
        let fallback = || Regex::new("$^").expect("never-matching pattern");
        let cli = &profile.cli;

        let prompt = compile_one(&mut problems, "cli.prompt", &cli.prompt);
        let user_prompt = cli
            .user_prompt
            .as_deref()
            .and_then(|pattern| compile_one(&mut problems, "cli.user_prompt", pattern));
        let config_prompt = cli
            .config_prompt
            .as_deref()
            .and_then(|pattern| compile_one(&mut problems, "cli.config_prompt", pattern));
        let enable_password = cli.enable.as_ref().and_then(|enable| {
            compile_one(
                &mut problems,
                "cli.enable.password_prompt",
                &enable.password_prompt,
            )
        });
        let username_prompt = compile_one(
            &mut problems,
            "cli.login.username_prompt",
            &cli.login.username_prompt,
        );
        let password_prompt = compile_one(
            &mut problems,
            "cli.login.password_prompt",
            &cli.login.password_prompt,
        );
        let login_failure = compile_one(&mut problems, "cli.login.failure", &cli.login.failure);
        let pager = cli
            .pager
            .as_ref()
            .and_then(|pager| compile_one(&mut problems, "cli.pager.pattern", &pager.pattern));
        let exec_errors = compile_list(&mut problems, "cli.errors", &cli.errors);
        let config_errors = compile_list(&mut problems, "config.errors", &profile.config.errors);
        let config_ignore = compile_list(&mut problems, "config.ignore", &profile.config.ignore);
        let banner = compile_list(&mut problems, "detect.banner", &profile.detect.banner);
        let sysdescr = compile_list(&mut problems, "detect.sysdescr", &profile.detect.sysdescr);
        let exec_answers = compile_answers(&mut problems, "cli.answers", &cli.answers);
        let config_answers =
            compile_answers(&mut problems, "config.answers", &profile.config.answers);
        for (index, probe) in profile.detect.probes.iter().enumerate() {
            compile_one(
                &mut problems,
                &format!("detect.probes[{index}].matches"),
                &probe.matches,
            );
        }
        for (index, pattern) in profile.config.secrets.iter().enumerate() {
            compile_one(&mut problems, &format!("config.secrets[{index}]"), pattern);
        }

        let policy = match Policy::new(&profile.policy.read_only, &profile.policy.dangerous) {
            Ok(policy) => policy,
            Err(err) => {
                problems.push(format!(
                    "policy: invalid regular expression ({})",
                    first_line(&err.to_string())
                ));
                Policy::generic()
            }
        };

        if profile.id.is_empty() {
            problems.push("id: a profile needs an id".into());
        } else if !profile
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            problems.push("id: use only letters, digits, `_` and `-`".into());
        }
        if profile.cli.newline.is_empty() {
            problems.push("cli.newline: must not be empty".into());
        }
        if !profile.config.enter.is_empty() && profile.cli.config_prompt.is_none() {
            problems.push("cli.config_prompt: required when config.enter is set, so the driver can tell it is in configuration mode".into());
        }

        for (kind, steps) in &profile.collect {
            for (index, step) in steps.iter().enumerate() {
                let at = format!("collect.{}[{index}]", kind.as_str());
                if step.command.trim().is_empty() {
                    problems.push(format!("{at}.command: must not be empty"));
                }
                if let Parser::Regex(pattern) = &step.parser {
                    compile_one(&mut problems, &format!("{at}.parser.regex"), pattern);
                }
                if step.map.is_empty() {
                    problems.push(format!("{at}.map: at least one field mapping is required"));
                }
                for (field, expression) in &step.map {
                    if let Err(err) = expressions.check(expression) {
                        problems.push(format!("{at}.map.{field}: {err}"));
                    }
                }
                if let Some(condition) = &step.when
                    && let Err(err) = expressions.check(condition)
                {
                    problems.push(format!("{at}.when: {err}"));
                }
                if let Some(key) = &step.merge_on
                    && !step.map.contains_key(key)
                {
                    problems.push(format!(
                        "{at}.merge_on: `{key}` is not one of this step's mapped fields"
                    ));
                }
            }
        }

        if !problems.is_empty() {
            return Err(ProfileError {
                profile: profile.id.clone(),
                problems,
            });
        }
        Ok(Platform {
            policy,
            prompt: prompt.unwrap_or_else(fallback),
            user_prompt,
            config_prompt,
            enable_password,
            username_prompt: username_prompt.unwrap_or_else(fallback),
            password_prompt: password_prompt.unwrap_or_else(fallback),
            login_failure: login_failure.unwrap_or_else(fallback),
            pager,
            exec_errors,
            config_errors,
            exec_answers,
            config_answers,
            config_ignore,
            banner,
            sysdescr,
            profile,
            origin,
            source,
        })
    }
}

fn compile_one(problems: &mut Vec<String>, field: &str, pattern: &str) -> Option<Regex> {
    match Regex::new(pattern) {
        Ok(compiled) => Some(compiled),
        Err(err) => {
            problems.push(format!(
                "{field}: invalid regular expression `{pattern}` ({})",
                first_line(&err.to_string())
            ));
            None
        }
    }
}

fn compile_list(problems: &mut Vec<String>, field: &str, patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .enumerate()
        .filter_map(|(index, pattern)| compile_one(problems, &format!("{field}[{index}]"), pattern))
        .collect()
}

fn compile_answers(
    problems: &mut Vec<String>,
    field: &str,
    answers: &[Answer],
) -> Vec<(Regex, String)> {
    answers
        .iter()
        .enumerate()
        .filter_map(|(index, answer)| {
            compile_one(
                problems,
                &format!("{field}[{index}].prompt"),
                &answer.prompt,
            )
            .map(|pattern| (pattern, answer.answer.clone()))
        })
        .collect()
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or(text)
}

/// For listing profiles in the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub os: String,
    pub description: String,
    pub origin: Origin,
    pub extends: Option<String>,
    /// What the profile can collect.
    pub collects: Vec<Kind>,
    pub can_configure: bool,
    pub syntax: Syntax,
}

/// All known platforms.
pub struct Registry {
    platforms: IndexMap<String, Arc<Platform>>,
    /// Fully merged YAML per profile, the base for children.
    merged: IndexMap<String, Yaml>,
    expressions: Arc<Expressions>,
}

impl Registry {
    pub fn empty() -> Registry {
        Registry {
            platforms: IndexMap::new(),
            merged: IndexMap::new(),
            expressions: Arc::new(Expressions::new()),
        }
    }

    /// The profiles shipped with Nocturne. Problems in a bundled profile are
    /// returned rather than hidden, and are a test failure in this crate.
    pub fn builtin() -> (Registry, Vec<ProfileError>) {
        let mut registry = Registry::empty();
        let mut pending: Vec<(String, String)> = BUILTIN
            .files()
            .filter(|file| {
                file.path()
                    .extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .filter_map(|file| {
                Some((
                    file.path().display().to_string(),
                    file.contents_utf8()?.to_string(),
                ))
            })
            .collect();
        pending.sort();

        // Parents must load before children; keep sweeping until nothing more loads.
        let mut errors = Vec::new();
        loop {
            let before = pending.len();
            let mut waiting = Vec::new();
            for (path, yaml) in pending {
                match registry.add(&yaml, Origin::Builtin) {
                    Ok(_) => {}
                    Err(err)
                        if err
                            .problems
                            .iter()
                            .any(|problem| problem.starts_with("extends:")) =>
                    {
                        waiting.push((path, yaml))
                    }
                    Err(mut err) => {
                        if err.profile.is_empty() {
                            err.profile = path;
                        }
                        errors.push(err);
                    }
                }
            }
            pending = waiting;
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        for (path, yaml) in pending {
            if let Err(mut err) = registry.add(&yaml, Origin::Builtin) {
                if err.profile.is_empty() {
                    err.profile = path;
                }
                errors.push(err);
            }
        }
        (registry, errors)
    }

    /// Load every `*.yaml` in a directory as user profiles (they may replace
    /// built-ins of the same id). Files with problems are reported and skipped.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<(PathBuf, ProfileError)> {
        let mut failures = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return failures;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .collect();
        paths.sort();
        for path in paths {
            match std::fs::read_to_string(&path) {
                Ok(yaml) => {
                    if let Err(err) = self.add(&yaml, Origin::User) {
                        failures.push((path, err));
                    }
                }
                Err(err) => failures.push((
                    path.clone(),
                    ProfileError {
                        profile: path.display().to_string(),
                        problems: vec![format!("could not read the file: {err}")],
                    },
                )),
            }
        }
        failures
    }

    /// Parse, merge with its parent, validate and register one profile.
    pub fn add(&mut self, yaml: &str, origin: Origin) -> Result<Arc<Platform>, ProfileError> {
        let fail = |profile: &str, problem: String| ProfileError {
            profile: profile.to_string(),
            problems: vec![problem],
        };
        let document: Yaml = serde_yaml_ng::from_str(yaml)
            .map_err(|err| fail("", format!("not valid YAML: {err}")))?;
        if !document.is_mapping() {
            return Err(fail("", "a profile must be a YAML mapping".into()));
        }
        let id = document
            .get("id")
            .and_then(Yaml::as_str)
            .unwrap_or_default()
            .to_string();

        let merged = match document.get("extends").and_then(Yaml::as_str) {
            Some(parent) if parent == id => {
                return Err(fail(&id, "extends: a profile cannot extend itself".into()));
            }
            Some(parent) => {
                let base = self.merged.get(parent).ok_or_else(|| {
                    fail(&id, format!("extends: unknown parent profile `{parent}`"))
                })?;
                merge(base.clone(), document)
            }
            None => document,
        };

        let profile: Profile =
            serde_yaml_ng::from_value(merged.clone()).map_err(|err| fail(&id, err.to_string()))?;
        let platform = Arc::new(Platform::compile(
            profile,
            origin,
            yaml.to_string(),
            &self.expressions,
        )?);
        self.merged.insert(id.clone(), merged);
        self.platforms.insert(id, platform.clone());
        Ok(platform)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.merged.shift_remove(id);
        self.platforms.shift_remove(id).is_some()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Platform>> {
        self.platforms.get(id).cloned()
    }

    pub fn expressions(&self) -> Arc<Expressions> {
        self.expressions.clone()
    }

    /// Selectable (non-abstract) profiles, sorted by vendor then name.
    pub fn list(&self) -> Vec<ProfileSummary> {
        let mut list: Vec<ProfileSummary> = self
            .platforms
            .values()
            .filter(|platform| !platform.is_abstract())
            .map(|platform| {
                let profile = &platform.profile;
                ProfileSummary {
                    id: profile.id.clone(),
                    name: profile.name.clone(),
                    vendor: profile.vendor.clone(),
                    os: profile.os.clone(),
                    description: profile.description.clone(),
                    origin: platform.origin,
                    extends: profile.extends.clone(),
                    collects: profile
                        .collect
                        .iter()
                        .filter(|(_, steps)| !steps.is_empty())
                        .map(|(kind, _)| *kind)
                        .collect(),
                    can_configure: !profile.config.enter.is_empty()
                        || profile.config.syntax == Syntax::Routeros,
                    syntax: profile.config.syntax,
                }
            })
            .collect();
        list.sort_by(|a, b| {
            (a.vendor.to_lowercase(), a.name.to_lowercase())
                .cmp(&(b.vendor.to_lowercase(), b.name.to_lowercase()))
        });
        list
    }

    /// Best match for text seen before or right after login.
    pub fn detect_banner(&self, text: &str) -> Option<Arc<Platform>> {
        self.best(|platform| platform.banner.iter().any(|pattern| pattern.is_match(text)))
    }

    /// Best match for SNMP system information.
    pub fn detect_snmp(&self, sysdescr: &str, sysobjectid: &str) -> Option<Arc<Platform>> {
        let object_id = sysobjectid.trim_start_matches('.');
        self.best(|platform| {
            platform
                .sysdescr
                .iter()
                .any(|pattern| pattern.is_match(sysdescr))
                || (!object_id.is_empty()
                    && platform
                        .profile
                        .detect
                        .sysobjectid
                        .iter()
                        .any(|prefix| object_id.starts_with(prefix.trim_start_matches('.'))))
        })
    }

    /// Probe commands to try on an unidentified session, most specific first.
    pub fn probes(&self) -> Vec<(Arc<Platform>, Probe)> {
        let mut probes: Vec<(Arc<Platform>, Probe)> = self
            .platforms
            .values()
            .filter(|platform| !platform.is_abstract())
            .flat_map(|platform| {
                platform
                    .profile
                    .detect
                    .probes
                    .iter()
                    .map(|probe| (platform.clone(), probe.clone()))
            })
            .collect();
        probes.sort_by_key(|(platform, _)| std::cmp::Reverse(platform.profile.detect.priority));
        probes
    }

    fn best(&self, matches: impl Fn(&Platform) -> bool) -> Option<Arc<Platform>> {
        self.platforms
            .values()
            .filter(|platform| !platform.is_abstract() && matches(platform))
            .max_by_key(|platform| platform.profile.detect.priority)
            .cloned()
    }
}

/// Merge `child` over `base`: mappings merge key by key, everything else is replaced.
fn merge(base: Yaml, child: Yaml) -> Yaml {
    match (base, child) {
        (Yaml::Mapping(mut base), Yaml::Mapping(child)) => {
            for (key, value) in child {
                let merged = match base.remove(&key) {
                    Some(existing) => merge(existing, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            Yaml::Mapping(base)
        }
        (_, child) => child,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
id: _switch_base
name: Base
cli:
  prompt: '^(?P<host>[\w.-]+)(?:\(config[^)]*\))?[>#]\s?$'
  user_prompt: '>\s?$'
  config_prompt: '\(config[^)]*\)#\s?$'
  init: ["terminal length 0"]
  errors: ['^% ']
config:
  syntax: ios
  enter: [configure terminal]
  exit: [end]
  save: [write memory]
  running: show running-config
  ignore: ['^Building configuration', '^Current configuration']
policy:
  read_only: ['^show\b']
  dangerous: ['^reload\b']
collect:
  vlans:
    - command: show vlan brief
      parser: {regex: '(?m)^(?P<ID>\d+)\s+(?P<NAME>\S+)\s+(?P<STATUS>\S+)'}
      map: {id: ID, name: NAME, status: STATUS}
"#;

    const CHILD: &str = r#"
id: acme_os
name: Acme OS
vendor: Acme
os: AcmeOS
extends: _switch_base
detect:
  banner: ['(?i)acme networks']
  sysobjectid: ['1.3.6.1.4.1.99999.']
  priority: 10
cli:
  init: ["no page"]
config:
  save: [copy running-config startup-config]
collect:
  facts:
    - command: show system
      parser: colon
      map: {hostname: name, os_version: "version | regex_search('([\\d.]+)')"}
"#;

    #[test]
    fn child_profiles_merge_over_their_parent() {
        let mut registry = Registry::empty();
        registry.add(BASE, Origin::Builtin).unwrap();
        let platform = registry.add(CHILD, Origin::User).unwrap();
        let profile = &platform.profile;
        assert_eq!(profile.id, "acme_os");
        assert_eq!(profile.cli.init, vec!["no page"], "lists replace");
        assert_eq!(
            profile.cli.errors,
            vec!["^% "],
            "untouched keys are inherited"
        );
        assert_eq!(profile.config.enter, vec!["configure terminal"]);
        assert_eq!(
            profile.config.save,
            vec!["copy running-config startup-config"]
        );
        assert_eq!(profile.config.syntax, Syntax::Ios);
        assert!(
            profile.collect.contains_key(&Kind::Vlans)
                && profile.collect.contains_key(&Kind::Facts)
        );
        assert_eq!(profile.collect[&Kind::Facts][0].parser, Parser::Colon);

        assert!(platform.policy.is_read_only("show vlan"));
        assert!(platform.is_config_prompt("sw1(config-if)#"));
        assert!(platform.is_user_prompt("sw1>"));
        assert!(!platform.is_user_prompt("sw1#"));
        assert_eq!(
            platform.find_error("vlan 5000\n% Invalid input", false),
            Some("% Invalid input")
        );
        assert_eq!(
            platform.clean_config("Building configuration...\n\nhostname sw1  \n!\n"),
            "hostname sw1\n!\n"
        );

        let listed = registry.list();
        assert_eq!(listed.len(), 1, "abstract bases are not selectable");
        assert_eq!(listed[0].collects, vec![Kind::Vlans, Kind::Facts]);
        assert!(listed[0].can_configure);
    }

    #[test]
    fn detection_prefers_the_highest_priority_match() {
        let mut registry = Registry::empty();
        registry.add(BASE, Origin::Builtin).unwrap();
        registry.add(CHILD, Origin::User).unwrap();
        registry
            .add("id: generic\nname: Generic\ndetect: {banner: ['.'], sysdescr: ['.'], priority: -100}\n", Origin::Builtin)
            .unwrap();
        assert_eq!(
            registry
                .detect_banner("Welcome to ACME Networks OS")
                .unwrap()
                .id(),
            "acme_os"
        );
        assert_eq!(
            registry.detect_banner("something else").unwrap().id(),
            "generic"
        );
        assert_eq!(
            registry
                .detect_snmp("whatever", ".1.3.6.1.4.1.99999.1.2")
                .unwrap()
                .id(),
            "acme_os"
        );
    }

    #[test]
    fn problems_are_reported_with_their_location() {
        let mut registry = Registry::empty();
        let unknown_key = registry
            .add("id: x\nname: X\ncli:\n  promt: 'oops'\n", Origin::User)
            .unwrap_err();
        assert!(
            unknown_key.to_string().contains("unknown field `promt`"),
            "{unknown_key}"
        );

        let bad = r#"
id: bad one
cli:
  prompt: '('
config:
  enter: [configure]
collect:
  interfaces:
    - command: ""
      parser: {regex: '['}
      map: {name: "NAME |"}
      when: "(("
      merge_on: missing
"#;
        let err = registry.add(bad, Origin::User).unwrap_err();
        let text = err.to_string();
        for expected in [
            "cli.prompt: invalid regular expression",
            "id: use only letters",
            "cli.config_prompt: required",
            "collect.interfaces[0].command",
            "collect.interfaces[0].parser.regex",
            "collect.interfaces[0].map.name",
            "collect.interfaces[0].when",
            "collect.interfaces[0].merge_on",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in: {text}");
        }
        assert!(
            registry.get("bad one").is_none(),
            "a broken profile is not registered"
        );

        let orphan = registry
            .add("id: child\nextends: nowhere\n", Origin::User)
            .unwrap_err();
        assert!(
            orphan
                .to_string()
                .contains("unknown parent profile `nowhere`")
        );
        assert!(registry.add("- not\n- a mapping\n", Origin::User).is_err());
        assert!(registry.add("id: me\nextends: me\n", Origin::User).is_err());
        let bad_parser = registry
            .add("id: p\ncollect:\n  facts:\n    - {command: x, parser: {xml: y}, map: {hostname: a}}\n", Origin::User)
            .unwrap_err();
        assert!(
            bad_parser.to_string().contains("unknown parser `xml`"),
            "{bad_parser}"
        );
    }

    #[test]
    fn user_profiles_load_from_a_directory_and_override() {
        let dir = std::env::temp_dir().join(format!("nocturne-profiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a_base.yaml"), BASE).unwrap();
        std::fs::write(dir.join("b_child.yaml"), CHILD).unwrap();
        std::fs::write(
            dir.join("c_broken.yaml"),
            "id: broken\ncli: {prompt: '('}\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let mut registry = Registry::empty();
        let failures = registry.load_dir(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].0.ends_with("c_broken.yaml"));
        assert_eq!(registry.get("acme_os").unwrap().origin, Origin::User);
        assert!(registry.remove("acme_os") && registry.get("acme_os").is_none());
    }

    #[test]
    fn bundled_profiles_all_load_cleanly() {
        let (registry, errors) = Registry::builtin();
        assert!(errors.is_empty(), "bundled profile problems: {errors:#?}");
        for platform in registry.platforms.values() {
            assert_eq!(platform.origin, Origin::Builtin);
        }
    }
}
