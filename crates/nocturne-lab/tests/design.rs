//! `design.*`: requirements in, a network out, booted as a lab.

mod common;

use common::*;
use serde_json::json;

#[tokio::test]
async fn a_design_is_generated_validated_and_booted_as_a_lab() {
    let app = app();

    // The examples are the engine's own, so the screen has somewhere to start.
    let examples = app.rpc("design.examples", json!(null)).await.unwrap();
    let examples = examples.as_array().unwrap();
    assert!(examples.len() >= 3, "{examples:?}");
    let requirements = examples[0]["requirements"].clone();

    // Requirements that cannot be met are refused in words, before anything
    // is built, and `validate` and `generate` agree about it.
    let impossible = json!({"requirements": {
        "project": "Too small",
        "sites": [{"name": "HQ", "users": 2000}],
        "addressing": {"base": "10.0.0.0/24"},
    }});
    let checked = app
        .rpc("design.validate", impossible.clone())
        .await
        .unwrap();
    assert_eq!(checked["ok"], json!(false));
    let problems = checked["problems"].as_array().unwrap();
    assert!(!problems.is_empty());
    let refused = app.rpc("design.generate", impossible).await.unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(
        problems
            .iter()
            .any(|problem| refused.message.contains(problem.as_str().unwrap())),
        "the refusal should carry the same problems: {refused:?} vs {problems:?}"
    );

    // A design that can be met comes with the arithmetic behind it.
    let design = app
        .rpc("design.generate", json!({"requirements": requirements}))
        .await
        .unwrap();
    assert!(design["plan"]["devices"].as_array().unwrap().len() >= 2);
    assert!(!design["plan"]["vlans"].as_array().unwrap().is_empty());
    assert!(!design["plan"]["checks"].as_array().unwrap().is_empty());
    assert!(!design["decisions"].as_array().unwrap().is_empty());
    assert!(design["document"].as_str().unwrap().contains("CHANGEME"));
    assert!(design["lab"]["nodes"].as_u64().unwrap() >= 2);

    // And it really boots: the lab it makes is an ordinary lab, with the
    // devices in the inventory, which is the whole point of designing into
    // a simulator rather than into a document.
    let lab = app
        .rpc(
            "design.create_lab",
            json!({"requirements": requirements, "name": "Designed"}),
        )
        .await
        .unwrap();
    assert_eq!(lab["name"], json!("Designed"));
    let id = lab["id"].as_str().unwrap().to_string();
    assert_eq!(
        lab["snapshot"]["nodes"].as_array().unwrap().len() as u64,
        design["lab"]["nodes"].as_u64().unwrap(),
        "the lab holds exactly the devices the plan named"
    );
    // Every device with a command line is in the inventory, so the terminal,
    // the collectors and a change plan can all be pointed at the design.
    assert!(!lab["devices"].as_object().unwrap().is_empty());

    let started = app.rpc("labs.start", json!({"id": id})).await.unwrap();
    assert_eq!(started["running"], json!(true));
    app.rpc("labs.stop", json!({"id": id})).await.unwrap();
}

/// `design.export` is in the lab contract (`docs/dev/features/lab.md`) and was
/// the one method of that surface never registered — found by diffing the
/// specifications against the methods the assembled backend answers.
#[tokio::test]
async fn a_design_is_exported_as_a_document_or_as_configurations() {
    let app = app();
    let examples = app.rpc("design.examples", json!(null)).await.unwrap();
    let requirements = examples.as_array().unwrap()[0]["requirements"].clone();

    let document = app
        .rpc("design.export", json!({"requirements": requirements}))
        .await
        .expect("markdown is the default format");
    assert_eq!(document["format"], json!("markdown"));
    let text = document["document"].as_str().unwrap();
    // The three things that make it a deliverable rather than a dump: what to
    // change before it is deployed, what to buy, and why it is built this way.
    for heading in [
        "## Before you deploy this",
        "## Sites",
        "## Why it is built this way",
    ] {
        assert!(text.contains(heading), "missing {heading}:\n{text}");
    }
    assert!(
        text.contains("CHANGEME"),
        "every secret in it is a placeholder, and it must say so"
    );

    let configs = app
        .rpc(
            "design.export",
            json!({"requirements": requirements, "format": "configs"}),
        )
        .await
        .expect("configurations are exported too");
    assert_eq!(configs["format"], json!("configs"));
    let by_device = configs["configs"].as_object().unwrap();
    assert!(!by_device.is_empty());
    for (name, config) in by_device {
        let config = config.as_str().unwrap();
        assert!(
            config.contains(&format!("hostname {name}")),
            "{name}'s configuration should be {name}'s:\n{config}"
        );
    }

    // A host has addressing and services, not a command line. It is named
    // rather than silently left out, so an export is accountable for every
    // node in the design.
    let without = configs["without_a_configuration"].as_array().unwrap();
    let named: usize = by_device.len() + without.len();
    let generated = app
        .rpc("design.generate", json!({"requirements": requirements}))
        .await
        .unwrap();
    assert_eq!(
        named as u64,
        generated["lab"]["nodes"].as_u64().unwrap(),
        "every node is either exported or listed as having nothing to export"
    );

    // The same requirements must export the same document: the engine reads
    // no clock and no randomness, which is what lets the export regenerate
    // the design instead of carrying a whole `Design` back and forth.
    let again = app
        .rpc("design.export", json!({"requirements": requirements}))
        .await
        .unwrap();
    assert_eq!(again, document);
}
