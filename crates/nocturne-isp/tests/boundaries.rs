//! The three lines this crate is not allowed to cross, checked rather than
//! trusted: no float on a money path, no reach towards a device, and no
//! personal detail in the one result a model is given.
//!
//! Two of these read the crate's own source. That is deliberate. "A float
//! never appears in a money path" and "nothing here configures a device" are
//! properties of the code, not of one execution of it, and a test that only
//! exercised today's call graph would pass the day somebody adds a new path.

mod common;

use std::path::{Path, PathBuf};

use common::Isp;
use serde_json::json;

fn src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn read(name: &str) -> String {
    let path = src().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

fn sources() -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = std::fs::read_dir(src())
        .expect("the crate's source directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            (name, std::fs::read_to_string(&path).expect("a source file"))
        })
        .collect();
    found.sort();
    assert!(found.len() >= 8, "every module is scanned: {found:?}");
    found
}

#[test]
fn no_floating_point_type_appears_anywhere_on_a_money_path() {
    // `network.rs` is where bandwidth lives — bits per second, percentages
    // and ratios are measurements, and a float is the right type for them.
    // No money is worked out there: it takes `Money` from a subscription or a
    // link and only ever adds, subtracts, multiplies or scales it, all of
    // which are integer operations on this type.
    for module in [
        "money.rs",
        "model.rs",
        "db.rs",
        "billing.rs",
        "reports.rs",
        "records.rs",
        "api.rs",
    ] {
        let text = read(module)
            // `is_f64()` is the guard itself: it is how a JSON number with a
            // point in it is refused on the way in.
            .replace("is_f64", "")
            // The module documentation talks about floats in order to say
            // there are none.
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in ["f64", "f32", "as_f64", "parse::<f"] {
            assert!(
                !text.contains(forbidden),
                "{module} mentions `{forbidden}`: money is whole minor units, and a float on \
                 this path is how a ledger stops adding up"
            );
        }
    }
    // The one module that may use floats says in its own words why.
    let network = read("network.rs");
    assert!(
        network.contains("f64"),
        "network.rs is where the bandwidth measurements live"
    );
    assert!(
        !network.contains("Money::new("),
        "network.rs does not mint money from a measurement"
    );
}

#[test]
fn nothing_in_this_crate_can_reach_a_device() {
    for (module, text) in sources() {
        let code: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in [
            // The core's only ways to touch a device.
            "connect::",
            "nocturne_transport",
            "nocturne_drivers",
            "session()",
            "CliOptions",
            // The change pipeline, which is the only thing that may.
            "changes.create",
            "changes.apply",
            "config.push",
        ] {
            assert!(
                !code.contains(forbidden),
                "{module} mentions `{forbidden}`: this feature is bookkeeping, and a billing bug \
                 that disconnects a paying customer is a worse failure than a late suspension"
            );
        }
    }
    // It reaches the network through exactly two method names, both reads.
    let network = read("network.rs");
    for method in ["monitor.interfaces", "metrics.query"] {
        assert!(network.contains(method), "{method} is how usage is read");
    }
    // And the manifest names no feature crate at all.
    let manifest =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect("the manifest");
    for feature_crate in [
        "nocturne-monitor",
        "nocturne-changes",
        "nocturne-config",
        "nocturne-ipam",
        "nocturne-ops",
        "nocturne-suite",
    ] {
        assert!(
            !manifest.contains(feature_crate),
            "a feature crate must not depend on {feature_crate}"
        );
    }
}

#[tokio::test]
async fn the_summary_is_counts_and_states_and_names_nobody() {
    let isp = Isp::start(false, &["RB1"]).await;
    let device = isp.device("RB1");
    // A customer whose every personal field is something unmistakable.
    let customer = isp
        .customer(
            "Zainab Al-Khafaji",
            "IQD",
            json!({
                "code": "ACC-90210",
                "phone": "0770 123 4567",
                "email": "zainab@example.com",
                "address": "14 Palm Street, Erbil",
                "location": "roof of the blue building",
                "notes": "pays late, always rings first",
                "device_id": device,
                "interface": "ether3",
                "tower": "Tower A",
            }),
        )
        .await;
    let tariff = isp
        .tariff("Home 10", 10, "flat", 25_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    isp.invoice(&customer, common::last_month(), None, None)
        .await;

    let summary = isp.ok("isp.summary", json!({})).await;
    let text = serde_json::to_string(&summary).expect("the summary serialises");
    for personal in [
        "Zainab",
        "Al-Khafaji",
        "ACC-90210",
        "0770 123 4567",
        "zainab@example.com",
        "Palm Street",
        "blue building",
        "rings first",
    ] {
        assert!(
            !text.contains(personal),
            "`{personal}` is in isp.summary, and that is the one result a model is given: {text}"
        );
    }
    // What it does say is the shape of the business.
    assert_eq!(summary["customers"], json!(1));
    assert_eq!(summary["by_state"][0]["state"], json!("active"));
    assert_eq!(summary["by_state"][0]["customers"], json!(1));
    assert_eq!(summary["by_currency"][0]["currency"], json!("IQD"));
    assert_eq!(summary["subscriptions_active"], json!(1));
    assert_eq!(summary["sold_mbit"], json!(10));
    assert_eq!(summary["tariffs"], json!(1));
    assert_eq!(summary["invoices"]["issued"], json!(1));
    assert_eq!(summary["arrears"][0]["currency"], json!("IQD"));
    assert_eq!(summary["arrears"][0]["total"]["minor"], json!(25_000));
    assert_eq!(
        summary["usage_coverage"]["with_device_and_interface"],
        json!(1)
    );
    assert!(
        summary["note"]
            .as_str()
            .unwrap_or_default()
            .contains("no customer's name"),
        "it says what it is for"
    );
    // The tower is a piece of network, not a person, so it may appear in the
    // capacity report — but a customer's name may not.
    isp.ok(
        "isp.link.save",
        json!({"name": "Tower A feed", "tower": "Tower A", "capacity_mbit": 50}),
    )
    .await;
    let capacity = serde_json::to_string(&isp.ok("isp.capacity", json!({})).await)
        .expect("the capacity report serialises");
    assert!(capacity.contains("Tower A feed"));
    assert!(!capacity.contains("Zainab"), "{capacity}");
    let margin = serde_json::to_string(&isp.ok("isp.margin", json!({})).await)
        .expect("the margin report serialises");
    assert!(!margin.contains("Zainab"), "{margin}");
}
