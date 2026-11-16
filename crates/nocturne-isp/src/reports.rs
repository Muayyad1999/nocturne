//! Arrears, revenue and the summary.
//!
//! Every figure in here is per currency. There is no field anywhere in this
//! module that holds a total across currencies, because such a figure would
//! be invented — and an invented figure about money is the one somebody will
//! act on. A caller who wants one number asks for it explicitly, supplies the
//! rates, and gets an answer labelled a conversion with those rates and their
//! date attached ([`Revenue::converted`]).

use std::collections::BTreeMap;

use nocturne_app::{App, AppError, AppResult, util};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::model::{CustomerState, InvoiceState, Settings};
use crate::money::{
    Conversion, ConversionInput, Converted, Currency, Money, Totals, convert_totals,
};

const DAY_MS: i64 = 86_400_000;

// ------------------------------------------------------------------ arrears

/// One age band of what is owed, in one currency.
#[derive(Clone, Debug, Serialize)]
pub struct Bucket {
    /// `not yet due`, `1–30 days`, `over 90 days`, …
    pub label: String,
    /// Days overdue this band starts at; `-1` for "not yet due".
    pub from_days: i64,
    /// `None` on the last band.
    pub to_days: Option<i64>,
    pub amount: Money,
    pub invoices: u32,
}

/// What is owed in one currency, aged. A report, never a row in a bigger sum.
#[derive(Clone, Debug, Serialize)]
pub struct CurrencyArrears {
    pub currency: Currency,
    pub total: Money,
    /// What is owed but not yet due, kept out of `overdue`.
    pub not_yet_due: Money,
    pub overdue: Money,
    pub buckets: Vec<Bucket>,
    pub customers: u32,
    pub invoices: u32,
}

/// What one customer owes, in one currency.
#[derive(Clone, Debug, Serialize)]
pub struct CustomerArrears {
    pub customer_id: String,
    pub customer_name: String,
    pub state: CustomerState,
    pub outstanding: Money,
    /// Days overdue of their oldest unpaid invoice; `None` when nothing of
    /// theirs is due yet.
    pub oldest_days: Option<i64>,
    pub invoices: u32,
}

#[derive(Debug, Serialize)]
pub struct Arrears {
    pub as_of: i64,
    /// One aged report per currency. **There is no total across them**: that
    /// figure is not a number, and this result deliberately has nowhere to
    /// put it.
    pub by_currency: Vec<CurrencyArrears>,
    /// Who owes what, worst first within each currency.
    pub customers: Vec<CustomerArrears>,
    pub note: String,
}

fn bands(settings: &Settings) -> Vec<(i64, Option<i64>, String)> {
    let mut edges: Vec<i64> = settings
        .arrears_buckets
        .iter()
        .map(|days| i64::from(*days))
        .filter(|days| *days > 0)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    if edges.is_empty() {
        edges = vec![30, 60, 90];
    }
    let mut bands = vec![(-1, Some(-1), "not yet due".to_string())];
    let mut from = 0;
    for edge in &edges {
        bands.push((from, Some(*edge), format!("{}–{} days", from.max(1), edge)));
        from = edge + 1;
    }
    bands.push((from, None, format!("over {} days", from - 1)));
    bands
}

