//! Running one check: aim it, ask the engine, and judge the answer.
//!
//! Nothing here opens a socket itself. Every kind drives an engine that is
//! already tested against real servers — `nocturne-netprobe` for ICMP, TCP,
//! DNS, HTTP, TLS and NTP, and this crate's own SNMP session for SNMP — so
//! what this module contributes is the aiming and the judging.
//!
//! Judging is where a service check earns its keep, and it is all of the form
//! "the service answered, but not with what was asked for": a 503 is a
//! response, an `NXDOMAIN` from the resolver that is supposed to serve the
//! zone is a response, a certificate that expired yesterday is a response.
//! Those are [`CheckStatus::Down`] with the measurement kept. A probe that
//! never went out is [`CheckStatus::CannotRun`] and keeps nothing, because
//! there is nothing to keep.

use std::time::{Duration, Instant};

use nocturne_app::model::Device;
use nocturne_app::{App, inventory};
use nocturne_netprobe::http::HttpOptions;
use nocturne_netprobe::{dns, http, ntp, tcp, tls};
use nocturne_snmp::{Oid, oids};

use super::model::{CheckFacts, CheckSpec, HttpCheck, ServiceCheck};
use super::outcome::{Outcome, from_probe_error};
use crate::settings::MonitorSettings;
use crate::state::MonitorState;

/// Echoes an ICMP check sends when it does not say.
const DEFAULT_ECHOES: u32 = 3;
/// Redirects an HTTP check follows when it does not say.
const DEFAULT_REDIRECTS: u8 = 3;
/// How much of a body the HTTP engine keeps as text, and therefore how far
/// `expect_text` can see.
const PREVIEW_BYTES: u64 = 2048;

/// Where a check points, once the device has had its say.
#[derive(Debug)]
pub struct Aim {
    /// The host or address the engine is given.
    pub host: String,
    /// Whether the host came from the check rather than from the device.
    pub explicit: bool,
}

/// Work out what to aim at, or why the check cannot be aimed at all.
///
/// The failure is boxed because it is an [`Outcome`] — a whole verdict, not
/// an error code: "this check could not run, and here is the sentence to show
/// for it" is the answer, and it goes on to be recorded like any other.
pub fn aim(check: &ServiceCheck, device: &Device) -> Result<Aim, Box<Outcome>> {
    let explicit = !check.host.trim().is_empty();
    let host = if explicit {
        check.host.trim().to_string()
    } else {
        device.address.trim().to_string()
    };
    // A simulated device's `address` field is either empty or belongs to
    // somebody else on the real network, exactly as the device poller warns.
    // The simulator serves a console, not services, so there is nothing here
    // to check — and a packet must not be sent at a stranger on its behalf.
    if device.lab.is_some() && !explicit {
        return Err(Box::new(Outcome::cannot_run(
            "sim",
            format!(
                "`{}` is a simulated device: it has a console but no listening services, so this check has nothing to send to. Give the check its own host if the service is real.",
                device.name
            ),
        )));
    }
    if host.is_empty() {
        return Err(Box::new(Outcome::cannot_run(
            "no_address",
            format!(
                "`{}` has no address, and this check does not name a host of its own, so there is nothing to send to",
                device.name
            ),
        )));
    }
    Ok(Aim { host, explicit })
}

