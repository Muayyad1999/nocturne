//! Comparing configurations: two versions of a device, or two devices.

use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_drivers::hier::{self, TextDiff};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::outline::Outline;
use crate::redact::Redaction;
use crate::scope::{self, Target};
use crate::versions::{self, ConfigVersion, Kind};

/// How much of a diff falls into one configuration section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SectionChange {
    /// The section, outermost line first (`["router bgp 65001", "address-family ipv4"]`).
    /// Empty for lines at the top level of the configuration.
    pub path: Vec<String>,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Serialize)]
pub struct DiffResult {
    /// The older (or left) side.
    pub a: ConfigVersion,
    /// The newer (or right) side.
    pub b: ConfigVersion,
    pub diff: TextDiff,
    pub sections: Vec<SectionChange>,
    pub redacted: bool,
}

/// One side of a comparison.
struct Side {
    version: ConfigVersion,
    text: String,
    /// The device the version belongs to, while it exists.
    target: Option<Target>,
}

impl Side {
    fn of_version(app: &App, id: i64) -> AppResult<Side> {
        let (version, text) = versions::read(app, id)?;
        Ok(Side {
            target: scope::target(app, &version.device_id).ok(),
            version,
            text,
        })
    }

    fn latest_of(app: &App, device: &str, kind: Kind) -> AppResult<Side> {
        let target = scope::target(app, device)?;
        let version = versions::require_latest(app, &target.device.name, &target.device.id, kind)?;
        let (version, text) = versions::read(app, version.id)?;
        Ok(Side {
            version,
            text,
            target: Some(target),
        })
    }

    fn profile(&self) -> &str {
        self.target.as_ref().map_or("", |target| target.profile.as_str())
    }

    fn outline(&self, app: &App, text: &str) -> Outline {
        let syntax = self
            .target
            .as_ref()
            .map_or(hier::Syntax::Flat, |target| target.syntax(app));
        Outline::parse(text, syntax)
    }

    fn audit_unredacted(&self, app: &App) {
        let name = self
            .target
            .as_ref()
            .map_or(self.version.device_id.as_str(), |target| target.device.name.as_str());
        versions::audit_unredacted(
            app,
            &self.version.device_id,
            name,
            json!({"version_id": self.version.id, "kind": self.version.kind}),
        );
    }
}

/// Group the changed lines of a diff by the section each one sits in, in the
/// order the sections first appear.
fn sections(diff: &TextDiff, old: &Outline, new: &Outline) -> Vec<SectionChange> {
    let mut sections: Vec<SectionChange> = Vec::new();
    for line in &diff.lines {
        let (path, added) = match (line.tag.as_str(), line.old_line, line.new_line) {
            ("-", Some(no), _) => (old.section_of(no), false),
            ("+", _, Some(no)) => (new.section_of(no), true),
            _ => continue,
        };
        let index = match sections.iter().position(|section| section.path == path) {
            Some(index) => index,
            None => {
                sections.push(SectionChange {
                    path,
                    added: 0,
                    removed: 0,
                });
                sections.len() - 1
            }
        };
        if added {
            sections[index].added += 1;
        } else {
            sections[index].removed += 1;
        }
    }
    sections
}

fn compare(app: &App, a: Side, b: Side, context: usize, redact: bool) -> DiffResult {
    let (old, new) = if redact {
        // One table for both sides: an unchanged secret keeps its placeholder, a changed one gets another.
        let redaction = Redaction::for_profiles(app, [a.profile(), b.profile()]);
        (redaction.text(&a.text), redaction.text(&b.text))
    } else {
        a.audit_unredacted(app);
        if b.version.device_id != a.version.device_id {
            b.audit_unredacted(app);
        }
        (a.text.clone(), b.text.clone())
    };
    let diff = hier::text_diff(&old, &new, Some(context));
    let sections = sections(&diff, &a.outline(app, &old), &b.outline(app, &new));
    DiffResult {
        a: a.version,
        b: b.version,
        diff,
        sections,
        redacted: redact,
    }
}

#[derive(Deserialize)]
struct DiffParams {
    a: Option<i64>,
    b: Option<i64>,
    device_id: Option<String>,
    kind: Option<Kind>,
    context: Option<usize>,
    redact: Option<bool>,
}

/// A device (its latest version) or one particular version.
#[derive(Deserialize)]
struct SideRef {
    device_id: Option<String>,
    version_id: Option<i64>,
    kind: Option<Kind>,
}

impl SideRef {
    fn resolve(&self, app: &App, name: &str, kind: Option<Kind>) -> AppResult<Side> {
        match (&self.version_id, &self.device_id) {
            (Some(id), None) => Side::of_version(app, *id),
            (None, Some(device)) => {
                Side::latest_of(app, device, self.kind.or(kind).unwrap_or_default())
            }
            _ => Err(AppError::invalid(format!(
                "`{name}` must name either a device (`device_id`, its latest configuration) or a version (`version_id`)"
            ))),
        }
    }
}

#[derive(Deserialize)]
struct CompareParams {
    a: SideRef,
    b: SideRef,
    kind: Option<Kind>,
    context: Option<usize>,
    redact: Option<bool>,
}

const DEFAULT_CONTEXT: usize = 3;

pub(crate) fn register(router: &mut Router) {
    router.add("config.diff", |app: App, params: DiffParams| async move {
        let (a, b) = match (params.a, params.b, &params.device_id) {
            (Some(a), Some(b), _) => (Side::of_version(&app, a)?, Side::of_version(&app, b)?),
            (None, None, Some(device)) => {
                let target = scope::target(&app, device)?;
                let kind = params.kind.unwrap_or_default();
                let newest = versions::newest(&app, &target.device.id, kind, 2)?;
                let [latest, previous] = newest.as_slice() else {
                    return Err(match newest.len() {
                        0 => versions::no_backup(&target.device.name, kind),
                        _ => AppError::invalid(format!(
                            "`{}` has only one stored {} version, so there is nothing to compare it with yet",
                            target.device.name,
                            kind.label()
                        )),
                    });
                };
                (
                    Side::of_version(&app, previous.id)?,
                    Side::of_version(&app, latest.id)?,
                )
            }
            _ => {
                return Err(AppError::invalid(
                    "name two versions (`a` and `b`), or a `device_id` to compare its latest configuration with the one before",
                ));
            }
        };
        Ok(compare(
            &app,
            a,
            b,
            params.context.unwrap_or(DEFAULT_CONTEXT),
            params.redact.unwrap_or(true),
        ))
    });

    router.add("config.compare", |app: App, params: CompareParams| async move {
        let a = params.a.resolve(&app, "a", params.kind)?;
        let b = params.b.resolve(&app, "b", params.kind)?;
        Ok(compare(
            &app,
            a,
            b,
            params.context.unwrap_or(DEFAULT_CONTEXT),
            params.redact.unwrap_or(true),
        ))
    });
}
