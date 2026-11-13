//! Turning findings into a change request.
//!
//! This crate never configures anything. `compliance.remediate` renders the
//! remediation lines of the failing rules for each device and hands them to
//! `changes.create`, which plans them, shows a human the exact commands and
//! applies them. If the change pipeline is not part of the build, the request
//! is refused with `unavailable` and nothing happens.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use minijinja::value::{Serde, Value as JinjaValue};
use minijinja::{Environment, UndefinedBehavior};
use nocturne_app::model::Device;
use nocturne_app::{App, AppError, AppResult};
use serde_json::{Value, json};

use super::model::{Finding, Rule};
use crate::handoff::{self, ChangeTarget};

static ENV: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    // A command with a silently empty substitution in it is the kind of thing
    // that reaches a device and does the wrong thing: refuse instead.
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env
});

/// What a remediation line may refer to.
fn context(device: Option<&Device>, section: &str) -> Value {
    let device = match device {
        Some(device) => json!({
            "id": device.id,
            "name": device.name,
            "address": device.address,
            "profile": device.profile,
            "site": device.site,
            "role": device.role,
            "tags": device.tags,
        }),
        None => json!({
            "id": "sample", "name": "SW1", "address": "192.0.2.1",
            "profile": "cisco_ios", "site": "HQ", "role": "access", "tags": [],
        }),
    };
    json!({"device": device, "section": section})
}

/// Whether a set of remediation lines is written per section (it mentions
/// `{{ section }}`), in which case it is rendered once for each section the
/// rule found wanting, and the author writes the section header themselves.
pub fn per_section(lines: &[String]) -> bool {
    lines.iter().any(|line| line.contains("section"))
        && lines.iter().any(|line| {
            ENV.render_str(line, JinjaValue::from(Serde(&context(None, "<section>"))))
                .is_ok_and(|rendered| rendered.contains("<section>"))
        })
}

/// Check that a remediation line is a template that can be rendered.
pub fn check_template(line: &str) -> Result<(), String> {
    ENV.render_str(
        line,
        JinjaValue::from(Serde(&context(None, "interface GigabitEthernet0/1"))),
    )
    .map(|_| ())
    .map_err(|err| format!("`{line}` cannot be rendered: {err}"))
}

fn render(line: &str, device: &Device, section: &str) -> AppResult<String> {
    ENV.render_str(
        line,
        JinjaValue::from(Serde(&context(Some(device), section))),
    )
    .map_err(|err| {
        AppError::invalid(format!(
            "the remediation line `{line}` cannot be rendered for `{}`: {err}",
            device.name
        ))
    })
}

/// The configuration lines that would fix one finding on one device.
pub fn lines_for(finding: &Finding, rule: &Rule, device: &Device) -> AppResult<Vec<String>> {
    let Some(template) = rule.remediation.as_ref().filter(|lines| !lines.is_empty()) else {
        return Err(AppError::invalid(format!(
            "rule `{}` ({}) carries no remediation, so Nocturne cannot write the fix for you. \
             Create the change by hand.",
            rule.id, rule.title
        )));
    };
    let mut out = Vec::new();
    if per_section(template) && !finding.sections.is_empty() {
        for section in &finding.sections {
            for line in template {
                out.push(render(line, device, section)?);
            }
        }
    } else {
        let section = finding.sections.first().map(String::as_str).unwrap_or("");
        for line in template {
            out.push(render(line, device, section)?);
        }
    }
    out.retain(|line| !line.trim().is_empty());
    Ok(out)
}

/// One device's share of a remediation change.
pub struct DeviceFix {
    pub device: Device,
    pub lines: Vec<String>,
    /// `(rule id, title)` of every rule the lines come from, in order.
    pub rules: Vec<(String, String)>,
}

/// Hand the fixes to the change pipeline and return the change id.
pub async fn hand_over(app: &App, fixes: Vec<DeviceFix>) -> AppResult<String> {
    if fixes.is_empty() {
        return Err(AppError::invalid(
            "none of the findings has a remediation to apply",
        ));
    }
    let devices = fixes.len();
    let mut rules: BTreeMap<String, String> = BTreeMap::new();
    for fix in &fixes {
        for (id, title) in &fix.rules {
            rules.insert(id.clone(), title.clone());
        }
    }
    let listed: Vec<String> = rules.values().map(|title| format!("- {title}")).collect();
    let title = match fixes.as_slice() {
        [one] => format!(
            "Fix {} compliance finding(s) on `{}`",
            one.rules.len(),
            one.device.name
        ),
        many => format!(
            "Fix {} compliance rule(s) on {} devices",
            rules.len(),
            many.len()
        ),
    };
    let description = format!(
        "Compliance remediation for {devices} device{}. The rules being fixed:\n{}\n\n\
         The commands come from the rules' own remediation lines; the change pipeline plans them \
         against each device and nothing is sent before the plan is approved.",
        if devices == 1 { "" } else { "s" },
        listed.join("\n")
    );
    let targets: Vec<ChangeTarget> = fixes
        .iter()
        .map(|fix| ChangeTarget::lines(&fix.device.id, fix.lines.clone()))
        .collect();
    handoff::create_change(app, &title, &description, "compliance", targets).await
}
