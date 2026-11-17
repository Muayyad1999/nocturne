//! Where the ledger lives: four tables of this feature's own, and two
//! document collections for the records there are only dozens of.
//!
//! A feature crate cannot add a migration to the core's `SCHEMA`
//! (`nocturne-app/src/store.rs`), so these are created with `CREATE TABLE IF
//! NOT EXISTS` at init, exactly as `nocturne-ipam` does.
//!
//! Why tables rather than documents: the arrears report is "every invoice
//! that fell due before this date and is still short, aged, per currency",
//! over ten years of invoices and payments. That is a join and two indexes,
//! not a scan of a document collection. Tariffs and links are dozens of rows
//! that are always read whole, so they stay documents.
//!
//! **Every money column is an INTEGER of minor units with its currency in a
//! column beside it.** There is no REAL column in this schema and there must
//! never be one: `tests/money.rs` reads the schema back and fails if one
//! appears.

use nocturne_app::store::Store;
use nocturne_app::{AppError, AppResult};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::Deserialize;

use crate::model::{
    CreditNote, Customer, CustomerState, Invoice, InvoiceState, Payment, Subscription,
};
use crate::money::{Currency, Money};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS isp_customer (
    id         TEXT PRIMARY KEY,
    -- The ISP's own account number, '' when they do not use one.
    code       TEXT NOT NULL DEFAULT '',
    name       TEXT NOT NULL,
    state      TEXT NOT NULL,
    currency   TEXT NOT NULL,
    site       TEXT NOT NULL DEFAULT '',
    tower      TEXT NOT NULL DEFAULT '',
    device_id  TEXT NOT NULL DEFAULT '',
    interface  TEXT NOT NULL DEFAULT '',
    -- Lowercased text a free-text search matches.
    search     TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    body       TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS isp_customer_state ON isp_customer (state, name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS isp_customer_tower ON isp_customer (tower);
CREATE INDEX IF NOT EXISTS isp_customer_device ON isp_customer (device_id);
CREATE UNIQUE INDEX IF NOT EXISTS isp_customer_code ON isp_customer (code) WHERE code <> '';

CREATE TABLE IF NOT EXISTS isp_subscription (
    id          TEXT PRIMARY KEY,
    customer_id TEXT NOT NULL,
    -- Provenance only: the agreed figures are copied into the body.
    tariff_id   TEXT NOT NULL DEFAULT '',
    state       TEXT NOT NULL,
    mbit        INTEGER NOT NULL,
    currency    TEXT NOT NULL,
    price_minor INTEGER NOT NULL,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    body        TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS isp_subscription_customer ON isp_subscription (customer_id, state);
CREATE INDEX IF NOT EXISTS isp_subscription_tariff ON isp_subscription (tariff_id, state);

CREATE TABLE IF NOT EXISTS isp_invoice (
    id           TEXT PRIMARY KEY,
    -- Allocated at issue; '' while it is a draft.
    number       TEXT NOT NULL DEFAULT '',
    customer_id  TEXT NOT NULL,
    currency     TEXT NOT NULL,
    state        TEXT NOT NULL,
    total_minor  INTEGER NOT NULL,
    period_start INTEGER NOT NULL,
    period_end   INTEGER NOT NULL,
    issued_at    INTEGER,
    due_at       INTEGER,
    created_at   INTEGER NOT NULL,
    body         TEXT NOT NULL
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS isp_invoice_number ON isp_invoice (number) WHERE number <> '';
CREATE INDEX IF NOT EXISTS isp_invoice_customer ON isp_invoice (customer_id, period_start);
CREATE INDEX IF NOT EXISTS isp_invoice_due ON isp_invoice (state, due_at);

CREATE TABLE IF NOT EXISTS isp_payment (
    id               TEXT PRIMARY KEY,
    customer_id      TEXT NOT NULL,
    -- '' for a payment on account: received, not yet against an invoice.
    invoice_id       TEXT NOT NULL DEFAULT '',
    -- What was handed over.
    currency         TEXT NOT NULL,
    minor            INTEGER NOT NULL,
    -- What it settles, in the invoice's currency, worked out once at the rate
    -- it was taken at. '' and 0 when it settles nothing.
    settled_currency TEXT NOT NULL DEFAULT '',
    settled_minor    INTEGER NOT NULL DEFAULT 0,
    state            TEXT NOT NULL,
    at               INTEGER NOT NULL,
    body             TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS isp_payment_customer ON isp_payment (customer_id, at);
CREATE INDEX IF NOT EXISTS isp_payment_invoice ON isp_payment (invoice_id);

-- A credit note: money a customer stopped owing. Its own table rather than a
-- negative payment, because a negative payment would be counted as money
-- received by every report that sums `isp_payment`, and no money moved.
CREATE TABLE IF NOT EXISTS isp_credit (
    id          TEXT PRIMARY KEY,
    number      TEXT NOT NULL DEFAULT '',
    customer_id TEXT NOT NULL,
    -- '' when it credits the account rather than one invoice.
    invoice_id  TEXT NOT NULL DEFAULT '',
    -- Always the customer's billing currency: nothing is converted here.
    currency    TEXT NOT NULL,
    minor       INTEGER NOT NULL,
    state       TEXT NOT NULL,
    at          INTEGER NOT NULL,
    body        TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS isp_credit_customer ON isp_credit (customer_id, at);
CREATE INDEX IF NOT EXISTS isp_credit_invoice ON isp_credit (invoice_id);
";

pub fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
}

/// Records there are only dozens of, and that are always read whole.
pub const TARIFFS: &str = "isp_tariffs";
pub const LINKS: &str = "isp_links";

/// The key the next invoice number's counter lives under, per year.
fn number_key(year: i32) -> String {
    format!("isp.invoice_number.{year}")
}

fn credit_number_key(year: i32) -> String {
    format!("isp.credit_number.{year}")
}

fn body_of<T: serde::de::DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    let body: String = row.get(index)?;
    serde_json::from_str(&body).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn to_sql<T: serde::Serialize>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))
}

/// How many rows to skip, from a cursor that is the number already received.
pub fn offset_of(cursor: Option<&str>) -> AppResult<i64> {
    match cursor.map(str::trim).filter(|text| !text.is_empty()) {
        None => Ok(0),
        Some(text) => text
            .parse::<i64>()
            .map_err(|_| AppError::invalid("the cursor is the number of rows already received"))
            .map(|offset| offset.max(0)),
    }
}

// ----------------------------------------------------------------- customers

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CustomerQuery {
    /// Free text over the name, code, contact details, site and tower.
    pub query: Option<String>,
    pub state: Option<String>,
    pub currency: Option<String>,
    pub tower: Option<String>,
    pub device_id: Option<String>,
    pub limit: Option<u32>,
    /// The number of rows already received, as a decimal string.
    pub cursor: Option<String>,
}

fn customer_search(customer: &Customer) -> String {
    format!(
        "{} {} {} {} {} {} {} {}",
        customer.name,
        customer.code,
        customer.contact.phone,
        customer.contact.email,
        customer.contact.address,
        customer.site,
        customer.service.tower.clone().unwrap_or_default(),
        customer.tags.join(" ")
    )
    .to_lowercase()
}

pub fn put_customer(store: &Store, customer: &Customer) -> AppResult<()> {
    store.with(|conn| write_customer(conn, customer))
}

fn write_customer(conn: &Connection, customer: &Customer) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO isp_customer (id, code, name, state, currency, site, tower, device_id,
                                   interface, search, created_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET code = excluded.code, name = excluded.name,
             state = excluded.state, currency = excluded.currency, site = excluded.site,
             tower = excluded.tower, device_id = excluded.device_id,
             interface = excluded.interface, search = excluded.search, body = excluded.body",
        params![
            customer.id,
            customer.code,
            customer.name,
            customer.state.as_str(),
            customer.currency.code(),
            customer.site,
            customer.service.tower.clone().unwrap_or_default(),
            customer.service.device_id.clone().unwrap_or_default(),
            customer.service.interface.clone().unwrap_or_default(),
            customer_search(customer),
            customer.created_at,
            to_sql(customer)?,
        ],
    )?;
    Ok(())
}

pub fn customer(store: &Store, id: &str) -> AppResult<Option<Customer>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_customer WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_customer(store: &Store, id: &str) -> AppResult<Customer> {
    customer(store, id)?.ok_or_else(|| AppError::not_found(format!("customer `{id}`")))
}

/// The id of the customer with this account code, if any.
pub fn customer_by_code(store: &Store, code: &str) -> AppResult<Option<String>> {
    if code.trim().is_empty() {
        return Ok(None);
    }
    store.with(|conn| {
        conn.query_row(
            "SELECT id FROM isp_customer WHERE code = ?1",
            params![code.trim()],
            |row| row.get::<_, String>(0),
        )
        .optional()
    })
}

/// Forget a customer's subscriptions. Only for a customer with no ledger at
/// all: an invoice refers to the subscription it charged for.
pub fn delete_subscriptions_of(store: &Store, customer_id: &str) -> AppResult<usize> {
    store.with(|conn| {
        conn.execute(
            "DELETE FROM isp_subscription WHERE customer_id = ?1",
            params![customer_id],
        )
    })
}

pub fn delete_customer(store: &Store, id: &str) -> AppResult<bool> {
    let removed =
        store.with(|conn| conn.execute("DELETE FROM isp_customer WHERE id = ?1", params![id]))?;
    Ok(removed > 0)
}

/// One page of customers, with how many match in all.
pub struct CustomerPage {
    pub customers: Vec<Customer>,
    pub matching: u64,
}

pub fn customers(store: &Store, query: &CustomerQuery) -> AppResult<CustomerPage> {
    let limit = i64::from(query.limit.unwrap_or(200).clamp(1, 2_000));
    let offset = offset_of(query.cursor.as_deref())?;
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(state) = &query.state {
        clauses.push("state = ?".to_string());
        values.push(CustomerState::of(state)?.as_str().to_string().into());
    }
    if let Some(currency) = &query.currency {
        clauses.push("currency = ?".to_string());
        values.push(Currency::of(currency)?.code().to_string().into());
    }
    if let Some(tower) = &query.tower {
        clauses.push("tower = ?".to_string());
        values.push(tower.clone().into());
    }
    if let Some(device_id) = &query.device_id {
        clauses.push("device_id = ?".to_string());
        values.push(device_id.clone().into());
    }
    // Every word of the query must appear somewhere in the searchable text,
    // which is what `util::matches_query` does in memory.
    if let Some(text) = &query.query {
        for word in text.split_whitespace() {
            clauses.push("search LIKE ? ESCAPE '\\'".to_string());
            values.push(
                format!(
                    "%{}%",
                    word.to_lowercase()
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
                .into(),
            );
        }
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let matching: i64 = store.with(|conn| {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM isp_customer{where_clause}"),
            params_from_iter(values.iter()),
            |row| row.get(0),
        )
    })?;
    let mut paged = values.clone();
    paged.push(limit.into());
    paged.push(offset.into());
    let customers: Vec<Customer> = store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT body FROM isp_customer{where_clause}
             ORDER BY name COLLATE NOCASE, id LIMIT ? OFFSET ?"
        ))?;
        statement
            .query_map(params_from_iter(paged.iter()), |row| body_of(row, 0))?
            .collect()
    })?;
    Ok(CustomerPage {
        customers,
        matching: matching.max(0) as u64,
    })
}

