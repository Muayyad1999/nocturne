//! The records a service check keeps: what it is, what it last found, and the
//! changes behind it.
//!
//! These shapes are a contract (`docs/dev/features/monitor.md`,
//! `ui/src/api/types/monitor.ts`), so field names and enum spellings here are
//! part of the RPC surface.
//!
//! The one distinction the whole module exists to preserve: **a check that
//! could not run is not a check that found a service down.** They are
//! different values of [`CheckStatus`], they are stored differently, they are
//! alerted on differently, and only one of them writes a sample.

use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------- kinds

/// What a check speaks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    #[default]
    Icmp,
    Tcp,
    Dns,
    Http,
    Https,
    Tls,
    Ntp,
    Snmp,
}

impl CheckKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckKind::Icmp => "icmp",
            CheckKind::Tcp => "tcp",
            CheckKind::Dns => "dns",
            CheckKind::Http => "http",
            CheckKind::Https => "https",
            CheckKind::Tls => "tls",
            CheckKind::Ntp => "ntp",
            CheckKind::Snmp => "snmp",
        }
    }

    pub fn parse(text: &str) -> Option<CheckKind> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "icmp" | "ping" => CheckKind::Icmp,
            "tcp" | "port" => CheckKind::Tcp,
            "dns" => CheckKind::Dns,
            "http" => CheckKind::Http,
            "https" => CheckKind::Https,
            "tls" | "ssl" | "certificate" => CheckKind::Tls,
            "ntp" | "sntp" => CheckKind::Ntp,
            "snmp" => CheckKind::Snmp,
            _ => return None,
        })
    }

    /// How long one run of this kind gets by default, in milliseconds.
    ///
    /// A handshake and a request take longer than a connect, and nothing is
    /// slower than a web server warming up a page.
    pub fn default_timeout_ms(self) -> u64 {
        match self {
            CheckKind::Icmp | CheckKind::Tcp | CheckKind::Dns | CheckKind::Ntp => 2_000,
            CheckKind::Snmp => 3_000,
            CheckKind::Tls => 5_000,
            CheckKind::Http | CheckKind::Https => 10_000,
        }
    }

    /// Whether a run of this kind can say anything about a certificate.
    pub fn sees_certificates(self) -> bool {
        matches!(self, CheckKind::Tls | CheckKind::Https)
    }
}

// --------------------------------------------------------------- the status

/// What a check is saying right now.
///
/// Five values, not three, because three cannot tell the truth:
///
/// * [`Unknown`](CheckStatus::Unknown) — it has never run. Nothing is known.
/// * [`Up`](CheckStatus::Up) — it ran and the service answered properly.
/// * [`Warning`](CheckStatus::Warning) — it ran, the service answered, and
///   something it measures is inside its warning margin (a certificate that
///   expires in nine days). Available now; not for much longer.
/// * [`Down`](CheckStatus::Down) — it ran and the service did not answer, or
///   answered wrongly. This is a measurement of the service.
/// * [`CannotRun`](CheckStatus::CannotRun) — the check could not be attempted
///   at all: no credential, a target with no address, a machine that may not
///   send ICMP, a simulated device with no services. This says nothing
///   whatsoever about the service, and calling it "down" would invent an
///   outage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    #[default]
    Unknown,
    Up,
    Warning,
    Down,
    CannotRun,
}

impl CheckStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Unknown => "unknown",
            CheckStatus::Up => "up",
            CheckStatus::Warning => "warning",
            CheckStatus::Down => "down",
            CheckStatus::CannotRun => "cannot_run",
        }
    }

    pub fn parse(text: &str) -> Option<CheckStatus> {
        Some(match text {
            "unknown" => CheckStatus::Unknown,
            "up" => CheckStatus::Up,
            "warning" => CheckStatus::Warning,
            "down" => CheckStatus::Down,
            "cannot_run" => CheckStatus::CannotRun,
            _ => return None,
        })
    }

    /// The service is available. A warning is available: the thing it warns
    /// about has not happened yet.
    pub fn is_up(self) -> bool {
        matches!(self, CheckStatus::Up | CheckStatus::Warning)
    }

    /// Whether this status is a verdict *about the service*, and so belongs
    /// in the `check.up` series and in an availability figure. `Unknown` and
    /// `CannotRun` are not: they are the absence of a measurement.
    pub fn measured(self) -> bool {
        matches!(
            self,
            CheckStatus::Up | CheckStatus::Warning | CheckStatus::Down
        )
    }
}

// ----------------------------------------------------------- what it checks

