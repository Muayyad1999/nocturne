//! Searching, counting and retention.

mod support;

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

const T0: i64 = 1_763_000_000_000;

/// A quarter of an hour of a switch having a bad afternoon.
fn afternoon() -> Vec<(&'static str, &'static str, i64)> {
    vec![
        (
            "<187>45: *Nov 13 09:00:00.000: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down",
            "127.0.0.1",
            T0,
        ),
        (
            "<189>46: *Nov 13 09:00:01.000: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to down",
            "127.0.0.1",
            T0 + 1_000,
        ),
        (
            "<187>47: *Nov 13 09:01:00.000: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to up",
            "127.0.0.1",
            T0 + 60_000,
        ),
        (
            "<189>48: *Nov 13 09:02:00.000: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)",
            "127.0.0.1",
            T0 + 120_000,
        ),
        (
            "<26>Nov 13 09:03:00 MikroTik system,error,critical login failure for user admin from 10.0.0.5 via ssh",
            "127.0.0.2",
            T0 + 180_000,
        ),
    ]
}

async fn seeded() -> nocturne_app::testing::Lab {
    let lab = lab(&["SW1", "RB1"]).await;
    set_address(&lab.app, &lab.id("SW1"), "127.0.0.1").await;
    set_address(&lab.app, &lab.id("RB1"), "127.0.0.2").await;
    seed(&lab.app, &afternoon());
    lab
}

#[tokio::test]
async fn a_query_filters_by_device_severity_facility_mnemonic_and_words() {
    let lab = seeded().await;
    let app = &lab.app;

    let all = query(app, json!({"limit": 100})).await;
    assert_eq!(all.len(), 5);
    assert!(
        all[0]["ts"].as_i64() > all[1]["ts"].as_i64(),
        "newest first"
    );

    let one_device = query(app, json!({"device_id": lab.id("SW1")})).await;
    assert_eq!(one_device.len(), 4);

    let severe = query(app, json!({"max_severity": 3})).await;
    assert_eq!(
        severe.len(),
        3,
        "a severity floor keeps what is at least that bad: {severe:#?}"
    );

    let local7 = query(app, json!({"facility": 23})).await;
    assert_eq!(
        local7.len(),
        4,
        "the RouterOS line came in as daemon (3), not local7"
    );

    let links = query(app, json!({"mnemonic": "LINK-3-UPDOWN"})).await;
    assert_eq!(links.len(), 2);
    let any_link = query(app, json!({"mnemonic": "LIN*"})).await;
    assert_eq!(
        any_link.len(),
        3,
        "a trailing star matches a prefix: LINK and LINEPROTO"
    );

    let down = messages(app, json!({"text": "\"changed state to down\""})).await;
    assert_eq!(down.len(), 2, "a quoted phrase is one term: {down:#?}");
    let updown = query(app, json!({"text": "updown"})).await;
    assert_eq!(
        updown.len(),
        3,
        "a word is looked for in the vendor's mnemonic as well as in the message"
    );
    let not_up = query(app, json!({"text": "updown -\"state to up\""})).await;
    assert_eq!(
        not_up.len(),
        2,
        "a term with a leading minus excludes: {not_up:#?}"
    );
    let phrase = query(app, json!({"text": "\"login failure\""})).await;
    assert_eq!(phrase.len(), 1);

    let by_source = query(app, json!({"source": "127.0.0.2"})).await;
    assert_eq!(by_source.len(), 1);

    let window = query(app, json!({"since": T0 + 60_000, "until": T0 + 120_000})).await;
    assert_eq!(window.len(), 2, "the range includes both ends");

    let interface = query(app, json!({"interface": "GigabitEthernet1/0/3"})).await;
    assert!(
        interface.is_empty(),
        "nothing was collected from this device, so no line claims to be about a port it might not have"
    );
}