/// How many customers there are in each state and currency.
pub fn customer_counts(store: &Store) -> AppResult<Vec<(String, String, u64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT state, currency, COUNT(*) FROM isp_customer GROUP BY state, currency
             ORDER BY state, currency",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?.max(0) as u64,
            ))
        })?;
        rows.collect()
    })
}

/// The customers whose service names this device, for forgetting a device.
pub fn customers_on_device(store: &Store, device_id: &str) -> AppResult<Vec<String>> {
    store.with(|conn| {
        let mut statement =
            conn.prepare_cached("SELECT id FROM isp_customer WHERE device_id = ?1")?;
        let rows = statement.query_map(params![device_id], |row| row.get::<_, String>(0))?;
        rows.collect()
    })
}

// ------------------------------------------------------------- subscriptions

pub fn put_subscription(store: &Store, subscription: &Subscription) -> AppResult<()> {
    store.with(|conn| write_subscription(conn, subscription))
}

fn write_subscription(conn: &Connection, subscription: &Subscription) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO isp_subscription (id, customer_id, tariff_id, state, mbit, currency,
                                       price_minor, started_at, ended_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (id) DO UPDATE SET state = excluded.state, mbit = excluded.mbit,
             currency = excluded.currency, price_minor = excluded.price_minor,
             started_at = excluded.started_at, ended_at = excluded.ended_at,
             body = excluded.body",
        params![
            subscription.id,
            subscription.customer_id,
            subscription.tariff_id,
            serde_json::to_value(subscription.state)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            subscription.mbit,
            subscription.price.currency.code(),
            subscription.price.minor,
            subscription.started_at,
            subscription.ended_at,
            to_sql(subscription)?,
        ],
    )?;
    Ok(())
}

