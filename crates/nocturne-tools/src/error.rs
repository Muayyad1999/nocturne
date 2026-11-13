//! The one error type every tool returns.

use serde::{Deserialize, Serialize};

/// How much of the user's input an error message quotes back.
const ECHO_CHARS: usize = 80;

/// Why a tool could not produce a result.
///
/// The `Display` text is written for the person who typed the input, e.g.
/// ``invalid IPv4 address `10.0.300.1`: `300` is not a valid octet (0-255)``.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolError {
    /// The input is not what the tool expects.
    #[error("invalid {what}{}: {reason}", quoted(.input))]
    Invalid {
        /// What the input was supposed to be ("IPv4 address", "MAC address", ...).
        what: String,
        /// The offending input, shortened if it is very long.
        input: String,
        /// What is wrong with it, and where possible how to fix it.
        reason: String,
    },
    /// The request is well-formed but the answer would be unreasonably big.
    #[error("{0}")]
    TooLarge(String),
    /// What was asked for does not fit into the space that was given.
    #[error("{0}")]
    DoesNotFit(String),
}

impl ToolError {
    /// An [`ToolError::Invalid`] that quotes at most [`ECHO_CHARS`] characters of `input`.
    pub(crate) fn invalid(what: &str, input: &str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            what: what.to_owned(),
            input: snip(input, ECHO_CHARS),
            reason: reason.into(),
        }
    }
}

fn quoted(input: &str) -> String {
    if input.is_empty() {
        String::new()
    } else {
        format!(" `{input}`")
    }
}

/// `text` cut to `max` characters, with control characters made visible, so
/// that junk input cannot blow up or garble an error message.
pub(crate) fn snip(text: &str, max: usize) -> String {
    let mut out: String = text
        .chars()
        .take(max)
        .map(|c| if c.is_control() { '\u{fffd}' } else { c })
        .collect();
    if text.chars().nth(max).is_some() {
        out.push_str("...");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_quote_the_input_and_say_what_is_wrong() {
        let error = ToolError::invalid(
            "IPv4 address",
            "10.0.300.1",
            "`300` is not a valid octet (0-255)",
        );
        assert_eq!(
            error.to_string(),
            "invalid IPv4 address `10.0.300.1`: `300` is not a valid octet (0-255)"
        );
        let empty = ToolError::invalid("MAC address", "", "nothing was entered");
        assert_eq!(
            empty.to_string(),
            "invalid MAC address: nothing was entered"
        );
        assert_eq!(
            ToolError::DoesNotFit("no room".into()).to_string(),
            "no room"
        );
    }

    #[test]
    fn long_or_binary_input_is_not_echoed_verbatim() {
        let error = ToolError::invalid("number", &"9".repeat(10_000), "too long");
        let ToolError::Invalid { input, .. } = &error else {
            panic!("wrong variant")
        };
        assert_eq!(input.chars().count(), ECHO_CHARS + 3);
        assert!(input.ends_with("..."));
        assert_eq!(snip("a\u{1b}[31mb\n", 20), "a\u{fffd}[31mb\u{fffd}");
        assert_eq!(snip("héllo", 2), "hé...");
    }

    #[test]
    fn serialises_as_snake_case_json() {
        let json = serde_json::to_string(&ToolError::TooLarge("big".into())).unwrap();
        assert_eq!(json, r#"{"too_large":"big"}"#);
        let back: ToolError = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ToolError::TooLarge("big".into()));
    }
}
