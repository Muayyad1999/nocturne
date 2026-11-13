//! Turning the engines' errors into the one error the RPC surface speaks.
//!
//! Both engines write their messages for the person who typed the input, so
//! the message is passed through unchanged; what this module decides is the
//! machine-stable `code` the UI switches on, and it keeps the engine's own
//! classification in `details` so nothing is lost on the way.

use nocturne_app::AppError;
use nocturne_netprobe::ProbeError;
use nocturne_tools::ToolError;
use serde_json::json;

/// A calculator refused its input.
///
/// Every variant is the caller's fault — bad input, or a request whose answer
/// would be unreasonably large — so they are all `invalid`, which is what the
/// specification asks for. The variant name travels in `details.reason` for a
/// UI that wants to say "too large" rather than "invalid".
pub fn tool(error: ToolError) -> AppError {
    let reason = match &error {
        ToolError::Invalid { .. } => "invalid",
        ToolError::TooLarge(_) => "too_large",
        ToolError::DoesNotFit(_) => "does_not_fit",
    };
    AppError::invalid(error.to_string()).with_details(json!({"reason": reason}))
}

/// A probe could not do what was asked.
///
/// `Permission` is `forbidden` and not `internal`, because it is a fact about
/// this machine that the person can act on (the message says how) — and it
/// must never be mistaken for "the target did not answer".
pub fn probe(error: ProbeError) -> AppError {
    let code = match &error {
        ProbeError::Resolve => "not_found",
        ProbeError::Timeout => "timeout",
        ProbeError::Unreachable(_) | ProbeError::Refused => "unavailable",
        ProbeError::Permission(_) => "forbidden",
        ProbeError::Unsupported(_) => "unsupported",
        ProbeError::Invalid(_) => "invalid",
        ProbeError::Io(_) | ProbeError::Protocol(_) => "unavailable",
    };
    AppError::new(code, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn a_calculators_refusal_keeps_the_engines_words() {
        let error = tool(nocturne_tools::subnet::info("10.0.300.1/24").unwrap_err());
        assert_eq!(error.code, "invalid");
        assert!(
            error.message.contains("300") && error.message.contains("octet"),
            "{}",
            error.message
        );
        assert_eq!(error.details.unwrap()["reason"], "invalid");

        let too_large = tool(ToolError::TooLarge("that is a lot of subnets".into()));
        assert_eq!(
            (too_large.code, too_large.message.as_str()),
            ("invalid", "that is a lot of subnets")
        );
        assert_eq!(too_large.details.unwrap()["reason"], "too_large");
    }

    #[test]
    fn a_probe_that_may_not_run_is_forbidden_not_an_answer() {
        // The distinction the whole crate rests on: "this machine may not send
        // that" is not "the target is down".
        let denied = probe(ProbeError::Permission(
            "raw sockets need CAP_NET_RAW".into(),
        ));
        assert_eq!(denied.code, "forbidden");
        assert!(denied.message.contains("CAP_NET_RAW"));

        assert_eq!(probe(ProbeError::Timeout).code, "timeout");
        assert_eq!(probe(ProbeError::Resolve).code, "not_found");
        assert_eq!(probe(ProbeError::Refused).code, "unavailable");
        assert_eq!(
            probe(ProbeError::Unsupported("no ping program".into())).code,
            "unsupported"
        );
        assert_eq!(
            probe(ProbeError::Io(io::Error::other("socket gone"))).code,
            "unavailable"
        );
    }
}