pub fn subscription(store: &Store, id: &str) -> AppResult<Option<Subscription>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_subscription WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_subscription(store: &Store, id: &str) -> AppResult<Subscription> {
    subscription(store, id)?.ok_or_else(|| AppError::not_found(format!("subscription `{id}`")))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SubscriptionQuery {
    pub customer_id: Option<String>,
    pub tariff_id: Option<String>,
    /// `active` or `ended`.
    pub state: Option<String>,
}

pub fn subscriptions(store: &Store, query: &SubscriptionQuery) -> AppResult<Vec<Subscription>> {
    let mut clauses: Vec<&str> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(customer_id) = &query.customer_id {
        clauses.push("customer_id = ?");
        values.push(customer_id.clone().into());
    }
    if let Some(tariff_id) = &query.tariff_id {
        clauses.push("tariff_id = ?");
        values.push(tariff_id.clone().into());
    }
    if let Some(state) = &query.state {
        let state = match state.trim().to_ascii_lowercase().as_str() {
            "active" => "active",
            "ended" => "ended",
            other => {
                return Err(AppError::invalid(format!(
                    "`{other}` is not a subscription state (active, ended)"
                )));
            }
        };
        clauses.push("state = ?");
        values.push(state.to_string().into());
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT body FROM isp_subscription{where_clause} ORDER BY started_at DESC, id"
        ))?;
        statement
            .query_map(params_from_iter(values.iter()), |row| body_of(row, 0))?
            .collect()
    })
}