/// An HTTP or HTTPS check.
///
/// No request headers: one of them would eventually carry a token, and a
/// stored check is not a place to keep a secret.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpCheck {
    /// Path and query, `/` when empty.
    pub path: String,
    /// The port; 80 for `http` and 443 for `https` when unset.
    pub port: Option<u16>,
    /// `GET` when unset.
    pub method: Option<String>,
    /// Statuses that count as up. Empty means any 2xx or 3xx.
    pub expect_status: Vec<u16>,
    /// Unless this text appears in the body, the check is down.
    pub expect_text: Option<String>,
    /// How many redirects to follow; 0 makes a redirect the answer. Default 3.
    pub follow_redirects: Option<u8>,
    /// HTTPS only: a certificate that does not verify makes the check down.
    /// On by default — switch it off to watch a service that is knowingly
    /// behind a self-signed certificate.
    pub verify_tls: Option<bool>,
    /// HTTPS only: days of certificate life left at which the check goes to
    /// `warning`. The `monitoring` setting `cert_warn_days` when unset.
    pub warn_days: Option<i64>,
}

/// What a check does, and the parameters of that one kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckSpec {
    /// Echo requests.
    Icmp {
        /// Echoes per run, 1–16; 3 when unset.
        #[serde(default)]
        count: Option<u32>,
        /// Loss at or above this percentage is down. 100 by default: with
        /// three echoes, one lost packet is not an outage.
        #[serde(default)]
        loss_pct_down: Option<f64>,
    },
    /// A TCP connect, which is what "is anything listening?" means.
    Tcp {
        port: u16,
    },
    /// One DNS question, asked of the target itself rather than of the
    /// system resolvers: this watches *that* server.
    Dns {
        /// The name to ask about.
        name: String,
        /// `A` when empty; any type `probe.dns` takes.
        #[serde(default)]
        record: String,
        /// Unless an answer record contains this text, the check is down.
        #[serde(default)]
        expect: Option<String>,
        /// 53 when unset.
        #[serde(default)]
        port: Option<u16>,
    },
    Http(HttpCheck),
    Https(HttpCheck),
    /// A TLS handshake, for the certificate rather than for a service.
    Tls {
        /// 443 when unset.
        #[serde(default)]
        port: Option<u16>,
        /// The server name to send and check instead of the target's.
        #[serde(default)]
        sni: Option<String>,
        /// A chain that does not verify is down. On by default.
        #[serde(default)]
        require_trusted: Option<bool>,
        /// A certificate that is not valid for the name asked for is down.
        /// On by default.
        #[serde(default)]
        require_hostname: Option<bool>,
        /// Days of life left at which the check goes to `warning`. The
        /// `monitoring` setting `cert_warn_days` when unset.
        #[serde(default)]
        warn_days: Option<i64>,
    },
    /// One SNTP exchange: is the time server answering, and how far off is
    /// this machine's clock from its.
    Ntp {
        /// 123 when unset.
        #[serde(default)]
        port: Option<u16>,
        /// A clock further out than this is down. Unset: any answer is up.
        #[serde(default)]
        max_offset_ms: Option<f64>,
    },
    /// One SNMP GET over the device's own SNMP credential and port — the
    /// same session the poller uses, so this costs nothing extra.
    Snmp {
        /// The object to read; `sysUpTime` when unset.
        #[serde(default)]
        oid: Option<String>,
    },
}

impl Default for CheckSpec {
    fn default() -> Self {
        CheckSpec::Icmp {
            count: None,
            loss_pct_down: None,
        }
    }
}

impl CheckSpec {
    pub fn kind(&self) -> CheckKind {
        match self {
            CheckSpec::Icmp { .. } => CheckKind::Icmp,
            CheckSpec::Tcp { .. } => CheckKind::Tcp,
            CheckSpec::Dns { .. } => CheckKind::Dns,
            CheckSpec::Http(_) => CheckKind::Http,
            CheckSpec::Https(_) => CheckKind::Https,
            CheckSpec::Tls { .. } => CheckKind::Tls,
            CheckSpec::Ntp { .. } => CheckKind::Ntp,
            CheckSpec::Snmp { .. } => CheckKind::Snmp,
        }
    }

    /// The port this check will talk to, where it is a fixed one.
    pub fn port(&self) -> Option<u16> {
        match self {
            CheckSpec::Icmp { .. } | CheckSpec::Snmp { .. } => None,
            CheckSpec::Tcp { port } => Some(*port),
            CheckSpec::Dns { port, .. } => Some(port.unwrap_or(53)),
            CheckSpec::Http(http) => Some(http.port.unwrap_or(80)),
            CheckSpec::Https(http) => Some(http.port.unwrap_or(443)),
            CheckSpec::Tls { port, .. } => Some(port.unwrap_or(443)),
            CheckSpec::Ntp { port, .. } => Some(port.unwrap_or(123)),
        }
    }
}

