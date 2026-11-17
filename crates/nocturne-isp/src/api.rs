//! The RPC surface and the events.
//!
//! Specification: `docs/dev/features/isp.md`. Every method is reachable only
//! through `App::rpc`, and every mutation calls `audit(...)`, because this is
//! money.
//!
//! There is no method here that sends anything to a device, and no code path
//! in this crate that opens a session, creates a change or writes a
//! configuration. Suspending a customer for non-payment sets a field on a
//! record and tells whoever is reading that nothing was sent — cutting the
//! service off is a decision a person makes on the router.

use nocturne_app::{App, AppBuilder, AppError, AppResult, NoParams, settings, util};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::{Customer, Invoice, InvoiceView, Settings, Subscription};
use crate::money::{CURRENCIES, Money, Totals};
use crate::{billing, db, network, records, reports};

pub const SETTINGS_SECTION: &str = "isp";

fn config(app: &App) -> AppResult<Settings> {
    settings::get::<Settings>(app, SETTINGS_SECTION)
}

/// Records changed: a screen showing them refetches.
fn changed(app: &App, kinds: &[&str]) {
    app.publish("isp.changed", &json!({"kinds": kinds}));
}

fn invoices_changed(app: &App, change: &str, invoices: &[&Invoice]) {
    app.publish(
        "isp.invoices.changed",
        &json!({
            "change": change,
            "ids": invoices.iter().map(|invoice| invoice.id.as_str()).collect::<Vec<_>>(),
            "customer_ids": invoices
                .iter()
                .map(|invoice| invoice.customer_id.as_str())
                .collect::<Vec<_>>(),
        }),
    );
}

fn payment_changed(app: &App, change: &str, payment: &crate::model::Payment) {
    app.publish(
        "isp.payments.changed",
        &json!({
            "change": change,
            "ids": [payment.id],
            "customer_ids": [payment.customer_id],
            "invoice_ids": payment.invoice_id.iter().collect::<Vec<_>>(),
        }),
    );
}

fn credit_changed(app: &App, change: &str, credit: &crate::model::CreditNote) {
    app.publish(
        "isp.credits.changed",
        &json!({
            "change": change,
            "ids": [credit.id],
            "customer_ids": [credit.customer_id],
            "invoice_ids": credit.invoice_id.iter().collect::<Vec<_>>(),
        }),
    );
}

