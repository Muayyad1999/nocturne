//! Notification channels against a listener on loopback: what each kind
//! really posts, that retries happen, and that the secret stays in the vault.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::model::{Alert, AlertState, Severity};
use crate::notify::{self, Stage};
use crate::testing::{Harness, ManualClock};

/// What the listener saw.
#[derive(Default)]
struct Seen {
    requests: Vec<(String, Value)>,
    raw: Vec<String>,
}

/// A one-connection-at-a-time HTTP listener. `statuses` is the status code to
/// answer each request with, in order; once it runs out, 200.
pub(super) struct Listener {
    pub(super) url: String,
    seen: Arc<Mutex<Seen>>,
}

pub(super) async fn listen(statuses: Vec<u16>) -> Listener {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        let mut answered = 0usize;
        loop {
            let Ok((mut stream, _)) = socket.accept().await else {
                return;
            };
            let mut buffer = Vec::new();
            // Read until the body is complete, going by Content-Length.
            loop {
                let mut chunk = [0u8; 4096];
                let read = match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => read,
                };
                buffer.extend_from_slice(&chunk[..read]);
                let text = String::from_utf8_lossy(&buffer).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let length = head
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&buffer).to_string();
            let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split(' ').nth(1))
                .unwrap_or("/")
                .to_string();
            {
                let mut seen = recorded.lock();
                seen.raw.push(text.clone());
                seen.requests
                    .push((path, serde_json::from_str(body).unwrap_or(Value::Null)));
            }
            let status = statuses.get(answered).copied().unwrap_or(200);
            answered += 1;
            let _ = stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                    )
                    .as_bytes(),
                )
                .await;
            let _ = stream.flush().await;
        }
    });
    Listener {
        url: format!("http://{address}/hooks/T000/B111/verysecrettoken"),
        seen,
    }
}

impl Listener {
    pub(super) fn requests(&self) -> Vec<(String, Value)> {
        self.seen.lock().requests.clone()
    }

    fn raw(&self) -> Vec<String> {
        self.seen.lock().raw.clone()
    }
}

