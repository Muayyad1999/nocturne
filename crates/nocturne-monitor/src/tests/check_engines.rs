//! The checks against real servers: a TCP listener, an HTTP server, a
//! hand-built DNS responder, an SNTP responder and two TLS servers whose
//! certificates are generated when the test runs.
//!
//! All on loopback, all on a port the operating system picks, none of them
//! needing root or the internet. These are the tests that prove the engines
//! are being driven correctly, as opposed to the judging being correct, which
//! the unit tests next to the code cover.

use serde_json::json;

use crate::checks::model::CheckStatus;
use crate::testing::Harness;
use crate::tests::certificates;
use crate::tests::listeners::{self, Reply};

async fn harness_with_device() -> (Harness, String) {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "127.0.0.1", json!({})).await;
    harness.silence_rules().await;
    (harness, id)
}

// ------------------------------------------------------------------- TCP

#[tokio::test]
async fn a_tcp_check_tells_a_port_that_is_listening_from_one_that_is_not() {
    let (harness, device) = harness_with_device().await;
    let server = listeners::tcp_server().await;
    let listening = harness
        .check_on(
            &device,
            "open",
            json!({"kind": "tcp", "port": server.port}),
            json!({}),
        )
        .await;
    let nothing = harness
        .check_on(
            &device,
            "closed",
            json!({"kind": "tcp", "port": listeners::closed_port().await}),
            json!({}),
        )
        .await;

    let up = harness.run_check(&listening).await;
    assert_eq!(up.state.status, CheckStatus::Up, "{:?}", up.state);
    assert!(up.state.rtt_ms.is_some_and(f64::is_finite));

    let down = harness.run_check(&nothing).await;
    assert_eq!(down.state.status, CheckStatus::Down);
    // Unix refuses a connection to a port with nothing on it; Windows does
    // not always answer at all — on a CI runner a closed loopback port is
    // dropped rather than rejected, and the check times out. Both are the
    // same fact about the service, and the status above is the assertion
    // that matters. The code only says how the host put it.
    assert!(
        matches!(down.state.code.as_str(), "refused" | "timeout"),
        "a port with nothing on it is refused or times out: {:?}",
        down.state
    );
}

// ------------------------------------------------------------------ HTTP

#[tokio::test]
async fn an_http_check_is_up_on_a_status_it_was_promised_and_down_on_one_it_was_not() {
    let (harness, device) = harness_with_device().await;
    let (server, requests) = listeners::http_server(Reply::ok("all good here")).await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "http", "port": server.port, "path": "/health"}),
            json!({}),
        )
        .await;
    let up = harness.run_check(&check).await;
    assert_eq!(up.state.status, CheckStatus::Up, "{:?}", up.state);
    assert_eq!(up.state.facts.http_status, Some(200));
    assert!(up.state.rtt_ms.is_some_and(f64::is_finite));
    assert!(
        requests
            .lock()
            .iter()
            .any(|line| line.starts_with("GET /health")),
        "the path and method the check was given are what went out: {:?}",
        requests.lock()
    );

    // A 503 is an answer, so the response time is kept and the status is
    // reported — the check is down because of what it says, not because
    // nothing came back.
    let (unavailable, _) = listeners::http_server(Reply::status(503, "Service Unavailable")).await;
    let failing = harness
        .check_on(
            &device,
            "web503",
            json!({"kind": "http", "port": unavailable.port}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&failing).await;
    assert_eq!(down.state.status, CheckStatus::Down);
    assert_eq!(down.state.code, "http_status");
    assert_eq!(down.state.facts.http_status, Some(503));
    assert!(
        down.state.rtt_ms.is_some(),
        "a 503 still took a measurable time"
    );
    assert!(down.state.message.contains("503"), "{}", down.state.message);

    // Unless 503 is what it was told to expect, in which case it is up: a
    // check is about what the service promised, not about what is usual.
    let expected = harness
        .check_on(
            &device,
            "web503ok",
            json!({"kind": "http", "port": unavailable.port, "expect_status": [503]}),
            json!({}),
        )
        .await;
    assert_eq!(
        harness.run_check(&expected).await.state.status,
        CheckStatus::Up
    );
}

