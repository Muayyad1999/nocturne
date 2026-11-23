//! What a bill is made of, and what a billing run owes the person who
//! started it: everybody reached or named, every reason given, no figure
//! that could not have been meant, and no word that claims money arrived
//! when none did.

mod common;

use common::{DAY_MS, Isp, list};
use serde_json::{Value, json};

fn at(text: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap_or_else(|err| panic!("`{text}` is not a moment: {err}"))
        .timestamp_millis()
}

fn usd(minor: i64) -> Value {
    json!({"currency": "USD", "minor": minor})
}

/// November 2025 in UTC, which is the calendar these tests leave the
/// business on: thirty days.
fn november() -> (i64, i64) {
    (at("2025-11-01T00:00:00Z"), at("2025-12-01T00:00:00Z"))
}

fn october() -> (i64, i64) {
    (at("2025-10-01T00:00:00Z"), at("2025-11-01T00:00:00Z"))
}

/// `params` with a period in it.
fn over(period: (i64, i64), mut params: Value) -> Value {
    let fields = params.as_object_mut().expect("params are an object");
    fields.insert("period_start".to_string(), json!(period.0));
    fields.insert("period_end".to_string(), json!(period.1));
    params
}

#[tokio::test]
async fn a_run_reaches_every_active_customer_however_many_there_are() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 1_000, "USD", "monthly")
        .await;
    // More accounts than one page of customers holds. Only the three whose
    // names sort last are on a plan: they are the ones a run that stopped
    // at the end of a page would never have got to.
    for index in 0..2_000 {
        isp.customer(&format!("Account {index:04}"), "USD", json!({}))
            .await;
    }
    let mut last = Vec::new();
    for name in ["Zz One", "Zz Three", "Zz Two"] {
        let customer = isp.customer(name, "USD", json!({})).await;
        isp.subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
            .await;
        last.push(customer);
    }

    let preview = isp.ok("isp.invoice.run", over(november(), json!({}))).await;
    let billed: Vec<Value> = list(&preview["invoices"])
        .into_iter()
        .map(|row| row["customer_id"].clone())
        .collect();
    assert_eq!(
        billed,
        last.iter().map(|id| json!(id)).collect::<Vec<_>>(),
        "the customers at the end of the alphabet were not reached"
    );
    assert_eq!(
        list(&preview["invoices"]).len() + list(&preview["skipped"]).len(),
        2_003,
        "every customer is either billed or named with the reason they were not"
    );
    assert_eq!(preview["totals"][0]["minor"], json!(3_000));
    assert_eq!(preview["considered"], json!(2_003), "{}", preview["note"]);

    let run = isp
        .ok(
            "isp.invoice.run",
            over(november(), json!({"dry_run": false, "issue": true})),
        )
        .await;
    assert_eq!(
        (&run["considered"], &run["drafted"], &run["issued"]),
        (&json!(2_003), &json!(3), &json!(3))
    );
    assert_eq!(list(&run["skipped"]).len(), 2_000);
    assert!(list(&run["warnings"]).is_empty(), "{:#}", run["warnings"]);
}

