//! Money, through the RPC surface: the minor unit, the absence of floats, the
//! absence of a mixed total, and a conversion that still reconciles later.

mod common;

use common::{DAY_MS, Isp, assert_money_is_integer, last_month, list, money_objects};
use serde_json::{Value, json};

#[tokio::test]
async fn a_thousand_minor_units_is_ten_dollars_or_a_thousand_dinars() {
    let isp = Isp::start(false, &[]).await;
    // The same figure, 1,000 minor units, in two currencies.
    let dollars = isp.customer("Dollar Cafe", "USD", json!({})).await;
    let dinars = isp.customer("Dinar Cafe", "IQD", json!({})).await;
    let usd = isp
        .tariff("Office 10", 10, "flat", 1_000, "USD", "monthly")
        .await;
    let iqd = isp
        .tariff("Office 10 (IQD)", 10, "flat", 1_000, "IQD", "monthly")
        .await;
    isp.subscribe(&dollars, &usd, None).await;
    isp.subscribe(&dinars, &iqd, None).await;

    let period = last_month();
    let one = isp.invoice(&dollars, period, None, None).await;
    let other = isp.invoice(&dinars, period, None, None).await;
    assert_eq!(
        (
            one["total"]["minor"].as_i64(),
            one["total"]["digits"].as_u64(),
            one["total"]["text"].as_str()
        ),
        (Some(1_000), Some(2), Some("10.00")),
        "a thousand cents is ten dollars"
    );
    assert_eq!(
        (
            other["total"]["minor"].as_i64(),
            other["total"]["digits"].as_u64(),
            other["total"]["text"].as_str()
        ),
        (Some(1_000), Some(0), Some("1000")),
        "a thousand dinars is a thousand dinars: the fils is out of circulation"
    );

    // The currency list says so too, so no screen has to assume hundredths.
    let currencies = list(&isp.ok("isp.currencies", json!({})).await);
    let iqd_row = currencies
        .iter()
        .find(|row| row["code"] == json!("IQD"))
        .expect("IQD is accepted");
    assert_eq!(
        (
            iqd_row["digits"].as_u64(),
            iqd_row["scale"].as_i64(),
            iqd_row["thousand_minor_units"].as_str()
        ),
        (Some(0), Some(1), Some("1000"))
    );
    let usd_row = currencies
        .iter()
        .find(|row| row["code"] == json!("USD"))
        .expect("USD is accepted");
    assert_eq!(
        (usd_row["digits"].as_u64(), usd_row["scale"].as_i64()),
        (Some(2), Some(100))
    );
    // A three-digit currency is in the table, so nothing may assume two.
    assert!(
        currencies
            .iter()
            .any(|row| row["digits"] == json!(3) && row["code"] == json!("JOD"))
    );
}

#[tokio::test]
async fn a_float_is_refused_wherever_money_is_entered() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Rounding Errors Ltd", "USD", json!({})).await;

    // A tariff's price.
    for bad in [json!(12.5), json!(1_000.0), json!("1000"), json!(null)] {
        let err = isp
            .err(
                "isp.tariff.save",
                json!({
                    "name": format!("bad {bad}"),
                    "mbit": 10,
                    "unit": "flat",
                    "price": {"currency": "USD", "minor": bad},
                }),
            )
            .await;
        assert_eq!(err.code, "invalid", "{bad} was accepted as money");
    }
    // Including the one that looks harmless: 1000.0 is a float, and the
    // moment a float is allowed in, so is 1000.0000000001.
    let err = isp
        .err(
            "isp.tariff.save",
            json!({"name": "t", "mbit": 10, "unit": "flat",
                   "price": {"currency": "USD", "minor": 1_000.0}}),
        )
        .await;
    assert!(
        err.message.contains("whole number of minor units"),
        "{}",
        err.message
    );

    let tariff = isp
        .tariff("Office", 10, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let invoice = isp.invoice(&customer, last_month(), None, None).await;
    let invoice_id = invoice["id"].as_str().expect("an id").to_string();

    // A payment's amount.
    assert_eq!(
        isp.err(
            "isp.payment.record",
            json!({"customer_id": customer, "amount": {"currency": "USD", "minor": 25.5},
                   "method": "cash"}),
        )
        .await
        .code,
        "invalid"
    );
    // A rate.
    assert_eq!(
        isp.err(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": invoice_id,
                "amount": {"currency": "IQD", "minor": 33_000},
                "method": "cash",
                "conversion": {"from": "USD", "to": "IQD", "rate": 1_320.5},
            }),
        )
        .await
        .code,
        "invalid"
    );
    // An invoice line on a draft.
    let draft = isp
        .ok(
            "isp.invoice.draft",
            json!({"customer_id": customer, "period_start": last_month().0 - 62 * DAY_MS,
                   "period_end": last_month().0 - 31 * DAY_MS, "allow_empty": true}),
        )
        .await;
    assert_eq!(
        isp.err(
            "isp.invoice.line.add",
            json!({
                "invoice_id": draft["invoice"]["id"],
                "description": "installation",
                "amount": {"currency": "USD", "minor": 49.99},
            }),
        )
        .await
        .code,
        "invalid"
    );
    // An unknown currency is refused rather than assumed to have two digits.
    let err = isp
        .err(
            "isp.customer.save",
            json!({"name": "Who Knows", "currency": "XBT"}),
        )
        .await;
    assert!(err.message.contains("not a currency"), "{}", err.message);
    // And a customer with no currency at all is refused, not defaulted.
    let err = isp
        .err("isp.customer.save", json!({"name": "No Currency"}))
        .await;
    assert!(err.message.contains("which currency"), "{}", err.message);
}

