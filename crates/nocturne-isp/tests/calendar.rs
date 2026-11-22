//! The business's calendar.
//!
//! A billing period is a month, an invoice falls due on a day, and "overdue"
//! begins when that day is over. All three need to know where midnight is,
//! and the answer is the ISP's — not UTC's, and not that of whichever browser
//! happens to be open. These tests set `utc_offset_minutes` and hold the
//! engine to it.

mod common;

use common::{DAY_MS, Isp, list};
use serde_json::{Value, json};

/// A moment, written the way a person would say it.
fn at(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|err| panic!("`{text}` is not a moment: {err}"))
        .timestamp_millis()
}

fn now() -> i64 {
    nocturne_app::util::now_ms()
}

async fn set_calendar(isp: &Isp, utc_offset_minutes: i64) {
    isp.ok(
        "settings.set",
        json!({"section": "isp", "value": {"utc_offset_minutes": utc_offset_minutes}}),
    )
    .await;
}

/// Set a calendar in which it is midday right now, and answer the first
/// moment of today in it.
///
/// The tests below are about which side of a midnight something falls, and
/// they run against the real clock. With midnight twelve hours away in both
/// directions none of them can straddle one, however slowly it runs — and
/// the offset is whatever that takes, so the calendar is rarely UTC.
async fn midday(isp: &Isp) -> i64 {
    let now = now();
    let minute_of_day = now.rem_euclid(DAY_MS) / 60_000;
    let offset = 720 - minute_of_day;
    set_calendar(isp, offset).await;
    let offset_ms = offset * 60_000;
    (now + offset_ms).div_euclid(DAY_MS) * DAY_MS - offset_ms
}

/// A customer with one issued invoice of 100.00 USD, issued a month ago and
/// due at `due_at`.
async fn invoice_due(isp: &Isp, name: &str, due_at: i64) -> (String, String) {
    let customer = isp.customer(name, "USD", json!({})).await;
    let tariff = isp
        .tariff(
            &format!("{name}'s plan"),
            10,
            "flat",
            10_000,
            "USD",
            "monthly",
        )
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let issued = isp
        .invoice(
            &customer,
            (now() - 70 * DAY_MS, now() - 40 * DAY_MS),
            Some(now() - 40 * DAY_MS),
            Some(due_at),
        )
        .await;
    (customer, issued["id"].as_str().expect("an id").to_string())
}

#[tokio::test]
async fn the_calendar_is_a_setting_that_defaults_to_utc_and_is_refused_outside_its_range() {
    let isp = Isp::start(false, &[]).await;
    let settings = isp.ok("settings.get", json!({"section": "isp"})).await;
    assert_eq!(
        settings["utc_offset_minutes"],
        json!(0),
        "the calendar is not among the settings: {settings:#}"
    );

    // Saved where the other settings are, and read back from there.
    set_calendar(&isp, 180).await;
    let settings = isp.ok("settings.get", json!({"section": "isp"})).await;
    assert_eq!(settings["utc_offset_minutes"], json!(180));

    // The edges of the range are calendars somebody really keeps.
    for offset in [-720, 840] {
        set_calendar(&isp, offset).await;
        let run = isp.ok("isp.invoice.run", json!({"month": "2025-10"})).await;
        assert_eq!(run["utc_offset_minutes"], json!(offset));
    }
    // Beyond them, nothing is billed, aged or called overdue on a guess.
    for offset in [-721, 841, 100_000] {
        set_calendar(&isp, offset).await;
        for (method, params) in [
            ("isp.invoice.run", json!({"month": "2025-10"})),
            ("isp.arrears", json!({})),
            ("isp.invoices", json!({})),
            ("isp.summary", json!({})),
        ] {
            let err = isp.err(method, params).await;
            assert_eq!(err.code, "invalid", "{method} with {offset}: {err}");
            assert!(
                err.message.contains("utc_offset_minutes"),
                "{method} does not say which setting to put right: {}",
                err.message
            );
        }
    }
}