#[tokio::test]
async fn a_customer_left_out_of_a_run_is_told_the_reason_and_not_a_count_of_reasons() {
    let isp = Isp::start(false, &[]).await;
    let yearly = isp
        .tariff("Yearly 50", 50, "flat", 120_000, "USD", "yearly")
        .await;
    let monthly = isp
        .tariff("Monthly 10", 10, "flat", 2_500, "USD", "monthly")
        .await;
    let on_a_yearly_plan = isp.customer("Yearly Co", "USD", json!({})).await;
    isp.subscribe(&on_a_yearly_plan, &yearly, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let not_started = isp.customer("Next Month Co", "USD", json!({})).await;
    isp.subscribe(&not_started, &monthly, Some(at("2025-12-01T00:00:00Z")))
        .await;

    let run = isp.ok("isp.invoice.run", over(november(), json!({}))).await;
    let reason_for = |customer: &str| -> Value {
        list(&run["skipped"])
            .into_iter()
            .find(|row| row["customer_id"] == json!(customer))
            .unwrap_or_else(|| panic!("{customer} is not among those left out: {run:#}"))
    };

    // Their plan is in force. It was not charged, and the run says why and
    // what to do about it.
    let yearly_row = reason_for(&on_a_yearly_plan);
    let reason = yearly_row["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("Yearly 50") && reason.contains("agreed yearly"),
        "the reason the plan was not charged is not in the result: {reason}"
    );
    assert!(
        !reason.contains("none of them in force"),
        "the plan is in force, and the run says it is not: {reason}"
    );
    assert_eq!(
        yearly_row["skipped_subscriptions"][0]["tariff_name"],
        json!("Yearly 50"),
        "{yearly_row:#}"
    );
    // Drafting by hand is refused in the same words.
    let err = isp
        .err(
            "isp.invoice.draft",
            over(november(), json!({"customer_id": on_a_yearly_plan})),
        )
        .await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("agreed yearly"), "{}", err.message);

    // A plan that had not begun is a different thing, and is said differently.
    let reason = reason_for(&not_started)["reason"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(reason.contains("none of them in force"), "{reason}");
    assert!(
        list(&reason_for(&not_started)["skipped_subscriptions"]).is_empty(),
        "nothing was skipped: nothing applied"
    );
}

#[tokio::test]
async fn the_parts_of_an_amended_plan_add_up_to_what_the_plan_costs() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Half Way", "USD", json!({})).await;
    // 25.01 a month: half of it is not a whole number of cents.
    let tariff = isp
        .tariff("Odd Price", 10, "flat", 2_501, "USD", "monthly")
        .await;
    let plan = isp
        .subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    // More megabits for the same money, from exactly half way through
    // November's thirty days.
    isp.ok(
        "isp.subscription.amend",
        json!({"id": plan, "mbit": 20, "from_at": at("2025-11-16T00:00:00Z")}),
    )
    .await;

    let drafted = isp
        .ok(
            "isp.invoice.draft",
            over(november(), json!({"customer_id": customer})),
        )
        .await;
    let lines = list(&drafted["invoice"]["lines"]);
    let mut amounts: Vec<i64> = lines
        .iter()
        .map(|line| line["amount"]["minor"].as_i64().expect("an amount"))
        .collect();
    amounts.sort_unstable();
    assert_eq!(
        (amounts, &drafted["invoice"]["total"]["minor"]),
        (vec![1_250, 1_251], &json!(2_501)),
        "the customer agreed 25.01 for the month and nothing about that changed"
    );
    for line in &lines {
        assert_eq!(line["prorated"]["exact"], json!(false));
        assert!(
            line["prorated"]["basis"]
                .as_str()
                .unwrap_or_default()
                .contains("rounded together"),
            "the line does not say how its figure was reached: {line:#}"
        );
    }

    // A change of price is two different agreements, and each part is its
    // own figure: there is no single price for them to add up to.
    let other = isp.customer("Price Rise", "USD", json!({})).await;
    let plan = isp
        .subscribe(&other, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    isp.ok(
        "isp.subscription.amend",
        json!({"id": plan, "price": usd(3_001), "from_at": at("2025-11-16T00:00:00Z")}),
    )
    .await;
    let drafted = isp
        .ok(
            "isp.invoice.draft",
            over(november(), json!({"customer_id": other})),
        )
        .await;
    let mut amounts: Vec<i64> = list(&drafted["invoice"]["lines"])
        .iter()
        .map(|line| line["amount"]["minor"].as_i64().expect("an amount"))
        .collect();
    amounts.sort_unstable();
    assert_eq!(amounts, [1_251, 1_501], "12.505 and 15.005, each rounded");

    // A price change and then more megabits at the new price, all in one
    // month: ten days at 25.01, then twenty at 30.03 in parts of five, five
    // and ten. The twenty days are one plan at one price and come to 20.02
    // between them — not the 20.03 that rounding 5.005 up twice would make.
    let both = isp.customer("Rise Then More", "USD", json!({})).await;
    let mut plan = json!(
        isp.subscribe(&both, &tariff, Some(at("2025-01-01T00:00:00Z")))
            .await
    );
    for (change, from_at) in [
        (json!({"price": usd(3_003)}), "2025-11-11T00:00:00Z"),
        (json!({"mbit": 20}), "2025-11-16T00:00:00Z"),
        (json!({"mbit": 30}), "2025-11-21T00:00:00Z"),
    ] {
        let mut params = change;
        params["id"] = plan;
        params["from_at"] = json!(at(from_at));
        plan = isp.ok("isp.subscription.amend", params).await["subscription"]["id"].clone();
    }
    let drafted = isp
        .ok(
            "isp.invoice.draft",
            over(november(), json!({"customer_id": both})),
        )
        .await;
    let mut amounts: Vec<i64> = list(&drafted["invoice"]["lines"])
        .iter()
        .map(|line| line["amount"]["minor"].as_i64().expect("an amount"))
        .collect();
    amounts.sort_unstable();
    assert_eq!(
        (amounts, &drafted["invoice"]["total"]["minor"]),
        (vec![500, 501, 834, 1_001], &json!(2_836)),
        "8.34 for the ten days at the old price and 20.02 for the twenty at the new"
    );
}

#[tokio::test]
async fn amended_and_ended_terms_obey_the_rules_new_terms_do() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Back Dated", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    let started = at("2025-06-01T00:00:00Z");
    let plan = isp.subscribe(&customer, &tariff, Some(started)).await;

    let err = isp
        .err(
            "isp.subscription.amend",
            json!({"id": plan, "price": usd(-500)}),
        )
        .await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("negative"), "{}", err.message);

    let err = isp
        .err(
            "isp.subscription.amend",
            json!({"id": plan, "mbit": 20, "from_at": started - DAY_MS}),
        )
        .await;
    assert_eq!(err.code, "invalid", "new terms from before the plan began");
    let err = isp
        .err(
            "isp.subscription.end",
            json!({"id": plan, "ended_at": started - 90 * DAY_MS}),
        )
        .await;
    assert_eq!(err.code, "invalid", "a plan that ended before it began");

    // None of the refusals changed anything.
    let plans = list(
        &isp.ok("isp.subscriptions", json!({"customer_id": customer}))
            .await,
    );
    assert_eq!(plans.len(), 1);
    assert_eq!(
        (&plans[0]["state"], &plans[0]["price"]["minor"]),
        (&json!("active"), &json!(2_500))
    );
    // The first moment of the plan is not before it: terms may be corrected
    // from the very start, and a plan that never ran may be taken back.
    let corrected = isp
        .ok(
            "isp.subscription.amend",
            json!({"id": plan, "mbit": 20, "from_at": started}),
        )
        .await;
    assert_eq!(corrected["ended"]["ended_at"], json!(started));
    assert_eq!(corrected["subscription"]["started_at"], json!(started));
    let ended = isp
        .ok(
            "isp.subscription.end",
            json!({"id": corrected["subscription"]["id"], "ended_at": started,
                   "reason": "never installed"}),
        )
        .await;
    assert_eq!(ended["ended_at"], json!(started));
}

