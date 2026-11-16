//! The ledger: what happens to an invoice, and what must not.

mod common;

use common::{DAY_MS, Isp, last_month, list};
use serde_json::json;

fn now() -> i64 {
    nocturne_app::util::now_ms()
}

#[tokio::test]
async fn an_issued_invoice_survives_a_tariff_change_unchanged() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Hilltop Hotel", "USD", json!({})).await;
    // $1.00 per megabit, ten megabits: $10.00 a month.
    let tariff = isp
        .tariff("Business 10", 10, "per_mbit", 100, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;

    let first = isp
        .invoice(
            &customer,
            (now() - 62 * DAY_MS, now() - 31 * DAY_MS),
            Some(now() - 31 * DAY_MS),
            None,
        )
        .await;
    let first_id = first["id"].as_str().expect("an id").to_string();
    assert_eq!(first["total"]["minor"], json!(1_000));
    assert_eq!(first["lines"][0]["unit_price"]["minor"], json!(100));
    assert_eq!(first["lines"][0]["mbit"], json!(10));

    // The tariff doubles.
    let raised = isp
        .ok(
            "isp.tariff.save",
            json!({"id": tariff, "price": {"currency": "USD", "minor": 200}}),
        )
        .await;
    assert_eq!(raised["period_price"]["minor"], json!(2_000));
    assert_eq!(
        raised["subscriptions_unaffected"],
        json!(1),
        "the save says how many agreed figures it did not move"
    );
    let tariffs = list(&isp.ok("isp.tariffs", json!({})).await);
    assert_eq!(tariffs[0]["period_price"]["minor"], json!(2_000));
    assert_eq!(tariffs[0]["active_subscriptions"], json!(1));

    // The issued invoice says exactly what it said.
    let again = isp.ok("isp.invoice.get", json!({"id": first_id})).await;
    assert_eq!(again["total"]["minor"], json!(1_000));
    assert_eq!(again["lines"][0]["unit_price"]["minor"], json!(100));
    assert_eq!(again["lines"], first["lines"]);
    assert_eq!(again["number"], first["number"]);

    // And so does the subscription, which is why the next invoice is $10 too.
    let subscriptions = list(&isp.ok("isp.subscriptions", json!({})).await);
    assert_eq!(subscriptions[0]["unit_price"]["minor"], json!(100));
    assert_eq!(subscriptions[0]["price"]["minor"], json!(1_000));
    let second = isp.invoice(&customer, last_month(), None, None).await;
    assert_eq!(
        second["total"]["minor"],
        json!(1_000),
        "a tariff change is not a price rise for somebody already on it"
    );

    // Raising what *this* customer pays is an amendment: the old terms end
    // and new ones start, and last month's invoice is still last month's.
    let amended = isp
        .ok(
            "isp.subscription.amend",
            json!({
                "id": subscriptions[0]["id"],
                "price": {"currency": "USD", "minor": 200},
                "from_at": now() - DAY_MS,
                "note": "renewal",
            }),
        )
        .await;
    assert_eq!(amended["subscription"]["price"]["minor"], json!(2_000));
    assert_eq!(amended["ended"]["price"]["minor"], json!(1_000));
    assert_eq!(amended["ended"]["state"], json!("ended"));
    assert_eq!(
        amended["subscription"]["amends"], amended["ended"]["id"],
        "the new terms say which they replaced"
    );
    let unchanged = isp.ok("isp.invoice.get", json!({"id": first_id})).await;
    assert_eq!(unchanged["total"]["minor"], json!(1_000));
}