/// Run one check once and say what it found. Writes nothing.
pub async fn run_once(app: &App, check: &ServiceCheck) -> Outcome {
    let device = match inventory::get(app, &check.device_id) {
        Ok(device) => device,
        Err(error) => {
            return Outcome::cannot_run("no_device", error.message);
        }
    };
    let aimed = match aim(check, &device) {
        Ok(aimed) => aimed,
        Err(outcome) => return *outcome,
    };
    let state = app.state::<MonitorState>();
    let settings = MonitorSettings::load(app);
    let timeout = Duration::from_millis(check.timeout_ms.clamp(100, 120_000));
    let now_unix = state.now() / 1_000;

    match &check.spec {
        CheckSpec::Icmp {
            count,
            loss_pct_down,
        } => {
            let echoes = count.unwrap_or(DEFAULT_ECHOES).clamp(1, 16);
            let limit = loss_pct_down.unwrap_or(100.0);
            match state.probe.ping(&aimed.host, echoes, timeout).await {
                Ok(report) => {
                    let loss = report.loss_pct();
                    let facts = CheckFacts {
                        loss_pct: Some(loss),
                        ..CheckFacts::default()
                    };
                    let measured = Outcome::up("ok", report.rtt_ms).with_facts(facts);
                    if !report.answered() {
                        measured.failing(
                            "no_reply",
                            format!("none of the {echoes} echo requests came back"),
                        )
                    } else if loss >= limit {
                        measured.failing(
                            "loss",
                            format!("{loss:.0} % of the echo requests were lost, at or above the {limit:.0} % this check calls down"),
                        )
                    } else {
                        measured
                    }
                }
                // The `Reachability` contract: an error means the probe could
                // not be *attempted*. A host that did not answer comes back
                // as `Ok` with nothing received, and is handled above.
                Err(error) => Outcome::cannot_run("probe_unavailable", error),
            }
        }

        CheckSpec::Tcp { port } => match tcp::check(&aimed.host, *port, timeout).await {
            Ok(result) => match result.state {
                tcp::PortState::Open => Outcome::up("ok", result.rtt_ms),
                tcp::PortState::Closed => Outcome::down(
                    "refused",
                    format!("the connection to port {port} was refused: nothing is listening"),
                ),
                tcp::PortState::Filtered => Outcome::down(
                    "timeout",
                    format!("port {port} did not answer within {} ms", check.timeout_ms),
                ),
            },
            Err(error) => from_probe_error(&error),
        },

        CheckSpec::Dns {
            name,
            record,
            expect,
            port,
        } => {
            let kind = if record.trim().is_empty() {
                "A"
            } else {
                record.trim()
            };
            let server = server_address(&aimed.host, port.unwrap_or(53));
            match dns::lookup(name, kind, Some(&server), timeout).await {
                Ok(answer) => {
                    let facts = CheckFacts {
                        dns_rcode: Some(answer.rcode.clone()),
                        dns_records: Some(answer.records.len() as u32),
                        ..CheckFacts::default()
                    };
                    let measured =
                        Outcome::up("ok", Some(answer.elapsed_ms)).with_facts(facts.clone());
                    // A negative response code is an answer, and from the
                    // server that is supposed to serve the name it is the
                    // wrong one.
                    if answer.rcode != "NOERROR" {
                        return measured.failing(
                            "dns_rcode",
                            format!("the server answered {} for {name} {kind}", answer.rcode),
                        );
                    }
                    if answer.records.is_empty() {
                        return measured.failing(
                            "dns_empty",
                            format!("the server has no {kind} record for {name}"),
                        );
                    }
                    match expect.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                        Some(wanted)
                            if !answer
                                .records
                                .iter()
                                .any(|record| record.data.contains(wanted)) =>
                        {
                            measured.failing(
                                "dns_expect",
                                format!(
                                    "none of the {} record(s) for {name} contain `{wanted}`",
                                    answer.records.len()
                                ),
                            )
                        }
                        _ => measured,
                    }
                }
                Err(error) => from_probe_error(&error),
            }
        }

        CheckSpec::Http(options) => {
            web(check, &aimed.host, options, false, &settings, now_unix).await
        }
        CheckSpec::Https(options) => {
            web(check, &aimed.host, options, true, &settings, now_unix).await
        }

        CheckSpec::Tls {
            port,
            sni,
            require_trusted,
            require_hostname,
            warn_days,
        } => {
            let port = port.unwrap_or(443);
            let sni = sni.as_deref().map(str::trim).filter(|s| !s.is_empty());
            match tls::inspect(&aimed.host, port, sni, timeout, now_unix).await {
                Ok(report) => {
                    let facts = certificate_facts(&report);
                    let measured =
                        Outcome::up("ok", Some(report.handshake_ms)).with_facts(facts.clone());
                    if require_hostname.unwrap_or(true) && !report.hostname_matches {
                        return measured.failing(
                            "tls_name_mismatch",
                            format!(
                                "the certificate is not valid for `{}`",
                                sni.unwrap_or(&aimed.host)
                            ),
                        );
                    }
                    judge_certificate(
                        measured,
                        &report.trust_error,
                        require_trusted.unwrap_or(true),
                        report.trusted,
                        report.chain.first().map(|_| report.expires_in_days),
                        warn_days.unwrap_or(i64::from(settings.cert_warn_days)),
                    )
                }
                Err(error) => from_probe_error(&error),
            }
        }

        CheckSpec::Ntp {
            port,
            max_offset_ms,
        } => {
            let server = server_address(&aimed.host, port.unwrap_or(123));
            // The engine never reads the wall clock, so the offset is
            // measured against this installation's own clock — which is the
            // question "is our time right?" actually asks.
            let local_unix = state.now() as f64 / 1_000.0;
            match ntp::query(&server, timeout, local_unix).await {
                Ok(report) => {
                    let facts = CheckFacts {
                        ntp_offset_ms: Some(report.offset_ms),
                        ntp_stratum: Some(report.stratum),
                        ..CheckFacts::default()
                    };
                    let measured = Outcome::up("ok", Some(report.delay_ms)).with_facts(facts);
                    // Stratum 16 is the server saying its own clock is not
                    // set. Taking the time from it would be worse than not
                    // asking.
                    if report.leap == "unsynchronized" || report.stratum >= 16 {
                        return measured.failing(
                            "ntp_unsynchronised",
                            format!(
                                "the server answered at stratum {}, which means its own clock is not set",
                                report.stratum
                            ),
                        );
                    }
                    match max_offset_ms {
                        Some(limit) if report.offset_ms.abs() > *limit => measured.failing(
                            "ntp_offset",
                            format!(
                                "this machine's clock is {:+.0} ms off the server's, beyond the {limit:.0} ms this check allows",
                                report.offset_ms
                            ),
                        ),
                        _ => measured,
                    }
                }
                Err(error) => from_probe_error(&error),
            }
        }

        CheckSpec::Snmp { oid } => snmp(app, &device, oid.as_deref(), timeout).await,
    }
}