/// A plan sold today to start on the first of next month has not begun.
/// "End it" and "change it", with no date given, cannot mean now — that is
/// before it starts — so they mean from its start: a plan that never ran,
/// or terms corrected before anybody was charged on the old ones.
#[tokio::test]
async fn a_plan_that_has_not_begun_is_ended_or_amended_from_its_start_when_no_date_is_given() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Next Month", "IQD", json!({})).await;
    let tariff = isp
        .tariff("Home", 10, "flat", 50_000, "IQD", "monthly")
        .await;
    let starts = nocturne_app::util::now_ms() + 10 * DAY_MS;
    let plan = isp.subscribe(&customer, &tariff, Some(starts)).await;

    let amended = isp
        .ok("isp.subscription.amend", json!({"id": plan, "mbit": 20}))
        .await;
    assert_eq!(
        (
            &amended["ended"]["ended_at"],
            &amended["subscription"]["started_at"]
        ),
        (&json!(starts), &json!(starts)),
        "the new terms replace the old from the day the plan begins"
    );
    let ended = isp
        .ok(
            "isp.subscription.end",
            json!({"id": amended["subscription"]["id"], "reason": "sold by mistake"}),
        )
        .await;
    assert_eq!(
        ended["ended_at"],
        json!(starts),
        "ended before it began, so it never ran: it ends where it would have started"
    );
    // Nothing of theirs covers any time at all, and nothing ends before it
    // started.
    for plan in list(
        &isp.ok("isp.subscriptions", json!({"customer_id": customer}))
            .await,
    ) {
        assert_eq!(
            (&plan["state"], &plan["started_at"], &plan["ended_at"]),
            (&json!("ended"), &json!(starts), &json!(starts))
        );
    }
    // A date that is given is still taken at its word, and refused.
    let other = isp.subscribe(&customer, &tariff, Some(starts)).await;
    for (method, params) in [
        (
            "isp.subscription.end",
            json!({"id": other, "ended_at": starts - 1}),
        ),
        (
            "isp.subscription.amend",
            json!({"id": other, "mbit": 20, "from_at": starts - 1}),
        ),
    ] {
        assert_eq!(isp.err(method, params).await.code, "invalid", "{method}");
    }
}

