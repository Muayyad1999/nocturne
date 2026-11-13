//! Compliance: written-down rules, evaluated against stored configurations.
//!
//! A policy is a named list of rules with a scope; a run answers every rule
//! of every policy in scope for every device in scope, from the latest stored
//! running configuration. Nothing here opens a session to a device, and
//! nothing here changes one: a fix becomes a change request
//! (`compliance.remediate` → `changes.create`) that a person approves.
//!
//! Built-in policies ship as data and are read-only. They can be switched off
//! and they can be duplicated into a policy of one's own.

pub mod builtin;
pub mod eval;
pub mod model;
pub mod remediate;
pub mod run;
pub mod validate;

use nocturne_app::inventory::DeviceFilter;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_drivers::hier::Syntax;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::scope::{self, Target};
use eval::Subject;
use model::{Finding, Policy, Rule, Status};
use run::{ResultsFilter, RunSummary};

/// Where user policies are stored.
const POLICIES: &str = "config_policies";
/// Which built-in policies the user switched off.
const DISABLED: &str = "config.compliance.disabled_builtins";

fn disabled(app: &App) -> Vec<String> {
    app.store()
        .kv_get::<Vec<String>>(DISABLED)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn set_disabled(app: &App, ids: &[String]) -> AppResult<()> {
    app.store().kv_set(DISABLED, &ids.to_vec())
}

/// Every policy: the built-ins as shipped (with the user's enabled flag) and
/// the user's own, built-ins first and then by name.
pub fn policies(app: &App) -> AppResult<Vec<Policy>> {
    let off = disabled(app);
    let mut all: Vec<Policy> = builtin::all()
        .iter()
        .map(|policy| Policy {
            enabled: !off.contains(&policy.id),
            ..policy.clone()
        })
        .collect();
    // A stored document is whatever was saved; it is never a built-in.
    all.extend(
        app.store()
            .list::<Policy>(POLICIES)?
            .into_iter()
            .map(|policy| Policy {
                builtin: false,
                ..policy
            }),
    );
    all.sort_by_key(|policy| (!policy.builtin, policy.name.to_lowercase()));
    Ok(all)
}

pub fn policy(app: &App, id: &str) -> AppResult<Policy> {
    policies(app)?
        .into_iter()
        .find(|policy| policy.id == id)
        .ok_or_else(|| AppError::not_found(format!("compliance policy `{id}`")))
}

/// The policies a run uses: those named, or every enabled one.
fn chosen(app: &App, ids: Option<&[String]>) -> AppResult<Vec<Policy>> {
    let all = policies(app)?;
    let Some(ids) = ids else {
        return Ok(all.into_iter().filter(|policy| policy.enabled).collect());
    };
    ids.iter()
        .map(|id| {
            all.iter()
                .find(|policy| &policy.id == id)
                .cloned()
                .ok_or_else(|| AppError::not_found(format!("compliance policy `{id}`")))
        })
        .collect()
}

fn store_policy(app: &App, policy: &Policy) -> AppResult<()> {
    app.store().put(POLICIES, &policy.id, policy)
}

fn announce(app: &App, policy_id: &str, change: &str) {
    app.publish(
        "compliance.policies_changed",
        &json!({"policy_id": policy_id, "change": change}),
    );
}

/// Save a user policy, or switch a built-in on or off.
fn save(app: &App, mut incoming: Policy) -> AppResult<Policy> {
    let problems = validate::problems(&incoming);
    if !validate::ok(&problems) {
        return Err(AppError::invalid(
            "the policy has problems that must be fixed first; `compliance.policies.validate` lists them",
        )
        .with_details(json!({"problems": problems})));
    }
    if let Some(shipped) = builtin::get(&incoming.id) {
        // A built-in may be switched off, and nothing else: a rule that
        // quietly changed under a name people trust is worse than no rule.
        let same = Policy {
            enabled: shipped.enabled,
            ..incoming.clone()
        } == *shipped;
        if !same {
            return Err(AppError::new(
                "forbidden",
                format!(
                    "`{}` is a built-in policy and cannot be edited. Duplicate it (`compliance.policies.duplicate`) and change the copy; only switching it on or off is allowed here.",
                    shipped.name
                ),
            ));
        }
        let mut off = disabled(app);
        off.retain(|id| id != &incoming.id);
        if !incoming.enabled {
            off.push(incoming.id.clone());
        }
        set_disabled(app, &off)?;
        app.audit(
            "compliance.policies.save",
            None,
            format!(
                "{} the built-in compliance policy `{}`",
                if incoming.enabled {
                    "Switched on"
                } else {
                    "Switched off"
                },
                shipped.name
            ),
            json!({"policy_id": shipped.id, "enabled": incoming.enabled}),
        );
        announce(app, &shipped.id, "saved");
        return policy(app, &shipped.id);
    }

    let existing: Option<Policy> = if incoming.id.trim().is_empty() {
        incoming.id = new_id();
        None
    } else {
        app.store().get::<Policy>(POLICIES, &incoming.id)?
    };
    incoming.builtin = false;
    incoming.updated_at = now_ms();
    store_policy(app, &incoming)?;
    app.audit(
        "compliance.policies.save",
        None,
        format!(
            "{} the compliance policy `{}` ({} rule{})",
            if existing.is_some() {
                "Changed"
            } else {
                "Created"
            },
            incoming.name,
            incoming.rules.len(),
            if incoming.rules.len() == 1 { "" } else { "s" }
        ),
        json!({"policy_id": incoming.id, "rules": incoming.rules.len()}),
    );
    announce(app, &incoming.id, "saved");
    Ok(incoming)
}

/// Run the chosen policies over the chosen devices and store the result.
fn run_now(app: &App, targets: Vec<Target>, policies: &[Policy]) -> AppResult<RunSummary> {
    let requested = targets.len();
    let findings = run::execute(app, targets, policies)?;
    let summary = run::summarize(&findings, requested);
    let summary = run::store(app, &findings, summary)?;
    run::announce(app, &summary);
    Ok(summary)
}

/// The rule a finding came from, looked up in the policy it names.
fn rule_of<'a>(policies: &'a [Policy], finding: &Finding) -> Option<&'a Rule> {
    policies
        .iter()
        .find(|policy| policy.id == finding.policy_id)?
        .rules
        .iter()
        .find(|rule| rule.id == finding.rule_id)
}

