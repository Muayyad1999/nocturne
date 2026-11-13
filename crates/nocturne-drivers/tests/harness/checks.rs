//! Checks that need no recorded command output: prompts, error patterns,
//! the command policy, detection evidence and configuration handling.

use nocturne_drivers::collect::Recorded;
use nocturne_drivers::detect::{self, Detection};
use nocturne_drivers::hier::{ConfigTree, Syntax, remediate};
use nocturne_drivers::policy::CommandClass;

use super::cases::{DetectCases, ErrorSamples, Errors, PolicyCases, Prompts, Rejected};
use super::{Subject, entries, name_of};

/// `prompts.yml`: lines that must be recognised as exec, user or config
/// prompts, and lines that must not be recognised at all.
pub fn prompts(subject: &mut Subject<'_>) {
    const FILE: &str = "prompts.yml";
    if !subject.exists(FILE) {
        subject.fail(
            FILE,
            "missing: list prompts the device really shows (`exec`, `user`, `config`) and output lines that are not prompts (`not_prompts`)",
        );
        return;
    }
    let Some(cases) = subject.load::<Prompts>(FILE) else {
        return;
    };
    let platform = subject.platform.clone();
    let cli = &platform.profile.cli;
    if cases.exec.is_empty() {
        subject.fail(FILE, "`exec` is empty: list at least one real prompt");
    }
    if cases.not_prompts.is_empty() {
        subject.fail(
            FILE,
            "`not_prompts` is empty: list output lines that end like a prompt but are not one",
        );
    }
    if cli.user_prompt.is_some() && cases.user.is_empty() {
        subject.fail(
            FILE,
            "the profile has `cli.user_prompt` but `user` lists no prompt for it",
        );
    }
    if cli.config_prompt.is_some() && cases.config.is_empty() {
        subject.fail(
            FILE,
            "the profile has `cli.config_prompt` but `config` lists no prompt for it",
        );
    }

    for line in &cases.exec {
        if !platform.is_prompt(line) {
            subject.fail(FILE, format!("exec: `{line}` is not matched by cli.prompt"));
        } else if platform.is_config_prompt(line) {
            subject.fail(
                FILE,
                format!("exec: `{line}` is taken for a configuration prompt (cli.config_prompt matches it)"),
            );
        } else if platform.is_user_prompt(line) {
            subject.fail(
                FILE,
                format!("exec: `{line}` is taken for an unprivileged prompt (cli.user_prompt matches it)"),
            );
        }
    }
    for line in &cases.user {
        if !platform.is_prompt(line) {
            subject.fail(FILE, format!("user: `{line}` is not matched by cli.prompt"));
        } else if !platform.is_user_prompt(line) {
            subject.fail(
                FILE,
                format!(
                    "user: `{line}` is not recognised as an unprivileged prompt (cli.user_prompt)"
                ),
            );
        }
    }
    for line in &cases.config {
        if !platform.is_prompt(line) {
            subject.fail(
                FILE,
                format!("config: `{line}` is not matched by cli.prompt"),
            );
        } else if !platform.is_config_prompt(line) {
            subject.fail(
                FILE,
                format!("config: `{line}` is not recognised as a configuration prompt (cli.config_prompt)"),
            );
        }
    }
    for line in &cases.not_prompts {
        if platform.is_prompt(line) {
            subject.fail(
                FILE,
                format!("not_prompts: `{line}` is matched by cli.prompt, so output like it would end a command early"),
            );
        }
    }
    for (line, host) in &cases.hosts {
        match platform.prompt_host(line) {
            Some(found) if found == host => {}
            Some(found) => subject.fail(
                FILE,
                format!("hosts: `{line}` gives the hostname `{found}`, expected `{host}`"),
            ),
            None => subject.fail(
                FILE,
                format!("hosts: `{line}` gives no hostname (cli.prompt needs a `(?P<host>…)` group that matches here)"),
            ),
        }
    }
    subject.tally.prompts +=
        cases.exec.len() + cases.user.len() + cases.config.len() + cases.not_prompts.len();
}

