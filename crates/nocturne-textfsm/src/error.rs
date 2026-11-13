//! Error types.

use crate::ntc::MergeError;

/// A template that could not be compiled.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("template line {line}: {message}")]
pub struct TemplateError {
    line: usize,
    message: String,
}

impl TemplateError {
    pub(crate) fn new(line: usize, message: impl Into<String>) -> TemplateError {
        TemplateError {
            line,
            message: message.into(),
        }
    }

    /// The 1-based line of the template the error refers to. Errors about the
    /// template as a whole (a missing `Start` state) point at its last line.
    pub fn line(&self) -> usize {
        self.line
    }

    /// What is wrong, without the line number.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Running a template over some input failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RunError {
    /// A rule with the `Error` action matched: the template declares the input
    /// as something it cannot parse.
    #[error("{}", error_action(message.as_deref(), *rule_line, input_line))]
    ErrorAction {
        /// The message given in the template (`-> Error "message"`), if any.
        message: Option<String>,
        /// The 1-based template line of the rule.
        rule_line: usize,
        /// The input line the rule matched.
        input_line: String,
    },
    /// The regex engine gave up on a rule: a rule with look-around or
    /// back-references exceeded the step limit of the backtracking engine, or
    /// the Unicode-aware form of a rule, which is only built when a line
    /// outside ASCII first needs it, turned out too large to compile.
    #[error("template line {rule_line}: {message}")]
    Regex {
        /// The 1-based template line of the rule.
        rule_line: usize,
        /// The reason reported by the regex engine.
        message: String,
    },
}

/// Follows the wording of the reference implementation.
fn error_action(message: Option<&str>, rule_line: usize, input_line: &str) -> String {
    match message {
        Some(message) => {
            format!("Error: {message}. Rule Line: {rule_line}. Input Line: {input_line}.")
        }
        None => format!("State Error raised. Rule Line: {rule_line}. Input Line: {input_line}"),
    }
}

/// Any error of this crate.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The template does not compile.
    #[error(transparent)]
    Template(#[from] TemplateError),
    /// The template rejected the input.
    #[error(transparent)]
    Run(#[from] RunError),
    /// The tables of the templates listed for one command cannot be combined.
    #[error(transparent)]
    Merge(#[from] MergeError),
}
