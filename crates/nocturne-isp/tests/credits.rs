//! Credit notes: money a customer stops owing.
//!
//! The thing worth testing is not that the arithmetic works but that a
//! write-off never pretends to be a payment. An ISP that cannot tell "we
//! collected it" from "we gave up on it" cannot answer the only question
//! anybody asks of a billing system at the end of a month.

mod common;

use common::{Isp, last_month, list};
use serde_json::json;

/// An invoice cleared by a credit note reads `credited`, not `paid`.
#[tokio::test]
async fn a_credit_note_clears_an_invoice_without_claiming_the_money_arrived() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Zagros Telecom", "IQD", json!({})).await;
    let tariff = isp
        .tariff("P2P 100M", 100, "per_mbit", 15_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let issued = isp.invoice(&customer, last_month(), None, None).await;
    let invoice = issued["id"].as_str().expect("an id").to_string();
    let total = issued["total"]["minor"].as_i64().expect("a total");
    assert_eq!(total, 1_500_000, "100 Mbit at 15,000 dinars each");

    let credited = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": invoice,
                "amount": {"currency": "IQD", "minor": total},
                "reason": "the link was down for the whole month",
            }),
        )
        .await;

    let view = &credited["invoice"];
    assert_eq!(view["outstanding"]["minor"], 0, "nothing is owed");
    assert_eq!(view["paid"]["minor"], 0, "and nothing was paid");
    assert_eq!(view["credited"]["minor"], total);
    assert_eq!(
        view["status"], "credited",
        "a written-off invoice must not be called paid: {}",
        view["status"]
    );
    // The reason is on the document, not in a log line somebody has to find.
    assert_eq!(
        credited["credit"]["reason"],
        "the link was down for the whole month"
    );
    // And its number cannot be mistaken for an invoice number.
    let number = credited["credit"]["number"].as_str().expect("a number");
    assert!(number.contains("CN-"), "{number}");
}

/// Partly credited, partly paid, and the two numbers stay apart.
#[tokio::test]
async fn a_part_credited_invoice_still_says_how_much_money_came_in() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Erbil Cloud", "USD", json!({})).await;
    let tariff = isp
        .tariff("P2P 50M", 50, "per_mbit", 1_200, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let issued = isp.invoice(&customer, last_month(), None, None).await;
    let invoice = issued["id"].as_str().expect("an id").to_string();

    // $600 charged, $100 written off, $200 paid.
    isp.ok(
        "isp.credit.issue",
        json!({
            "customer_id": customer,
            "invoice_id": invoice,
            "amount": {"currency": "USD", "minor": 10_000},
            "reason": "two days of outage",
        }),
    )
    .await;
    isp.ok(
        "isp.payment.record",
        json!({
            "customer_id": customer,
            "invoice_id": invoice,
            "amount": {"currency": "USD", "minor": 20_000},
            "method": "bank transfer",
        }),
    )
    .await;

    let view = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
    assert_eq!(view["paid"]["minor"], 20_000, "money received");
    assert_eq!(view["credited"]["minor"], 10_000, "written off");
    assert_eq!(view["outstanding"]["minor"], 30_000, "600 - 200 - 100");
    assert_eq!(
        view["status"], "part_paid",
        "money arrived for some of it, so this one is part paid"
    );
}

/// A credit note with no payment behind it does not make an invoice look
/// part paid — money is still owed and nobody has paid any of it.
#[tokio::test]
async fn crediting_part_of_an_invoice_is_not_a_part_payment() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Duhok Fibre", "USD", json!({})).await;
    let tariff = isp
        .tariff("P2P 50M", 50, "per_mbit", 1_200, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let issued = isp.invoice(&customer, last_month(), None, None).await;
    let invoice = issued["id"].as_str().expect("an id").to_string();

    let out = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": invoice,
                "amount": {"currency": "USD", "minor": 10_000},
                "reason": "two days of outage",
            }),
        )
        .await;
    assert_eq!(out["invoice"]["status"], "issued");
    assert_eq!(out["invoice"]["paid"]["minor"], 0);
    assert_eq!(out["invoice"]["outstanding"]["minor"], 50_000);
}