/// `host:port`, with an IPv6 literal bracketed.
fn server_address(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

async fn web(
    check: &ServiceCheck,
    host: &str,
    wanted: &HttpCheck,
    secure: bool,
    settings: &MonitorSettings,
    now_unix: i64,
) -> Outcome {
    let scheme = if secure { "https" } else { "http" };
    let port = wanted.port.unwrap_or(if secure { 443 } else { 80 });
    let path = wanted.path.trim();
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let url = format!("{scheme}://{}{path}", server_address(host, port));
    let options = HttpOptions {
        method: wanted
            .method
            .as_deref()
            .map(str::trim)
            .filter(|method| !method.is_empty())
            .unwrap_or("GET")
            .to_uppercase(),
        headers: Vec::new(),
        follow_redirects: wanted.follow_redirects.unwrap_or(DEFAULT_REDIRECTS).min(10),
        timeout_ms: check.timeout_ms.clamp(100, 120_000),
        // Only meaningful over TLS; the engine ignores it for plain HTTP.
        verify_tls: secure && wanted.verify_tls.unwrap_or(true),
        max_body_bytes: PREVIEW_BYTES,
    };
    let report = match http::check(&url, &options, now_unix).await {
        Ok(report) => report,
        Err(error) => return from_probe_error(&error),
    };

    let mut facts = CheckFacts {
        http_status: Some(report.status),
        final_url: Some(report.final_url.clone()),
        ..CheckFacts::default()
    };
    if let Some(session) = &report.tls {
        let certificate = certificate_facts(session);
        facts.cert_expires_in_days = certificate.cert_expires_in_days;
        facts.cert_subject = certificate.cert_subject;
        facts.cert_issuer = certificate.cert_issuer;
        facts.cert_not_after = certificate.cert_not_after;
        facts.cert_trusted = certificate.cert_trusted;
        facts.tls_protocol = certificate.tls_protocol;
    }
    let measured = Outcome::up("ok", Some(report.total_ms)).with_facts(facts);

    let accepted = if wanted.expect_status.is_empty() {
        (200..400).contains(&report.status)
    } else {
        wanted.expect_status.contains(&report.status)
    };
    if !accepted {
        let expected = if wanted.expect_status.is_empty() {
            "a 2xx or 3xx".to_string()
        } else {
            wanted
                .expect_status
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(" or ")
        };
        return measured.failing(
            "http_status",
            format!(
                "the server answered {} {} where {expected} was expected",
                report.status,
                report.reason.trim()
            ),
        );
    }
    if let Some(wanted_text) = wanted
        .expect_text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        && !report.body_preview.contains(wanted_text)
    {
        return measured.failing(
            "http_expect_text",
            if report.body_preview.is_empty() {
                format!(
                    "`{wanted_text}` was expected in the body, which is {} byte(s) of something that is not text",
                    report.body_bytes
                )
            } else {
                format!(
                    "`{wanted_text}` does not appear in the first {} byte(s) of the body, which is as far as a check reads",
                    report.body_preview.len()
                )
            },
        );
    }
    // The certificate is judged after the request, so "it serves pages but
    // its certificate expires on Friday" comes out as a warning rather than
    // hiding behind a 200.
    match &report.tls {
        Some(session) => judge_certificate(
            measured,
            &session.trust_error,
            // Trust was already enforced by the handshake when `verify_tls`
            // is on; repeating it here would double-report.
            false,
            session.trusted,
            session.chain.first().map(|_| session.expires_in_days),
            wanted
                .warn_days
                .unwrap_or(i64::from(settings.cert_warn_days)),
        ),
        None => measured,
    }
}

fn certificate_facts(report: &tls::TlsReport) -> CheckFacts {
    let leaf = report.chain.first();
    CheckFacts {
        cert_expires_in_days: leaf.map(|_| report.expires_in_days),
        cert_subject: leaf.map(|cert| {
            if cert.common_name.is_empty() {
                cert.subject.clone()
            } else {
                cert.common_name.clone()
            }
        }),
        cert_issuer: leaf.map(|cert| cert.issuer.clone()),
        cert_not_after: leaf.map(|cert| cert.not_after.clone()),
        cert_trusted: Some(report.trusted),
        tls_protocol: Some(report.protocol.clone()),
        ..CheckFacts::default()
    }
}

/// Expiry and trust, in the order that reports the most useful thing.
///
/// An expired certificate also makes the chain untrusted, so expiry is judged
/// first: `tls_expired` with the number of days says more than "the chain
/// could not be verified".
fn judge_certificate(
    measured: Outcome,
    trust_error: &Option<String>,
    require_trusted: bool,
    trusted: bool,
    expires_in_days: Option<i64>,
    warn_days: i64,
) -> Outcome {
    let days = match expires_in_days {
        Some(days) => days,
        // No certificate was presented; nothing to say about expiry.
        None => return measured,
    };
    if days < 0 {
        return measured.failing(
            "tls_expired",
            format!("the certificate expired {} day(s) ago", -days),
        );
    }
    if require_trusted && !trusted {
        return measured.failing(
            "tls_untrusted",
            trust_error
                .clone()
                .unwrap_or_else(|| "the certificate chain could not be verified".to_string()),
        );
    }
    if warn_days > 0 && days <= warn_days {
        return measured.warning_about(
            "tls_expiring",
            format!(
                "the certificate expires in {days} day(s), inside this check's {warn_days}-day warning"
            ),
        );
    }
    measured
}

async fn snmp(app: &App, device: &Device, oid: Option<&str>, timeout: Duration) -> Outcome {
    let state = app.state::<MonitorState>();
    let wanted: Oid = match oid.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => match text.parse() {
            Ok(oid) => oid,
            Err(error) => {
                return Outcome::cannot_run("invalid", format!("`{text}` is not an OID: {error}"));
            }
        },
        None => oids::SYS_UP_TIME.parse().expect("a well-formed OID"),
    };
    // A missing or unusable credential is the textbook check that cannot
    // run: no request is made, so the agent is not implicated.
    let target = match crate::snmp::target(app, device) {
        Ok(target) => target,
        Err(error) => return Outcome::cannot_run("no_credential", error.message),
    };
    let started = Instant::now();
    let mut lease = match crate::snmp::acquire(
        &state.sessions,
        &state.snmp,
        &device.id,
        &target,
        timeout,
    )
    .await
    {
        Ok(lease) => lease,
        Err(error) => {
            return Outcome::down("snmp_unreachable", error);
        }
    };
    match lease.session().get(std::slice::from_ref(&wanted)).await {
        Ok(binds) => {
            let rtt_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
            match binds.iter().find(|bind| bind.oid == wanted) {
                Some(bind)
                    if matches!(
                        bind.value,
                        nocturne_snmp::Value::NoSuchObject | nocturne_snmp::Value::NoSuchInstance
                    ) =>
                {
                    // The agent answered, so the service is not down — but it
                    // does not have what the check asks for, so the check has
                    // no verdict to give. That is the check's fault, and
                    // reporting it as an outage would be a lie about the
                    // device.
                    Outcome::cannot_run(
                        "snmp_no_such_object",
                        format!("the agent answered, but it does not have {wanted}"),
                    )
                }
                Some(bind) => Outcome::up("ok", rtt_ms).with_facts(CheckFacts {
                    snmp_value: Some(bind.value.display()),
                    ..CheckFacts::default()
                }),
                // Same thing said more quietly: a reply with the varbind
                // left out. The agent is there; the object is not.
                None => Outcome::cannot_run(
                    "snmp_no_such_object",
                    format!("the agent answered, but it did not return {wanted}"),
                ),
            }
        }
        Err(error) => {
            // Whatever went wrong, the session is no longer trusted.
            lease.discard();
            Outcome::down("snmp_failed", error)
        }
    }
}