#[tokio::test]
async fn nothing_in_a_report_adds_two_currencies_into_one_figure() {
    let isp = Isp::start(false, &[]).await;
    let dollars = isp.customer("Dollar Cafe", "USD", json!({})).await;
    let dinars = isp.customer("Dinar Cafe", "IQD", json!({})).await;
    let usd = isp
        .tariff("USD plan", 10, "flat", 2_500, "USD", "monthly")
        .await;
    let iqd = isp
        .tariff("IQD plan", 10, "flat", 33_000, "IQD", "monthly")
        .await;
    isp.subscribe(&dollars, &usd, None).await;
    isp.subscribe(&dinars, &iqd, None).await;
    let period = last_month();
    isp.invoice(&dollars, period, None, None).await;
    isp.invoice(&dinars, period, None, None).await;

    // Arrears: one aged report per currency, and nowhere to put a total.
    let arrears = isp.ok("isp.arrears", json!({})).await;
    let by_currency = list(&arrears["by_currency"]);
    assert_eq!(by_currency.len(), 2, "one report per currency");
    let codes: Vec<&str> = by_currency
        .iter()
        .map(|row| row["currency"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(codes, ["IQD", "USD"]);
    assert_eq!(by_currency[0]["total"]["minor"], json!(33_000));
    assert_eq!(by_currency[1]["total"]["minor"], json!(2_500));
    for forbidden in ["total_minor", "grand_total", "total_all", "sum"] {
        assert!(
            arrears.get(forbidden).is_none(),
            "`{forbidden}` would be a figure that is not true"
        );
    }
    assert_money_is_integer(&arrears, "isp.arrears");

    // Revenue: per currency, and a conversion only when the caller gives the
    // rate — there is no rate feed and nothing guesses one.
    let window = json!({"since": period.0 - DAY_MS, "until": period.1 + DAY_MS});
    let revenue = isp.ok("isp.revenue", window.clone()).await;
    assert_eq!(list(&revenue["issued"]).len(), 2);
    assert!(revenue["converted"].is_null(), "nobody asked for one");
    let err = isp
        .err(
            "isp.revenue",
            json!({"since": period.0 - DAY_MS, "until": period.1 + DAY_MS,
                   "convert": {"to": "USD"}}),
        )
        .await;
    assert!(
        err.message.contains("no rate between"),
        "a converted figure needs the rate: {}",
        err.message
    );
    let converted = isp
        .ok(
            "isp.revenue",
            json!({
                "since": period.0 - DAY_MS,
                "until": period.1 + DAY_MS,
                "convert": {
                    "to": "USD",
                    "rates": [{"from": "USD", "to": "IQD", "rate": 1_320}],
                    "at": 1_700_000_000_000_i64,
                },
            }),
        )
        .await;
    let issued = &converted["converted"]["issued"];
    assert_eq!(
        issued["basis"],
        json!("conversion"),
        "labelled, not the ledger"
    );
    assert_eq!(issued["amount"]["currency"], json!("USD"));
    // 33,000 IQD at 1,320 to the dollar is $25, plus the $25 invoice.
    assert_eq!(issued["amount"]["minor"], json!(5_000));
    assert_eq!(issued["at"], json!(1_700_000_000_000_i64));
    assert_eq!(list(&issued["rates"]).len(), 1);
    assert_eq!(issued["rates"][0]["rate"], json!(1_320));
    assert_eq!(
        list(&issued["of"]).len(),
        2,
        "the per-currency ledger travels with the converted figure"
    );
    assert!(
        issued["note"]
            .as_str()
            .unwrap_or_default()
            .contains("not the ledger"),
        "{}",
        issued["note"]
    );

    // The summary is per currency too.
    let summary = isp.ok("isp.summary", json!({})).await;
    assert_eq!(list(&summary["arrears"]).len(), 2);
    assert_eq!(list(&summary["by_currency"]).len(), 2);
    assert_money_is_integer(&summary, "isp.summary");

    // And no money object anywhere in any of it is missing its currency.
    for value in [&arrears, &revenue, &summary] {
        for money in money_objects(value) {
            assert!(
                money["currency"].is_string() && money["minor"].is_i64(),
                "{money}"
            );
        }
    }
}

#[tokio::test]
async fn a_payment_in_another_currency_keeps_its_rate_and_still_reconciles() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp.customer("Baghdad Fibre", "USD", json!({})).await;
    let tariff = isp
        .tariff("Business 25", 25, "flat", 2_500, "USD", "monthly")
        .await;
    isp.subscribe(
        &customer,
        &tariff,
        Some(nocturne_app::util::now_ms() - 120 * DAY_MS),
    )
    .await;
    let now = nocturne_app::util::now_ms();
    let first = isp
        .invoice(
            &customer,
            (now - 62 * DAY_MS, now - 31 * DAY_MS),
            Some(now - 31 * DAY_MS),
            Some(now - 17 * DAY_MS),
        )
        .await;
    let second = isp
        .invoice(&customer, (now - 31 * DAY_MS, now), Some(now), None)
        .await;
    let first_id = first["id"].as_str().expect("an id").to_string();
    let second_id = second["id"].as_str().expect("an id").to_string();

    // 33,000 dinars at 1,320 to the dollar settles $25.00 exactly.
    let paid = isp
        .ok(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": first_id,
                "amount": {"currency": "IQD", "minor": 33_000},
                "method": "cash",
                "reference": "receipt 41",
                "conversion": {"from": "USD", "to": "IQD", "rate": 1_320},
            }),
        )
        .await;
    assert_eq!(paid["payment"]["amount"]["currency"], json!("IQD"));
    assert_eq!(paid["payment"]["settled"]["minor"], json!(2_500));
    assert_eq!(paid["payment"]["settled"]["currency"], json!("USD"));
    assert_eq!(paid["payment"]["conversion"]["rate"], json!(1_320));
    assert_eq!(paid["payment"]["settled_exact"], json!(true));
    assert_eq!(paid["invoice"]["status"], json!("paid"));
    assert_eq!(paid["invoice"]["outstanding"]["minor"], json!(0));

    // Months later the dinar has moved; a payment at the new rate settles
    // less, and the first receipt is untouched by it.
    let later = isp
        .ok(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": second_id,
                "amount": {"currency": "IQD", "minor": 33_000},
                "method": "cash",
                "conversion": {"from": "USD", "to": "IQD", "rate": 1_500},
            }),
        )
        .await;
    assert_eq!(later["payment"]["settled"]["minor"], json!(2_200));
    assert_eq!(later["invoice"]["status"], json!("part_paid"));
    assert_eq!(later["invoice"]["outstanding"]["minor"], json!(300));

    // The March receipt still reconciles in November: the rate it was taken
    // at is stored, not recomputed.
    let first_again = isp.ok("isp.invoice.get", json!({"id": first_id})).await;
    assert_eq!(first_again["status"], json!("paid"));
    assert_eq!(first_again["paid"]["minor"], json!(2_500));
    let payments = list(
        &isp.ok("isp.payments", json!({"invoice_id": first_id}))
            .await,
    );
    assert_eq!(payments.len(), 1);
    assert_eq!(payments[0]["conversion"]["rate"], json!(1_320));
    assert_eq!(payments[0]["settled"]["minor"], json!(2_500));
    assert_eq!(payments[0]["amount"]["minor"], json!(33_000));

    // A payment across currencies with no rate is refused, not guessed.
    let err = isp
        .err(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": second_id,
                "amount": {"currency": "IQD", "minor": 1_000},
                "method": "cash",
            }),
        )
        .await;
    assert!(
        err.message.contains("Enter the rate you actually got"),
        "{}",
        err.message
    );
    // So is a rate for the wrong pair.
    assert_eq!(
        isp.err(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": second_id,
                "amount": {"currency": "IQD", "minor": 1_000},
                "method": "cash",
                "conversion": {"from": "EUR", "to": "GBP", "rate": 85},
            }),
        )
        .await
        .code,
        "invalid"
    );
    // And a rate where none is needed: both in USD.
    assert_eq!(
        isp.err(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": second_id,
                "amount": {"currency": "USD", "minor": 300},
                "method": "cash",
                "conversion": {"from": "USD", "to": "IQD", "rate": 1_320},
            }),
        )
        .await
        .code,
        "invalid"
    );
    // A rounded conversion is settled and *says* it was rounded.
    let third = isp
        .ok(
            "isp.payment.record",
            json!({
                "customer_id": customer,
                "invoice_id": second_id,
                "amount": {"currency": "IQD", "minor": 4_000},
                "method": "cash",
                "conversion": {"from": "USD", "to": "IQD", "rate": 1_320},
            }),
        )
        .await;
    assert_eq!(third["payment"]["settled"]["minor"], json!(303));
    assert_eq!(third["payment"]["settled_exact"], json!(false));
    assert!(
        !list(&third["warnings"]).is_empty(),
        "a rounding is reported, not hidden"
    );
}

