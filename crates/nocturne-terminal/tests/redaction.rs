//! What a redacted read of a session shows of a private key the device
//! printed — whatever part of the key the read happens to cover — and the
//! record a read with `redact: false` leaves.
//!
//! A snapshot is a window on the scrollback and a page is a window on a log
//! file. A key is the one secret that is longer than a line, so it is the one
//! a window can cut: these tests put the edges of the window in every place a
//! key can meet them.

mod common;

use common::{Events, builder_at, session_id};
use nocturne_app::testing::{self, Lab};
use serde_json::{Value, json};

/// Not a key: lines of the base64 alphabet, as long as the lines of one,
/// written by hand for these tests (synthetic, not recorded from a device).
const KEY_LINES: [&str; 12] = [
    "MIIEpAIBAAKCAQEA0nocturneTest01aaaaBBBBccccDDDDeeeeFFFFggggHHHHii",
    "Zm9yIHRlc3RzIG9ubHkgMDIgbm90IGEga2V5IGF0IGFsbCBqdXN0IGxldHRlcnMg",
    "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVowMTIzNDU2Nzg5YWJjZGVmZ2hpams0",
    "bm9jdHVybmUgdGVybWluYWwgcmVkYWN0aW9uIHRlc3QgbGluZSAwNCBwYWRkaW5n",
    "c3ludGhldGljIGtleSBtYXRlcmlhbCBsaW5lIDA1IG5vdGhpbmcgcmVhbCBoZXJl",
    "MDYvMDYrMDYvMDYrMDYvMDYrMDYvMDYrMDYvMDYrMDYvMDYrMDYvMDYrMDYvMDYr",
    "c2V2ZW50aCBsaW5lIG9mIHRoZSBwcmV0ZW5kIGtleSB3aXRoIG1vcmUgbGV0dGVy",
    "ZWlnaHRoIGxpbmUgb2YgdGhlIHByZXRlbmQga2V5IHN0aWxsIG5vdCBhIGtleSAw",
    "bmludGggbGluZSBvZiB0aGUgcHJldGVuZCBrZXkgYW5kIHNvIGl0IGdvZXMgb24w",
    "dGVudGggbGluZSBvZiB0aGUgcHJldGVuZCBrZXkgYWxtb3N0IGF0IGl0cyBlbmQw",
    "ZWxldmVudGggbGluZSBvZiB0aGUgcHJldGVuZCBrZXkgb25lIG1vcmUgdG8gZ28w",
    "dHdlbGZ0aCBhbmQgbGFzdCBsaW5lIG9mIGl0PT0=",
];
const BEGIN: &str = "-----BEGIN RSA PRIVATE KEY-----";
const END: &str = "-----END RSA PRIVATE KEY-----";

fn whole_key() -> String {
    format!("{BEGIN}\n{}\n{END}\n", KEY_LINES.join("\n"))
}

/// The lines of the pretend key that `text` shows.
fn shown(text: &str) -> Vec<&'static str> {
    KEY_LINES
        .iter()
        .copied()
        .filter(|line| text.contains(line))
        .collect()
}

/// A fake switch that prints the key for `show key`, and the first lines of
/// it — as a pager that stopped would leave it — for `show key head`.
async fn lab_with_a_key(builder: nocturne_app::AppBuilder) -> Lab {
    let lab = testing::lab(builder, &["SW1"]).await;
    lab.fake("SW1").with(|device| {
        device.outputs.insert("show key".into(), whole_key());
        device.outputs.insert(
            "show key head".into(),
            format!("{BEGIN}\n{}\n", KEY_LINES[..6].join("\n")),
        );
        device
            .outputs
            .insert("show clock".into(), "12:00:00.000 UTC\n".into());
        device
            .outputs
            .insert("show uptime".into(), "up 3 weeks\n".into());
    });
    lab
}

async fn type_line(lab: &Lab, events: &mut Events, id: &str, command: &str, until: &str) {
    lab.app
        .rpc(
            "terminal.input",
            json!({"session_id": id, "data": format!("{command}\r")}),
        )
        .await
        .expect("input");
    events.wait_for(id, until).await;
}