/// The metric names a run writes. `instance` is the check's id, so renaming a
/// check does not split its series.
pub const METRIC_UP: &str = "check.up";
pub const METRIC_RTT: &str = "check.rtt_ms";
pub const METRIC_CERT_DAYS: &str = "check.cert_days";

/// The samples one run produced.
///
/// `check.up` is written only for a run that reached a verdict about the
/// service: a run that could not happen leaves a gap, exactly as a failed
/// device read does.
pub fn samples(check: &ServiceCheck, outcome: &Outcome) -> Vec<crate::poll::Measure> {
    let mut measures = Vec::new();
    if outcome.status.measured() {
        measures.push(crate::poll::Measure::new(
            METRIC_UP,
            &check.id,
            if outcome.status.is_up() { 1.0 } else { 0.0 },
        ));
    }
    if let Some(rtt) = outcome.rtt_ms {
        measures.push(crate::poll::Measure::new(METRIC_RTT, &check.id, rtt));
    }
    if let Some(days) = outcome.facts.cert_expires_in_days {
        measures.push(crate::poll::Measure::new(
            METRIC_CERT_DAYS,
            &check.id,
            days as f64,
        ));
    }
    if let Some(loss) = outcome.facts.loss_pct {
        measures.push(crate::poll::Measure::new("check.loss_pct", &check.id, loss));
    }
    measures
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::model::{CheckState, CheckStatus};
    use nocturne_app::model::LabRef;

    fn check(spec: CheckSpec) -> ServiceCheck {
        ServiceCheck {
            id: "chk1".into(),
            device_id: "dev1".into(),
            name: "web".into(),
            spec,
            host: String::new(),
            enabled: true,
            interval_s: 60,
            timeout_ms: 2_000,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn device() -> Device {
        Device {
            id: "dev1".into(),
            name: "SW1".into(),
            address: "10.0.0.1".into(),
            ..Device::default()
        }
    }

    #[test]
    fn a_check_follows_the_devices_address_unless_it_names_a_host() {
        let aimed = aim(&check(CheckSpec::Tcp { port: 80 }), &device()).unwrap();
        assert_eq!(aimed.host, "10.0.0.1");
        assert!(!aimed.explicit);

        let mut named = check(CheckSpec::Tcp { port: 80 });
        named.host = " www.example.com ".into();
        let aimed = aim(&named, &device()).unwrap();
        assert_eq!(aimed.host, "www.example.com");
        assert!(aimed.explicit);
    }

    #[test]
    fn a_device_with_no_address_gives_a_check_that_could_not_run() {
        let mut nowhere = device();
        nowhere.address = String::new();
        let outcome = aim(&check(CheckSpec::Tcp { port: 80 }), &nowhere).unwrap_err();
        assert_eq!(outcome.status, CheckStatus::CannotRun);
        assert_eq!(outcome.code, "no_address");
        assert!(
            outcome.message.contains("no address"),
            "{}",
            outcome.message
        );
    }

    #[test]
    fn a_simulated_device_is_not_probed_on_somebody_elses_address() {
        let mut simulated = device();
        simulated.lab = Some(LabRef {
            lab_id: "lab1".into(),
            node: "SW1".into(),
        });
        let outcome = aim(&check(CheckSpec::Tcp { port: 80 }), &simulated).unwrap_err();
        assert_eq!(outcome.status, CheckStatus::CannotRun);
        assert_eq!(outcome.code, "sim");

        // Unless the check names a host of its own, which is a deliberate act.
        let mut named = check(CheckSpec::Tcp { port: 80 });
        named.host = "127.0.0.1".into();
        assert_eq!(aim(&named, &simulated).unwrap().host, "127.0.0.1");
    }

    #[test]
    fn a_run_that_could_not_happen_writes_no_samples_at_all() {
        let check = check(CheckSpec::Icmp {
            count: None,
            loss_pct_down: None,
        });
        let blind = Outcome::cannot_run("permission", "no ICMP sockets");
        assert!(
            samples(&check, &blind).is_empty(),
            "a gap, not a zero: `check.up` = 0 here would be an invented outage"
        );

        let down = Outcome::down("timeout", "nothing came back");
        let written = samples(&check, &down);
        assert_eq!(written.len(), 1);
        assert_eq!(
            (written[0].metric.as_str(), written[0].value),
            ("check.up", 0.0)
        );
        assert_eq!(
            written[0].instance, "chk1",
            "keyed by id, so a rename keeps the series"
        );
    }

    #[test]
    fn a_warning_is_written_as_available_with_the_days_beside_it() {
        let check = check(CheckSpec::Tls {
            port: None,
            sni: None,
            require_trusted: None,
            require_hostname: None,
            warn_days: None,
        });
        let outcome = Outcome::up("ok", Some(31.0))
            .with_facts(CheckFacts {
                cert_expires_in_days: Some(9),
                ..CheckFacts::default()
            })
            .warning_about("tls_expiring", "expires in 9 days");
        let written = samples(&check, &outcome);
        let value = |metric: &str| {
            written
                .iter()
                .find(|measure| measure.metric == metric)
                .map(|measure| measure.value)
        };
        assert_eq!(value("check.up"), Some(1.0), "it is still serving");
        assert_eq!(value("check.rtt_ms"), Some(31.0));
        assert_eq!(value("check.cert_days"), Some(9.0));
    }

    #[test]
    fn expiry_is_reported_before_the_untrust_it_causes() {
        let expired = judge_certificate(
            Outcome::up("ok", Some(10.0)),
            &Some("the certificate has expired".into()),
            true,
            false,
            Some(-3),
            14,
        );
        assert_eq!(expired.code, "tls_expired");
        assert!(expired.message.contains("3 day"), "{}", expired.message);
        assert_eq!(expired.status, CheckStatus::Down);

        let untrusted = judge_certificate(
            Outcome::up("ok", Some(10.0)),
            &Some("self-signed".into()),
            true,
            false,
            Some(300),
            14,
        );
        assert_eq!(untrusted.code, "tls_untrusted");
        assert_eq!(untrusted.message, "self-signed");

        // Trust not required: a self-signed certificate with life left is up.
        let tolerated = judge_certificate(
            Outcome::up("ok", Some(10.0)),
            &Some("self-signed".into()),
            false,
            false,
            Some(300),
            14,
        );
        assert_eq!(tolerated.status, CheckStatus::Up);

        let soon = judge_certificate(Outcome::up("ok", None), &None, true, true, Some(9), 14);
        assert_eq!(soon.status, CheckStatus::Warning);
        assert_eq!(soon.code, "tls_expiring");

        let fine = judge_certificate(Outcome::up("ok", None), &None, true, true, Some(90), 14);
        assert_eq!(fine.status, CheckStatus::Up);

        // On the boundary, the warning is on: a day of notice is the point.
        let edge = judge_certificate(Outcome::up("ok", None), &None, true, true, Some(14), 14);
        assert_eq!(edge.status, CheckStatus::Warning);
    }

    #[test]
    fn an_ipv6_literal_is_bracketed_before_it_reaches_an_engine() {
        assert_eq!(server_address("2001:db8::53", 53), "[2001:db8::53]:53");
        assert_eq!(server_address("[2001:db8::53]", 53), "[2001:db8::53]:53");
        assert_eq!(server_address("10.0.0.53", 5353), "10.0.0.53:5353");
    }

    #[test]
    fn a_state_starts_out_knowing_nothing() {
        // `unknown`, not `up`: a check that has never run has found nothing.
        let fresh = CheckState::default();
        assert_eq!(fresh.status, CheckStatus::Unknown);
        assert_eq!(fresh.last_run_at, None);
        assert_eq!(fresh.since, None);
    }
}