impl Listener {
    /// The `event` of every notification received, once at least `count`
    /// have arrived — or after five seconds, which is only ever waited out
    /// by a test that is about to fail: the machine may be busy, and a
    /// delivery is its own task.
    pub(super) async fn events(&self, count: usize) -> Vec<String> {
        for _ in 0..250 {
            if self.requests().len() >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.requests()
            .iter()
            .map(|(_, body)| body["event"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// The same after a pause long enough for a notification that should
    /// not have been sent to arrive anyway.
    pub(super) async fn events_after_a_pause(&self) -> Vec<String> {
        tokio::time::sleep(Duration::from_millis(250)).await;
        self.events(0).await
    }
}

pub(super) const MINUTES: Duration = Duration::from_secs(60);

/// A harness on a clock the test steps by hand, with one webhook channel and
/// a listener behind it. These tests talk to a socket, so they run on real
/// time, and the hours a maintenance window takes are stepped instead.
pub(super) async fn harness_with_a_channel() -> (Harness, Arc<ManualClock>, Listener) {
    let clock = Arc::new(ManualClock::new(crate::testing::epoch()));
    let harness = Harness::with_clock(clock.clone()).await;
    let listener = listen(vec![]).await;
    harness
        .rpc(
            "notify.channels.save",
            json!({"name": "hook", "kind": "webhook", "url": listener.url}),
        )
        .await;
    (harness, clock, listener)
}

/// A device with one ICMP check named `web`, watched by the service-down
/// rule alone.
async fn a_watched_service(harness: &Harness) -> String {
    harness.only_rule("builtin.service_check_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness
        .check_on(&device, "web", json!({"kind": "icmp"}), json!({}))
        .await
}

pub(super) async fn window(harness: &Harness, starts_in: Duration, lasts: Duration) -> Value {
    let starts_at = harness.now() + starts_in.as_millis() as i64;
    harness
        .rpc(
            "maintenance.save",
            json!({"name": "Evening work", "starts_at": starts_at,
                   "ends_at": starts_at + lasts.as_millis() as i64}),
        )
        .await
}

async fn raise(harness: &Harness) -> Value {
    harness
        .rpc(
            "alerts.raise",
            json!({"severity": "critical", "title": "Something", "dedup_key": "k1"}),
        )
        .await
}

fn alert() -> Alert {
    Alert {
        id: "a1".into(),
        rule_id: Some("builtin.device_down".into()),
        rule_name: "Device down".into(),
        device_id: Some("dev1".into()),
        device: Some("CORE1".into()),
        instance: Some("GigabitEthernet1/0/1".into()),
        severity: Severity::Critical,
        state: AlertState::Firing,
        title: "Device down — CORE1".into(),
        detail: "The device has not answered for several polls in a row.".into(),
        value: Some(100.0),
        started_at: 1_767_225_600_000,
        last_seen_at: 1_767_225_660_000,
        count: 2,
        dedup_key: "rule:builtin.device_down:dev1:".into(),
        ..Alert::default()
    }
}

#[test]
fn each_channel_kind_builds_the_payload_its_service_expects() {
    let alert = alert();
    let webhook = notify::body(crate::model::ChannelKind::Webhook, &alert, Stage::Firing);
    assert_eq!(webhook["event"], "firing");
    assert_eq!(webhook["source"], "nocturne");
    assert_eq!(webhook["alert"]["id"], "a1");
    assert_eq!(webhook["alert"]["severity"], "critical");
    assert_eq!(webhook["alert"]["count"], 2);
    assert!(
        webhook["text"]
            .as_str()
            .unwrap()
            .starts_with("[ALERT] CORE1")
    );

    let slack = notify::body(crate::model::ChannelKind::Slack, &alert, Stage::Firing);
    assert!(slack["text"].as_str().unwrap().contains("CORE1"));
    let attachment = &slack["attachments"][0];
    assert_eq!(attachment["color"], "#cc0000");
    assert_eq!(attachment["title"], "Device down — CORE1");
    assert_eq!(attachment["ts"], 1_767_225_660i64);
    let fields: Vec<&str> = attachment["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        fields,
        [
            "Device", "Instance", "Severity", "Rule", "Value", "Started", "Seen"
        ]
    );

    let teams = notify::body(crate::model::ChannelKind::Teams, &alert, Stage::Firing);
    assert_eq!(teams["@type"], "MessageCard");
    assert_eq!(teams["@context"], "https://schema.org/extensions");
    assert_eq!(teams["themeColor"], "cc0000");
    assert_eq!(teams["sections"][0]["facts"][0]["name"], "Device");
    assert_eq!(teams["sections"][0]["facts"][0]["value"], "CORE1");

    let discord = notify::body(crate::model::ChannelKind::Discord, &alert, Stage::Firing);
    assert!(discord["content"].as_str().unwrap().contains("CORE1"));
    assert_eq!(discord["embeds"][0]["color"], 0x00cc_0000);
    assert_eq!(discord["embeds"][0]["fields"][0]["name"], "Device");
    assert_eq!(discord["embeds"][0]["fields"][0]["inline"], true);

    // Resolved is sent too, in green and with the word in the headline.
    let resolved = notify::body(
        crate::model::ChannelKind::Slack,
        &Alert {
            state: AlertState::Resolved,
            resolved_at: Some(1_767_225_720_000),
            ..alert.clone()
        },
        Stage::Resolved,
    );
    assert_eq!(resolved["attachments"][0]["color"], "#2eb886");
    assert!(
        resolved["text"]
            .as_str()
            .unwrap()
            .starts_with("[RESOLVED] CORE1")
    );

    let body = notify::email_body(&alert, Stage::Firing);
    assert!(body.contains("Device: CORE1"));
    assert!(body.contains("Severity: critical"));
    assert!(body.contains("Seen: 2 times"));
    assert!(body.ends_with("Sent by Nocturne.\n"));
}

#[tokio::test]
async fn a_webhook_channel_posts_to_the_url_in_the_vault() {
    let harness = Harness::new().await;
    let listener = listen(vec![]).await;
    let channel = harness
        .rpc(
            "notify.channels.save",
            json!({"name": "NOC Slack", "kind": "slack", "url": listener.url}),
        )
        .await;
    assert_eq!(channel["has_secret"], true);
    // The document holds no part of the URL.
    let document = serde_json::to_string(&channel).unwrap();
    assert!(
        !document.contains("verysecrettoken") && !document.contains("127.0.0.1"),
        "{document}"
    );

    let test = harness
        .rpc("notify.channels.test", json!({"id": channel["id"]}))
        .await;
    assert_eq!(test["sent"], true);
    assert_eq!(test["attempts"], 1);
    let requests = listener.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, "/hooks/T000/B111/verysecrettoken");
    assert!(
        requests[0].1["text"]
            .as_str()
            .unwrap()
            .contains("Nocturne test notification"),
        "a test message says it is one: {:?}",
        requests[0].1
    );
}

/// The back-off schedule, measured on the virtual clock.
///
/// Deliberately with no network in it. A paused tokio runtime fast-forwards
/// to the next armed timer whenever it has nothing ready to run, and waiting
/// on a real socket is exactly that: measured here, one loopback request with
/// an HTTP timeout armed advanced a paused clock by the whole 15 s timeout
/// when the peer was 300 ms slow. So what is timed is only the two `sleep`s
/// inside `deliver_with_retries`, and the channel is one whose delivery fails
/// before it reaches the wire.
#[tokio::test(start_paused = true)]
async fn the_back_off_between_attempts_is_one_second_then_five() {
    let harness = Harness::new().await;
    let channel = crate::model::Channel {
        id: "no-url".into(),
        name: "never configured".into(),
        kind: crate::model::ChannelKind::Webhook,
        enabled: true,
        has_secret: false,
        ..crate::model::Channel::default()
    };

    let started = harness.now();
    let error = notify::deliver_with_retries(&harness.app, &channel, &alert(), Stage::Firing)
        .await
        .unwrap_err();
    assert!(error.contains("no URL"), "{error}");
    assert_eq!(
        harness.now() - started,
        6_000,
        "three attempts, one second then five between them"
    );
}

/// And the retrying really happens over the transport. On real time, because
/// this one does talk to a socket.
#[tokio::test]
async fn a_failing_channel_is_retried_against_the_real_transport() {
    let harness = Harness::new().await;
    let listener = listen(vec![500]).await;
    let channel = harness
        .rpc(
            "notify.channels.save",
            json!({"name": "flaky", "kind": "webhook", "url": listener.url}),
        )
        .await;
    let stored: crate::model::Channel = harness
        .app
        .store()
        .get(
            crate::db::collections::CHANNELS,
            channel["id"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();

    let attempts = notify::deliver_with_retries(&harness.app, &stored, &alert(), Stage::Firing)
        .await
        .unwrap();
    assert_eq!(attempts, 2, "the first answer was a 500, the second a 200");
    let requests = listener.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].0, "/hooks/T000/B111/verysecrettoken",
        "the retry goes to the same place"
    );
}

/// Real time, not paused: it waits on a socket, so a paused clock would
/// fast-forward over the request and could fire the HTTP timeout instead.
#[tokio::test]
async fn a_channel_that_never_answers_is_recorded_without_its_url() {
    let harness = Harness::new().await;
    let listener = listen(vec![403, 403, 403]).await;
    let channel = harness
        .rpc(
            "notify.channels.save",
            json!({"name": "broken", "kind": "slack", "url": listener.url}),
        )
        .await;
    let error = harness
        .app
        .rpc("notify.channels.test", json!({"id": channel["id"]}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "unavailable");
    assert!(error.message.contains("HTTP 403"), "{error}");
    assert!(
        !error.message.contains("verysecrettoken"),
        "an error must not carry the URL: {error}"
    );

    let stored: crate::model::Channel = harness
        .app
        .store()
        .get(
            crate::db::collections::CHANNELS,
            channel["id"].as_str().unwrap(),
        )
        .unwrap()
        .unwrap();
    let recorded = stored.last_error.clone().unwrap();
    assert!(!recorded.contains("verysecrettoken"), "{recorded}");

    // Nor does anything in the event log, the documents or the events.
    let logs = harness
        .app
        .store()
        .log_query(&nocturne_app::store::LogQuery {
            limit: Some(500),
            ..Default::default()
        })
        .unwrap();
    let written = serde_json::to_string(&logs).unwrap();
    assert!(!written.contains("verysecrettoken"), "{written}");
    assert!(written.contains("could not be reached"), "{written}");

    let status = harness.rpc("monitor.status", json!({})).await;
    let problems = serde_json::to_string(&status["problems"]).unwrap();
    assert!(problems.contains("broken") && !problems.contains("verysecrettoken"));
}

#[tokio::test]
async fn an_alert_reaches_every_enabled_channel_above_its_severity() {
    let harness = Harness::new().await;
    let loud = listen(vec![]).await;
    let quiet = listen(vec![]).await;
    harness
        .rpc(
            "notify.channels.save",
            json!({"name": "everything", "kind": "webhook", "url": loud.url,
                   "min_severity": "info"}),
        )
        .await;
    harness
        .rpc(
            "notify.channels.save",
            json!({"name": "pager", "kind": "webhook", "url": quiet.url,
                   "min_severity": "critical"}),
        )
        .await;
    let off = listen(vec![]).await;
    harness
        .rpc(
            "notify.channels.save",
            json!({"name": "disabled", "kind": "webhook", "url": off.url, "enabled": false}),
        )
        .await;

    harness
        .rpc(
            "alerts.raise",
            json!({"severity": "high", "title": "Something", "dedup_key": "k1"}),
        )
        .await;
    // The notification runs as its own task.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if !loud.requests().is_empty() {
            break;
        }
    }
    assert_eq!(loud.requests().len(), 1);
    assert!(quiet.requests().is_empty(), "high is below critical");
    assert!(off.requests().is_empty(), "a disabled channel is not used");
}

#[tokio::test]
async fn a_muted_alert_is_recorded_and_not_sent() {
    let harness = Harness::new().await;
    let listener = listen(vec![]).await;
    harness
        .rpc(
            "notify.channels.save",
            json!({"name": "hook", "kind": "webhook", "url": listener.url}),
        )
        .await;
    let now = harness.now();
    harness
        .rpc(
            "maintenance.save",
            json!({"name": "window", "starts_at": now - 1_000, "ends_at": now + 3_600_000}),
        )
        .await;
    let alert = harness
        .rpc(
            "alerts.raise",
            json!({"severity": "critical", "title": "Something", "dedup_key": "k1"}),
        )
        .await;
    assert_eq!(alert["muted_by_maintenance"], true);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        listener.requests().is_empty(),
        "a maintenance window mutes the notification but keeps the alert"
    );
    assert_eq!(harness.open_alerts().len(), 1);
}

#[tokio::test]
async fn channels_are_validated_and_their_secrets_removed_with_them() {
    let harness = Harness::new().await;
    for bad in [
        json!({"name": "", "kind": "webhook", "url": "https://example.net/x"}),
        json!({"name": "x", "kind": "webhook", "url": "ftp://example.net/x"}),
        json!({"name": "x", "kind": "webhook"}),
        json!({"name": "x", "kind": "email"}),
        json!({"name": "x", "kind": "email", "email": {"host": "smtp.example.net"}}),
        json!({"name": "x", "kind": "email",
               "email": {"host": "h", "from": "a@b", "to": ["c@d"], "starttls": "maybe"}}),
    ] {
        let error = harness
            .app
            .rpc("notify.channels.save", bad.clone())
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid", "{bad} should be refused");
    }

    let channel = harness
        .rpc(
            "notify.channels.save",
            json!({"name": "mail", "kind": "email", "password": "hunter2",
                   "email": {"host": "smtp.example.net", "port": 587, "from": "n@example.net",
                             "to": ["noc@example.net"], "username": "n", "starttls": "auto"}}),
        )
        .await;
    let id = channel["id"].as_str().unwrap().to_string();
    assert_eq!(channel["has_secret"], true);
    assert!(!serde_json::to_string(&channel).unwrap().contains("hunter2"));
    assert!(
        harness
            .app
            .vault()
            .get(&crate::db::channel_secret_id(&id))
            .unwrap()
            .is_some()
    );

    let listed = harness.rpc("notify.channels.list", json!({})).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(!serde_json::to_string(&listed).unwrap().contains("hunter2"));

    harness
        .rpc("notify.channels.delete", json!({"id": id}))
        .await;
    assert!(
        harness
            .app
            .vault()
            .get(&crate::db::channel_secret_id(&id))
            .unwrap()
            .is_none(),
        "deleting a channel takes its secret with it"
    );
    assert!(
        harness
            .app
            .rpc("notify.channels.test", json!({"id": id}))
            .await
            .unwrap_err()
            .code
            == "not_found"
    );
}

#[tokio::test]
async fn the_raw_request_carries_nothing_but_the_payload() {
    let harness = Harness::new().await;
    let listener = listen(vec![]).await;
    let channel = harness
        .rpc(
            "notify.channels.save",
            json!({"name": "hook", "kind": "webhook", "url": listener.url}),
        )
        .await;
    harness
        .rpc("notify.channels.test", json!({"id": channel["id"]}))
        .await;
    let raw = listener.raw();
    assert_eq!(raw.len(), 1);
    assert!(raw[0].starts_with("POST /hooks/T000/B111/verysecrettoken HTTP/1.1"));
    assert!(
        raw[0]
            .to_lowercase()
            .contains("content-type: application/json"),
        "{}",
        raw[0]
    );
    assert!(
        raw[0].contains("nocturne/"),
        "a user agent that says who it is"
    );
}

/// A relay that accepts one message and reports every line it was sent.
async fn smtp_relay() -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        while let Ok((stream, _)) = listener.accept().await {
            let mut reader = BufReader::new(stream);
            async fn say(reader: &mut BufReader<tokio::net::TcpStream>, text: &str) {
                let _ = reader.get_mut().write_all(text.as_bytes()).await;
            }
            say(&mut reader, "220 relay\r\n").await;
            let mut in_data = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                let line = line.trim_end_matches(['\r', '\n']).to_string();
                recorded.lock().push(line.clone());
                if in_data {
                    if line == "." {
                        in_data = false;
                        say(&mut reader, "250 queued\r\n").await;
                    }
                    continue;
                }
                let reply = match line.split(' ').next().unwrap_or("") {
                    "EHLO" => "250-relay\r\n250 AUTH PLAIN\r\n",
                    "AUTH" => "235 ok\r\n",
                    "DATA" => {
                        in_data = true;
                        "354 go ahead\r\n"
                    }
                    "QUIT" => "221 bye\r\n",
                    _ => "250 ok\r\n",
                };
                say(&mut reader, reply).await;
                if line == "QUIT" {
                    break;
                }
            }
        }
    });
    (address, seen)
}