/// `errors.yml`: rejected-command output that must be detected, and clean
/// output that must not.
pub fn errors(subject: &mut Subject<'_>) {
    const FILE: &str = "errors.yml";
    let platform = subject.platform.clone();
    let has_exec = !platform.profile.cli.errors.is_empty();
    let has_config = !platform.profile.config.errors.is_empty();
    if !subject.exists(FILE) {
        if has_exec || has_config {
            subject.fail(
                FILE,
                "missing: the profile has error patterns, so show output they must catch (`rejected`) and output they must leave alone (`clean`)",
            );
        }
        return;
    }
    let Some(cases) = subject.load::<Errors>(FILE) else {
        return;
    };
    for (section, samples, config_mode, required) in [
        ("exec", &cases.exec, false, has_exec),
        ("config", &cases.config, true, has_config),
    ] {
        if required && (samples.rejected.is_empty() || samples.clean.is_empty()) {
            subject.fail(
                FILE,
                format!("{section}: needs at least one `rejected` and one `clean` sample"),
            );
        }
        error_samples(subject, section, samples, config_mode);
    }
}

fn error_samples(
    subject: &mut Subject<'_>,
    section: &str,
    samples: &ErrorSamples,
    config_mode: bool,
) {
    const FILE: &str = "errors.yml";
    let platform = subject.platform.clone();
    let patterns = if config_mode && !platform.profile.config.errors.is_empty() {
        "config.errors"
    } else {
        "cli.errors"
    };
    for (index, sample) in samples.rejected.iter().enumerate() {
        let (output, wanted) = match sample {
            Rejected::Output(output) => (output, None),
            Rejected::Detailed { output, error } => (output, Some(error)),
        };
        match (platform.find_error(output, config_mode), wanted) {
            (None, _) => subject.fail(
                FILE,
                format!(
                    "{section}.rejected[{index}]: no {patterns} pattern matches this output, so the rejection would go unnoticed:\n{}",
                    indent(output)
                ),
            ),
            (Some(found), Some(wanted)) if found != wanted.trim() => subject.fail(
                FILE,
                format!("{section}.rejected[{index}]: the error reported is `{found}`, expected `{}`", wanted.trim()),
            ),
            _ => {}
        }
    }
    for (index, output) in samples.clean.iter().enumerate() {
        if let Some(found) = platform.find_error(output, config_mode) {
            subject.fail(
                FILE,
                format!("{section}.clean[{index}]: `{found}` is taken for an error by a {patterns} pattern, but the command worked"),
            );
        }
    }
    subject.tally.errors += samples.rejected.len() + samples.clean.len();
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("      | {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `policy.yml`: commands with the class they must get.
pub fn policy(subject: &mut Subject<'_>) {
    const FILE: &str = "policy.yml";
    if !subject.exists(FILE) {
        subject.fail(
            FILE,
            "missing: list commands under `read_only`, `change` and `dangerous`",
        );
        return;
    }
    let Some(cases) = subject.load::<PolicyCases>(FILE) else {
        return;
    };
    let platform = subject.platform.clone();
    if cases.read_only.is_empty() || cases.change.is_empty() {
        subject.fail(
            FILE,
            "needs commands under both `read_only` and `change` (anything not known to be read-only must come out as a change)",
        );
    }
    if !platform.profile.policy.dangerous.is_empty() && cases.dangerous.is_empty() {
        subject.fail(
            FILE,
            "the profile has `policy.dangerous` patterns but `dangerous` lists no command",
        );
    }
    for (label, commands, wanted) in [
        ("read_only", &cases.read_only, CommandClass::ReadOnly),
        ("change", &cases.change, CommandClass::Change),
        ("dangerous", &cases.dangerous, CommandClass::Dangerous),
    ] {
        for command in commands {
            let verdict = platform.policy().classify(command);
            if verdict.class != wanted {
                subject.fail(
                    FILE,
                    format!(
                        "{label}: `{command}` is classified {} ({})",
                        class_name(verdict.class),
                        verdict.reason
                    ),
                );
            }
        }
        subject.tally.commands += commands.len();
    }
}

fn class_name(class: CommandClass) -> &'static str {
    match class {
        CommandClass::ReadOnly => "read_only",
        CommandClass::Change => "change",
        CommandClass::Dangerous => "dangerous",
    }
}

/// `detect.yml`: banners, SNMP identities and probe output that must — or
/// must not — select this profile, judged against the whole built-in pack.
pub async fn detect(subject: &mut Subject<'_>) {
    const FILE: &str = "detect.yml";
    let platform = subject.platform.clone();
    let spec = &platform.profile.detect;
    let has_snmp = !spec.sysdescr.is_empty() || !spec.sysobjectid.is_empty();
    if !subject.exists(FILE) {
        if !spec.banner.is_empty() || has_snmp || !spec.probes.is_empty() {
            subject.fail(
                FILE,
                "missing: the profile has a `detect` section, so show evidence that must select it (`match`) and evidence that must not (`reject`)",
            );
        }
        return;
    }
    let Some(cases) = subject.load::<DetectCases>(FILE) else {
        return;
    };
    for (source, declared, tested) in [
        (
            "banner",
            !spec.banner.is_empty(),
            cases.banner.matching.len(),
        ),
        ("snmp", has_snmp, cases.snmp.matching.len()),
        ("probe", !spec.probes.is_empty(), cases.probe.matching.len()),
    ] {
        if declared && tested == 0 {
            subject.fail(
                FILE,
                format!("{source}.match is empty although the profile detects by {source}"),
            );
        }
    }
    if cases.banner.reject.len() + cases.snmp.reject.len() + cases.probe.reject.len() == 0 {
        subject.fail(
            FILE,
            "no `reject` entry anywhere: show at least one piece of evidence from another platform that must not select this profile",
        );
    }

    let registry = subject.registry;
    let id = subject.id().to_string();
    let judge =
        |subject: &mut Subject<'_>, entry: String, must_match: bool, found: Option<Detection>| {
            let selected = found.as_ref().map(|found| found.profile_id.as_str());
            match (must_match, selected == Some(id.as_str())) {
                (true, false) => subject.fail(
                    FILE,
                    format!(
                        "{entry} must select `{id}` but selects {}",
                        match &found {
                            Some(found) => format!("`{}` ({})", found.profile_id, found.evidence),
                            None => "nothing".to_string(),
                        }
                    ),
                ),
                (false, true) => subject.fail(
                    FILE,
                    format!(
                        "{entry} must not select `{id}`, but does ({})",
                        found.map(|found| found.evidence).unwrap_or_default()
                    ),
                ),
                _ => {}
            }
            subject.tally.detections += 1;
        };

    for (must_match, list) in [
        (true, &cases.banner.matching),
        (false, &cases.banner.reject),
    ] {
        for (index, text) in list.iter().enumerate() {
            let entry = format!(
                "banner.{}[{index}]",
                if must_match { "match" } else { "reject" }
            );
            judge(
                subject,
                entry,
                must_match,
                detect::from_banner(registry, text),
            );
        }
    }
    for (must_match, list) in [(true, &cases.snmp.matching), (false, &cases.snmp.reject)] {
        for (index, snmp) in list.iter().enumerate() {
            let entry = format!(
                "snmp.{}[{index}]",
                if must_match { "match" } else { "reject" }
            );
            let found = detect::from_snmp(registry, &snmp.sysdescr, &snmp.sysobjectid);
            judge(subject, entry, must_match, found);
        }
    }
    for (must_match, list) in [(true, &cases.probe.matching), (false, &cases.probe.reject)] {
        for (index, outputs) in list.iter().enumerate() {
            let entry = format!(
                "probe.{}[{index}]",
                if must_match { "match" } else { "reject" }
            );
            let mut device = outputs
                .iter()
                .fold(Recorded::new(), |device, (command, output)| {
                    device.with(command, output)
                });
            match detect::from_probes(registry, &mut device).await {
                Ok(found) => judge(subject, entry, must_match, found),
                Err(err) => subject.fail(FILE, format!("{entry}: {err}")),
            }
        }
    }
}