/// Arrears come down, per currency, and never gain a total across them.
#[tokio::test]
async fn a_credit_note_comes_off_the_arrears_in_its_own_currency() {
    let isp = Isp::start(false, &[]).await;
    let dinars = isp.customer("Zagros Telecom", "IQD", json!({})).await;
    let dollars = isp.customer("Erbil Cloud", "USD", json!({})).await;
    let iqd = isp
        .tariff("P2P 100M", 100, "per_mbit", 15_000, "IQD", "monthly")
        .await;
    let usd = isp
        .tariff("P2P 50M", 50, "per_mbit", 1_200, "USD", "monthly")
        .await;
    isp.subscribe(&dinars, &iqd, None).await;
    isp.subscribe(&dollars, &usd, None).await;
    let first = isp.invoice(&dinars, last_month(), None, None).await;
    isp.invoice(&dollars, last_month(), None, None).await;

    isp.ok(
        "isp.credit.issue",
        json!({
            "customer_id": dinars,
            "invoice_id": first["id"],
            "amount": {"currency": "IQD", "minor": 500_000},
            "reason": "a third of the month was lost to a fibre cut",
        }),
    )
    .await;

    let arrears = isp.ok("isp.arrears", json!({})).await;
    let by_currency = list(&arrears["by_currency"]);
    let owed = |code: &str| -> i64 {
        by_currency
            .iter()
            .find(|row| row["currency"] == code)
            .map(|row| row["total"]["minor"].as_i64().expect("a total"))
            .unwrap_or_else(|| panic!("no {code} row in {arrears:#}"))
    };
    assert_eq!(
        owed("IQD"),
        1_000_000,
        "1,500,000 less the 500,000 credited"
    );
    assert_eq!(owed("USD"), 60_000, "the other customer is untouched");
    for key in ["total", "total_minor", "grand_total", "sum"] {
        assert!(
            arrears.get(key).is_none(),
            "arrears gained a `{key}` across currencies"
        );
    }
}

/// Voiding a credit note puts the debt back, and says why.
#[tokio::test]
async fn a_voided_credit_note_puts_back_what_the_customer_owes() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Zagros Telecom", "IQD", json!({})).await;
    let tariff = isp
        .tariff("P2P 100M", 100, "per_mbit", 15_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let issued = isp.invoice(&customer, last_month(), None, None).await;
    let invoice = issued["id"].as_str().expect("an id").to_string();
    let out = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": invoice,
                "amount": {"currency": "IQD", "minor": 1_500_000},
                "reason": "agreed with the customer",
            }),
        )
        .await;
    let note = out["credit"]["id"].as_str().expect("an id").to_string();
    assert_eq!(out["invoice"]["outstanding"]["minor"], 0);

    let voided = isp
        .ok(
            "isp.credit.void",
            json!({"id": note, "reason": "raised against the wrong customer"}),
        )
        .await;
    assert_eq!(voided["state"], "void");
    assert_eq!(voided["void_reason"], "raised against the wrong customer");

    let view = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
    assert_eq!(
        view["outstanding"]["minor"], 1_500_000,
        "the debt is back in full"
    );
    assert_eq!(view["credited"]["minor"], 0);
    assert_eq!(view["status"], "issued");

    // Nothing was deleted: the withdrawn note is still there to be read.
    let all = list(
        &isp.ok("isp.credits", json!({"customer_id": customer}))
            .await,
    );
    assert_eq!(all.len(), 1, "a voided note is kept, not removed");
    assert_eq!(all[0]["state"], "void");
    // A second void is refused rather than silently repeated.
    let again = isp
        .err("isp.credit.void", json!({"id": note, "reason": "again"}))
        .await;
    assert_eq!(again.code, "conflict");
}

/// The four refusals, each saying what to do instead.
#[tokio::test]
async fn a_credit_note_is_refused_when_it_would_invent_something() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Zagros Telecom", "IQD", json!({})).await;
    let tariff = isp
        .tariff("P2P 100M", 100, "per_mbit", 15_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let issued = isp.invoice(&customer, last_month(), None, None).await;
    let invoice = issued["id"].as_str().expect("an id").to_string();
    let good = json!({
        "customer_id": customer,
        "invoice_id": invoice,
        "amount": {"currency": "IQD", "minor": 1_000},
        "reason": "goodwill",
    });
    let with = |key: &str, value: serde_json::Value| {
        let mut params = good.clone();
        params[key] = value;
        params
    };

    // No reason: the whole point of the document.
    let no_reason = isp
        .err("isp.credit.issue", with("reason", json!(" ")))
        .await;
    assert_eq!(no_reason.code, "invalid");
    assert!(no_reason.message.contains("why"), "{}", no_reason.message);

    // A negative or zero note.
    for bad in [json!(-1_000), json!(0)] {
        let err = isp
            .err(
                "isp.credit.issue",
                with("amount", json!({"currency": "IQD", "minor": bad})),
            )
            .await;
        assert_eq!(err.code, "invalid", "{bad} was accepted");
    }
    // And a float, like everywhere else money is entered.
    let float = isp
        .err(
            "isp.credit.issue",
            with("amount", json!({"currency": "IQD", "minor": 10.5})),
        )
        .await;
    assert_eq!(float.code, "invalid");

    // Another currency than the customer is billed in: no money moved, so
    // there is no rate anybody actually got.
    let wrong_currency = isp
        .err(
            "isp.credit.issue",
            with("amount", json!({"currency": "USD", "minor": 1_000})),
        )
        .await;
    assert_eq!(wrong_currency.code, "invalid");
    assert!(
        wrong_currency.message.contains("IQD"),
        "{}",
        wrong_currency.message
    );

    // More than was ever charged.
    let too_much = isp
        .err(
            "isp.credit.issue",
            with("amount", json!({"currency": "IQD", "minor": 1_500_001})),
        )
        .await;
    assert_eq!(too_much.code, "invalid");
    assert!(
        too_much.message.contains("more than was ever charged"),
        "{}",
        too_much.message
    );
}