#[tokio::test]
async fn an_issued_invoice_is_voided_and_reissued_rather_than_corrected() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Souq Electronics", "USD", json!({})).await;
    let tariff = isp
        .tariff("Shop 20", 20, "flat", 4_000, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let invoice = isp.invoice(&customer, last_month(), None, None).await;
    let id = invoice["id"].as_str().expect("an id").to_string();
    let number = invoice["number"].as_str().expect("a number").to_string();
    assert!(number.contains('-'), "numbered at issue: {number}");

    // Every kind of edit is refused, and the refusal says what to do instead.
    let err = isp
        .err(
            "isp.invoice.line.add",
            json!({"invoice_id": id, "description": "oops",
                   "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(
        err.message.contains("void it and issue a new one"),
        "{}",
        err.message
    );
    assert_eq!(
        isp.err(
            "isp.invoice.line.remove",
            json!({"invoice_id": id, "line_id": invoice["lines"][0]["id"]}),
        )
        .await
        .code,
        "conflict"
    );
    assert_eq!(
        isp.err("isp.invoice.issue", json!({"id": id})).await.code,
        "conflict"
    );
    // Voiding needs a reason: it is what the trail will say happened.
    assert_eq!(
        isp.err("isp.invoice.void", json!({"id": id})).await.code,
        "invalid"
    );

    let reissued = isp
        .ok(
            "isp.invoice.reissue",
            json!({"id": id, "reason": "charged for 20 Mbit, they bought 10"}),
        )
        .await;
    assert_eq!(reissued["voided"]["state"], json!("void"));
    assert_eq!(
        reissued["voided"]["number"],
        json!(number),
        "a void invoice keeps its number: the trail has to show it existed"
    );
    assert_eq!(reissued["draft"]["state"], json!("draft"));
    assert!(
        reissued["draft"]["number"].is_null(),
        "a draft has no number"
    );
    assert_eq!(reissued["draft"]["replaces"], json!(id));
    assert_eq!(reissued["voided"]["replaced_by"], reissued["draft"]["id"]);
    assert_eq!(
        reissued["draft"]["total"]["minor"],
        json!(4_000),
        "the lines came over so the correction can be made"
    );

    // The replacement is a draft, so it can be corrected, and its number is
    // its own when it is issued.
    let draft_id = reissued["draft"]["id"].as_str().expect("an id").to_string();
    isp.ok(
        "isp.invoice.line.remove",
        json!({"invoice_id": draft_id, "line_id": reissued["draft"]["lines"][0]["id"]}),
    )
    .await;
    let fixed = isp
        .ok(
            "isp.invoice.line.add",
            json!({"invoice_id": draft_id, "description": "Shop 10 — 10 Mbit/s",
                   "amount": {"currency": "USD", "minor": 2_000}}),
        )
        .await;
    assert_eq!(fixed["total"]["minor"], json!(2_000));
    let issued = isp.ok("isp.invoice.issue", json!({"id": draft_id})).await;
    assert_ne!(issued["number"], json!(number), "a number is never reused");

    // The voided one is still there to be read.
    let voided = isp.ok("isp.invoice.get", json!({"id": id})).await;
    assert_eq!(voided["status"], json!("void"));
    assert_eq!(voided["total"]["minor"], json!(4_000));
    assert!(
        voided["void_reason"]
            .as_str()
            .unwrap_or_default()
            .contains("they bought 10")
    );
}

#[tokio::test]
async fn nothing_marks_an_invoice_paid_except_a_recorded_payment() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Lake View Resort", "USD", json!({})).await;
    let tariff = isp
        .tariff("Resort 50", 50, "flat", 10_000, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let invoice = isp
        .invoice(&customer, last_month(), None, Some(now() + 14 * DAY_MS))
        .await;
    let id = invoice["id"].as_str().expect("an id").to_string();

    let fresh = isp.ok("isp.invoice.get", json!({"id": id})).await;
    assert_eq!(fresh["status"], json!("issued"));
    assert_eq!(fresh["paid"]["minor"], json!(0));
    assert_eq!(fresh["outstanding"]["minor"], json!(10_000));
    assert!(fresh["days_overdue"].is_null());

    // Part of it.
    let part = isp
        .ok(
            "isp.payment.record",
            json!({"customer_id": customer, "invoice_id": id, "method": "cash",
                   "amount": {"currency": "USD", "minor": 4_000}}),
        )
        .await;
    assert_eq!(part["invoice"]["status"], json!("part_paid"));
    assert_eq!(part["invoice"]["outstanding"]["minor"], json!(6_000));

    // The rest.
    let rest = isp
        .ok(
            "isp.payment.record",
            json!({"customer_id": customer, "invoice_id": id, "method": "bank transfer",
                   "reference": "IBAN-77", "amount": {"currency": "USD", "minor": 6_000}}),
        )
        .await;
    assert_eq!(rest["invoice"]["status"], json!("paid"));
    assert_eq!(rest["invoice"]["outstanding"]["minor"], json!(0));
    assert_eq!(list(&rest["invoice"]["payments"]).len(), 2);

    // An invoice with money against it cannot be voided: that would hide it.
    let err = isp
        .err("isp.invoice.void", json!({"id": id, "reason": "mistake"}))
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("actually received"), "{}", err.message);

    // Voiding the payment puts the invoice back to owing — nothing else can.
    let payment_id = part["payment"]["id"].as_str().expect("an id").to_string();
    let voided = isp
        .ok(
            "isp.payment.void",
            json!({"id": payment_id, "reason": "recorded against the wrong account"}),
        )
        .await;
    assert_eq!(voided["state"], json!("void"));
    let after = isp.ok("isp.invoice.get", json!({"id": id})).await;
    assert_eq!(after["paid"]["minor"], json!(6_000));
    assert_eq!(after["outstanding"]["minor"], json!(4_000));
    assert_eq!(after["status"], json!("part_paid"));
    assert_eq!(
        list(&after["payments"]).len(),
        2,
        "the void payment is still in the trail"
    );
    assert_eq!(
        isp.err(
            "isp.payment.void",
            json!({"id": payment_id, "reason": "again"})
        )
        .await
        .code,
        "conflict"
    );

    // Money cannot be taken against a draft or a void invoice.
    let draft = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": now() - 200 * DAY_MS,
                   "period_end": now() - 169 * DAY_MS}),
        )
        .await;
    let err = isp
        .err(
            "isp.payment.record",
            json!({"customer_id": customer, "invoice_id": draft["invoice"]["id"],
                   "method": "cash", "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("still a draft"), "{}", err.message);
    // A payment with no invoice is money on account, and shows as such.
    let on_account = isp
        .ok(
            "isp.payment.record",
            json!({"customer_id": customer, "method": "cash",
                   "amount": {"currency": "USD", "minor": 500}}),
        )
        .await;
    assert!(on_account["invoice"].is_null());
    let detail = isp.ok("isp.customer.get", json!({"id": customer})).await;
    assert_eq!(detail["balance"]["on_account"][0]["minor"], json!(500));
    assert_eq!(detail["balance"]["outstanding"][0]["minor"], json!(4_000));
}

#[tokio::test]
async fn billing_the_same_period_twice_is_refused() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Twice Shy", "USD", json!({})).await;
    let tariff = isp.tariff("Plan", 5, "flat", 500, "USD", "monthly").await;
    isp.subscribe(&customer, &tariff, None).await;
    let period = last_month();
    let first = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": period.0, "period_end": period.1}),
        )
        .await;
    let err = isp
        .err(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": period.0, "period_end": period.1}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("already has"), "{}", err.message);

    // A billing run skips them rather than billing them twice, and says why.
    let run = isp
        .ok(
            "isp.invoice.run",
            json!({"period_start": period.0, "period_end": period.1}),
        )
        .await;
    assert_eq!(run["dry_run"], json!(true), "a run previews by default");
    assert_eq!(run["drafted"], json!(0));
    assert_eq!(list(&run["skipped"]).len(), 1);
    assert!(
        list(&run["skipped"])[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("already has")
    );

    // Once the wrong one is void, the period can be billed again.
    isp.ok(
        "isp.invoice.void",
        json!({"id": first["invoice"]["id"], "reason": "wrong period"}),
    )
    .await;
    let run = isp
        .ok(
            "isp.invoice.run",
            json!({"period_start": period.0, "period_end": period.1, "dry_run": false,
                   "issue": true}),
        )
        .await;
    assert_eq!(
        (run["drafted"].as_u64(), run["issued"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(list(&run["totals"]).len(), 1);
    assert_eq!(list(&run["totals"])[0]["minor"], json!(500));
}

#[tokio::test]
async fn a_part_period_states_the_arithmetic_and_an_odd_window_is_not_guessed_at() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Mid Month", "USD", json!({})).await;
    let tariff = isp
        .tariff("Monthly 10", 10, "flat", 3_100, "USD", "monthly")
        .await;
    // A 31-day period, joined on day 21: ten days of it.
    let period = (now() - 31 * DAY_MS, now());
    isp.subscribe(&customer, &tariff, Some(period.1 - 10 * DAY_MS))
        .await;
    let invoice = isp.invoice(&customer, period, None, None).await;
    let line = &invoice["lines"][0];
    assert_eq!(
        line["amount"]["minor"],
        json!(1_000),
        "3,100 cents for 31 days is 100 a day, and ten days is 1,000"
    );
    assert_eq!(line["full_period_price"]["minor"], json!(3_100));
    assert_eq!(line["prorated"]["covered_days"], json!(10));
    assert_eq!(line["prorated"]["period_days"], json!(31));
    assert_eq!(line["prorated"]["exact"], json!(true));
    assert!(
        line["prorated"]["basis"]
            .as_str()
            .unwrap_or_default()
            .contains("of the period's"),
        "{}",
        line["prorated"]["basis"]
    );
    assert!(
        line["description"]
            .as_str()
            .unwrap_or_default()
            .contains("part period")
    );

    // A week is not a month, and nothing works out what a seventh of a
    // monthly plan costs on its own: the subscription is skipped, with its
    // reason, and the draft refuses to be an invoice for nothing.
    let err = isp
        .err(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": now() - 7 * DAY_MS,
                   "period_end": now()}),
        )
        .await;
    assert!(err.message.contains("skipped"), "{}", err.message);
    let drafted = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": now() - 7 * DAY_MS,
                   "period_end": now(), "allow_empty": true}),
        )
        .await;
    let skipped = list(&drafted["skipped"]);
    assert_eq!(skipped.len(), 1);
    assert!(
        skipped[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("add the figure by hand"),
        "{}",
        skipped[0]["reason"]
    );
    assert_eq!(drafted["invoice"]["total"]["minor"], json!(0));
    assert!(
        !list(&drafted["warnings"]).is_empty(),
        "the draft says something was left out"
    );
    // An empty invoice is never issued.
    assert_eq!(
        isp.err("isp.invoice.issue", json!({"id": drafted["invoice"]["id"]}))
            .await
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn arrears_are_aged_per_currency_and_say_who_owes_what() {
    let isp = Isp::start(false, &[]).await;
    let one = isp.customer("Late Larry", "USD", json!({})).await;
    let two = isp.customer("Slow Sam", "USD", json!({})).await;
    let three = isp.customer("Dinar Dawn", "IQD", json!({})).await;
    let usd = isp
        .tariff("USD 10", 10, "flat", 1_000, "USD", "monthly")
        .await;
    let iqd = isp
        .tariff("IQD 10", 10, "flat", 15_000, "IQD", "monthly")
        .await;
    isp.subscribe(&one, &usd, None).await;
    isp.subscribe(&two, &usd, None).await;
    isp.subscribe(&three, &iqd, None).await;

    // Larry is 70 days late, Sam 10 days late, Dawn is not due yet.
    isp.invoice(
        &one,
        (now() - 120 * DAY_MS, now() - 89 * DAY_MS),
        Some(now() - 89 * DAY_MS),
        Some(now() - 70 * DAY_MS),
    )
    .await;
    isp.invoice(
        &two,
        (now() - 62 * DAY_MS, now() - 31 * DAY_MS),
        Some(now() - 31 * DAY_MS),
        Some(now() - 10 * DAY_MS),
    )
    .await;
    isp.invoice(&three, last_month(), None, Some(now() + 10 * DAY_MS))
        .await;

    let arrears = isp.ok("isp.arrears", json!({})).await;
    let by_currency = list(&arrears["by_currency"]);
    assert_eq!(by_currency.len(), 2);
    let usd_row = by_currency
        .iter()
        .find(|row| row["currency"] == json!("USD"))
        .expect("dollars are owed");
    assert_eq!(usd_row["total"]["minor"], json!(2_000));
    assert_eq!(usd_row["overdue"]["minor"], json!(2_000));
    assert_eq!(usd_row["not_yet_due"]["minor"], json!(0));
    assert_eq!(usd_row["customers"], json!(2));
    let buckets: Vec<(&str, i64)> = list(&usd_row["buckets"])
        .iter()
        .map(|bucket| {
            (
                bucket["label"].as_str().unwrap_or_default().to_string(),
                bucket["amount"]["minor"].as_i64().unwrap_or_default(),
            )
        })
        .map(|(label, minor)| (Box::leak(label.into_boxed_str()) as &str, minor))
        .collect();
    assert_eq!(
        buckets,
        [
            ("not yet due", 0),
            ("1–30 days", 1_000),
            ("31–60 days", 0),
            ("61–90 days", 1_000),
            ("over 90 days", 0),
        ],
        "aged into the settings' bands"
    );
    let dinars = by_currency
        .iter()
        .find(|row| row["currency"] == json!("IQD"))
        .expect("dinars are owed");
    assert_eq!(dinars["not_yet_due"]["minor"], json!(15_000));
    assert_eq!(dinars["overdue"]["minor"], json!(0));

    // Who owes what, worst first inside each currency.
    let customers = list(&arrears["customers"]);
    assert_eq!(customers.len(), 3);
    let larry = customers
        .iter()
        .find(|row| row["customer_id"] == json!(one))
        .expect("Larry owes");
    assert_eq!(larry["oldest_days"], json!(70));
    assert_eq!(larry["customer_name"], json!("Late Larry"));
    let dawn = customers
        .iter()
        .find(|row| row["customer_id"] == json!(three))
        .expect("Dawn owes");
    assert!(dawn["oldest_days"].is_null(), "nothing of hers is due yet");

    // One customer's arrears alone.
    let just_larry = isp.ok("isp.arrears", json!({"customer_id": one})).await;
    assert_eq!(list(&just_larry["customers"]).len(), 1);
    assert_eq!(list(&just_larry["by_currency"]).len(), 1);

    // And the summary counts the overdue ones without naming anybody.
    let summary = isp.ok("isp.summary", json!({})).await;
    assert_eq!(summary["invoices"]["issued"], json!(3));
    assert_eq!(summary["invoices"]["overdue"], json!(2));
}

#[tokio::test]
async fn a_customer_with_a_ledger_is_cancelled_rather_than_deleted() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp
        .customer("Gone Away", "USD", json!({"code": "G-9"}))
        .await;
    let tariff = isp.tariff("Plan", 5, "flat", 500, "USD", "monthly").await;
    isp.subscribe(&customer, &tariff, None).await;
    isp.invoice(&customer, last_month(), None, None).await;

    let err = isp
        .err("isp.customer.delete", json!({"id": customer}))
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("cancelled"), "{}", err.message);
    assert!(
        err.message.contains("ask about them"),
        "it says why: {}",
        err.message
    );

    let cancelled = isp
        .ok(
            "isp.customer.state",
            json!({"id": customer, "state": "cancelled", "reason": "moved away"}),
        )
        .await;
    assert_eq!(cancelled["customer"]["state"], json!("cancelled"));
    assert_eq!(
        cancelled["device_changed"],
        json!(false),
        "a customer's state is a record; nothing was sent to a device"
    );
    assert!(
        cancelled["note"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing was sent to a device")
    );

    // Their ledger is still there.
    let invoices = list(
        &isp.ok("isp.invoices", json!({"customer_id": customer}))
            .await,
    );
    assert_eq!(invoices.len(), 1);
    let arrears = isp.ok("isp.arrears", json!({})).await;
    assert_eq!(list(&arrears["customers"]).len(), 1);
    // And selling a cancelled customer a new plan is refused.
    assert_eq!(
        isp.err(
            "isp.subscription.create",
            json!({"customer_id": customer, "tariff_id": tariff}),
        )
        .await
        .code,
        "conflict"
    );

    // A customer with no ledger at all can be deleted.
    let empty = isp.customer("Never Billed", "USD", json!({})).await;
    let deleted = isp.ok("isp.customer.delete", json!({"id": empty})).await;
    assert_eq!(deleted["deleted"], json!(true));
    assert_eq!(
        isp.err("isp.customer.get", json!({"id": empty})).await.code,
        "not_found"
    );
}

#[tokio::test]
async fn a_customers_currency_cannot_be_changed_under_their_ledger() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Switcher", "USD", json!({})).await;
    // Before anything is billed, a mistake can be corrected.
    let fixed = isp
        .ok(
            "isp.customer.save",
            json!({"id": customer, "currency": "IQD"}),
        )
        .await;
    assert_eq!(fixed["customer"]["currency"], json!("IQD"));

    // A plan priced in another currency than the customer is billed in is
    // refused rather than converted.
    let usd = isp
        .tariff("USD plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    let err = isp
        .err(
            "isp.subscription.create",
            json!({"customer_id": customer, "tariff_id": usd}),
        )
        .await;
    assert!(
        err.message.contains("Nothing here converts between them"),
        "{}",
        err.message
    );

    let iqd = isp
        .tariff("IQD plan", 10, "flat", 33_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &iqd, None).await;
    isp.invoice(&customer, last_month(), None, None).await;
    let err = isp
        .err(
            "isp.customer.save",
            json!({"id": customer, "currency": "USD"}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(
        err.message.contains("ledger in two currencies"),
        "{}",
        err.message
    );
}
