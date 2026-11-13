//! The crate's error type and the SNMP error-status names.

/// Everything that can go wrong while talking SNMP.
#[derive(thiserror::Error, Debug)]
pub enum SnmpError {
    /// No acceptable response arrived within `timeout` for any of the `1 + retries` attempts.
    #[error("request timed out")]
    Timeout,
    /// The UDP socket failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A datagram (or a textual OID) could not be decoded, or the peer violated the protocol.
    #[error("malformed SNMP data: {0}")]
    Decode(String),
    /// error-status from the agent (tooBig, noSuchName, genErr, authorizationError, …) with error-index.
    ///
    /// `index` is 1-based and refers to the variable binding the agent blames; 0 means "none".
    #[error("agent returned {name} (error-status {status}, error-index {index})")]
    Agent {
        status: u32,
        name: &'static str,
        index: u32,
    },
    /// SNMPv3 Report PDUs that are terminal: unknown user, wrong digest, unsupported sec level,
    /// decryption error…
    #[error("SNMPv3 security error: {0}")]
    Auth(String),
    /// The request cannot be expressed in the selected protocol version or configuration.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl SnmpError {
    /// Builds the [`SnmpError::Agent`] variant for a non-zero error-status.
    pub(crate) fn agent(status: u32, index: u32) -> Self {
        SnmpError::Agent {
            status,
            name: error_status_name(status),
            index,
        }
    }

    pub(crate) fn decode(message: impl Into<String>) -> Self {
        SnmpError::Decode(message.into())
    }

    /// `true` for the error-status `tooBig`.
    pub(crate) fn is_too_big(&self) -> bool {
        matches!(self, SnmpError::Agent { status, .. } if *status == status::TOO_BIG)
    }
}

/// error-status values used by the crate itself.
pub(crate) mod status {
    pub const NO_ERROR: u32 = 0;
    pub const TOO_BIG: u32 = 1;
    pub const NO_SUCH_NAME: u32 = 2;
    pub const NOT_WRITABLE: u32 = 17;
}

/// The RFC 3416 name of an error-status value (`"tooBig"`, `"noSuchName"`, …).
pub fn error_status_name(status: u32) -> &'static str {
    match status {
        0 => "noError",
        1 => "tooBig",
        2 => "noSuchName",
        3 => "badValue",
        4 => "readOnly",
        5 => "genErr",
        6 => "noAccess",
        7 => "wrongType",
        8 => "wrongLength",
        9 => "wrongEncoding",
        10 => "wrongValue",
        11 => "noCreation",
        12 => "inconsistentValue",
        13 => "resourceUnavailable",
        14 => "commitFailed",
        15 => "undoFailed",
        16 => "authorizationError",
        17 => "notWritable",
        18 => "inconsistentName",
        _ => "unknownError",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names_cover_rfc_3416() {
        assert_eq!(error_status_name(1), "tooBig");
        assert_eq!(error_status_name(2), "noSuchName");
        assert_eq!(error_status_name(5), "genErr");
        assert_eq!(error_status_name(16), "authorizationError");
        assert_eq!(error_status_name(18), "inconsistentName");
        assert_eq!(error_status_name(19), "unknownError");
    }

    #[test]
    fn agent_error_display_names_the_status() {
        let err = SnmpError::agent(2, 3);
        assert!(!err.is_too_big());
        assert!(SnmpError::agent(1, 0).is_too_big());
        assert_eq!(
            err.to_string(),
            "agent returned noSuchName (error-status 2, error-index 3)"
        );
    }
}