#[tokio::test]
async fn an_email_channel_hands_the_alert_to_a_relay() {
    let harness = Harness::new().await;
    let (address, seen) = smtp_relay().await;
    let channel = harness
        .rpc(
            "notify.channels.save",
            json!({"name": "NOC mail", "kind": "email", "password": "hunter2",
                   "email": {"host": address.ip().to_string(), "port": address.port(),
                             "starttls": "never", "username": "noc",
                             "from": "nocturne@example.net",
                             "to": ["noc@example.net", "oncall@example.net"]}}),
        )
        .await;
    assert_eq!(channel["has_secret"], true);
    assert!(channel["email"]["port"].as_u64().unwrap() > 0);

    let sent = harness
        .rpc("notify.channels.test", json!({"id": channel["id"]}))
        .await;
    assert_eq!(sent["sent"], true);

    let lines = seen.lock().clone();
    assert!(lines.iter().any(|line| line == "EHLO nocturne"));
    assert!(lines.contains(&"MAIL FROM:<nocturne@example.net>".to_string()));
    assert!(lines.contains(&"RCPT TO:<noc@example.net>".to_string()));
    assert!(lines.contains(&"RCPT TO:<oncall@example.net>".to_string()));
    assert!(
        lines.iter().any(
            |line| line.starts_with("Subject: ") && line.contains("Nocturne test notification")
        ),
        "{lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.starts_with("AUTH PLAIN ")),
        "{lines:?}"
    );
    // The password is base64 inside the AUTH line and nowhere else in the clear.
    assert!(!lines.iter().any(|line| line.contains("hunter2")));
}