#[tokio::test]
async fn a_plan_that_starts_on_the_first_is_billed_for_that_month_and_not_the_one_before() {
    // East of UTC: the first moment of 1 October in Baghdad is still
    // September in UTC, and September's bill must not contain three hours
    // of a service that had not begun.
    let isp = Isp::start(false, &[]).await;
    set_calendar(&isp, 180).await;
    let customer = isp.customer("Tigris Net", "IQD", json!({})).await;
    let tariff = isp
        .tariff("Home 10", 10, "flat", 50_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, Some(at("2025-10-01T00:00:00+03:00")))
        .await;

    let september = isp.ok("isp.invoice.run", json!({"month": "2025-09"})).await;
    assert_eq!(
        (&september["period_start"], &september["period_end"]),
        (
            &json!(at("2025-09-01T00:00:00+03:00")),
            &json!(at("2025-10-01T00:00:00+03:00"))
        ),
        "September is the business's September"
    );
    assert!(
        list(&september["invoices"]).is_empty(),
        "billed for a month before the service began: {september:#}"
    );
    assert_eq!(list(&september["skipped"]).len(), 1, "{september:#}");

    let october = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "month": "2025-10"}),
        )
        .await;
    let invoice = &october["invoice"];
    assert_eq!(
        invoice["period_start"],
        json!(at("2025-10-01T00:00:00+03:00"))
    );
    assert_eq!(
        invoice["period_end"],
        json!(at("2025-11-01T00:00:00+03:00"))
    );
    assert_eq!(invoice["total"]["minor"], json!(50_000));
    assert_eq!(
        invoice["lines"][0]["prorated"],
        Value::Null,
        "a whole month of the plan is not a part period"
    );
    assert_eq!(
        invoice["utc_offset_minutes"],
        json!(180),
        "an invoice says which calendar its dates are days of"
    );

    // West of UTC the same plan must not lose the first hours of its month.
    let isp = Isp::start(false, &[]).await;
    set_calendar(&isp, -240).await;
    let customer = isp.customer("Hudson Wireless", "USD", json!({})).await;
    let tariff = isp
        .tariff("Home 10", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, Some(at("2025-10-01T00:00:00-04:00")))
        .await;
    let october = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "month": "2025-10"}),
        )
        .await;
    assert_eq!(
        october["invoice"]["total"]["minor"],
        json!(2_500),
        "25.00 was agreed for October and October is what was used"
    );
    assert_eq!(october["invoice"]["lines"][0]["prorated"], Value::Null);
}