/// A plan ended at the moment it started never ran. It is not "in force" in
/// the month that moment falls in, whatever kind of plan it was, and a run
/// must not say it was.
#[tokio::test]
async fn a_plan_that_never_ran_is_not_one_the_run_could_not_charge() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Changed Their Mind", "USD", json!({})).await;
    let yearly = isp
        .tariff("Yearly 50", 50, "flat", 120_000, "USD", "yearly")
        .await;
    let never = at("2025-11-10T00:00:00Z");
    let plan = isp.subscribe(&customer, &yearly, Some(never)).await;
    isp.ok(
        "isp.subscription.end",
        json!({"id": plan, "ended_at": never, "reason": "never installed"}),
    )
    .await;

    let run = isp.ok("isp.invoice.run", over(november(), json!({}))).await;
    let row = &list(&run["skipped"])[0];
    let reason = row["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("none of them in force"),
        "a plan that never ran is said to have been in force: {reason}"
    );
    assert!(list(&row["skipped_subscriptions"]).is_empty(), "{row:#}");
    // A draft made by hand for the month does not list it either.
    let drafted = isp
        .ok(
            "isp.invoice.draft",
            over(
                november(),
                json!({"customer_id": customer, "allow_empty": true}),
            ),
        )
        .await;
    assert!(list(&drafted["skipped"]).is_empty(), "{drafted:#}");
    assert!(list(&drafted["invoice"]["lines"]).is_empty());
}