// ------------------------------------------------ muting, decided when due

#[tokio::test]
async fn an_outage_that_outlasts_its_maintenance_window_is_announced_when_the_window_closes() {
    // 22:00–23:00 covers SW1; `web` goes down at 22:10 and stays down. The
    // alert is raised muted, which is right — and used to be the last
    // chance anybody had of hearing about it.
    let (harness, clock, listener) = harness_with_a_channel().await;
    let check = a_watched_service(&harness).await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;

    clock.advance(10 * MINUTES);
    harness.probe.down("10.0.0.1");
    harness.run_check(&check).await;
    let raised = harness.open_alerts();
    assert_eq!(raised.len(), 1);
    assert!(raised[0].muted_by_maintenance);
    assert_eq!(raised[0].notified_at, None, "nobody has been told yet");
    assert!(listener.events_after_a_pause().await.is_empty());

    // 23:03: the window has closed and the service is still down.
    clock.advance(53 * MINUTES);
    harness.run_check(&check).await;
    assert_eq!(listener.events(1).await, ["firing"]);
    assert_eq!(listener.requests()[0].1["alert"]["id"], json!(raised[0].id));
    let told = crate::alerts::get(&harness.app, &raised[0].id).unwrap();
    assert!(!told.muted_by_maintenance);
    assert_eq!(told.notified_at, Some(harness.now()));

    // Once: the next sighting is not another announcement.
    clock.advance(5 * MINUTES);
    harness.run_check(&check).await;
    assert_eq!(listener.events_after_a_pause().await, ["firing"]);

    // And having been announced, its recovery is too.
    clock.advance(5 * MINUTES);
    harness.probe.up("10.0.0.1");
    harness.run_check(&check).await;
    assert_eq!(listener.events(2).await, ["firing", "resolved"]);
}