/// A logged session that printed the whole key between two other commands,
/// closed, so that its log is complete. Returns the log's name.
async fn a_log_with_a_key(lab: &Lab) -> String {
    let app = &lab.app;
    let mut events = Events::new(app);
    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("SW1"), "cols": 100, "rows": 30, "log": true}),
        )
        .await
        .expect("open");
    let id = session_id(&info);
    let name = info["logging"].as_str().expect("a log name").to_string();
    events.wait_for(&id, "SW1>").await;
    type_line(lab, &mut events, &id, "show clock", "UTC").await;
    type_line(lab, &mut events, &id, "show key", END).await;
    type_line(lab, &mut events, &id, "show uptime", "up 3 weeks\r\nSW1>").await;
    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
    name
}

/// Every page of a log, read the way a client pages: from `next_offset`.
async fn pages(lab: &Lab, name: &str, limit: u64) -> Vec<Value> {
    let mut offset = 0u64;
    let mut pages = Vec::new();
    loop {
        let page = lab
            .app
            .rpc(
                "terminal.logs.read",
                json!({"name": name, "offset": offset, "limit": limit}),
            )
            .await
            .expect("read");
        let next = page["next_offset"].as_u64().expect("next_offset");
        let eof = page["eof"].as_bool().expect("eof");
        assert!(next > offset || eof, "the read at {offset} went nowhere");
        offset = next;
        pages.push(page);
        if eof {
            return pages;
        }
        assert!(pages.len() < 10_000, "paging does not end");
    }
}

#[tokio::test]
async fn a_key_in_a_session_log_is_hidden_although_every_line_carries_a_timestamp() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = lab_with_a_key(builder_at(dir.path())).await;
    let name = a_log_with_a_key(&lab).await;

    // The file holds what the device printed, each line under its timestamp.
    let on_disk = std::fs::read_to_string(dir.path().join("sessions").join(&name)).expect("log");
    assert_eq!(shown(&on_disk).len(), KEY_LINES.len(), "{on_disk}");
    for line in on_disk.lines().filter(|line| line.contains(KEY_LINES[0])) {
        assert!(!line.starts_with(KEY_LINES[0]), "no timestamp: {line}");
    }

    let read = lab
        .app
        .rpc("terminal.logs.read", json!({"name": name}))
        .await
        .expect("read");
    let text = read["text"].as_str().expect("text");
    assert_eq!(shown(text), Vec::<&str>::new(), "{text}");
    assert_eq!(read["redacted"], true);
    // What is not the key is still there, under its timestamp.
    assert!(text.contains(" SW1>show key\n"), "{text}");
    assert!(text.contains(" 12:00:00.000 UTC\n"), "{text}");
    assert!(text.contains(" up 3 weeks\n"), "{text}");
}

#[tokio::test]
async fn a_key_is_hidden_on_every_page_of_a_log_however_small_the_pages() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = lab_with_a_key(builder_at(dir.path())).await;
    let name = a_log_with_a_key(&lab).await;

    // 4096: the key is cut once. 200: two lines to a page, so most pages of
    // it hold neither of its markers. 40: less than a line.
    for limit in [4096u64, 200, 40] {
        let pages = pages(&lab, &name, limit).await;
        let whole: String = pages
            .iter()
            .map(|page| page["text"].as_str().expect("text"))
            .collect();
        assert_eq!(
            shown(&whole),
            Vec::<&str>::new(),
            "pages of {limit} bytes showed the key:\n{whole}"
        );
        // Nothing else went missing on the way: the pages are the log.
        assert!(whole.contains(" SW1>show key\n"), "{limit}: {whole}");
        assert!(whole.contains(" 12:00:00.000 UTC\n"), "{limit}: {whole}");
        assert!(whole.contains(" up 3 weeks\n"), "{limit}: {whole}");
        assert!(
            whole.contains("--- log ended: closed by user ---"),
            "{whole}"
        );
    }
}