/// A subscription together with the customer it belongs to — the one query
/// that the capacity, margin and usage reports are all built from.
#[derive(Clone, Debug)]
pub struct Served {
    pub subscription: Subscription,
    pub customer_id: String,
    pub customer_name: String,
    pub customer_state: CustomerState,
    pub currency: Currency,
    pub tower: String,
    pub device_id: String,
    pub interface: String,
}

/// Every active subscription, optionally only those behind one tower.
pub fn served(store: &Store, tower: Option<&str>) -> AppResult<Vec<Served>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match tower {
        Some(tower) => (" AND c.tower = ?", vec![tower.to_string().into()]),
        None => ("", Vec::new()),
    };
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT s.body, c.id, c.name, c.state, c.currency, c.tower, c.device_id, c.interface
             FROM isp_subscription s JOIN isp_customer c ON c.id = s.customer_id
             WHERE s.state = 'active'{filter}
             ORDER BY c.name COLLATE NOCASE, s.started_at"
        ))?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            let currency: String = row.get(4)?;
            Ok(Served {
                subscription: body_of(row, 0)?,
                customer_id: row.get(1)?,
                customer_name: row.get(2)?,
                customer_state: CustomerState::of(&row.get::<_, String>(3)?)
                    .unwrap_or(CustomerState::Active),
                currency: Currency::of(&currency).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::other(err.message)),
                    )
                })?,
                tower: row.get(5)?,
                device_id: row.get(6)?,
                interface: row.get(7)?,
            })
        })?;
        rows.collect()
    })
}

// ------------------------------------------------------------------ invoices

pub fn put_invoice(store: &Store, invoice: &Invoice) -> AppResult<()> {
    store.with(|conn| write_invoice(conn, invoice))
}

fn write_invoice(conn: &Connection, invoice: &Invoice) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO isp_invoice (id, number, customer_id, currency, state, total_minor,
                                  period_start, period_end, issued_at, due_at, created_at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET number = excluded.number, state = excluded.state,
             total_minor = excluded.total_minor, currency = excluded.currency,
             period_start = excluded.period_start, period_end = excluded.period_end,
             issued_at = excluded.issued_at, due_at = excluded.due_at, body = excluded.body",
        params![
            invoice.id,
            invoice.number.clone().unwrap_or_default(),
            invoice.customer_id,
            invoice.currency.code(),
            invoice.state.as_str(),
            invoice.total.minor,
            invoice.period_start,
            invoice.period_end,
            invoice.issued_at,
            invoice.due_at,
            invoice.created_at,
            to_sql(invoice)?,
        ],
    )?;
    Ok(())
}