/// Once the first draft is written a run has to finish and say what it did.
/// A draft it cannot issue is not a reason to stop half way with some
/// customers billed and nobody told which.
#[tokio::test]
async fn a_draft_the_run_cannot_issue_stays_a_draft_and_the_run_goes_on() {
    let isp = Isp::start(false, &[]).await;
    // Two plans, each the most one entry may be: either is a price somebody
    // could enter, and together they are an invoice nobody may issue.
    let limit = 1_000_000_000_000_000_i64;
    let most = isp
        .tariff("The Limit", 10, "flat", limit, "USD", "monthly")
        .await;
    let ordinary = isp
        .tariff("Ordinary", 10, "flat", 10_000, "USD", "monthly")
        .await;
    let too_much = isp.customer("Aa Too Much", "USD", json!({})).await;
    for _ in 0..2 {
        isp.subscribe(&too_much, &most, Some(at("2025-01-01T00:00:00Z")))
            .await;
    }
    let after_them = isp.customer("Bb Ordinary", "USD", json!({})).await;
    isp.subscribe(&after_them, &ordinary, Some(at("2025-01-01T00:00:00Z")))
        .await;

    let mut events = isp.app.subscribe();
    let run = isp
        .ok(
            "isp.invoice.run",
            over(november(), json!({"dry_run": false, "issue": true})),
        )
        .await;
    assert_eq!(
        (&run["considered"], &run["drafted"], &run["issued"]),
        (&json!(2), &json!(2), &json!(1)),
        "{run:#}"
    );
    let row_of = |customer: &str| -> Value {
        list(&run["invoices"])
            .into_iter()
            .find(|row| row["customer_id"] == json!(customer))
            .unwrap_or_else(|| panic!("{customer} is not in the run: {run:#}"))
    };
    let stuck = row_of(&too_much);
    assert_eq!(stuck["number"], Value::Null);
    assert!(
        stuck["not_issued"]
            .as_str()
            .is_some_and(|why| why.contains("more than one entry")),
        "the draft does not say why it was not issued: {stuck:#}"
    );
    assert_eq!(stuck["total"]["minor"], json!(2 * limit));
    // The customer after them was reached, and issued.
    let reached = row_of(&after_them);
    assert_eq!(reached["not_issued"], Value::Null);
    assert!(reached["number"].is_string(), "{reached:#}");
    assert_eq!(list(&run["warnings"]).len(), 1, "{:#}", run["warnings"]);

    // What the run says is what is stored, and it left its entry in the trail.
    let draft = isp
        .ok("isp.invoice.get", json!({"id": stuck["invoice_id"]}))
        .await;
    assert_eq!(
        (&draft["state"], &draft["number"]),
        (&json!("draft"), &Value::Null)
    );
    assert!(
        isp.audit()
            .iter()
            .any(|(source, message)| source == "isp.invoice.run"
                && message.contains("Billed 2 of 2")),
        "a run that wrote invoices left no audit entry"
    );
    // One event for the draft it left and one for the invoice it issued.
    let mut changes = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == "isp.invoices.changed" {
            changes.push((
                event.payload["change"].clone(),
                event.payload["customer_ids"].clone(),
            ));
        }
    }
    assert_eq!(
        changes,
        [
            (json!("drafted"), json!([too_much])),
            (json!("issued"), json!([after_them]))
        ]
    );
}

/// A run that is cut off — a power cut, the process killed — leaves numbered
/// invoices behind it, and they must never be invoices the trail knows
/// nothing about. So the run says it has begun, with what it set out to do,
/// before it writes its first invoice; and its closing entry is the same
/// run's.
#[tokio::test]
async fn a_billing_run_is_in_the_trail_before_its_first_invoice_is_written() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    for name in ["Run One", "Run Two", "Run Three"] {
        let customer = isp.customer(name, "USD", json!({})).await;
        isp.subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
            .await;
    }
    // A preview writes nothing, and the trail does not mention it.
    let before = isp.audit().len();
    isp.ok("isp.invoice.run", over(november(), json!({}))).await;
    assert_eq!(isp.audit().len(), before, "a preview is not a mutation");

    let run = isp
        .ok(
            "isp.invoice.run",
            over(november(), json!({"dry_run": false, "issue": true})),
        )
        .await;
    let entries = isp.audit_entries();
    let started = entries
        .iter()
        .find(|entry| entry.source == "isp.invoice.run.start")
        .expect("the run said it had begun");
    let detail = started.body.clone().expect("what the run set out to do");
    assert_eq!(detail["considered"], json!(3), "{detail}");
    assert_eq!(detail["issue"], json!(true), "{detail}");
    assert_eq!(detail["period_start"], json!(november().0));
    assert_eq!(detail["period_end"], json!(november().1));
    for row in list(&run["invoices"]) {
        let invoice = isp
            .ok("isp.invoice.get", json!({"id": row["invoice_id"]}))
            .await;
        assert!(
            started.ts <= invoice["created_at"].as_i64().unwrap_or(i64::MIN),
            "invoice {} was written before the trail said a run had begun",
            invoice["number"]
        );
    }
    let finished = entries
        .iter()
        .find(|entry| entry.source == "isp.invoice.run")
        .expect("the run's closing entry");
    assert!(finished.ts >= started.ts);
    let closing = finished.body.clone().expect("the run's outcome");
    assert!(detail["run"].is_string(), "{detail}");
    assert_eq!(
        closing["run"], detail["run"],
        "the two entries are one run's"
    );
    assert_eq!(closing["not_reached"], json!(0));
}