#[tokio::test]
async fn an_announcement_held_back_by_a_window_does_not_wait_for_a_slow_check_to_run_again() {
    // A check that runs once an hour — or a certificate check, once a day —
    // found the service down inside the window. Until it runs again nothing
    // new is measured, so nothing is counted; but the announcement is owed
    // from the moment the window closes, and the next look at the verdict
    // pays it. Waiting for the next run would leave a live outage unsaid
    // for as long as the check's interval.
    let (harness, clock, listener) = harness_with_a_channel().await;
    harness.only_rule("builtin.service_check_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.80");
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.0.0.80", "interval_s": 3_600}),
        )
        .await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;

    clock.advance(10 * MINUTES);
    harness.run_check(&check).await;
    let seen_at = harness.now();
    let raised = harness.open_alerts();
    assert_eq!(raised.len(), 1);
    assert!(raised[0].muted_by_maintenance);
    // A poll inside the window changes nothing.
    clock.advance(MINUTES);
    harness.poll_and_evaluate(&device).await;
    assert!(listener.events_after_a_pause().await.is_empty());

    // 23:01: the window has closed, the check is not due for another
    // forty-nine minutes, and the device is polled as it is every minute.
    clock.advance(50 * MINUTES);
    harness.poll_and_evaluate(&device).await;
    assert_eq!(listener.events(1).await, ["firing"]);
    let told = crate::alerts::get(&harness.app, &raised[0].id).unwrap();
    assert_eq!(told.notified_at, Some(harness.now()));
    assert!(!told.muted_by_maintenance);
    assert_eq!(
        (told.count, told.last_seen_at),
        (1, seen_at),
        "told about, not seen again: nothing new was measured"
    );

    // Once.
    clock.advance(MINUTES);
    harness.poll_and_evaluate(&device).await;
    assert_eq!(listener.events_after_a_pause().await, ["firing"]);
}