pub fn invoice(store: &Store, id: &str) -> AppResult<Option<Invoice>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_invoice WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_invoice(store: &Store, id: &str) -> AppResult<Invoice> {
    invoice(store, id)?.ok_or_else(|| AppError::not_found(format!("invoice `{id}`")))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct InvoiceQuery {
    pub customer_id: Option<String>,
    /// `draft`, `issued` or `void`.
    pub state: Option<String>,
    pub currency: Option<String>,
    /// Invoices whose period ends at or after this moment.
    pub since: Option<i64>,
    /// Invoices whose period starts before this moment.
    pub until: Option<i64>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

pub fn invoices(store: &Store, query: &InvoiceQuery) -> AppResult<Vec<Invoice>> {
    let limit = i64::from(query.limit.unwrap_or(100).clamp(1, 2_000));
    let offset = offset_of(query.cursor.as_deref())?;
    let mut clauses: Vec<&str> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(customer_id) = &query.customer_id {
        clauses.push("customer_id = ?");
        values.push(customer_id.clone().into());
    }
    if let Some(state) = &query.state {
        clauses.push("state = ?");
        values.push(InvoiceState::of(state)?.as_str().to_string().into());
    }
    if let Some(currency) = &query.currency {
        clauses.push("currency = ?");
        values.push(Currency::of(currency)?.code().to_string().into());
    }
    if let Some(since) = query.since {
        clauses.push("period_end >= ?");
        values.push(since.into());
    }
    if let Some(until) = query.until {
        clauses.push("period_start < ?");
        values.push(until.into());
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    values.push(limit.into());
    values.push(offset.into());
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT body FROM isp_invoice{where_clause}
             ORDER BY period_start DESC, created_at DESC, id LIMIT ? OFFSET ?"
        ))?;
        statement
            .query_map(params_from_iter(values.iter()), |row| body_of(row, 0))?
            .collect()
    })
}

/// Is there already an invoice for this customer and period that is not void?
/// Billing the same period twice is the mistake this stops.
pub fn invoice_for_period(
    store: &Store,
    customer_id: &str,
    period_start: i64,
    period_end: i64,
) -> AppResult<Option<Invoice>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_invoice
              WHERE customer_id = ?1 AND period_start = ?2 AND period_end = ?3
                AND state <> 'void'
              LIMIT 1",
            params![customer_id, period_start, period_end],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn count_invoices(store: &Store, customer_id: &str) -> AppResult<u64> {
    let count: i64 = store.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM isp_invoice WHERE customer_id = ?1",
            params![customer_id],
            |row| row.get(0),
        )
    })?;
    Ok(count.max(0) as u64)
}

/// How many invoices there are in each state.
pub fn invoice_counts(store: &Store) -> AppResult<Vec<(String, u64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT state, COUNT(*) FROM isp_invoice GROUP BY state ORDER BY state",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?.max(0) as u64,
            ))
        })?;
        rows.collect()
    })
}

/// How many customers could have a usage figure at all: those with a device
/// and an interface, those with only a device, and those with neither.
pub fn service_coverage(store: &Store) -> AppResult<(u32, u32, u32)> {
    store.with(|conn| {
        conn.query_row(
            "SELECT
               COALESCE(SUM(device_id <> '' AND interface <> ''), 0),
               COALESCE(SUM(device_id <> '' AND interface = ''), 0),
               COALESCE(SUM(device_id = ''), 0)
             FROM isp_customer",
            [],
            |row| {
                Ok((
                    u32::try_from(row.get::<_, i64>(0)?).unwrap_or(u32::MAX),
                    u32::try_from(row.get::<_, i64>(1)?).unwrap_or(u32::MAX),
                    u32::try_from(row.get::<_, i64>(2)?).unwrap_or(u32::MAX),
                ))
            },
        )
    })
}

/// What was invoiced in a window, per currency: the currency, the total in
/// minor units, and how many invoices. One query rather than a page of them,
/// because an estate has years of invoices.
pub fn issued_between(store: &Store, since: i64, until: i64) -> AppResult<Vec<(String, i64, u64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT currency, SUM(total_minor), COUNT(*) FROM isp_invoice
              WHERE state = 'issued' AND issued_at >= ?1 AND issued_at < ?2
              GROUP BY currency ORDER BY currency",
        )?;
        let rows = statement.query_map(params![since, until], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?.max(0) as u64,
            ))
        })?;
        rows.collect()
    })
}

