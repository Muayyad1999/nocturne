//! The profile test harness: walks `tests/profiles/<profile id>/` and checks
//! each kind of test file against the profile it belongs to.
//!
//! Every failure names the profile, the file and the entry or field, so that
//! whoever feeds the harness can go straight to the line to fix.

mod captures;
mod cases;
mod checks;
mod scripted;

use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nocturne_drivers::model::Kind;
use nocturne_drivers::profile::{Maturity, Platform, Registry};
use serde::de::DeserializeOwned;

/// What the harness found.
pub struct Report {
    pub failures: Vec<String>,
    pub profiles: usize,
    lines: Vec<String>,
}

impl Report {
    /// One line per profile: how much was checked.
    pub fn summary(&self) -> String {
        format!(
            "profile tests ({} profile(s)):\n{}",
            self.profiles,
            self.lines.join("\n")
        )
    }
}

/// How much of one profile was checked, for the summary.
#[derive(Default)]
pub struct Tally {
    pub prompts: usize,
    pub errors: usize,
    pub commands: usize,
    pub detections: usize,
    pub captures: usize,
    pub rows: usize,
    pub configs: usize,
    pub sessions: usize,
}

/// One profile under test, and where its problems are collected.
pub struct Subject<'a> {
    pub registry: &'a Registry,
    pub platform: Arc<Platform>,
    pub dir: PathBuf,
    pub failures: Vec<String>,
    pub tally: Tally,
}

impl Subject<'_> {
    pub fn id(&self) -> &str {
        self.platform.id()
    }

    /// Record a problem in `file` (a path relative to the profile's test directory).
    pub fn fail(&mut self, file: &str, message: impl Display) {
        let id = self.id().to_string();
        self.failures.push(format!("[{id}] {file}: {message}"));
    }

    /// Read a YAML test file. A missing file is `None`; a file that does not
    /// parse is a failure that says where.
    pub fn load<T: DeserializeOwned>(&mut self, file: &str) -> Option<T> {
        let text = std::fs::read_to_string(self.dir.join(file)).ok()?;
        match serde_yaml_ng::from_str(&text) {
            Ok(parsed) => Some(parsed),
            Err(err) => {
                self.fail(file, format!("cannot be read: {err}"));
                None
            }
        }
    }

    pub fn exists(&self, file: &str) -> bool {
        self.dir.join(file).exists()
    }
}

/// Files and directories directly inside `dir`, sorted by name.
pub fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

pub fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub async fn run(registry: &Registry, root: &Path, only: Option<&str>) -> Report {
    let mut report = Report {
        failures: Vec::new(),
        profiles: 0,
        lines: Vec::new(),
    };

    let tested: BTreeSet<String> = entries(root)
        .iter()
        .filter(|path| path.is_dir())
        .map(|path| name_of(path))
        .collect();
    for name in &tested {
        match registry.get(name) {
            None => report.failures.push(format!(
                "tests/profiles/{name}: no built-in profile has the id `{name}` (profiles are files in `profiles/`)"
            )),
            Some(platform) if platform.is_abstract() => report.failures.push(format!(
                "tests/profiles/{name}: `{name}` is an abstract base; it is tested through the profiles that extend it"
            )),
            Some(_) => {}
        }
    }

    for platform in registry.platforms() {
        if platform.is_abstract() || only.is_some_and(|only| only != platform.id()) {
            continue;
        }
        let dir = root.join(platform.id());
        if !dir.is_dir() {
            report.failures.push(format!(
                "[{id}] tests/profiles/{id}/ is missing: every built-in profile needs its test data (see docs/profiles.md, \"Testing a profile\")",
                id = platform.id()
            ));
            continue;
        }
        let mut subject = Subject {
            registry,
            platform: platform.clone(),
            dir,
            failures: Vec::new(),
            tally: Tally::default(),
        };
        checks::prompts(&mut subject);
        checks::errors(&mut subject);
        checks::policy(&mut subject);
        checks::detect(&mut subject).await;
        let coverage = captures::run(&mut subject).await;
        checks::configs(&mut subject);
        scripted::run(&mut subject).await;
        unverified(&mut subject, &coverage);
        maturity(&mut subject, &coverage);

        let tally = &subject.tally;
        report.lines.push(format!(
            "  {:<22} {:<12} {:>3} prompts {:>3} error samples {:>3} commands {:>3} detections {:>2} captures ({} rows) {:>2} configs {:>2} sessions{}",
            subject.id(),
            subject.platform.profile.maturity.as_str(),
            tally.prompts,
            tally.errors,
            tally.commands,
            tally.detections,
            tally.captures,
            tally.rows,
            tally.configs,
            tally.sessions,
            if subject.failures.is_empty() { "" } else { "   <-- FAILED" },
        ));
        report.profiles += 1;
        report.failures.append(&mut subject.failures);
    }
    report
}

/// A kind the profile lists under `unverified` is a read nobody has seen
/// work on the real platform. That is allowed, on two conditions: the parser
/// is tested against something, and the caveat goes the day it stops being
/// true — not whenever somebody remembers.
fn unverified(subject: &mut Subject<'_>, coverage: &captures::Coverage) {
    for kind in subject.platform.profile.unverified.clone() {
        if let Some(cases) = coverage.recorded_in.get(&kind) {
            subject.fail(
                "captures/",
                format!(
                    "the profile still lists {kind} under `unverified`, but {} {} recorded from a real device: the read has met one now, so take {kind} off the list (if the files are not a recording, say what they are with `origin: documented` or `origin: synthetic` in expect.yml)",
                    cases.join(", "),
                    if cases.len() == 1 { "is" } else { "are" },
                ),
            );
        } else if !coverage.any.contains(&kind) {
            subject.fail(
                "captures/",
                format!(
                    "the profile lists {kind} under `unverified`, which excuses it from a recording, not from being tested: add at least one capture that collects it — a sample from the vendor's documentation (`origin: documented`) or a synthetic one"
                ),
            );
        }
    }
}

/// A `verified` profile must have earned the word.
fn maturity(subject: &mut Subject<'_>, coverage: &captures::Coverage) {
    let profile = &subject.platform.profile;
    if profile.maturity != Maturity::Verified {
        return;
    }
    let collected: Vec<Kind> = profile
        .collect
        .iter()
        .filter(|(kind, steps)| !steps.is_empty() && !profile.is_unverified(**kind))
        .map(|(kind, _)| *kind)
        .collect();
    for kind in collected {
        if !coverage.real.contains(&kind) {
            subject.fail(
                "captures/",
                format!(
                    "the profile says `maturity: verified`, but {kind} is not covered by a capture recorded from a real device (only by documented or synthetic ones, or none): add one, list {kind} under `unverified`, or lower the maturity to `community`"
                ),
            );
        }
    }
}