#[tokio::test]
async fn a_log_read_that_starts_anywhere_inside_a_key_shows_none_of_it() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = lab_with_a_key(builder_at(dir.path())).await;
    let name = a_log_with_a_key(&lab).await;
    let on_disk = std::fs::read(dir.path().join("sessions").join(&name)).expect("log");
    let find = |needle: &str| {
        on_disk
            .windows(needle.len())
            .position(|window| window == needle.as_bytes())
            .expect("the line is in the file")
    };

    // A client is free to ask for any offset. Into the timestamp of a line
    // of the key, into the line itself, and exactly at its start; near the
    // first marker, in the middle, and next to the last one.
    for line in [KEY_LINES[0], KEY_LINES[5], KEY_LINES[11]] {
        let at = find(line);
        for offset in [at - 31, at - 12, at, at + 7] {
            for limit in [None, Some(150u64)] {
                let read = lab
                    .app
                    .rpc(
                        "terminal.logs.read",
                        json!({"name": name, "offset": offset, "limit": limit}),
                    )
                    .await
                    .expect("read");
                let text = read["text"].as_str().expect("text");
                assert_eq!(
                    shown(text),
                    Vec::<&str>::new(),
                    "a read at {offset} (limit {limit:?}) showed the key:\n{text}"
                );
                // Not even the piece of a line the offset fell into.
                for line in KEY_LINES {
                    assert!(
                        !text.contains(&line[20..36]),
                        "a read at {offset} (limit {limit:?}) showed part of the key:\n{text}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_secret_on_one_line_is_hidden_however_a_log_is_paged() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = testing::lab(builder_at(dir.path()), &["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1").with(|device| {
        device.outputs.insert(
            "show secret".into(),
            "enable secret Sup3rS3cret\nsnmp-server community c0mmunity-RO RO\n".into(),
        );
    });
    let mut events = Events::new(app);
    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("SW1"), "cols": 100, "rows": 30, "log": true}),
        )
        .await
        .expect("open");
    let id = session_id(&info);
    let name = info["logging"].as_str().expect("a log name").to_string();
    events.wait_for(&id, "SW1>").await;
    type_line(&lab, &mut events, &id, "show secret", "RO RO\r\nSW1>").await;
    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
    let on_disk = std::fs::read_to_string(dir.path().join("sessions").join(&name)).expect("log");
    let size = on_disk.len() as u64;
    assert!(on_disk.contains("Sup3rS3cret") && on_disk.contains("c0mmunity-RO"));

    // Pages far smaller than a line: each is still a whole line, so the rule
    // that knows `enable secret` gets to see it.
    for limit in [1u64, 10, 37, 64] {
        let pages = pages(&lab, &name, limit).await;
        let whole: String = pages
            .iter()
            .map(|page| page["text"].as_str().expect("text"))
            .collect();
        for secret in ["Sup3rS3cret", "c0mmunity-RO", "Sup3r", "S3cret", "0mmunity"] {
            assert!(
                !whole.contains(secret),
                "pages of {limit} bytes showed {secret}:\n{whole}"
            );
        }
        // One line to a page, each where the last one ended.
        assert_eq!(pages.len(), on_disk.lines().count(), "{limit}");
        assert!(whole.contains("enable secret <secret:"), "{whole}");
        assert_eq!(whole.lines().count(), on_disk.lines().count());
    }

    // And a read from any byte of the file, into the secret itself included.
    for offset in 0..size {
        let read = app
            .rpc(
                "terminal.logs.read",
                json!({"name": name, "offset": offset, "limit": 16}),
            )
            .await
            .expect("read");
        let text = read["text"].as_str().expect("text");
        for secret in ["Sup3r", "S3cret", "0mmunity"] {
            assert!(
                !text.contains(secret),
                "a read at {offset} showed {secret}: {text}"
            );
        }
        // The page says where it really starts: at the line the offset is in.
        let start = read["offset"].as_u64().expect("offset");
        assert!(start <= offset, "{read}");
        assert!(
            start == 0 || on_disk.as_bytes()[start as usize - 1] == b'\n',
            "{read}"
        );
        assert!(read["next_offset"].as_u64().expect("next") > offset);
    }
}

