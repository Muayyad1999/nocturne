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

    // It is in the audit trail, with its reason, like every other mutation.
    let audited = isp
        .audit()
        .into_iter()
        .any(|(source, message)| source == "isp.credit.issue" && message.contains("never came up"));
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
