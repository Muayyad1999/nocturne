//! Output under load: a device that prints megabytes must not become ten
//! thousand events, must not grow the backend's memory, and must not lose or
//! corrupt a byte on the way.

mod common;

use std::time::{Duration, Instant};

use common::{Events, PATIENCE, Script, open_device, scripted, session_id};
use serde_json::json;

/// Flushed at most every 8 ms or 32 KiB (`session.rs`).
const FLUSH_BYTES: usize = 32 * 1024;
/// The smallest scrollback the settings allow, in bytes.
const SMALL_SCROLLBACK: usize = 64 * 1024;

/// About five megabytes of checkable text: numbered lines of three-byte
/// characters, whose length does not divide the flush size, so a character
/// sooner or later straddles two events.
fn five_megabytes() -> String {
    let snowmen = "☃".repeat(30);
    let mut text: String = (0..53_000)
        .map(|index| format!("{index:07} {snowmen}\n"))
        .collect();
    // The lines all look alike, so the end is marked.
    text.push_str(END);
    text
}

/// The last thing the burst prints.
const END: &str = "=== end of burst ===\n";

fn blast(text: &str) -> Script {
    Script {
        blast: Some(text.as_bytes().to_vec()),
        ..Script::default()
    }
}

async fn set_scrollback(app: &nocturne_app::App, kib: u32) {
    app.rpc(
        "settings.set",
        json!({"section": "terminal", "value": {"scrollback_kib": kib}}),
    )
    .await
    .expect("settings");
}

fn decode(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64")
}

#[tokio::test]
async fn a_five_megabyte_burst_stays_bounded_and_arrives_intact() {
    let text = five_megabytes();
    let lab = scripted(&[("BLAST", blast(&text))], false).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("BLAST")).await.expect("open"));

    events.wait_for(&id, END).await;

    let seen = events.output(&id);
    assert_eq!(seen.len(), text.len(), "bytes went missing or were doubled");
    assert_eq!(seen, text.as_bytes(), "the bytes arrived changed");

    // Bounded event count: 5 MB at 32 KiB apiece, not ten thousand events.
    let count = events.counts[&id];
    let floor = text.len().div_ceil(FLUSH_BYTES);
    assert!(
        (floor..floor + 16).contains(&count),
        "{count} events for {} bytes (expected about {floor})",
        text.len()
    );
    assert!(
        events.largest(&id) <= FLUSH_BYTES,
        "an event of {} bytes is larger than the flush size",
        events.largest(&id)
    );

    // A character was split across events, and came back whole.
    assert!(
        events.split_characters.get(&id).copied().unwrap_or(0) > 0,
        "no event boundary fell inside a character, so nothing was proved"
    );
    let assembled = String::from_utf8(seen).expect("the stream is still valid UTF-8");
    assert_eq!(assembled.matches('☃').count(), 53_000 * 30);
    assert_eq!(assembled, text);
}

#[tokio::test]
async fn memory_stays_bounded_when_nothing_is_attached() {
    // Twelve megabytes into a 64 KiB scrollback, with no UI subscribed at any
    // point: what the backend keeps is the ring, not the stream.
    let text = "x".repeat(12 * 1024 * 1024);
    let lab = scripted(&[("BLAST", blast(&text))], false).await;
    let app = &lab.app;
    set_scrollback(app, 64).await;
    let id = session_id(&open_device(app, &lab.id("BLAST")).await.expect("open"));

    let deadline = Instant::now() + PATIENCE;
    loop {
        let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
        if listed[0]["bytes_in"].as_u64().unwrap_or(0) >= text.len() as u64 {
            break;
        }
        assert!(Instant::now() < deadline, "the burst never ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let attached = app
        .rpc("terminal.attach", json!({"session_id": id}))
        .await
        .expect("attach");
    let scrollback = decode(attached["scrollback"].as_str().expect("scrollback"));
    assert!(
        scrollback.len() <= SMALL_SCROLLBACK,
        "the scrollback grew to {} bytes",
        scrollback.len()
    );
    assert!(
        scrollback.len() >= SMALL_SCROLLBACK - FLUSH_BYTES,
        "the scrollback shrank to {} bytes",
        scrollback.len()
    );
    assert!(scrollback.iter().all(|byte| *byte == b'x'));
}

#[tokio::test]
async fn one_flooding_session_does_not_hold_up_another() {
    let text = five_megabytes();
    let lab = scripted(
        &[
            ("BLAST", blast(&text)),
            (
                "QUIET",
                Script {
                    banner: "\r\nQUIET>".into(),
                    listen_only: true,
                    ..Script::default()
                },
            ),
        ],
        false,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let flooding = session_id(&open_device(app, &lab.id("BLAST")).await.expect("open"));
    let quiet = session_id(&open_device(app, &lab.id("QUIET")).await.expect("open"));

    // The quiet session takes input while the other is still printing.
    events.wait_for(&quiet, "QUIET>").await;
    app.rpc(
        "terminal.input",
        json!({"session_id": quiet, "data": "hello\r"}),
    )
    .await
    .expect("input");
    let deadline = Instant::now() + PATIENCE;
    while lab.heard("QUIET").lines().is_empty() {
        assert!(Instant::now() < deadline, "the quiet session was held up");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(lab.heard("QUIET").lines(), ["hello"]);

    events.wait_for(&flooding, END).await;
    assert_eq!(events.output(&flooding).len(), text.len());
}