#[tokio::test]
async fn a_recovery_is_announced_although_the_alert_was_last_seen_inside_a_window() {
    // Announced at 21:55, seen again at 22:58 inside the window, recovered
    // at 23:03 after it. The mute flag of the 22:58 sighting used to
    // swallow the "resolved", leaving the incident open in the chat.
    let (harness, clock, listener) = harness_with_a_channel().await;
    let check = a_watched_service(&harness).await;
    window(&harness, 5 * MINUTES, 60 * MINUTES).await;

    harness.probe.down("10.0.0.1");
    harness.run_check(&check).await;
    assert_eq!(listener.events(1).await, ["firing"]);

    clock.advance(63 * MINUTES);
    harness.run_check(&check).await;
    assert!(harness.open_alerts()[0].muted_by_maintenance);

    clock.advance(5 * MINUTES);
    harness.probe.up("10.0.0.1");
    harness.run_check(&check).await;
    assert_eq!(listener.events(2).await, ["firing", "resolved"]);
    let resolved = &harness.all_alerts()[0];
    assert_eq!(resolved.state, AlertState::Resolved);
    assert!(
        !resolved.muted_by_maintenance,
        "the record says what was true when it resolved"
    );
}

#[tokio::test]
async fn an_alert_that_was_announced_is_closed_for_its_audience_even_inside_a_window() {
    // A window that opens after people were told must not leave them with
    // an incident that never ends: the recovery is the other half of a
    // notification they already have.
    let (harness, clock, listener) = harness_with_a_channel().await;
    let alert = raise(&harness).await;
    assert_eq!(listener.events(1).await, ["firing"]);

    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    clock.advance(10 * MINUTES);
    harness
        .rpc("alerts.resolve", json!({"id": alert["id"]}))
        .await;
    assert_eq!(listener.events(2).await, ["firing", "resolved"]);

    // A snooze is no different.
    clock.advance(60 * MINUTES);
    let again = raise(&harness).await;
    assert_eq!(listener.events(3).await.len(), 3);
    harness
        .rpc("alerts.snooze", json!({"id": again["id"], "minutes": 240}))
        .await;
    harness
        .rpc("alerts.resolve", json!({"id": again["id"]}))
        .await;
    assert_eq!(
        listener.events(4).await,
        ["firing", "resolved", "firing", "resolved"]
    );
}