/// Both ends of a month, on both sides of UTC. Baghdad's months begin three
/// hours before UTC's and a business five hours west begins its own five
/// hours after: a plan that runs from the 1st to the 1st is that month's and
/// nobody else's, a number is counted in the year the business is in, and a
/// due day is the business's day even where UTC has already left it.
#[tokio::test]
async fn a_month_ends_where_the_business_s_next_one_begins_east_and_west_of_utc() {
    for (offset, zone, currency, price) in [
        (180, "+03:00", "IQD", 50_000),
        (-300, "-05:00", "USD", 2_500),
    ] {
        let local = |date_and_time: &str| at(&format!("{date_and_time}{zone}"));
        let isp = Isp::start(false, &[]).await;
        set_calendar(&isp, offset).await;
        let tariff = isp
            .tariff("Home 10", 10, "flat", price, currency, "monthly")
            .await;

        // October and nothing else: from the 1st, ended on 1 November.
        let customer = isp.customer("One Month", currency, json!({})).await;
        let plan = isp
            .subscribe(&customer, &tariff, Some(local("2025-10-01T00:00:00")))
            .await;
        isp.ok(
            "isp.subscription.end",
            json!({"id": plan, "ended_at": local("2025-11-01T00:00:00")}),
        )
        .await;
        for month in ["2025-09", "2025-11"] {
            let run = isp.ok("isp.invoice.run", json!({"month": month})).await;
            assert!(
                list(&run["invoices"]).is_empty(),
                "UTC{zone}: billed in {month} for a plan that ran in October only: {run:#}"
            );
        }
        let october = isp
            .ok(
                "isp.invoice.run",
                json!({"month": "2025-10", "dry_run": false}),
            )
            .await;
        assert_eq!(
            (&october["period_start"], &october["period_end"]),
            (
                &json!(local("2025-10-01T00:00:00")),
                &json!(local("2025-11-01T00:00:00"))
            ),
            "UTC{zone}"
        );
        assert_eq!(
            october["totals"][0]["minor"],
            json!(price),
            "UTC{zone}: {october:#}"
        );
        let invoice = isp
            .ok(
                "isp.invoice.get",
                json!({"id": october["invoices"][0]["invoice_id"]}),
            )
            .await;
        assert_eq!(invoice["lines"][0]["prorated"], Value::Null, "UTC{zone}");
        assert_eq!(invoice["utc_offset_minutes"], json!(offset));
        // The month's own bounds list it, and the months beside it do not.
        for (since, until, expected) in [
            ("2025-10-01T00:00:00", "2025-11-01T00:00:00", 1),
            ("2025-09-01T00:00:00", "2025-10-01T00:00:00", 0),
            ("2025-11-01T00:00:00", "2025-12-01T00:00:00", 0),
        ] {
            let listed = list(
                &isp.ok(
                    "isp.invoices",
                    json!({"since": local(since), "until": local(until)}),
                )
                .await,
            );
            assert_eq!(listed.len(), expected, "UTC{zone}: {since} to {until}");
        }

        // From the 16th: sixteen of October's thirty-one days, whichever
        // side of UTC those days are counted on.
        let half = isp.customer("Half Month", currency, json!({})).await;
        isp.subscribe(&half, &tariff, Some(local("2025-10-16T00:00:00")))
            .await;
        let part = isp
            .ok(
                "isp.invoice.draft",
                json!({"customer_id": half, "month": "2025-10"}),
            )
            .await;
        let line = &part["invoice"]["lines"][0];
        assert_eq!(
            (
                &line["prorated"]["covered_days"],
                &line["prorated"]["period_days"]
            ),
            (&json!(16), &json!(31)),
            "UTC{zone}: {line:#}"
        );
        // 50,000 × 16 ÷ 31 = 25,806.45 dinars; 25.00 × 16 ÷ 31 = 12.903 dollars.
        let expected = if currency == "IQD" { 25_806 } else { 1_290 };
        assert_eq!(
            part["invoice"]["total"]["minor"],
            json!(expected),
            "UTC{zone}"
        );

        // The last and the first hour of a year. In Baghdad 01:00 on
        // 1 January is still the old year in UTC; five hours west, 22:00 on
        // 31 December is already the new one there.
        let old = isp.customer("Year End", currency, json!({})).await;
        isp.subscribe(&old, &tariff, Some(local("2024-01-01T00:00:00")))
            .await;
        let before_midnight = isp
            .invoice(
                &old,
                (local("2024-11-01T00:00:00"), local("2024-12-01T00:00:00")),
                Some(local("2024-12-31T22:00:00")),
                None,
            )
            .await;
        assert_eq!(before_midnight["number"], json!("2024-0001"), "UTC{zone}");
        let after_midnight = isp
            .invoice(
                &old,
                (local("2024-12-01T00:00:00"), local("2025-01-01T00:00:00")),
                Some(local("2025-01-01T01:00:00")),
                None,
            )
            .await;
        assert_eq!(after_midnight["number"], json!("2025-0001"), "UTC{zone}");

        // A due date is compared with the day of issue as the business
        // counts days. Issued half an hour into 1 November and due at eight
        // the evening before is due the day before, and refused — though in
        // UTC, in both of these calendars, the two are the same day.
        let draft = isp
            .ok(
                "isp.invoice.draft",
                json!({"customer_id": old, "month": "2025-09"}),
            )
            .await["invoice"]["id"]
            .clone();
        let err = isp
            .err(
                "isp.invoice.issue",
                json!({"id": draft, "issued_at": local("2025-11-01T00:30:00"),
                       "due_at": local("2025-10-31T20:00:00")}),
            )
            .await;
        assert_eq!(err.code, "invalid", "UTC{zone}: {err}");
        assert!(
            err.message.contains("2025-11-01") && err.message.contains("2025-10-31"),
            "the refusal names the business's days: {}",
            err.message
        );
        // Issued half an hour before midnight on the 31st and due on the
        // 31st, typed as its first moment, is due the day it is issued, and
        // allowed — though in UTC, in both calendars, that moment is the day
        // before the issue.
        let issued = isp
            .ok(
                "isp.invoice.issue",
                json!({"id": draft, "issued_at": local("2025-10-31T23:30:00"),
                       "due_at": local("2025-10-31T00:00:00")}),
            )
            .await;
        assert_eq!(issued["due_at"], json!(local("2025-10-31T00:00:00")));
    }
}

