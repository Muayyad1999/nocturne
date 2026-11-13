//! The semantics of the TextFSM language, case by case.
//!
//! The cases live in `tests/semantics/*.yml`. Each one states what the Python
//! reference implementation does for a template and an input, and
//! `tools/semantics.py check` verifies those statements against the reference
//! itself, so these tests pin the engine to the reference's actual behaviour.
//!
//! `tools/semantics.py fuzz` writes random cases in the same format; point
//! `TEXTFSM_CASES` at such a file to run them as well.

use std::path::Path;

use nocturne_textfsm::{RunError, Template, Value};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    name: String,
    template: String,
    #[serde(default)]
    input: String,
    /// The value names, when the case is about them.
    header: Option<Vec<String>>,
    /// The cells of every recorded row, in header order.
    rows: Option<Vec<Vec<Value>>>,
    /// The template must be rejected, on this line if one is given.
    template_error: Option<TemplateErrorCase>,
    /// Running must stop at an `Error` action.
    run_error: Option<RunErrorCase>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateErrorCase {
    line: Option<usize>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct RunErrorCase {
    message: Option<String>,
    rule_line: usize,
    input_line: String,
}

fn check(case: &Case) -> Result<(), String> {
    let template = match (Template::parse(&case.template), &case.template_error) {
        (Ok(template), None) => template,
        (Ok(_), Some(_)) => return Err("the template must be rejected".to_owned()),
        (Err(error), None) => return Err(format!("unexpected error: {error}")),
        (Err(error), Some(expected)) => {
            return match expected.line {
                Some(line) if line != error.line() => Err(format!(
                    "rejected on line {} instead of {line}: {error}",
                    error.line()
                )),
                _ => Ok(()),
            };
        }
    };
    if let Some(header) = &case.header
        && template.header() != header
    {
        return Err(format!(
            "header {:?}, expected {header:?}",
            template.header()
        ));
    }
    match (template.run(&case.input), &case.run_error) {
        (Ok(rows), None) => {
            let rows: Vec<Vec<Value>> = rows
                .into_iter()
                .map(|row| row.into_values().collect())
                .collect();
            let expected = case.rows.as_ref().ok_or("the case states no outcome")?;
            if &rows == expected {
                Ok(())
            } else {
                Err(format!("rows {rows:?}, expected {expected:?}"))
            }
        }
        (Ok(rows), Some(expected)) => Err(format!("rows {rows:?}, expected {expected:?}")),
        (
            Err(RunError::ErrorAction {
                message,
                rule_line,
                input_line,
            }),
            Some(expected),
        ) => {
            let error = RunErrorCase {
                message,
                rule_line,
                input_line,
            };
            if &error == expected {
                Ok(())
            } else {
                Err(format!("{error:?}, expected {expected:?}"))
            }
        }
        (Err(error), _) => Err(format!("unexpected error: {error}")),
    }
}

fn check_file(name: &str) {
    check_cases(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/semantics")
            .join(name),
    );
}

/// Runs the cases of a YAML or JSON file and reports all that fail.
fn check_cases(path: &Path) {
    let text = std::fs::read_to_string(path).unwrap();
    let cases: Vec<Case> = if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        serde_json::from_str(&text).unwrap()
    } else {
        serde_yaml_ng::from_str(&text).unwrap()
    };
    assert!(!cases.is_empty());
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            check(case)
                .err()
                .map(|reason| format!("{}: {reason}", case.name))
        })
        .collect();
    let shown: Vec<&str> = failures.iter().take(20).map(String::as_str).collect();
    assert!(
        failures.is_empty(),
        "{}: {} of {} cases fail:\n{}",
        path.display(),
        failures.len(),
        cases.len(),
        shown.join("\n")
    );
    println!("{}: {} cases", path.display(), cases.len());
}

#[test]
fn values() {
    check_file("values.yml");
}

#[test]
fn rules() {
    check_file("rules.yml");
}

#[test]
fn states() {
    check_file("states.yml");
}

#[test]
fn input() {
    check_file("input.yml");
}

#[test]
fn regex() {
    check_file("regex.yml");
}

/// Further cases from the file named by `TEXTFSM_CASES`, such as the random
/// ones written by `tools/semantics.py fuzz`.
#[test]
fn cases_from_the_environment() {
    match std::env::var_os("TEXTFSM_CASES") {
        Some(path) => check_cases(Path::new(&path)),
        None => eprintln!("TEXTFSM_CASES is not set; no additional cases"),
    }
}
