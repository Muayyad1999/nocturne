//! Every report kind in every format, through `App::rpc`.
//!
//! What is being proved: the maths (availability over hand-made sample
//! series, including gaps), the escaping (a device whose description is
//! `<script>` must come out inert), and that a kind whose source is absent
//! produces a report that says so rather than a page of zeros.

mod common;

use common::{With, alert, backdate, backup, change, fixture, lab, reachability, resolved, rpc};
use nocturne_app::util::now_ms;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

const HOUR: i64 = 3_600_000;

/// The rows of a section of the JSON rendering, by heading.
fn rows(content: &str, heading: &str) -> Vec<Value> {
    let document: Value = serde_json::from_str(content).expect("the json format parses");
    document["sections"]
        .as_array()
        .expect("sections")
        .iter()
        .find(|section| section["heading"] == json!(heading))
        .unwrap_or_else(|| {
            panic!(
                "no section `{heading}`; there are {:?}",
                document["sections"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|section| section["heading"].clone())
                    .collect::<Vec<_>>()
            )
        })["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

async fn generate(app: &nocturne_app::App, kind: &str, format: &str, extra: Value) -> Value {
    let mut params = json!({"kind": kind, "format": format, "utc_offset_minutes": 0});
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().cloned().unwrap_or_default());
    rpc(app, "reports.generate", params).await
}

#[tokio::test]
async fn the_kinds_say_what_this_build_can_and_cannot_report_on() {
    let all = lab(&["SW1"], With::all()).await;
    let kinds = rpc(&all.app, "reports.kinds", json!({})).await;
    assert_eq!(kinds["kinds"].as_array().unwrap().len(), 7);
    assert!(
        kinds["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .all(|kind| kind["available"] == json!(true)),
        "{kinds}"
    );
    assert_eq!(kinds["formats"], json!(["markdown", "csv", "html", "json"]));

    let bare = lab(&["SW1"], With::nothing()).await;
    let kinds = rpc(&bare.app, "reports.kinds", json!({})).await;
    let unavailable: Vec<&str> = kinds["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|kind| kind["available"] == json!(false))
        .map(|kind| kind["id"].as_str().unwrap())
        .collect();
    assert_eq!(unavailable, ["changes", "compliance", "alerts"]);
    let changes = kinds["kinds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|kind| kind["id"] == json!("changes"))
        .unwrap();
    assert_eq!(changes["needs"], "changes.list");
    assert_eq!(
        changes["unavailable_reason"],
        "`changes.list` is not part of this build"
    );
    // The four kinds that need nothing but the core are still available.
    for kind in ["inventory", "software", "availability", "backups"] {
        assert!(
            kinds["kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|known| known["id"] == json!(kind) && known["available"] == json!(true)),
            "{kind} should not need another feature"
        );
    }
    assert_eq!(
        bare.app
            .rpc("reports.generate", json!({"kind": "nonsense"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn availability_is_measured_from_the_samples_and_refuses_to_guess() {
    let lab = lab(&["SW1", "SW2", "SW3", "SW4"], With::nothing()).await;
    let app = &lab.app;
    let now = now_ms();
    let since = now - 100 * 60_000;
    for name in ["SW1", "SW2", "SW3", "SW4"] {
        backdate(app, &lab.id(name), since - 60_000);
    }
    // SW1: 100 minutes observed, down for 10 of them.
    reachability(
        app,
        &lab.id("SW1"),
        since,
        &format!("{}{}{}", "1".repeat(45), "0".repeat(10), "1".repeat(45)),
    );
    // SW2: perfect.
    reachability(app, &lab.id("SW2"), since, &"1".repeat(100));
    // SW3: only 30 of the 100 minutes were observed at all.
    reachability(
        app,
        &lab.id("SW3"),
        since,
        &format!("{}{}", "1".repeat(30), ".".repeat(70)),
    );
    // SW4: never monitored.

    let report = generate(
        app,
        "availability",
        "json",
        json!({"since": since, "until": now}),
    )
    .await;
    let content = report["content"].as_str().unwrap();
    let measured = rows(content, "Measured devices");
    assert_eq!(measured.len(), 2, "{measured:#?}");
    let sw1 = &measured[0];
    assert_eq!(sw1["device"], "SW1");
    assert_eq!(sw1["availability_pct"], 90.0);
    assert_eq!(sw1["outages"], 1);
    assert_eq!(sw1["coverage_pct"], 100.0);
    assert_eq!(sw1["observations"], 100);
    assert_eq!(sw1["longest_outage"], 10 * 60_000);
    assert_eq!(measured[1]["availability_pct"], 100.0);

    // SW3 is listed separately and deliberately given no percentage.
    let thin = rows(content, "Too little data to measure");
    assert_eq!(thin.len(), 1);
    assert_eq!(thin[0]["device"], "SW3");
    assert_eq!(thin[0]["coverage_pct"], 30.0);
    assert!(
        thin[0]["why"]
            .as_str()
            .unwrap()
            .contains("30 of the 100 expected observations"),
        "{}",
        thin[0]
    );
    assert!(
        thin[0].get("availability_pct").is_none(),
        "no percentage for a device that was barely watched"
    );

    // SW4 is in its own list: nothing is known, which is not 0%.
    let document: Value = serde_json::from_str(content).unwrap();
    let never = document["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["heading"] == json!("Never monitored"))
        .expect("a never-monitored section");
    assert_eq!(never["items"], json!(["SW4"]));

    // A gap in the middle of an outage does not join two outages into one.
    let patchy = lab.id("SW2");
    let report = generate(
        app,
        "availability",
        "markdown",
        json!({"since": since, "until": now, "filter": {"query": "SW2"}}),
    )
    .await;
    assert!(report["content"].as_str().unwrap().contains("| SW2 |"));
    assert_eq!(report["rows"], 1);
    assert!(!patchy.is_empty());
}

#[tokio::test]
async fn hostile_text_comes_out_inert_in_every_format() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    // A device whose name and site are an attack, and an interface
    // description in a compliance finding.
    let nasty = rpc(
        app,
        "devices.create",
        json!({"name": "<script>alert(1)</script>", "address": "10.9.9.9",
               "site": "=HYPERLINK(\"http://evil\")", "role": "pipe|role"}),
    )
    .await;
    let id = nasty["id"].as_str().unwrap().to_string();
    *fixture(app).compliance.lock() = json!({
        "findings": [{"policy_id": "p1", "rule_id": "r1", "device_id": id, "status": "fail",
                      "detail": "<img src=x onerror=alert(1)> on \"Gi0/3\", comma"}],
        "summary": {"devices": 1, "rules": 1, "pass": 0, "fail": 1, "score_pct": 0.0},
        "run_at": now_ms(),
    });

    let html = generate(app, "inventory", "html", json!({})).await;
    let html = html["content"].as_str().unwrap();
    assert!(html.starts_with("<!doctype html>"));
    assert!(!html.contains("<script"), "a tag got through: {html}");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("@media print"), "it prints well");
    assert!(!html.contains("onerror="), "no event handler survives");

    let html = generate(app, "compliance", "html", json!({})).await;
    let html = html["content"].as_str().unwrap();
    assert!(!html.contains("<img src=x"), "{html}");
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));

    let markdown = generate(app, "inventory", "markdown", json!({})).await;
    let markdown = markdown["content"].as_str().unwrap();
    assert!(
        markdown.contains(r"\<script\>alert(1)\</script\>"),
        "{markdown}"
    );
    assert!(
        markdown.contains(r"pipe\|role"),
        "a pipe cannot add a table column: {markdown}"
    );
    // Every row of the markdown table has the same number of cells.
    let table: Vec<&str> = markdown
        .lines()
        .filter(|line| line.starts_with('|'))
        .collect();
    // A `\|` inside a cell is a literal pipe, not a column boundary.
    let cells = |line: &str| line.replace(r"\|", "").matches('|').count();
    let columns = cells(table[0]);
    assert!(
        table.iter().all(|line| cells(line) == columns),
        "a cell broke the table: {table:#?}"
    );

    let csv = generate(app, "inventory", "csv", json!({})).await;
    let csv = csv["content"].as_str().unwrap();
    assert!(
        csv.contains("\"'=HYPERLINK(\"\"http://evil\"\")\""),
        "a cell that looks like a formula is quoted out: {csv}"
    );
    assert!(csv.contains("<script>alert(1)</script>"), "{csv}");

    let value = generate(app, "inventory", "json", json!({})).await;
    let document: Value = serde_json::from_str(value["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        document["sections"][0]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["device"] == json!("<script>alert(1)</script>"))
            .count(),
        1,
        "json carries the value, not an escaping of it"
    );
}

#[tokio::test]
async fn the_inventory_and_software_reports_are_blank_where_nothing_was_collected() {
    let lab = lab(&["SW1", "SW2", "SW3", "SW4"], With::nothing()).await;
    let app = &lab.app;
    // Facts as a collector would leave them: two devices on one version of
    // the model, one on another, and one never collected at all.
    for (name, model, version) in [
        ("SW1", "C9300-24T", "17.9.4"),
        ("SW2", "C9300-24T", "17.9.4"),
        ("SW3", "C9300-24T", "16.12.1"),
    ] {
        nocturne_app::inventory::update_state(app, &lab.id(name), |state| {
            state.facts = Some(nocturne_drivers::model::Facts {
                hostname: name.into(),
                vendor: "Cisco".into(),
                model: model.into(),
                os: "IOS-XE".into(),
                os_version: version.into(),
                serial: format!("FOC{name}"),
                uptime_s: Some(7_200),
                ..nocturne_drivers::model::Facts::default()
            });
            state.facts_at = Some(now_ms());
        })
        .unwrap();
    }

    let report = generate(app, "inventory", "json", json!({})).await;
    let devices = rows(report["content"].as_str().unwrap(), "Devices");
    assert_eq!(devices.len(), 4);
    let uncollected = devices
        .iter()
        .find(|row| row["device"] == json!("SW4"))
        .unwrap();
    for blank in [
        "model",
        "serial",
        "os",
        "os_version",
        "uptime",
        "collected_at",
    ] {
        assert_eq!(
            uncollected[blank],
            Value::Null,
            "`{blank}` must be blank, not zero"
        );
    }
    let markdown = generate(app, "inventory", "markdown", json!({})).await;
    assert!(
        markdown["content"]
            .as_str()
            .unwrap()
            .contains("1 of 4 device(s) have never had their facts collected"),
        "the header counts them"
    );

    let software = generate(app, "software", "json", json!({})).await;
    let content = software["content"].as_str().unwrap();
    let odd = rows(content, "Devices out of step with their model");
    assert_eq!(odd.len(), 1);
    assert_eq!(odd[0]["device"], "SW3");
    assert_eq!(odd[0]["expected"], "17.9.4");
    let document: Value = serde_json::from_str(content).unwrap();
    assert_eq!(
        document["sections"][2]["items"],
        json!(["SW4"]),
        "a device with no version is listed as unknown, not as a version"
    );
}

#[tokio::test]
async fn the_changes_and_alerts_reports_count_only_what_is_in_the_period() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let now = now_ms();
    let since = now - 8 * HOUR;
    fixture(app).changes.lock().extend([
        change("c1", "Shut Gi0/3", "applied", &one, since, since + HOUR),
        change("c2", "Add VLAN 9", "failed", &two, since, since + 2 * HOUR),
        change(
            "old",
            "Last week",
            "applied",
            &one,
            since - 90 * HOUR,
            since - 80 * HOUR,
        ),
    ]);
    fixture(app).alerts.lock().extend([
        resolved(
            alert("a1", &one, "critical", since + HOUR),
            since + HOUR + 120_000,
            since + HOUR + 600_000,
        ),
        alert("a2", &one, "high", since + 2 * HOUR),
        resolved(
            alert("old", &two, "low", since - 90 * HOUR),
            since - 90 * HOUR,
            since - 89 * HOUR,
        ),
    ]);

    let report = generate(
        app,
        "changes",
        "json",
        json!({"since": since, "until": now}),
    )
    .await;
    let content = report["content"].as_str().unwrap();
    assert_eq!(rows(content, "Changes").len(), 2, "the old one is outside");
    let by_origin = rows(content, "By origin");
    assert_eq!(by_origin.len(), 1);
    assert_eq!(by_origin[0]["origin"], "user");
    assert_eq!(by_origin[0]["applied"], 1);
    assert_eq!(by_origin[0]["failed"], 1);
    assert_eq!(by_origin[0]["rolled_back"], 0);

    let report = generate(app, "alerts", "json", json!({"since": since, "until": now})).await;
    let content = report["content"].as_str().unwrap();
    let summary = rows(content, "Summary");
    let value_of = |what: &str| {
        summary
            .iter()
            .find(|row| row["what"] == json!(what))
            .unwrap_or_else(|| panic!("no row `{what}`"))["value"]
            .clone()
    };
    assert_eq!(value_of("alerts fired"), json!(2));
    assert_eq!(value_of("still open"), json!(1));
    // Two minutes to acknowledge, ten to resolve — over the one alert that
    // was each, not over all of them.
    assert_eq!(value_of("mean time to acknowledge"), json!(120_000));
    assert_eq!(value_of("mean time to resolve"), json!(600_000));
    assert_eq!(
        summary
            .iter()
            .find(|row| row["what"] == json!("mean time to resolve"))
            .unwrap()["of"],
        "1 resolved alert(s)"
    );
    let by_device = rows(content, "Devices that cost the most attention");
    assert_eq!(by_device[0]["subject"], "SW1");
    assert_eq!(by_device[0]["alerts"], 2);
    let by_severity = rows(content, "By severity");
    assert_eq!(by_severity.len(), 5, "every severity, zeros included");
    assert_eq!(by_severity[0]["severity"], "critical");
    assert_eq!(by_severity[0]["alerts"], 1);
    assert_eq!(by_severity[4]["alerts"], 0);
}

#[tokio::test]
async fn a_report_whose_source_is_missing_says_so_in_every_format() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    for kind in ["changes", "alerts", "compliance"] {
        for format in ["markdown", "html", "csv", "json"] {
            let report = generate(app, kind, format, json!({})).await;
            let content = report["content"].as_str().unwrap();
            assert_eq!(report["rows"], 0, "{kind}/{format} invented rows");
            let unavailable = report["unavailable"].as_array().unwrap();
            assert_eq!(unavailable.len(), 1, "{kind}/{format}: {unavailable:?}");
            assert_eq!(unavailable[0]["reason"], "not_installed");
            assert!(
                unavailable[0]["message"]
                    .as_str()
                    .unwrap()
                    .contains("is not part of this build"),
                "{kind}/{format}: {unavailable:?}"
            );
            assert!(
                content.to_lowercase().contains("not available")
                    || content.contains("\"kind\": \"unavailable\""),
                "{kind}/{format} does not say it is not available: {content}"
            );
        }
    }
}

#[tokio::test]
async fn the_backups_report_separates_coverage_from_movement() {
    let lab = lab(&["SW1", "SW2"], With::nothing()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    backup(app, &one, "hostname SW1\nvlan 10\n", "scheduled");
    backup(app, &one, "hostname SW1\nvlan 10\nvlan 20\n", "post-change");

    let report = generate(app, "backups", "json", json!({})).await;
    let content = report["content"].as_str().unwrap();
    let coverage = rows(content, "Coverage");
    assert_eq!(coverage[0]["count"], 1, "with a backup");
    assert_eq!(coverage[1]["count"], 1, "never backed up");
    assert_eq!(coverage[2]["count"], 2, "in scope");
    let newest = rows(content, "Newest backup per device");
    assert_eq!(newest.len(), 1);
    assert_eq!(newest[0]["device"], "SW1");
    assert_eq!(newest[0]["lines"], 3);
    assert_eq!(newest[0]["versions"], 2);
    let document: Value = serde_json::from_str(content).unwrap();
    assert_eq!(
        document["sections"][2]["items"],
        json!(["SW2"]),
        "never backed up"
    );
    let moved = rows(content, "Configurations that changed in the last 7 days");
    assert_eq!(moved.len(), 1, "the baseline is not a change");
    assert_eq!(moved[0]["lines_added"], 1);
    assert_eq!(moved[0]["source"], "post-change");
}

#[tokio::test]
async fn a_filter_is_in_the_header_and_narrows_the_report() {
    let lab = lab(&["SW1", "SW2"], With::nothing()).await;
    let app = &lab.app;
    rpc(
        app,
        "devices.update",
        json!({"id": lab.id("SW1"), "site": "HQ"}),
    )
    .await;
    rpc(
        app,
        "devices.update",
        json!({"id": lab.id("SW2"), "site": "Branch"}),
    )
    .await;
    let report = generate(
        app,
        "inventory",
        "markdown",
        json!({"filter": {"site": "HQ"}}),
    )
    .await;
    let content = report["content"].as_str().unwrap();
    assert!(content.contains("**Filter:** site = HQ"), "{content}");
    assert!(content.contains("**Devices covered:** 1"));
    assert!(content.contains("| SW1 |") && !content.contains("| SW2 |"));
    assert_eq!(report["rows"], 1);
    assert!(content.contains("**Generated:** "));
    assert!(content.contains("**Times shown in:** UTC"));
    // A period report says which period, in the stated zone.
    let report = generate(app, "availability", "markdown", json!({})).await;
    assert!(
        report["content"].as_str().unwrap().contains("**Period:** "),
        "a period report carries its period"
    );
    assert_eq!(
        app.rpc(
            "reports.generate",
            json!({"kind": "inventory", "filter": "not an object"})
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_saved_report_keeps_the_text_the_server_generated() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let saved = rpc(
        app,
        "reports.save",
        json!({"kind": "inventory", "format": "html", "title": "Inventory for the auditor",
               "notes": "Asked for by finance.", "author": "mo", "utc_offset_minutes": 0}),
    )
    .await;
    let id = saved["id"].as_str().unwrap().to_string();
    assert_eq!(saved["kind"], "inventory");
    assert_eq!(saved["format"], "html");
    assert_eq!(saved["title"], "Inventory for the auditor");
    assert!(saved["content"].as_str().unwrap().starts_with("<!doctype"));
    assert_eq!(saved["rows"], 1);

    let listed = rpc(app, "reports.list", json!({})).await;
    assert_eq!(listed["reports"].as_array().unwrap().len(), 1);
    assert_eq!(listed["reports"][0]["id"], json!(id));
    assert!(
        listed["reports"][0]["bytes"].as_u64().unwrap() > 100,
        "the list carries the size, not the text"
    );
    assert!(listed["reports"][0].get("content").is_none());
    assert_eq!(
        rpc(app, "reports.list", json!({"kind": "availability"})).await["reports"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let fetched = rpc(app, "reports.get", json!({"id": id})).await;
    assert_eq!(fetched["content"], saved["content"]);
    assert_eq!(fetched["notes"], "Asked for by finance.");
    assert_eq!(
        rpc(app, "reports.delete", json!({"id": id})).await["deleted"],
        true
    );
    assert_eq!(
        app.rpc("reports.get", json!({"id": id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}
