//! Two requests at the same instant.
//!
//! `App::rpc` runs handlers on as many threads as the server or the desktop
//! shell gives it, so every check in the ledger ("is this period already
//! billed?", "is it still a draft?", "how much has been credited?") has to
//! hold until the write it guards has happened. These tests make the calls
//! from real OS threads released by a barrier, because that is what two HTTP
//! requests — or one request and its retry — are. Two tasks on one runtime
//! worker take turns and would prove nothing.
//!
//! Each race is run many times: an interleaving that loses is not certain on
//! any one attempt, and a test that passed by luck would be worse than none.

mod common;

use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};

use common::{DAY_MS, Isp, list};
use nocturne_app::AppResult;
use serde_json::{Value, json};

/// How many times each race is run.
const ROUNDS: usize = 40;

/// 15 June 2025. Documents are dated here so that every number in a test
/// comes from one year's sequence whatever day the test is run on.
const MID_2025: i64 = 1_750_000_000_000;

/// Make these calls from one OS thread each, all released together.
async fn at_once(isp: &Isp, calls: Vec<(&'static str, Value)>) -> Vec<AppResult<Value>> {
    let handle = tokio::runtime::Handle::current();
    let barrier = Arc::new(Barrier::new(calls.len()));
    let app = isp.app.clone();
    tokio::task::spawn_blocking(move || {
        let callers: Vec<_> = calls
            .into_iter()
            .map(|(method, params)| {
                let (app, handle, barrier) = (app.clone(), handle.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    handle.block_on(app.rpc(method, params))
                })
            })
            .collect();
        callers
            .into_iter()
            .map(|caller| caller.join().expect("a caller panicked"))
            .collect()
    })
    .await
    .expect("the callers ran")
}

fn succeeded(results: &[AppResult<Value>]) -> Vec<&Value> {
    results
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .collect()
}

fn refusals(results: &[AppResult<Value>]) -> Vec<&str> {
    results
        .iter()
        .filter_map(|result| result.as_ref().err())
        .map(|err| err.code)
        .collect()
}

/// A monthly window `back` months ago, so one fixture can be billed for
/// several different periods.
///
/// The windows are a day apart rather than end to end. Each call reads the
/// clock afresh, so windows laid end to end would overlap by however long
/// the test had been running — and a period that overlaps one already
/// billed is, rightly, refused.
fn month(back: i64) -> (i64, i64) {
    let end = nocturne_app::util::now_ms() - back * 32 * DAY_MS;
    (end - 31 * DAY_MS, end)
}

/// The counter at the end of a number such as `2026-0007` or `CN-2026-0007`.
fn sequence(number: &Value) -> u32 {
    number
        .as_str()
        .and_then(|text| text.rsplit('-').next())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| panic!("a number ending in its sequence, got {number}"))
}