#[tokio::test]
async fn a_period_is_named_once_and_a_month_has_to_be_one() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Once Only", "USD", json!({})).await;
    for params in [
        json!({"customer_id": customer}),
        json!({"customer_id": customer, "month": "2025-10", "period_start": 0, "period_end": 1}),
        json!({"customer_id": customer, "period_start": 0}),
        json!({"customer_id": customer, "month": "2025-13"}),
        json!({"customer_id": customer, "month": "October"}),
        json!({"customer_id": customer, "month": "2025-1"}),
    ] {
        let err = isp.err("isp.invoice.draft", params.clone()).await;
        assert_eq!(err.code, "invalid", "{params}: {err}");
    }
    let err = isp.err("isp.invoice.run", json!({})).await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("month"), "{}", err.message);
    // A period that is not one is the run's mistake, said once — not a list
    // of every customer left out for a reason that is nothing to do with them.
    let err = isp
        .err(
            "isp.invoice.run",
            json!({"period_start": 10, "period_end": 5}),
        )
        .await;
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("ends after it starts"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn a_number_takes_its_year_from_the_business_calendar() {
    let isp = Isp::start(false, &[]).await;
    set_calendar(&isp, 180).await;
    let customer = isp.customer("New Year", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, Some(at("2024-01-01T00:00:00+03:00")))
        .await;
    // One in the morning on New Year's Day in Baghdad, which UTC still calls
    // the old year.
    let first_hour = at("2025-01-01T01:00:00+03:00");
    let issued = isp
        .invoice(
            &customer,
            (
                at("2024-12-01T00:00:00+03:00"),
                at("2025-01-01T00:00:00+03:00"),
            ),
            Some(first_hour),
            None,
        )
        .await;
    assert_eq!(
        issued["number"],
        json!("2025-0001"),
        "an invoice issued on 1 January is numbered in the new year"
    );
    let credited = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "amount": {"currency": "USD", "minor": 100},
                "reason": "a late start",
                "at": first_hour,
            }),
        )
        .await;
    assert_eq!(credited["credit"]["number"], json!("CN-2025-0001"));
}

