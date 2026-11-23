//! The surface itself: what is registered, what it refuses, what it
//! publishes and what it writes to the audit trail.

mod common;

use common::{DAY_MS, Isp, last_month, list};
use serde_json::{Value, json};

/// Every `isp.*` event published so far, oldest first. Nothing waits on a
/// clock: `publish` happens inside the call, so by the time it has returned
/// the events are in the channel.
fn drained(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
) -> Vec<(String, Value)> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic.starts_with("isp.") {
            seen.push((event.topic, event.payload));
        }
    }
    seen
}

#[tokio::test]
async fn every_method_in_the_specification_is_registered() {
    let isp = Isp::start(false, &[]).await;
    let methods = isp.app.methods();
    for method in [
        "isp.currencies",
        "isp.summary",
        "isp.customers",
        "isp.customer.get",
        "isp.customer.save",
        "isp.customer.state",
        "isp.customer.delete",
        "isp.tariffs",
        "isp.tariff.save",
        "isp.tariff.delete",
        "isp.subscriptions",
        "isp.subscription.create",
        "isp.subscription.amend",
        "isp.subscription.end",
        "isp.links",
        "isp.link.save",
        "isp.link.delete",
        "isp.invoices",
        "isp.invoice.get",
        "isp.invoice.draft",
        "isp.invoice.run",
        "isp.invoice.line.add",
        "isp.invoice.line.remove",
        "isp.invoice.issue",
        "isp.invoice.void",
        "isp.invoice.reissue",
        "isp.payments",
        "isp.payment.record",
        "isp.payment.void",
        "isp.credits",
        "isp.credit.issue",
        "isp.credit.allocate",
        "isp.credit.void",
        "isp.arrears",
        "isp.revenue",
        "isp.usage",
        "isp.capacity",
        "isp.margin",
    ] {
        assert!(methods.contains(&method), "{method} is not registered");
    }
    assert_eq!(
        isp.app.rpc("isp.nope", Value::Null).await.unwrap_err().code,
        "unknown_method"
    );
    // The settings section is declared with its defaults.
    let settings = isp.ok("settings.get", json!({"section": "isp"})).await;
    assert_eq!(settings["payment_terms_days"], json!(14));
    assert_eq!(settings["arrears_buckets"], json!([30, 60, 90]));
    assert_eq!(settings["usage_basis"], json!("greater"));
    assert_eq!(settings["invoice_number_prefix"], json!(""));
    assert_eq!(
        settings["utc_offset_minutes"],
        json!(0),
        "the business's calendar is UTC until somebody says otherwise"
    );
}

