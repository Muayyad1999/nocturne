//! Turning an intent into the exact commands that will be sent, and working
//! out what they will do before anybody is asked to approve them.
//!
//! Everything here happens over one CLI session and changes nothing on the
//! device: it reads the running configuration, computes the commands,
//! *applies them to a copy of the configuration in memory* to predict the
//! result, computes the rollback from the same pair, and classifies the lot.
//! The prediction is what the apply compares the device against afterwards,
//! so what is shown, what is sent and what is verified are the same thing.

use nocturne_app::App;
use nocturne_app::collect;
use nocturne_app::connect::Cli;
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::model::Device;
use nocturne_app::util::now_ms;
use nocturne_drivers::hier::{self, ApplyReport, ConfigTree, Syntax};
use nocturne_drivers::model::{Interface, Kind, Neighbor};
use nocturne_drivers::policy::CommandClass;
use nocturne_drivers::profile::Platform;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::abbrev::{self, Verdict};
use crate::compare;
use crate::model::{GuardPlan, Intent, Options, Plan};
use crate::protect;
use crate::risk;
use crate::settings::Settings;

/// Sub-commands that only mean something inside a section. Typing one at the
/// top level is a mistake, not a command to guess at.
const SUB_COMMANDS: &[&str] = &[
    "switchport",
    "shutdown",
    "description ",
    "ip address ",
    "ip ospf ",
    "ip access-group ",
    "ip helper-address ",
    "speed ",
    "duplex ",
    "channel-group ",
    "spanning-tree portfast",
    "spanning-tree bpduguard",
    "spanning-tree cost",
    "spanning-tree port-priority",
    "exec-timeout ",
    "transport input",
    "transport output",
    "access-class ",
    "network ",
    "neighbor ",
    "redistribute ",
    "passive-interface ",
    "standby ",
    "encapsulation ",
    "vrf forwarding ",
    "permit ",
    "deny ",
    "remark ",
    "name ",
];

/// Apply `commands` to `running` and give back the predicted configuration.
pub fn predict(running: &str, commands: &[String], syntax: Syntax) -> (ConfigTree, ApplyReport) {
    let mut tree = ConfigTree::parse(running, syntax);
    let report = tree.apply(commands, syntax);
    (tree, report)
}

/// Check a hand-written list of configuration lines.
///
/// Returns the lines as they will be sent, plus the words in them Nocturne
/// does not recognise (which stop the change from being called low risk).
fn validate_lines(lines: &[String], syntax: Syntax) -> AppResult<(Vec<String>, Vec<String>)> {
    let mut commands: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut section: Option<String> = None;
    for raw in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('!') || line.starts_with('#') {
            continue;
        }
        let refuse = |message: String| {
            Err(AppError::invalid(message)
                .with_details(json!({"line": line, "section": section.clone()})))
        };
        if line.contains(['\n', '\r']) {
            return refuse(format!(
                "`{line}` contains a line break; give one command per line"
            ));
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("interface range ") {
            return refuse(format!(
                "`{line}` configures several ports at once, and Nocturne cannot predict (or verify, or roll back) what it does to each of them. Write one `interface …` section per port."
            ));
        }
        if syntax == Syntax::Ios {
            match abbrev::inspect(line) {
                Verdict::Full => {}
                Verdict::Refuse(message) => return refuse(message),
                Verdict::Unknown(word) => {
                    if !unknown.contains(&word) {
                        unknown.push(word);
                    }
                }
            }
        }
        let opens = hier::opens_section(line, syntax);
        if lower == "exit" || lower == "quit" || lower == "end" {
            section = None;
            commands.push(line.to_string());
            continue;
        }
        if opens {
            section = Some(line.to_string());
            commands.push(line.to_string());
            continue;
        }
        match &section {
            None => {
                if let Some(sub) = SUB_COMMANDS.iter().find(|sub| {
                    lower.starts_with(**sub)
                        || lower
                            .strip_prefix("no ")
                            .is_some_and(|rest| rest.starts_with(**sub))
                }) {
                    return refuse(format!(
                        "`{line}` is a sub-command (`{}` belongs inside a section): put the `interface …`, `line …`, `router …` or `vlan …` line it belongs to in front of it.",
                        sub.trim()
                    ));
                }
            }
            Some(open) => {
                if hier::is_global(line, syntax) {
                    return refuse(format!(
                        "`{line}` is a top-level command but `{open}` is still open. Put an `exit` in front of it: Nocturne will not guess whether you meant it to apply to `{open}`."
                    ));
                }
            }
        }
        commands.push(line.to_string());
    }
    if commands.is_empty() {
        return Err(AppError::invalid(
            "there are no configuration lines to send",
        ));
    }
    Ok((commands, unknown))
}