/// The next invoice number for a year, allocated under the store's lock so
/// two issues in flight cannot take the same one.
pub fn next_invoice_number(store: &Store, year: i32, prefix: &str) -> AppResult<String> {
    let key = number_key(year);
    store.transaction(|tx| {
        let stored: Option<String> = tx
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
                row.get(0)
            })
            .optional()?;
        let last: i64 = stored
            .as_deref()
            .and_then(|text| serde_json::from_str::<i64>(text).ok())
            .unwrap_or(0);
        let next = last + 1;
        tx.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, next.to_string()],
        )?;
        Ok(format!("{prefix}{year}-{next:04}"))
    })
}

// ------------------------------------------------------------------ payments

pub fn put_payment(store: &Store, payment: &Payment) -> AppResult<()> {
    store.with(|conn| write_payment(conn, payment))
}

fn write_payment(conn: &Connection, payment: &Payment) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO isp_payment (id, customer_id, invoice_id, currency, minor,
                                  settled_currency, settled_minor, state, at, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT (id) DO UPDATE SET invoice_id = excluded.invoice_id,
             settled_currency = excluded.settled_currency,
             settled_minor = excluded.settled_minor, state = excluded.state,
             body = excluded.body",
        params![
            payment.id,
            payment.customer_id,
            payment.invoice_id.clone().unwrap_or_default(),
            payment.amount.currency.code(),
            payment.amount.minor,
            payment
                .settled
                .map(|settled| settled.currency.code())
                .unwrap_or_default(),
            payment.settled.map(|settled| settled.minor).unwrap_or(0),
            serde_json::to_value(payment.state)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            payment.at,
            to_sql(payment)?,
        ],
    )?;
    Ok(())
}

pub fn payment(store: &Store, id: &str) -> AppResult<Option<Payment>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_payment WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_payment(store: &Store, id: &str) -> AppResult<Payment> {
    payment(store, id)?.ok_or_else(|| AppError::not_found(format!("payment `{id}`")))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PaymentQuery {
    pub customer_id: Option<String>,
    pub invoice_id: Option<String>,
    /// Payments taken at or after this moment.
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// `recorded` or `void`; both by default.
    pub state: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

pub fn payments(store: &Store, query: &PaymentQuery) -> AppResult<Vec<Payment>> {
    let limit = i64::from(query.limit.unwrap_or(100).clamp(1, 2_000));
    let offset = offset_of(query.cursor.as_deref())?;
    let mut clauses: Vec<&str> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(customer_id) = &query.customer_id {
        clauses.push("customer_id = ?");
        values.push(customer_id.clone().into());
    }
    if let Some(invoice_id) = &query.invoice_id {
        clauses.push("invoice_id = ?");
        values.push(invoice_id.clone().into());
    }
    if let Some(since) = query.since {
        clauses.push("at >= ?");
        values.push(since.into());
    }
    if let Some(until) = query.until {
        clauses.push("at < ?");
        values.push(until.into());
    }
    if let Some(state) = &query.state {
        let state = match state.trim().to_ascii_lowercase().as_str() {
            "recorded" => "recorded",
            "void" | "voided" => "void",
            other => {
                return Err(AppError::invalid(format!(
                    "`{other}` is not a payment state (recorded, void)"
                )));
            }
        };
        clauses.push("state = ?");
        values.push(state.to_string().into());
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    values.push(limit.into());
    values.push(offset.into());
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT body FROM isp_payment{where_clause} ORDER BY at DESC, id LIMIT ? OFFSET ?"
        ))?;
        statement
            .query_map(params_from_iter(values.iter()), |row| body_of(row, 0))?
            .collect()
    })
}

/// What has been settled against one invoice, in that invoice's currency.
///
/// Only recorded payments whose settled figure is in the invoice's own
/// currency count — and a payment's settled figure was worked out once, at
/// the rate it was taken at, so this sum does not move when a later payment
/// uses a different rate.
pub fn settled_of_invoice(store: &Store, invoice: &Invoice) -> AppResult<Money> {
    let minor: i64 = store.with(|conn| {
        conn.query_row(
            "SELECT COALESCE(SUM(settled_minor), 0) FROM isp_payment
              WHERE invoice_id = ?1 AND state = 'recorded' AND settled_currency = ?2",
            params![invoice.id, invoice.currency.code()],
            |row| row.get(0),
        )
    })?;
    Ok(Money::new(minor, invoice.currency))
}

