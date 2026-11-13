//! Intended configuration, drift against it, and restoring a stored version.
//!
//! The intended configuration of a device is text a person (or a template)
//! wrote: a whole configuration, or the part of it that matters. Which of the
//! two it is has to be said, because the answer to "what is wrong with the
//! device" differs: for a whole configuration (`exact`) everything else on the
//! device is drift and would be removed; for a fragment nothing is ever
//! removed, only what is missing is added.
//!
//! Drift is computed against the latest stored running configuration — it
//! needs no device access. Fixing it, like restoring an old version, is
//! handed to the change pipeline.

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_drivers::hier::{self, ConfigTree, Syntax, TextDiff};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::handoff::{self, ChangeTarget};
use crate::outline;
use crate::redact::{self, Redaction};
use crate::scope::{self, Target};
use crate::versions::{self, ConfigVersion, Kind};

const COLLECTION: &str = "config_intended";
/// Larger than any configuration; a paste beyond it is a mistake.
const MAX_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_CONTEXT: usize = 3;

/// The configuration a device should have, as stored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Intended {
    device_id: String,
    text: String,
    /// `true`: the device must equal `text`. `false`: `text` must be present on the device.
    exact: bool,
    updated_at: i64,
}

/// The intended configuration as the RPC surface shows it.
#[derive(Debug, Serialize)]
struct IntendedView {
    device_id: String,
    text: String,
    exact: bool,
    updated_at: i64,
    lines: usize,
    redacted: bool,
}

fn stored(app: &App, target: &Target) -> AppResult<Intended> {
    app.store()
        .get::<Intended>(COLLECTION, &target.device.id)?
        .ok_or_else(|| {
            AppError::new(
                "not_found",
                format!(
                    "`{}` has no intended configuration; set one first",
                    target.device.name
                ),
            )
        })
}

fn view(app: &App, target: &Target, intended: Intended, redact: bool) -> IntendedView {
    let text = if redact {
        Redaction::for_profiles(app, [target.profile.as_str()]).text(&intended.text)
    } else {
        versions::audit_unredacted(
            app,
            &target.device.id,
            &target.device.name,
            json!({"what": "intended configuration"}),
        );
        intended.text
    };
    IntendedView {
        device_id: intended.device_id,
        lines: text.lines().count(),
        text,
        exact: intended.exact,
        updated_at: intended.updated_at,
        redacted: redact,
    }
}

/// Remove what is stored about a deleted device.
pub(crate) fn forget(app: &App, device_id: &str) -> AppResult<()> {
    app.store().delete(COLLECTION, device_id).map(|_| ())
}

// --------------------------------------------------------------------- drift

/// What separates a device from its intended configuration.
#[derive(Debug, Serialize)]
struct Drift {
    device_id: String,
    name: String,
    /// False when the device type's configuration grammar is not modelled:
    /// the difference is shown, the commands cannot be derived.
    supported: bool,
    /// Nothing would have to be sent and nothing is left unexplained.
    in_sync: bool,
    exact: bool,
    /// Configuration-mode lines that would make the device match, in order.
    commands: Vec<String>,
    warnings: Vec<String>,
    /// `exact`: the running configuration → the intended one. Otherwise: the
    /// intended lines the device already has → all intended lines.
    diff: TextDiff,
    /// The running-config version the device was judged by, and when it was last fetched.
    version_id: i64,
    fetched_at: i64,
    redacted: bool,
}

struct Computed {
    version: ConfigVersion,
    remediation: hier::Remediation,
    /// The two texts whose difference shows the drift.
    before: String,
    after: String,
}

impl Computed {
    fn in_sync(&self, diff: &TextDiff) -> bool {
        if self.remediation.supported {
            self.remediation.commands.is_empty() && self.remediation.warnings.is_empty()
        } else {
            diff.added == 0 && diff.removed == 0
        }
    }
}

/// The intended text in the form the remediation engine reads for this grammar.
fn engine_form(text: &str, syntax: Syntax) -> String {
    match syntax {
        // One complete command per line, however the text was exported or typed.
        Syntax::Routeros => outline::routeros_terse(text),
        _ => text.to_string(),
    }
}

fn compute(app: &App, target: &Target, intended: &Intended) -> AppResult<Computed> {
    let version = versions::require_latest(app, &target.device.name, &target.device.id, Kind::Running)?;
    let (version, running) = versions::read(app, version.id)?;
    let syntax = target.syntax(app);
    let running = engine_form(&running, syntax);
    let wanted = engine_form(&intended.text, syntax);
    let have = ConfigTree::parse(&running, syntax);
    let want = ConfigTree::parse(&wanted, syntax);

    Ok(if intended.exact {
        Computed {
            version,
            remediation: hier::remediate(&running, &wanted, syntax),
            before: have.render(),
            after: want.render(),
        }
    } else {
        // What the device should look like when nothing may be removed: itself, plus what is missing.
        let goal = ConfigTree {
            roots: outline::merged(&have.roots, &want.roots),
        };
        let already = ConfigTree {
            roots: outline::present(&want.roots, &have.roots),
        };
        Computed {
            version,
            remediation: hier::remediate(&running, &goal.render(), syntax),
            before: already.render(),
            after: want.render(),
        }
    })
}