#[tokio::test]
async fn a_snapshot_whose_window_lies_inside_a_key_shows_none_of_it() {
    let lab = lab_with_a_key(common::builder()).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(
        &common::open_device(app, &lab.id("SW1"))
            .await
            .expect("open"),
    );
    events.wait_for(&id, "SW1>").await;

    // The key has been printed whole and the prompt is back: the last lines
    // are the end of the key, its last marker and the prompt.
    type_line(&lab, &mut events, &id, "show key", END).await;
    events.wait_for(&id, &format!("{END}\r\nSW1>")).await;
    for lines in [1, 2, 3, 5, 8, 13, 14, 15, 200] {
        let snapshot = app
            .rpc(
                "terminal.snapshot",
                json!({"session_id": id, "lines": lines}),
            )
            .await
            .expect("snapshot");
        let text = snapshot["text"].as_str().expect("text");
        assert_eq!(shown(text), Vec::<&str>::new(), "{lines} lines:\n{text}");
    }

    // The device stopped in the middle of a key — a pager, an interrupted
    // command — and the window is the last few lines: no marker is in it.
    type_line(&lab, &mut events, &id, "show key head", KEY_LINES[5]).await;
    events
        .wait_for(&id, &format!("{}\r\nSW1>", KEY_LINES[5]))
        .await;
    for lines in [1, 2, 3, 4, 6, 7, 8, 200] {
        let snapshot = app
            .rpc(
                "terminal.snapshot",
                json!({"session_id": id, "lines": lines}),
            )
            .await
            .expect("snapshot");
        let text = snapshot["text"].as_str().expect("text");
        assert_eq!(shown(text), Vec::<&str>::new(), "{lines} lines:\n{text}");
        assert_eq!(snapshot["redacted"], lines > 1, "{lines} lines:\n{text}");
        assert!(text.ends_with("SW1>"), "{text}");
    }

    // What follows the cut-off key is not taken for more of it.
    type_line(&lab, &mut events, &id, "show uptime", "up 3 weeks\r\nSW1>").await;
    let snapshot = app
        .rpc("terminal.snapshot", json!({"session_id": id, "lines": 3}))
        .await
        .expect("snapshot");
    assert_eq!(
        snapshot["text"], "SW1>show uptime\nup 3 weeks\nSW1>",
        "{snapshot}"
    );
    assert_eq!(snapshot["redacted"], false);
}

// ------------------------------------------- every place a window can be cut

/// A piece of the pretend key that `text` shows, if it shows any: the start,
/// the middle or the end of one of its lines.
fn part_shown(text: &str) -> Option<&'static str> {
    KEY_LINES.iter().copied().find_map(|line| {
        let middle = line.len() / 2;
        [
            &line[..12],
            &line[middle - 6..middle + 6],
            &line[line.len() - 12..],
        ]
        .into_iter()
        .find(|part| text.contains(part))
    })
}