/// `config/<case>.txt`: a running configuration must survive cleaning
/// unchanged the second time, parse with the profile's syntax, and need no
/// commands to be turned into itself. `config/<case>.again.txt`, when
/// present, is a second fetch of the same unchanged device: after cleaning
/// the two must be identical.
pub fn configs(subject: &mut Subject<'_>) {
    let platform = subject.platform.clone();
    let syntax = platform.profile.config.syntax;
    let dir = subject.dir.join("config");
    let files: Vec<_> = entries(&dir)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .collect();
    if files.is_empty() {
        if !platform.profile.config.running.is_empty() {
            subject.fail(
                "config/",
                "missing: the profile reads a running configuration, so add one as `config/<case>.txt` (and say where it came from in `config/SOURCE.md`)",
            );
        }
        return;
    }
    if std::fs::read_to_string(dir.join("SOURCE.md")).map_or(true, |text| text.trim().is_empty()) {
        subject.fail(
            "config/SOURCE.md",
            "missing or empty: say where each configuration came from",
        );
    }

    for path in &files {
        let name = name_of(path);
        let file = format!("config/{name}");
        let Ok(raw) = std::fs::read_to_string(path) else {
            subject.fail(&file, "cannot be read as text");
            continue;
        };
        let cleaned = platform.clean_config(&raw);
        if let Some(stem) = name.strip_suffix(".again.txt") {
            // Compared against its first fetch below; nothing more to check here.
            if !dir.join(format!("{stem}.txt")).exists() {
                subject.fail(
                    &file,
                    format!("there is no `{stem}.txt` to compare it with"),
                );
            }
            continue;
        }

        let again = platform.clean_config(&cleaned);
        if again != cleaned {
            subject.fail(
                &file,
                format!(
                    "cleaning is not idempotent: a second pass of config.ignore changes the text again ({})",
                    first_difference(&cleaned, &again)
                ),
            );
        }
        let tree = ConfigTree::parse(&cleaned, syntax);
        if tree.roots.is_empty() {
            subject.fail(
                &file,
                format!("no configuration lines are left after parsing it as `{syntax:?}`"),
            );
        }
        if matches!(syntax, Syntax::Ios | Syntax::Vrp)
            && ConfigTree::parse(&tree.render(), syntax) != tree
        {
            subject.fail(
                &file,
                "does not survive parse → render → parse with the profile's syntax",
            );
        }
        let remediation = remediate(&cleaned, &cleaned, syntax);
        if !remediation.commands.is_empty() {
            subject.fail(
                &file,
                format!(
                    "turning the configuration into itself should need no commands, but gives {} (first: `{}`)",
                    remediation.commands.len(),
                    remediation.commands[0]
                ),
            );
        }

        let second = dir.join(format!("{}.again.txt", name.trim_end_matches(".txt")));
        if let Ok(second) = std::fs::read_to_string(&second) {
            let second = platform.clean_config(&second);
            if second != cleaned {
                subject.fail(
                    &file,
                    format!(
                        "differs from `{}` after cleaning, so an unchanged device would look changed: add a config.ignore pattern ({})",
                        name.replace(".txt", ".again.txt"),
                        first_difference(&cleaned, &second)
                    ),
                );
            }
        }
        subject.tally.configs += 1;
    }
}

fn first_difference(a: &str, b: &str) -> String {
    let mut left = a.lines();
    let mut right = b.lines();
    let mut number = 1;
    loop {
        match (left.next(), right.next()) {
            (Some(x), Some(y)) if x == y => number += 1,
            (None, None) => return "no differing line".to_string(),
            (x, y) => {
                return format!(
                    "line {number}: `{}` vs `{}`",
                    x.unwrap_or("<end>"),
                    y.unwrap_or("<end>")
                );
            }
        }
    }
}
