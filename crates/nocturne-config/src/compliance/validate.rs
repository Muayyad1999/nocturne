//! Telling a policy author what is wrong before a run does.
//!
//! A rule that cannot be evaluated is worse than a missing one: it looks like
//! a check and answers nothing. `compliance.policies.save` refuses a policy
//! with an error here, and the rule editor shows the warnings as it is typed.

use serde::Serialize;

use super::model::{Check, Policy};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// The policy cannot be saved like this.
    Error,
    /// It will run, but probably not as meant.
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// The rule it is about; empty for a problem with the policy itself.
    pub rule_id: String,
    /// Which part of the rule, e.g. `check.pattern`.
    pub field: String,
    pub level: Level,
    pub message: String,
}

fn problem(rule_id: &str, field: &str, level: Level, message: impl Into<String>) -> Problem {
    Problem {
        rule_id: rule_id.to_string(),
        field: field.to_string(),
        level,
        message: message.into(),
    }
}

fn regex(out: &mut Vec<Problem>, rule_id: &str, field: &str, pattern: &str) {
    if pattern.trim().is_empty() {
        out.push(problem(
            rule_id,
            field,
            Level::Error,
            "the pattern is empty, so it matches every line",
        ));
        return;
    }
    if let Err(err) = regex::Regex::new(pattern) {
        out.push(problem(
            rule_id,
            field,
            Level::Error,
            format!("not a valid regular expression: {err}"),
        ));
    }
}

/// An entry of `must` / `must_not`: a literal line, or a pattern. A literal
/// that is not a valid pattern still works, but say so.
fn entry(out: &mut Vec<Problem>, rule_id: &str, field: &str, text: &str) {
    if text.trim().is_empty() {
        out.push(problem(
            rule_id,
            field,
            Level::Error,
            "an empty line matches nothing; remove it",
        ));
    } else if let Err(err) = regex::Regex::new(text) {
        out.push(problem(
            rule_id,
            field,
            Level::Warning,
            format!("used as a literal line: it is not a valid regular expression ({err})"),
        ));
    }
}

fn line(out: &mut Vec<Problem>, rule_id: &str, field: &str, text: &str) {
    if text.trim().is_empty() {
        out.push(problem(
            rule_id,
            field,
            Level::Error,
            "the line is empty: say which configuration line the rule is about",
        ));
    }
}

/// Everything wrong with a policy, worst first.
pub fn problems(policy: &Policy) -> Vec<Problem> {
    let mut out: Vec<Problem> = Vec::new();
    if policy.name.trim().is_empty() {
        out.push(problem("", "name", Level::Error, "the policy has no name"));
    }
    if policy.rules.is_empty() {
        out.push(problem(
            "",
            "rules",
            Level::Error,
            "a policy without rules judges nothing",
        ));
    }
    let mut seen: Vec<&str> = Vec::new();
    for rule in &policy.rules {
        let id = rule.id.as_str();
        if id.trim().is_empty() {
            out.push(problem(
                "",
                "rules[].id",
                Level::Error,
                "every rule needs an id: findings and exceptions are filed under it",
            ));
        } else if seen.contains(&id) {
            out.push(problem(
                id,
                "id",
                Level::Error,
                "two rules of this policy share an id",
            ));
        }
        seen.push(id);
        if rule.title.trim().is_empty() {
            out.push(problem(
                id,
                "title",
                Level::Error,
                "the title is what a person reads in the result; it cannot be empty",
            ));
        }
        if rule.rationale.trim().is_empty() {
            out.push(problem(
                id,
                "rationale",
                Level::Warning,
                "without a rationale nobody can tell whether the rule is right about the platform",
            ));
        }
        match &rule.check {
            Check::Present { line: text } => line(&mut out, id, "check.line", text),
            Check::Absent { line: text } => line(&mut out, id, "check.line", text),
            Check::Matches { pattern } => regex(&mut out, id, "check.pattern", pattern),
            Check::NotMatches { pattern } => regex(&mut out, id, "check.pattern", pattern),
            Check::Section {
                parent,
                must,
                must_not,
                ..
            } => {
                regex(&mut out, id, "check.parent", parent);
                if must.is_empty() && must_not.is_empty() {
                    out.push(problem(
                        id,
                        "check.must",
                        Level::Error,
                        "the rule requires nothing of the sections it selects, so it always passes",
                    ));
                }
                for text in must {
                    entry(&mut out, id, "check.must", text);
                }
                for text in must_not {
                    entry(&mut out, id, "check.must_not", text);
                }
            }
            Check::SectionWhen {
                parent,
                when,
                must,
                must_not,
            } => {
                regex(&mut out, id, "check.parent", parent);
                regex(&mut out, id, "check.when", when);
                if must.is_empty() && must_not.is_empty() {
                    out.push(problem(
                        id,
                        "check.must",
                        Level::Error,
                        "the rule requires nothing of the sections it selects, so it always passes",
                    ));
                }
                for text in must {
                    entry(&mut out, id, "check.must", text);
                }
                for text in must_not {
                    entry(&mut out, id, "check.must_not", text);
                }
            }
            Check::Snippet { config } => {
                if config.trim().is_empty() {
                    out.push(problem(
                        id,
                        "check.config",
                        Level::Error,
                        "the snippet holds no configuration lines",
                    ));
                }
            }
        }
        if let Some(lines) = &rule.remediation {
            if lines.iter().all(|line| line.trim().is_empty()) {
                out.push(problem(
                    id,
                    "remediation",
                    Level::Error,
                    "the remediation has no commands; leave it out instead",
                ));
            }
            for line in lines {
                if let Err(err) = super::remediate::check_template(line) {
                    out.push(problem(id, "remediation", Level::Error, err));
                }
            }
        }
    }
    out.sort_by_key(|problem| problem.level != Level::Error);
    out
}

/// Whether a policy may be saved and run.
pub fn ok(problems: &[Problem]) -> bool {
    !problems.iter().any(|problem| problem.level == Level::Error)
}