#[tokio::test]
async fn every_money_column_in_the_schema_is_an_integer() {
    let isp = Isp::start(false, &[]).await;
    let columns: Vec<(String, String, String)> = isp
        .app
        .store()
        .with(|conn| {
            let mut statement = conn.prepare(
                "SELECT m.name, p.name, p.type FROM sqlite_master m
                 JOIN pragma_table_info(m.name) p
                WHERE m.type = 'table' AND m.name LIKE 'isp_%'
                ORDER BY m.name, p.cid",
            )?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect()
        })
        .expect("the schema reads back");
    assert!(
        columns.len() > 20,
        "the feature's tables exist: {columns:?}"
    );
    for (table, column, kind) in &columns {
        assert_ne!(
            kind.to_uppercase(),
            "REAL",
            "{table}.{column} is a REAL column, and a ledger has no business holding a float"
        );
    }
    for money in [
        ("isp_invoice", "total_minor"),
        ("isp_payment", "minor"),
        ("isp_payment", "settled_minor"),
        ("isp_subscription", "price_minor"),
    ] {
        let found = columns
            .iter()
            .find(|(table, column, _)| table == money.0 && column == money.1)
            .unwrap_or_else(|| panic!("{}.{} exists", money.0, money.1));
        assert_eq!(
            found.2.to_uppercase(),
            "INTEGER",
            "{}.{} holds minor units",
            money.0,
            money.1
        );
        // And the currency travels with it.
        assert!(
            columns
                .iter()
                .any(|(table, column, _)| table == money.0 && column.contains("currency")),
            "{} has a currency column beside its amount",
            money.0
        );
    }
}