/// Shutting down stops a run between two customers, rather than wherever the
/// process happens to be when it exits, and the run says whom it did not
/// reach — in its result and in the trail. Nothing is written for a customer
/// it did not reach, so the next run bills them as it would have.
#[tokio::test]
async fn a_billing_run_stopped_by_a_shutdown_says_whom_it_did_not_reach() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    for name in ["Late One", "Late Two"] {
        let customer = isp.customer(name, "USD", json!({})).await;
        isp.subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
            .await;
    }
    isp.app.shutdown();
    let run = isp
        .ok(
            "isp.invoice.run",
            over(november(), json!({"dry_run": false, "issue": true})),
        )
        .await;
    assert_eq!(
        (&run["considered"], &run["drafted"], &run["not_reached"]),
        (&json!(0), &json!(0), &json!(2)),
        "{run:#}"
    );
    assert!(
        list(&run["warnings"]).iter().any(|warning| {
            let warning = warning.as_str().unwrap_or_default();
            warning.contains("shutting down") && warning.contains('2')
        }),
        "{:#}",
        run["warnings"]
    );
    assert!(list(&isp.ok("isp.invoices", json!({})).await).is_empty());
    let finished = isp
        .audit_entries()
        .into_iter()
        .find(|entry| entry.source == "isp.invoice.run")
        .expect("the run's closing entry");
    assert!(finished.message.contains("stopped"), "{}", finished.message);
    assert_eq!(
        finished.body.expect("the run's outcome")["not_reached"],
        json!(2)
    );
}

#[tokio::test]
async fn a_customer_named_twice_is_billed_once_and_an_unknown_one_stops_the_run_before_it_writes() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    let active = isp.customer("Active", "USD", json!({})).await;
    isp.subscribe(&active, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let suspended = isp.customer("Suspended", "USD", json!({})).await;
    isp.subscribe(&suspended, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    isp.ok(
        "isp.customer.state",
        json!({"id": suspended, "state": "suspended"}),
    )
    .await;

    // Nobody named: the active accounts, and only those.
    let run = isp.ok("isp.invoice.run", over(november(), json!({}))).await;
    assert_eq!(run["considered"], json!(1));
    // Named, a suspended account is billed; named twice, once — in the
    // preview's total as well as in what a real run writes.
    let named = json!({"customer_ids": [active, suspended, active]});
    let preview = isp
        .ok("isp.invoice.run", over(november(), named.clone()))
        .await;
    assert_eq!(preview["considered"], json!(2));
    assert_eq!(preview["totals"][0]["minor"], json!(5_000), "{preview:#}");

    // A name that is nobody's is the run's mistake, found before it writes.
    let err = isp
        .err(
            "isp.invoice.run",
            over(
                november(),
                json!({"customer_ids": [active, "nobody"], "dry_run": false}),
            ),
        )
        .await;
    assert_eq!(err.code, "not_found");
    assert!(list(&isp.ok("isp.invoices", json!({})).await).is_empty());

    let mut real = named;
    real["dry_run"] = json!(false);
    let run = isp.ok("isp.invoice.run", over(november(), real)).await;
    assert_eq!(
        (&run["considered"], &run["drafted"]),
        (&json!(2), &json!(2))
    );
    assert!(list(&run["skipped"]).is_empty(), "{run:#}");
    assert_eq!(list(&isp.ok("isp.invoices", json!({})).await).len(), 2);
}

/// The dinar has no minor unit, so a third of 50,000 is not a figure anybody
/// can be charged: three equal parts alone are 16,667 each and come to a
/// dinar more than the plan.
#[tokio::test]
async fn the_parts_of_a_plan_priced_in_dinars_come_to_the_whole_dinar_price() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Thirds", "IQD", json!({})).await;
    let tariff = isp
        .tariff("Home", 10, "flat", 50_000, "IQD", "monthly")
        .await;
    let first = isp
        .subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let second = isp
        .ok(
            "isp.subscription.amend",
            json!({"id": first, "mbit": 20, "from_at": at("2025-11-11T00:00:00Z")}),
        )
        .await["subscription"]["id"]
        .clone();
    isp.ok(
        "isp.subscription.amend",
        json!({"id": second, "mbit": 30, "from_at": at("2025-11-21T00:00:00Z")}),
    )
    .await;

    let drafted = isp
        .ok(
            "isp.invoice.draft",
            over(november(), json!({"customer_id": customer})),
        )
        .await;
    let mut amounts: Vec<i64> = list(&drafted["invoice"]["lines"])
        .iter()
        .map(|line| line["amount"]["minor"].as_i64().expect("an amount"))
        .collect();
    amounts.sort_unstable();
    assert_eq!(
        (amounts, &drafted["invoice"]["total"]),
        (
            vec![16_666, 16_667, 16_667],
            &json!({"currency": "IQD", "minor": 50_000, "digits": 0, "text": "50000"})
        )
    );
    // The month after, only the last terms ran: one line, the whole price.
    let december = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "month": "2025-12"}),
        )
        .await;
    assert_eq!(list(&december["invoice"]["lines"]).len(), 1);
    assert_eq!(december["invoice"]["total"]["minor"], json!(50_000));
}