/// A draft is not owed yet and a void invoice is not owed at all.
#[tokio::test]
async fn an_invoice_nobody_owes_cannot_be_credited() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Zagros Telecom", "IQD", json!({})).await;
    let tariff = isp
        .tariff("P2P 100M", 100, "per_mbit", 15_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let period = last_month();

    let draft = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": period.0, "period_end": period.1}),
        )
        .await;
    let draft_id = draft["invoice"]["id"].as_str().expect("an id").to_string();
    let still_a_draft = isp
        .err(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": draft_id,
                "amount": {"currency": "IQD", "minor": 1_000},
                "reason": "goodwill",
            }),
        )
        .await;
    assert_eq!(still_a_draft.code, "conflict");
    assert!(
        still_a_draft.message.contains("draft"),
        "{}",
        still_a_draft.message
    );

    isp.ok("isp.invoice.issue", json!({"id": draft_id})).await;
    isp.ok(
        "isp.invoice.void",
        json!({"id": draft_id, "reason": "billed the wrong period"}),
    )
    .await;
    let voided = isp
        .err(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": draft_id,
                "amount": {"currency": "IQD", "minor": 1_000},
                "reason": "goodwill",
            }),
        )
        .await;
    assert_eq!(voided.code, "conflict");
    assert!(voided.message.contains("void"), "{}", voided.message);
}

/// A credit note is not money, and no report may count it as received.
#[tokio::test]
async fn a_credit_note_is_never_counted_as_money_received() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Zagros Telecom", "IQD", json!({})).await;
    let tariff = isp
        .tariff("P2P 100M", 100, "per_mbit", 15_000, "IQD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let period = last_month();
    let issued = isp.invoice(&customer, period, None, None).await;

    isp.ok(
        "isp.credit.issue",
        json!({
            "customer_id": customer,
            "invoice_id": issued["id"],
            "amount": {"currency": "IQD", "minor": 1_500_000},
            "reason": "the link never came up",
        }),
    )
    .await;

    // Revenue is money that came in. None did.
    let revenue = isp
        .ok(
            "isp.revenue",
            json!({"since": period.0 - 1, "until": period.1 + 1}),
        )
        .await;
    let received = list(&revenue["received"]);
    assert!(
        received.is_empty() || received.iter().all(|row| row["minor"] == 0),
        "a credit note was counted as revenue: {revenue:#}"
    );

    // Nor is it a payment.
    assert!(
        list(
            &isp.ok("isp.payments", json!({"customer_id": customer}))
                .await
        )
        .is_empty(),
        "a credit note turned up in the payments"
    );

    // It is in the audit trail, with its reason, like every other mutation:
    // the reason is in the entry's detail, and not in the message other
    // features repeat.
    let audited = isp.audit_entries().into_iter().any(|entry| {
        entry.source == "isp.credit.issue"
            && !entry.message.contains("never came up")
            && entry.body.as_ref().is_some_and(|body| {
                body["reason"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("never came up")
            })
    });
    assert!(audited, "the credit note is not in the audit trail");
}

/// A note against the account, for when what is being made good is not one
/// invoice. It says plainly that it reduces nothing by itself.
#[tokio::test]
async fn a_credit_on_the_account_says_it_is_not_against_anything_yet() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Zagros Telecom", "IQD", json!({})).await;
    let out = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "amount": {"currency": "IQD", "minor": 250_000},
                "reason": "agreed settlement for the tower move",
            }),
        )
        .await;
    assert!(out["credit"]["invoice_id"].is_null());
    assert!(out["invoice"].is_null());
    let warnings = list(&out["warnings"]).iter().any(|warning| {
        warning
            .as_str()
            .is_some_and(|text| text.contains("reduces nothing"))
    });
    assert!(warnings, "no warning that it is against nothing: {out:#}");

    let balance = isp.ok("isp.customer.get", json!({"id": customer})).await;
    let on_account = &balance["balance"]["credit_on_account"];
    assert_eq!(
        on_account[0]["minor"], 250_000,
        "it stands against the account: {balance:#}"
    );
    // And it is not on the money side of the balance.
    let as_money = &balance["balance"]["on_account"];
    assert!(
        as_money.as_array().is_some_and(|rows| rows.is_empty()),
        "a credit note was counted as money on account: {as_money:#}"
    );
}

/// A month that ends a day before `last_month()` begins.
///
/// `last_month()` ends now, and now has moved on since the customer's first
/// invoice was drafted: a window that ended where this call's `last_month()`
/// begins would overlap that invoice by the milliseconds in between, which
/// is the same period billed twice and is refused.
fn the_month_before() -> (i64, i64) {
    let start_of_last = last_month().0;
    (
        start_of_last - 32 * common::DAY_MS,
        start_of_last - common::DAY_MS,
    )
}

