//! What a scan leaves behind: readable while it runs, and never stuck.

use std::time::Duration;

use nocturne_app::{AppBuilder, AppConfig};
use serde_json::json;

use super::support::{self, FakeHost};
use crate::db;
use crate::model::Scan;

#[tokio::test]
async fn a_running_scan_can_be_read_while_it_runs() {
    let (lab, probes) = support::lab(&[]).await;
    for last in 1..=12u8 {
        probes.host(&format!("10.0.0.{last}"), FakeHost::alive());
    }
    probes.delay(Duration::from_millis(30));

    let started = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "10.0.0.1-10.0.0.12", "ports": "", "concurrency": 1}),
        )
        .await
        .unwrap();
    let (scan_id, job_id) = (
        started["scan_id"].as_str().unwrap().to_string(),
        started["job_id"].as_str().unwrap().to_string(),
    );
    tokio::time::sleep(Duration::from_millis(120)).await;

    let midway = lab
        .app
        .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
        .await
        .unwrap();
    assert_eq!(midway["scan"]["state"], "running");
    let seen = midway["hosts"].as_array().unwrap().len();
    assert!(
        seen > 0 && seen < 12,
        "hosts appear as they answer, not only at the end: {seen}"
    );
    assert!(
        lab.app
            .rpc("discovery.scans", json!({}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|scan| scan["id"] == scan_id.as_str() && scan["state"] == "running")
    );

    let job = nocturne_app::jobs::wait(&lab.app, &job_id).await.unwrap();
    assert_eq!(job.state, nocturne_app::jobs::JobState::Succeeded);
    let hosts = support::hosts(&lab.app, &scan_id).await;
    assert_eq!(hosts.len(), 12);
}

#[tokio::test]
async fn deleting_a_running_scan_stops_it_first() {
    let (lab, probes) = support::lab(&[]).await;
    for last in 1..=30u8 {
        probes.host(&format!("10.0.0.{last}"), FakeHost::alive());
    }
    probes.delay(Duration::from_millis(30));

    let started = lab
        .app
        .rpc(
            "discovery.scan",
            json!({"targets": "10.0.0.1-10.0.0.30", "ports": "", "concurrency": 1}),
        )
        .await
        .unwrap();
    let (scan_id, job_id) = (
        started["scan_id"].as_str().unwrap().to_string(),
        started["job_id"].as_str().unwrap().to_string(),
    );
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(
        lab.app
            .rpc("discovery.scan_delete", json!({"scan_ids": [scan_id]}))
            .await
            .unwrap()["deleted"],
        1
    );

    let job = nocturne_app::jobs::wait(&lab.app, &job_id).await.unwrap();
    assert_eq!(job.state, nocturne_app::jobs::JobState::Cancelled);
    let sent = probes.sent_of("echo").len();
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert_eq!(probes.sent_of("echo").len(), sent, "it stopped sending");
    assert_eq!(
        lab.app
            .rpc("discovery.scan_get", json!({"scan_id": scan_id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn a_scan_interrupted_by_a_restart_is_not_left_running() {
    let dir = tempfile::tempdir().unwrap();
    let interrupted = Scan {
        id: "leftover".into(),
        kind: "sweep".into(),
        targets: "10.0.0.0/24".into(),
        state: "running".into(),
        started_at: 1,
        planned: 254,
        ..Scan::default()
    };
    {
        let builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
        builder
            .store()
            .put(db::SCANS, &interrupted.id, &interrupted)
            .unwrap();
    }

    let mut builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
    nocturne_app::init(&mut builder);
    crate::init(&mut builder);
    let app = builder.build();

    let scan = app
        .rpc("discovery.scan_get", json!({"scan_id": "leftover"}))
        .await
        .unwrap();
    assert_eq!(scan["scan"]["state"], "failed");
    assert!(
        scan["scan"]["error"]
            .as_str()
            .unwrap()
            .contains("closed while this scan was running"),
        "{}",
        scan["scan"]["error"]
    );
    assert!(scan["scan"]["finished_at"].as_i64().unwrap() > 0);
    assert!(
        scan["hosts"].as_array().unwrap().is_empty(),
        "it recorded no finds, and none are invented for it"
    );
}

#[tokio::test]
async fn the_finds_of_a_big_scan_are_paged() {
    let (lab, probes) = support::lab(&[]).await;
    for last in 1..=30u8 {
        probes.host(&format!("10.0.0.{last}"), FakeHost::alive());
    }
    let (scan_id, _) = support::scan(
        &lab.app,
        json!({"targets": "10.0.0.1-10.0.0.30", "ports": "", "rate": 20_000}),
    )
    .await
    .unwrap();

    let page = lab
        .app
        .rpc(
            "discovery.scan_get",
            json!({"scan_id": scan_id, "limit": 10}),
        )
        .await
        .unwrap();
    assert_eq!(page["total"], 30, "the count is of everything that matched");
    assert_eq!(page["hosts"].as_array().unwrap().len(), 10);
    assert_eq!(page["hosts"][0]["address"], "10.0.0.1");

    let next = lab
        .app
        .rpc(
            "discovery.scan_get",
            json!({"scan_id": scan_id, "limit": 10, "cursor": "10"}),
        )
        .await
        .unwrap();
    assert_eq!(next["hosts"][0]["address"], "10.0.0.11");

    let last = lab
        .app
        .rpc(
            "discovery.scan_get",
            json!({"scan_id": scan_id, "limit": 10, "cursor": "25"}),
        )
        .await
        .unwrap();
    assert_eq!(
        last["hosts"].as_array().unwrap().len(),
        5,
        "a short page is the last one"
    );

    assert_eq!(
        lab.app
            .rpc(
                "discovery.scan_get",
                json!({"scan_id": scan_id, "cursor": "halfway"}),
            )
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}