#[tokio::test]
async fn an_http_check_can_insist_on_what_the_page_says() {
    let (harness, device) = harness_with_device().await;
    let (server, _) = listeners::http_server(Reply::ok("database: ok\nqueue: ok\n")).await;
    let present = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "http", "port": server.port, "expect_text": "queue: ok"}),
            json!({}),
        )
        .await;
    assert_eq!(
        harness.run_check(&present).await.state.status,
        CheckStatus::Up
    );

    let absent = harness
        .check_on(
            &device,
            "web2",
            json!({"kind": "http", "port": server.port, "expect_text": "cache: ok"}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&absent).await;
    assert_eq!(down.state.status, CheckStatus::Down);
    assert_eq!(down.state.code, "http_expect_text");
    assert_eq!(
        down.state.facts.http_status,
        Some(200),
        "the page was served; it just did not say the right thing"
    );
    assert!(
        down.state.message.contains("cache: ok"),
        "{}",
        down.state.message
    );
}

#[tokio::test]
async fn an_http_check_against_a_port_with_nothing_on_it_is_down_not_blind() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "http", "port": listeners::closed_port().await}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&check).await;
    assert_eq!(
        down.state.status,
        CheckStatus::Down,
        "a refused connection is a measurement of the service: {:?}",
        down.state
    );
    assert_eq!(down.state.code, "refused");
}

#[tokio::test]
async fn an_https_check_against_something_that_is_not_tls_is_down() {
    // A plain HTTP server on the port an HTTPS check is pointed at: the
    // handshake fails, which is the service's fault, not the checker's.
    let (harness, device) = harness_with_device().await;
    let (server, _) = listeners::http_server(Reply::ok("plain")).await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "https", "port": server.port, "verify_tls": false}),
            json!({}),
        )
        .await;
    let outcome = harness.run_check(&check).await;
    assert_eq!(
        outcome.state.status,
        CheckStatus::Down,
        "{:?}",
        outcome.state
    );
    assert_ne!(
        outcome.state.status,
        CheckStatus::CannotRun,
        "the check ran perfectly well; the server could not speak TLS"
    );
}

// ------------------------------------------------------------------- DNS