/// An issued invoice of 100.00 dollars for a customer on a flat plan, and the
/// customer it belongs to.
async fn a_hundred_dollar_invoice(isp: &Isp, name: &str) -> (String, String) {
    let customer = isp.customer(name, "USD", json!({})).await;
    let tariff = match list(&isp.ok("isp.tariffs", json!({})).await).first() {
        Some(tariff) => tariff["id"].as_str().expect("an id").to_string(),
        None => {
            isp.tariff("Flat 10M", 10, "flat", 10_000, "USD", "monthly")
                .await
        }
    };
    isp.subscribe(&customer, &tariff, None).await;
    let issued = isp.invoice(&customer, last_month(), None, None).await;
    assert_eq!(issued["total"]["minor"], 10_000);
    (customer, issued["id"].as_str().expect("an id").to_string())
}

/// A credit note is a numbered document the customer was given. Voiding the
/// invoice it stands against would leave it issued against nothing, and what
/// it wrote off would stop counting anywhere without anybody saying so.
#[tokio::test]
async fn an_invoice_is_not_voided_or_reissued_from_under_a_credit_note() {
    let isp = Isp::start(false, &[]).await;
    let (customer, invoice) = a_hundred_dollar_invoice(&isp, "Halabja Wireless").await;
    let note = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": invoice,
                "amount": {"currency": "USD", "minor": 4_000},
                "reason": "three days of outage",
            }),
        )
        .await["credit"]
        .clone();
    let number = note["number"].as_str().expect("a number").to_string();

    for method in ["isp.invoice.void", "isp.invoice.reissue"] {
        let err = isp
            .err(method, json!({"id": invoice, "reason": "a line was wrong"}))
            .await;
        assert_eq!(err.code, "conflict", "{method}: {}", err.message);
        assert!(
            err.message.contains(&number),
            "{method} does not say which note is in the way: {}",
            err.message
        );
        assert!(
            err.message.contains("Void") && err.message.contains("its own reason"),
            "{method} does not say what to do instead: {}",
            err.message
        );
    }
    // Nothing moved: the invoice is still owed less its credit.
    let view = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
    assert_eq!(view["state"], "issued");
    assert_eq!(view["credited"]["minor"], 4_000);
    assert_eq!(view["outstanding"]["minor"], 6_000);
    let balance = isp.ok("isp.customer.get", json!({"id": customer})).await;
    assert_eq!(balance["balance"]["outstanding"][0]["minor"], 6_000);

    // Once the note is withdrawn, with its own reason, the invoice can be
    // corrected, and the replacement is owed in full until it is credited
    // itself.
    isp.ok(
        "isp.credit.void",
        json!({"id": note["id"], "reason": "the invoice it credits is being reissued"}),
    )
    .await;
    let reissued = isp
        .ok(
            "isp.invoice.reissue",
            json!({"id": invoice, "reason": "a line was wrong"}),
        )
        .await;
    assert_eq!(reissued["voided"]["state"], "void");
    let notes = list(&isp.ok("isp.credits", json!({"invoice_id": invoice})).await);
    assert!(
        notes.iter().all(|note| note["state"] == "void"),
        "an issued credit note points at a void invoice: {notes:#?}"
    );
}

/// `Paid` is a claim that the customer's money arrived for all of it. A
/// goodwill credit afterwards does not take that back.
#[tokio::test]
async fn an_invoice_paid_in_full_still_reads_paid_after_a_goodwill_credit() {
    let isp = Isp::start(false, &[]).await;
    let (customer, invoice) = a_hundred_dollar_invoice(&isp, "Kirkuk Net").await;
    let paid = isp
        .ok(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": invoice,
                "amount": {"currency": "USD", "minor": 10_000},
                "method": "cash",
            }),
        )
        .await;
    assert_eq!(paid["invoice"]["status"], "paid");

    let credited = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": invoice,
                "amount": {"currency": "USD", "minor": 2_000},
                "reason": "goodwill for the slow week",
            }),
        )
        .await;
    let view = &credited["invoice"];
    assert_eq!(
        view["status"], "paid",
        "their money arrived for all of it, and a credit afterwards relabelled it"
    );
    assert_eq!(view["paid"]["minor"], 10_000);
    assert_eq!(view["credited"]["minor"], 2_000);
    assert_eq!(view["outstanding"]["minor"], -2_000);
    assert!(
        list(&credited["warnings"])
            .iter()
            .any(|warning| warning.as_str().is_some_and(|text| text.contains("favour"))),
        "no warning that the customer is now in credit: {credited:#}"
    );
}