#[tokio::test]
async fn an_invoice_never_reads_paid_without_a_payment() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Free Month", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let drafted = isp
        .ok(
            "isp.invoice.draft",
            over(november(), json!({"customer_id": customer})),
        )
        .await;
    let id = drafted["invoice"]["id"].clone();
    isp.ok(
        "isp.invoice.line.add",
        json!({"invoice_id": id, "description": "the month is on us", "amount": usd(-10_000)}),
    )
    .await;

    // It comes to nothing, and it may still be issued: it is a record of a
    // month nobody was charged for. What it may not do is say money arrived.
    let issued = isp.ok("isp.invoice.issue", json!({"id": id})).await;
    assert_eq!(issued["total"]["minor"], json!(0));
    let view = isp.ok("isp.invoice.get", json!({"id": id})).await;
    assert_eq!(
        (&view["status"], &view["paid"]["minor"]),
        (&json!("issued"), &json!(0)),
        "nothing marks an invoice paid except a recorded payment"
    );
    assert!(list(&view["payments"]).is_empty());
    let arrears = isp.ok("isp.arrears", json!({})).await;
    assert!(
        list(&arrears["by_currency"]).is_empty() && list(&arrears["customers_overpaid"]).is_empty(),
        "an invoice for nothing is neither a debt nor a credit: {arrears:#}"
    );

    // Less than nothing is not an invoice at all: what a customer is owed is
    // a credit note, with its reason.
    let drafted = isp
        .ok(
            "isp.invoice.draft",
            over(october(), json!({"customer_id": customer})),
        )
        .await;
    let id = drafted["invoice"]["id"].clone();
    isp.ok(
        "isp.invoice.line.add",
        json!({"invoice_id": id, "description": "typed twice", "amount": usd(-10_500)}),
    )
    .await;
    let err = isp.err("isp.invoice.issue", json!({"id": id})).await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("credit note"), "{}", err.message);
    let still = isp.ok("isp.invoice.get", json!({"id": id})).await;
    assert_eq!(
        (&still["state"], &still["number"]),
        (&json!("draft"), &Value::Null)
    );
}

