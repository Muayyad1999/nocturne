//! The semantics of the TextFSM language, case by case.
//!
//! The cases live in `tests/semantics/*.yml`. Each one states what the Python
//! reference implementation does for a template and an input, and
//! `tools/check_semantics.py` checks those statements against the reference
//! itself, so these tests pin the engine to the reference's actual behaviour.

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
                Some(line) if line != error.line() => {
                    Err(format!("rejected on line {} instead of {line}: {error}", error.line()))
                }
                _ => Ok(()),
            };
        }
    };
    if let Some(header) = &case.header
        && template.header() != header
    {
        return Err(format!("header {:?}, expected {header:?}", template.header()));
    }
    match (template.run(&case.input), &case.run_error) {
        (Ok(rows), None) => {
            let rows: Vec<Vec<Value>> =
                rows.into_iter().map(|row| row.into_values().collect()).collect();
            let expected = case.rows.as_ref().ok_or("the case states no outcome")?;
            if &rows == expected { Ok(()) } else { Err(format!("rows {rows:?}, expected {expected:?}")) }
        }
        (Ok(rows), Some(expected)) => Err(format!("rows {rows:?}, expected {expected:?}")),
        (Err(RunError::ErrorAction { message, rule_line, input_line }), Some(expected)) => {
            let error = RunErrorCase { message, rule_line, input_line };
            if &error == expected { Ok(()) } else { Err(format!("{error:?}, expected {expected:?}")) }
        }
        (Err(error), _) => Err(format!("unexpected error: {error}")),
    }
}

fn check_file(name: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantics").join(name);
    let text = std::fs::read_to_string(&path).unwrap();
    let cases: Vec<Case> = serde_yaml_ng::from_str(&text).unwrap();
    assert!(!cases.is_empty());
    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| check(case).err().map(|reason| format!("{}: {reason}", case.name)))
        .collect();
    assert!(failures.is_empty(), "{name}: {} of {} cases fail:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

#[test]
fn values() {
    check_file("values.yml");
}