#[tokio::test]
async fn an_alert_nobody_was_told_about_resolves_without_a_word() {
    // Raised and resolved inside the window: recorded, and that is all. A
    // "resolved" for something never announced would be the first anybody
    // heard of it.
    let (harness, clock, listener) = harness_with_a_channel().await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let inside = raise(&harness).await;
    assert_eq!(inside["muted_by_maintenance"], true);
    assert!(inside["notified_at"].is_null());
    clock.advance(10 * MINUTES);
    harness
        .rpc("alerts.resolve", json!({"id": inside["id"]}))
        .await;

    // Nor when it resolves after the window without being seen again.
    let again = raise(&harness).await;
    clock.advance(60 * MINUTES);
    let resolved = harness
        .rpc("alerts.resolve", json!({"id": again["id"]}))
        .await;
    assert_eq!(resolved["state"], "resolved");
    assert!(listener.events_after_a_pause().await.is_empty());
    assert_eq!(harness.all_alerts().len(), 2, "both are on record");
}

#[tokio::test]
async fn a_snooze_holds_an_announcement_back_and_its_end_lets_it_through() {
    let (harness, clock, listener) = harness_with_a_channel().await;
    let opened = window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let alert = raise(&harness).await;
    harness
        .rpc("alerts.snooze", json!({"id": alert["id"], "minutes": 30}))
        .await;

    // The window goes away, but the snooze still stands.
    harness
        .rpc("maintenance.delete", json!({"id": opened["id"]}))
        .await;
    clock.advance(10 * MINUTES);
    let seen = raise(&harness).await;
    assert_eq!(seen["muted_by_maintenance"], false);
    assert!(seen["notified_at"].is_null());
    assert!(listener.events_after_a_pause().await.is_empty());

    clock.advance(21 * MINUTES);
    let seen = raise(&harness).await;
    assert_eq!(seen["id"], alert["id"]);
    assert_eq!(seen["notified_at"], json!(harness.now()));
    assert_eq!(listener.events(1).await, ["firing"]);
}