async fn customer_on_a_plan(isp: &Isp, name: &str, tariff: &str) -> String {
    let customer = isp.customer(name, "USD", json!({})).await;
    isp.subscribe(&customer, tariff, None).await;
    customer
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_drafts_for_one_period_at_the_same_instant_leave_one_invoice() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    let period = month(0);
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Twice {round}"), &tariff).await;
        let draft = json!({
            "customer_id": customer,
            "period_start": period.0,
            "period_end": period.1,
        });
        let results = at_once(
            &isp,
            vec![
                ("isp.invoice.draft", draft.clone()),
                ("isp.invoice.draft", draft),
            ],
        )
        .await;
        assert_eq!(
            succeeded(&results).len(),
            1,
            "round {round}: one period was drafted twice: {results:#?}"
        );
        assert_eq!(refusals(&results), ["conflict"], "round {round}");
        let stored = list(
            &isp.ok("isp.invoices", json!({"customer_id": customer}))
                .await,
        );
        assert_eq!(
            stored.len(),
            1,
            "round {round}: the customer has two invoices for one period"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_billing_run_racing_itself_bills_each_customer_once() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    // Enough customers that each run takes long enough for the other to be
    // part of the way through it: with a handful, one run is often over
    // before the second thread has started its own, and nothing races.
    let mut customers = Vec::new();
    for index in 0..24 {
        customers.push(customer_on_a_plan(&isp, &format!("Run {index:02}"), &tariff).await);
    }
    // One period per round: the subscriptions are a year old, so the last ten
    // months are each a period nobody has billed yet.
    for round in 0..10 {
        let period = month(round);
        let run = json!({
            "period_start": period.0,
            "period_end": period.1,
            "dry_run": false,
        });
        let results = at_once(
            &isp,
            vec![("isp.invoice.run", run.clone()), ("isp.invoice.run", run)],
        )
        .await;
        assert_eq!(succeeded(&results).len(), 2, "{results:#?}");
        for customer in &customers {
            let for_period: Vec<Value> = list(
                &isp.ok("isp.invoices", json!({"customer_id": customer}))
                    .await,
            )
            .into_iter()
            .filter(|invoice| invoice["period_start"] == json!(period.0))
            .collect();
            assert_eq!(
                for_period.len(),
                1,
                "round {round}: two runs at once billed {customer} twice for one period"
            );
        }
        // Between them the two runs wrote each invoice once, and each says of
        // the ones it did not write that they were already there.
        let written: usize = succeeded(&results)
            .iter()
            .map(|run| list(&run["invoices"]).len())
            .sum();
        assert_eq!(written, customers.len(), "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invoice_issued_twice_at_once_gets_one_number_and_burns_none() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    let period = month(0);
    let mut numbers = BTreeSet::new();
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Issue {round}"), &tariff).await;
        let draft = isp
            .ok(
                "isp.invoice.draft",
                json!({"customer_id": customer, "period_start": period.0,
                       "period_end": period.1}),
            )
            .await;
        let id = draft["invoice"]["id"].clone();
        let results = at_once(
            &isp,
            vec![
                (
                    "isp.invoice.issue",
                    json!({"id": id, "issued_at": MID_2025}),
                ),
                (
                    "isp.invoice.issue",
                    json!({"id": id, "issued_at": MID_2025}),
                ),
            ],
        )
        .await;
        let issued = succeeded(&results);
        assert_eq!(
            issued.len(),
            1,
            "round {round}: one draft was issued twice: {results:#?}"
        );
        assert_eq!(refusals(&results), ["conflict"], "round {round}");
        let stored = isp.ok("isp.invoice.get", json!({"id": id})).await;
        assert_eq!(
            stored["number"], issued[0]["number"],
            "round {round}: the caller was told a number the invoice does not carry"
        );
        numbers.insert(sequence(&stored["number"]));
    }
    // A number taken by the call that lost would leave a hole in the
    // sequence, and a hole in an invoice sequence is what an auditor asks
    // about first.
    let expected: BTreeSet<u32> = (1..=ROUNDS as u32).collect();
    assert_eq!(numbers, expected, "the sequence has gaps");
    let audited = isp
        .audit()
        .iter()
        .filter(|(source, _)| source == "isp.invoice.issue")
        .count();
    assert_eq!(
        audited, ROUNDS,
        "the trail records issues that never happened"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_full_credits_at_once_cannot_write_off_an_invoice_twice() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Credit {round}"), &tariff).await;
        let invoice = isp.invoice(&customer, month(0), None, None).await["id"].clone();
        let credit = json!({
            "customer_id": customer,
            "invoice_id": invoice,
            "amount": {"currency": "USD", "minor": 10_000},
            "reason": "the link was down all month",
            "at": MID_2025,
        });
        let results = at_once(
            &isp,
            vec![
                ("isp.credit.issue", credit.clone()),
                ("isp.credit.issue", credit),
            ],
        )
        .await;
        assert_eq!(
            succeeded(&results).len(),
            1,
            "round {round}: 200.00 was written off a 100.00 invoice: {results:#?}"
        );
        assert_eq!(refusals(&results), ["invalid"], "round {round}");
        let view = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
        assert_eq!(view["credited"]["minor"], 10_000, "round {round}");
        assert_eq!(view["outstanding"]["minor"], 0, "round {round}");
    }
    // The refused note took no number with it.
    let notes = list(&isp.ok("isp.credits", json!({"limit": 2_000})).await);
    let numbers: BTreeSet<u32> = notes.iter().map(|note| sequence(&note["number"])).collect();
    let expected: BTreeSet<u32> = (1..=ROUNDS as u32).collect();
    assert_eq!(numbers, expected, "the credit note sequence has gaps");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_credits_on_account_set_against_one_invoice_at_once_respect_the_cap() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Allocate {round}"), &tariff).await;
        let invoice = isp.invoice(&customer, month(0), None, None).await["id"].clone();
        let mut notes = Vec::new();
        for _ in 0..2 {
            let note = isp
                .ok(
                    "isp.credit.issue",
                    json!({
                        "customer_id": customer,
                        "amount": {"currency": "USD", "minor": 10_000},
                        "reason": "agreed settlement",
                    }),
                )
                .await;
            notes.push(note["credit"]["id"].clone());
        }
        let results = at_once(
            &isp,
            vec![
                (
                    "isp.credit.allocate",
                    json!({"id": notes[0], "invoice_id": invoice}),
                ),
                (
                    "isp.credit.allocate",
                    json!({"id": notes[1], "invoice_id": invoice}),
                ),
            ],
        )
        .await;
        assert_eq!(
            succeeded(&results).len(),
            1,
            "round {round}: both notes were set against one invoice: {results:#?}"
        );
        let view = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
        assert_eq!(view["credited"]["minor"], 10_000, "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_line_added_while_the_invoice_is_being_issued_cannot_unissue_it() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Line {round}"), &tariff).await;
        let period = month(0);
        let draft = isp
            .ok(
                "isp.invoice.draft",
                json!({"customer_id": customer, "period_start": period.0,
                       "period_end": period.1}),
            )
            .await;
        let id = draft["invoice"]["id"].clone();
        let results = at_once(
            &isp,
            vec![
                (
                    "isp.invoice.line.add",
                    json!({
                        "invoice_id": id,
                        "description": "Installation",
                        "amount": {"currency": "USD", "minor": 2_500},
                    }),
                ),
                ("isp.invoice.issue", json!({"id": id})),
            ],
        )
        .await;
        let issued = results[1]
            .as_ref()
            .unwrap_or_else(|err| panic!("round {round}: the issue was refused: {err}"));
        let stored = isp.ok("isp.invoice.get", json!({"id": id})).await;
        assert_eq!(
            stored["state"], "issued",
            "round {round}: the line's write put the draft back over the issued invoice"
        );
        assert_eq!(stored["number"], issued["number"], "round {round}");
        // The line either got in before the invoice froze, and is on it, or
        // was refused. It is never accepted and then lost, and never added
        // to an invoice that had already gone out.
        let lines = list(&stored["lines"]).len();
        match &results[0] {
            Ok(_) => {
                assert_eq!(lines, 2, "round {round}: an accepted line is missing");
                assert_eq!(stored["total"]["minor"], 12_500, "round {round}");
                assert_eq!(issued["total"]["minor"], 12_500, "round {round}");
            }
            Err(err) => {
                assert_eq!(err.code, "conflict", "round {round}: {err}");
                assert_eq!(lines, 1, "round {round}: a refused line is on the invoice");
                assert_eq!(stored["total"]["minor"], 10_000, "round {round}");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invoice_cannot_be_voided_from_under_a_payment_or_a_credit_note() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Void {round}"), &tariff).await;
        let invoice = isp.invoice(&customer, month(0), None, None).await["id"].clone();
        // Alternate what the void races against.
        let against = if round % 2 == 0 {
            (
                "isp.payment.record",
                json!({
                    "customer_id": customer,
                    "invoice_id": invoice,
                    "method": "cash",
                    "amount": {"currency": "USD", "minor": 10_000},
                }),
            )
        } else {
            (
                "isp.credit.issue",
                json!({
                    "customer_id": customer,
                    "invoice_id": invoice,
                    "amount": {"currency": "USD", "minor": 4_000},
                    "reason": "three days of outage",
                }),
            )
        };
        let method = against.0;
        let results = at_once(
            &isp,
            vec![
                (
                    "isp.invoice.void",
                    json!({"id": invoice, "reason": "raised against the wrong account"}),
                ),
                against,
            ],
        )
        .await;
        assert_eq!(
            succeeded(&results).len(),
            1,
            "round {round}: the invoice was voided and {method} still went through against \
             it: {results:#?}"
        );
        assert_eq!(refusals(&results), ["conflict"], "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_invoice_reissued_twice_at_once_is_replaced_once() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Reissue {round}"), &tariff).await;
        let invoice = isp.invoice(&customer, month(0), None, None).await["id"].clone();
        let reissue = json!({"id": invoice, "reason": "a line was wrong"});
        let results = at_once(
            &isp,
            vec![
                ("isp.invoice.reissue", reissue.clone()),
                ("isp.invoice.reissue", reissue),
            ],
        )
        .await;
        assert_eq!(
            succeeded(&results).len(),
            1,
            "round {round}: one invoice was voided and replaced twice: {results:#?}"
        );
        assert_eq!(refusals(&results), ["conflict"], "round {round}");
        // The original, void and pointing at its one replacement.
        let stored = list(
            &isp.ok("isp.invoices", json!({"customer_id": customer}))
                .await,
        );
        let drafts: Vec<&Value> = stored
            .iter()
            .filter(|invoice| invoice["state"] == "draft")
            .collect();
        assert_eq!(
            (stored.len(), drafts.len()),
            (2, 1),
            "round {round}: two replacements for one invoice"
        );
        let original = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
        assert_eq!(original["replaced_by"], drafts[0]["id"], "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terms_amended_twice_at_once_leave_one_plan_running() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    for round in 0..ROUNDS {
        let customer = isp
            .customer(&format!("Amend {round}"), "USD", json!({}))
            .await;
        let plan = isp.subscribe(&customer, &tariff, None).await;
        let results = at_once(
            &isp,
            vec![
                ("isp.subscription.amend", json!({"id": plan, "mbit": 20})),
                ("isp.subscription.amend", json!({"id": plan, "mbit": 30})),
            ],
        )
        .await;
        assert_eq!(
            succeeded(&results).len(),
            1,
            "round {round}: one plan was amended to two sets of terms: {results:#?}"
        );
        assert_eq!(refusals(&results), ["conflict"], "round {round}");
        let running = list(
            &isp.ok(
                "isp.subscriptions",
                json!({"customer_id": customer, "state": "active"}),
            )
            .await,
        );
        assert_eq!(
            running.len(),
            1,
            "round {round}: the customer is on two plans, and would be charged for both"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_customer_cannot_be_deleted_while_their_first_invoice_is_being_drafted() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    for round in 0..ROUNDS {
        let customer = customer_on_a_plan(&isp, &format!("Leaving {round}"), &tariff).await;
        let period = month(0);
        let results = at_once(
            &isp,
            vec![
                ("isp.customer.delete", json!({"id": customer})),
                (
                    "isp.invoice.draft",
                    json!({"customer_id": customer, "period_start": period.0,
                           "period_end": period.1}),
                ),
            ],
        )
        .await;
        assert_eq!(
            succeeded(&results).len(),
            1,
            "round {round}: an invoice was written for a customer who was deleted: {results:#?}"
        );
        let invoices = list(
            &isp.ok("isp.invoices", json!({"customer_id": customer}))
                .await,
        );
        let exists = isp
            .rpc("isp.customer.get", json!({"id": customer}))
            .await
            .is_ok();
        assert_eq!(
            invoices.len(),
            usize::from(exists),
            "round {round}: a ledger with no customer, or a customer deleted with a ledger"
        );
    }
}