// ----------------------------------------------------------------------- rpc

#[derive(Deserialize)]
struct PolicyIdParams {
    policy_id: String,
}

#[derive(Deserialize)]
struct PolicyParams {
    policy: Policy,
}

#[derive(Deserialize)]
struct DuplicateParams {
    policy_id: String,
    name: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RunParams {
    policy_ids: Option<Vec<String>>,
    device_ids: Option<Vec<String>>,
    filter: Option<DeviceFilter>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ResultsParams {
    run_id: Option<String>,
    status: Option<String>,
    severity: Option<String>,
    device_id: Option<String>,
    policy_id: Option<String>,
    rule_id: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RunsParams {
    limit: Option<usize>,
    before_id: Option<String>,
}

#[derive(Deserialize)]
struct FindingRef {
    device_id: String,
    rule_id: String,
    policy_id: Option<String>,
}

#[derive(Deserialize)]
struct RemediateParams {
    run_id: Option<String>,
    findings: Vec<FindingRef>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TestParams {
    rule: Option<Rule>,
    text: Option<String>,
    device_id: Option<String>,
    /// Which grammar `text` is written in; the device's own when `device_id`
    /// is given, `ios` otherwise.
    syntax: Option<Syntax>,
}

#[derive(Serialize)]
struct ValidationResult {
    ok: bool,
    problems: Vec<validate::Problem>,
}

#[derive(Serialize)]
struct RunResult {
    run_id: String,
    summary: RunSummary,
}

#[derive(Serialize)]
struct ResultsResult {
    findings: Vec<Finding>,
    summary: RunSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct TestResult {
    finding: Finding,
    /// The grammar the text was read with.
    syntax: Syntax,
    /// What would be sent to fix it, when the rule carries a remediation and
    /// the check ran against a device.
    fix: Vec<String>,
}

#[derive(Serialize)]
struct Handover {
    change_id: String,
    devices: usize,
    commands: usize,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|err| {
        AppError::internal(format!("the compliance run stopped unexpectedly: {err}"))
    })?
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "compliance.policies.list",
        |app: App, _params: nocturne_app::NoParams| async move { policies(&app) },
    );

    router.add(
        "compliance.policies.get",
        |app: App, params: PolicyIdParams| async move { policy(&app, &params.policy_id) },
    );

    router.add(
        "compliance.policies.save",
        |app: App, params: PolicyParams| async move { save(&app, params.policy) },
    );

    router.add(
        "compliance.policies.validate",
        |_app: App, params: PolicyParams| async move {
            let problems = validate::problems(&params.policy);
            Ok(ValidationResult {
                ok: validate::ok(&problems),
                problems,
            })
        },
    );

    router.add(
        "compliance.policies.delete",
        |app: App, params: PolicyIdParams| async move {
            if let Some(shipped) = builtin::get(&params.policy_id) {
                return Err(AppError::new(
                    "forbidden",
                    format!(
                        "`{}` is a built-in policy and cannot be deleted. Switch it off instead (save it with `enabled: false`).",
                        shipped.name
                    ),
                ));
            }
            let existing = policy(&app, &params.policy_id)?;
            app.store().delete(POLICIES, &params.policy_id)?;
            app.audit(
                "compliance.policies.delete",
                None,
                format!("Deleted the compliance policy `{}`", existing.name),
                json!({"policy_id": existing.id}),
            );
            announce(&app, &existing.id, "deleted");
            Ok(json!({"deleted": true}))
        },
    );

    router.add(
        "compliance.policies.duplicate",
        |app: App, params: DuplicateParams| async move {
            let source = policy(&app, &params.policy_id)?;
            let name = params
                .name
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("{} (copy)", source.name));
            let copy = Policy {
                id: new_id(),
                name,
                builtin: false,
                enabled: true,
                updated_at: now_ms(),
                description: source.description.clone(),
                scope: source.scope.clone(),
                rules: source.rules.clone(),
            };
            store_policy(&app, &copy)?;
            app.audit(
                "compliance.policies.duplicate",
                None,
                format!(
                    "Copied the compliance policy `{}` to `{}`",
                    source.name, copy.name
                ),
                json!({"policy_id": copy.id, "from": source.id}),
            );
            announce(&app, &copy.id, "saved");
            Ok(copy)
        },
    );

    router.add("compliance.run", |app: App, params: RunParams| async move {
        let policies = chosen(&app, params.policy_ids.as_deref())?;
        if policies.is_empty() {
            return Err(AppError::invalid(
                "no compliance policy is enabled, so a run would judge nothing",
            ));
        }
        let targets = scope::targets(&app, params.device_ids.as_deref(), params.filter.as_ref())?;
        if targets.is_empty() {
            return Err(AppError::invalid(
                "no device matches, so there is nothing to check",
            ));
        }
        let devices = targets.len();
        let summary = {
            let app = app.clone();
            blocking(move || run_now(&app, targets, &policies)).await?
        };
        app.audit(
            "compliance.run",
            None,
            format!(
                "Checked {devices} device{} against {} compliance polic{}: {} passed, {} failed",
                if devices == 1 { "" } else { "s" },
                summary.policies,
                if summary.policies == 1 { "y" } else { "ies" },
                summary.pass,
                summary.fail
            ),
            json!({"run_id": summary.run_id, "devices": devices, "pass": summary.pass, "fail": summary.fail}),
        );
        Ok(RunResult {
            run_id: summary.run_id.clone(),
            summary,
        })
    });

    router.add(
        "compliance.results",
        |app: App, params: ResultsParams| async move {
            let limit = params.limit.unwrap_or(200).clamp(1, 2000);
            let page = run::results(
                &app,
                params.run_id.as_deref(),
                &ResultsFilter {
                    status: params.status,
                    severity: params.severity,
                    device_id: params.device_id,
                    policy_id: params.policy_id,
                    rule_id: params.rule_id,
                },
                params.cursor.as_deref(),
                limit,
            )?;
            Ok(ResultsResult {
                findings: page.findings,
                summary: page.summary,
                next_cursor: page.next_cursor,
            })
        },
    );

    router.add(
        "compliance.runs",
        |app: App, params: RunsParams| async move {
            let limit = params.limit.unwrap_or(50).clamp(1, 500);
            run::history(&app, params.before_id.as_deref(), limit)
        },
    );

    router.add(
        "compliance.remediate",
        |app: App, params: RemediateParams| async move {
            if params.findings.is_empty() {
                return Err(AppError::invalid(
                    "name the findings to fix: `findings: [{device_id, rule_id}]`",
                ));
            }
            let known = policies(&app)?;
            let mut fixes: Vec<remediate::DeviceFix> = Vec::new();
            let mut commands = 0;
            for wanted in &params.findings {
                let device = nocturne_app::inventory::find(&app, &wanted.device_id)?;
                let page = run::results(
                    &app,
                    params.run_id.as_deref(),
                    &ResultsFilter {
                        device_id: Some(device.id.clone()),
                        rule_id: Some(wanted.rule_id.clone()),
                        policy_id: wanted.policy_id.clone(),
                        ..ResultsFilter::default()
                    },
                    None,
                    8,
                )?;
                let Some(finding) = page
                    .findings
                    .into_iter()
                    .find(|finding| finding.status == Status::Fail)
                else {
                    return Err(AppError::not_found(format!(
                        "a failing finding for rule `{}` on `{}` in that compliance run",
                        wanted.rule_id, device.name
                    )));
                };
                let rule = rule_of(&known, &finding).ok_or_else(|| {
                    AppError::conflict(format!(
                        "rule `{}` of policy `{}` no longer exists, so its fix cannot be rendered",
                        finding.rule_id, finding.policy_id
                    ))
                })?;
                let lines = remediate::lines_for(&finding, rule, &device)?;
                commands += lines.len();
                let entry = (finding.rule_id.clone(), finding.title.clone());
                match fixes.iter_mut().find(|fix| fix.device.id == device.id) {
                    Some(fix) => {
                        fix.lines.extend(lines);
                        fix.rules.push(entry);
                    }
                    None => fixes.push(remediate::DeviceFix {
                        device,
                        lines,
                        rules: vec![entry],
                    }),
                }
            }
            let devices = fixes.len();
            let change_id = remediate::hand_over(&app, fixes).await?;
            app.audit(
                "compliance.remediate",
                None,
                format!(
                    "Created change {change_id} to fix {} compliance finding{} on {devices} device{}",
                    params.findings.len(),
                    if params.findings.len() == 1 { "" } else { "s" },
                    if devices == 1 { "" } else { "s" }
                ),
                json!({"change_id": change_id, "devices": devices, "commands": commands}),
            );
            Ok(Handover {
                change_id,
                devices,
                commands,
            })
        },
    );

    router.add("compliance.test", |app: App, params: TestParams| async move {
        let Some(rule) = params.rule else {
            return Err(AppError::invalid(
                "`rule` is missing: `compliance.test` answers one rule against one configuration",
            ));
        };
        let preview = Policy {
            id: "preview".into(),
            name: "Preview".into(),
            enabled: true,
            rules: vec![rule.clone()],
            ..Policy::default()
        };
        let problems = validate::problems(&preview);
        if !validate::ok(&problems) {
            return Err(
                AppError::invalid("the rule cannot be evaluated as it stands")
                    .with_details(json!({"problems": problems})),
            );
        }
        let (text, syntax, target) = match (&params.text, &params.device_id) {
            (Some(text), None) => (text.clone(), params.syntax.unwrap_or(Syntax::Ios), None),
            (None, Some(device_id)) => {
                let target = scope::target(&app, device_id)?;
                let version = crate::versions::require_latest(
                    &app,
                    &target.device.name,
                    &target.device.id,
                    crate::versions::Kind::Running,
                )?;
                let (_, text) = app.store().config_get(version.id)?;
                let syntax = params.syntax.unwrap_or_else(|| target.syntax(&app));
                (text, syntax, Some(target))
            }
            _ => {
                return Err(AppError::invalid(
                    "say what to check the rule against: `text` (a pasted configuration) or `device_id` (its latest backup)",
                ));
            }
        };
        let profile = target.as_ref().map_or("", |target| target.profile.as_str());
        let redaction = crate::redact::Redaction::for_profiles(&app, [profile]);
        let base = Finding {
            rule_id: rule.id.clone(),
            title: rule.title.clone(),
            severity: rule.severity,
            device_id: target
                .as_ref()
                .map_or_else(String::new, |target| target.device.id.clone()),
            device_name: target
                .as_ref()
                .map_or_else(String::new, |target| target.device.name.clone()),
            remediable: rule
                .remediation
                .as_ref()
                .is_some_and(|lines| !lines.is_empty()),
            ..Finding::default()
        };
        let subject = Subject::parse(&text, syntax);
        let finding = run::dress(eval::evaluate(&rule.check, &subject)?, &redaction, base);
        let fix = match (&target, finding.status) {
            (Some(target), Status::Fail) if finding.remediable => {
                remediate::lines_for(&finding, &rule, &target.device)?
            }
            _ => Vec::new(),
        };
        Ok(TestResult {
            finding,
            syntax,
            fix,
        })
    });
}