/// What is settled beyond an invoice's total stands in the customer's favour.
/// It is reported, per currency, and never quietly netted off another debt.
#[tokio::test]
async fn money_standing_in_a_customers_favour_is_in_their_balance_and_the_arrears_report() {
    let isp = Isp::start(false, &[]).await;
    let (customer, invoice) = a_hundred_dollar_invoice(&isp, "Kirkuk Net").await;
    for (method, params) in [
        (
            "isp.payment.record",
            json!({"customer_id": customer, "invoice_id": invoice, "method": "cash",
                   "amount": {"currency": "USD", "minor": 10_000}}),
        ),
        (
            "isp.credit.issue",
            json!({"customer_id": customer, "invoice_id": invoice,
                   "amount": {"currency": "USD", "minor": 2_000},
                   "reason": "goodwill for the slow week"}),
        ),
    ] {
        isp.ok(method, params).await;
    }

    let balance = isp.ok("isp.customer.get", json!({"id": customer})).await["balance"].clone();
    assert_eq!(
        balance["overpaid"][0]["minor"], 2_000,
        "20.00 stands in their favour and the balance does not say so: {balance:#}"
    );
    assert_eq!(balance["overpaid"][0]["currency"], "USD");
    assert_eq!(balance["invoices_overpaid"], 1);
    assert!(list(&balance["outstanding"]).is_empty(), "{balance:#}");

    // A second, unpaid invoice is still owed in full: the 20.00 is shown
    // beside it, not taken off it, because nothing has set it against it.
    let earlier = the_month_before();
    isp.invoice(&customer, earlier, None, None).await;
    let balance = isp.ok("isp.customer.get", json!({"id": customer})).await["balance"].clone();
    assert_eq!(balance["outstanding"][0]["minor"], 10_000, "{balance:#}");
    assert_eq!(balance["overpaid"][0]["minor"], 2_000, "{balance:#}");

    let arrears = isp.ok("isp.arrears", json!({})).await;
    assert_eq!(arrears["by_currency"][0]["total"]["minor"], 10_000);
    assert_eq!(
        arrears["overpaid"][0]["minor"], 2_000,
        "the report that says who to chase does not say who is in credit: {arrears:#}"
    );
    let rows = list(&arrears["customers_overpaid"]);
    assert_eq!(rows.len(), 1, "{arrears:#}");
    assert_eq!(rows[0]["customer_id"], json!(customer));
    assert_eq!(rows[0]["overpaid"]["minor"], 2_000);
    assert_eq!(rows[0]["invoices"], 1);

    // An overpayment is the same fact reached the other way.
    let (other, second) = a_hundred_dollar_invoice(&isp, "Tikrit Link").await;
    isp.ok(
        "isp.payment.record",
        json!({"customer_id": other, "invoice_id": second, "method": "bank transfer",
               "amount": {"currency": "USD", "minor": 12_500}}),
    )
    .await;
    let theirs = isp.ok("isp.arrears", json!({"customer_id": other})).await;
    assert_eq!(theirs["overpaid"][0]["minor"], 2_500, "{theirs:#}");
    assert_eq!(list(&theirs["customers_overpaid"]).len(), 1);
    let everyone = isp.ok("isp.arrears", json!({})).await;
    assert_eq!(everyone["overpaid"][0]["minor"], 4_500, "{everyone:#}");

    // Withdrawing the credit takes the first customer out of credit again.
    let note = list(
        &isp.ok("isp.credits", json!({"customer_id": customer}))
            .await,
    )[0]["id"]
        .clone();
    isp.ok(
        "isp.credit.void",
        json!({"id": note, "reason": "issued twice"}),
    )
    .await;
    let balance = isp.ok("isp.customer.get", json!({"id": customer})).await["balance"].clone();
    assert!(list(&balance["overpaid"]).is_empty(), "{balance:#}");
    assert_eq!(balance["invoices_overpaid"], 0);
}

