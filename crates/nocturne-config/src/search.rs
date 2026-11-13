//! Searching the latest configuration of every device.
//!
//! Search always runs over redacted text: a secret can neither be found nor
//! confirmed by searching for it. Redacting a configuration is far more work
//! than scanning it, so a device is first scanned as stored and only redacted
//! when something matched; what is reported are the matches in the redacted
//! text.

use nocturne_app::inventory::DeviceFilter;
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_drivers::hier::Syntax;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::outline::{self, Outline};
use crate::redact::{self, Redaction};
use crate::scope::{self, Target};
use crate::versions::{self, Kind};

const SECRET_NOTE: &str = "The query looks like it contains a secret value. Configurations are searched with \
    their secrets hidden, so a secret value never matches; search for the line without the value.";

#[derive(Debug, Serialize)]
pub struct Match {
    pub device_id: String,
    pub name: String,
    pub lab: bool,
    pub version_id: i64,
    /// Line number (from 1) in the version's text.
    pub line_no: usize,
    /// The sections the line sits in, outermost first; empty at the top level.
    pub section: Vec<String>,
    pub line: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct SearchResult {
    pub matches: Vec<Match>,
    /// Devices whose latest configuration was looked at. Fewer than there are
    /// when the search stopped at `limit` (see `truncated`).
    pub devices_searched: usize,
    pub devices_matched: usize,
    /// Devices in scope with no stored configuration of this kind: nothing is known about them.
    pub devices_without_backup: usize,
    /// There are more matches than `limit`; the search stopped there.
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What a device's configuration is tested against.
enum Query {
    /// Some line matches the pattern.
    Pattern(Regex),
    /// Some line is exactly this (spacing and, unless asked, case aside).
    Line { text: String, case: bool },
}

fn pattern(query: &str, regex: bool, case: bool) -> AppResult<Regex> {
    if query.trim().is_empty() {
        return Err(AppError::invalid("`query` is empty: there is nothing to search for"));
    }
    let source = if regex {
        query.to_string()
    } else {
        regex::escape(query.trim())
    };
    RegexBuilder::new(&source)
        .case_insensitive(!case)
        .build()
        .map_err(|err| AppError::invalid(format!("`{query}` is not a valid regular expression: {err}")))
}

impl Query {
    fn matches_line(&self, line: &str) -> bool {
        match self {
            Query::Pattern(pattern) => pattern.is_match(line),
            Query::Line { text, case } => {
                let line = outline::collapse(line);
                if *case {
                    line == *text
                } else {
                    line.to_lowercase() == *text
                }
            }
        }
    }

    /// Whether a configuration contains the query. On RouterOS a line also
    /// counts with its menu in front, however the export was printed.
    fn found_in(&self, text: &str, syntax: Syntax) -> bool {
        if text.lines().any(|line| self.matches_line(line)) {
            return true;
        }
        syntax == Syntax::Routeros && {
            let outline = Outline::parse(text, syntax);
            (0..outline.lines.len())
                .filter_map(|index| outline.statement(index))
                .any(|statement| self.matches_line(&statement))
        }
    }
}

fn scan(
    app: &App,
    targets: &[Target],
    pattern: &Regex,
    kind: Kind,
    context: usize,
    limit: usize,
) -> AppResult<SearchResult> {
    let ids: Vec<&str> = targets.iter().map(|target| target.device.id.as_str()).collect();
    let latest = versions::latest_of_each(app, &ids, kind)?;
    let redaction = Redaction::for_profiles(app, targets.iter().map(|target| target.profile.as_str()));
    let mut result = SearchResult::default();

    'devices: for target in targets {
        let Some(version) = latest.get(&target.device.id) else {
            result.devices_without_backup += 1;
            continue;
        };
        let (_, stored) = app.store().config_get(version.id)?;
        result.devices_searched += 1;
        if !stored.lines().any(|line| pattern.is_match(line)) {
            continue;
        }
        let text = redaction.text(&stored);
        let lines: Vec<&str> = text.lines().collect();
        let mut outline: Option<Outline> = None;
        let mut matched = false;
        for (index, line) in lines.iter().enumerate() {
            if !pattern.is_match(line) {
                continue;
            }
            if result.matches.len() == limit {
                result.truncated = true;
                result.devices_matched += usize::from(matched);
                break 'devices;
            }
            matched = true;
            let outline = outline.get_or_insert_with(|| Outline::parse(&text, target.syntax(app)));
            let lines_of = |range: std::ops::Range<usize>| -> Vec<String> {
                lines[range].iter().map(|line| line.to_string()).collect()
            };
            result.matches.push(Match {
                device_id: target.device.id.clone(),
                name: target.device.name.clone(),
                lab: target.device.lab.is_some(),
                version_id: version.id,
                line_no: index + 1,
                section: outline.enclosing(index + 1),
                line: line.to_string(),
                before: lines_of(index.saturating_sub(context)..index),
                after: lines_of(index + 1..(index + 1 + context).min(lines.len())),
            });
        }
        result.devices_matched += usize::from(matched);
    }
    Ok(result)
}

/// A device that does not have what was asked for.
#[derive(Debug, Serialize)]
pub struct DeviceRef {
    pub device_id: String,
    pub name: String,
    pub lab: bool,
    /// The version that was looked at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_id: Option<i64>,
    /// When that version was last fetched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fetched_at: Option<i64>,
}

#[derive(Debug, Default, Serialize)]
pub struct MissingResult {
    /// Devices whose latest configuration does not contain the line or pattern.
    pub devices: Vec<DeviceRef>,
    pub devices_searched: usize,
    /// Devices with no stored configuration: whether they have the line is unknown.
    pub without_backup: Vec<DeviceRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn lacking(app: &App, targets: &[Target], query: &Query, kind: Kind) -> AppResult<MissingResult> {
    let ids: Vec<&str> = targets.iter().map(|target| target.device.id.as_str()).collect();
    let latest = versions::latest_of_each(app, &ids, kind)?;
    let redaction = Redaction::for_profiles(app, targets.iter().map(|target| target.profile.as_str()));
    let mut result = MissingResult::default();
    for target in targets {
        let version = latest.get(&target.device.id);
        let device = DeviceRef {
            device_id: target.device.id.clone(),
            name: target.device.name.clone(),
            lab: target.device.lab.is_some(),
            version_id: version.map(|version| version.id),
            fetched_at: version.map(|version| version.last_seen),
        };
        let Some(version) = version else {
            result.without_backup.push(device);
            continue;
        };
        let (_, stored) = app.store().config_get(version.id)?;
        result.devices_searched += 1;
        let syntax = target.syntax(app);
        // As in search, only what is still there once secrets are hidden counts.
        let found = query.found_in(&stored, syntax) && query.found_in(&redaction.text(&stored), syntax);
        if !found {
            result.devices.push(device);
        }
    }
    Ok(result)
}

#[derive(Deserialize)]
struct SearchParams {
    query: String,
    regex: Option<bool>,
    case: Option<bool>,
    filter: Option<DeviceFilter>,
    device_ids: Option<Vec<String>>,
    kind: Option<Kind>,
    context: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct MissingParams {
    query: String,
    regex: Option<bool>,
    case: Option<bool>,
    filter: Option<DeviceFilter>,
    device_ids: Option<Vec<String>>,
    kind: Option<Kind>,
}

/// Run a scan off the async runtime: it reads and inflates every device's configuration.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> AppResult<T> + Send + 'static) -> AppResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| AppError::internal(format!("the search stopped unexpectedly: {err}")))?
}

pub(crate) fn register(router: &mut Router) {
    router.add("config.search", |app: App, params: SearchParams| async move {
        let regex = params.regex.unwrap_or(false);
        let pattern = pattern(&params.query, regex, params.case.unwrap_or(false))?;
        let targets = scope::targets(&app, params.device_ids.as_deref(), params.filter.as_ref())?;
        let kind = params.kind.unwrap_or_default();
        let context = params.context.unwrap_or(1).min(20);
        let limit = params.limit.unwrap_or(500).clamp(1, 5000);
        let mut result = blocking(move || scan(&app, &targets, &pattern, kind, context, limit)).await?;
        if result.matches.is_empty() && !regex && redact::has_secret(&params.query) {
            result.note = Some(SECRET_NOTE.to_string());
        }
        Ok(result)
    });

    router.add("config.missing", |app: App, params: MissingParams| async move {
        let regex = params.regex.unwrap_or(false);
        let case = params.case.unwrap_or(false);
        let query = if regex {
            Query::Pattern(pattern(&params.query, true, case)?)
        } else {
            let text = outline::collapse(params.query.trim());
            if text.is_empty() || text.contains('\n') {
                return Err(AppError::invalid(
                    "`query` must be one configuration line (or a pattern with `regex: true`)",
                ));
            }
            Query::Line {
                text: if case { text } else { text.to_lowercase() },
                case,
            }
        };
        let targets = scope::targets(&app, params.device_ids.as_deref(), params.filter.as_ref())?;
        let kind = params.kind.unwrap_or_default();
        let mut result = blocking(move || lacking(&app, &targets, &query, kind)).await?;
        if !regex && redact::has_secret(&params.query) {
            result.note = Some(SECRET_NOTE.to_string());
        }
        Ok(result)
    });
}