#[tokio::test]
async fn an_invoice_is_overdue_once_its_due_day_is_over_and_every_figure_agrees() {
    let isp = Isp::start(false, &[]).await;
    let today = midday(&isp).await;

    // Due today: one that fell due this morning, one that falls due tonight.
    // Neither is late until the day is out.
    let (_, this_morning) = invoice_due(&isp, "Due This Morning", today + 1).await;
    let (_, tonight) = invoice_due(&isp, "Due Tonight", now() + 11 * 3_600_000).await;
    for id in [&this_morning, &tonight] {
        let view = isp.ok("isp.invoice.get", json!({"id": id})).await;
        assert_eq!(
            (&view["status"], &view["days_overdue"]),
            (&json!("issued"), &Value::Null),
            "an invoice is not overdue on the day it falls due"
        );
    }
    let arrears = isp.ok("isp.arrears", json!({})).await;
    let usd = &arrears["by_currency"][0];
    assert_eq!(
        (&usd["overdue"]["minor"], &usd["not_yet_due"]["minor"]),
        (&json!(0), &json!(20_000)),
        "the arrears report and the invoices disagree about what is late: {usd:#}"
    );
    assert_eq!(usd["buckets"][0]["invoices"], json!(2), "{usd:#}");
    for row in list(&arrears["customers"]) {
        assert_eq!(row["oldest_days"], Value::Null, "{row:#}");
    }
    let summary = isp.ok("isp.summary", json!({})).await;
    assert_eq!(summary["invoices"]["overdue"], json!(0));

    // Due yesterday, at the last moment of it and at the first: a day late
    // either way, and the invoice and the report say the same number.
    let (last_moment, _) = invoice_due(&isp, "Due Last Night", today - 1).await;
    let (first_moment, first_id) = invoice_due(&isp, "Due Yesterday", today - DAY_MS).await;
    let (three_days, three_id) = invoice_due(&isp, "Due Earlier", today - 2 * DAY_MS - 5).await;
    for (id, days) in [(&first_id, 1), (&three_id, 3)] {
        let view = isp.ok("isp.invoice.get", json!({"id": id})).await;
        assert_eq!(
            (&view["status"], &view["days_overdue"]),
            (&json!("overdue"), &json!(days))
        );
    }
    let arrears = isp.ok("isp.arrears", json!({})).await;
    let usd = &arrears["by_currency"][0];
    assert_eq!(usd["overdue"]["minor"], json!(30_000), "{usd:#}");
    assert_eq!(usd["not_yet_due"]["minor"], json!(20_000), "{usd:#}");
    assert_eq!(usd["buckets"][1]["label"], json!("1–30 days"));
    assert_eq!(usd["buckets"][1]["invoices"], json!(3), "{usd:#}");
    let oldest = |customer: &str| {
        list(&arrears["customers"])
            .into_iter()
            .find(|row| row["customer_id"] == json!(customer))
            .unwrap_or_else(|| panic!("{customer} is not in the report"))["oldest_days"]
            .clone()
    };
    assert_eq!(oldest(&last_moment), json!(1));
    assert_eq!(oldest(&first_moment), json!(1));
    assert_eq!(oldest(&three_days), json!(3));
    let summary = isp.ok("isp.summary", json!({})).await;
    assert_eq!(summary["invoices"]["overdue"], json!(3));
}

#[tokio::test]
async fn an_invoice_may_fall_due_on_the_day_it_is_issued_but_not_the_day_before() {
    let isp = Isp::start(false, &[]).await;
    let today = midday(&isp).await;
    let customer = isp.customer("Cash On The Day", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let draft = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": now() - 70 * DAY_MS,
                   "period_end": now() - 40 * DAY_MS}),
        )
        .await;
    let id = draft["invoice"]["id"].clone();

    // A due date is a day. Yesterday is before the day of issue…
    let err = isp
        .err("isp.invoice.issue", json!({"id": id, "due_at": today - 1}))
        .await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("due"), "{}", err.message);
    // …and today is not, though its first moment is hours behind us.
    let issued = isp
        .ok("isp.invoice.issue", json!({"id": id, "due_at": today}))
        .await;
    assert_eq!(issued["due_at"], json!(today));
    let view = isp.ok("isp.invoice.get", json!({"id": id})).await;
    assert_eq!(view["status"], json!("issued"));
}