/// Drop the commands that remove configuration, keeping the sections that
/// still have something to do. This is what makes an `Intended` change
/// additive: the configuration must be *present*, not exclusive.
fn strip_removals(commands: &[String], syntax: Syntax) -> Vec<String> {
    let kept: Vec<&String> = commands
        .iter()
        .filter(|line| {
            let lower = line.trim().to_ascii_lowercase();
            !(lower.starts_with("no ")
                || lower.starts_with("undo ")
                || lower.starts_with("default interface ")
                || lower.starts_with("delete ")
                || lower.contains("switchport trunk allowed vlan remove "))
        })
        .collect();

    // A section that has nothing left to do in it is not worth entering.
    let leaves = |line: &str| matches!(line.trim(), "exit" | "quit" | "end" | "return");
    let mut out: Vec<String> = Vec::new();
    let mut index = 0;
    while index < kept.len() {
        let line = kept[index];
        if !hier::opens_section(line, syntax) {
            out.push(line.clone());
            index += 1;
            continue;
        }
        let mut body = 0;
        let mut end = index + 1;
        while end < kept.len() {
            let next = kept[end].trim();
            if leaves(next)
                || (hier::opens_section(next, syntax) && !hier::opens_nested(next, syntax))
            {
                break;
            }
            body += 1;
            end += 1;
        }
        if body > 0 {
            out.push(line.clone());
            index += 1;
            continue;
        }
        index = end;
        if index < kept.len() && matches!(kept[index].trim(), "exit" | "quit") {
            index += 1;
        }
    }
    out
}