/// Money received in a window, per currency as it was received.
pub fn received_between(store: &Store, since: i64, until: i64) -> AppResult<Vec<(String, i64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT currency, SUM(minor) FROM isp_payment
              WHERE state = 'recorded' AND at >= ?1 AND at < ?2
              GROUP BY currency ORDER BY currency",
        )?;
        let rows = statement.query_map(params![since, until], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect()
    })
}

/// Payments on account: received, recorded, not against any invoice.
pub fn on_account(store: &Store, customer_id: Option<&str>) -> AppResult<Vec<(String, i64)>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match customer_id {
        Some(id) => (" AND customer_id = ?", vec![id.to_string().into()]),
        None => ("", Vec::new()),
    };
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT currency, SUM(minor) FROM isp_payment
              WHERE state = 'recorded' AND invoice_id = ''{filter}
              GROUP BY currency ORDER BY currency"
        ))?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect()
    })
}

pub fn count_payments(store: &Store, customer_id: &str) -> AppResult<u64> {
    let count: i64 = store.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM isp_payment WHERE customer_id = ?1",
            params![customer_id],
            |row| row.get(0),
        )
    })?;
    Ok(count.max(0) as u64)
}

// ------------------------------------------------------------- credit notes

/// The next credit-note number. Its own sequence, and prefixed `CN-` so that
/// a credit note can never be mistaken for an invoice in a conversation about
/// "number 14".
pub fn next_credit_number(store: &Store, year: i32, prefix: &str) -> AppResult<String> {
    let key = credit_number_key(year);
    store.transaction(|tx| {
        let stored: Option<String> = tx
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
                row.get(0)
            })
            .optional()?;
        let last: i64 = stored
            .as_deref()
            .and_then(|text| serde_json::from_str::<i64>(text).ok())
            .unwrap_or(0);
        let next = last + 1;
        tx.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, next.to_string()],
        )?;
        Ok(format!("{prefix}CN-{year}-{next:04}"))
    })
}

pub fn put_credit(store: &Store, credit: &CreditNote) -> AppResult<()> {
    store.with(|conn| {
        conn.execute(
            "INSERT INTO isp_credit (id, number, customer_id, invoice_id, currency,
                                     minor, state, at, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (id) DO UPDATE SET state = excluded.state,
                 body = excluded.body",
            params![
                credit.id,
                credit.number,
                credit.customer_id,
                credit.invoice_id.clone().unwrap_or_default(),
                credit.amount.currency.code(),
                credit.amount.minor,
                credit.state.as_str(),
                credit.at,
                to_sql(credit)?,
            ],
        )?;
        Ok(())
    })
}

pub fn credit(store: &Store, id: &str) -> AppResult<Option<CreditNote>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_credit WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_credit(store: &Store, id: &str) -> AppResult<CreditNote> {
    credit(store, id)?.ok_or_else(|| AppError::not_found(format!("credit note `{id}`")))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct CreditQuery {
    pub customer_id: Option<String>,
    pub invoice_id: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// `issued` or `void`; both by default.
    pub state: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

pub fn credits(store: &Store, query: &CreditQuery) -> AppResult<Vec<CreditNote>> {
    let limit = i64::from(query.limit.unwrap_or(100).clamp(1, 2_000));
    let offset = offset_of(query.cursor.as_deref())?;
    let mut clauses: Vec<&str> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(customer_id) = &query.customer_id {
        clauses.push("customer_id = ?");
        values.push(customer_id.clone().into());
    }
    if let Some(invoice_id) = &query.invoice_id {
        clauses.push("invoice_id = ?");
        values.push(invoice_id.clone().into());
    }
    if let Some(since) = query.since {
        clauses.push("at >= ?");
        values.push(since.into());
    }
    if let Some(until) = query.until {
        clauses.push("at < ?");
        values.push(until.into());
    }
    if let Some(state) = &query.state {
        let state = match state.trim().to_ascii_lowercase().as_str() {
            "issued" => "issued",
            "void" | "voided" => "void",
            other => {
                return Err(AppError::invalid(format!(
                    "`{other}` is not a credit note state (issued, void)"
                )));
            }
        };
        clauses.push("state = ?");
        values.push(state.to_string().into());
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    values.push(limit.into());
    values.push(offset.into());
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT body FROM isp_credit{where_clause} ORDER BY at DESC, id LIMIT ? OFFSET ?"
        ))?;
        statement
            .query_map(params_from_iter(values.iter()), |row| body_of(row, 0))?
            .collect()
    })
}

