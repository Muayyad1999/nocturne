//! The history: what makes the toolbox useful during an incident rather than
//! merely convenient.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::support;
use crate::probe::history::{self, Run};

/// Put a finished run straight into the store, so that the list, the filters
/// and the paging can be tested without running fifty probes.
fn record(app: &nocturne_app::App, kind: &str, target: &str, ok: bool, at: i64) -> String {
    let run = Run {
        id: nocturne_app::util::new_id(),
        kind: kind.to_string(),
        title: format!("{kind} {target}"),
        target: target.to_string(),
        from: "this computer".into(),
        started_at: at,
        finished_at: Some(at + 1),
        ok: Some(ok),
        summary: if ok {
            "answered".into()
        } else {
            "no answer".into()
        },
        error: (!ok).then(|| "timed out waiting for a reply".to_string()),
        error_code: (!ok).then(|| "timeout".to_string()),
        result: Some(json!({"detail": "the whole report"})),
        ..Run::default()
    };
    app.store()
        .put(history::COLLECTION, &run.id, &run)
        .expect("the record is stored");
    run.id
}

#[tokio::test]
async fn the_history_lists_newest_first_filters_and_pages() {
    let lab = support::lab(&[]).await;
    let ping = record(&lab.app, "ping", "10.0.0.1", true, 1_000);
    record(&lab.app, "ping", "10.0.0.2", false, 2_000);
    let newest = record(&lab.app, "dns", "sw1.lab.example", true, 3_000);

    let all = lab.app.rpc("probe.history", Value::Null).await.unwrap();
    assert_eq!(all["total"], 3);
    assert_eq!(all["runs"][0]["id"], newest);
    assert_eq!(all["runs"][2]["id"], ping);
    // A list does not carry every echo of every ping.
    assert_eq!(all["runs"][0]["result"], Value::Null);
    assert_eq!(all["cursor"], Value::Null);

    let pings = lab
        .app
        .rpc("probe.history", json!({"kind": "ping"}))
        .await
        .unwrap();
    assert_eq!(pings["total"], 2);
    assert!(
        pings["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["kind"] == "ping"),
    );
    let several = lab
        .app
        .rpc("probe.history", json!({"kind": "ping,dns"}))
        .await
        .unwrap();
    assert_eq!(several["total"], 3);

    let searched = lab
        .app
        .rpc("probe.history", json!({"query": "10.0.0.2"}))
        .await
        .unwrap();
    assert_eq!(searched["total"], 1);
    assert_eq!(searched["runs"][0]["ok"], false);
    assert_eq!(searched["runs"][0]["error_code"], "timeout");

    // Paging, because an incident can mean a great many probes.
    let page = lab
        .app
        .rpc("probe.history", json!({"limit": 2}))
        .await
        .unwrap();
    assert_eq!(page["runs"].as_array().unwrap().len(), 2);
    assert_eq!(page["cursor"], "2");
    let rest = lab
        .app
        .rpc("probe.history", json!({"limit": 2, "cursor": "2"}))
        .await
        .unwrap();
    assert_eq!(rest["runs"].as_array().unwrap().len(), 1);
    assert_eq!(rest["cursor"], Value::Null);

    // One run comes back whole, with its result.
    let one = lab
        .app
        .rpc("probe.history.get", json!({"id": ping}))
        .await
        .unwrap();
    assert_eq!(one["result"]["detail"], "the whole report");
    assert_eq!(
        lab.app
            .rpc("probe.history.get", json!({"id": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );

    // And can be forgotten.
    let deleted = lab
        .app
        .rpc("probe.history.delete", json!({"ids": [ping]}))
        .await
        .unwrap();
    assert_eq!(deleted["removed"], 1);
    assert_eq!(
        lab.app.rpc("probe.history", Value::Null).await.unwrap()["total"],
        2
    );
    let cleared = lab
        .app
        .rpc("probe.history.delete", json!({"all": true}))
        .await
        .unwrap();
    assert_eq!(cleared["removed"], 2);
    // Forgetting records is a mutation, so it is in the trail.
    let audit = lab
        .app
        .store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(
        audit
            .iter()
            .any(|entry| entry.source == "probe.history.delete"
                && entry.message.contains("2 probe runs")),
        "{:?}",
        audit.iter().map(|entry| &entry.source).collect::<Vec<_>>()
    );
    assert_eq!(
        lab.app.rpc("probe.history", Value::Null).await.unwrap()["total"],
        0
    );
}

#[tokio::test]
async fn only_the_newest_runs_are_kept() {
    let lab = support::lab(&[]).await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "toolbox", "value": {"keep_runs": 3}}),
        )
        .await
        .unwrap();
    for index in 0..5 {
        record(
            &lab.app,
            "dns",
            &format!("host{index}"),
            true,
            1_000 + index,
        );
    }
    // Pruning happens when a run finishes, so run one.
    let port = support::dns_responder([10, 0, 0, 9]).await;
    lab.app
        .rpc(
            "probe.dns",
            json!({"name": "host9.lab.example", "server": format!("127.0.0.1:{port}")}),
        )
        .await
        .unwrap();

    let kept = lab.app.rpc("probe.history", Value::Null).await.unwrap();
    assert_eq!(kept["total"], 3, "{kept}");
    // The newest survive: the one that just ran, and the last two records.
    assert_eq!(kept["runs"][0]["target"], "host9.lab.example");
    assert_eq!(kept["runs"][1]["target"], "host4");
}

#[tokio::test]
async fn a_run_interrupted_by_the_process_ending_says_so_instead_of_running_for_ever() {
    let dir = tempfile::tempdir().unwrap();
    {
        let app = support::on_disk(dir.path());
        let run = Run {
            id: "interrupted".into(),
            kind: "ping".into(),
            title: "Ping 10.0.0.1".into(),
            target: "10.0.0.1".into(),
            from: "this computer".into(),
            started_at: 1_000,
            ..Run::default()
        };
        app.store().put(history::COLLECTION, &run.id, &run).unwrap();
        assert_eq!(
            app.rpc("probe.history", Value::Null).await.unwrap()["runs"][0]["ok"],
            Value::Null,
            "while it runs, `ok` is unknown"
        );
    }
    // A new process over the same data directory closes it.
    let app = support::on_disk(dir.path());
    let run = app
        .rpc("probe.history.get", json!({"id": "interrupted"}))
        .await
        .unwrap();
    assert_eq!(run["ok"], false);
    assert_eq!(run["error_code"], "cancelled");
    assert!(
        run["error"].as_str().unwrap().contains("closed"),
        "{}",
        run["error"]
    );
}