// ---------------------------------------------------------------- the check

/// A named check attached to a device, with its own schedule.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceCheck {
    pub id: String,
    pub device_id: String,
    /// What a person calls it: `web`, `mail`, `resolver`. Unique per device.
    pub name: String,
    /// What it talks to and how.
    pub spec: CheckSpec,
    /// The host or address to check. Empty means the device's own address,
    /// which is the usual case; set it for a service that answers on a
    /// virtual address or under a name.
    pub host: String,
    pub enabled: bool,
    /// Seconds between runs.
    pub interval_s: u32,
    /// How long one run gets.
    pub timeout_ms: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl ServiceCheck {
    pub fn kind(&self) -> CheckKind {
        self.spec.kind()
    }
}

// ----------------------------------------------------------------- the facts

/// What a run measured, beyond up or down.
///
/// Every field is `None` until something actually measured it: an HTTP check
/// that timed out has no status, and saying `0` would be a lie.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckFacts {
    /// Echoes lost, as a percentage of those sent (ICMP only).
    pub loss_pct: Option<f64>,
    /// The HTTP status of the final response.
    pub http_status: Option<u16>,
    /// The URL that answered, after redirects.
    pub final_url: Option<String>,
    /// The DNS response code (`NOERROR`, `NXDOMAIN`, …).
    pub dns_rcode: Option<String>,
    /// Records in the answer section. `Some(0)` with `NOERROR` means the name
    /// exists and has no such record, which is not the same as no answer.
    pub dns_records: Option<u32>,
    /// How far this machine's clock is behind (positive) the server's.
    pub ntp_offset_ms: Option<f64>,
    pub ntp_stratum: Option<u8>,
    /// Days until the server's certificate expires; negative once expired.
    pub cert_expires_in_days: Option<i64>,
    pub cert_subject: Option<String>,
    pub cert_issuer: Option<String>,
    /// End of validity, `YYYY-MM-DDTHH:MM:SSZ`.
    pub cert_not_after: Option<String>,
    /// Whether the chain verified against this machine's trust store.
    pub cert_trusted: Option<bool>,
    /// The negotiated protocol (`TLSv1.3`).
    pub tls_protocol: Option<String>,
    /// The SNMP object that was read, in presentation form.
    pub snmp_value: Option<String>,
}

// ----------------------------------------------------------------- the state

/// What a check last found. One row per check, so a restart does not lose the
/// picture, and `since` still answers "down for how long".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckState {
    pub check_id: String,
    pub device_id: String,
    pub status: CheckStatus,
    /// A stable reason code: `ok`, `no_reply`, `timeout`, `refused`,
    /// `http_status`, `tls_expired`, `no_credential`, `sim`, … The UI
    /// switches on this; `message` is for the person.
    pub code: String,
    /// The engine's own sentence. Empty while all is well.
    pub message: String,
    /// What the run took, where it was measured.
    pub rtt_ms: Option<f64>,
    pub facts: CheckFacts,
    /// When the current status began. With `status`: "down since 09:14".
    pub since: Option<i64>,
    pub last_run_at: Option<i64>,
    /// The last run that found the service up (a warning counts as up).
    pub last_ok_at: Option<i64>,
    /// The last run that found it down.
    pub last_down_at: Option<i64>,
    /// Consecutive runs that came out with this same status.
    pub runs: u32,
}

/// A check and what it last found, as the RPC surface returns it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ServiceCheckView {
    #[serde(flatten)]
    pub check: ServiceCheck,
    /// The kind, beside the spec that carries it, so a list can group on it.
    pub kind: CheckKind,
    /// The device's name, for lists that span devices.
    pub device: String,
    pub state: CheckState,
}

// --------------------------------------------------------------- the history

/// One change of status, which is what a history is made of: the runs in
/// between said the same thing and are in the `check.up` series instead.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CheckEvent {
    pub id: i64,
    pub check_id: String,
    pub device_id: String,
    pub at: i64,
    pub status: CheckStatus,
    /// What it was before this change.
    pub previous: CheckStatus,
    /// How long `previous` had lasted. `None` when that is not known —
    /// the first change after the check was created, or after a restart.
    pub held_s: Option<i64>,
    pub code: String,
    pub message: String,
}