fn drift(app: &App, target: &Target, context: usize, redact: bool) -> AppResult<Drift> {
    let intended = stored(app, target)?;
    let computed = compute(app, target, &intended)?;
    let diff = hier::text_diff(&computed.before, &computed.after, Some(context));
    let in_sync = computed.in_sync(&diff);
    let (commands, warnings, diff) = if redact {
        let redaction = Redaction::for_profiles(app, [target.profile.as_str()]);
        let each = |lines: &[String]| -> Vec<String> {
            lines.iter().map(|line| redaction.text(line)).collect()
        };
        (
            each(&computed.remediation.commands),
            each(&computed.remediation.warnings),
            hier::text_diff(
                &redaction.text(&computed.before),
                &redaction.text(&computed.after),
                Some(context),
            ),
        )
    } else {
        versions::audit_unredacted(
            app,
            &target.device.id,
            &target.device.name,
            json!({"what": "drift", "version_id": computed.version.id}),
        );
        (
            computed.remediation.commands.clone(),
            computed.remediation.warnings.clone(),
            diff,
        )
    };
    Ok(Drift {
        device_id: target.device.id.clone(),
        name: target.device.name.clone(),
        supported: computed.remediation.supported,
        in_sync,
        exact: intended.exact,
        commands,
        warnings,
        diff,
        version_id: computed.version.id,
        fetched_at: computed.version.last_seen,
        redacted: redact,
    })
}

/// A timestamp as people read it.
fn when(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map_or_else(
        || "an unknown time".to_string(),
        |time| time.format("%Y-%m-%d %H:%M UTC").to_string(),
    )
}

async fn fix(app: &App, target: &Target) -> AppResult<String> {
    let intended = stored(app, target)?;
    let computed = compute(app, target, &intended)?;
    let name = &target.device.name;
    let as_of = when(computed.version.last_seen);
    if !computed.remediation.supported {
        return Err(AppError::new(
            "unsupported",
            format!(
                "the commands that would fix the drift on `{name}` cannot be derived: the configuration grammar of its device type is not modelled. \
                 Create a change with the lines to send instead."
            ),
        ));
    }
    let diff = hier::text_diff(&computed.before, &computed.after, Some(0));
    if computed.in_sync(&diff) {
        return Err(AppError::invalid(format!(
            "`{name}` matches its intended configuration as of its last backup ({as_of}), so there is nothing to fix. \
             Back the device up again if it may have changed since."
        )));
    }
    let commands = computed.remediation.commands.len();
    if commands == 0 {
        // Warnings quote configuration lines; they leave here redacted like everything else.
        let redaction = Redaction::for_profiles(app, [target.profile.as_str()]);
        let reasons: Vec<String> = computed
            .remediation
            .warnings
            .iter()
            .map(|warning| redaction.text(warning))
            .collect();
        return Err(AppError::invalid(format!(
            "what separates `{name}` from its intended configuration cannot be fixed automatically: {}",
            reasons.join("; ")
        ))
        .with_details(json!({"warnings": reasons})));
    }
    let syntax = target.syntax(app);
    let change_id = handoff::create_change(
        app,
        &format!("Fix configuration drift on `{name}`"),
        &format!(
            "Bring `{name}` in line with its intended configuration ({}). As of the backup of {as_of} (version {}) that takes {commands} command{}; the change pipeline computes the final commands from the device itself.",
            if intended.exact {
                "the device must equal it"
            } else {
                "what it contains must be present; nothing else is removed"
            },
            computed.version.id,
            if commands == 1 { "" } else { "s" },
        ),
        "drift",
        vec![ChangeTarget::intended(
            &target.device.id,
            &engine_form(&intended.text, syntax),
            intended.exact,
        )],
    )
    .await?;
    app.audit(
        "config.drift.fix",
        Some(&target.device.id),
        format!("Created change {change_id} to fix configuration drift on `{name}`"),
        json!({"change_id": change_id, "version_id": computed.version.id, "commands": commands}),
    );
    Ok(change_id)
}