// ------------------------------------------------------------------ params

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct StateParams {
    id: String,
    state: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
struct ReasonParams {
    id: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
struct CustomerGetParams {
    id: String,
    /// How many of their invoices to send back, newest first.
    #[serde(default = "twenty")]
    invoices: u32,
}

fn twenty() -> u32 {
    20
}

#[derive(Debug, Deserialize)]
struct AmendParams {
    id: String,
    #[serde(default)]
    mbit: Option<i64>,
    #[serde(default)]
    price: Option<Money>,
    /// When the new terms start; now by default.
    #[serde(default)]
    from_at: Option<i64>,
    #[serde(default)]
    note: String,
}

#[derive(Debug, Deserialize)]
struct EndParams {
    id: String,
    #[serde(default)]
    ended_at: Option<i64>,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
struct DraftParams {
    customer_id: String,
    period_start: i64,
    period_end: i64,
    /// Make a draft with no lines, to add them by hand.
    #[serde(default)]
    allow_empty: bool,
}

#[derive(Debug, Deserialize)]
struct RunParams {
    period_start: i64,
    period_end: i64,
    /// Only these customers; everybody active by default.
    #[serde(default)]
    customer_ids: Vec<String>,
    /// Nothing is written unless this is explicitly `false`.
    #[serde(default = "yes")]
    dry_run: bool,
    /// Issue each draft as it is made, instead of leaving it to be checked.
    #[serde(default)]
    issue: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct IssueParams {
    id: String,
    #[serde(default)]
    due_at: Option<i64>,
    /// For entering an invoice that already exists on paper. It may not be
    /// in the future.
    #[serde(default)]
    issued_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct LineRemoveParams {
    invoice_id: String,
    line_id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ArrearsParams {
    customer_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RevenueParams {
    since: i64,
    #[serde(default)]
    until: Option<i64>,
    /// Ask for one figure instead of several, at rates you supply.
    #[serde(default)]
    convert: Option<reports::ConvertRequest>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct UsageParams {
    /// One customer, or the page described by the rest of these.
    customer_id: Option<String>,
    query: Option<String>,
    state: Option<String>,
    currency: Option<String>,
    tower: Option<String>,
    device_id: Option<String>,
    limit: Option<u32>,
    cursor: Option<String>,
    /// Average over a window instead of taking the latest reading.
    since: Option<i64>,
    until: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CapacityParams {
    link_id: Option<String>,
    tower: Option<String>,
    since: Option<i64>,
    until: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MarginParams {
    link_id: Option<String>,
    tower: Option<String>,
    customer_id: Option<String>,
}

// ----------------------------------------------------------------- results

#[derive(Debug, Serialize)]
struct CurrencyRow {
    code: &'static str,
    /// Digits after the point in everyday use.
    digits: u32,
    /// Minor units in one major unit.
    scale: i64,
    name: &'static str,
    /// What 1,000 minor units of it is, written out — so a screen cannot
    /// assume hundredths.
    thousand_minor_units: String,
}

#[derive(Debug, Serialize)]
struct CustomerPage {
    customers: Vec<Customer>,
    /// How many match the filter in all.
    matching: u64,
    /// Pass this back as `cursor` for the next page; `None` on the last one.
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubscriptionRow {
    #[serde(flatten)]
    subscription: Subscription,
    /// The agreed figure as a monthly one, for comparing plans. Derived, and
    /// rounded when the period does not divide evenly.
    monthly_price: Money,
    monthly_exact: bool,
}

#[derive(Debug, Serialize)]
struct RunResult {
    period_start: i64,
    period_end: i64,
    dry_run: bool,
    drafted: u32,
    issued: u32,
    /// What each customer's invoice would say, or does.
    invoices: Vec<RunRow>,
    /// Customers deliberately not billed, each with a reason.
    skipped: Vec<RunSkip>,
    /// What this run comes to, per currency. Never one number.
    totals: Totals,
    note: String,
}

#[derive(Debug, Serialize)]
struct RunRow {
    customer_id: String,
    /// `None` in a dry run and until it is issued.
    invoice_id: Option<String>,
    number: Option<String>,
    total: Money,
    lines: u32,
    skipped_subscriptions: Vec<billing::Skipped>,
}

#[derive(Debug, Serialize)]
struct RunSkip {
    customer_id: String,
    reason: String,
}

fn subscription_rows(subscriptions: Vec<Subscription>) -> AppResult<Vec<SubscriptionRow>> {
    subscriptions
        .into_iter()
        .map(|subscription| {
            let monthly = subscription.monthly_price()?;
            Ok(SubscriptionRow {
                subscription,
                monthly_price: monthly.amount,
                monthly_exact: monthly.exact,
            })
        })
        .collect()
}

fn invoice_views(app: &App, invoices: Vec<Invoice>) -> AppResult<Vec<InvoiceView>> {
    let now = util::now_ms();
    invoices
        .into_iter()
        .map(|invoice| billing::invoice_view(app, invoice, now))
        .collect()
}

// -------------------------------------------------------------------- init

pub fn init(builder: &mut AppBuilder) {
    settings::declare(builder, SETTINGS_SECTION, &Settings::default());
    // Without its tables there is nowhere to keep a ledger: failing here
    // means a broken database, not a condition to carry on with.
    db::init(builder.store()).expect("the isp feature's tables can be created");
    let router = &mut builder.router;

    // ------------------------------------------------------------ reference

    router.add("isp.currencies", |_: App, _: NoParams| async move {
        Ok::<Vec<CurrencyRow>, AppError>(
            CURRENCIES
                .iter()
                .map(|known| {
                    let currency = crate::money::Currency::of(known.code)
                        .expect("every code in the table is a currency");
                    CurrencyRow {
                        code: known.code,
                        digits: known.digits,
                        scale: currency.scale(),
                        name: known.name,
                        thousand_minor_units: Money::new(1_000, currency).text(),
                    }
                })
                .collect(),
        )
    });

    router.add("isp.summary", |app: App, _: NoParams| async move {
        reports::summary(&app, &config(&app)?)
    });

    // ------------------------------------------------------------ customers

    router.add(
        "isp.customers",
        |app: App, params: db::CustomerQuery| async move {
            let offset = db::offset_of(params.cursor.as_deref())?;
            let page = db::customers(app.store(), &params)?;
            let seen = offset + page.customers.len() as i64;
            Ok(CustomerPage {
                next_cursor: (seen < page.matching as i64 && !page.customers.is_empty())
                    .then(|| seen.to_string()),
                matching: page.matching,
                customers: page.customers,
            })
        },
    );

    router.add(
        "isp.customer.get",
        |app: App, params: CustomerGetParams| async move {
            let settings = config(&app)?;
            let customer = db::require_customer(app.store(), &params.id)?;
            let subscriptions = db::subscriptions(
                app.store(),
                &db::SubscriptionQuery {
                    customer_id: Some(customer.id.clone()),
                    ..db::SubscriptionQuery::default()
                },
            )?;
            let invoices = db::invoices(
                app.store(),
                &db::InvoiceQuery {
                    customer_id: Some(customer.id.clone()),
                    limit: Some(params.invoices.clamp(1, 500)),
                    ..db::InvoiceQuery::default()
                },
            )?;
            let usage = network::usage(&app, &settings, std::slice::from_ref(&customer), None)
                .await?
                .rows
                .into_iter()
                .next()
                .ok_or_else(|| AppError::internal("the usage report lost its only row"))?;
            Ok(json!({
                "customer": customer,
                "currency": customer.currency,
                "subscriptions": subscription_rows(subscriptions)?,
                "invoices": invoice_views(&app, invoices)?,
                "balance": billing::balance(&app, &customer.id)?,
                "usage": usage,
            }))
        },
    );

    router.add(
        "isp.customer.save",
        |app: App, params: records::CustomerInput| async move {
            let saved = records::save_customer(&app, params)?;
            app.audit(
                "isp.customer.save",
                saved.customer.service.device_id.as_deref(),
                format!(
                    "{} customer {} ({})",
                    if saved.created { "Added" } else { "Updated" },
                    saved.customer.name,
                    saved.customer.currency
                ),
                json!({
                    "customer": saved.customer.id,
                    "currency": saved.customer.currency,
                    "created": saved.created,
                    "warnings": saved.warnings,
                }),
            );
            changed(&app, &["customers"]);
            Ok(saved)
        },
    );

    router.add(
        "isp.customer.state",
        |app: App, params: StateParams| async move {
            let changedstate = records::set_customer_state(&app, &params.id, &params.state)?;
            app.audit(
                "isp.customer.state",
                changedstate.customer.service.device_id.as_deref(),
                format!(
                    "Set {} from {} to {}{}",
                    changedstate.customer.name,
                    changedstate.was.as_str(),
                    changedstate.customer.state.as_str(),
                    if params.reason.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", params.reason.trim())
                    }
                ),
                json!({
                    "customer": changedstate.customer.id,
                    "was": changedstate.was,
                    "now": changedstate.customer.state,
                    "device_changed": false,
                }),
            );
            changed(&app, &["customers"]);
            Ok(changedstate)
        },
    );

    router.add(
        "isp.customer.delete",
        |app: App, params: IdParams| async move {
            let deleted = records::delete_customer(&app, &params.id)?;
            if deleted.deleted {
                app.audit(
                    "isp.customer.delete",
                    None,
                    format!(
                        "Deleted customer {} (they had no invoices or payments)",
                        deleted.name
                    ),
                    json!({"customer": params.id}),
                );
                changed(&app, &["customers", "subscriptions"]);
            }
            Ok(deleted)
        },
    );

    // -------------------------------------------------------------- tariffs

    router.add("isp.tariffs", |app: App, _: NoParams| async move {
        records::tariff_rows(&app)
    });

    router.add(
        "isp.tariff.save",
        |app: App, params: records::TariffInput| async move {
            let saved = records::save_tariff(&app, params)?;
            app.audit(
                "isp.tariff.save",
                None,
                format!(
                    "{} tariff {} ({} Mbit, {} per {})",
                    if saved.created { "Added" } else { "Updated" },
                    saved.tariff.name,
                    saved.tariff.mbit,
                    saved.period_price.text(),
                    saved.tariff.period.as_str()
                ),
                json!({
                    "tariff": saved.tariff.id,
                    "period_price": saved.period_price,
                    "subscriptions_unaffected": saved.subscriptions_unaffected,
                }),
            );
            changed(&app, &["tariffs"]);
            Ok(saved)
        },
    );

    router.add(
        "isp.tariff.delete",
        |app: App, params: IdParams| async move {
            let (deleted, name) = records::delete_tariff(&app, &params.id)?;
            if deleted {
                app.audit(
                    "isp.tariff.delete",
                    None,
                    format!("Deleted tariff {name}"),
                    json!({"tariff": params.id}),
                );
                changed(&app, &["tariffs"]);
            }
            Ok(json!({"deleted": deleted, "name": name}))
        },
    );

    // --------------------------------------------------------- subscriptions

    router.add(
        "isp.subscriptions",
        |app: App, params: db::SubscriptionQuery| async move {
            subscription_rows(db::subscriptions(app.store(), &params)?)
        },
    );

    router.add(
        "isp.subscription.create",
        |app: App, params: records::SubscriptionInput| async move {
            let saved = records::create_subscription(&app, params)?;
            app.audit(
                "isp.subscription.create",
                None,
                format!(
                    "Subscribed customer {} to {} at {} per {} ({} Mbit)",
                    saved.subscription.customer_id,
                    saved.subscription.tariff_name,
                    saved.subscription.price.text(),
                    saved.subscription.period.as_str(),
                    saved.subscription.mbit
                ),
                json!({
                    "subscription": saved.subscription.id,
                    "customer": saved.subscription.customer_id,
                    "tariff": saved.subscription.tariff_id,
                    "agreed_price": saved.subscription.price,
                    "agreed_mbit": saved.subscription.mbit,
                }),
            );
            changed(&app, &["subscriptions"]);
            Ok(saved)
        },
    );

    router.add(
        "isp.subscription.amend",
        |app: App, params: AmendParams| async move {
            let saved = records::amend_subscription(
                &app,
                &params.id,
                params.from_at,
                params.mbit,
                params.price,
                &params.note,
            )?;
            app.audit(
                "isp.subscription.amend",
                None,
                format!(
                    "Amended customer {}'s terms to {} per {} ({} Mbit); the old subscription \
                     ended rather than being rewritten",
                    saved.subscription.customer_id,
                    saved.subscription.price.text(),
                    saved.subscription.period.as_str(),
                    saved.subscription.mbit
                ),
                json!({
                    "subscription": saved.subscription.id,
                    "replaced": saved.subscription.amends,
                    "customer": saved.subscription.customer_id,
                    "agreed_price": saved.subscription.price,
                }),
            );
            changed(&app, &["subscriptions"]);
            Ok(saved)
        },
    );

    router.add(
        "isp.subscription.end",
        |app: App, params: EndParams| async move {
            let ended =
                records::end_subscription(&app, &params.id, params.ended_at, &params.reason)?;
            app.audit(
                "isp.subscription.end",
                None,
                format!(
                    "Ended customer {}'s subscription to {}",
                    ended.customer_id, ended.tariff_name
                ),
                json!({
                    "subscription": ended.id,
                    "customer": ended.customer_id,
                    "ended_at": ended.ended_at,
                }),
            );
            changed(&app, &["subscriptions"]);
            Ok(ended)
        },
    );

    // ---------------------------------------------------------------- links

    router.add("isp.links", |app: App, _: NoParams| async move {
        records::links(&app)
    });

    router.add(
        "isp.link.save",
        |app: App, params: records::LinkInput| async move {
            let (link, created) = records::save_link(&app, params)?;
            app.audit(
                "isp.link.save",
                link.device_id.as_deref(),
                format!(
                    "{} link {} ({} Mbit{})",
                    if created { "Added" } else { "Updated" },
                    link.name,
                    link.capacity_mbit,
                    match &link.cost_per_mbit {
                        Some(cost) => format!(
                            ", {} per Mbit per {}",
                            cost.text(),
                            link.cost_period.as_str()
                        ),
                        None => String::new(),
                    }
                ),
                json!({
                    "link": link.id,
                    "capacity_mbit": link.capacity_mbit,
                    "cost_per_mbit": link.cost_per_mbit,
                }),
            );
            changed(&app, &["links"]);
            Ok(json!({"link": link, "created": created}))
        },
    );

    router.add("isp.link.delete", |app: App, params: IdParams| async move {
        let (deleted, name) = records::delete_link(&app, &params.id)?;
        if deleted {
            app.audit(
                "isp.link.delete",
                None,
                format!("Deleted link {name}"),
                json!({"link": params.id}),
            );
            changed(&app, &["links"]);
        }
        Ok(json!({"deleted": deleted, "name": name}))
    });

    // ------------------------------------------------------------- invoices

    router.add(
        "isp.invoices",
        |app: App, params: db::InvoiceQuery| async move {
            invoice_views(&app, db::invoices(app.store(), &params)?)
        },
    );

    router.add("isp.invoice.get", |app: App, params: IdParams| async move {
        let invoice = db::require_invoice(app.store(), &params.id)?;
        billing::invoice_view(&app, invoice, util::now_ms())
    });

    router.add(
        "isp.invoice.draft",
        |app: App, params: DraftParams| async move {
            let customer = db::require_customer(app.store(), &params.customer_id)?;
            let drafted = billing::draft(
                &app,
                &customer,
                params.period_start,
                params.period_end,
                params.allow_empty,
            )?;
            app.audit(
                "isp.invoice.draft",
                None,
                format!(
                    "Drafted an invoice for {} of {} ({} line(s))",
                    customer.name,
                    drafted.invoice.total.text(),
                    drafted.invoice.lines.len()
                ),
                json!({
                    "invoice": drafted.invoice.id,
                    "customer": customer.id,
                    "total": drafted.invoice.total,
                    "skipped": drafted.skipped,
                }),
            );
            invoices_changed(&app, "drafted", &[&drafted.invoice]);
            Ok(drafted)
        },
    );

    router.add("isp.invoice.run", |app: App, params: RunParams| async move {
        let settings = config(&app)?;
        let store = app.store();
        let customers: Vec<Customer> = if params.customer_ids.is_empty() {
            db::customers(
                store,
                &db::CustomerQuery {
                    state: Some("active".to_string()),
                    limit: Some(2_000),
                    ..db::CustomerQuery::default()
                },
            )?
            .customers
        } else {
            params
                .customer_ids
                .iter()
                .map(|id| db::require_customer(store, id))
                .collect::<AppResult<Vec<_>>>()?
        };
        let mut rows: Vec<RunRow> = Vec::new();
        let mut skipped: Vec<RunSkip> = Vec::new();
        let mut totals = Totals::new();
        let mut issued_count = 0_u32;
        let mut issued_invoices: Vec<Invoice> = Vec::new();
        for customer in &customers {
            let built = match billing::build(
                &app,
                customer,
                params.period_start,
                params.period_end,
                false,
            ) {
                Ok(built) => built,
                Err(err) => {
                    skipped.push(RunSkip {
                        customer_id: customer.id.clone(),
                        reason: err.message,
                    });
                    continue;
                }
            };
            totals.add(built.invoice.total)?;
            if params.dry_run {
                rows.push(RunRow {
                    customer_id: customer.id.clone(),
                    invoice_id: None,
                    number: None,
                    total: built.invoice.total,
                    lines: u32::try_from(built.invoice.lines.len()).unwrap_or(u32::MAX),
                    skipped_subscriptions: built.skipped,
                });
                continue;
            }
            db::put_invoice(store, &built.invoice)?;
            let mut invoice = built.invoice;
            if params.issue {
                invoice = billing::issue(&app, &settings, &invoice.id, None, None)?;
                issued_count += 1;
            }
            rows.push(RunRow {
                customer_id: customer.id.clone(),
                invoice_id: Some(invoice.id.clone()),
                number: invoice.number.clone(),
                total: invoice.total,
                lines: u32::try_from(invoice.lines.len()).unwrap_or(u32::MAX),
                skipped_subscriptions: built.skipped,
            });
            issued_invoices.push(invoice);
        }
        if !params.dry_run {
            app.audit(
                "isp.invoice.run",
                None,
                format!(
                    "Billed {} customer(s) for the period, {} issued, {} skipped",
                    rows.len(),
                    issued_count,
                    skipped.len()
                ),
                json!({
                    "period_start": params.period_start,
                    "period_end": params.period_end,
                    "drafted": rows.len(),
                    "issued": issued_count,
                    "totals": totals,
                    "skipped": skipped,
                }),
            );
            invoices_changed(
                &app,
                if params.issue { "issued" } else { "drafted" },
                &issued_invoices.iter().collect::<Vec<_>>(),
            );
        }
        Ok(RunResult {
            period_start: params.period_start,
            period_end: params.period_end,
            dry_run: params.dry_run,
            drafted: u32::try_from(rows.len()).unwrap_or(u32::MAX),
            issued: issued_count,
            invoices: rows,
            skipped,
            totals,
            note: if params.dry_run {
                "nothing was written: this is what would be billed, including every customer \
                 left out and why"
                    .to_string()
            } else {
                "one figure per currency; customers billed in different currencies are not added \
                 together"
                    .to_string()
            },
        })
    });

    router.add(
        "isp.invoice.line.add",
        |app: App, params: billing::LineInput| async move {
            let description = params.description.clone();
            let amount = params.amount;
            let invoice = billing::add_line(&app, params)?;
            app.audit(
                "isp.invoice.line.add",
                None,
                format!(
                    "Added `{}` ({}) to a draft invoice for {}",
                    description,
                    amount.text(),
                    invoice.customer_name
                ),
                json!({"invoice": invoice.id, "amount": amount, "total": invoice.total}),
            );
            invoices_changed(&app, "updated", &[&invoice]);
            Ok(invoice)
        },
    );

    router.add(
        "isp.invoice.line.remove",
        |app: App, params: LineRemoveParams| async move {
            let invoice = billing::remove_line(&app, &params.invoice_id, &params.line_id)?;
            app.audit(
                "isp.invoice.line.remove",
                None,
                format!(
                    "Removed a line from a draft invoice for {}",
                    invoice.customer_name
                ),
                json!({"invoice": invoice.id, "line": params.line_id, "total": invoice.total}),
            );
            invoices_changed(&app, "updated", &[&invoice]);
            Ok(invoice)
        },
    );

    router.add(
        "isp.invoice.issue",
        |app: App, params: IssueParams| async move {
            let settings = config(&app)?;
            let invoice =
                billing::issue(&app, &settings, &params.id, params.due_at, params.issued_at)?;
            app.audit(
                "isp.invoice.issue",
                None,
                format!(
                    "Issued invoice {} to {} for {}, due {}",
                    invoice.number.clone().unwrap_or_default(),
                    invoice.customer_name,
                    invoice.total.text(),
                    invoice.due_at.unwrap_or_default()
                ),
                json!({
                    "invoice": invoice.id,
                    "number": invoice.number,
                    "customer": invoice.customer_id,
                    "total": invoice.total,
                    "due_at": invoice.due_at,
                }),
            );
            app.publish(
                "isp.invoices.changed",
                &json!({
                    "change": "issued",
                    "ids": [invoice.id],
                    "customer_ids": [invoice.customer_id],
                    "currency": invoice.currency,
                    "total_minor": invoice.total.minor,
                    "due_at": invoice.due_at,
                }),
            );
            Ok(invoice)
        },
    );

    router.add(
        "isp.invoice.void",
        |app: App, params: ReasonParams| async move {
            let invoice = billing::void(&app, &params.id, &params.reason)?;
            app.audit(
                "isp.invoice.void",
                None,
                format!(
                    "Voided invoice {} for {} ({}): {}",
                    invoice
                        .number
                        .clone()
                        .unwrap_or_else(|| "draft".to_string()),
                    invoice.customer_name,
                    invoice.total.text(),
                    invoice.void_reason
                ),
                json!({
                    "invoice": invoice.id,
                    "number": invoice.number,
                    "customer": invoice.customer_id,
                    "reason": invoice.void_reason,
                }),
            );
            invoices_changed(&app, "voided", &[&invoice]);
            Ok(invoice)
        },
    );

    router.add(
        "isp.invoice.reissue",
        |app: App, params: ReasonParams| async move {
            let reissued = billing::reissue(&app, &params.id, &params.reason)?;
            app.audit(
                "isp.invoice.reissue",
                None,
                format!(
                    "Voided invoice {} for {} and drafted its replacement: {}",
                    reissued
                        .voided
                        .number
                        .clone()
                        .unwrap_or_else(|| "a draft".to_string()),
                    reissued.voided.customer_name,
                    reissued.voided.void_reason
                ),
                json!({
                    "voided": reissued.voided.id,
                    "draft": reissued.draft.id,
                    "customer": reissued.voided.customer_id,
                    "reason": reissued.voided.void_reason,
                }),
            );
            invoices_changed(&app, "voided", &[&reissued.voided]);
            invoices_changed(&app, "drafted", &[&reissued.draft]);
            Ok(reissued)
        },
    );

    // ------------------------------------------------------------- payments

    router.add(
        "isp.payments",
        |app: App, params: db::PaymentQuery| async move { db::payments(app.store(), &params) },
    );

    router.add(
        "isp.payment.record",
        |app: App, params: billing::PaymentInput| async move {
            let recorded = billing::record_payment(&app, params)?;
            let payment = &recorded.payment;
            app.audit(
                "isp.payment.record",
                None,
                format!(
                    "Recorded {} from customer {} by {}{}{}",
                    payment.amount.text(),
                    payment.customer_id,
                    payment.method,
                    match (&payment.conversion, &payment.settled) {
                        (Some(conversion), Some(settled)) => format!(
                            " ({})",
                            billing::conversion_note(conversion, payment.amount, *settled)
                        ),
                        _ => String::new(),
                    },
                    match &payment.invoice_id {
                        Some(id) => format!(" against invoice {id}"),
                        None => " on account".to_string(),
                    }
                ),
                json!({
                    "payment": payment.id,
                    "customer": payment.customer_id,
                    "invoice": payment.invoice_id,
                    "amount": payment.amount,
                    "conversion": payment.conversion,
                    "settled": payment.settled,
                    "settled_exact": payment.settled_exact,
                }),
            );
            payment_changed(&app, "recorded", payment);
            Ok(recorded)
        },
    );

    router.add(
        "isp.payment.void",
        |app: App, params: ReasonParams| async move {
            let payment = billing::void_payment(&app, &params.id, &params.reason)?;
            app.audit(
                "isp.payment.void",
                None,
                format!(
                    "Voided a payment of {} from customer {}: {}",
                    payment.amount.text(),
                    payment.customer_id,
                    payment.void_reason
                ),
                json!({
                    "payment": payment.id,
                    "customer": payment.customer_id,
                    "invoice": payment.invoice_id,
                    "amount": payment.amount,
                    "reason": payment.void_reason,
                }),
            );
            payment_changed(&app, "voided", &payment);
            Ok(payment)
        },
    );

    // --------------------------------------------------------- credit notes

    router.add(
        "isp.credits",
        |app: App, params: db::CreditQuery| async move { db::credits(app.store(), &params) },
    );

    router.add(
        "isp.credit.issue",
        |app: App, params: billing::CreditInput| async move {
            let settings = config(&app)?;
            let issued = billing::issue_credit(&app, &settings, params)?;
            let credit = &issued.credit;
            app.audit(
                "isp.credit.issue",
                None,
                format!(
                    "Credited {} to customer {} ({}){}",
                    credit.amount.text(),
                    credit.customer_id,
                    credit.reason,
                    match &credit.invoice_id {
                        Some(id) => format!(" against invoice {id}"),
                        None => " on account".to_string(),
                    }
                ),
                json!({
                    "credit": credit.id,
                    "number": credit.number,
                    "customer": credit.customer_id,
                    "invoice": credit.invoice_id,
                    "amount": credit.amount,
                    "reason": credit.reason,
                }),
            );
            credit_changed(&app, "issued", credit);
            Ok(issued)
        },
    );

    router.add(
        "isp.credit.void",
        |app: App, params: ReasonParams| async move {
            let credit = billing::void_credit(&app, &params.id, &params.reason)?;
            app.audit(
                "isp.credit.void",
                None,
                format!(
                    "Voided a credit note of {} to customer {}: {}",
                    credit.amount.text(),
                    credit.customer_id,
                    credit.void_reason
                ),
                json!({
                    "credit": credit.id,
                    "number": credit.number,
                    "customer": credit.customer_id,
                    "invoice": credit.invoice_id,
                    "amount": credit.amount,
                    "reason": credit.void_reason,
                }),
            );
            credit_changed(&app, "voided", &credit);
            Ok(credit)
        },
    );

    // -------------------------------------------------------------- reports

    router.add(
        "isp.arrears",
        |app: App, params: ArrearsParams| async move {
            reports::arrears(&app, &config(&app)?, params.customer_id.as_deref())
        },
    );

    router.add(
        "isp.revenue",
        |app: App, params: RevenueParams| async move {
            reports::revenue(
                &app,
                params.since,
                params.until.unwrap_or_else(util::now_ms),
                params.convert,
            )
        },
    );

    router.add("isp.usage", |app: App, params: UsageParams| async move {
        let settings = config(&app)?;
        let customers = match &params.customer_id {
            Some(id) => vec![db::require_customer(app.store(), id)?],
            None => {
                db::customers(
                    app.store(),
                    &db::CustomerQuery {
                        query: params.query.clone(),
                        state: params.state.clone(),
                        currency: params.currency.clone(),
                        tower: params.tower.clone(),
                        device_id: params.device_id.clone(),
                        limit: Some(params.limit.unwrap_or(200)),
                        cursor: params.cursor.clone(),
                    },
                )?
                .customers
            }
        };
        let window = params
            .since
            .map(|since| (since, params.until.unwrap_or_else(util::now_ms)));
        network::usage(&app, &settings, &customers, window).await
    });

    router.add(
        "isp.capacity",
        |app: App, params: CapacityParams| async move {
            let settings = config(&app)?;
            let window = params
                .since
                .map(|since| (since, params.until.unwrap_or_else(util::now_ms)));
            network::capacity(
                &app,
                &settings,
                params.link_id.as_deref(),
                params.tower.as_deref(),
                window,
            )
            .await
        },
    );

    router.add("isp.margin", |app: App, params: MarginParams| async move {
        network::margin(
            &app,
            params.link_id.as_deref(),
            params.tower.as_deref(),
            params.customer_id.as_deref(),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_billing_run_previews_unless_it_is_told_otherwise() {
        let params: RunParams =
            serde_json::from_value(json!({"period_start": 0, "period_end": 1})).unwrap();
        assert!(params.dry_run, "a run that writes has to be asked for");
        assert!(!params.issue);
        let asked: RunParams = serde_json::from_value(
            json!({"period_start": 0, "period_end": 1, "dry_run": false, "issue": true}),
        )
        .unwrap();
        assert!(!asked.dry_run && asked.issue);
    }

    #[test]
    fn a_subscription_state_is_a_record_and_not_a_device_action() {
        // Nothing in this crate can be asked to change a device, so there is
        // no method name here that could be mistaken for one.
        let mut builder = nocturne_app::AppBuilder::new(nocturne_app::AppConfig::in_memory())
            .expect("an in-memory app");
        nocturne_app::init(&mut builder);
        init(&mut builder);
        let app = builder.build();
        let methods = app.methods();
        for method in methods.iter().filter(|name| name.starts_with("isp.")) {
            for forbidden in [
                "apply",
                "push",
                "configure",
                "suspend_service",
                "disconnect",
            ] {
                assert!(
                    !method.contains(forbidden),
                    "{method} sounds like it touches a device"
                );
            }
        }
    }

    #[test]
    fn subscription_state_serialises_in_snake_case() {
        assert_eq!(
            serde_json::to_value(crate::model::SubscriptionState::Ended).unwrap(),
            json!("ended")
        );
        assert_eq!(
            serde_json::to_value(crate::model::CustomerState::Cancelled).unwrap(),
            json!("cancelled")
        );
    }

    #[test]
    fn unknown_parameters_are_refused_rather_than_ignored() {
        // A caller who means `price` and types `prices` on a money path has
        // to be told, not quietly given the tariff's figure.
        let input: Result<records::TariffInput, _> =
            serde_json::from_value(json!({"name": "x", "prices": {"currency": "USD", "minor": 1}}));
        assert!(input.is_err(), "deny_unknown_fields on every input struct");
    }
}
