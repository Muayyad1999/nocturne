//! The records a person writes: customers, tariffs, subscriptions and links.
//!
//! Nothing in here touches a device. `service.device_id` is checked against
//! the inventory so that a customer cannot be hung off a device that does not
//! exist, and that is the whole of this feature's reach into the network from
//! the writing side.
//!
//! The writes that are guarded by a check on the customer's books — deleting
//! them, changing the currency they are billed in, agreeing a plan in it — do
//! the check and the write in one transaction ([`db::atomically`]), for the
//! reason `billing.rs` gives: a check that another request can overtake is
//! not a check.

use nocturne_app::{App, AppError, AppResult, inventory, util};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::model::{
    Customer, CustomerState, Link, Period, Service, Subscription, SubscriptionState, Tariff, Unit,
    agreed_price, megabits, period_price,
};
use crate::money::{Currency, Money};

// ----------------------------------------------------------------- customers

/// Everything about a customer a caller may set. Absent fields are left as
/// they are on an update; `state` is deliberately not one of them — it goes
/// through `isp.customer.state`, so that the reason is recorded with it.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CustomerInput {
    pub id: Option<String>,
    pub code: Option<String>,
    pub name: Option<String>,
    /// Required when creating; changing it afterwards is refused while the
    /// customer has a ledger.
    pub currency: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,
    pub address: Option<String>,
    pub site: Option<String>,
    /// An inventory device id; it must exist.
    pub device_id: Option<String>,
    pub interface: Option<String>,
    pub tower: Option<String>,
    pub location: Option<String>,
    pub tags: Option<Vec<String>>,
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SavedCustomer {
    pub customer: Customer,
    pub created: bool,
    /// Things worth telling the person who saved it, that are not errors.
    pub warnings: Vec<String>,
}

fn trimmed(value: Option<String>) -> Option<String> {
    value.map(|text| text.trim().to_string())
}

