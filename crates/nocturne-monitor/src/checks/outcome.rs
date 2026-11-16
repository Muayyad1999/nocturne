//! Turning what an engine said into a status, a stable code and a sentence.
//!
//! This module is where the feature's one hard rule lives: an error from a
//! probe is **either** a measurement of the service **or** a failure to make
//! the measurement, and the two are never mixed up.
//!
//! * A timeout, a refusal, an unreachable destination, a peer that answers
//!   rubbish — the probe went out and the service failed it. That is
//!   [`CheckStatus::Down`].
//! * A name that will not resolve, a machine that may not send ICMP, a
//!   credential that is not set, parameters that are not valid — nothing was
//!   asked of the service, so nothing is known about it. That is
//!   [`CheckStatus::CannotRun`], and it must never be reported as an outage.
//!
//! The codes are the same strings `nocturne-toolbox` maps a `ProbeError` to
//! for its RPC refusals where they mean the same thing, but this is
//! deliberately *not* that function: the toolbox answers "how should this
//! call be refused", and this answers "did the check run". Mapping
//! `Resolve` to `not_found` would be right there and wrong here.

use std::io;

use nocturne_netprobe::ProbeError;

use super::model::{CheckFacts, CheckStatus};

/// One run's verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub status: CheckStatus,
    /// A stable reason code the UI can switch on.
    pub code: String,
    /// The engine's own words, for the person reading them. Empty when the
    /// check is simply up.
    pub message: String,
    /// What the run took, where that was measured. A run that never left the
    /// process has no response time, and `0.0` would be a lie.
    pub rtt_ms: Option<f64>,
    pub facts: CheckFacts,
}

impl Outcome {
    pub fn up(code: &str, rtt_ms: Option<f64>) -> Outcome {
        Outcome {
            status: CheckStatus::Up,
            code: code.to_string(),
            message: String::new(),
            rtt_ms,
            facts: CheckFacts::default(),
        }
    }

    pub fn down(code: &str, message: impl Into<String>) -> Outcome {
        Outcome {
            status: CheckStatus::Down,
            code: code.to_string(),
            message: message.into(),
            rtt_ms: None,
            facts: CheckFacts::default(),
        }
    }

    /// The service answered, and something about the answer wants attention
    /// before it becomes an outage.
    pub fn warning(code: &str, message: impl Into<String>) -> Outcome {
        Outcome {
            status: CheckStatus::Warning,
            code: code.to_string(),
            message: message.into(),
            rtt_ms: None,
            facts: CheckFacts::default(),
        }
    }

    /// The check could not be attempted. Says nothing about the service.
    pub fn cannot_run(code: &str, message: impl Into<String>) -> Outcome {
        Outcome {
            status: CheckStatus::CannotRun,
            code: code.to_string(),
            message: message.into(),
            rtt_ms: None,
            facts: CheckFacts::default(),
        }
    }

    pub fn with_rtt(mut self, rtt_ms: Option<f64>) -> Outcome {
        self.rtt_ms = rtt_ms;
        self
    }

    pub fn with_facts(mut self, facts: CheckFacts) -> Outcome {
        self.facts = facts;
        self
    }

    /// Keep the measurements of a run that answered but failed its
    /// expectations: a 503 is still a response time and still a status.
    pub fn failing(mut self, code: &str, message: impl Into<String>) -> Outcome {
        self.status = CheckStatus::Down;
        self.code = code.to_string();
        self.message = message.into();
        self
    }

    pub fn warning_about(mut self, code: &str, message: impl Into<String>) -> Outcome {
        self.status = CheckStatus::Warning;
        self.code = code.to_string();
        self.message = message.into();
        self
    }
}