/// The warning on a note against the account says it waits to be set against
/// an invoice. This is the setting.
#[tokio::test]
async fn a_credit_on_the_account_can_be_set_against_an_invoice_and_then_reduces_it() {
    let isp = Isp::start(false, &[]).await;
    let (customer, invoice) = a_hundred_dollar_invoice(&isp, "Samarra Fibre").await;
    let issued = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "amount": {"currency": "USD", "minor": 10_000},
                "reason": "agreed settlement for the tower move",
            }),
        )
        .await;
    let note = issued["credit"]["id"].as_str().expect("an id").to_string();
    assert!(
        list(&issued["warnings"]).iter().any(|warning| warning
            .as_str()
            .is_some_and(|text| text.contains("isp.credit.allocate"))),
        "the warning does not say how it is set against an invoice: {issued:#}"
    );
    // Until then the debt stands, as the warning said.
    let before = isp.ok("isp.customer.get", json!({"id": customer})).await["balance"].clone();
    assert_eq!(before["outstanding"][0]["minor"], 10_000);
    assert_eq!(before["credit_on_account"][0]["minor"], 10_000);

    let allocated = isp
        .ok(
            "isp.credit.allocate",
            json!({"id": note, "invoice_id": invoice}),
        )
        .await;
    assert_eq!(allocated["credit"]["invoice_id"], json!(invoice));
    assert_eq!(
        allocated["credit"]["number"], issued["credit"]["number"],
        "it is the same document, not a new one"
    );
    assert_eq!(allocated["credit"]["amount"]["minor"], 10_000);
    assert_eq!(allocated["credit"]["at"], issued["credit"]["at"]);
    assert!(
        allocated["credit"]["allocated_at"].is_i64(),
        "the note does not say when it was set against the invoice: {allocated:#}"
    );
    assert_eq!(allocated["invoice"]["status"], "credited");
    assert_eq!(allocated["invoice"]["outstanding"]["minor"], 0);
    assert_eq!(allocated["invoice"]["credited"]["minor"], 10_000);

    let after = isp.ok("isp.customer.get", json!({"id": customer})).await["balance"].clone();
    assert!(list(&after["outstanding"]).is_empty(), "{after:#}");
    assert!(list(&after["credit_on_account"]).is_empty(), "{after:#}");
    let arrears = isp.ok("isp.arrears", json!({})).await;
    assert!(list(&arrears["customers"]).is_empty(), "{arrears:#}");
    // It is listed under the invoice now, and it is in the audit trail.
    let against = list(&isp.ok("isp.credits", json!({"invoice_id": invoice})).await);
    assert_eq!(against.len(), 1);
    assert!(
        isp.audit()
            .iter()
            .any(|(source, message)| source == "isp.credit.allocate"
                && message.contains(issued["credit"]["number"].as_str().unwrap_or("?"))),
        "setting a credit against an invoice left no audit entry"
    );

    // Voiding it afterwards puts the debt back like any other note.
    isp.ok(
        "isp.credit.void",
        json!({"id": note, "reason": "the settlement fell through"}),
    )
    .await;
    let view = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
    assert_eq!(view["outstanding"]["minor"], 10_000);
}

