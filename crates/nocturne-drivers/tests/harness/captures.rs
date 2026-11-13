//! `captures/<case>/`: recorded command output and the model data it must produce.
//!
//! Each `<command slug>.txt` is the raw output of one command the profile's
//! collectors run (see [`slug`]); `expect.yml` lists, per kind, the exact row
//! count and the rows and fields that must come out; `SOURCE.md` says where
//! the output was recorded.

use std::collections::{BTreeSet, HashMap};

use nocturne_drivers::collect::{Collector, Recorded, Snapshot, command_slug as slug};
use nocturne_drivers::model::Kind;
use nocturne_drivers::profile::InterfaceNames;
use serde_json::Value;

use super::cases::{Expect, KindExpect};
use super::{Subject, entries, name_of};

/// Which kinds the captures of a profile cover.
#[derive(Default)]
pub struct Coverage {
    /// Covered by at least one capture of real device output.
    pub real: BTreeSet<Kind>,
    /// Covered by any capture.
    pub any: BTreeSet<Kind>,
}

pub async fn run(subject: &mut Subject<'_>) -> Coverage {
    let platform = subject.platform.clone();
    let mut coverage = Coverage::default();
    let steps: Vec<(Kind, usize, String)> = platform
        .profile
        .collect
        .iter()
        .flat_map(|(kind, steps)| {
            steps
                .iter()
                .enumerate()
                .map(|(index, step)| (*kind, index, step.command.trim().to_string()))
        })
        .collect();

    // File name → command, over every command the profile's collectors run.
    let mut commands: HashMap<String, String> = HashMap::new();
    for (kind, index, command) in &steps {
        if let Some(other) = commands.insert(slug(command), command.clone())
            && other != *command
        {
            subject.fail(
                "captures/",
                format!(
                    "collect.{kind}[{index}]: `{command}` and `{other}` would both be recorded as `{}.txt`; the harness cannot tell their captures apart",
                    slug(command)
                ),
            );
        }
    }

    let cases: Vec<_> = entries(&subject.dir.join("captures"))
        .into_iter()
        .filter(|path| path.is_dir())
        .collect();
    if cases.is_empty() {
        if !steps.is_empty() {
            subject.fail(
                "captures/",
                "missing: the profile has collectors, so record what their commands print (one directory per device, see docs/profiles.md)",
            );
        }
        return coverage;
    }

    let collector = Collector::new(subject.registry.expressions());
    // Steps whose command had recorded output in a case that collected their kind.
    let mut exercised: BTreeSet<(Kind, usize)> = BTreeSet::new();
    for case_dir in cases {
        let case = format!("captures/{}", name_of(&case_dir));
        let expect_file = format!("{case}/expect.yml");
        if !subject.exists(&expect_file) {
            subject.fail(
                &expect_file,
                "missing: say what the recorded output must produce (`kinds:` with a `count` and `rows` per kind)",
            );
            continue;
        }
        let Some(expect) = subject.load::<Expect>(&expect_file) else {
            continue;
        };
        if std::fs::read_to_string(case_dir.join("SOURCE.md"))
            .map_or(true, |text| text.trim().is_empty())
        {
            subject.fail(
                &format!("{case}/SOURCE.md"),
                "missing or empty: say where each capture file was recorded (for the ntc-templates corpus, the path of the `.raw` file)",
            );
        }
        if expect.kinds.is_empty() {
            subject.fail(&expect_file, "`kinds` is empty: nothing would be checked");
            continue;
        }

        let mut device = Recorded::new();
        let mut recorded: Vec<(String, String)> = Vec::new(); // (file, command)
        for path in entries(&case_dir) {
            let name = name_of(&path);
            let Some(stem) = name.strip_suffix(".txt") else {
                continue;
            };
            let file = format!("{case}/{name}");
            let Some(command) = commands.get(stem) else {
                let mut known: Vec<String> =
                    commands.keys().map(|slug| format!("{slug}.txt")).collect();
                known.sort();
                subject.fail(
                    &file,
                    format!(
                        "no collector of this profile runs a command with this file name; the names are {}",
                        known.join(", ")
                    ),
                );
                continue;
            };
            match std::fs::read_to_string(&path) {
                Ok(output) => {
                    device = device.with(command, &output);
                    recorded.push((file, command.clone()));
                }
                Err(err) => subject.fail(&file, format!("cannot be read as text: {err}")),
            }
        }

        let kinds: Vec<Kind> = expect.kinds.keys().copied().collect();
        let snapshot = collector.collect(&mut device, &platform, &kinds).await;

        for (file, command) in &recorded {
            if !device.commands_run().iter().any(|run| run == command) {
                subject.fail(
                    file,
                    format!(
                        "`{command}` was never asked for while collecting {}: list the kind that uses it in expect.yml, or remove the file",
                        kinds.iter().map(Kind::to_string).collect::<Vec<_>>().join(", ")
                    ),
                );
            }
        }
        for (kind, index, command) in &steps {
            if kinds.contains(kind) && recorded.iter().any(|(_, recorded)| recorded == command) {
                exercised.insert((*kind, *index));
            }
        }

        let before = subject.failures.len();
        problems(subject, &expect_file, &expect, &snapshot);
        for (kind, wanted) in &expect.kinds {
            rows(subject, &expect_file, *kind, wanted, &snapshot);
        }
        if subject.failures.len() == before {
            for kind in &kinds {
                coverage.any.insert(*kind);
                if !expect.synthetic {
                    coverage.real.insert(*kind);
                }
            }
        }
        subject.tally.captures += 1;
    }

    for (kind, index, command) in &steps {
        if !exercised.contains(&(*kind, *index)) {
            subject.fail(
                "captures/",
                format!(
                    "collect.{kind}[{index}] (`{command}`) is not exercised by any capture: record its output as `{}.txt` in a capture directory whose expect.yml lists {kind}",
                    slug(command)
                ),
            );
        }
    }
    coverage
}