/// `None` for an empty string: "no interface" and "the interface called ''"
/// are different things, and only one of them is real.
fn optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn save_customer(app: &App, mut input: CustomerInput) -> AppResult<SavedCustomer> {
    // The inventory is read through the app, which takes the store's lock, so
    // it is asked before the transaction below rather than inside it.
    // `Some(None)` is a caller clearing the device.
    let device: Option<Option<String>> = match trimmed(input.device_id.take()) {
        Some(device_id) => Some(match optional(&device_id) {
            // A customer hung off a device that is not in the inventory
            // would silently have no usage figure for ever.
            Some(id) => Some(inventory::get(app, &id)?.id),
            None => None,
        }),
        None => None,
    };
    db::atomically(app.store(), |conn| {
        let now = util::now_ms();
        let mut warnings: Vec<String> = Vec::new();
        let existing = match &input.id {
            Some(id) if !id.trim().is_empty() => Some(db::require_customer(conn, id.trim())?),
            _ => None,
        };
        let created = existing.is_none();
        let mut customer = existing.clone().unwrap_or_else(|| Customer {
            id: util::new_id(),
            created_at: now,
            ..Customer::default()
        });

        if let Some(name) = trimmed(input.name) {
            if name.is_empty() {
                return Err(AppError::invalid("a customer needs a name"));
            }
            customer.name = name;
        }
        if customer.name.trim().is_empty() {
            return Err(AppError::invalid("a customer needs a name"));
        }

        match (trimmed(input.currency), &existing) {
            (Some(code), None) => customer.currency = Currency::of(&code)?,
            (Some(code), Some(before)) => {
                let wanted = Currency::of(&code)?;
                if wanted != before.currency {
                    let invoices = db::count_invoices(conn, &before.id)?;
                    let payments = db::count_payments(conn, &before.id)?;
                    // A credit note is as much a part of their books as an
                    // invoice: a numbered document, in the old currency.
                    let credits = db::count_credits(conn, &before.id)?;
                    if invoices > 0 || payments > 0 || credits > 0 {
                        return Err(AppError::conflict(format!(
                            "{} is billed in {} and has {invoices} invoice(s), {payments} \
                             payment(s) and {credits} credit note(s) in it. Changing the \
                             currency now would leave a ledger in two currencies with nothing \
                             saying which figure was which. Cancel this customer and create the \
                             new account instead.",
                            before.name, before.currency
                        )));
                    }
                    // Ended ones count too. A plan is invoiced in the currency
                    // it was agreed in, so with the customer's currency moved
                    // from under it every draft would leave it out — for an
                    // ended plan, the part of it nobody has billed yet.
                    let subscriptions = db::count_subscriptions(conn, &before.id)?;
                    if subscriptions > 0 {
                        return Err(AppError::conflict(format!(
                            "{} has {subscriptions} subscription(s) agreed in {}, and a plan is \
                             invoiced in the currency it was agreed in: with the customer billed \
                             in {wanted} instead, every billing run would leave them out. \
                             Nothing has been billed, so this customer can be deleted and the \
                             account created again in {wanted}; or cancel it and open a new \
                             one, if what was agreed in {} still has to be invoiced.",
                            before.name, before.currency, before.currency
                        )));
                    }
                    customer.currency = wanted;
                }
            }
            (None, None) => {
                return Err(AppError::invalid(format!(
                    "say which currency this customer is billed in: nothing here guesses one. \
                     Accepted: {}",
                    crate::money::CURRENCIES
                        .iter()
                        .map(|known| known.code)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            (None, Some(_)) => {}
        }

        if let Some(code) = trimmed(input.code) {
            if !code.is_empty()
                && let Some(other) = db::customer_by_code(conn, &code)?
                && other != customer.id
            {
                return Err(AppError::conflict(format!(
                    "account code `{code}` already belongs to another customer"
                )));
            }
            customer.code = code;
        }
        if let Some(phone) = trimmed(input.phone) {
            customer.contact.phone = phone;
        }
        if let Some(email) = trimmed(input.email) {
            customer.contact.email = email;
        }
        if let Some(address) = trimmed(input.address) {
            customer.contact.address = address;
        }
        if let Some(site) = trimmed(input.site) {
            customer.site = site;
        }
        if let Some(tags) = input.tags {
            customer.tags = tags
                .into_iter()
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty())
                .collect();
        }
        if let Some(notes) = input.notes {
            customer.notes = notes;
        }

        let mut service = Service {
            ..customer.service.clone()
        };
        if let Some(device_id) = device {
            service.device_id = device_id;
        }
        if let Some(interface) = trimmed(input.interface) {
            service.interface = optional(&interface);
        }
        if let Some(tower) = trimmed(input.tower) {
            service.tower = optional(&tower);
        }
        if let Some(location) = input.location {
            service.location = location.trim().to_string();
        }
        if service.interface.is_some() && service.device_id.is_none() {
            warnings.push(
                "an interface is recorded but no device, so there is nothing to measure this \
                 customer's usage on"
                    .to_string(),
            );
        }
        if service.device_id.is_some() && service.interface.is_none() {
            warnings.push(
                "a device is recorded but no interface, so this customer has no usage figure until \
                 one is"
                    .to_string(),
            );
        }
        customer.service = service;
        customer.updated_at = now;
        db::put_customer(conn, &customer)?;
        Ok(SavedCustomer {
            customer,
            created,
            warnings,
        })
    })
}

#[derive(Debug, Serialize)]
pub struct StateChange {
    pub customer: Customer,
    pub was: CustomerState,
    /// Always false, and here to be read: nothing in this crate configures a
    /// device, so suspending a customer changes a record and nothing else.
    pub device_changed: bool,
    pub note: String,
}

/// Why the account changed state is not stored on the customer: it belongs in
/// the audit trail with the person and the moment, which is where the caller
/// puts it.
pub fn set_customer_state(app: &App, id: &str, state: &str) -> AppResult<StateChange> {
    // One transaction, so that a save of the same customer that read its
    // copy a moment earlier cannot write the old state back over this one.
    db::atomically(app.store(), |conn| {
        let mut customer = db::require_customer(conn, id)?;
        let was = customer.state;
        customer.state = CustomerState::of(state)?;
        customer.updated_at = util::now_ms();
        db::put_customer(conn, &customer)?;
        Ok(StateChange {
            customer,
            was,
            device_changed: false,
            note: "this records a decision about the account; nothing was sent to a device. \
                   Suspending the service itself is a change somebody makes on the router."
                .to_string(),
        })
    })
}

#[derive(Debug, Serialize)]
pub struct DeletedCustomer {
    pub deleted: bool,
    pub name: String,
}

/// Delete a customer who has no ledger at all. One invoice, payment or
/// credit note — void ones included, because they are still documents — and
/// this is refused: their books outlive the account.
///
/// The count and the delete are one transaction, so an invoice being drafted
/// for them at the same moment either lands first and stops this, or finds
/// them gone.
pub fn delete_customer(app: &App, id: &str) -> AppResult<DeletedCustomer> {
    db::atomically(app.store(), |conn| {
        let customer = db::require_customer(conn, id)?;
        let invoices = db::count_invoices(conn, id)?;
        let payments = db::count_payments(conn, id)?;
        let credits = db::count_credits(conn, id)?;
        if invoices > 0 || payments > 0 || credits > 0 {
            return Err(AppError::conflict(format!(
                "{} has {invoices} invoice(s), {payments} payment(s) and {credits} credit \
                 note(s), and deleting a customer does not delete their ledger — somebody will \
                 ask about them. Set their state to `cancelled` instead; their invoices, \
                 payments and credit notes stay.",
                customer.name
            )));
        }
        // Nothing charged for these, so there is nothing for them to be the
        // provenance of; leaving them would point at a customer who is gone.
        db::delete_subscriptions_of(conn, id)?;
        let deleted = db::delete_customer(conn, id)?;
        Ok(DeletedCustomer {
            deleted,
            name: customer.name,
        })
    })
}

// ------------------------------------------------------------------- tariffs

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TariffInput {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub mbit: Option<i64>,
    /// `per_mbit` or `flat`.
    pub unit: Option<String>,
    /// Per megabit for `per_mbit`, the whole plan for `flat`.
    pub price: Option<Money>,
    /// `monthly`, `quarterly` or `yearly`.
    pub period: Option<String>,
}

/// A tariff with the figure one period of it costs worked out.
#[derive(Debug, Serialize)]
pub struct TariffRow {
    #[serde(flatten)]
    pub tariff: Tariff,
    /// `price × mbit` for a per-megabit tariff, `price` for a flat one.
    pub period_price: Money,
    /// How many subscriptions were agreed on these terms, and how many of
    /// those are still running. A subscription keeps its own copy of the
    /// figures, so changing this tariff will not move them.
    pub subscriptions: u32,
    pub active_subscriptions: u32,
}

pub fn tariff_rows(app: &App) -> AppResult<Vec<TariffRow>> {
    let store = app.store();
    let mut rows = Vec::new();
    for tariff in store.list::<Tariff>(db::TARIFFS)? {
        let all = db::subscriptions(
            store,
            &db::SubscriptionQuery {
                tariff_id: Some(tariff.id.clone()),
                ..db::SubscriptionQuery::default()
            },
        )?;
        let active = all
            .iter()
            .filter(|subscription| subscription.state == SubscriptionState::Active)
            .count();
        rows.push(TariffRow {
            period_price: period_price(tariff.unit, tariff.price, tariff.mbit)?,
            subscriptions: u32::try_from(all.len()).unwrap_or(u32::MAX),
            active_subscriptions: u32::try_from(active).unwrap_or(u32::MAX),
            tariff,
        });
    }
    rows.sort_by_key(|row| row.tariff.name.to_lowercase());
    Ok(rows)
}

#[derive(Debug, Serialize)]
pub struct SavedTariff {
    pub tariff: Tariff,
    pub period_price: Money,
    pub created: bool,
    /// How many running subscriptions keep their own copy of the old figures
    /// and are therefore untouched by this save.
    pub subscriptions_unaffected: u32,
}

pub fn save_tariff(app: &App, input: TariffInput) -> AppResult<SavedTariff> {
    let store = app.store();
    let now = util::now_ms();
    let existing = match &input.id {
        Some(id) if !id.trim().is_empty() => {
            Some(store.require::<Tariff>(db::TARIFFS, id.trim(), "tariff")?)
        }
        _ => None,
    };
    let created = existing.is_none();
    let mut tariff = existing.unwrap_or_else(|| Tariff {
        id: util::new_id(),
        created_at: now,
        ..Tariff::default()
    });
    if let Some(name) = trimmed(input.name) {
        if name.is_empty() {
            return Err(AppError::invalid("a tariff needs a name"));
        }
        if let Some(other) = store.named(db::TARIFFS, &name)?
            && other != tariff.id
        {
            return Err(AppError::conflict(format!(
                "there is already a tariff called `{name}`"
            )));
        }
        tariff.name = name;
    }
    if tariff.name.trim().is_empty() {
        return Err(AppError::invalid("a tariff needs a name"));
    }
    if let Some(description) = input.description {
        tariff.description = description;
    }
    if let Some(mbit) = input.mbit {
        tariff.mbit = mbit;
    }
    if let Some(unit) = &input.unit {
        tariff.unit = Unit::of(unit)?;
    }
    if let Some(price) = input.price {
        tariff.price = price;
    }
    if let Some(period) = &input.period {
        tariff.period = Period::of(period)?;
    }
    if tariff.mbit <= 0 {
        return Err(AppError::invalid(
            "a tariff sells a number of megabits: say how many",
        ));
    }
    if tariff.price.is_negative() {
        return Err(AppError::invalid("a tariff's price cannot be negative"));
    }
    let period_price = agreed_price(tariff.unit, tariff.price, tariff.mbit)?;
    tariff.updated_at = now;
    store.put(db::TARIFFS, &tariff.id, &tariff)?;
    let unaffected = db::subscriptions(
        store,
        &db::SubscriptionQuery {
            tariff_id: Some(tariff.id.clone()),
            state: Some("active".to_string()),
            ..db::SubscriptionQuery::default()
        },
    )?
    .len();
    Ok(SavedTariff {
        tariff,
        period_price,
        created,
        subscriptions_unaffected: u32::try_from(unaffected).unwrap_or(u32::MAX),
    })
}

pub fn delete_tariff(app: &App, id: &str) -> AppResult<(bool, String)> {
    let store = app.store();
    let tariff = store.require::<Tariff>(db::TARIFFS, id, "tariff")?;
    let active = db::subscriptions(
        store,
        &db::SubscriptionQuery {
            tariff_id: Some(id.to_string()),
            state: Some("active".to_string()),
            ..db::SubscriptionQuery::default()
        },
    )?;
    if !active.is_empty() {
        return Err(AppError::conflict(format!(
            "{} customer(s) are on `{}` right now. End their subscriptions first — the figures \
             they were agreed at are their own copies and will survive this either way.",
            active.len(),
            tariff.name
        )));
    }
    Ok((store.delete(db::TARIFFS, id)?, tariff.name))
}

// ------------------------------------------------------------- subscriptions

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SubscriptionInput {
    pub customer_id: Option<String>,
    pub tariff_id: Option<String>,
    /// Override the tariff's megabits for this customer.
    pub mbit: Option<i64>,
    /// Override the tariff's unit price (a negotiated figure).
    pub price: Option<Money>,
    /// Override the tariff's pricing unit.
    pub unit: Option<String>,
    /// Override the tariff's billing period.
    pub period: Option<String>,
    /// When it starts; now by default.
    pub started_at: Option<i64>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SavedSubscription {
    pub subscription: Subscription,
    /// The subscription this one replaced, when terms were amended.
    pub ended: Option<Subscription>,
    pub note: String,
}

/// Put a customer on a tariff, copying the agreed figures into the
/// subscription. Nothing afterwards reads the tariff to work them out again.
pub fn create_subscription(app: &App, input: SubscriptionInput) -> AppResult<SavedSubscription> {
    let store = app.store();
    let now = util::now_ms();
    let customer_id = input
        .customer_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AppError::invalid("say which customer this subscription is for"))?;
    let tariff_id = input
        .tariff_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AppError::invalid("say which tariff they are on"))?;
    // A tariff is a document in the core's collection, read through the
    // store, so it is fetched before the transaction rather than inside it.
    // The subscription takes a copy of its figures either way.
    let tariff = store.require::<Tariff>(db::TARIFFS, tariff_id, "tariff")?;

    let unit = match &input.unit {
        Some(unit) => Unit::of(unit)?,
        None => tariff.unit,
    };
    let period = match &input.period {
        Some(period) => Period::of(period)?,
        None => tariff.period,
    };
    let mbit = input.mbit.unwrap_or(tariff.mbit);
    if mbit <= 0 {
        return Err(AppError::invalid(
            "a subscription is for a number of megabits: say how many",
        ));
    }
    let unit_price = input.price.unwrap_or(tariff.price);
    if unit_price.is_negative() {
        return Err(AppError::invalid("an agreed price cannot be negative"));
    }
    let price = agreed_price(unit, unit_price, mbit)?;

    // The customer's currency is checked in the transaction that writes the
    // subscription: a currency change is refused once a plan exists, and the
    // two would otherwise each pass the other's check.
    let subscription = db::atomically(store, |conn| {
        let customer = db::require_customer(conn, customer_id)?;
        if customer.state == CustomerState::Cancelled {
            return Err(AppError::conflict(format!(
                "{} is cancelled; reactivate the account before selling them a plan",
                customer.name
            )));
        }
        if unit_price.currency != customer.currency {
            return Err(AppError::invalid(format!(
                "{} is billed in {} and this price is in {}. Nothing here converts between \
                 them: price the plan in the customer's currency, or bill the customer in the \
                 price's.",
                customer.name, customer.currency, unit_price.currency
            )));
        }
        let subscription = Subscription {
            id: util::new_id(),
            customer_id: customer.id.clone(),
            tariff_id: tariff.id.clone(),
            tariff_name: tariff.name.clone(),
            mbit,
            unit,
            unit_price,
            price,
            period,
            started_at: input.started_at.unwrap_or(now),
            ended_at: None,
            state: SubscriptionState::Active,
            amends: None,
            note: input.note.clone().unwrap_or_default(),
            created_at: now,
            updated_at: now,
        };
        db::put_subscription(conn, &subscription)?;
        Ok(subscription)
    })?;
    Ok(SavedSubscription {
        subscription,
        ended: None,
        note: format!(
            "the agreed megabits and price are this subscription's own copy; changing the tariff \
             `{}` later will not move them",
            tariff.name
        ),
    })
}

pub fn end_subscription(
    app: &App,
    id: &str,
    ended_at: Option<i64>,
    reason: &str,
) -> AppResult<Subscription> {
    db::atomically(app.store(), |conn| {
        let subscription = db::require_subscription(conn, id)?;
        if subscription.state == SubscriptionState::Ended {
            return Err(AppError::conflict(format!(
                "that subscription already ended{}",
                subscription
                    .ended_at
                    .map(|at| format!(" (at {at})"))
                    .unwrap_or_default()
            )));
        }
        // With no date given it ends now — unless it has not begun yet. A
        // plan sold to start on the first of next month and taken back today
        // never ran, and "now" would end it before it started; it ends where
        // it would have begun.
        let ended_at = ended_at.unwrap_or_else(|| util::now_ms().max(subscription.started_at));
        // Ending it at its first moment is a plan that never ran, which is a
        // thing that happens. Ending it before that is a plan that covers
        // less than no time, and every later sum over it would be nonsense.
        // A date the caller gave is taken at its word and refused.
        if ended_at < subscription.started_at {
            return Err(AppError::invalid(
                "a subscription cannot end before it started: `ended_at` is earlier than its \
                 `started_at`. To take back a plan that never ran, end it at the moment it \
                 started.",
            ));
        }
        let ended = Subscription {
            state: SubscriptionState::Ended,
            ended_at: Some(ended_at),
            note: if reason.trim().is_empty() {
                subscription.note.clone()
            } else {
                format!("{} ended: {}", subscription.note, reason.trim())
                    .trim()
                    .to_string()
            },
            updated_at: util::now_ms(),
            ..subscription
        };
        db::put_subscription(conn, &ended)?;
        Ok(ended)
    })
}

/// Change what a customer pays from a date: the old subscription ends and a
/// new one starts, so last year's invoices still say what they said.
///
/// One transaction: the old terms ending and the new ones starting are one
/// event, and half of it would be a customer with no plan or with two.
pub fn amend_subscription(
    app: &App,
    id: &str,
    from_at: Option<i64>,
    mbit: Option<i64>,
    price: Option<Money>,
    note: &str,
) -> AppResult<SavedSubscription> {
    db::atomically(app.store(), |conn| {
        let now = util::now_ms();
        let before = db::require_subscription(conn, id)?;
        // With no date given the new terms start now — or, for a plan that
        // has not begun yet, when it begins: they replace the old terms
        // before anybody was charged on them.
        let from = from_at.unwrap_or_else(|| now.max(before.started_at));
        if before.state == SubscriptionState::Ended {
            return Err(AppError::conflict(
                "that subscription has ended; sell a new one rather than amending history",
            ));
        }
        let customer = db::require_customer(conn, &before.customer_id)?;
        if mbit.is_none() && price.is_none() {
            return Err(AppError::invalid(
                "an amendment changes the megabits, the price or both",
            ));
        }
        let mbit = mbit.unwrap_or(before.mbit);
        if mbit <= 0 {
            return Err(AppError::invalid("a subscription needs its megabits"));
        }
        let unit_price = price.unwrap_or(before.unit_price);
        if unit_price.currency != customer.currency {
            return Err(AppError::invalid(format!(
                "{} is billed in {} and this price is in {}",
                customer.name, customer.currency, unit_price.currency
            )));
        }
        // Amended terms are terms: whatever a new subscription may not be
        // agreed at, an existing one may not be amended to.
        if unit_price.is_negative() {
            return Err(AppError::invalid("an agreed price cannot be negative"));
        }
        let agreed = agreed_price(before.unit, unit_price, mbit)?;
        // The old terms end where the new ones start. Before the old ones
        // began, that would end them before they started and leave two
        // subscriptions claiming the same time. Only a date the caller gave
        // can be that early.
        if from < before.started_at {
            return Err(AppError::invalid(
                "new terms cannot start before the subscription they amend did: `from_at` is \
                 earlier than its `started_at`. To correct terms from the very start, amend \
                 from the moment it started.",
            ));
        }
        let ended = Subscription {
            state: SubscriptionState::Ended,
            ended_at: Some(from),
            updated_at: now,
            ..before.clone()
        };
        db::put_subscription(conn, &ended)?;
        let after = Subscription {
            id: util::new_id(),
            mbit,
            unit_price,
            price: agreed,
            started_at: from,
            ended_at: None,
            state: SubscriptionState::Active,
            amends: Some(before.id.clone()),
            note: note.trim().to_string(),
            created_at: now,
            updated_at: now,
            ..before
        };
        db::put_subscription(conn, &after)?;
        Ok(SavedSubscription {
            subscription: after,
            ended: Some(ended),
            note: "the old terms ended rather than being overwritten: invoices already issued on \
                   them are untouched and still say what they said"
                .to_string(),
        })
    })
}

// --------------------------------------------------------------------- links

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LinkInput {
    pub id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub device_id: Option<String>,
    pub interface: Option<String>,
    /// The label the customers behind it carry in their service.
    pub tower: Option<String>,
    /// What it can carry. `0` takes a capacity off again: the link then has
    /// no ratio, or falls back to its interface's own speed.
    pub capacity_mbit: Option<i64>,
    /// The wholesale cost of one megabit for one `cost_period`.
    pub cost_per_mbit: Option<Money>,
    /// Take the wholesale cost off the link. A field left out of a save
    /// keeps what is stored, and `null` reads as left out, so neither can
    /// say "there is no cost any more" — and without a way to say it, a
    /// margin goes on subtracting a cost that was removed. No cost is no
    /// margin, which is not the same as a cost of nothing.
    pub clear_cost: bool,
    pub cost_period: Option<String>,
}

pub fn links(app: &App) -> AppResult<Vec<Link>> {
    let mut links = app.store().list::<Link>(db::LINKS)?;
    links.sort_by_key(|link| link.name.to_lowercase());
    Ok(links)
}

pub fn save_link(app: &App, input: LinkInput) -> AppResult<(Link, bool)> {
    let store = app.store();
    let now = util::now_ms();
    let existing = match &input.id {
        Some(id) if !id.trim().is_empty() => {
            Some(store.require::<Link>(db::LINKS, id.trim(), "link")?)
        }
        _ => None,
    };
    let created = existing.is_none();
    let mut link = existing.unwrap_or_else(|| Link {
        id: util::new_id(),
        created_at: now,
        ..Link::default()
    });
    if let Some(name) = trimmed(input.name) {
        if name.is_empty() {
            return Err(AppError::invalid("a link needs a name"));
        }
        if let Some(other) = store.named(db::LINKS, &name)?
            && other != link.id
        {
            return Err(AppError::conflict(format!(
                "there is already a link called `{name}`"
            )));
        }
        link.name = name;
    }
    if link.name.trim().is_empty() {
        return Err(AppError::invalid("a link needs a name"));
    }
    if let Some(description) = input.description {
        link.description = description;
    }
    if let Some(device_id) = trimmed(input.device_id) {
        link.device_id = match optional(&device_id) {
            Some(id) => Some(inventory::get(app, &id)?.id),
            None => None,
        };
    }
    if let Some(interface) = trimmed(input.interface) {
        link.interface = optional(&interface);
    }
    if let Some(tower) = trimmed(input.tower) {
        link.tower = optional(&tower);
    }
    if let Some(capacity) = input.capacity_mbit {
        link.capacity_mbit = megabits(capacity, "a link's capacity")?;
    }
    if input.clear_cost {
        if input.cost_per_mbit.is_some() {
            return Err(AppError::invalid(
                "`clear_cost` takes the wholesale cost off the link and `cost_per_mbit` sets one: \
                 send one or the other",
            ));
        }
        link.cost_per_mbit = None;
    }
    if let Some(cost) = input.cost_per_mbit {
        if cost.is_negative() {
            return Err(AppError::invalid("a wholesale cost cannot be negative"));
        }
        link.cost_per_mbit = Some(cost.enterable("a wholesale cost")?);
    }
    if let Some(period) = &input.cost_period {
        link.cost_period = Period::of(period)?;
    }
    if link.capacity_mbit < 0 {
        return Err(AppError::invalid("a link's capacity cannot be negative"));
    }
    link.updated_at = now;
    store.put(db::LINKS, &link.id, &link)?;
    Ok((link, created))
}

pub fn delete_link(app: &App, id: &str) -> AppResult<(bool, String)> {
    let store = app.store();
    let link = store.require::<Link>(db::LINKS, id, "link")?;
    Ok((store.delete(db::LINKS, id)?, link.name))
}