#[tokio::test]
async fn setting_a_credit_against_an_invoice_is_refused_when_it_would_invent_something() {
    let isp = Isp::start(false, &[]).await;
    let (customer, invoice) = a_hundred_dollar_invoice(&isp, "Samarra Fibre").await;
    let (_, theirs) = a_hundred_dollar_invoice(&isp, "Somebody Else").await;
    let on_account = |minor: i64| {
        json!({
            "customer_id": customer,
            "amount": {"currency": "USD", "minor": minor},
            "reason": "agreed settlement",
        })
    };
    let credit = |issued: serde_json::Value| issued["credit"]["id"].clone();

    // More than the invoice was ever charged.
    let too_much = credit(isp.ok("isp.credit.issue", on_account(15_000)).await);
    let err = isp
        .err(
            "isp.credit.allocate",
            json!({"id": too_much, "invoice_id": invoice}),
        )
        .await;
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("more than was ever charged"),
        "{}",
        err.message
    );

    // Somebody else's invoice.
    let note = credit(isp.ok("isp.credit.issue", on_account(3_000)).await);
    let err = isp
        .err(
            "isp.credit.allocate",
            json!({"id": note, "invoice_id": theirs}),
        )
        .await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("another customer"), "{}", err.message);

    // An invoice nobody owes yet, and one nobody owes any more.
    let earlier = the_month_before();
    let draft = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": earlier.0,
                   "period_end": earlier.1}),
        )
        .await["invoice"]["id"]
        .clone();
    let err = isp
        .err(
            "isp.credit.allocate",
            json!({"id": note, "invoice_id": draft}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("draft"), "{}", err.message);
    isp.ok(
        "isp.invoice.void",
        json!({"id": draft, "reason": "not needed"}),
    )
    .await;
    let err = isp
        .err(
            "isp.credit.allocate",
            json!({"id": note, "invoice_id": draft}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("void"), "{}", err.message);

    // A note or an invoice that does not exist.
    for params in [
        json!({"id": "nope", "invoice_id": invoice}),
        json!({"id": note, "invoice_id": "nope"}),
    ] {
        assert_eq!(
            isp.err("isp.credit.allocate", params).await.code,
            "not_found"
        );
    }

    // Once it is against an invoice it stays there: moving it is a void and
    // a new note, so the trail shows both.
    isp.ok(
        "isp.credit.allocate",
        json!({"id": note, "invoice_id": invoice}),
    )
    .await;
    let err = isp
        .err(
            "isp.credit.allocate",
            json!({"id": note, "invoice_id": invoice}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("already"), "{}", err.message);

    // A withdrawn note credits nothing.
    isp.ok(
        "isp.credit.void",
        json!({"id": too_much, "reason": "entered with the wrong amount"}),
    )
    .await;
    let err = isp
        .err(
            "isp.credit.allocate",
            json!({"id": too_much, "invoice_id": invoice}),
        )
        .await;
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("void"), "{}", err.message);

    // Every refusal left the invoice exactly as the one accepted note made it.
    let view = isp.ok("isp.invoice.get", json!({"id": invoice})).await;
    assert_eq!(view["credited"]["minor"], 3_000);
    assert_eq!(view["outstanding"]["minor"], 7_000);
}

/// The screens refetch on `isp.changed`, `isp.invoices.changed` and
/// `isp.payments.changed`. A credit note changes what an invoice and a
/// balance read, so each of its mutations has to reach one of those as well
/// as publishing its own topic.
#[tokio::test]
async fn a_credit_note_reaches_the_screens_that_show_the_invoice_or_the_balance_it_changed() {
    fn drained(
        events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
    ) -> Vec<(String, serde_json::Value)> {
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.topic.starts_with("isp.") {
                seen.push((event.topic, event.payload));
            }
        }
        seen
    }
    fn find<'a>(seen: &'a [(String, serde_json::Value)], topic: &str) -> &'a serde_json::Value {
        seen.iter()
            .find(|(name, _)| name == topic)
            .map(|(_, payload)| payload)
            .unwrap_or_else(|| panic!("no `{topic}` among {seen:#?}"))
    }

    let isp = Isp::start(false, &[]).await;
    let (customer, invoice) = a_hundred_dollar_invoice(&isp, "Event Emma").await;
    let mut events = isp.app.subscribe();

    // Against an invoice: the invoice's settlement changed.
    let note = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "invoice_id": invoice,
                "amount": {"currency": "USD", "minor": 4_000},
                "reason": "three days of outage",
            }),
        )
        .await["credit"]["id"]
        .clone();
    let seen = drained(&mut events);
    let own = find(&seen, "isp.credits.changed");
    assert_eq!(own["change"], "issued");
    assert_eq!(own["ids"], json!([note]));
    assert_eq!(own["customer_ids"], json!([customer]));
    assert_eq!(own["invoice_ids"], json!([invoice]));
    let followed = find(&seen, "isp.invoices.changed");
    assert_eq!(followed["change"], "updated");
    assert_eq!(followed["ids"], json!([invoice]));
    assert_eq!(followed["customer_ids"], json!([customer]));

    isp.ok(
        "isp.credit.void",
        json!({"id": note, "reason": "raised twice"}),
    )
    .await;
    let seen = drained(&mut events);
    assert_eq!(find(&seen, "isp.credits.changed")["change"], "voided");
    assert_eq!(find(&seen, "isp.invoices.changed")["ids"], json!([invoice]));

    // On the account: no invoice moved, but the customer's balance did.
    let on_account = isp
        .ok(
            "isp.credit.issue",
            json!({
                "customer_id": customer,
                "amount": {"currency": "USD", "minor": 1_000},
                "reason": "agreed settlement",
            }),
        )
        .await["credit"]["id"]
        .clone();
    let seen = drained(&mut events);
    assert_eq!(find(&seen, "isp.credits.changed")["invoice_ids"], json!([]));
    assert_eq!(find(&seen, "isp.changed")["kinds"], json!(["credits"]));
    assert!(
        seen.iter()
            .all(|(topic, _)| topic != "isp.invoices.changed"),
        "no invoice changed: {seen:#?}"
    );

    // Set against the invoice: now one did.
    isp.ok(
        "isp.credit.allocate",
        json!({"id": on_account, "invoice_id": invoice}),
    )
    .await;
    let seen = drained(&mut events);
    let own = find(&seen, "isp.credits.changed");
    assert_eq!(own["change"], "allocated");
    assert_eq!(own["invoice_ids"], json!([invoice]));
    assert_eq!(find(&seen, "isp.invoices.changed")["ids"], json!([invoice]));

    // A read publishes nothing.
    isp.ok("isp.credits", json!({})).await;
    assert!(drained(&mut events).is_empty(), "reads are silent");
}