/// The collection must report exactly the problems the case expects.
fn problems(subject: &mut Subject<'_>, file: &str, expect: &Expect, snapshot: &Snapshot) {
    let mut unexpected: Vec<&nocturne_drivers::collect::Problem> =
        snapshot.problems.iter().collect();
    for wanted in &expect.problems {
        match unexpected.iter().position(|problem| {
            problem.kind == wanted.kind
                && problem.code == wanted.code
                && problem.message.contains(&wanted.contains)
        }) {
            Some(position) => {
                unexpected.remove(position);
            }
            None => subject.fail(
                file,
                format!(
                    "problems: expected a {:?} problem for {}{}, but the collection reported {}",
                    wanted.code,
                    wanted.kind,
                    if wanted.contains.is_empty() {
                        String::new()
                    } else {
                        format!(" containing `{}`", wanted.contains)
                    },
                    if snapshot.problems.is_empty() {
                        "none".to_string()
                    } else {
                        snapshot
                            .problems
                            .iter()
                            .map(|problem| {
                                format!(
                                    "{:?} for {}: {}",
                                    problem.code, problem.kind, problem.message
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(" | ")
                    }
                ),
            ),
        }
    }
    for problem in unexpected {
        subject.fail(
            file,
            format!(
                "{}: the collection reported a problem ({:?}): {}",
                problem.kind, problem.code, problem.message
            ),
        );
    }
}

/// Exact row count; every listed row must be there with every listed field equal.
fn rows(
    subject: &mut Subject<'_>,
    file: &str,
    kind: Kind,
    wanted: &KindExpect,
    snapshot: &Snapshot,
) {
    let Some(actual) = snapshot.rows(kind) else {
        // Why it is missing is already reported as a problem, unless the case expects that problem.
        if wanted.count.is_some() || !wanted.rows.is_empty() {
            subject.fail(
                file,
                format!("{kind}: nothing was collected, but rows are expected"),
            );
        }
        return;
    };
    let count = match (wanted.count, kind.is_single_row()) {
        (Some(count), _) => count,
        (None, true) => 1,
        (None, false) => {
            subject.fail(
                file,
                format!("{kind}: `count` is missing (the exact number of rows the capture must produce; it produced {})", actual.len()),
            );
            return;
        }
    };
    if actual.len() != count {
        subject.fail(
            file,
            format!(
                "{kind}: {} rows were collected, expected exactly {count}",
                actual.len()
            ),
        );
    }
    if wanted.rows.is_empty() {
        subject.fail(
            file,
            format!("{kind}: `rows` is empty: list rows (and the fields that matter) read from the raw output"),
        );
    }

    let verbatim = subject.platform.profile.interface_names == InterfaceNames::Verbatim;
    let mut claimed = vec![false; actual.len()];
    for (number, row) in wanted.rows.iter().enumerate() {
        let Some((key, key_value)) = row.first() else {
            subject.fail(
                file,
                format!("{kind}.rows[{number}]: the row lists no field"),
            );
            continue;
        };
        let found = actual.iter().enumerate().position(|(position, candidate)| {
            !claimed[position]
                && candidate
                    .get(key)
                    .is_some_and(|value| same(value, key_value))
        });
        let Some(position) = found else {
            let present: Vec<String> = actual
                .iter()
                .filter_map(|candidate| candidate.get(key))
                .take(12)
                .map(Value::to_string)
                .collect();
            subject.fail(
                file,
                format!(
                    "{kind}.rows[{number}]: no collected row has {key} = {key_value}; the collected rows have {key} = {}{}{}",
                    present.join(", "),
                    if actual.len() > 12 { ", …" } else { "" },
                    if !verbatim && kind.interface_fields().contains(&key.as_str()) {
                        " (interface names are expanded to their long form; set `interface_names: verbatim` in the profile if this platform's names must be kept as printed)"
                    } else {
                        ""
                    }
                ),
            );
            continue;
        };
        claimed[position] = true;
        let mut differences = Vec::new();
        for (field, value) in row {
            match actual[position].get(field) {
                Some(have) => differ(field, have, value, &mut differences),
                None => differences.push(format!("`{field}` is not a field of {kind} rows")),
            }
        }
        for difference in differences {
            subject.fail(
                file,
                format!("{kind}.rows[{number}] ({key} = {key_value}): {difference}"),
            );
        }
        subject.tally.rows += 1;
    }
}

/// Equality as the expectations mean it: numbers by value, the rest exactly.
fn same(have: &Value, want: &Value) -> bool {
    match (have, want) {
        (Value::Number(have), Value::Number(want)) => have.as_f64() == want.as_f64(),
        _ => have == want,
    }
}

/// Describe where `have` differs from `want`. Of an object, only the fields
/// that `want` lists are compared; everything else must be equal.
fn differ(path: &str, have: &Value, want: &Value, out: &mut Vec<String>) {
    match (have, want) {
        (Value::Object(have), Value::Object(want)) => {
            for (field, want) in want {
                match have.get(field) {
                    Some(have) => differ(&format!("{path}.{field}"), have, want, out),
                    None => out.push(format!("`{path}.{field}` is missing, expected {want}")),
                }
            }
        }
        (Value::Array(have_items), Value::Array(want_items))
            if have_items.len() == want_items.len()
                && have_items
                    .iter()
                    .zip(want_items)
                    .all(|(have, want)| same(have, want)) => {}
        _ if same(have, want) => {}
        _ => out.push(format!("`{path}` is {have}, expected {want}")),
    }
}