/// Classify an engine error: did the probe measure the service, or did it
/// never get the chance?
pub fn from_probe_error(error: &ProbeError) -> Outcome {
    match error {
        // The target has no address to aim at. Nothing was sent.
        ProbeError::Resolve => Outcome::cannot_run("dns_resolve", error.to_string()),
        // This machine is not allowed to send this, or cannot. The message
        // says what to do about it; the service is not implicated.
        ProbeError::Permission(_) => Outcome::cannot_run("permission", error.to_string()),
        ProbeError::Unsupported(_) => Outcome::cannot_run("unsupported", error.to_string()),
        // The check is misconfigured: a port of 0, a record type that does
        // not exist. A person has to fix the check, not the service.
        ProbeError::Invalid(_) => Outcome::cannot_run("invalid", error.to_string()),

        // These are measurements. The probe went out and this came back.
        ProbeError::Timeout => Outcome::down("timeout", error.to_string()),
        ProbeError::Refused => Outcome::down("refused", error.to_string()),
        ProbeError::Unreachable(_) => Outcome::down("unreachable", error.to_string()),
        // The peer answered, but not with what the protocol requires: a
        // broken service, which is exactly what a service check is for.
        ProbeError::Protocol(_) => Outcome::down("protocol", error.to_string()),

        // What is left in `Io` after the engine has classified the
        // network-meaning kinds is mostly local. A connection that was torn
        // down, though, is something the far end did, so it is a
        // measurement; everything else is this machine's problem and must
        // not be charged to the service.
        ProbeError::Io(io) => {
            if matches!(
                io.kind(),
                io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::UnexpectedEof
            ) {
                Outcome::down("io", error.to_string())
            } else {
                Outcome::cannot_run("local_error", error.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_will_not_resolve_is_a_check_that_could_not_run() {
        let outcome = from_probe_error(&ProbeError::Resolve);
        assert_eq!(outcome.status, CheckStatus::CannotRun);
        assert_eq!(outcome.code, "dns_resolve");
        assert!(outcome.message.contains("resolved"), "{}", outcome.message);
        assert_eq!(outcome.rtt_ms, None, "nothing was measured");
    }

    #[test]
    fn a_machine_that_may_not_send_icmp_has_not_found_an_outage() {
        // The mistake this guards against: reporting every device as down
        // because the container cannot open a socket.
        let outcome = from_probe_error(&ProbeError::Permission("no CAP_NET_RAW".into()));
        assert_eq!(outcome.status, CheckStatus::CannotRun);
        assert_eq!(outcome.code, "permission");
        assert!(outcome.message.contains("CAP_NET_RAW"));
    }

    #[test]
    fn a_timeout_or_a_refusal_is_a_measurement_of_the_service() {
        for (error, code) in [
            (ProbeError::Timeout, "timeout"),
            (ProbeError::Refused, "refused"),
            (ProbeError::Unreachable("no route".into()), "unreachable"),
            (ProbeError::Protocol("not an NTP reply".into()), "protocol"),
        ] {
            let outcome = from_probe_error(&error);
            assert_eq!(outcome.status, CheckStatus::Down, "{code}");
            assert_eq!(outcome.code, code);
            assert!(!outcome.message.is_empty());
        }
    }

    #[test]
    fn misconfiguration_is_not_an_outage_either() {
        let outcome = from_probe_error(&ProbeError::Invalid("0 is not a TCP port".into()));
        assert_eq!(outcome.status, CheckStatus::CannotRun);
        assert_eq!(outcome.code, "invalid");
        let outcome = from_probe_error(&ProbeError::Unsupported("no ping program".into()));
        assert_eq!(outcome.status, CheckStatus::CannotRun);
        assert_eq!(outcome.code, "unsupported");
    }

    #[test]
    fn a_local_socket_failure_is_told_apart_from_one_the_far_end_caused() {
        let out_of_files = ProbeError::Io(io::Error::other("too many open files"));
        assert_eq!(
            from_probe_error(&out_of_files).status,
            CheckStatus::CannotRun
        );
        assert_eq!(from_probe_error(&out_of_files).code, "local_error");

        let torn_down = ProbeError::Io(io::Error::from(io::ErrorKind::ConnectionReset));
        assert_eq!(from_probe_error(&torn_down).status, CheckStatus::Down);
        assert_eq!(from_probe_error(&torn_down).code, "io");
    }

    #[test]
    fn a_failing_answer_keeps_what_the_run_measured() {
        let answered = Outcome::up("ok", Some(12.5)).with_facts(CheckFacts {
            http_status: Some(503),
            ..CheckFacts::default()
        });
        let failed = answered.failing("http_status", "the server answered 503 Service Unavailable");
        assert_eq!(failed.status, CheckStatus::Down);
        assert_eq!(failed.rtt_ms, Some(12.5), "a 503 still took 12.5 ms");
        assert_eq!(failed.facts.http_status, Some(503));
    }
}