/// Every figure about one invoice has to be the same figure wherever it is
/// read: on the invoice, in the customer's balance, in arrears and in
/// revenue — in dinars, which have no minor unit, and in dollars, which
/// never meet them.
#[tokio::test]
async fn the_invoice_the_balance_and_the_reports_agree_in_dinars_and_in_dollars() {
    fn iqd(minor: i64) -> serde_json::Value {
        json!({"currency": "IQD", "minor": minor, "digits": 0, "text": minor.to_string()})
    }
    let isp = Isp::start(false, &[]).await;
    let period = last_month();

    // 50,000 dinars invoiced; 30,000 paid; 20,000 credited on the account
    // and then set against the invoice.
    let dinars = isp.customer("Dijla Net", "IQD", json!({})).await;
    let home = isp
        .tariff("Home", 10, "flat", 50_000, "IQD", "monthly")
        .await;
    isp.subscribe(&dinars, &home, None).await;
    let invoice = isp.invoice(&dinars, period, None, None).await["id"].clone();
    isp.ok(
        "isp.payment.record",
        json!({"customer_id": dinars, "invoice_id": invoice, "method": "cash",
               "amount": {"currency": "IQD", "minor": 30_000}}),
    )
    .await;
    let note = isp
        .ok(
            "isp.credit.issue",
            json!({"customer_id": dinars, "reason": "a week of outage",
                   "amount": {"currency": "IQD", "minor": 20_000}}),
        )
        .await["credit"]["id"]
        .clone();
    let balance = isp.ok("isp.customer.get", json!({"id": dinars})).await["balance"].clone();
    assert_eq!(
        (&balance["outstanding"], &balance["credit_on_account"]),
        (&json!([iqd(20_000)]), &json!([iqd(20_000)])),
        "owed and credited stand side by side until the note is set against the invoice"
    );
    let allocated = isp
        .ok(
            "isp.credit.allocate",
            json!({"id": note, "invoice_id": invoice}),
        )
        .await;
    let view = &allocated["invoice"];
    assert_eq!(
        (
            &view["status"],
            &view["paid"],
            &view["credited"],
            &view["outstanding"]
        ),
        (&json!("credited"), &iqd(30_000), &iqd(20_000), &iqd(0))
    );
    let balance = isp.ok("isp.customer.get", json!({"id": dinars})).await["balance"].clone();
    for figure in ["outstanding", "credit_on_account", "on_account", "overpaid"] {
        assert_eq!(balance[figure], json!([]), "{figure}: {balance:#}");
    }

    // A goodwill 5,000 on top is inside the cap (25,000 of 50,000 credited)
    // and leaves them 5,000 in credit — said once, in every place.
    let goodwill = isp
        .ok(
            "isp.credit.issue",
            json!({"customer_id": dinars, "invoice_id": invoice, "reason": "goodwill",
                   "amount": {"currency": "IQD", "minor": 5_000}}),
        )
        .await;
    assert_eq!(goodwill["invoice"]["outstanding"], iqd(-5_000));
    assert_eq!(
        goodwill["invoice"]["status"], "credited",
        "30,000 of 50,000 arrived as money: it took the credit to clear it"
    );
    let balance = isp.ok("isp.customer.get", json!({"id": dinars})).await["balance"].clone();
    assert_eq!(
        (&balance["outstanding"], &balance["overpaid"]),
        (&json!([]), &json!([iqd(5_000)]))
    );

    // 100.00 dollars invoiced to somebody else, half of it paid — in dinars,
    // at 1,320 to the dollar.
    let dollars = isp.customer("Hudson Wireless", "USD", json!({})).await;
    let business = isp
        .tariff("Business", 10, "flat", 10_000, "USD", "monthly")
        .await;
    isp.subscribe(&dollars, &business, None).await;
    let theirs = isp.invoice(&dollars, period, None, None).await["id"].clone();
    let paid = isp
        .ok(
            "isp.payment.record",
            json!({"customer_id": dollars, "invoice_id": theirs, "method": "cash",
                   "amount": {"currency": "IQD", "minor": 66_000},
                   "conversion": {"from": "USD", "to": "IQD", "rate": 1_320}}),
        )
        .await;
    assert_eq!(paid["invoice"]["paid"]["minor"], 5_000);
    assert_eq!(paid["invoice"]["outstanding"]["minor"], 5_000);

    // Arrears: 50.00 dollars owed, 5,000 dinars the other way, not netted.
    let arrears = isp.ok("isp.arrears", json!({})).await;
    let owed: Vec<(&serde_json::Value, &serde_json::Value)> = arrears["by_currency"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|row| (&row["currency"], &row["total"]["minor"]))
        .collect();
    assert_eq!(owed, [(&json!("USD"), &json!(5_000))], "{arrears:#}");
    assert_eq!(arrears["overpaid"], json!([iqd(5_000)]));

    // Revenue: what was invoiced per currency, what came in as it came in
    // (all of it dinars), and what is still owed. Credit notes are in none
    // of the three as money.
    let revenue = isp.ok("isp.revenue", json!({"since": 0})).await;
    assert_eq!(
        revenue["issued"],
        json!([
            iqd(50_000),
            {"currency": "USD", "minor": 10_000, "digits": 2, "text": "100.00"}
        ])
    );
    assert_eq!(revenue["received"], json!([iqd(96_000)]));
    assert_eq!(
        revenue["outstanding"],
        json!([{"currency": "USD", "minor": 5_000, "digits": 2, "text": "50.00"}])
    );
    // For each invoice: total = paid + credited + outstanding, to the unit.
    for id in [&invoice, &theirs] {
        let view = isp.ok("isp.invoice.get", json!({"id": id})).await;
        let minor = |field: &str| view[field]["minor"].as_i64().expect("a figure");
        assert_eq!(
            minor("total"),
            minor("paid") + minor("credited") + minor("outstanding")
        );
    }
}