async fn restore(app: &App, version_id: i64) -> AppResult<String> {
    let (version, text) = versions::read(app, version_id)?;
    let target = scope::target(app, &version.device_id)?;
    let name = &target.device.name;
    let kind = if version.kind == Kind::Startup.as_str() {
        Kind::Startup
    } else {
        Kind::Running
    };
    let change_id = handoff::create_change(
        app,
        &format!(
            "Restore `{name}` to its {} of {} (version {})",
            kind.label(),
            when(version.first_seen),
            version.id
        ),
        &format!(
            "Make the running configuration of `{name}` equal to stored version {} ({} lines, sha256 {}). \
             The commands are computed by the change pipeline from what the device has now; nothing is overwritten blindly.",
            version.id,
            version.lines,
            &version.sha256[..version.sha256.len().min(12)],
        ),
        "restore",
        vec![ChangeTarget::intended(
            &target.device.id,
            &engine_form(&text, target.syntax(app)),
            true,
        )],
    )
    .await?;
    app.audit(
        "config.restore",
        Some(&target.device.id),
        format!(
            "Created change {change_id} to restore `{name}` to configuration version {}",
            version.id
        ),
        json!({"change_id": change_id, "version_id": version.id}),
    );
    Ok(change_id)
}

// ----------------------------------------------------------------------- rpc

#[derive(Deserialize)]
struct GetParams {
    device_id: String,
    redact: Option<bool>,
}

#[derive(Deserialize)]
struct SetParams {
    device_id: String,
    text: String,
    exact: Option<bool>,
}

#[derive(Deserialize)]
struct DeviceParams {
    device_id: String,
}

#[derive(Deserialize)]
struct DriftParams {
    device_id: String,
    context: Option<usize>,
    redact: Option<bool>,
}

#[derive(Deserialize)]
struct RestoreParams {
    version_id: i64,
}

#[derive(Serialize)]
struct Handover {
    change_id: String,
}

pub(crate) fn register(router: &mut Router) {
    router.add("config.intended.get", |app: App, params: GetParams| async move {
        let target = scope::target(&app, &params.device_id)?;
        Ok(app
            .store()
            .get::<Intended>(COLLECTION, &target.device.id)?
            .map(|intended| view(&app, &target, intended, params.redact.unwrap_or(true))))
    });

    router.add("config.intended.set", |app: App, params: SetParams| async move {
        let target = scope::target(&app, &params.device_id)?;
        let name = &target.device.name;
        if params.text.trim().is_empty() {
            return Err(AppError::invalid(
                "the intended configuration is empty; use `config.intended.clear` to remove it",
            ));
        }
        if params.text.len() > MAX_BYTES {
            return Err(AppError::invalid(format!(
                "the text is {} bytes; an intended configuration is at most {MAX_BYTES} bytes",
                params.text.len()
            )));
        }
        if redact::has_placeholders(&params.text) {
            return Err(AppError::invalid(
                "the text contains redaction placeholders (`<secret:…>`): it was copied from a redacted view. \
                 Load the configuration with `redact: false` and edit that, so that the real values are kept.",
            ));
        }
        let existing: Option<Intended> = app.store().get(COLLECTION, &target.device.id)?;
        let intended = Intended {
            device_id: target.device.id.clone(),
            text: params.text,
            // Keep the meaning a text already has unless told otherwise; a new one is a fragment,
            // because treating a fragment as the whole configuration would remove everything else.
            exact: params
                .exact
                .unwrap_or_else(|| existing.is_some_and(|existing| existing.exact)),
            updated_at: now_ms(),
        };
        app.store().put(COLLECTION, &target.device.id, &intended)?;
        app.audit(
            "config.intended.set",
            Some(&target.device.id),
            format!(
                "Set the intended configuration of `{name}` ({} lines, {})",
                intended.text.lines().count(),
                if intended.exact {
                    "the whole configuration"
                } else {
                    "a fragment"
                }
            ),
            json!({"exact": intended.exact}),
        );
        app.publish(
            "config.intended_changed",
            &json!({"device_id": target.device.id, "set": true}),
        );
        Ok(view(&app, &target, intended, true))
    });

    router.add("config.intended.clear", |app: App, params: DeviceParams| async move {
        let target = scope::target(&app, &params.device_id)?;
        let cleared = app.store().delete(COLLECTION, &target.device.id)?;
        if cleared {
            app.audit(
                "config.intended.clear",
                Some(&target.device.id),
                format!(
                    "Removed the intended configuration of `{}`",
                    target.device.name
                ),
                serde_json::Value::Null,
            );
            app.publish(
                "config.intended_changed",
                &json!({"device_id": target.device.id, "set": false}),
            );
        }
        Ok(json!({"cleared": cleared}))
    });

    router.add("config.drift", |app: App, params: DriftParams| async move {
        let target = scope::target(&app, &params.device_id)?;
        drift(
            &app,
            &target,
            params.context.unwrap_or(DEFAULT_CONTEXT),
            params.redact.unwrap_or(true),
        )
    });

    router.add("config.drift.fix", |app: App, params: DeviceParams| async move {
        let target = scope::target(&app, &params.device_id)?;
        Ok(Handover {
            change_id: fix(&app, &target).await?,
        })
    });

    router.add("config.restore", |app: App, params: RestoreParams| async move {
        Ok(Handover {
            change_id: restore(&app, params.version_id).await?,
        })
    });
}
