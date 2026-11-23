//! Every profile in `docs/profiles.md` must really load.
//!
//! The guide teaches by example, so an example that does not work teaches the
//! wrong thing. Each fenced `yaml` block whose first line is `# profile` is
//! pulled out of the document and loaded on top of the built-in pack, exactly
//! as a profile a reader pastes into Nocturne would be.

use std::path::{Path, PathBuf};

use nocturne_drivers::profile::{Maturity, Origin, Registry};

fn guide() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/profiles.md")
}

/// The fenced ```yaml blocks whose first line is `# profile`, with the line
/// the block starts on.
fn profile_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut lines = markdown.lines().enumerate();
    while let Some((number, line)) = lines.next() {
        if line.trim_end() != "```yaml" {
            continue;
        }
        let mut body = String::new();
        for (_, line) in lines.by_ref() {
            if line.trim_end() == "```" {
                break;
            }
            body.push_str(line);
            body.push('\n');
        }
        if body
            .lines()
            .next()
            .is_some_and(|first| first.trim() == "# profile")
        {
            blocks.push((number + 2, body));
        }
    }
    blocks
}

#[test]
fn every_profile_in_the_guide_loads() {
    let markdown = std::fs::read_to_string(guide()).expect("docs/profiles.md is there");
    let blocks = profile_blocks(&markdown);
    assert!(
        blocks.len() >= 10,
        "only {} profile examples were found in docs/profiles.md; a fenced ```yaml block is \
         taken as an example when its first line is `# profile`",
        blocks.len()
    );

    let mut failures = Vec::new();
    for (line, yaml) in &blocks {
        // A fresh registry per example: the built-in pack is there to be
        // extended (`extends: _cli_ios_like`), and one example cannot disturb
        // the next.
        let (mut registry, errors) = Registry::builtin();
        assert!(errors.is_empty(), "built-in profiles: {errors:#?}");
        match registry.add(yaml, Origin::User) {
            Ok(platform) => {
                let profile = &platform.profile;
                if profile.id.is_empty() {
                    failures.push(format!("docs/profiles.md:{line}: the example has no `id`"));
                }
                if profile.name.is_empty() {
                    failures.push(format!(
                        "docs/profiles.md:{line}: `{}` has no `name`, so the UI would show a blank row",
                        profile.id
                    ));
                }
                if profile.maturity == Maturity::Verified {
                    failures.push(format!(
                        "docs/profiles.md:{line}: `{}` claims `maturity: verified`, which no \
                         example can honestly claim",
                        profile.id
                    ));
                }
            }
            Err(err) => failures.push(format!(
                "docs/profiles.md:{line}: the example does not load:\n    {}",
                err.problems.join("\n    ")
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "\n{} broken example(s) in the guide:\n\n{}\n",
        failures.len(),
        failures.join("\n")
    );
    println!("{} profile examples in docs/profiles.md load", blocks.len());
}

/// The guide once taught `ssCpuIdle` as a CPU load: the percentage of the
/// last minute spent *idle*, read as busy. Under `cpu` it must say that it
/// is turned round.
#[test]
fn no_example_in_the_guide_reads_an_idle_percentage_as_a_load() {
    const SS_CPU_IDLE: &str = "1.3.6.1.4.1.2021.11.11";
    let markdown = std::fs::read_to_string(guide()).expect("docs/profiles.md is there");
    let mut looked_at = 0;
    for (line, yaml) in profile_blocks(&markdown) {
        let (mut registry, _) = Registry::builtin();
        let Ok(platform) = registry.add(&yaml, Origin::User) else {
            continue;
        };
        for entry in &platform.profile.snmp.cpu {
            let Some(oid) = entry.target() else { continue };
            if oid.trim().trim_start_matches('.').starts_with(SS_CPU_IDLE) {
                looked_at += 1;
                assert_eq!(
                    entry.complement,
                    Some(100.0),
                    "docs/profiles.md:{line}: ssCpuIdle under `cpu` without `complement: 100`"
                );
            }
        }
    }
    assert!(
        looked_at > 0,
        "the guide shows how an idle figure is turned round"
    );
}

/// The worked example is built up step by step: every step must be a profile
/// in its own right, and the steps must build on one another rather than
/// restate the whole file each time.
#[test]
fn the_worked_example_grows_with_every_step() {
    let markdown = std::fs::read_to_string(guide()).expect("docs/profiles.md is there");
    let steps: Vec<String> = profile_blocks(&markdown)
        .into_iter()
        .map(|(_, yaml)| yaml)
        .filter(|yaml| yaml.contains("id: arista_eos"))
        .collect();
    assert!(
        steps.len() >= 5,
        "the guide should build one example up in steps; found {}",
        steps.len()
    );
    assert!(
        steps
            .first()
            .is_some_and(|first| !first.contains("collect:")),
        "the first step should be the smallest thing that works"
    );
    let last = steps.last().expect("a last step");
    for expected in ["extends:", "detect:", "collect:", "maturity:", "snmp:"] {
        assert!(
            last.contains(expected),
            "the finished example should show `{expected}`"
        );
    }
}
