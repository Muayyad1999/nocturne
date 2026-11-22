//! Where the ledger lives: five tables of this feature's own, and two
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
//!
//! **No sum here can overflow in the database.** SQLite's `SUM` raises
//! "integer overflow" when a column of integers adds up to more than 64
//! bits, and that would arrive at the caller as an internal error on every
//! balance and report until the row responsible was found. Amounts are
//! bounded where they are entered ([`crate::money::ENTRY_LIMIT_MINOR`]), and
//! every sum of a money column is still taken in two halves ([`halves`]) and
//! put back together in Rust ([`whole`]), where not fitting is an ordinary
//! refusal that says what is wrong.
//!
//! **A check and the write it guards are one transaction** ([`atomically`]).
//! Every query here runs on anything that is [`Sql`] — the store, for a read
//! on its own, or the connection of a transaction that is already open — so
//! "is this period already billed?" and the insert that follows it cannot
//! have another request land between them.

use nocturne_app::store::Store;
use nocturne_app::{AppError, AppResult};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};
use serde::Deserialize;

use crate::model::{
    CreditNote, Customer, CustomerState, Invoice, InvoiceState, Payment, Subscription,
};
use crate::money::{Currency, Money, overflow};

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

/// Where a query runs: on the store, which takes its lock for that one
/// statement, or on the connection of a transaction that already holds it.
///
/// Every query below takes one of these rather than the store itself, so a
/// read that guards a write can be made inside the same transaction as the
/// write without a second copy of the query.
pub trait Sql {
    fn with<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> AppResult<T>;
}

impl Sql for Store {
    fn with<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> AppResult<T> {
        Store::with(self, work)
    }
}

impl Sql for Connection {
    fn with<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> AppResult<T> {
        Ok(work(self)?)
    }
}

/// `App::store()` hands out the store behind an `Arc`, and a generic
/// parameter does not see through one the way a `&Store` parameter did.
impl Sql for std::sync::Arc<Store> {
    fn with<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> AppResult<T> {
        Store::with(self, work)
    }
}