#[tokio::test]
async fn an_alert_somebody_acknowledged_while_it_was_muted_wakes_nobody_afterwards() {
    // Acknowledged means a person has it. Announcing it to everyone else
    // once the window closes would be telling them about handled work.
    let (harness, clock, listener) = harness_with_a_channel().await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let alert = raise(&harness).await;
    harness.rpc("alerts.ack", json!({"id": alert["id"]})).await;

    clock.advance(61 * MINUTES);
    let seen = raise(&harness).await;
    assert_eq!(seen["state"], "acknowledged");
    assert_eq!(seen["count"], 2);
    harness
        .rpc("alerts.resolve", json!({"id": alert["id"]}))
        .await;
    assert!(listener.events_after_a_pause().await.is_empty());
}

#[tokio::test]
async fn an_alert_from_before_notices_were_tracked_is_not_announced_a_second_time() {
    // An alert that was open when the application was upgraded has no
    // `notified_at`. Read as "nobody was told", every such alert would be
    // announced again at its next sighting — and one that only resolved
    // would never be closed for the people who had been told.
    let (harness, _clock, listener) = harness_with_a_channel().await;
    let old = |id: &str, key: &str, muted: bool| {
        let mut body = serde_json::to_value(Alert {
            id: id.into(),
            dedup_key: key.into(),
            title: "Something".into(),
            severity: Severity::Critical,
            started_at: harness.now() - 600_000,
            last_seen_at: harness.now() - 60_000,
            count: 3,
            muted_by_maintenance: muted,
            ..Alert::default()
        })
        .unwrap();
        body.as_object_mut().unwrap().remove("notified_at");
        harness
            .app
            .store()
            .with(|conn| {
                conn.execute(
                    "INSERT INTO monitor_alerts
                     (id, dedup_key, state, severity, device_id, started_at, last_seen_at, body)
                     VALUES (?1, ?2, 'firing', 4, NULL, ?3, ?4, ?5)",
                    rusqlite::params![
                        id,
                        key,
                        body["started_at"].as_i64(),
                        body["last_seen_at"].as_i64(),
                        body.to_string()
                    ],
                )
            })
            .unwrap();
    };
    old("told", "k1", false);
    old("quiet", "k2", true);
    // The next start of the application, and the one after: what the first
    // wrote down, the second leaves alone.
    crate::db::init(harness.app.store()).unwrap();
    crate::db::init(harness.app.store()).unwrap();

    let told = crate::alerts::get(&harness.app, "told").unwrap();
    assert_eq!(
        told.notified_at,
        Some(told.started_at),
        "raised outside any window: its notice went out then"
    );
    assert_eq!(
        crate::alerts::get(&harness.app, "quiet")
            .unwrap()
            .notified_at,
        None,
        "last seen inside a window: nothing says anybody was told"
    );

    let seen = raise(&harness).await;
    assert_eq!(
        (seen["id"].as_str(), seen["count"].as_u64()),
        (Some("told"), Some(4))
    );
    assert!(listener.events_after_a_pause().await.is_empty());
    harness.rpc("alerts.resolve", json!({"id": "told"})).await;
    assert_eq!(listener.events(1).await, ["resolved"]);
}