/// The commands a change means on this device, and why they are or are not
/// safe. The session is used to read; nothing is written.
pub async fn plan_on(
    app: &App,
    cli: &mut Cli,
    intent: &Intent,
    options: &Options,
) -> AppResult<Plan> {
    let device = cli.device.clone();
    let platform = cli.platform.clone();
    let syntax = platform.profile.config.syntax;
    let settings = Settings::read(app);

    let running = cli.session().fetch_config(false).await?;
    let (version, _) = app.store().config_insert(
        &device.id,
        "running",
        &running,
        "pre-change",
        "read while planning a change",
    )?;

    let mut warnings: Vec<String> = Vec::new();
    let (commands, unknown) = match intent {
        Intent::Lines { lines } => validate_lines(lines, syntax)?,
        Intent::Intended { config, exact } => {
            let remediation = hier::remediate(&running, config, syntax);
            if !remediation.supported {
                return Err(AppError::invalid(format!(
                    "Nocturne cannot work out the difference between two configurations on `{}` ({}). {} Give the exact commands to send instead.",
                    platform.profile.name,
                    platform.id(),
                    remediation.warnings.join(" ")
                )));
            }
            warnings.extend(remediation.warnings);
            let commands = if *exact {
                remediation.commands
            } else {
                strip_removals(&remediation.commands, syntax)
            };
            (commands, Vec::new())
        }
    };

    // The pipeline configures; it does not operate the device.
    if let Some((command, verdict)) = commands
        .iter()
        .map(|command| (command, platform.policy().classify(command)))
        .find(|(_, verdict)| verdict.class == CommandClass::Dangerous)
    {
        return Err(AppError::forbidden(format!(
            "`{command}` is refused: {}. The change pipeline configures devices, it does not operate them — use the terminal, deliberately, for that.",
            verdict.reason
        ))
        .with_details(json!({"command": command, "reason": verdict.reason})));
    }

    // What the device will look like afterwards.
    let base = ConfigTree::parse(&running, syntax);
    let base_render = base.render();
    let (predicted_tree, report) = predict(&running, &commands, syntax);
    let predicted = predicted_tree.render();
    warnings.extend(report.warnings.clone());
    for line in &report.unmodelled {
        warnings.push(format!(
            "Nocturne cannot predict what `{line}` changes, so the configuration after this change cannot be fully checked."
        ));
    }
    let prediction_supported = report.supported && report.unmodelled.is_empty();
    let diff = hier::text_diff(&base_render, &predicted, Some(3));
    if commands.is_empty() {
        warnings.push(format!(
            "There is nothing to do on `{}`: its configuration already matches.",
            device.name
        ));
    }

    // The rollback is the same difference in the other direction — and it is
    // checked by applying it to the prediction.
    let mut rollback = Vec::new();
    let mut rollback_ok = commands.is_empty();
    if prediction_supported && !commands.is_empty() {
        let back = hier::remediate(&predicted, &base_render, syntax);
        rollback = back.commands;
        if !back.supported {
            warnings.push(
                "Nocturne cannot compute how to undo this change on this platform; it would have to be undone by hand."
                    .into(),
            );
        } else if rollback.is_empty() {
            warnings.push(
                "Nocturne could not derive any command that undoes this change; there would be nothing to roll back to."
                    .into(),
            );
        } else {
            let (restored, _) = predict(&predicted, &rollback, syntax);
            let differences = compare::differences(&base, &restored);
            if differences.is_empty() {
                rollback_ok = true;
            } else {
                warnings.push(format!(
                    "The computed rollback does not put the configuration back exactly ({}); after a failure the device may need attention by hand.",
                    differences.describe(4)
                ));
            }
        }
        for warning in back.warnings {
            warnings.push(format!("Undoing this change: {warning}"));
        }
    } else if !prediction_supported {
        warnings.push(
            "Because the result cannot be predicted, no rollback could be computed from it.".into(),
        );
    }

    // What this device is, and what the commands would touch.
    let neighbors: Vec<Neighbor> = collect::latest(app, &device.id, Kind::Neighbors)?
        .map(|stored| stored.typed())
        .unwrap_or_default();
    let interfaces: Vec<Interface> = collect::latest(app, &device.id, Kind::Interfaces)?
        .map(|stored| stored.typed())
        .unwrap_or_default();
    let survey = protect::survey(&device, &base, syntax, &neighbors, &interfaces);
    warnings.extend(survey.notes.clone());
    let protected_hits = protect::detect(&device, &survey, &commands, syntax);

    let guard_spec = platform.profile.config.guard.clone();
    let risk = risk::assess(&risk::Inputs {
        commands: &commands,
        syntax,
        protected_hits: &protected_hits,
        rollback_ok,
        maturity: platform.profile.maturity,
        has_guard: guard_spec.is_some(),
        unknown_keywords: &unknown,
        prediction_supported,
    });

    let guard = guard_plan(&guard_spec, options, &settings, risk.level);
    if guard.is_none()
        && protected_hits
            .iter()
            .any(|hit| protect::is_management_or_access(&hit.kind))
    {
        warnings.push(match guard_spec {
            Some(_) => "This change touches the management path and no dead-man switch will be armed; set `guard_minutes` so the device undoes the change by itself if it cuts the session.".into(),
            None => format!(
                "`{}` has no dead-man switch, and this change touches the way Nocturne reaches `{}`: if it cuts the session, the device will not undo it by itself.",
                platform.profile.name, device.name
            ),
        });
    }

    let plan = Plan {
        plan_hash: plan_hash(&device, &commands, &rollback, &version.sha256, options),
        commands,
        rollback,
        diff,
        risk,
        protected_hits,
        warnings,
        base_version_id: version.id,
        base_sha256: version.sha256,
        predicted_sha256: if prediction_supported {
            hex::encode(Sha256::digest(predicted.as_bytes()))
        } else {
            String::new()
        },
        guard,
        planned_at: now_ms(),
        profile: platform.profile.id.clone(),
        maturity: platform.profile.maturity.as_str().to_string(),
    };
    Ok(plan)
}

/// The dead-man switch that will be armed for this plan, if any.
fn guard_plan(
    spec: &Option<nocturne_drivers::profile::Guard>,
    options: &Options,
    settings: &Settings,
    level: crate::model::RiskLevel,
) -> Option<GuardPlan> {
    let spec = spec.as_ref()?;
    let wanted = match options.guard_minutes {
        Some(0) => return None,
        Some(minutes) => minutes,
        None if level >= crate::model::RiskLevel::High => settings.guard_minutes,
        None => return None,
    };
    let minutes = wanted.clamp(1, 240);
    let substitute = |commands: &[String]| -> Vec<String> {
        commands
            .iter()
            .map(|command| command.replace("{minutes}", &minutes.to_string()))
            .collect()
    };
    Some(GuardPlan {
        name: spec.name.clone(),
        minutes,
        arm: substitute(&spec.arm),
        disarm: substitute(&spec.disarm),
        effect: spec.effect.clone(),
    })
}