#[tokio::test]
async fn the_events_say_what_a_screen_has_to_refetch() {
    let isp = Isp::start(false, &[]).await;
    let mut events = isp.app.subscribe();

    let customer = isp.customer("Event Emma", "USD", json!({})).await;
    let kinds: Vec<Value> = drained(&mut events)
        .into_iter()
        .filter(|(topic, _)| topic == "isp.changed")
        .map(|(_, payload)| payload["kinds"].clone())
        .collect();
    assert_eq!(kinds, [json!(["customers"])]);

    let tariff = isp
        .tariff("Plan", 10, "flat", 1_000, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let kinds: Vec<String> = drained(&mut events)
        .into_iter()
        .filter(|(topic, _)| topic == "isp.changed")
        .map(|(_, payload)| payload["kinds"][0].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(kinds, ["tariffs", "subscriptions"]);

    // Drafting, issuing and paying each say which invoice moved.
    let drafted = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": last_month().0,
                   "period_end": last_month().1}),
        )
        .await;
    let invoice_id = drafted["invoice"]["id"]
        .as_str()
        .expect("an id")
        .to_string();
    let seen = drained(&mut events);
    assert!(
        seen.iter()
            .any(|(topic, payload)| topic == "isp.invoices.changed"
                && payload["change"] == json!("drafted")
                && payload["ids"][0] == json!(invoice_id)),
        "{seen:?}"
    );

    isp.ok("isp.invoice.issue", json!({"id": invoice_id})).await;
    let seen = drained(&mut events);
    let issued = seen
        .iter()
        .find(|(topic, payload)| {
            topic == "isp.invoices.changed" && payload["change"] == json!("issued")
        })
        .expect("an issued event");
    assert_eq!(issued.1["customer_ids"][0], json!(customer));
    assert_eq!(issued.1["currency"], json!("USD"));
    assert_eq!(issued.1["total_minor"], json!(1_000));
    assert!(issued.1["due_at"].is_i64());

    isp.ok(
        "isp.payment.record",
        json!({"customer_id": customer, "invoice_id": invoice_id, "method": "cash",
               "amount": {"currency": "USD", "minor": 1_000}}),
    )
    .await;
    let seen = drained(&mut events);
    let paid = seen
        .iter()
        .find(|(topic, _)| topic == "isp.payments.changed")
        .expect("a payment event");
    assert_eq!(paid.1["change"], json!("recorded"));
    assert_eq!(paid.1["invoice_ids"][0], json!(invoice_id));
    assert_eq!(paid.1["customer_ids"][0], json!(customer));

    // A read publishes nothing.
    isp.ok("isp.summary", json!({})).await;
    isp.ok("isp.arrears", json!({})).await;
    assert!(drained(&mut events).is_empty(), "reads are silent");
}

#[tokio::test]
async fn every_mutation_is_in_the_audit_trail_and_no_read_is() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Audit Ana", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    let subscription = isp.subscribe(&customer, &tariff, None).await;
    let invoice = isp.invoice(&customer, last_month(), None, None).await;
    let invoice_id = invoice["id"].as_str().expect("an id").to_string();
    let payment = isp
        .ok(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": invoice_id,
                "method": "cash",
                "amount": {"currency": "IQD", "minor": 33_000},
                "conversion": {"from": "USD", "to": "IQD", "rate": 1_320},
            }),
        )
        .await;
    isp.ok(
        "isp.payment.void",
        json!({"id": payment["payment"]["id"], "reason": "wrong account"}),
    )
    .await;
    isp.ok(
        "isp.invoice.void",
        json!({"id": invoice_id, "reason": "reissued for the right period"}),
    )
    .await;
    isp.ok(
        "isp.subscription.end",
        json!({"id": subscription, "reason": "cancelled by the customer"}),
    )
    .await;
    isp.ok(
        "isp.link.save",
        json!({"name": "Feed", "tower": "Tower A", "capacity_mbit": 100}),
    )
    .await;
    isp.ok(
        "isp.customer.state",
        json!({"id": customer, "state": "suspended", "reason": "non-payment"}),
    )
    .await;

    let audit = isp.audit();
    let sources: Vec<&str> = audit.iter().map(|(source, _)| source.as_str()).collect();
    for expected in [
        "isp.customer.save",
        "isp.tariff.save",
        "isp.subscription.create",
        "isp.invoice.draft",
        "isp.invoice.issue",
        "isp.payment.record",
        "isp.payment.void",
        "isp.invoice.void",
        "isp.subscription.end",
        "isp.link.save",
        "isp.customer.state",
    ] {
        assert!(
            sources.contains(&expected),
            "{expected} left no audit entry; sources were {sources:?}"
        );
    }
    // The payment's entry carries the rate it was taken at, in words.
    let recorded = audit
        .iter()
        .find(|(source, _)| source == "isp.payment.record")
        .expect("the payment's entry");
    assert!(
        recorded.1.contains("1320 IQD per 1 USD") && recorded.1.contains("25.00 USD"),
        "{}",
        recorded.1
    );
    // Suspending says, in the trail, that nothing was sent to a device.
    let suspended = audit
        .iter()
        .find(|(source, _)| source == "isp.customer.state")
        .expect("the state entry");
    assert!(
        suspended.1.contains("from active to suspended"),
        "{}",
        suspended.1
    );

    let before = isp.audit().len();
    for (method, params) in [
        ("isp.summary", json!({})),
        ("isp.arrears", json!({})),
        ("isp.customers", json!({})),
        ("isp.invoices", json!({})),
        ("isp.payments", json!({})),
        ("isp.usage", json!({})),
        ("isp.currencies", json!({})),
        ("isp.revenue", json!({"since": 0})),
    ] {
        isp.ok(method, params).await;
    }
    assert_eq!(isp.audit().len(), before, "a read is not a mutation");
}