#[tokio::test]
async fn paging_backwards_walks_the_whole_list_without_repeating() {
    let lab = seeded().await;
    let app = &lab.app;

    let first = app.rpc("logs.query", json!({"limit": 2})).await.unwrap();
    assert_eq!(first["has_more"], json!(true));
    let entries = first["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let oldest = entries[1]["id"].as_i64().unwrap();

    // A line arriving now must not shift the page under the reader.
    seed(
        app,
        &[(
            "<187>49: *Nov 13 09:04:00.000: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/4, changed state to down",
            "127.0.0.1",
            T0 + 240_000,
        )],
    );
    let second = app
        .rpc("logs.query", json!({"limit": 2, "before_id": oldest}))
        .await
        .unwrap();
    let ids: Vec<i64> = second["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_i64().unwrap())
        .collect();
    assert!(
        ids.iter().all(|id| *id < oldest),
        "the next page is older than the page before it: {ids:?}"
    );
    assert_eq!(second["has_more"], json!(true));
    let last = app
        .rpc("logs.query", json!({"limit": 2, "before_id": ids[1]}))
        .await
        .unwrap();
    assert_eq!(last["entries"].as_array().unwrap().len(), 1);
    assert_eq!(last["has_more"], json!(false));
}

#[tokio::test]
async fn the_histogram_has_every_bucket_including_the_quiet_ones() {
    let lab = seeded().await;
    let app = &lab.app;

    let buckets = app
        .rpc(
            "logs.histogram",
            json!({"since": T0, "until": T0 + 300_000, "buckets": 5}),
        )
        .await
        .unwrap();
    let buckets = buckets.as_array().unwrap();
    assert_eq!(
        buckets.len(),
        5,
        "a histogram is as wide as it was asked to be"
    );
    assert_eq!(buckets[0]["t"], json!(T0));
    assert_eq!(buckets[0]["count"], json!(2), "{buckets:#?}");
    assert_eq!(buckets[1]["count"], json!(1));
    assert_eq!(buckets[2]["count"], json!(1));
    assert_eq!(buckets[3]["count"], json!(1));
    assert_eq!(
        buckets[4]["count"],
        json!(0),
        "the quiet minute is a zero, not a missing bucket"
    );
    // Severity 3 twice in the first minute: index 3 of the eight.
    assert_eq!(buckets[0]["by_severity"], json!([0, 0, 0, 1, 0, 1, 0, 0]));
    assert_eq!(buckets[0]["no_severity"], json!(0));

    let filtered = app
        .rpc(
            "logs.histogram",
            json!({"since": T0, "until": T0 + 300_000, "buckets": 5, "max_severity": 3}),
        )
        .await
        .unwrap();
    assert_eq!(
        filtered[0]["count"],
        json!(1),
        "the filter applies to the counts too"
    );

    let backwards = app
        .rpc("logs.histogram", json!({"since": T0, "until": T0}))
        .await
        .unwrap_err();
    assert_eq!(backwards.code, "invalid");
}

#[tokio::test]
async fn top_counts_by_device_mnemonic_severity_and_facility() {
    let lab = seeded().await;
    let app = &lab.app;

    let devices = app.rpc("logs.top", json!({"by": "device"})).await.unwrap();
    assert_eq!(devices[0]["key"], json!(lab.id("SW1")));
    assert_eq!(
        devices[0]["label"],
        json!("SW1"),
        "an id alone is unreadable"
    );
    assert_eq!(devices[0]["count"], json!(4));

    let mnemonics = app
        .rpc("logs.top", json!({"by": "mnemonic", "limit": 3}))
        .await
        .unwrap();
    assert_eq!(mnemonics[0]["key"], json!("LINK-3-UPDOWN"));
    assert_eq!(mnemonics[0]["count"], json!(2));

    let severities = app
        .rpc("logs.top", json!({"by": "severity"}))
        .await
        .unwrap();
    let error = severities
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == json!("3"))
        .expect("the error rows");
    assert_eq!(error["label"], json!("error"));

    let facilities = app
        .rpc("logs.top", json!({"by": "facility"}))
        .await
        .unwrap();
    assert_eq!(facilities[0]["key"], json!("23"));
    assert_eq!(facilities[0]["label"], json!("local7"));

    assert_eq!(
        app.rpc("logs.top", json!({"by": "phase of the moon"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn bursts_group_what_repeats_and_name_the_device() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let flapping: Vec<(&str, &str, i64)> = (0..40)
        .map(|index| {
            (
                "<187>45: *Nov 13 09:00:00.000: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down",
                "127.0.0.1",
                T0 + index * 1_000,
            )
        })
        .collect();
    seed(app, &flapping);
    seed(
        app,
        &[(
            "<189>48: *Nov 13 09:02:00.000: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)",
            "127.0.0.1",
            T0 + 120_000,
        )],
    );

    let bursts = app
        .rpc("logs.bursts", json!({"min_count": 5}))
        .await
        .unwrap();
    let bursts = bursts.as_array().unwrap();
    assert_eq!(
        bursts.len(),
        1,
        "only the run of forty qualifies: {bursts:#?}"
    );
    assert_eq!(bursts[0]["count"], json!(40));
    assert_eq!(bursts[0]["mnemonic"], json!("LINK-3-UPDOWN"));
    assert_eq!(bursts[0]["device"], json!("SW1"));
    assert_eq!(bursts[0]["first_at"], json!(T0));
    assert_eq!(bursts[0]["last_at"], json!(T0 + 39_000));
    assert!(
        bursts[0]["example"]
            .as_str()
            .unwrap()
            .contains("changed state to down")
    );
}

#[tokio::test]
async fn retention_enforces_age_then_the_row_cap() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let now = nocturne_app::util::now_ms();
    let day = 86_400_000;
    let lines: Vec<(&str, &str, i64)> = (0..10)
        .map(|index| {
            (
                "<189>1: *Nov 13 09:00:00.000: %SYS-6-LOGGINGHOST_STARTSTOP: Logging to host 10.0.0.1 started",
                "127.0.0.1",
                now - (index * 10) * day,
            )
        })
        .collect();
    seed(app, &lines);

    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"retention_days": 45, "max_rows": 1_000}}),
    )
    .await
    .unwrap();
    let pruned = nocturne_logs::ingest::prune_once(app).unwrap();
    assert_eq!(
        (pruned.by_age, pruned.by_rows),
        (5, 0),
        "everything older than 45 days went, nothing else"
    );
    assert_eq!(query(app, json!({"limit": 100})).await.len(), 5);

    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"retention_days": 0, "max_rows": 1_000}}),
    )
    .await
    .unwrap();
    assert_eq!(
        nocturne_logs::ingest::prune_once(app).unwrap().by_age,
        0,
        "retention_days 0 means age is not a reason to delete"
    );

    // The row cap is clamped to a thousand, so a thousand-and-five rows is
    // what it takes to see it bite.
    let many: Vec<(&str, &str, i64)> = (0..1_000)
        .map(|index| {
            (
                "<189>1: *Nov 13 09:00:00.000: %SYS-6-LOGGINGHOST_STARTSTOP: Logging to host 10.0.0.1 started",
                "127.0.0.1",
                now - index,
            )
        })
        .collect();
    seed(app, &many);
    let pruned = nocturne_logs::ingest::prune_once(app).unwrap();
    assert_eq!(pruned.by_rows, 5, "down to the cap, oldest first");

    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["store"]["rows"], json!(1_000));
    assert_eq!(status["store"]["pruned_by_age"], json!(5));
    assert_eq!(status["store"]["pruned_by_rows"], json!(5));
    assert!(status["store"]["last_prune_at"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn clearing_a_stream_takes_its_index_rows_with_it() {
    let lab = seeded().await;
    let app = &lab.app;

    let removed = app
        .rpc("logs.clear", json!({"stream": "syslog"}))
        .await
        .unwrap();
    assert_eq!(removed["removed"], json!(5));
    assert!(query(app, json!({"limit": 10})).await.is_empty());
    let orphans: i64 = app
        .store()
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM logs_syslog WHERE log_id NOT IN (SELECT id FROM logs)",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(orphans, 0);
    assert_eq!(
        app.rpc("logs.clear", json!({"stream": "*"}))
            .await
            .unwrap_err()
            .code,
        "invalid",
        "clearing everything at once is not something to do by accident"
    );
    // The audit trail remembers it, and is a stream of its own.
    let audit = query(app, json!({"stream": "audit", "text": "log entries"})).await;
    assert_eq!(audit.len(), 1, "{audit:#?}");
}

#[tokio::test]
async fn the_audit_trail_cannot_be_cleared_through_the_api_it_records() {
    let lab = seeded().await;
    let app = &lab.app;
    // Something to lose: clearing a stream is itself audited.
    app.rpc("logs.clear", json!({"stream": "syslog"}))
        .await
        .unwrap();
    let before = query(app, json!({"stream": "audit", "limit": 500})).await;
    assert!(!before.is_empty());

    for stream in ["audit", "AUDIT"] {
        let refused = app
            .rpc("logs.clear", json!({"stream": stream}))
            .await
            .unwrap_err();
        assert_eq!(refused.code, "forbidden", "{refused}");
        assert!(refused.message.contains("audit trail"), "{refused}");
    }
    let refused = app
        .rpc("logs.clear", json!({"stream": "audit", "before": 1}))
        .await
        .unwrap_err();
    assert_eq!(
        refused.code, "forbidden",
        "not even the part before some moment"
    );

    let after = query(app, json!({"stream": "audit", "limit": 500})).await;
    assert_eq!(
        after, before,
        "nothing went, and the refusals left no trace"
    );
}

#[tokio::test]
async fn an_audit_entry_goes_when_it_is_older_than_the_retention_and_not_before() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let now = nocturne_app::util::now_ms();
    let day = 86_400_000;
    let audit = |ts: i64, message: &str| nocturne_app::store::LogEntry {
        id: 0,
        ts,
        stream: "audit".into(),
        device_id: None,
        severity: Some(6),
        source: "test.action".into(),
        message: message.into(),
        body: None,
    };
    app.store()
        .log_append(&audit(now - 40 * day, "forty days ago"))
        .unwrap();
    app.store()
        .log_append(&audit(now - 5 * day, "five days ago"))
        .unwrap();
    // A flood of device lines must not be able to push the audit trail
    // out: the row cap counts syslog rows only.
    let many: Vec<(&str, &str, i64)> = (0..1_005)
        .map(|index| {
            (
                "<189>1: *Nov 13 09:00:00.000: %SYS-6-LOGGINGHOST_STARTSTOP: Logging to host 10.0.0.1 started",
                "127.0.0.1",
                now - index,
            )
        })
        .collect();
    seed(app, &many);

    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"retention_days": 30, "max_rows": 1_000}}),
    )
    .await
    .unwrap();
    let pruned = nocturne_logs::ingest::prune_once(app).unwrap();
    assert_eq!((pruned.by_age, pruned.by_rows), (1, 5));

    let kept: Vec<String> = messages(app, json!({"stream": "audit", "text": "days ago"})).await;
    assert_eq!(kept, ["five days ago"]);
}