/// Run a check and the write it guards as one transaction.
///
/// `App::rpc` runs handlers on several threads at once, and a query on the
/// store holds its lock for one statement only. A handler that reads and then
/// writes — is this period already billed, is this still a draft, how much
/// of this invoice is already credited — is therefore one interleaving away
/// from billing a period twice, issuing one draft under two numbers, or
/// writing off more than was charged. Inside this closure nothing else runs:
/// the store has one connection, and `Store::transaction` holds its lock from
/// the first read to the commit. A refusal rolls back whatever the closure
/// had already written, which is also what stops a number allocated for a
/// document that was then refused from being burned.
///
/// The closure is handed the connection and must not reach for the store or
/// the app: that lock is not re-entrant, and a second attempt to take it on
/// the same thread never returns.
pub fn atomically<T>(
    store: &Store,
    work: impl FnOnce(&Connection) -> AppResult<T>,
) -> AppResult<T> {
    let mut refused: Option<AppError> = None;
    let outcome = store.transaction(|tx| {
        work(tx).map_err(|err| {
            refused = Some(err);
            // Any error makes the store roll back. Which one is immaterial:
            // the caller is given the refusal itself, not this.
            rusqlite::Error::InvalidQuery
        })
    });
    match refused {
        Some(err) => Err(err),
        None => outcome,
    }
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

/// What a sum is split at. Each row contributes its quotient by this to one
/// sum and its remainder to another, so neither sum can reach 64 bits
/// before there are thousands of millions of rows.
const HALF: i64 = 1_000_000_000;

/// The two columns that stand in for `SUM(column)`: the sum of the high
/// halves and the sum of the low ones. SQLite's integer `/` and `%` truncate
/// towards zero together, so `high * HALF + low` is the column's value for
/// every row, negative ones included.
fn halves(column: &str) -> String {
    format!("COALESCE(SUM({column} / {HALF}), 0), COALESCE(SUM({column} % {HALF}), 0)")
}

/// Put a sum taken in [`halves`] back together.
///
/// Only rows from before amounts were bounded at entry, or more rows than a
/// ledger has, can make this not fit — and then the answer is a refusal in
/// the ledger's own words, not a database error.
fn whole(high: i64, low: i64) -> AppResult<i64> {
    i64::try_from(i128::from(high) * i128::from(HALF) + i128::from(low))
        .map_err(|_| overflow("adding up"))
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

pub fn put_customer(db: &impl Sql, customer: &Customer) -> AppResult<()> {
    db.with(|conn| write_customer(conn, customer))
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

pub fn customer(db: &impl Sql, id: &str) -> AppResult<Option<Customer>> {
    db.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_customer WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_customer(db: &impl Sql, id: &str) -> AppResult<Customer> {
    customer(db, id)?.ok_or_else(|| AppError::not_found(format!("customer `{id}`")))
}

/// The id of the customer with this account code, if any.
pub fn customer_by_code(db: &impl Sql, code: &str) -> AppResult<Option<String>> {
    if code.trim().is_empty() {
        return Ok(None);
    }
    db.with(|conn| {
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
pub fn delete_subscriptions_of(db: &impl Sql, customer_id: &str) -> AppResult<usize> {
    db.with(|conn| {
        conn.execute(
            "DELETE FROM isp_subscription WHERE customer_id = ?1",
            params![customer_id],
        )
    })
}

pub fn delete_customer(db: &impl Sql, id: &str) -> AppResult<bool> {
    let removed =
        db.with(|conn| conn.execute("DELETE FROM isp_customer WHERE id = ?1", params![id]))?;
    Ok(removed > 0)
}

/// One page of customers, with how many match in all.
pub struct CustomerPage {
    pub customers: Vec<Customer>,
    pub matching: u64,
}

pub fn customers(db: &impl Sql, query: &CustomerQuery) -> AppResult<CustomerPage> {
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
    let matching: i64 = db.with(|conn| {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM isp_customer{where_clause}"),
            params_from_iter(values.iter()),
            |row| row.get(0),
        )
    })?;
    let mut paged = values.clone();
    paged.push(limit.into());
    paged.push(offset.into());
    let customers: Vec<Customer> = db.with(|conn| {
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

/// Every customer in one state, by name — all of them, not a page.
///
/// A billing run is the one reader that must not stop at the end of a page:
/// whoever it does not reach is not billed and not told. [`customers`] is
/// for a screen, and caps what it returns.
pub fn customers_in_state(db: &impl Sql, state: CustomerState) -> AppResult<Vec<Customer>> {
    db.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM isp_customer WHERE state = ?1 ORDER BY name COLLATE NOCASE, id",
        )?;
        statement
            .query_map(params![state.as_str()], |row| body_of(row, 0))?
            .collect()
    })
}

/// How many customers there are in each state and currency.
pub fn customer_counts(db: &impl Sql) -> AppResult<Vec<(String, String, u64)>> {
    db.with(|conn| {
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
pub fn customers_on_device(db: &impl Sql, device_id: &str) -> AppResult<Vec<String>> {
    db.with(|conn| {
        let mut statement =
            conn.prepare_cached("SELECT id FROM isp_customer WHERE device_id = ?1")?;
        let rows = statement.query_map(params![device_id], |row| row.get::<_, String>(0))?;
        rows.collect()
    })
}

// ------------------------------------------------------------- subscriptions

pub fn put_subscription(db: &impl Sql, subscription: &Subscription) -> AppResult<()> {
    db.with(|conn| write_subscription(conn, subscription))
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

pub fn subscription(db: &impl Sql, id: &str) -> AppResult<Option<Subscription>> {
    db.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_subscription WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_subscription(db: &impl Sql, id: &str) -> AppResult<Subscription> {
    subscription(db, id)?.ok_or_else(|| AppError::not_found(format!("subscription `{id}`")))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SubscriptionQuery {
    pub customer_id: Option<String>,
    pub tariff_id: Option<String>,
    /// `active` or `ended`.
    pub state: Option<String>,
}

pub fn subscriptions(db: &impl Sql, query: &SubscriptionQuery) -> AppResult<Vec<Subscription>> {
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
    db.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT body FROM isp_subscription{where_clause} ORDER BY started_at DESC, id"
        ))?;
        statement
            .query_map(params_from_iter(values.iter()), |row| body_of(row, 0))?
            .collect()
    })
}

/// How many subscriptions a customer has ever agreed to, running or ended.
pub fn count_subscriptions(db: &impl Sql, customer_id: &str) -> AppResult<u64> {
    let count: i64 = db.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM isp_subscription WHERE customer_id = ?1",
            params![customer_id],
            |row| row.get(0),
        )
    })?;
    Ok(count.max(0) as u64)
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
pub fn served(db: &impl Sql, tower: Option<&str>) -> AppResult<Vec<Served>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match tower {
        Some(tower) => (" AND c.tower = ?", vec![tower.to_string().into()]),
        None => ("", Vec::new()),
    };
    db.with(|conn| {
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

pub fn put_invoice(db: &impl Sql, invoice: &Invoice) -> AppResult<()> {
    db.with(|conn| write_invoice(conn, invoice))
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

pub fn invoice(db: &impl Sql, id: &str) -> AppResult<Option<Invoice>> {
    db.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_invoice WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_invoice(db: &impl Sql, id: &str) -> AppResult<Invoice> {
    invoice(db, id)?.ok_or_else(|| AppError::not_found(format!("invoice `{id}`")))
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct InvoiceQuery {
    pub customer_id: Option<String>,
    /// `draft`, `issued` or `void`.
    pub state: Option<String>,
    pub currency: Option<String>,
    /// Invoices whose period ends after this moment.
    pub since: Option<i64>,
    /// Invoices whose period starts before this moment. A period is
    /// `[start, end)`, so `since` and `until` set to one month's bounds list
    /// that month's invoices and not also the month before's, which ends
    /// exactly where it begins.
    pub until: Option<i64>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

pub fn invoices(db: &impl Sql, query: &InvoiceQuery) -> AppResult<Vec<Invoice>> {
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
        clauses.push("period_end > ?");
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
    db.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT body FROM isp_invoice{where_clause}
             ORDER BY period_start DESC, created_at DESC, id LIMIT ? OFFSET ?"
        ))?;
        statement
            .query_map(params_from_iter(values.iter()), |row| body_of(row, 0))?
            .collect()
    })
}

/// This customer's invoices that are not void and whose period touches
/// `[from, to)`, oldest first.
///
/// "Has this period been billed already?" starts here. The caller widens
/// the window by as far as two calendars can disagree and then compares
/// days, not milliseconds: an invoice for October written under one offset
/// and a request for October under another are the same month, and their
/// instants are not equal.
pub fn invoices_touching(
    db: &impl Sql,
    customer_id: &str,
    from: i64,
    to: i64,
) -> AppResult<Vec<Invoice>> {
    db.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM isp_invoice
              WHERE customer_id = ?1 AND state <> 'void'
                AND period_end > ?2 AND period_start < ?3
              ORDER BY period_start, created_at, id",
        )?;
        statement
            .query_map(params![customer_id, from, to], |row| body_of(row, 0))?
            .collect()
    })
}

pub fn count_invoices(db: &impl Sql, customer_id: &str) -> AppResult<u64> {
    let count: i64 = db.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM isp_invoice WHERE customer_id = ?1",
            params![customer_id],
            |row| row.get(0),
        )
    })?;
    Ok(count.max(0) as u64)
}

/// How many invoices there are in each state.
pub fn invoice_counts(db: &impl Sql) -> AppResult<Vec<(String, u64)>> {
    db.with(|conn| {
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
pub fn service_coverage(db: &impl Sql) -> AppResult<(u32, u32, u32)> {
    db.with(|conn| {
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
pub fn issued_between(db: &impl Sql, since: i64, until: i64) -> AppResult<Vec<(String, i64, u64)>> {
    let rows: Vec<(String, i64, i64, i64)> = db.with(|conn| {
        let mut statement = conn.prepare_cached(&format!(
            "SELECT currency, {}, COUNT(*) FROM isp_invoice
              WHERE state = 'issued' AND issued_at >= ?1 AND issued_at < ?2
              GROUP BY currency ORDER BY currency",
            halves("total_minor")
        ))?;
        let rows = statement.query_map(params![since, until], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect()
    })?;
    rows.into_iter()
        .map(|(currency, high, low, count)| Ok((currency, whole(high, low)?, count.max(0) as u64)))
        .collect()
}

/// A sum per currency, taken in [`halves`] by `sql` — which selects the
/// currency and then the two halves — and put back together.
fn per_currency(
    db: &impl Sql,
    sql: &str,
    values: &[rusqlite::types::Value],
) -> AppResult<Vec<(String, i64)>> {
    let rows: Vec<(String, i64, i64)> = db.with(|conn| {
        let mut statement = conn.prepare(sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect()
    })?;
    rows.into_iter()
        .map(|(currency, high, low)| Ok((currency, whole(high, low)?)))
        .collect()
}

/// The next number in a per-year sequence kept under `key`.
///
/// It takes the connection of a transaction rather than the store on
/// purpose: the number has to be taken in the same transaction that writes
/// the document carrying it. Taken on its own it would survive a refusal a
/// moment later, and a hole in an invoice sequence is the first thing an
/// auditor asks about.
fn next_in_sequence(conn: &Connection, key: &str) -> AppResult<i64> {
    let stored: Option<String> = conn
        .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
            row.get(0)
        })
        .optional()?;
    let last: i64 = stored
        .as_deref()
        .and_then(|text| serde_json::from_str::<i64>(text).ok())
        .unwrap_or(0);
    let next = last + 1;
    conn.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, next.to_string()],
    )?;
    Ok(next)
}

/// The next invoice number for a year. Call it inside [`atomically`], with
/// the write of the invoice it is for.
pub fn next_invoice_number(conn: &Connection, year: i32, prefix: &str) -> AppResult<String> {
    let next = next_in_sequence(conn, &number_key(year))?;
    Ok(format!("{prefix}{year}-{next:04}"))
}

// ------------------------------------------------------------------ payments

pub fn put_payment(db: &impl Sql, payment: &Payment) -> AppResult<()> {
    db.with(|conn| write_payment(conn, payment))
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

pub fn payment(db: &impl Sql, id: &str) -> AppResult<Option<Payment>> {
    db.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_payment WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_payment(db: &impl Sql, id: &str) -> AppResult<Payment> {
    payment(db, id)?.ok_or_else(|| AppError::not_found(format!("payment `{id}`")))
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

pub fn payments(db: &impl Sql, query: &PaymentQuery) -> AppResult<Vec<Payment>> {
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
    db.with(|conn| {
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
pub fn settled_of_invoice(db: &impl Sql, invoice: &Invoice) -> AppResult<Money> {
    let (high, low): (i64, i64) = db.with(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM isp_payment
                  WHERE invoice_id = ?1 AND state = 'recorded' AND settled_currency = ?2",
                halves("settled_minor")
            ),
            params![invoice.id, invoice.currency.code()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    })?;
    Ok(Money::new(whole(high, low)?, invoice.currency))
}

/// Money received in a window, per currency as it was received.
pub fn received_between(db: &impl Sql, since: i64, until: i64) -> AppResult<Vec<(String, i64)>> {
    per_currency(
        db,
        &format!(
            "SELECT currency, {} FROM isp_payment
              WHERE state = 'recorded' AND at >= ? AND at < ?
              GROUP BY currency ORDER BY currency",
            halves("minor")
        ),
        &[since.into(), until.into()],
    )
}

/// Payments on account: received, recorded, not against any invoice.
pub fn on_account(db: &impl Sql, customer_id: Option<&str>) -> AppResult<Vec<(String, i64)>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match customer_id {
        Some(id) => (" AND customer_id = ?", vec![id.to_string().into()]),
        None => ("", Vec::new()),
    };
    per_currency(
        db,
        &format!(
            "SELECT currency, {} FROM isp_payment
              WHERE state = 'recorded' AND invoice_id = ''{filter}
              GROUP BY currency ORDER BY currency",
            halves("minor")
        ),
        &values,
    )
}

pub fn count_payments(db: &impl Sql, customer_id: &str) -> AppResult<u64> {
    let count: i64 = db.with(|conn| {
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
/// "number 14". Like an invoice number, it is taken inside [`atomically`].
pub fn next_credit_number(conn: &Connection, year: i32, prefix: &str) -> AppResult<String> {
    let next = next_in_sequence(conn, &credit_number_key(year))?;
    Ok(format!("{prefix}CN-{year}-{next:04}"))
}

pub fn put_credit(db: &impl Sql, credit: &CreditNote) -> AppResult<()> {
    db.with(|conn| {
        conn.execute(
            "INSERT INTO isp_credit (id, number, customer_id, invoice_id, currency,
                                     minor, state, at, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT (id) DO UPDATE SET invoice_id = excluded.invoice_id,
                 state = excluded.state, body = excluded.body",
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

pub fn credit(db: &impl Sql, id: &str) -> AppResult<Option<CreditNote>> {
    db.with(|conn| {
        conn.query_row(
            "SELECT body FROM isp_credit WHERE id = ?1",
            params![id],
            |row| body_of(row, 0),
        )
        .optional()
    })
}

pub fn require_credit(db: &impl Sql, id: &str) -> AppResult<CreditNote> {
    credit(db, id)?.ok_or_else(|| AppError::not_found(format!("credit note `{id}`")))
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

pub fn credits(db: &impl Sql, query: &CreditQuery) -> AppResult<Vec<CreditNote>> {
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
    db.with(|conn| {
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
pub fn credited_of_invoice(db: &impl Sql, invoice: &Invoice) -> AppResult<Money> {
    let (high, low): (i64, i64) = db.with(|conn| {
        conn.query_row(
            &format!(
                "SELECT {} FROM isp_credit
                  WHERE invoice_id = ?1 AND state = 'issued' AND currency = ?2",
                halves("minor")
            ),
            params![invoice.id, invoice.currency.code()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    })?;
    Ok(Money::new(whole(high, low)?, invoice.currency))
}

/// Credit notes that stand against the account rather than one invoice.
pub fn credit_on_account(
    db: &impl Sql,
    customer_id: Option<&str>,
) -> AppResult<Vec<(String, i64)>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match customer_id {
        Some(id) => (" AND customer_id = ?", vec![id.to_string().into()]),
        None => ("", Vec::new()),
    };
    per_currency(
        db,
        &format!(
            "SELECT currency, {} FROM isp_credit
              WHERE state = 'issued' AND invoice_id = ''{filter}
              GROUP BY currency ORDER BY currency",
            halves("minor")
        ),
        &values,
    )
}

pub fn count_credits(db: &impl Sql, customer_id: &str) -> AppResult<u64> {
    let count: i64 = db.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM isp_credit WHERE customer_id = ?1",
            params![customer_id],
            |row| row.get(0),
        )
    })?;
    Ok(count.max(0) as u64)
}

// ----------------------------------------------------------------- the books

/// One issued invoice whose settlement does not match its total, with what
/// has been settled against it. Short of it, it is a debt: the arrears report
/// and the customer balance are both [`shortfalls`]. Beyond it, the
/// difference stands in the customer's favour: [`overpayments`].
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
    /// The calendar the invoice was written in, which is what says which
    /// day `due_at` is.
    pub utc_offset_minutes: i32,
}

impl Shortfall {
    /// What is still owed. Negative for a row of [`overpayments`].
    pub fn outstanding(&self) -> AppResult<Money> {
        Money::new(self.total_minor, self.currency)
            .minus(Money::new(self.paid_minor, self.currency))?
            .minus(Money::new(self.credited_minor, self.currency))
    }

    /// What has been settled beyond the total, as a positive figure.
    pub fn overpaid(&self) -> AppResult<Money> {
        Money::new(self.paid_minor, self.currency)
            .plus(Money::new(self.credited_minor, self.currency))?
            .minus(Money::new(self.total_minor, self.currency))
    }
}

/// Every issued invoice that is still short, optionally for one customer.
pub fn shortfalls(db: &impl Sql, customer_id: Option<&str>) -> AppResult<Vec<Shortfall>> {
    unsettled(db, customer_id, ">")
}

/// Every issued invoice settled beyond its total — paid more than it charged,
/// or credited after it was paid — optionally for one customer.
///
/// The mirror of [`shortfalls`], and kept beside it for that reason: an
/// invoice that drops out of one because nothing more is owed must turn up
/// in the other when the customer is owed something instead, or the money
/// would be in no figure at all.
pub fn overpayments(db: &impl Sql, customer_id: Option<&str>) -> AppResult<Vec<Shortfall>> {
    unsettled(db, customer_id, "<")
}

/// Issued invoices whose total less what is paid and credited against them
/// is on one side of nothing (`>` or `<`; never caller-supplied text).
///
/// What is owed is worked out in the same two halves as the sums it is made
/// of, so that no step of it can overflow in the database: `owed_high` is
/// the high half once the low half's carry has been moved into it, which
/// leaves `owed_low` strictly between plus and minus one [`HALF`]. The sign
/// of the whole is then the sign of `owed_high`, or of `owed_low` when that
/// is nothing.
fn unsettled(
    db: &impl Sql,
    customer_id: Option<&str>,
    side: &'static str,
) -> AppResult<Vec<Shortfall>> {
    let (filter, values): (&str, Vec<rusqlite::types::Value>) = match customer_id {
        Some(id) => (" AND i.customer_id = ?", vec![id.to_string().into()]),
        None => ("", Vec::new()),
    };
    struct Row {
        shortfall: Shortfall,
        paid: (i64, i64),
        credited: (i64, i64),
    }
    let rows: Vec<Row> = db.with(|conn| {
        let mut statement = conn.prepare(&format!(
            "SELECT id, number, customer_id, currency, total_minor,
                    paid_high, paid_low, credited_high, credited_low,
                    issued_at, due_at, period_start, period_end, utc_offset_minutes
               FROM (
                 SELECT *,
                        total_minor / {HALF} - paid_high - credited_high
                          + (total_minor % {HALF} - paid_low - credited_low) / {HALF}
                          AS owed_high,
                        (total_minor % {HALF} - paid_low - credited_low) % {HALF} AS owed_low
                   FROM (
                     SELECT i.id, i.number, i.customer_id, i.currency, i.total_minor,
                            COALESCE((SELECT SUM(p.settled_minor / {HALF}) FROM isp_payment p
                                       WHERE p.invoice_id = i.id AND p.state = 'recorded'
                                         AND p.settled_currency = i.currency), 0) AS paid_high,
                            COALESCE((SELECT SUM(p.settled_minor % {HALF}) FROM isp_payment p
                                       WHERE p.invoice_id = i.id AND p.state = 'recorded'
                                         AND p.settled_currency = i.currency), 0) AS paid_low,
                            COALESCE((SELECT SUM(c.minor / {HALF}) FROM isp_credit c
                                       WHERE c.invoice_id = i.id AND c.state = 'issued'
                                         AND c.currency = i.currency), 0) AS credited_high,
                            COALESCE((SELECT SUM(c.minor % {HALF}) FROM isp_credit c
                                       WHERE c.invoice_id = i.id AND c.state = 'issued'
                                         AND c.currency = i.currency), 0) AS credited_low,
                            i.issued_at, i.due_at, i.period_start, i.period_end,
                            COALESCE(json_extract(i.body, '$.utc_offset_minutes'), 0)
                              AS utc_offset_minutes
                       FROM isp_invoice i
                      WHERE i.state = 'issued'{filter}
                   )
               )
              WHERE owed_high {side} 0 OR (owed_high = 0 AND owed_low {side} 0)
              ORDER BY due_at, period_start"
        ))?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            let currency: String = row.get(3)?;
            Ok(Row {
                shortfall: Shortfall {
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
                    paid_minor: 0,
                    credited_minor: 0,
                    issued_at: row.get(9)?,
                    due_at: row.get(10)?,
                    period_start: row.get(11)?,
                    period_end: row.get(12)?,
                    utc_offset_minutes: row.get(13)?,
                },
                paid: (row.get(5)?, row.get(6)?),
                credited: (row.get(7)?, row.get(8)?),
            })
        })?;
        rows.collect()
    })?;
    rows.into_iter()
        .map(|row| {
            Ok(Shortfall {
                paid_minor: whole(row.paid.0, row.paid.1)?,
                credited_minor: whole(row.credited.0, row.credited.1)?,
                ..row.shortfall
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> Store {
        let store = Store::in_memory().expect("an in-memory store");
        init(&store).expect("the tables");
        store
    }

    #[test]
    fn a_refused_document_does_not_take_its_number_with_it() {
        let store = ledger();
        let refused: AppResult<String> = atomically(&store, |conn| {
            let number = next_invoice_number(conn, 2026, "")?;
            assert_eq!(number, "2026-0001");
            // Whatever is found to be wrong after the number was taken.
            Err(AppError::conflict("refused after the number was taken"))
        });
        let err = refused.expect_err("the closure refused");
        assert_eq!(
            (err.code, err.message.as_str()),
            ("conflict", "refused after the number was taken"),
            "the caller is told the refusal itself, not that a transaction failed"
        );
        let next = atomically(&store, |conn| next_invoice_number(conn, 2026, "INV-"))
            .expect("the next issue");
        assert_eq!(
            next, "INV-2026-0001",
            "the refused issue left a hole in the sequence"
        );
        // Invoices and credit notes count separately, and so does each year.
        let numbers = atomically(&store, |conn| {
            Ok([
                next_invoice_number(conn, 2026, "")?,
                next_credit_number(conn, 2026, "")?,
                next_invoice_number(conn, 2027, "")?,
            ])
        })
        .expect("three numbers");
        assert_eq!(numbers, ["2026-0002", "CN-2026-0001", "2027-0001"]);
    }

    #[test]
    fn a_refusal_takes_back_everything_the_transaction_had_written() {
        let store = ledger();
        let customer = Customer {
            id: "c1".to_string(),
            name: "Half Written".to_string(),
            ..Customer::default()
        };
        let refused: AppResult<()> = atomically(&store, |conn| {
            put_customer(conn, &customer)?;
            assert!(
                self::customer(conn, "c1")?.is_some(),
                "inside the transaction the write is visible"
            );
            Err(AppError::invalid("the second half was refused"))
        });
        assert_eq!(refused.expect_err("refused").code, "invalid");
        assert!(
            self::customer(&store, "c1").expect("a read").is_none(),
            "the first half of a refused change was kept"
        );
        // The store is usable afterwards: the lock and the transaction were
        // both released.
        atomically(&store, |conn| put_customer(conn, &customer)).expect("a clean write");
        assert!(self::customer(&store, "c1").expect("a read").is_some());
    }

    fn usd() -> Currency {
        Currency::of("USD").expect("a currency")
    }

    fn issued(store: &Store, id: &str, total: i64) -> Invoice {
        let invoice = Invoice {
            id: id.to_string(),
            customer_id: "c1".to_string(),
            state: InvoiceState::Issued,
            currency: usd(),
            total: Money::new(total, usd()),
            issued_at: Some(1_000),
            ..Invoice::default()
        };
        put_invoice(store, &invoice).expect("an invoice");
        invoice
    }

    /// A payment written straight to the table: a row as it could have been
    /// left by a version that did not bound amounts where they are entered.
    fn paid(store: &Store, id: &str, invoice: Option<&str>, minor: i64) {
        let amount = Money::new(minor, usd());
        put_payment(
            store,
            &Payment {
                id: id.to_string(),
                customer_id: "c1".to_string(),
                invoice_id: invoice.map(str::to_string),
                amount,
                settled: invoice.map(|_| amount),
                at: 1_000,
                ..Payment::default()
            },
        )
        .expect("a payment");
    }

    fn credited(store: &Store, id: &str, invoice: Option<&str>, minor: i64) {
        put_credit(
            store,
            &CreditNote {
                id: id.to_string(),
                number: format!("CN-{id}"),
                customer_id: "c1".to_string(),
                invoice_id: invoice.map(str::to_string),
                amount: Money::new(minor, usd()),
                at: 1_000,
                ..CreditNote::default()
            },
        )
        .expect("a credit note");
    }

    #[test]
    fn a_sum_taken_in_two_halves_is_the_sum_to_the_unit() {
        let store = ledger();
        // Either side of the split, on it, far beyond it, and — though
        // nothing writes one — below nothing, where the two halves have to
        // carry the same sign.
        let amounts: [i64; 9] = [
            1,
            999_999_999,
            1_000_000_000,
            1_000_000_001,
            123_456_789_012_345,
            -1,
            -999_999_999,
            -1_000_000_001,
            -77_000_000_000_000,
        ];
        for (index, minor) in amounts.iter().enumerate() {
            paid(&store, &format!("p{index}"), None, *minor);
            credited(&store, &format!("c{index}"), None, *minor);
        }
        let expected: i64 = amounts.iter().sum();
        assert_eq!(
            on_account(&store, Some("c1")).expect("a sum"),
            [("USD".to_string(), expected)]
        );
        assert_eq!(
            received_between(&store, 0, 2_000).expect("a sum"),
            [("USD".to_string(), expected)]
        );
        assert_eq!(
            credit_on_account(&store, None).expect("a sum"),
            [("USD".to_string(), expected)]
        );
        // Nothing at all is nothing, not a missing row.
        let invoice = issued(&store, "i1", 500);
        assert_eq!(
            settled_of_invoice(&store, &invoice).expect("a sum").minor,
            0
        );
        assert_eq!(
            credited_of_invoice(&store, &invoice).expect("a sum").minor,
            0
        );
    }

    #[test]
    fn a_ledger_that_does_not_fit_in_64_bits_is_refused_in_its_own_words() {
        let store = ledger();
        let invoice = issued(&store, "i1", i64::MAX);
        issued(&store, "i2", 1);
        // On account, and against the invoice: each pair is one more than
        // 64 bits hold.
        paid(&store, "p1", None, i64::MAX);
        paid(&store, "p2", None, 1);
        paid(&store, "p3", Some("i1"), i64::MAX);
        paid(&store, "p4", Some("i1"), 1);
        credited(&store, "c1", None, i64::MAX);
        credited(&store, "c2", None, 1);
        credited(&store, "c3", Some("i1"), i64::MAX);
        credited(&store, "c4", Some("i1"), 1);

        // Even here the question "is anything still owed?" has an answer,
        // because it is asked of the halves: the invoice with more settled
        // against it than 64 bits hold is not a debt, and the other one is.
        let short: Vec<String> = shortfalls(&store, None)
            .expect("the debts")
            .into_iter()
            .map(|row| row.invoice_id)
            .collect();
        assert_eq!(short, ["i2"]);

        let refusals: [(&str, AppResult<()>); 8] = [
            ("on account", on_account(&store, Some("c1")).map(drop)),
            ("received", received_between(&store, 0, 2_000).map(drop)),
            ("issued", issued_between(&store, 0, 2_000).map(drop)),
            ("settled", settled_of_invoice(&store, &invoice).map(drop)),
            ("credited", credited_of_invoice(&store, &invoice).map(drop)),
            (
                "credit on account",
                credit_on_account(&store, None).map(drop),
            ),
            ("overpayments", overpayments(&store, Some("c1")).map(drop)),
            (
                "what is owed",
                Shortfall {
                    invoice_id: String::new(),
                    number: String::new(),
                    customer_id: String::new(),
                    currency: usd(),
                    total_minor: i64::MIN,
                    paid_minor: i64::MAX,
                    credited_minor: 0,
                    issued_at: None,
                    due_at: None,
                    period_start: 0,
                    period_end: 0,
                    utc_offset_minutes: 0,
                }
                .outstanding()
                .map(drop),
            ),
        ];
        for (what, refused) in refusals {
            let err = refused.expect_err(what);
            assert_eq!(
                err.code, "invalid",
                "{what}: a sum that does not fit is not a database error: {err}"
            );
            assert!(err.message.contains("64-bit"), "{what}: {}", err.message);
        }
    }

    #[test]
    fn what_is_owed_keeps_its_sign_when_the_halves_have_to_carry() {
        let store = ledger();
        // (invoice, total, payments, credits) → what is owed.
        let nearly = 999_999_999;
        let cases: [(&str, i64, &[i64], &[i64], i64); 8] = [
            ("a", 2_000_000_000, &[1_999_999_999], &[], 1),
            ("b", 2_000_000_000, &[2_000_000_000], &[], 0),
            ("c", 2_000_000_000, &[2_000_000_001], &[], -1),
            ("d", 5, &[nearly, nearly, nearly], &[], 5 - 3 * nearly),
            ("e", 3_000_000_000, &[nearly, nearly, nearly], &[2], 1),
            ("f", 3_000_000_000, &[nearly, nearly, nearly], &[3], 0),
            ("g", 1, &[], &[], 1),
            (
                "h",
                7_000_000_000_000_000,
                &[1],
                &[6_999_999_999_999_999],
                0,
            ),
        ];
        for (id, total, payments, credits, _) in cases {
            issued(&store, id, total);
            for (index, minor) in payments.iter().enumerate() {
                paid(&store, &format!("{id}p{index}"), Some(id), *minor);
            }
            for (index, minor) in credits.iter().enumerate() {
                credited(&store, &format!("{id}c{index}"), Some(id), *minor);
            }
        }
        let owed = |rows: Vec<Shortfall>| -> Vec<(String, i64)> {
            let mut rows: Vec<(String, i64)> = rows
                .into_iter()
                .map(|row| {
                    let outstanding = row.outstanding().expect("what is owed").minor;
                    (row.invoice_id, outstanding)
                })
                .collect();
            rows.sort();
            rows
        };
        let expected = |short: bool| -> Vec<(String, i64)> {
            cases
                .iter()
                .filter(|case| if short { case.4 > 0 } else { case.4 < 0 })
                .map(|case| (case.0.to_string(), case.4))
                .collect()
        };
        assert_eq!(
            owed(shortfalls(&store, None).expect("the debts")),
            expected(true)
        );
        assert_eq!(
            owed(overpayments(&store, None).expect("the credits")),
            expected(false)
        );
    }
}