/// An entry's message is the line other features repeat — an incident's
/// timeline takes in every entry filed under one of its devices, and a log
/// search hands messages on — so it names a customer by id and carries
/// nothing anybody typed about them. Who it was and why are in the entry's
/// detail, where a person reading the trail finds them. And a customer's
/// record is not filed under the device that serves them: it is not part of
/// that device's history.
#[tokio::test]
async fn the_trail_names_a_customer_by_id_and_keeps_what_was_typed_about_them_out_of_its_messages()
{
    const STATE_REASON: &str = "unpaid since March, rang her on 0770 123 4567";
    const LINE: &str = "Router for Jane's flat on Rasheed Street";
    const METHOD: &str = "cash, handed over by her son Ali";
    const PAYMENT_VOID: &str = "Ali paid it twice, Jane says";
    const CREDIT: &str = "Jane's line was down for a week";
    const ON_ACCOUNT: &str = "goodwill after Jane complained";
    const CREDIT_VOID: &str = "the outage was Jane's own router";
    const REISSUE: &str = "sent to Jane's old address";
    const VOID: &str = "Jane moved away";

    let isp = Isp::start(false, &["TOWER-3"]).await;
    let device = isp.device("TOWER-3");
    let jane = isp
        .customer(
            "Jane Doe",
            "USD",
            json!({"device_id": device, "interface": "ether5", "phone": "0770 123 4567"}),
        )
        .await;
    let tariff = isp
        .tariff("Home 10", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&jane, &tariff, None).await;
    isp.ok(
        "isp.customer.state",
        json!({"id": jane, "state": "suspended", "reason": STATE_REASON}),
    )
    .await;
    isp.ok("isp.customer.state", json!({"id": jane, "state": "active"}))
        .await;

    // A draft with a line typed by hand, and one taken off again.
    let draft = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": jane, "month": "2026-09"}),
        )
        .await;
    let invoice = draft["invoice"]["id"].as_str().expect("an id").to_string();
    let added = isp
        .ok(
            "isp.invoice.line.add",
            json!({"invoice_id": invoice, "description": LINE,
                   "amount": {"currency": "USD", "minor": 1_000}}),
        )
        .await;
    let line = list(&added["lines"])
        .into_iter()
        .find(|line| line["description"] == json!(LINE))
        .expect("the line typed by hand");
    isp.ok(
        "isp.invoice.line.remove",
        json!({"invoice_id": invoice, "line_id": line["id"]}),
    )
    .await;
    isp.ok("isp.invoice.issue", json!({"id": invoice})).await;

    // A payment and a credit note against it, each voided with a reason.
    let payment = isp
        .ok(
            "isp.payment.record",
            json!({"customer_id": jane, "invoice_id": invoice, "method": METHOD,
                   "amount": {"currency": "USD", "minor": 500}}),
        )
        .await;
    isp.ok(
        "isp.payment.void",
        json!({"id": payment["payment"]["id"], "reason": PAYMENT_VOID}),
    )
    .await;
    let credit = isp
        .ok(
            "isp.credit.issue",
            json!({"customer_id": jane, "invoice_id": invoice, "reason": CREDIT,
                   "amount": {"currency": "USD", "minor": 300}}),
        )
        .await;
    isp.ok(
        "isp.credit.void",
        json!({"id": credit["credit"]["id"], "reason": CREDIT_VOID}),
    )
    .await;
    // The invoice reissued, and its replacement voided before it went out.
    let reissued = isp
        .ok(
            "isp.invoice.reissue",
            json!({"id": invoice, "reason": REISSUE}),
        )
        .await;
    isp.ok(
        "isp.invoice.void",
        json!({"id": reissued["draft"]["id"], "reason": VOID}),
    )
    .await;

    // A month billed by a run, and a note on the account set against it.
    let run = isp
        .ok(
            "isp.invoice.run",
            json!({"month": "2026-10", "dry_run": false, "issue": true}),
        )
        .await;
    let billed = run["invoices"][0]["invoice_id"].clone();
    let on_account = isp
        .ok(
            "isp.credit.issue",
            json!({"customer_id": jane, "reason": ON_ACCOUNT,
                   "amount": {"currency": "USD", "minor": 200}}),
        )
        .await;
    isp.ok(
        "isp.credit.allocate",
        json!({"id": on_account["credit"]["id"], "invoice_id": billed}),
    )
    .await;

    // And a customer taken off again before anything was billed.
    let john = isp.customer("John Roe", "USD", json!({})).await;
    isp.ok("isp.customer.delete", json!({"id": john})).await;

    let entries: Vec<_> = isp
        .audit_entries()
        .into_iter()
        .filter(|entry| entry.source.starts_with("isp."))
        .collect();
    let typed = [
        "Jane Doe",
        "Jane",
        "John Roe",
        "0770 123 4567",
        STATE_REASON,
        LINE,
        METHOD,
        PAYMENT_VOID,
        CREDIT,
        ON_ACCOUNT,
        CREDIT_VOID,
        REISSUE,
        VOID,
    ];
    for entry in &entries {
        for text in typed {
            assert!(
                !entry.message.contains(text),
                "{} says `{text}` in its message: {}",
                entry.source,
                entry.message
            );
        }
        if entry.source.starts_with("isp.customer.") {
            assert_eq!(
                entry.device_id, None,
                "{} is filed under the device that serves the customer",
                entry.source
            );
        }
    }
    let under_device: Vec<_> = isp
        .app
        .store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".to_string()),
            device_id: Some(device.clone()),
            ..Default::default()
        })
        .expect("the device's trail")
        .into_iter()
        .filter(|entry| entry.source.starts_with("isp."))
        .collect();
    assert!(
        under_device.is_empty(),
        "the serving device's history holds the customer's records: {:?}",
        under_device
            .iter()
            .map(|entry| &entry.message)
            .collect::<Vec<_>>()
    );

    // Who and why are still there for a person to read, in the detail.
    let detail = |source: &str, key: &str| -> Vec<Value> {
        entries
            .iter()
            .filter(|entry| entry.source == source)
            .filter_map(|entry| entry.body.as_ref().map(|body| body[key].clone()))
            .collect()
    };
    for (source, key, expected) in [
        ("isp.customer.save", "name", "Jane Doe"),
        ("isp.customer.state", "reason", STATE_REASON),
        ("isp.invoice.line.add", "description", LINE),
        ("isp.payment.record", "method", METHOD),
        ("isp.payment.void", "reason", PAYMENT_VOID),
        ("isp.credit.issue", "reason", CREDIT),
        ("isp.credit.issue", "reason", ON_ACCOUNT),
        ("isp.credit.void", "reason", CREDIT_VOID),
        ("isp.invoice.reissue", "reason", REISSUE),
        ("isp.invoice.void", "reason", VOID),
        ("isp.customer.delete", "name", "John Roe"),
    ] {
        assert!(
            detail(source, key).contains(&json!(expected)),
            "{source} keeps no `{key}` of `{expected}` in its detail: {:?}",
            detail(source, key)
        );
    }
    // The messages say which customer by id.
    let state = entries
        .iter()
        .find(|entry| {
            entry.source == "isp.customer.state"
                && entry.message.contains("from active to suspended")
        })
        .expect("the suspension");
    assert!(state.message.contains(&jane), "{}", state.message);
}