/// How a check spent a window of time.
///
/// The five figures are kept apart on purpose. Time that could not be
/// measured — before the first run, or while the check could not run — is
/// neither uptime nor downtime, and is excluded from `availability_pct`
/// rather than quietly counted as either.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CheckSummary {
    pub up_s: i64,
    pub warning_s: i64,
    pub down_s: i64,
    pub cannot_run_s: i64,
    pub unknown_s: i64,
    /// `(up + warning) / (up + warning + down)`, as a percentage. `None` when
    /// nothing in the window was measured: no figure is better than 100 %.
    pub availability_pct: Option<f64>,
    /// How much of the window the check had a verdict for.
    pub measured_s: i64,
    /// How many times it went down in the window.
    pub down_count: u32,
    pub last_down_at: Option<i64>,
    /// How long that outage lasted, or has lasted so far.
    pub last_down_for_s: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_check_that_could_not_run_is_not_a_check_that_is_down() {
        // The distinction the feature rests on, asserted on the type itself.
        assert!(!CheckStatus::CannotRun.measured());
        assert!(!CheckStatus::CannotRun.is_up());
        assert!(!CheckStatus::Unknown.measured());
        assert!(CheckStatus::Down.measured());
        assert_ne!(CheckStatus::CannotRun, CheckStatus::Down);
        assert_ne!(CheckStatus::CannotRun, CheckStatus::Unknown);
    }

    #[test]
    fn a_warning_is_still_available() {
        // "The certificate expires in nine days" is not an outage.
        assert!(CheckStatus::Warning.is_up());
        assert!(CheckStatus::Warning.measured());
    }

    #[test]
    fn every_status_round_trips_through_its_spelling() {
        for status in [
            CheckStatus::Unknown,
            CheckStatus::Up,
            CheckStatus::Warning,
            CheckStatus::Down,
            CheckStatus::CannotRun,
        ] {
            assert_eq!(CheckStatus::parse(status.as_str()), Some(status));
            assert_eq!(
                serde_json::to_value(status).unwrap(),
                json!(status.as_str())
            );
        }
        assert_eq!(CheckStatus::parse("flapping"), None);
    }

    #[test]
    fn specs_round_trip_through_their_tagged_form() {
        let https = CheckSpec::Https(HttpCheck {
            path: "/health".into(),
            expect_status: vec![200, 204],
            expect_text: Some("ok".into()),
            ..HttpCheck::default()
        });
        let encoded = serde_json::to_value(&https).unwrap();
        assert_eq!(encoded["kind"], "https");
        assert_eq!(encoded["path"], "/health");
        assert_eq!(encoded["expect_status"], json!([200, 204]));
        assert_eq!(serde_json::from_value::<CheckSpec>(encoded).unwrap(), https);

        let tcp: CheckSpec = serde_json::from_value(json!({"kind": "tcp", "port": 25})).unwrap();
        assert_eq!(tcp, CheckSpec::Tcp { port: 25 });
        assert_eq!(tcp.kind(), CheckKind::Tcp);
        assert_eq!(tcp.port(), Some(25));

        // Every optional parameter may be left out.
        let dns: CheckSpec =
            serde_json::from_value(json!({"kind": "dns", "name": "www.example.com"})).unwrap();
        assert_eq!(dns.port(), Some(53), "the default DNS port");
        let icmp: CheckSpec = serde_json::from_value(json!({"kind": "icmp"})).unwrap();
        assert_eq!(icmp.kind(), CheckKind::Icmp);
        assert_eq!(icmp.port(), None);
    }

    #[test]
    fn kinds_are_named_the_way_a_person_would_name_them() {
        assert_eq!(CheckKind::parse("PING"), Some(CheckKind::Icmp));
        assert_eq!(CheckKind::parse("ssl"), Some(CheckKind::Tls));
        assert_eq!(CheckKind::parse(" https "), Some(CheckKind::Https));
        assert_eq!(CheckKind::parse("gopher"), None);
        for kind in [
            CheckKind::Icmp,
            CheckKind::Tcp,
            CheckKind::Dns,
            CheckKind::Http,
            CheckKind::Https,
            CheckKind::Tls,
            CheckKind::Ntp,
            CheckKind::Snmp,
        ] {
            assert_eq!(CheckKind::parse(kind.as_str()), Some(kind));
            assert!(kind.default_timeout_ms() >= 1_000);
        }
        assert!(CheckKind::Https.sees_certificates() && CheckKind::Tls.sees_certificates());
        assert!(!CheckKind::Http.sees_certificates());
    }
}
