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
    assert_eq!(first["next_cursor"], json!("2"));
    let second = isp
        .ok("isp.customers", json!({"limit": 2, "cursor": "2"}))
        .await;
    assert_eq!(list(&second["customers"]).len(), 2);
    let last = isp
        .ok("isp.customers", json!({"limit": 2, "cursor": "4"}))
        .await;
    assert_eq!(list(&last["customers"]).len(), 1);
    assert!(last["next_cursor"].is_null(), "the last page says so");
    assert_eq!(
        isp.err("isp.customers", json!({"cursor": "soon"}))
            .await
            .code,
        "invalid"
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