/// A plan is approved by its hash: anything that changes what would be sent,
/// what would undo it, which device it is for, what it was computed against
/// or how it would be applied makes the approval invalid.
fn plan_hash(
    device: &Device,
    commands: &[String],
    rollback: &[String],
    base_sha256: &str,
    options: &Options,
) -> String {
    let mut hasher = Sha256::new();
    let mut field = |text: &str| {
        hasher.update(text.as_bytes());
        hasher.update([0u8]);
    };
    field(&device.id);
    field(&device.name);
    for command in commands {
        field(command);
    }
    field("--rollback--");
    for command in rollback {
        field(command);
    }
    field(base_sha256);
    field(&serde_json::to_string(options).unwrap_or_default());
    hex::encode(hasher.finalize())
}

/// Read the platform of a device without connecting to it (for the plans
/// stored in a change).
pub fn platform_of(app: &App, device: &Device) -> Option<std::sync::Arc<Platform>> {
    nocturne_app::profiles::platform(app, &device.profile).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn hand_written_lines_keep_their_order_and_drop_blanks() {
        let (commands, unknown) = validate_lines(
            &lines(&[
                "interface GigabitEthernet0/2",
                "",
                "! a comment",
                " description desk 14 ",
                "exit",
                "ntp server 192.0.2.1",
            ]),
            Syntax::Ios,
        )
        .unwrap();
        assert_eq!(
            commands,
            vec![
                "interface GigabitEthernet0/2",
                "description desk 14",
                "exit",
                "ntp server 192.0.2.1"
            ]
        );
        assert!(unknown.is_empty());
    }

    #[test]
    fn a_sub_command_without_its_section_is_refused() {
        let err = validate_lines(&lines(&["switchport access vlan 10"]), Syntax::Ios).unwrap_err();
        assert_eq!(err.code, "invalid");
        assert!(err.message.contains("sub-command"), "{}", err.message);

        // …and a top-level command inside an open section is refused too.
        let err = validate_lines(
            &lines(&["interface Gi0/1", "description x", "hostname OTHER"]),
            Syntax::Ios,
        )
        .unwrap_err();
        assert!(err.message.contains("exit"), "{}", err.message);

        // With the `exit` it is fine.
        assert!(
            validate_lines(
                &lines(&["interface Gi0/1", "description x", "exit", "hostname OTHER"]),
                Syntax::Ios
            )
            .is_ok()
        );
    }

    #[test]
    fn interface_range_and_abbreviations_are_refused() {
        let err = validate_lines(&lines(&["interface range Gi0/1 - 4"]), Syntax::Ios).unwrap_err();
        assert!(err.message.contains("several ports"), "{}", err.message);

        let err = validate_lines(&lines(&["int gi0/1"]), Syntax::Ios).unwrap_err();
        assert!(err.message.contains("`interface`"), "{}", err.message);
    }

    #[test]
    fn unknown_words_are_reported_but_the_line_is_kept() {
        let (commands, unknown) =
            validate_lines(&lines(&["frobnicate the-widget"]), Syntax::Ios).unwrap();
        assert_eq!(commands, vec!["frobnicate the-widget"]);
        assert_eq!(unknown, vec!["frobnicate"]);
    }

    #[test]
    fn additive_intent_drops_removals_and_the_sections_they_emptied() {
        let commands = lines(&[
            "vlan 30",
            "name Voice",
            "exit",
            "interface GigabitEthernet0/1",
            "switchport trunk allowed vlan add 30",
            "switchport trunk allowed vlan remove 20",
            "exit",
            "interface GigabitEthernet0/2",
            "no description",
            "exit",
            "no vlan 20",
        ]);
        assert_eq!(
            strip_removals(&commands, Syntax::Ios),
            vec![
                "vlan 30",
                "name Voice",
                "exit",
                "interface GigabitEthernet0/1",
                "switchport trunk allowed vlan add 30",
                "exit",
            ]
        );
    }
}