#[tokio::test]
async fn a_figure_nobody_could_have_meant_is_refused_where_it_is_entered() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Fat Finger", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    let plan = isp
        .subscribe(&customer, &tariff, Some(at("2025-01-01T00:00:00Z")))
        .await;
    let issued = isp
        .invoice(
            &customer,
            (at("2025-11-01T00:00:00Z"), at("2025-12-01T00:00:00Z")),
            None,
            None,
        )
        .await;
    let draft = isp
        .ok(
            "isp.invoice.draft",
            over(october(), json!({"customer_id": customer})),
        )
        .await;

    // Ten million million dollars is the most one entry may be.
    let limit = 1_000_000_000_000_000_i64;
    for too_much in [limit + 1, i64::MAX] {
        let refusals = [
            (
                "isp.payment.record",
                json!({"customer_id": customer, "amount": usd(too_much), "method": "cash"}),
            ),
            (
                "isp.payment.record",
                json!({"customer_id": customer, "invoice_id": issued["id"],
                       "amount": usd(too_much), "method": "cash"}),
            ),
            (
                "isp.credit.issue",
                json!({"customer_id": customer, "amount": usd(too_much), "reason": "a slip"}),
            ),
            (
                "isp.invoice.line.add",
                json!({"invoice_id": draft["invoice"]["id"], "description": "a slip",
                       "amount": usd(too_much)}),
            ),
            (
                "isp.invoice.line.add",
                json!({"invoice_id": draft["invoice"]["id"], "description": "a slip",
                       "amount": usd(-too_much)}),
            ),
            (
                "isp.tariff.save",
                json!({"name": "A Slip", "mbit": 10, "unit": "flat", "price": usd(too_much),
                       "period": "monthly"}),
            ),
            (
                "isp.subscription.create",
                json!({"customer_id": customer, "tariff_id": tariff, "price": usd(too_much)}),
            ),
            (
                "isp.subscription.amend",
                json!({"id": plan, "price": usd(too_much)}),
            ),
            (
                "isp.link.save",
                json!({"name": "A Slip", "capacity_mbit": 100, "cost_per_mbit": usd(too_much)}),
            ),
        ];
        for (method, params) in refusals {
            let err = isp.err(method, params).await;
            assert_eq!(err.code, "invalid", "{method} with {too_much}: {err}");
        }
    }
    // What a payment settles is added up as well, in the invoice's currency,
    // and a rate can make an amount that is within the limit absurd: a
    // thousand million million dinars at one dinar to the dollar.
    let err = isp
        .err(
            "isp.payment.record",
            json!({"customer_id": customer, "invoice_id": issued["id"], "method": "cash",
                   "amount": {"currency": "IQD", "minor": limit},
                   "conversion": {"from": "USD", "to": "IQD", "rate": 1}}),
        )
        .await;
    assert_eq!(err.code, "invalid", "{err}");
    assert!(err.message.contains("settles"), "{}", err.message);
    // A price per megabit is within the limit and the plan's price is not.
    let err = isp
        .err(
            "isp.tariff.save",
            json!({"name": "Per Megabit", "mbit": 1_000_000, "unit": "per_mbit",
                   "price": usd(limit / 1_000), "period": "monthly"}),
        )
        .await;
    assert_eq!(err.code, "invalid");
    // And megabits have a limit of their own, for the same reason.
    for (method, params) in [
        (
            "isp.tariff.save",
            json!({"name": "Too Wide", "mbit": 1_000_000_001_i64, "unit": "flat",
                   "price": usd(100), "period": "monthly"}),
        ),
        (
            "isp.subscription.amend",
            json!({"id": plan, "mbit": i64::MAX}),
        ),
        (
            "isp.link.save",
            json!({"name": "Too Wide", "capacity_mbit": i64::MAX}),
        ),
    ] {
        let err = isp.err(method, params).await;
        assert_eq!(err.code, "invalid", "{method}: {err}");
    }

    // The limit itself is a figure like any other, and nothing that was
    // refused left a mark: the reads that add the ledger up still work.
    isp.ok(
        "isp.payment.record",
        json!({"customer_id": customer, "amount": usd(limit), "method": "cash",
               "at": nocturne_app::util::now_ms() - 1_000}),
    )
    .await;
    let detail = isp.ok("isp.customer.get", json!({"id": customer})).await;
    assert_eq!(detail["balance"]["on_account"][0]["minor"], json!(limit));
    assert_eq!(detail["balance"]["outstanding"][0]["minor"], json!(10_000));
    let revenue = isp.ok("isp.revenue", json!({"since": 0})).await;
    assert_eq!(revenue["received"][0]["minor"], json!(limit));
    let summary = isp.ok("isp.summary", json!({})).await;
    assert_eq!(summary["received_30d"][0]["minor"], json!(limit));
}