/// What has been credited against one invoice, in that invoice's currency.
///
/// A credit note is always written in the customer's billing currency, which
/// is the currency the invoice was raised in, so unlike a payment there is no
/// rate and nothing to have rounded.
pub fn credited_of_invoice(store: &Store, invoice: &Invoice) -> AppResult<Money> {
    let minor: i64 = store.with(|conn| {
        conn.query_row(
            "SELECT COALESCE(SUM(minor), 0) FROM isp_credit
              WHERE invoice_id = ?1 AND state = 'issued' AND currency = ?2",
            params![invoice.id, invoice.currency.code()],
            |row| row.get(0),
        )
    })?;
    Ok(Money::new(minor, invoice.currency))
}

/// Credit notes that stand against the account rather than one invoice.
pub fn credit_on_account(
    store: &Store,
    customer_id: Option<&str>,
) -> AppResult<Vec<(String, i64)>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match customer_id {
        Some(id) => (" AND customer_id = ?", vec![id.to_string().into()]),
        None => ("", Vec::new()),
    };
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT currency, SUM(minor) FROM isp_credit
              WHERE state = 'issued' AND invoice_id = ''{filter}
              GROUP BY currency ORDER BY currency"
        ))?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect()
    })
}

pub fn count_credits(store: &Store, customer_id: &str) -> AppResult<u64> {
    let count: i64 = store.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM isp_credit WHERE customer_id = ?1",
            params![customer_id],
            |row| row.get(0),
        )
    })?;
    Ok(count.max(0) as u64)
}

// ----------------------------------------------------------------- the books

/// One issued invoice that is still short, with what has been settled against
/// it. The arrears report and the customer balance are both this query.
#[derive(Clone, Debug)]
pub struct Shortfall {
    pub invoice_id: String,
    pub number: String,
    pub customer_id: String,
    pub currency: Currency,
    pub total_minor: i64,
    pub paid_minor: i64,
    /// Written off by credit notes, in the invoice's own currency.
    pub credited_minor: i64,
    pub issued_at: Option<i64>,
    pub due_at: Option<i64>,
    pub period_start: i64,
    pub period_end: i64,
}

impl Shortfall {
    pub fn outstanding(&self) -> Money {
        Money::new(
            self.total_minor - self.paid_minor - self.credited_minor,
            self.currency,
        )
    }
}

/// Every issued invoice that is still short, optionally for one customer.
pub fn shortfalls(store: &Store, customer_id: Option<&str>) -> AppResult<Vec<Shortfall>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match customer_id {
        Some(id) => (" AND i.customer_id = ?", vec![id.to_string().into()]),
        None => ("", Vec::new()),
    };
    store.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT * FROM (
               SELECT i.id, i.number, i.customer_id, i.currency, i.total_minor,
                      COALESCE((SELECT SUM(p.settled_minor) FROM isp_payment p
                                 WHERE p.invoice_id = i.id AND p.state = 'recorded'
                                   AND p.settled_currency = i.currency), 0) AS paid_minor,
                      COALESCE((SELECT SUM(c.minor) FROM isp_credit c
                                 WHERE c.invoice_id = i.id AND c.state = 'issued'
                                   AND c.currency = i.currency), 0) AS credited_minor,
                      i.issued_at, i.due_at, i.period_start, i.period_end
                 FROM isp_invoice i
                WHERE i.state = 'issued'{filter}
             ) WHERE total_minor - paid_minor - credited_minor > 0
             ORDER BY due_at, period_start"
        ))?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            let currency: String = row.get(3)?;
            Ok(Shortfall {
                invoice_id: row.get(0)?,
                number: row.get(1)?,
                customer_id: row.get(2)?,
                currency: Currency::of(&currency).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::other(err.message)),
                    )
                })?,
                total_minor: row.get(4)?,
                paid_minor: row.get(5)?,
                credited_minor: row.get(6)?,
                issued_at: row.get(7)?,
                due_at: row.get(8)?,
                period_start: row.get(9)?,
                period_end: row.get(10)?,
            })
        })?;
        rows.collect()
    })
}