/// What a redacted read of `raw` is to say: the same lines, with every line
/// of the key and its last marker hidden behind the timestamp it was logged
/// under (`stamp` characters; none in a snapshot).
fn hidden(raw: &str, stamp: usize) -> String {
    raw.split('\n')
        .map(|line| {
            let body = line.get(stamp..).unwrap_or_default();
            if KEY_LINES.contains(&body) || body == END {
                format!("{}<hidden>", &line[..stamp])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn a_log_read_from_any_byte_to_any_byte_hides_all_of_a_key() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = lab_with_a_key(builder_at(dir.path())).await;
    let name = a_log_with_a_key(&lab).await;
    let on_disk = std::fs::read_to_string(dir.path().join("sessions").join(&name)).expect("log");
    let size = on_disk.len() as u64;
    // The writer's timestamp and the space after it.
    let stamp = on_disk.find(" ---").expect("the first line") + 1;
    let begin = on_disk.find(BEGIN).expect("the first marker") as u64;
    let end = on_disk.find(END).expect("the last marker") as u64;

    let read = |offset: u64, limit: Option<u64>| {
        let (app, name) = (lab.app.clone(), name.clone());
        async move {
            app.rpc(
                "terminal.logs.read",
                json!({"name": name, "offset": offset, "limit": limit}),
            )
            .await
            .expect("read")
        }
    };
    // A page is the lines of the file from its `offset` to its `next_offset`,
    // and says of each exactly what a read of the whole log says of it.
    let check = |page: &Value, offset: u64, limit: Option<u64>| {
        let what = format!("the read at {offset} with limit {limit:?}");
        let text = page["text"].as_str().expect("text");
        assert_eq!(part_shown(text), None, "{what} shows the key:\n{text}");
        let start = page["offset"].as_u64().expect("offset") as usize;
        let next = page["next_offset"].as_u64().expect("next_offset") as usize;
        assert!(start as u64 <= offset.min(size), "{what}: {page}");
        assert!(
            start == 0 || on_disk.as_bytes()[start - 1] == b'\n',
            "{what} starts inside a line: {page}"
        );
        assert!(
            next as u64 == size || on_disk.as_bytes()[next - 1] == b'\n',
            "{what} ends inside a line: {page}"
        );
        assert!(
            next as u64 > offset || offset >= size,
            "{what} went nowhere"
        );
        assert_eq!(text, hidden(&on_disk[start..next], stamp), "{what}");
        assert_eq!(
            page["redacted"],
            text != &on_disk[start..next],
            "{what}: {page}"
        );
    };

    // The start of the page on every byte of the file — in a timestamp, in a
    // marker, in the key — one line at a time.
    for offset in 0..=size {
        check(&read(offset, Some(1)).await, offset, Some(1));
    }
    // Around both markers, one byte at a time: a page that starts there and
    // runs on, and a page that ends there, from the top of the file and from
    // the middle of the key.
    let middle = on_disk.find(KEY_LINES[5]).expect("the sixth line") as u64;
    let near = |marker: u64, length: usize| marker - 45..marker + length as u64 + 45;
    for at in near(begin, BEGIN.len()).chain(near(end, END.len())) {
        check(&read(at, None).await, at, None);
        check(&read(at, Some(200)).await, at, Some(200));
        check(&read(0, Some(at)).await, 0, Some(at));
        if at > middle {
            check(
                &read(middle, Some(at - middle)).await,
                middle,
                Some(at - middle),
            );
        }
    }
    // The page cut inside the key and the page after it are the log between them.
    let first = read(0, Some(middle)).await;
    let rest = read(first["next_offset"].as_u64().expect("next"), None).await;
    assert_eq!(
        format!(
            "{}{}",
            first["text"].as_str().expect("text"),
            rest["text"].as_str().expect("text")
        ),
        hidden(&on_disk, stamp)
    );
}

#[tokio::test]
async fn a_snapshot_of_any_length_hides_all_of_a_key() {
    let lab = lab_with_a_key(common::builder()).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(
        &common::open_device(app, &lab.id("SW1"))
            .await
            .expect("open"),
    );
    events.wait_for(&id, "SW1>").await;

    // A redacted snapshot of the last `lines` is the unredacted one with the
    // key's lines hidden and nothing else changed, however many lines it is:
    // one that ends the key, one that lies inside it, one that holds it all.
    let compare = |lines: usize| {
        let (app, id) = (app.clone(), id.clone());
        async move {
            let shown = app
                .rpc(
                    "terminal.snapshot",
                    json!({"session_id": id, "lines": lines}),
                )
                .await
                .expect("snapshot");
            let raw = app
                .rpc(
                    "terminal.snapshot",
                    json!({"session_id": id, "lines": lines, "redact": false}),
                )
                .await
                .expect("snapshot");
            let (text, raw) = (
                shown["text"].as_str().expect("text"),
                raw["text"].as_str().expect("text"),
            );
            assert_eq!(part_shown(text), None, "{lines} lines:\n{text}");
            assert_eq!(text, hidden(raw, 0), "{lines} lines");
            assert_eq!(shown["redacted"], text != raw, "{lines} lines");
            assert_eq!(shown["lines"], raw.split('\n').count(), "{lines} lines");
        }
    };

    // The whole key, and the prompt after it.
    type_line(&lab, &mut events, &id, "show key", END).await;
    events.wait_for(&id, &format!("{END}\r\nSW1>")).await;
    for lines in 1..=24 {
        compare(lines).await;
    }
    // Then one the device stopped in the middle of, with no last marker.
    type_line(&lab, &mut events, &id, "show key head", KEY_LINES[5]).await;
    events
        .wait_for(&id, &format!("{}\r\nSW1>", KEY_LINES[5]))
        .await;
    for lines in 1..=36 {
        compare(lines).await;
    }
    // And with the session gone on after it.
    type_line(&lab, &mut events, &id, "show uptime", "up 3 weeks\r\nSW1>").await;
    for lines in 1..=40 {
        compare(lines).await;
    }
}

#[tokio::test]
async fn a_line_that_begins_as_a_prompt_does_keeps_no_secret_in_a_snapshot_or_a_log() {
    // What `cat /etc/shadow` prints: a name, a colon, and a hash whose first
    // character is the `$` a shell prompt ends in. Synthetic: the salt and
    // the hash are letters typed for this test.
    let shadow = "root:$6$saltSALTsalt$hashHASHhashHASHhashHASHhashHASHhashHASH:19000:0:99999:7:::";
    let dir = tempfile::tempdir().expect("a directory");
    let lab = testing::lab(builder_at(dir.path()), &["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1").with(|device| {
        device
            .outputs
            .insert("show shadow".into(), format!("{shadow}\n"));
    });
    let mut events = Events::new(app);
    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("SW1"), "cols": 100, "rows": 30, "log": true}),
        )
        .await
        .expect("open");
    let id = session_id(&info);
    let name = info["logging"].as_str().expect("a log name").to_string();
    events.wait_for(&id, "SW1>").await;
    type_line(&lab, &mut events, &id, "show shadow", ":::\r\nSW1>").await;

    let snapshot = app
        .rpc("terminal.snapshot", json!({"session_id": id, "lines": 3}))
        .await
        .expect("snapshot");
    let text = snapshot["text"].as_str().expect("text");
    assert!(
        !text.contains("saltSALT") && !text.contains("hashHASH"),
        "{text}"
    );
    assert!(text.starts_with("SW1>show shadow\nroot:<secret:"), "{text}");
    assert!(text.ends_with("\nSW1>"), "{text}");
    assert_eq!(snapshot["redacted"], true);

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
    let on_disk = std::fs::read_to_string(dir.path().join("sessions").join(&name)).expect("log");
    assert!(on_disk.contains(shadow), "{on_disk}");
    for limit in [None, Some(1u64)] {
        let whole: String = match limit {
            Some(limit) => pages(&lab, &name, limit).await,
            None => vec![
                app.rpc("terminal.logs.read", json!({"name": name}))
                    .await
                    .expect("read"),
            ],
        }
        .iter()
        .map(|page| page["text"].as_str().expect("text"))
        .collect();
        assert!(
            !whole.contains("saltSALT") && !whole.contains("hashHASH"),
            "{limit:?}: {whole}"
        );
        assert!(whole.contains(" root:<secret:"), "{limit:?}: {whole}");
        assert_eq!(whole.lines().count(), on_disk.lines().count(), "{limit:?}");
    }
}

#[tokio::test]
async fn a_read_that_starts_far_into_a_long_line_still_starts_with_that_line() {
    // Written by hand (synthetic): lines of a length the log writer makes
    // only of output that never ends a line. It lets 64 KiB go by before it
    // breaks one itself, and bytes that are not text take three times their
    // number in the file. The second is nearly as long as a page can be.
    let dir = tempfile::tempdir().expect("a directory");
    let lab = testing::lab(builder_at(dir.path()), &["SW1"]).await;
    let month = dir.path().join("sessions").join("2026-11");
    std::fs::create_dir_all(&month).expect("the month's directory");
    let stamp = "2026-11-13T05:24:05.123+02:00 ";

    for (filler, limits) in [
        (300_000usize, vec![None, Some(16u64)]),
        (1_000_000, vec![Some(16)]),
    ] {
        let log = format!(
            "{stamp}SW1#show clock\n{stamp}{} enable secret Sup3rS3cret\n{stamp}SW1#\n",
            "x".repeat(filler)
        );
        let file = format!("long{filler}-20261113-052405.log");
        std::fs::write(month.join(&file), &log).expect("a log");
        let name = format!("2026-11/{file}");
        let line_at = log.find('\n').expect("the first line") as u64 + 1;
        let secret_at = log.find("Sup3rS3cret").expect("the secret") as u64;

        // Just before the secret, on its first character, inside it — where
        // half a line would be a word that no rule knows — and far from it.
        for offset in [
            secret_at - 20,
            secret_at,
            secret_at + 5,
            secret_at + 10,
            line_at + filler as u64 / 2,
        ] {
            for limit in &limits {
                let what = format!("a read at {offset} of {filler} (limit {limit:?})");
                let read = lab
                    .app
                    .rpc(
                        "terminal.logs.read",
                        json!({"name": name, "offset": offset, "limit": limit}),
                    )
                    .await
                    .expect("read");
                let text = read["text"].as_str().expect("text");
                for part in ["Sup3r", "S3cret", "3cret", "cret\n"] {
                    assert!(
                        !text.contains(part),
                        "{what} shows {part}: …{}",
                        &text[text.len().saturating_sub(60)..]
                    );
                }
                assert_eq!(read["offset"], line_at, "{what}");
                assert!(text.contains(" enable secret <secret:1>\n"), "{what}");
                assert_eq!(read["redacted"], true, "{what}");
            }
        }
    }
}

#[tokio::test]
async fn a_line_too_long_for_one_page_is_read_in_pieces_that_end_and_split_no_character() {
    // Written by hand (synthetic): one line of more than two pages, of
    // characters three and four bytes long, placed so that the edge of a
    // page falls one byte into a character in the first log and three bytes
    // into one in the second.
    let dir = tempfile::tempdir().expect("a directory");
    let lab = testing::lab(builder_at(dir.path()), &["SW1"]).await;
    let month = dir.path().join("sessions").join("2026-11");
    std::fs::create_dir_all(&month).expect("the month's directory");
    let stamp = "2026-11-13T05:24:05.123+02:00 ";
    let page = 1024 * 1024;

    for (file, line, cut_into) in [
        ("snow", "\u{2603}".repeat(800_000), 1),
        (
            "clef",
            format!(
                "{}{}",
                "x".repeat(page - 3 - stamp.len()),
                "\u{1d11e}".repeat(400_000)
            ),
            3,
        ),
    ] {
        let log = format!("{stamp}{line}\n{stamp}SW1#\n");
        assert!(!log.is_char_boundary(page) && log.is_char_boundary(page - cut_into));
        let file = format!("{file}-20261113-052405.log");
        std::fs::write(month.join(&file), &log).expect("a log");
        let name = format!("2026-11/{file}");

        // Read as a client pages: each page from where the last one ended.
        // The pages must get somewhere, come to an end, and be the file
        // between them.
        let pages = pages(&lab, &name, 262_144).await;
        assert!(pages.len() <= 5, "{name}: {} pages", pages.len());
        let whole: String = pages
            .iter()
            .map(|page| page["text"].as_str().expect("text"))
            .collect();
        assert!(
            !whole.contains('\u{fffd}'),
            "{name}: a character was cut in two"
        );
        assert!(whole == log, "{name}: the pages are not the log");
        // The first piece is a page less the bytes of the character it would
        // have split, and the next starts where it ended: there is no line
        // start within reach to go back to.
        assert_eq!(pages[0]["next_offset"], (page - cut_into) as u64, "{name}");
        assert_eq!(pages[1]["offset"], pages[0]["next_offset"], "{name}");
    }
}