#[tokio::test]
async fn the_refusals_say_what_to_do_instead() {
    let isp = Isp::start(false, &["RB1"]).await;
    let customer = isp.customer("Refusal Rafi", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 2_500, "USD", "monthly")
        .await;

    // A customer hung off a device that does not exist.
    assert_eq!(
        isp.err(
            "isp.customer.save",
            json!({"name": "Nowhere", "currency": "USD", "device_id": "nosuchdevice"}),
        )
        .await
        .code,
        "not_found"
    );
    // A tariff with no megabits, and a negative price.
    assert_eq!(
        isp.err(
            "isp.tariff.save",
            json!({"name": "Free", "unit": "flat", "price": {"currency": "USD", "minor": 0}}),
        )
        .await
        .code,
        "invalid"
    );
    assert_eq!(
        isp.err(
            "isp.tariff.save",
            json!({"name": "Negative", "mbit": 10, "unit": "flat",
                   "price": {"currency": "USD", "minor": -1}}),
        )
        .await
        .code,
        "invalid"
    );
    // Two tariffs of the same name.
    assert_eq!(
        isp.err(
            "isp.tariff.save",
            json!({"name": "Plan", "mbit": 5, "unit": "flat",
                   "price": {"currency": "USD", "minor": 100}}),
        )
        .await
        .code,
        "conflict"
    );
    // A unit, period or state that is not one of the known ones.
    for bad in [
        json!({"name": "U", "mbit": 1, "unit": "per_gigabyte", "price": {"currency": "USD", "minor": 1}}),
        json!({"name": "P", "mbit": 1, "unit": "flat", "period": "fortnightly", "price": {"currency": "USD", "minor": 1}}),
    ] {
        assert_eq!(isp.err("isp.tariff.save", bad).await.code, "invalid");
    }
    assert_eq!(
        isp.err(
            "isp.customer.state",
            json!({"id": customer, "state": "disconnected"}),
        )
        .await
        .code,
        "invalid"
    );
    // A tariff somebody is on cannot be deleted out from under them.
    isp.subscribe(&customer, &tariff, None).await;
    let err = isp.err("isp.tariff.delete", json!({"id": tariff})).await;
    assert_eq!(err.code, "conflict");
    assert!(
        err.message.contains("End their subscriptions"),
        "{}",
        err.message
    );
    // A period that ends before it starts.
    let err = isp
        .err(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": 100, "period_end": 100}),
        )
        .await;
    assert!(
        err.message.contains("ends after it starts"),
        "{}",
        err.message
    );
    // An invoice cannot be issued into the future, or fall due before it was.
    let draft = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": last_month().0,
                   "period_end": last_month().1}),
        )
        .await;
    let id = draft["invoice"]["id"].as_str().expect("an id").to_string();
    assert_eq!(
        isp.err(
            "isp.invoice.issue",
            json!({"id": id, "issued_at": nocturne_app::util::now_ms() + DAY_MS}),
        )
        .await
        .code,
        "invalid"
    );
    let err = isp
        .err(
            "isp.invoice.issue",
            json!({"id": id, "issued_at": nocturne_app::util::now_ms() - DAY_MS,
                   "due_at": nocturne_app::util::now_ms() - 2 * DAY_MS}),
        )
        .await;
    assert!(err.message.contains("fall due before"), "{}", err.message);
    // A payment of nothing, or with no method.
    isp.ok("isp.invoice.issue", json!({"id": id})).await;
    assert_eq!(
        isp.err(
            "isp.payment.record",
            json!({"customer_id": customer, "invoice_id": id, "method": "cash",
                   "amount": {"currency": "USD", "minor": 0}}),
        )
        .await
        .code,
        "invalid"
    );
    let err = isp
        .err(
            "isp.payment.record",
            json!({"customer_id": customer, "invoice_id": id,
                   "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
    assert!(
        err.message.contains("how the money came in"),
        "{}",
        err.message
    );
    // A payment against somebody else's invoice.
    let other = isp.customer("Someone Else", "USD", json!({})).await;
    let err = isp
        .err(
            "isp.payment.record",
            json!({"customer_id": other, "invoice_id": id, "method": "cash",
                   "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
    assert!(err.message.contains("another customer"), "{}", err.message);
    // Things that are simply not there.
    for (method, params) in [
        ("isp.invoice.get", json!({"id": "nope"})),
        ("isp.customer.get", json!({"id": "nope"})),
        ("isp.payment.void", json!({"id": "nope", "reason": "x"})),
        ("isp.capacity", json!({"link_id": "nope"})),
    ] {
        assert_eq!(isp.err(method, params).await.code, "not_found", "{method}");
    }
    // An account code belongs to one customer.
    isp.ok("isp.customer.save", json!({"id": customer, "code": "C-1"}))
        .await;
    assert_eq!(
        isp.err("isp.customer.save", json!({"id": other, "code": "C-1"}),)
            .await
            .code,
        "conflict"
    );
    // An amendment that changes nothing is a mistake worth saying out loud.
    let subscription = list(&isp.ok("isp.subscriptions", json!({})).await)[0]["id"].clone();
    let err = isp
        .err("isp.subscription.amend", json!({"id": subscription}))
        .await;
    assert!(
        err.message
            .contains("changes the megabits, the price or both"),
        "{}",
        err.message
    );
}

/// A payment taken with no date is taken now, and "the last thirty days"
/// asked for straight afterwards — in the same millisecond, as often as not
/// on a fast machine — includes it.
#[tokio::test]
async fn the_summary_counts_a_payment_taken_in_the_millisecond_it_is_asked_in() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Prompt Payer", "USD", json!({})).await;
    for round in 1..=300_i64 {
        isp.ok(
            "isp.payment.record",
            json!({"customer_id": customer, "method": "cash",
                   "amount": {"currency": "USD", "minor": 1}}),
        )
        .await;
        let summary = isp.ok("isp.summary", json!({})).await;
        assert_eq!(
            summary["received_30d"][0]["minor"],
            json!(round),
            "after {round} payment(s) the last thirty days hold {}",
            summary["received_30d"]
        );
    }
}

#[tokio::test]
async fn long_lists_are_paged_and_filtered() {
    let isp = Isp::start(false, &[]).await;
    for index in 0..5 {
        isp.customer(
            &format!("Customer {index}"),
            if index % 2 == 0 { "USD" } else { "IQD" },
            json!({"tower": if index < 2 { "Tower A" } else { "Tower B" },
                   "phone": format!("0770 00 {index}")}),
        )
        .await;
    }
    let first = isp.ok("isp.customers", json!({"limit": 2})).await;
    assert_eq!(first["matching"], json!(5));
    assert_eq!(list(&first["customers"]).len(), 2);
    assert!(first["next_cursor"].is_string(), "{first}");
    let second = isp
        .ok(
            "isp.customers",
            json!({"limit": 2, "cursor": first["next_cursor"]}),
        )
        .await;
    assert_eq!(list(&second["customers"]).len(), 2);
    let last = isp
        .ok(
            "isp.customers",
            json!({"limit": 2, "cursor": second["next_cursor"]}),
        )
        .await;
    assert_eq!(list(&last["customers"]).len(), 1);
    assert!(last["next_cursor"].is_null(), "the last page says so");
    assert_eq!(
        isp.err("isp.customers", json!({"cursor": "soon"}))
            .await
            .code,
        "invalid"
    );
    // A count of rows already received is what the cursor used to be. It is
    // refused rather than read as a position, because it no longer is one.
    let counted = isp
        .err("isp.customers", json!({"limit": 2, "cursor": "2"}))
        .await;
    assert_eq!(counted.code, "invalid");
    assert!(
        counted.message.contains("next_cursor"),
        "{}",
        counted.message
    );

    // Filters: by currency, by tower, and free text over the details.
    assert_eq!(
        isp.ok("isp.customers", json!({"currency": "IQD"})).await["matching"],
        json!(2)
    );
    assert_eq!(
        isp.ok("isp.customers", json!({"tower": "Tower A"})).await["matching"],
        json!(2)
    );
    assert_eq!(
        isp.ok("isp.customers", json!({"query": "customer 3"}))
            .await["matching"],
        json!(1)
    );
    assert_eq!(
        isp.ok("isp.customers", json!({"query": "0770 00 4"})).await["matching"],
        json!(1),
        "a search finds a phone number"
    );
    assert_eq!(
        isp.ok("isp.customers", json!({"query": "%"})).await["matching"],
        json!(0),
        "a wildcard is matched literally, not as a pattern"
    );
    assert_eq!(
        isp.err("isp.customers", json!({"currency": "XYZ"}))
            .await
            .code,
        "invalid"
    );
}

/// Every id in a list result.
fn ids(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|row| row["id"].as_str().expect("an id").to_string())
        .collect()
}

/// A list is newest first, so whatever is written between two page requests
/// sorts in front of the page already read. The next page continues after
/// the last row the caller received — its `id` is the cursor — rather than
/// after a count of rows, which would hand back rows already seen.
#[tokio::test]
async fn a_page_of_invoices_continues_after_the_last_one_received_whatever_was_drafted_since() {
    let isp = Isp::start(false, &[]).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 1_000, "USD", "monthly")
        .await;
    for index in 0..6 {
        let customer = isp
            .customer(&format!("Paged {index}"), "USD", json!({}))
            .await;
        isp.subscribe(&customer, &tariff, None).await;
        isp.ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "month": "2026-08"}),
        )
        .await;
    }
    let first = list(&isp.ok("isp.invoices", json!({"limit": 3})).await);
    assert_eq!(first.len(), 3);

    // A run drafts the month after for everybody, and its drafts sort first.
    isp.ok(
        "isp.invoice.run",
        json!({"month": "2026-09", "dry_run": false}),
    )
    .await;
    let cursor = first.last().expect("a last row")["id"].clone();
    let second = list(
        &isp.ok("isp.invoices", json!({"limit": 3, "cursor": cursor}))
            .await,
    );
    assert_eq!(second.len(), 3);
    for id in ids(&second) {
        assert!(
            !ids(&first).contains(&id),
            "invoice {id} was on the page before as well"
        );
    }
    assert!(
        second
            .iter()
            .all(|row| row["period_start"] == first[0]["period_start"]),
        "the second page is the rest of the month the first page was of"
    );

    // Walked from the start, every invoice is on exactly one page.
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<Value> = None;
    loop {
        let mut params = json!({"limit": 4});
        if let Some(cursor) = &cursor {
            params["cursor"] = cursor.clone();
        }
        let page = list(&isp.ok("isp.invoices", params).await);
        if page.is_empty() {
            break;
        }
        cursor = Some(page.last().expect("a last row")["id"].clone());
        seen.extend(ids(&page));
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!((seen.len(), unique.len()), (12, 12), "{seen:?}");

    // What the cursor used to be — a count of rows — is not an invoice to
    // continue after, and is refused rather than guessed at.
    let counted = isp.err("isp.invoices", json!({"cursor": "3"})).await;
    assert_eq!(counted.code, "invalid");
    assert!(counted.message.contains("last"), "{}", counted.message);
}