#[tokio::test]
async fn a_window_that_overlaps_one_already_billed_is_refused_however_it_is_shifted() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Shifted", "USD", json!({})).await;
    let monthly = isp
        .tariff("Monthly", 10, "flat", 10_000, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &monthly, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let october = (at("2025-10-01T00:00:00Z"), at("2025-11-01T00:00:00Z"));
    isp.invoice(&customer, october, None, None).await;

    // A millisecond, a day or a fortnight out, it is still October's days.
    for shift in [1, -1, DAY_MS, -DAY_MS, 15 * DAY_MS] {
        let err = isp
            .err(
                "isp.invoice.draft",
                json!({"customer_id": customer, "period_start": october.0 + shift,
                       "period_end": october.1 + shift}),
            )
            .await;
        assert_eq!(err.code, "conflict", "shifted by {shift} ms: {err}");
        assert!(err.message.contains("already has"), "{}", err.message);
    }
    // A run says so for each customer rather than billing them again.
    let run = isp
        .ok(
            "isp.invoice.run",
            json!({"period_start": october.0 + DAY_MS, "period_end": october.1 + DAY_MS,
                   "dry_run": false}),
        )
        .await;
    assert_eq!(run["drafted"], json!(0), "{run:#}");
    assert!(
        list(&run["skipped"])[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("already has"),
        "{run:#}"
    );

    // The months either side are other days, and are billed.
    for month in ["2025-09", "2025-11", "2025-12"] {
        let drafted = isp
            .ok(
                "isp.invoice.draft",
                json!({"customer_id": customer, "month": month}),
            )
            .await;
        assert_eq!(drafted["invoice"]["total"]["minor"], json!(10_000));
    }

    // A quarter overlaps its three months, and that is not billing anything
    // twice: a plan is only ever charged in a window of its own kind. The
    // quarterly plan is billed for the quarter and the monthly one is left
    // out of it, with the reason.
    let quarterly = isp
        .tariff("Quarterly", 50, "flat", 90_000, "USD", "quarterly")
        .await;
    isp.subscribe(&customer, &quarterly, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let quarter = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": at("2025-10-01T00:00:00Z"),
                   "period_end": at("2026-01-01T00:00:00Z")}),
        )
        .await;
    assert_eq!(quarter["invoice"]["total"]["minor"], json!(90_000));
    assert_eq!(list(&quarter["invoice"]["lines"]).len(), 1);
    assert_eq!(list(&quarter["skipped"]).len(), 1);
    // And the same quarter, a day out, is the same quarter.
    let err = isp
        .err(
            "isp.invoice.draft",
            json!({"customer_id": customer,
                   "period_start": at("2025-10-02T00:00:00Z"),
                   "period_end": at("2026-01-02T00:00:00Z")}),
        )
        .await;
    assert_eq!(err.code, "conflict");
}

#[tokio::test]
async fn changing_the_calendar_neither_bills_a_month_again_nor_blocks_the_next() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Moved Clocks", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    // October, billed while the calendar was still UTC.
    isp.invoice(
        &customer,
        (at("2025-10-01T00:00:00Z"), at("2025-11-01T00:00:00Z")),
        None,
        None,
    )
    .await;

    // The business puts its calendar right. October has different
    // milliseconds now and is still October, already billed.
    set_calendar(&isp, 180).await;
    let err = isp
        .err(
            "isp.invoice.draft",
            json!({"customer_id": customer,
                   "period_start": at("2025-10-01T00:00:00+03:00"),
                   "period_end": at("2025-11-01T00:00:00+03:00")}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("already has"), "{}", err.message);

    // November's first three hours were, in milliseconds, inside the old
    // October. They are not October's days, and November is billed in full.
    let november = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "month": "2025-11"}),
        )
        .await;
    assert_eq!(november["invoice"]["total"]["minor"], json!(2_500));
    assert_eq!(november["invoice"]["lines"][0]["prorated"], Value::Null);
    // The earlier invoice still says which calendar it was written in.
    let invoices = list(
        &isp.ok("isp.invoices", json!({"customer_id": customer}))
            .await,
    );
    let offsets: Vec<&Value> = invoices
        .iter()
        .map(|invoice| &invoice["utc_offset_minutes"])
        .collect();
    assert_eq!(offsets, [&json!(180), &json!(0)]);
}

#[tokio::test]
async fn a_month_of_invoices_is_that_month_and_not_the_one_before_as_well() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Month By Month", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let months = [
        (at("2025-09-01T00:00:00Z"), at("2025-10-01T00:00:00Z")),
        (at("2025-10-01T00:00:00Z"), at("2025-11-01T00:00:00Z")),
        (at("2025-11-01T00:00:00Z"), at("2025-12-01T00:00:00Z")),
    ];
    for month in months {
        isp.invoice(&customer, month, None, None).await;
    }
    let october = months[1];
    let listed = list(
        &isp.ok(
            "isp.invoices",
            json!({"since": october.0, "until": october.1}),
        )
        .await,
    );
    let periods: Vec<(i64, i64)> = listed
        .iter()
        .map(|invoice| {
            (
                invoice["period_start"].as_i64().expect("a start"),
                invoice["period_end"].as_i64().expect("an end"),
            )
        })
        .collect();
    assert_eq!(
        periods,
        [october],
        "a period ends where the next begins, so October's filter is October's invoices"
    );
}
