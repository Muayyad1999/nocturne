//! Used against sold, oversubscription and margin — and, mostly, the
//! difference between "using nothing" and "nobody has looked".

mod common;

use common::{DAY_MS, Isp, last_month, list};
use serde_json::json;

fn now() -> i64 {
    nocturne_app::util::now_ms()
}

/// The row of a usage report for one customer.
fn usage_row<'a>(report: &'a serde_json::Value, customer_id: &str) -> &'a serde_json::Value {
    report["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|row| row["customer_id"] == json!(customer_id))
        .unwrap_or_else(|| panic!("a row for {customer_id}"))
}

#[tokio::test]
async fn a_customer_with_no_collected_data_has_no_usage_figure_rather_than_a_zero() {
    let isp = Isp::start(true, &["RB1"]).await;
    let device = isp.device("RB1");
    let tariff = isp
        .tariff("Home 10", 10, "flat", 2_500, "USD", "monthly")
        .await;

    // Four customers, four different nothings.
    let measured = isp
        .customer(
            "Measured Mo",
            "USD",
            json!({"device_id": device, "interface": "ether3"}),
        )
        .await;
    let no_rate = isp
        .customer(
            "Rebooted Rita",
            "USD",
            json!({"device_id": device, "interface": "ether4"}),
        )
        .await;
    let never = isp
        .customer(
            "Uncollected Umar",
            "USD",
            json!({"device_id": device, "interface": "ether9"}),
        )
        .await;
    let no_device = isp.customer("Paper-only Pola", "USD", json!({})).await;
    for customer in [&measured, &no_rate, &never, &no_device] {
        isp.subscribe(customer, &tariff, None).await;
    }
    // Mo's interface is measured; Rita's is known but its rate could not be
    // worked out (a reboot); Umar's has never been collected at all.
    isp.monitor().interface(
        &device,
        "ether3",
        Some(3_000_000.0),
        Some(9_000_000.0),
        Some(100_000_000),
    );
    isp.monitor()
        .interface(&device, "ether4", None, None, Some(100_000_000));

    let report = isp.ok("isp.usage", json!({})).await;
    assert_eq!(report["customers"], json!(4));
    assert_eq!(report["measured"], json!(1));
    assert_eq!(report["without_usage"], json!(3));

    let mo = usage_row(&report, &measured);
    assert_eq!(mo["coverage"], json!("measured"));
    assert_eq!(mo["used"]["used_mbit"], json!(9.0));
    assert_eq!(mo["used"]["in_mbit"], json!(3.0));
    assert_eq!(mo["sold_mbit"], json!(10));
    assert_eq!(mo["of_plan_pct"], json!(90.0));
    assert_eq!(mo["used"]["speed_mbit"], json!(100.0));

    for (customer, coverage, says) in [
        (&no_rate, "no_rate_yet", "a gap, not a zero"),
        (&never, "interface_never_collected", "nobody has looked"),
        (&no_device, "no_serving_device", "not a usage of zero"),
    ] {
        let row = usage_row(&report, customer);
        assert_eq!(row["coverage"], json!(coverage), "{}", row);
        assert!(
            row["used"].is_null(),
            "{coverage}: there is no figure at all, not a zero — {row}"
        );
        assert!(row["of_plan_pct"].is_null(), "{coverage}");
        assert!(
            row["coverage_note"]
                .as_str()
                .unwrap_or_default()
                .contains(says),
            "{}",
            row["coverage_note"]
        );
        assert_eq!(
            row["sold_mbit"],
            json!(10),
            "what they were sold is known even when what they use is not"
        );
    }

    // The report's own coverage, so nobody reads three nulls as three zeroes.
    let coverage = list(&report["coverage"]);
    assert_eq!(coverage.len(), 4, "one line per kind of nothing");
    assert!(
        coverage
            .iter()
            .all(|line| line["customers"].as_u64() == Some(1))
    );
    assert!(
        report["note"]
            .as_str()
            .unwrap_or_default()
            .contains("not a customer using nothing")
    );

    // A customer with no subscription has no sold figure either — not zero.
    let nothing_sold = isp
        .customer(
            "Prospect Pat",
            "USD",
            json!({"device_id": device, "interface": "ether3"}),
        )
        .await;
    let report = isp
        .ok("isp.usage", json!({"customer_id": nothing_sold}))
        .await;
    let pat = usage_row(&report, &nothing_sold);
    assert!(pat["sold_mbit"].is_null(), "nobody sold them anything");
    assert_eq!(pat["subscriptions"], json!(0));
    assert_eq!(pat["coverage"], json!("measured"));
    assert!(
        pat["of_plan_pct"].is_null(),
        "there is no plan to be a percentage of"
    );
}

