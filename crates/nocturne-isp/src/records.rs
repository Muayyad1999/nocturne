//! The records a person writes: customers, tariffs, subscriptions and links.
//!
//! Nothing in here touches a device. `service.device_id` is checked against
//! the inventory so that a customer cannot be hung off a device that does not
//! exist, and that is the whole of this feature's reach into the network from
//! the writing side.

use nocturne_app::{App, AppError, AppResult, inventory, util};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::model::{
    Customer, CustomerState, Link, Period, Service, Subscription, SubscriptionState, Tariff, Unit,
    period_price,
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

pub fn save_customer(app: &App, input: CustomerInput) -> AppResult<SavedCustomer> {
    let store = app.store();
    let now = util::now_ms();
    let mut warnings: Vec<String> = Vec::new();
    let existing = match &input.id {
        Some(id) if !id.trim().is_empty() => Some(db::require_customer(store, id.trim())?),
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
                let invoices = db::count_invoices(store, &before.id)?;
                let payments = db::count_payments(store, &before.id)?;
                if invoices > 0 || payments > 0 {
                    return Err(AppError::conflict(format!(
                        "{} is billed in {} and has {invoices} invoice(s) and {payments} \
                         payment(s) in it. Changing the currency now would leave a ledger in two \
                         currencies with nothing saying which figure was which. Cancel this \
                         customer and create the new account instead.",
                        before.name, before.currency
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
            && let Some(other) = db::customer_by_code(store, &code)?
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
    if let Some(device_id) = trimmed(input.device_id) {
        service.device_id = match optional(&device_id) {
            Some(id) => {
                // A customer hung off a device that is not in the inventory
                // would silently have no usage figure for ever.
                let device = inventory::get(app, &id)?;
                Some(device.id)
            }
            None => None,
        };
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
    db::put_customer(store, &customer)?;
    Ok(SavedCustomer {
        customer,
        created,
        warnings,
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
    let store = app.store();
    let mut customer = db::require_customer(store, id)?;
    let was = customer.state;
    customer.state = CustomerState::of(state)?;
    customer.updated_at = util::now_ms();
    db::put_customer(store, &customer)?;
    Ok(StateChange {
        customer,
        was,
        device_changed: false,
        note: "this records a decision about the account; nothing was sent to a device. \
               Suspending the service itself is a change somebody makes on the router."
            .to_string(),
    })
}

#[derive(Debug, Serialize)]
pub struct DeletedCustomer {
    pub deleted: bool,
    pub name: String,
}

/// Delete a customer who has no ledger at all. One invoice or payment and
/// this is refused: their books outlive the account.
pub fn delete_customer(app: &App, id: &str) -> AppResult<DeletedCustomer> {
    let store = app.store();
    let customer = db::require_customer(store, id)?;
    let invoices = db::count_invoices(store, id)?;
    let payments = db::count_payments(store, id)?;
    if invoices > 0 || payments > 0 {
        return Err(AppError::conflict(format!(
            "{} has {invoices} invoice(s) and {payments} payment(s), and deleting a customer does \
             not delete their ledger — somebody will ask about them. Set their state to \
             `cancelled` instead; their invoices and payments stay.",
            customer.name
        )));
    }
    // Nothing charged for these, so there is nothing for them to be the
    // provenance of; leaving them would point at a customer who is gone.
    db::delete_subscriptions_of(store, id)?;
    let deleted = db::delete_customer(store, id)?;
    Ok(DeletedCustomer {
        deleted,
        name: customer.name,
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
    let period_price = period_price(tariff.unit, tariff.price, tariff.mbit)?;
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
    let customer = db::require_customer(store, customer_id)?;
    if customer.state == CustomerState::Cancelled {
        return Err(AppError::conflict(format!(
            "{} is cancelled; reactivate the account before selling them a plan",
            customer.name
        )));
    }
    let tariff_id = input
        .tariff_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| AppError::invalid("say which tariff they are on"))?;
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
    if unit_price.currency != customer.currency {
        return Err(AppError::invalid(format!(
            "{} is billed in {} and this price is in {}. Nothing here converts between them: \
             price the plan in the customer's currency, or bill the customer in the price's.",
            customer.name, customer.currency, unit_price.currency
        )));
    }
    if unit_price.is_negative() {
        return Err(AppError::invalid("an agreed price cannot be negative"));
    }
    let price = period_price(unit, unit_price, mbit)?;
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
        note: input.note.unwrap_or_default(),
        created_at: now,
        updated_at: now,
    };
    db::put_subscription(store, &subscription)?;
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
    let store = app.store();
    let subscription = db::require_subscription(store, id)?;
    if subscription.state == SubscriptionState::Ended {
        return Err(AppError::conflict(format!(
            "that subscription already ended{}",
            subscription
                .ended_at
                .map(|at| format!(" (at {at})"))
                .unwrap_or_default()
        )));
    }
    let ended = Subscription {
        state: SubscriptionState::Ended,
        ended_at: Some(ended_at.unwrap_or_else(util::now_ms)),
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
    db::put_subscription(store, &ended)?;
    Ok(ended)
}

/// Change what a customer pays from a date: the old subscription ends and a
/// new one starts, so last year's invoices still say what they said.
pub fn amend_subscription(
    app: &App,
    id: &str,
    from_at: Option<i64>,
    mbit: Option<i64>,
    price: Option<Money>,
    note: &str,
) -> AppResult<SavedSubscription> {
    let store = app.store();
    let now = util::now_ms();
    let from = from_at.unwrap_or(now);
    let before = db::require_subscription(store, id)?;
    if before.state == SubscriptionState::Ended {
        return Err(AppError::conflict(
            "that subscription has ended; sell a new one rather than amending history",
        ));
    }
    let customer = db::require_customer(store, &before.customer_id)?;
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
    let ended = Subscription {
        state: SubscriptionState::Ended,
        ended_at: Some(from),
        updated_at: now,
        ..before.clone()
    };
    db::put_subscription(store, &ended)?;
    let after = Subscription {
        id: util::new_id(),
        mbit,
        unit_price,
        price: period_price(before.unit, unit_price, mbit)?,
        started_at: from,
        ended_at: None,
        state: SubscriptionState::Active,
        amends: Some(before.id.clone()),
        note: note.trim().to_string(),
        created_at: now,
        updated_at: now,
        ..before
    };
    db::put_subscription(store, &after)?;
    Ok(SavedSubscription {
        subscription: after,
        ended: Some(ended),
        note: "the old terms ended rather than being overwritten: invoices already issued on \
               them are untouched and still say what they said"
            .to_string(),
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
    pub capacity_mbit: Option<i64>,
    /// The wholesale cost of one megabit for one `cost_period`.
    pub cost_per_mbit: Option<Money>,
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
        link.capacity_mbit = capacity;
    }
    if let Some(cost) = input.cost_per_mbit {
        if cost.is_negative() {
            return Err(AppError::invalid("a wholesale cost cannot be negative"));
        }
        link.cost_per_mbit = Some(cost);
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