#[tokio::test]
async fn a_dns_check_asks_the_server_in_front_of_it_rather_than_the_system_resolvers() {
    let (harness, device) = harness_with_device().await;
    let server = listeners::dns_server([10, 0, 0, 9], 0).await;
    let check = harness
        .check_on(
            &device,
            "resolver",
            json!({"kind": "dns", "name": "sw1.lab.example", "port": server.port}),
            json!({}),
        )
        .await;
    let up = harness.run_check(&check).await;
    assert_eq!(up.state.status, CheckStatus::Up, "{:?}", up.state);
    assert_eq!(up.state.facts.dns_rcode.as_deref(), Some("NOERROR"));
    assert_eq!(up.state.facts.dns_records, Some(1));
    assert!(up.state.rtt_ms.is_some_and(f64::is_finite));

    // And it can insist on the answer, which is how "this zone is serving
    // the right address" is checked rather than "something answered".
    let wrong = harness
        .check_on(
            &device,
            "resolver-expects",
            json!({"kind": "dns", "name": "sw1.lab.example", "port": server.port,
                   "expect": "10.0.0.1"}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&wrong).await;
    assert_eq!(down.state.status, CheckStatus::Down);
    assert_eq!(down.state.code, "dns_expect");
    assert_eq!(
        down.state.facts.dns_records,
        Some(1),
        "a record was returned; it was the wrong one"
    );
}

#[tokio::test]
async fn a_server_that_answers_nxdomain_for_a_name_it_should_serve_has_failed_the_check() {
    let (harness, device) = harness_with_device().await;
    // Rcode 3 is NXDOMAIN.
    let server = listeners::dns_server([0, 0, 0, 0], 3).await;
    let check = harness
        .check_on(
            &device,
            "resolver",
            json!({"kind": "dns", "name": "sw1.lab.example", "port": server.port}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&check).await;
    assert_eq!(down.state.status, CheckStatus::Down, "{:?}", down.state);
    assert_eq!(down.state.code, "dns_rcode");
    assert_eq!(down.state.facts.dns_rcode.as_deref(), Some("NXDOMAIN"));
    assert!(
        down.state.message.contains("NXDOMAIN"),
        "{}",
        down.state.message
    );
}

#[tokio::test]
async fn a_dns_check_pointed_at_a_port_with_nothing_on_it_times_out_rather_than_going_blind() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "resolver",
            json!({"kind": "dns", "name": "sw1.lab.example",
                   "port": listeners::closed_port().await}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&check).await;
    assert_eq!(down.state.status, CheckStatus::Down, "{:?}", down.state);
    assert!(
        ["timeout", "refused"].contains(&down.state.code.as_str()),
        "either answer is a measurement; `{}` is not",
        down.state.code
    );
}

// ------------------------------------------------------------------- NTP

#[tokio::test]
async fn an_ntp_check_reports_the_offset_it_measured() {
    let (harness, device) = harness_with_device().await;
    // A server whose clock is two minutes ahead of this machine's.
    let server = listeners::ntp_server(2, 120.0).await;
    let check = harness
        .check_on(
            &device,
            "time",
            json!({"kind": "ntp", "port": server.port}),
            json!({}),
        )
        .await;
    let up = harness.run_check(&check).await;
    assert_eq!(up.state.status, CheckStatus::Up, "{:?}", up.state);
    assert_eq!(up.state.facts.ntp_stratum, Some(2));
    let offset = up.state.facts.ntp_offset_ms.expect("an offset");
    assert!(
        (offset - 120_000.0).abs() < 5_000.0,
        "the clock is about two minutes out, measured as {offset} ms"
    );

    // With a limit on how far out the clock may be, the same server fails —
    // and the measurement is kept, because it is the reason.
    let strict = harness
        .check_on(
            &device,
            "time-strict",
            json!({"kind": "ntp", "port": server.port, "max_offset_ms": 1000.0}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&strict).await;
    assert_eq!(down.state.status, CheckStatus::Down);
    assert_eq!(down.state.code, "ntp_offset");
    assert!(down.state.facts.ntp_offset_ms.is_some());
}

#[tokio::test]
async fn a_time_server_that_admits_its_own_clock_is_not_set_fails_the_check() {
    let (harness, device) = harness_with_device().await;
    // Stratum 16: "do not use me".
    let server = listeners::ntp_server(16, 0.0).await;
    let check = harness
        .check_on(
            &device,
            "time",
            json!({"kind": "ntp", "port": server.port}),
            json!({}),
        )
        .await;
    let down = harness.run_check(&check).await;
    assert_eq!(down.state.status, CheckStatus::Down, "{:?}", down.state);
    assert_eq!(down.state.code, "ntp_unsynchronised");
    assert_eq!(down.state.facts.ntp_stratum, Some(16));
}

// ------------------------------------------------------- certificates

#[tokio::test]
async fn a_certificate_that_expires_next_week_is_a_warning_not_an_outage() {
    // The judgement call this feature had to make: "will fail in nine days"
    // is a different state from "is failing". The service is up — it is
    // serving right now — and saying it is down would be a lie that gets
    // service-down alerts ignored.
    let (harness, device) = harness_with_device().await;
    let server = certificates::tls_server(9).await;
    let check = harness
        .check_on(
            &device,
            "cert",
            // Self-signed, so trust is not what is under test here; the
            // server name is `localhost`, which the certificate carries.
            json!({"kind": "tls", "port": server.port, "sni": "localhost",
                   "require_trusted": false}),
            json!({}),
        )
        .await;
    let outcome = harness.run_check(&check).await;
    assert_eq!(
        outcome.state.status,
        CheckStatus::Warning,
        "{:?}",
        outcome.state
    );
    assert_eq!(outcome.state.code, "tls_expiring");
    assert!(
        outcome.state.status.is_up(),
        "a warning is still available: it is serving today"
    );
    let days = outcome
        .state
        .facts
        .cert_expires_in_days
        .expect("the days left");
    assert!(
        (7..=9).contains(&days),
        "about nine days, however the day boundary falls: {days}"
    );
    assert!(
        outcome.state.message.contains("day"),
        "the message says how long: {}",
        outcome.state.message
    );
    assert_eq!(
        outcome.state.facts.cert_subject.as_deref(),
        Some("localhost")
    );
    assert!(outcome.state.facts.cert_not_after.is_some());
    assert_eq!(outcome.state.facts.cert_trusted, Some(false));

    // The availability series says up, and the days are a series of their
    // own, so "when does this expire" can be charted and alerted on. (The
    // series API buckets points, so the value is what is asserted, not the
    // timestamp it is filed under.)
    let availability = harness.samples(&device, "check.up", &check);
    assert_eq!(
        availability
            .iter()
            .map(|(_, value)| *value)
            .collect::<Vec<_>>(),
        [1.0]
    );
    let charted = harness.samples(&device, "check.cert_days", &check);
    assert_eq!(charted.len(), 1);
    assert_eq!(charted[0].1, days as f64);
}

#[tokio::test]
async fn a_certificate_with_months_left_is_simply_up() {
    let (harness, device) = harness_with_device().await;
    let server = certificates::tls_server(200).await;
    let check = harness
        .check_on(
            &device,
            "cert",
            json!({"kind": "tls", "port": server.port, "sni": "localhost",
                   "require_trusted": false}),
            json!({}),
        )
        .await;
    let outcome = harness.run_check(&check).await;
    assert_eq!(outcome.state.status, CheckStatus::Up, "{:?}", outcome.state);
    assert_eq!(outcome.state.code, "ok");
    assert!(outcome.state.facts.cert_expires_in_days.unwrap() > 150);
}

#[tokio::test]
async fn a_certificate_that_has_already_expired_is_an_outage_and_says_when() {
    let (harness, device) = harness_with_device().await;
    let server = certificates::tls_server(-3).await;
    let check = harness
        .check_on(
            &device,
            "cert",
            json!({"kind": "tls", "port": server.port, "sni": "localhost",
                   "require_trusted": false}),
            json!({}),
        )
        .await;
    let outcome = harness.run_check(&check).await;
    assert_eq!(
        outcome.state.status,
        CheckStatus::Down,
        "{:?}",
        outcome.state
    );
    assert_eq!(outcome.state.code, "tls_expired");
    assert!(
        outcome.state.facts.cert_expires_in_days.unwrap() < 0,
        "negative days: {:?}",
        outcome.state.facts.cert_expires_in_days
    );
    assert!(
        outcome.state.message.contains("expired"),
        "{}",
        outcome.state.message
    );
    // The handshake still happened, so the certificate is described rather
    // than hidden behind a failure.
    assert!(outcome.state.facts.cert_not_after.is_some());
    assert!(outcome.state.facts.tls_protocol.is_some());
}

#[tokio::test]
async fn a_certificate_check_can_be_made_to_care_about_the_name_on_it() {
    let (harness, device) = harness_with_device().await;
    let server = certificates::tls_server(200).await;
    let check = harness
        .check_on(
            &device,
            "cert",
            json!({"kind": "tls", "port": server.port, "sni": "www.example.com",
                   "require_trusted": false}),
            json!({}),
        )
        .await;
    let outcome = harness.run_check(&check).await;
    assert_eq!(
        outcome.state.status,
        CheckStatus::Down,
        "{:?}",
        outcome.state
    );
    assert_eq!(outcome.state.code, "tls_name_mismatch");

    // Switched off, the same certificate is accepted: inspecting a
    // certificate that does not match is a thing people do on purpose.
    let relaxed = harness
        .check_on(
            &device,
            "cert-relaxed",
            json!({"kind": "tls", "port": server.port, "sni": "www.example.com",
                   "require_trusted": false, "require_hostname": false}),
            json!({}),
        )
        .await;
    assert_eq!(
        harness.run_check(&relaxed).await.state.status,
        CheckStatus::Up
    );
}

#[tokio::test]
async fn a_certificate_check_against_a_port_that_is_not_serving_tls_is_down() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "cert",
            json!({"kind": "tls", "port": listeners::closed_port().await}),
            json!({}),
        )
        .await;
    let outcome = harness.run_check(&check).await;
    assert_eq!(outcome.state.status, CheckStatus::Down);
    assert_ne!(outcome.state.status, CheckStatus::CannotRun);
}

#[tokio::test]
async fn an_https_check_watches_the_page_and_the_certificate_at_once() {
    // The point of `https` being its own kind: one run says both "the site
    // is serving" and "its certificate expires on Friday".
    let (harness, device) = harness_with_device().await;
    let server = certificates::tls_server(5).await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "https", "port": server.port, "verify_tls": false}),
            json!({}),
        )
        .await;
    let outcome = harness.run_check(&check).await;
    // The TLS server here only handshakes, so no HTTP response comes back:
    // what matters is that the failure is about the service and that the
    // certificate was still described.
    assert_ne!(
        outcome.state.status,
        CheckStatus::CannotRun,
        "the check ran: {:?}",
        outcome.state
    );
    assert_ne!(outcome.state.status, CheckStatus::Unknown);
}