#[tokio::test]
async fn without_monitoring_nobody_has_a_usage_figure_and_the_report_says_so() {
    let isp = Isp::start(false, &["RB1"]).await;
    let device = isp.device("RB1");
    let customer = isp
        .customer(
            "Would Be Measured",
            "USD",
            json!({"device_id": device, "interface": "ether3"}),
        )
        .await;
    let tariff = isp
        .tariff("Home 10", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;

    let report = isp.ok("isp.usage", json!({})).await;
    let row = usage_row(&report, &customer);
    assert_eq!(row["coverage"], json!("monitor_unavailable"));
    assert!(row["used"].is_null());
    assert_eq!(report["measured"], json!(0));

    let summary = isp.ok("isp.summary", json!({})).await;
    assert_eq!(
        summary["usage_coverage"]["monitoring_present"],
        json!(false),
        "the summary admits there is nothing measuring interfaces"
    );
    assert_eq!(
        summary["usage_coverage"]["with_device_and_interface"],
        json!(1)
    );

    // A customer whose device has been removed from the inventory is a
    // different nothing again, and the record is not quietly cleared.
    let isp = Isp::start(true, &["RB1"]).await;
    let device = isp.device("RB1");
    let customer = isp
        .customer(
            "Orphaned Omar",
            "USD",
            json!({"device_id": device, "interface": "ether3"}),
        )
        .await;
    isp.ok("devices.delete", json!({"ids": [device]})).await;
    let report = isp.ok("isp.usage", json!({"customer_id": customer})).await;
    let orphaned = usage_row(&report, &customer);
    assert_eq!(orphaned["coverage"], json!("device_missing"));
    assert!(orphaned["used"].is_null());
    assert_eq!(
        orphaned["device_id"].as_str(),
        Some(device.as_str()),
        "the record still says which device served them"
    );
}

#[tokio::test]
async fn used_against_sold_over_a_window_states_the_window_it_covers() {
    let isp = Isp::start(true, &["RB1"]).await;
    let device = isp.device("RB1");
    let customer = isp
        .customer(
            "Heavy Hana",
            "USD",
            json!({"device_id": device, "interface": "ether3"}),
        )
        .await;
    let tariff = isp
        .tariff("Home 10", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    isp.monitor().interface(
        &device,
        "ether3",
        Some(1_000_000.0),
        Some(2_000_000.0),
        Some(100_000_000),
    );

    // Over the last day the averages are 2 and 14 Mbit, peaking at 20.
    let (since, until) = (now() - DAY_MS, now() + 1);
    isp.monitor().samples(
        &device,
        "if.in_bps",
        "ether3",
        &[(since + 1_000, 1_000_000.0), (since + 2_000, 3_000_000.0)],
    );
    isp.monitor().samples(
        &device,
        "if.out_bps",
        "ether3",
        &[(since + 1_000, 8_000_000.0), (since + 2_000, 20_000_000.0)],
    );

    let report = isp
        .ok("isp.usage", json!({"since": since, "until": until}))
        .await;
    let row = usage_row(&report, &customer);
    assert_eq!(row["coverage"], json!("measured"));
    assert_eq!(row["used"]["in_mbit"], json!(2.0));
    assert_eq!(row["used"]["out_mbit"], json!(14.0));
    assert_eq!(row["used"]["used_mbit"], json!(14.0));
    assert_eq!(row["used"]["peak_mbit"], json!(20.0));
    assert_eq!(row["used"]["samples"], json!(2));
    assert_eq!(row["used"]["window"]["since"], json!(since));
    assert_eq!(row["used"]["window"]["rollup"], json!("5m"));
    assert!(row["used"]["at"].is_null(), "a window has no single moment");
    assert_eq!(
        row["of_plan_pct"],
        json!(140.0),
        "a customer at 140% of their plan is a conversation, not an error"
    );
    assert!(
        row["used"]["basis"]
            .as_str()
            .unwrap_or_default()
            .contains("averaged over the window")
    );

    // A window with nothing in it is a gap, not a zero.
    let empty = isp
        .ok(
            "isp.usage",
            json!({"since": now() - 400 * DAY_MS, "until": now() - 399 * DAY_MS}),
        )
        .await;
    let gap = usage_row(&empty, &customer);
    assert_eq!(gap["coverage"], json!("no_samples_in_window"));
    assert!(gap["used"].is_null());
}

#[tokio::test]
async fn oversubscription_is_a_ratio_with_both_figures_and_what_is_measured() {
    let isp = Isp::start(true, &["RB1"]).await;
    let device = isp.device("RB1");
    let tariff = isp
        .tariff("Home 10", 10, "flat", 2_500, "USD", "monthly")
        .await;
    for name in ["A1", "A2", "A3"] {
        let customer = isp
            .customer(
                name,
                "USD",
                json!({"tower": "Tower A", "device_id": device, "interface": "ether3"}),
            )
            .await;
        isp.subscribe(&customer, &tariff, None).await;
    }
    // A fourth customer is suspended: still sold, counted apart.
    let suspended = isp.customer("A4", "USD", json!({"tower": "Tower A"})).await;
    isp.subscribe(&suspended, &tariff, None).await;
    isp.ok(
        "isp.customer.state",
        json!({"id": suspended, "state": "suspended", "reason": "non-payment"}),
    )
    .await;
    // And one behind another tower, who must not be counted here at all.
    let elsewhere = isp.customer("B1", "USD", json!({"tower": "Tower B"})).await;
    isp.subscribe(&elsewhere, &tariff, None).await;

    isp.monitor().interface(
        &device,
        "ether1",
        Some(18_000_000.0),
        Some(4_000_000.0),
        Some(100_000_000),
    );
    isp.ok(
        "isp.link.save",
        json!({
            "name": "Tower A feed",
            "tower": "Tower A",
            "device_id": device,
            "interface": "ether1",
            "capacity_mbit": 10,
            "cost_per_mbit": {"currency": "USD", "minor": 500},
            "cost_period": "monthly",
        }),
    )
    .await;

    let capacity = list(&isp.ok("isp.capacity", json!({})).await);
    assert_eq!(capacity.len(), 1);
    let tower = &capacity[0];
    assert_eq!(tower["sold_mbit"], json!(30));
    assert_eq!(tower["suspended_mbit"], json!(10));
    assert_eq!(tower["customers"], json!(3));
    assert_eq!(tower["suspended_customers"], json!(1));
    assert_eq!(tower["capacity_mbit"], json!(10));
    assert_eq!(tower["capacity_basis"], json!("declared"));
    assert_eq!(tower["ratio"], json!(3.0));
    assert_eq!(tower["ratio_text"], json!("3.0:1"));
    assert_eq!(tower["coverage"], json!("measured"));
    assert_eq!(
        tower["measured"]["used_mbit"],
        json!(18.0),
        "3:1 on a link that is never busy reads differently from 3:1 on one that saturates"
    );

    // A link with no capacity recorded has no ratio, and says so instead of
    // dividing by zero or guessing.
    isp.ok(
        "isp.link.save",
        json!({"name": "Tower B feed", "tower": "Tower B", "capacity_mbit": 0}),
    )
    .await;
    let b = list(&isp.ok("isp.capacity", json!({"tower": "Tower B"})).await);
    assert_eq!(b.len(), 1);
    assert!(b[0]["ratio"].is_null());
    assert!(
        b[0]["ratio_text"]
            .as_str()
            .unwrap_or_default()
            .contains("no capacity recorded")
    );
    assert_eq!(b[0]["coverage"], json!("no_serving_device"));
    assert_eq!(b[0]["sold_mbit"], json!(10));

    // A link whose capacity nobody wrote down can take the interface's own
    // speed — and labels the figure as having come from there.
    isp.monitor().interface(
        &device,
        "ether2",
        Some(1_000_000.0),
        None,
        Some(1_000_000_000),
    );
    let saved = isp
        .ok(
            "isp.link.save",
            json!({"name": "Upstream", "tower": "Tower C", "device_id": device,
                   "interface": "ether2", "capacity_mbit": 0}),
        )
        .await;
    let upstream = list(
        &isp.ok("isp.capacity", json!({"link_id": saved["link"]["id"]}))
            .await,
    );
    assert_eq!(upstream[0]["capacity_mbit"], json!(1_000));
    assert_eq!(upstream[0]["capacity_basis"], json!("interface_speed"));
}

#[tokio::test]
async fn a_margin_is_only_worked_out_in_the_currency_the_cost_is_in() {
    let isp = Isp::start(false, &[]).await;
    let usd_tariff = isp
        .tariff("USD 10", 10, "flat", 5_000, "USD", "monthly")
        .await;
    let iqd_tariff = isp
        .tariff("IQD 10", 10, "flat", 60_000, "IQD", "monthly")
        .await;
    let yearly = isp
        .tariff("USD yearly", 10, "flat", 60_000, "USD", "yearly")
        .await;
    for (name, currency, tariff) in [
        ("Dollar One", "USD", &usd_tariff),
        ("Dollar Two", "USD", &yearly),
        ("Dinar One", "IQD", &iqd_tariff),
    ] {
        let customer = isp
            .customer(name, currency, json!({"tower": "Tower A"}))
            .await;
        isp.subscribe(&customer, tariff, None).await;
    }
    isp.ok(
        "isp.link.save",
        json!({
            "name": "Tower A feed",
            "tower": "Tower A",
            "capacity_mbit": 20,
            "cost_per_mbit": {"currency": "USD", "minor": 1_000},
            "cost_period": "monthly",
        }),
    )
    .await;

    let margins = list(&isp.ok("isp.margin", json!({})).await);
    assert_eq!(margins.len(), 1);
    let margin = &margins[0];
    // $50 a month, plus $600 a year as $50 a month: $100.
    let revenue = list(&margin["revenue"]);
    assert_eq!(revenue.len(), 2, "one figure per currency, as ever");
    let usd = revenue
        .iter()
        .find(|row| row["currency"] == json!("USD"))
        .expect("dollars");
    assert_eq!(usd["minor"], json!(10_000));
    // 20 Mbit at $10 is $200 a month, so the tower loses $100 a month.
    assert_eq!(margin["cost"]["minor"], json!(20_000));
    assert_eq!(margin["cost"]["currency"], json!("USD"));
    assert_eq!(margin["margin"]["minor"], json!(-10_000));
    assert_eq!(margin["margin"]["currency"], json!("USD"));
    // The dinars are left exactly as they are.
    let unconverted = list(&margin["unconverted"]);
    assert_eq!(unconverted.len(), 1);
    assert_eq!(unconverted[0]["currency"], json!("IQD"));
    assert_eq!(unconverted[0]["minor"], json!(60_000));
    assert!(
        margin["note"]
            .as_str()
            .unwrap_or_default()
            .contains("without a rate somebody entered"),
        "{}",
        margin["note"]
    );
    assert!(margin["exact"].as_bool().unwrap_or(false));

    // Without a wholesale cost there is no margin — only what is charged.
    isp.ok(
        "isp.link.save",
        json!({"name": "Tower B feed", "tower": "Tower B", "capacity_mbit": 100}),
    )
    .await;
    let b = list(&isp.ok("isp.margin", json!({"tower": "Tower B"})).await);
    assert!(b[0]["cost"].is_null());
    assert!(b[0]["margin"].is_null());
    assert!(
        b[0]["note"]
            .as_str()
            .unwrap_or_default()
            .contains("no wholesale cost")
    );

    // A tower nobody recorded a link for is a refusal, not an empty answer
    // that reads like "no margin".
    let err = isp.err("isp.margin", json!({"tower": "Tower Z"})).await;
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn one_customers_margin_comes_from_the_link_that_feeds_them() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp
        .customer("Tower A tenant", "USD", json!({"tower": "Tower A"}))
        .await;
    let tariff = isp
        .tariff("Business 20", 20, "flat", 10_000, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    isp.ok(
        "isp.link.save",
        json!({
            "name": "Tower A feed",
            "tower": "Tower A",
            "capacity_mbit": 100,
            "cost_per_mbit": {"currency": "USD", "minor": 200},
            "cost_period": "monthly",
        }),
    )
    .await;
    let margins = list(&isp.ok("isp.margin", json!({"customer_id": customer})).await);
    assert_eq!(margins[0]["of"], json!("customer"));
    assert_eq!(margins[0]["cost_mbit"], json!(20), "what they were sold");
    // 20 Mbit at $2 is $40; they pay $100.
    assert_eq!(margins[0]["cost"]["minor"], json!(4_000));
    assert_eq!(margins[0]["margin"]["minor"], json!(6_000));

    // A customer behind no link with a cost gets no margin and a reason.
    let orphan = isp.customer("Nowhere", "USD", json!({})).await;
    isp.subscribe(&orphan, &tariff, None).await;
    let none = list(&isp.ok("isp.margin", json!({"customer_id": orphan})).await);
    assert!(none[0]["cost"].is_null());
    assert!(none[0]["margin"].is_null());
    assert!(
        none[0]["note"]
            .as_str()
            .unwrap_or_default()
            .contains("no link with a wholesale cost")
    );
}

#[tokio::test]
async fn a_customers_detail_puts_what_they_owe_beside_what_they_use() {
    let isp = Isp::start(true, &["RB1"]).await;
    let device = isp.device("RB1");
    let customer = isp
        .customer(
            "Detail Dana",
            "USD",
            json!({"device_id": device, "interface": "ether3", "tower": "Tower A",
                   "phone": "0770 000 0000", "code": "D-1"}),
        )
        .await;
    let tariff = isp
        .tariff("Home 10", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    isp.monitor().interface(
        &device,
        "ether3",
        Some(4_000_000.0),
        Some(1_000_000.0),
        None,
    );
    isp.invoice(&customer, last_month(), None, None).await;

    let detail = isp.ok("isp.customer.get", json!({"id": customer})).await;
    assert_eq!(detail["customer"]["code"], json!("D-1"));
    assert_eq!(detail["currency"], json!("USD"));
    assert_eq!(list(&detail["subscriptions"]).len(), 1);
    assert_eq!(list(&detail["invoices"]).len(), 1);
    assert_eq!(detail["balance"]["outstanding"][0]["minor"], json!(2_500));
    assert_eq!(detail["usage"]["coverage"], json!("measured"));
    assert_eq!(detail["usage"]["used_mbit"], json!(null), "it is in `used`");
    assert_eq!(detail["usage"]["used"]["used_mbit"], json!(4.0));
    assert_eq!(detail["usage"]["sold_mbit"], json!(10));
    assert!(
        detail["usage"]["used"]["speed_mbit"].is_null(),
        "the interface did not report a speed, so there is none"
    );
}
