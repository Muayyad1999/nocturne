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
use crate::redact::{self, Hider};
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
///
/// A refusal names the line it is about, and a refusal is an error message:
/// it goes to the caller, into a job's record and its log. So the line is
/// named with its secret value hidden (`hide`), like everything else that is
/// said about a change.
fn validate_lines(
    lines: &[String],
    syntax: Syntax,
    hide: &Hider,
) -> AppResult<(Vec<String>, Vec<String>)> {
    let mut commands: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut section: Option<String> = None;
    for (index, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('!') || line.starts_with('#') {
            continue;
        }
        // The line as it may be quoted.
        let shown = hide.text(line);
        let shown = shown.as_str();
        let refuse = |message: String| {
            Err(AppError::invalid(message).with_details(json!({
                "index": index,
                "line": shown,
                "section": section.as_deref().map(|open| hide.line(open)),
            })))
        };
        if line.contains(['\n', '\r']) {
            return refuse(format!(
                "`{shown}` contains a line break; give one command per line"
            ));
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("interface range ") {
            return refuse(format!(
                "`{shown}` configures several ports at once, and Nocturne cannot predict (or verify, or roll back) what it does to each of them. Write one `interface …` section per port."
            ));
        }
        if syntax == Syntax::Ios {
            // Examined with its value hidden: a community or a password made
            // of lower-case letters is a value, not a keyword somebody
            // abbreviated or one Nocturne does not know — and the word that
            // is reported ends up in an error or in the reasons for the risk.
            match abbrev::inspect(shown) {
                Verdict::Full => {}
                Verdict::Refuse(message) => {
                    // The rules know a secret by the keywords in front of it,
                    // and these are the keywords that were not understood:
                    // the line is pointed at, not quoted.
                    return Err(AppError::invalid(message).with_details(json!({"index": index})));
                }
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
                        "`{shown}` is a sub-command (`{}` belongs inside a section): put the `interface …`, `line …`, `router …` or `vlan …` line it belongs to in front of it.",
                        sub.trim()
                    ));
                }
            }
            Some(open) => {
                if hier::is_global(line, syntax) {
                    let open = hide.line(open);
                    return refuse(format!(
                        "`{shown}` is a top-level command but `{open}` is still open. Put an `exit` in front of it: Nocturne will not guess whether you meant it to apply to `{open}`."
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

    // Everything this plan says about its lines — a refusal, a warning, a
    // reason — goes through this, taught the configuration it is about.
    let hide = Hider::for_platform(&platform);
    let running = cli
        .session()
        .fetch_config(false)
        .await
        .map_err(|err| hide.error(err))?;
    hide.learn(&running);
    match intent {
        Intent::Lines { lines } => hide.learn_lines(lines),
        Intent::Intended { config, .. } => hide.learn(config),
    }
    let (version, _) = app.store().config_insert(
        &device.id,
        "running",
        &running,
        "pre-change",
        "read while planning a change",
    )?;

    let mut warnings: Vec<String> = Vec::new();
    let (commands, unknown) = match intent {
        Intent::Lines { lines } => validate_lines(lines, syntax, &hide)?,
        Intent::Intended { config, exact } => {
            let remediation = hier::remediate(&running, config, syntax);
            if !remediation.supported {
                return Err(AppError::invalid(hide.text(&format!(
                    "Nocturne cannot work out the difference between two configurations on `{}` ({}). {} Give the exact commands to send instead.",
                    platform.profile.name,
                    platform.id(),
                    remediation.warnings.join(" ")
                ))));
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
        let command = hide.line(command);
        return Err(AppError::forbidden(format!(
            "`{command}` is refused: {}. The change pipeline configures devices, it does not operate them — use the terminal, deliberately, for that.",
            verdict.reason
        ))
        .with_details(json!({"command": command, "reason": verdict.reason})));
    }

    // A placeholder is never sent. A hand-written line that carries one is
    // refused before it gets here; this is for the lines computed from an
    // intended configuration that was copied out of a view which hides
    // values (or out of a device that masks its own): sending the line would
    // configure the placeholder as the key.
    if let Some(command) = commands
        .iter()
        .find(|command| redact::has_placeholder(command))
    {
        return Err(AppError::invalid(format!(
            "`{}` holds a redaction placeholder where a value should be, and sending it would configure the placeholder itself. The configuration it comes from was copied from a view that hides secret values: give the real value (read the change or the configuration with `redact: false`).",
            hide.line(command)
        )));
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
                    differences.describe(4, &hide)
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

    hide.learn_lines(&commands);
    hide.learn_lines(&rollback);
    let mut plan = Plan {
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
    // The lines are kept exactly as they will be sent; what is said about
    // them (warnings, reasons, why a resource is protected) quotes them, and
    // is kept with the values hidden.
    hide.said_about(&mut plan);
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
pub(crate) fn plan_hash(
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

    /// The checks of a hand-written list, with the engine's own rules for
    /// what is a secret.
    fn validate_lines(lines: &[String], syntax: Syntax) -> AppResult<(Vec<String>, Vec<String>)> {
        super::validate_lines(lines, syntax, &Hider::for_patterns(&[]))
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
    fn a_plan_is_hashed_as_it_was_before_its_values_were_hidden() {
        // Approvals already given name hashes made this way, and the stored
        // lines they were made over are the ones sent. What a view shows is
        // not part of it: the same lines with their values hidden hash to
        // something else, which no approval names.
        let device = Device {
            id: "d1".into(),
            name: "SW1".into(),
            ..Device::default()
        };
        let options = Options::default();
        let hash = |commands: &[&str], rollback: &[&str]| {
            plan_hash(
                &device,
                &lines(commands),
                &lines(rollback),
                "the-base",
                &options,
            )
        };
        assert_eq!(
            hash(
                &["snmp-server community N0ctComm-R0 RO"],
                &["no snmp-server community N0ctComm-R0 RO"]
            ),
            // sha256 over the device's id and name, each line, the rollback,
            // the base and the options, each followed by a NUL: worked out
            // again outside Rust, not copied from a run of this function.
            "5be058fe90afe9918b6691bac301cc64882397f70d04187a9ff41be45cadffb3"
        );
        assert_ne!(
            hash(
                &["snmp-server community <secret:1> RO"],
                &["no snmp-server community <secret:1> RO"]
            ),
            hash(
                &["snmp-server community N0ctComm-R0 RO"],
                &["no snmp-server community N0ctComm-R0 RO"]
            )
        );
    }

    #[test]
    fn a_secret_is_a_value_not_a_keyword_and_a_refusal_does_not_quote_it() {
        // A secret made of lower-case letters used to be taken for a keyword
        // Nocturne does not know (and named in the reasons for the risk), or
        // for an abbreviation of one it does (and the line refused).
        let (commands, unknown) = validate_lines(
            &lines(&[
                "enable secret lowercaseenablepw",
                "line vty 0 4",
                "password pass",
                "exit",
            ]),
            Syntax::Ios,
        )
        .unwrap();
        assert_eq!(
            commands,
            vec![
                "enable secret lowercaseenablepw",
                "line vty 0 4",
                "password pass",
                "exit"
            ],
            "what will be sent is what was written"
        );
        assert!(unknown.is_empty(), "{unknown:?}");

        // A refusal quotes the line with the value hidden…
        let err = validate_lines(
            &lines(&["neighbor 192.0.2.1 password lowercasebgppw"]),
            Syntax::Ios,
        )
        .unwrap_err();
        assert!(
            err.message
                .contains("`neighbor 192.0.2.1 password <hidden>` is a sub-command"),
            "{}",
            err.message
        );
        let details = err.details.clone().unwrap();
        assert_eq!(details["index"], 0);
        assert_eq!(details["line"], "neighbor 192.0.2.1 password <hidden>");
        assert!(
            !serde_json::to_string(&err)
                .unwrap()
                .contains("lowercasebgppw")
        );

        // …and where the keywords themselves were not understood, which is
        // what a secret is known by, it points at the line and quotes nothing.
        let err = validate_lines(
            &lines(&["hostname SW1", "en sec lowercaseenablepw"]),
            Syntax::Ios,
        )
        .unwrap_err();
        assert!(
            err.message.contains("`en` is an abbreviation"),
            "{}",
            err.message
        );
        assert_eq!(
            err.details.clone().unwrap(),
            serde_json::json!({"index": 1})
        );
        assert!(
            !serde_json::to_string(&err)
                .unwrap()
                .contains("lowercaseenablepw")
        );
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