pub fn arrears(app: &App, settings: &Settings, customer_id: Option<&str>) -> AppResult<Arrears> {
    let store = app.store();
    let now = util::now_ms();
    let shortfalls = db::shortfalls(store, customer_id)?;
    let bands = bands(settings);

    // Per currency, then per customer; nothing crosses a currency boundary.
    let mut per_currency: BTreeMap<&'static str, CurrencyArrears> = BTreeMap::new();
    let mut per_customer: BTreeMap<(String, &'static str), CustomerArrears> = BTreeMap::new();
    let mut names: BTreeMap<String, (String, CustomerState)> = BTreeMap::new();

    for shortfall in &shortfalls {
        let currency = shortfall.currency;
        let outstanding = shortfall.outstanding();
        // No due date means nobody has been asked for it yet, so it is owed
        // but not overdue.
        let days_overdue = shortfall
            .due_at
            .map(|due| (now - due) / DAY_MS)
            .filter(|days| *days >= 0);
        let entry = per_currency
            .entry(currency.code())
            .or_insert_with(|| CurrencyArrears {
                currency,
                total: Money::zero(currency),
                not_yet_due: Money::zero(currency),
                overdue: Money::zero(currency),
                buckets: bands
                    .iter()
                    .map(|(from, to, label)| Bucket {
                        label: label.clone(),
                        from_days: *from,
                        to_days: *to,
                        amount: Money::zero(currency),
                        invoices: 0,
                    })
                    .collect(),
                customers: 0,
                invoices: 0,
            });
        entry.total = entry.total.plus(outstanding)?;
        entry.invoices += 1;
        match days_overdue {
            None => entry.not_yet_due = entry.not_yet_due.plus(outstanding)?,
            Some(_) => entry.overdue = entry.overdue.plus(outstanding)?,
        }
        let index = match days_overdue {
            None => 0,
            Some(days) => bands
                .iter()
                .position(|(from, to, _)| {
                    *from >= 0 && days >= *from && to.map(|to| days <= to).unwrap_or(true)
                })
                .unwrap_or(bands.len() - 1),
        };
        let bucket = &mut entry.buckets[index];
        bucket.amount = bucket.amount.plus(outstanding)?;
        bucket.invoices += 1;

        if !names.contains_key(&shortfall.customer_id) {
            let customer = db::customer(store, &shortfall.customer_id)?;
            names.insert(
                shortfall.customer_id.clone(),
                match customer {
                    Some(customer) => (customer.name, customer.state),
                    // The ledger outlives the customer record; it is still
                    // money owed, and saying so is better than dropping it.
                    None => ("(deleted customer)".to_string(), CustomerState::Cancelled),
                },
            );
        }
        let (name, state) = names
            .get(&shortfall.customer_id)
            .cloned()
            .unwrap_or_else(|| (String::new(), CustomerState::Active));
        let row = per_customer
            .entry((shortfall.customer_id.clone(), currency.code()))
            .or_insert_with(|| CustomerArrears {
                customer_id: shortfall.customer_id.clone(),
                customer_name: name,
                state,
                outstanding: Money::zero(currency),
                oldest_days: None,
                invoices: 0,
            });
        row.outstanding = row.outstanding.plus(outstanding)?;
        row.invoices += 1;
        row.oldest_days = match (row.oldest_days, days_overdue) {
            (Some(old), Some(days)) => Some(old.max(days)),
            (None, days) => days,
            (old, None) => old,
        };
    }

    // One customer owing in two currencies counts once in each of them.
    for (_, code) in per_customer.keys() {
        if let Some(entry) = per_currency.get_mut(code) {
            entry.customers += 1;
        }
    }

    let mut customers: Vec<CustomerArrears> = per_customer.into_values().collect();
    customers.sort_by(|a, b| {
        a.outstanding
            .currency
            .code()
            .cmp(b.outstanding.currency.code())
            .then(b.outstanding.minor.cmp(&a.outstanding.minor))
    });
    Ok(Arrears {
        as_of: now,
        by_currency: per_currency.into_values().collect(),
        customers,
        note: "one aged report per currency, and no total across them: an amount owed in dinars \
               and an amount owed in dollars are two facts, and adding them would be a third \
               thing that is not true"
            .to_string(),
    })
}

// ------------------------------------------------------------------ revenue

/// Ask for one figure instead of several: the caller names the currency and
/// supplies every rate, and the answer says so.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConvertRequest {
    /// The currency to express the figure in.
    pub to: String,
    /// The rates to use, as entered. There is no rate feed.
    #[serde(default)]
    pub rates: Vec<ConversionInput>,
    /// When these rates are from; now by default.
    #[serde(default)]
    pub at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ConvertedRevenue {
    pub issued: Converted,
    pub received: Converted,
}

#[derive(Debug, Serialize)]
pub struct Revenue {
    pub since: i64,
    pub until: i64,
    /// What was invoiced in the window, per currency.
    pub issued: Totals,
    /// What was actually received in the window, in the currency it was
    /// received in — which is not the same list.
    pub received: Totals,
    /// What is owed right now, per currency.
    pub outstanding: Totals,
    pub invoices_issued: u32,
    /// Only present when the caller supplied the rates, and labelled a
    /// conversion rather than the ledger.
    pub converted: Option<ConvertedRevenue>,
    pub note: String,
}

pub fn revenue(
    app: &App,
    since: i64,
    until: i64,
    convert: Option<ConvertRequest>,
) -> AppResult<Revenue> {
    if until <= since {
        return Err(AppError::invalid("a revenue window ends after it starts"));
    }
    let store = app.store();
    let mut issued = Totals::new();
    let mut invoices_issued = 0_u64;
    for (code, minor, count) in db::issued_between(store, since, until)? {
        issued.add(Money::new(minor, Currency::of(&code)?))?;
        invoices_issued += count;
    }
    let mut received = Totals::new();
    for (code, minor) in db::received_between(store, since, until)? {
        received.add(Money::new(minor, Currency::of(&code)?))?;
    }
    let mut outstanding = Totals::new();
    for shortfall in db::shortfalls(store, None)? {
        outstanding.add(shortfall.outstanding())?;
    }

    let converted = match convert {
        None => None,
        Some(request) => {
            let to = Currency::of(&request.to)?;
            let at = request.at.unwrap_or_else(util::now_ms);
            let rates: Vec<Conversion> = request
                .rates
                .iter()
                .map(|rate| rate.resolve(at))
                .collect::<AppResult<Vec<_>>>()?;
            Some(ConvertedRevenue {
                issued: convert_totals(&issued, to, &rates, at)?,
                received: convert_totals(&received, to, &rates, at)?,
            })
        }
    };
    Ok(Revenue {
        since,
        until,
        issued,
        received,
        outstanding,
        invoices_issued: u32::try_from(invoices_issued).unwrap_or(u32::MAX),
        converted,
        note: "issued and received are different facts and are not netted off; each is one \
               figure per currency. Any converted figure beside them carries the rates it used \
               and is not the ledger."
            .to_string(),
    })
}

// ------------------------------------------------------------------ summary

/// Counts and states — **no personal details**. This is the one method in
/// this feature that is safe to put in front of a model: there is no name,
/// no phone number, no email address and no street in it, and there must
/// never be. `tests/assistant_safety.rs` holds it to that.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub customers: u32,
    pub by_state: Vec<StateCount>,
    pub by_currency: Vec<CurrencyCount>,
    pub subscriptions_active: u32,
    /// The megabits of every active subscription.
    pub sold_mbit: i64,
    pub tariffs: u32,
    pub links: u32,
    pub invoices: InvoiceCounts,
    /// What is owed, per currency, with no total across them.
    pub arrears: Vec<CurrencyArrears>,
    /// What came in over the last 30 days, per currency as received.
    pub received_30d: Totals,
    pub usage_coverage: UsageCoverage,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct StateCount {
    pub state: String,
    pub customers: u32,
}

#[derive(Debug, Serialize)]
pub struct CurrencyCount {
    pub currency: Currency,
    pub customers: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct InvoiceCounts {
    pub draft: u32,
    pub issued: u32,
    pub void: u32,
    /// Issued, due and still short.
    pub overdue: u32,
}

/// How much of the estate a usage report can actually say anything about.
#[derive(Debug, Serialize)]
pub struct UsageCoverage {
    pub with_device_and_interface: u32,
    pub with_device_only: u32,
    pub without_device: u32,
    /// Whether this build has monitoring at all. Without it, no customer has
    /// a usage figure.
    pub monitoring_present: bool,
    pub note: String,
}

pub fn summary(app: &App, settings: &Settings) -> AppResult<Summary> {
    let store = app.store();
    let now = util::now_ms();
    let mut customers = 0_u32;
    let mut by_state: BTreeMap<String, u32> = BTreeMap::new();
    let mut by_currency: BTreeMap<&'static str, (Currency, u32)> = BTreeMap::new();
    for (state, currency, count) in db::customer_counts(store)? {
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        customers += count;
        *by_state.entry(state).or_insert(0) += count;
        let currency = Currency::of(&currency)?;
        let entry = by_currency.entry(currency.code()).or_insert((currency, 0));
        entry.1 += count;
    }
    let active = db::subscriptions(
        store,
        &db::SubscriptionQuery {
            state: Some("active".to_string()),
            ..db::SubscriptionQuery::default()
        },
    )?;
    let mut invoices = InvoiceCounts::default();
    for (state, count) in db::invoice_counts(store)? {
        let count = u32::try_from(count).unwrap_or(u32::MAX);
        match InvoiceState::of(&state)? {
            InvoiceState::Draft => invoices.draft = count,
            InvoiceState::Issued => invoices.issued = count,
            InvoiceState::Void => invoices.void = count,
        }
    }
    let arrears = arrears(app, settings, None)?;
    invoices.overdue = u32::try_from(
        arrears
            .by_currency
            .iter()
            .map(|row| {
                row.buckets
                    .iter()
                    .filter(|bucket| bucket.from_days >= 0)
                    .map(|bucket| u64::from(bucket.invoices))
                    .sum::<u64>()
            })
            .sum::<u64>(),
    )
    .unwrap_or(u32::MAX);
    let mut received_30d = Totals::new();
    for (code, minor) in db::received_between(store, now - 30 * DAY_MS, now)? {
        received_30d.add(Money::new(minor, Currency::of(&code)?))?;
    }
    let (with_both, with_device, without_device) = db::service_coverage(store)?;
    Ok(Summary {
        customers,
        by_state: by_state
            .into_iter()
            .map(|(state, customers)| StateCount { state, customers })
            .collect(),
        by_currency: by_currency
            .into_values()
            .map(|(currency, customers)| CurrencyCount {
                currency,
                customers,
            })
            .collect(),
        subscriptions_active: u32::try_from(active.len()).unwrap_or(u32::MAX),
        sold_mbit: active.iter().map(|plan| plan.mbit).sum(),
        tariffs: u32::try_from(store.count(db::TARIFFS)?).unwrap_or(u32::MAX),
        links: u32::try_from(store.count(db::LINKS)?).unwrap_or(u32::MAX),
        invoices,
        arrears: arrears.by_currency,
        received_30d,
        usage_coverage: UsageCoverage {
            with_device_and_interface: with_both,
            with_device_only: with_device,
            without_device,
            // A feature that is not in this build has no methods registered,
            // which is the honest way to say "nothing measures interfaces
            // here" without pretending to have asked a device.
            monitoring_present: app.methods().contains(&"monitor.interfaces"),
            note: "a customer without a device and an interface has no usage figure at all — not \
                   a usage of zero"
                .to_string(),
        },
        note: "counts, states and per-currency money only: no customer's name, contact details \
               or address appears in this result, which is what makes it the one safe to put in \
               front of a model"
            .to_string(),
    })
}