#[tokio::test]
async fn a_page_of_payments_or_credit_notes_continues_after_the_last_one_received() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Regular Rana", "USD", json!({})).await;
    let tariff = isp
        .tariff("Plan", 10, "flat", 10_000, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let invoice = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "month": "2026-08"}),
        )
        .await;
    let invoice = isp
        .ok("isp.invoice.issue", json!({"id": invoice["invoice"]["id"]}))
        .await;
    let earlier = nocturne_app::util::now_ms() - 10 * DAY_MS;
    for day in 0..4 {
        isp.ok(
            "isp.payment.record",
            json!({"customer_id": customer, "method": "cash", "at": earlier + day * DAY_MS,
                   "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
        isp.ok(
            "isp.credit.issue",
            json!({"customer_id": customer, "invoice_id": invoice["id"], "reason": "outage",
                   "at": earlier + day * DAY_MS,
                   "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
    }
    for method in ["isp.payments", "isp.credits"] {
        let first = list(&isp.ok(method, json!({"limit": 2})).await);
        assert_eq!(first.len(), 2, "{method}");
        // One more of each, taken now: the newest, so they sort first.
        isp.ok(
            "isp.payment.record",
            json!({"customer_id": customer, "method": "cash",
                   "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
        isp.ok(
            "isp.credit.issue",
            json!({"customer_id": customer, "invoice_id": invoice["id"], "reason": "outage",
                   "amount": {"currency": "USD", "minor": 100}}),
        )
        .await;
        let cursor = first.last().expect("a last row")["id"].clone();
        let second = list(&isp.ok(method, json!({"limit": 2, "cursor": cursor})).await);
        assert_eq!(second.len(), 2, "{method}");
        for id in ids(&second) {
            assert!(
                !ids(&first).contains(&id),
                "{method}: {id} was on the page before as well"
            );
        }
        let oldest_first_page = first.last().expect("a last row")["at"].as_i64();
        assert!(
            second
                .iter()
                .all(|row| row["at"].as_i64() <= oldest_first_page),
            "{method}: the second page goes on back in time from where the first stopped"
        );
        let unknown = isp
            .err(method, json!({"cursor": "nothing-like-this"}))
            .await;
        assert_eq!(unknown.code, "invalid", "{method}");
    }
}

/// Customers are listed by name, and taking one off the first page must not
/// make the second page start one further on.
#[tokio::test]
async fn a_page_of_customers_continues_after_the_last_one_received_when_one_before_it_is_deleted() {
    let isp = Isp::start(false, &[]).await;
    let mut by_name = Vec::new();
    for name in ["Ann", "Ben", "Cat", "Dan", "Eve"] {
        by_name.push(isp.customer(name, "USD", json!({})).await);
    }
    let first = isp.ok("isp.customers", json!({"limit": 2})).await;
    assert_eq!(ids(&list(&first["customers"])), by_name[..2].to_vec());
    // Ann had nothing billed, so she can be deleted.
    isp.ok("isp.customer.delete", json!({"id": by_name[0]}))
        .await;

    let second = isp
        .ok(
            "isp.customers",
            json!({"limit": 2, "cursor": first["next_cursor"]}),
        )
        .await;
    assert_eq!(
        ids(&list(&second["customers"])),
        by_name[2..4].to_vec(),
        "the page after Ben starts with Cat"
    );
    let third = isp
        .ok(
            "isp.customers",
            json!({"limit": 2, "cursor": second["next_cursor"]}),
        )
        .await;
    assert_eq!(ids(&list(&third["customers"])), by_name[4..].to_vec());
    assert!(third["next_cursor"].is_null(), "{third}");

    // The usage report is paged by the same cursor, so the screen that puts
    // the two side by side is showing the same customers in both.
    let usage = isp
        .ok(
            "isp.usage",
            json!({"limit": 2, "cursor": first["next_cursor"]}),
        )
        .await;
    let usage_ids: Vec<String> = list(&usage["rows"])
        .iter()
        .map(|row| row["customer_id"].as_str().expect("an id").to_string())
        .collect();
    assert_eq!(usage_ids, by_name[2..4].to_vec());
}