#[tokio::test]
async fn money_on_the_wire_is_always_whole_minor_units_with_a_currency() {
    let isp = Isp::start(false, &[]).await;
    let customer = isp
        .customer("Mixed Paths", "JOD", json!({"code": "A-1"}))
        .await;
    // A three-digit currency, where a hard-coded 100 would be wrong twice.
    let tariff = isp
        .tariff("Per megabit", 7, "per_mbit", 1_250, "JOD", "yearly")
        .await;
    isp.subscribe(&customer, &tariff, None).await;
    let tariffs = list(&isp.ok("isp.tariffs", json!({})).await);
    assert_eq!(tariffs[0]["period_price"]["minor"], json!(8_750));
    assert_eq!(tariffs[0]["period_price"]["text"], json!("8.750"));
    let subscriptions = list(&isp.ok("isp.subscriptions", json!({})).await);
    assert_eq!(subscriptions[0]["price"]["minor"], json!(8_750));
    // A yearly figure as a monthly one is a division, and it is rounded and
    // says so rather than pretending to be exact.
    assert_eq!(subscriptions[0]["monthly_price"]["minor"], json!(729));
    assert_eq!(subscriptions[0]["monthly_exact"], json!(false));

    for (method, params) in [
        ("isp.customers", json!({})),
        ("isp.customer.get", json!({"id": customer})),
        ("isp.tariffs", json!({})),
        ("isp.subscriptions", json!({})),
        ("isp.summary", json!({})),
        ("isp.arrears", json!({})),
    ] {
        let value: Value = isp.ok(method, params).await;
        assert_money_is_integer(&value, method);
    }
}
