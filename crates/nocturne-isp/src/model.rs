//! The records this feature keeps, and the types its reports are made of.
//!
//! Specification: `docs/dev/features/isp.md`. Two things about these types
//! are load-bearing and worth stating before the first field.
//!
//! **A subscription holds its own copy of what was agreed.** It keeps the
//! tariff's id for provenance, and the megabits, the unit, the unit price,
//! the period price and the period as *copies*. Raising a tariff next year
//! must not rewrite what somebody was charged last year, so nothing in a
//! report ever reads today's tariff to work out an old figure.
//!
//! **An issued invoice is frozen.** Its lines, its total, its currency and
//! its due date are written once, at [`InvoiceState::Issued`], and nothing
//! recalculates them — not a tariff change, not a correction, not a later
//! version of Nocturne. What changes afterwards is only what is *recorded
//! against* it (payments), and the settlement figures a reader works out from
//! those. An invoice that was wrong is voided and a new one issued, so the
//! trail shows what happened.

use nocturne_app::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::money::{Conversion, Currency, Money};

/// Settings section `isp`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Days after it is issued that an invoice falls due, when the caller
    /// does not give a date.
    pub payment_terms_days: u32,
    /// The boundaries, in days overdue, of the arrears report's buckets.
    pub arrears_buckets: Vec<u32>,
    /// What counts as what a customer is using on their serving interface:
    /// `greater` (of the two directions), `in` or `out`. There is no
    /// "correct" answer — which direction is the customer's traffic depends
    /// on which side of the link the interface is — so it is a setting and
    /// every usage figure says which it used.
    pub usage_basis: String,
    /// How far back a usage figure looks when the caller gives no window.
    pub usage_window_hours: u32,
    /// Put in front of an invoice number: `ACME-2026-0001`.
    pub invoice_number_prefix: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            payment_terms_days: 14,
            arrears_buckets: vec![30, 60, 90],
            usage_basis: "greater".to_string(),
            usage_window_hours: 24,
            invoice_number_prefix: String::new(),
        }
    }
}

// ------------------------------------------------------------------ customer

/// What state a customer's *account* is in. None of these does anything to a
/// device: suspending a customer here records a decision, and somebody still
/// has to go and change the router — deliberately, because a billing bug that
/// disconnects a paying customer is worse than a late suspension.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomerState {
    #[default]
    Active,
    Suspended,
    Cancelled,
}

impl CustomerState {
    pub fn as_str(self) -> &'static str {
        match self {
            CustomerState::Active => "active",
            CustomerState::Suspended => "suspended",
            CustomerState::Cancelled => "cancelled",
        }
    }

    pub fn of(text: &str) -> AppResult<CustomerState> {
        match text.trim().to_ascii_lowercase().as_str() {
            "active" => Ok(CustomerState::Active),
            "suspended" => Ok(CustomerState::Suspended),
            "cancelled" | "canceled" => Ok(CustomerState::Cancelled),
            other => Err(AppError::invalid(format!(
                "`{other}` is not a customer state (active, suspended, cancelled)"
            ))),
        }
    }
}

/// How to reach a customer. Personal details: never in anything a model sees.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Contact {
    pub phone: String,
    pub email: String,
    pub address: String,
}

/// The join to the rest of Nocturne: the device and interface that serve this
/// customer, and the tower or upstream they hang off.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Service {
    /// An inventory device id. `None` means nobody has said which device
    /// serves them — which is why they can have no usage figure.
    pub device_id: Option<String>,
    /// The interface name as the device reports it (`ether3`, `Gi0/2`).
    pub interface: Option<String>,
    /// A label shared with the link that feeds them (`Tower A`), which is how
    /// oversubscription and margin know who is behind what.
    pub tower: Option<String>,
    /// Where the service is, in the customer's own words.
    pub location: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Customer {
    pub id: String,
    /// The account number the ISP already uses, if any. Unique when given.
    pub code: String,
    pub name: String,
    pub contact: Contact,
    /// The site in the inventory this customer belongs to, when it matters.
    pub site: String,
    /// The currency this customer is billed in. Every subscription and
    /// invoice of theirs is in it; nothing converts between them.
    pub currency: Currency,
    pub state: CustomerState,
    pub service: Service,
    pub tags: Vec<String>,
    pub notes: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for Customer {
    fn default() -> Self {
        Customer {
            id: String::new(),
            code: String::new(),
            name: String::new(),
            contact: Contact::default(),
            site: String::new(),
            // There is no sensible default currency, so this one exists only
            // to satisfy `Default`; every path that creates a customer takes
            // the currency from the caller and refuses to guess.
            currency: Currency::of("USD").expect("USD is in the currency table"),
            state: CustomerState::Active,
            service: Service::default(),
            tags: Vec::new(),
            notes: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

// -------------------------------------------------------------------- tariff

/// How a tariff's price is quoted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// A price for each megabit: the agreed figure is `price × mbit`.
    PerMbit,
    /// One price for the plan, whatever its megabits.
    #[default]
    Flat,
}

impl Unit {
    pub fn of(text: &str) -> AppResult<Unit> {
        match text.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "per_mbit" | "per_megabit" | "mbit" => Ok(Unit::PerMbit),
            "flat" => Ok(Unit::Flat),
            other => Err(AppError::invalid(format!(
                "`{other}` is not a pricing unit (per_mbit, flat)"
            ))),
        }
    }
}

/// How often a tariff is charged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    #[default]
    Monthly,
    Quarterly,
    Yearly,
}

impl Period {
    pub fn of(text: &str) -> AppResult<Period> {
        match text.trim().to_ascii_lowercase().as_str() {
            "monthly" | "month" => Ok(Period::Monthly),
            "quarterly" | "quarter" => Ok(Period::Quarterly),
            "yearly" | "year" | "annual" | "annually" => Ok(Period::Yearly),
            other => Err(AppError::invalid(format!(
                "`{other}` is not a billing period (monthly, quarterly, yearly)"
            ))),
        }
    }

    pub fn months(self) -> i64 {
        match self {
            Period::Monthly => 1,
            Period::Quarterly => 3,
            Period::Yearly => 12,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Period::Monthly => "monthly",
            Period::Quarterly => "quarterly",
            Period::Yearly => "yearly",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tariff {
    pub id: String,
    pub name: String,
    pub description: String,
    /// What the plan sells, in megabits per second.
    pub mbit: i64,
    pub unit: Unit,
    /// Per megabit for [`Unit::PerMbit`], the whole plan for [`Unit::Flat`].
    pub price: Money,
    pub period: Period,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for Tariff {
    fn default() -> Self {
        Tariff {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            mbit: 0,
            unit: Unit::Flat,
            price: Currency::of("USD")
                .expect("USD is in the currency table")
                .zero(),
            period: Period::Monthly,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// The figure for one whole period at these terms.
pub fn period_price(unit: Unit, unit_price: Money, mbit: i64) -> AppResult<Money> {
    match unit {
        Unit::Flat => Ok(unit_price),
        Unit::PerMbit => {
            if mbit <= 0 {
                return Err(AppError::invalid(
                    "a price per megabit needs the megabits: say how many were sold",
                ));
            }
            unit_price.times(mbit)
        }
    }
}

// -------------------------------------------------------------- subscription

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionState {
    #[default]
    Active,
    Ended,
}

/// A customer on a tariff from a date — holding **its own copy** of the
/// agreed megabits and price, because the tariff will change and this must
/// not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Subscription {
    pub id: String,
    pub customer_id: String,
    /// Where the terms came from. Kept for provenance only: nothing reads the
    /// tariff to work out this subscription's figures, and the tariff may
    /// since have changed or been deleted.
    pub tariff_id: String,
    /// The tariff's name **as it was** when this was agreed.
    pub tariff_name: String,
    /// The agreed megabits.
    pub mbit: i64,
    pub unit: Unit,
    /// The agreed unit price (per megabit, or the flat figure).
    pub unit_price: Money,
    /// The agreed figure for one whole period, worked out once, here.
    pub price: Money,
    pub period: Period,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub state: SubscriptionState,
    /// The subscription this one replaced, when terms were amended.
    pub amends: Option<String>,
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for Subscription {
    fn default() -> Self {
        let usd = Currency::of("USD").expect("USD is in the currency table");
        Subscription {
            id: String::new(),
            customer_id: String::new(),
            tariff_id: String::new(),
            tariff_name: String::new(),
            mbit: 0,
            unit: Unit::Flat,
            unit_price: usd.zero(),
            price: usd.zero(),
            period: Period::Monthly,
            started_at: 0,
            ended_at: None,
            state: SubscriptionState::Active,
            amends: None,
            note: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl Subscription {
    /// Was this subscription in force at any point between `from` and `to`?
    pub fn overlaps(&self, from: i64, to: i64) -> bool {
        let ended = self.ended_at.unwrap_or(i64::MAX);
        self.started_at < to && ended > from
    }

    /// The milliseconds of `[from, to)` this subscription covers.
    pub fn covered_ms(&self, from: i64, to: i64) -> i64 {
        let start = self.started_at.max(from);
        let end = self.ended_at.unwrap_or(i64::MAX).min(to);
        (end - start).max(0)
    }

    /// The agreed figure as a monthly one, for reports that compare plans of
    /// different periods. Rounded, and the caller is told so.
    pub fn monthly_price(&self) -> AppResult<crate::money::Rounded> {
        self.price.scale_by(1, self.period.months())
    }
}

// ------------------------------------------------------------------- invoice

/// What has happened to an invoice as a *document*. Settlement is not in
/// here: whether it is paid is worked out from the payments recorded against
/// it, so that nothing but a recorded payment can make an invoice look paid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceState {
    /// Still being put together. Only a draft may be changed.
    #[default]
    Draft,
    /// Issued to the customer: frozen for good.
    Issued,
    /// Withdrawn, with a reason. A void invoice is never deleted.
    Void,
}

impl InvoiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            InvoiceState::Draft => "draft",
            InvoiceState::Issued => "issued",
            InvoiceState::Void => "void",
        }
    }

    pub fn of(text: &str) -> AppResult<InvoiceState> {
        match text.trim().to_ascii_lowercase().as_str() {
            "draft" => Ok(InvoiceState::Draft),
            "issued" => Ok(InvoiceState::Issued),
            "void" | "voided" => Ok(InvoiceState::Void),
            other => Err(AppError::invalid(format!(
                "`{other}` is not an invoice state (draft, issued, void)"
            ))),
        }
    }
}

/// What a reader should call an invoice: its state plus what the payments and
/// the clock say. Derived at read time, never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    Draft,
    /// Issued, nothing paid, not yet due.
    Issued,
    /// Something paid, something left.
    PartPaid,
    Paid,
    /// Issued, something left and the due date has passed.
    Overdue,
    Void,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    #[default]
    Subscription,
    /// Anything a person added by hand: an installation, a discount, a
    /// correction on a draft.
    Adjustment,
}

/// Why a line is not the whole period's price, and the arithmetic that got
/// it there — so somebody checking the invoice can redo the sum.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proration {
    pub covered_ms: i64,
    pub period_ms: i64,
    /// Whole days, for a human reading the invoice.
    pub covered_days: i64,
    pub period_days: i64,
    /// False when minor units were rounded away.
    pub exact: bool,
    pub basis: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InvoiceLine {
    pub id: String,
    pub kind: LineKind,
    pub description: String,
    pub subscription_id: Option<String>,
    /// The tariff's name as the subscription recorded it.
    pub tariff_name: Option<String>,
    pub mbit: Option<i64>,
    pub unit: Option<Unit>,
    pub unit_price: Option<Money>,
    /// What one whole period at these terms would have cost.
    pub full_period_price: Option<Money>,
    pub period_start: Option<i64>,
    pub period_end: Option<i64>,
    pub prorated: Option<Proration>,
    /// What this line charges. Always in the invoice's currency.
    pub amount: Money,
}

impl Default for InvoiceLine {
    fn default() -> Self {
        InvoiceLine {
            id: String::new(),
            kind: LineKind::Adjustment,
            description: String::new(),
            subscription_id: None,
            tariff_name: None,
            mbit: None,
            unit: None,
            unit_price: None,
            full_period_price: None,
            period_start: None,
            period_end: None,
            prorated: None,
            amount: Currency::of("USD")
                .expect("USD is in the currency table")
                .zero(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Invoice {
    pub id: String,
    /// Allocated when it is issued, never before, never reused.
    pub number: Option<String>,
    pub customer_id: String,
    /// The customer's name **as it was** when the invoice was issued: an
    /// invoice is a document that has to stay reproducible. Personal detail —
    /// it is not in anything a model sees.
    pub customer_name: String,
    pub currency: Currency,
    pub state: InvoiceState,
    pub period_start: i64,
    pub period_end: i64,
    pub lines: Vec<InvoiceLine>,
    pub total: Money,
    pub issued_at: Option<i64>,
    pub due_at: Option<i64>,
    pub voided_at: Option<i64>,
    pub void_reason: String,
    /// The invoice issued in this one's place.
    pub replaced_by: Option<String>,
    /// The invoice this one replaces.
    pub replaces: Option<String>,
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for Invoice {
    fn default() -> Self {
        Invoice {
            id: String::new(),
            number: None,
            customer_id: String::new(),
            customer_name: String::new(),
            currency: Currency::of("USD").expect("USD is in the currency table"),
            state: InvoiceState::Draft,
            period_start: 0,
            period_end: 0,
            lines: Vec::new(),
            total: Currency::of("USD")
                .expect("USD is in the currency table")
                .zero(),
            issued_at: None,
            due_at: None,
            voided_at: None,
            void_reason: String::new(),
            replaced_by: None,
            replaces: None,
            note: String::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl Invoice {
    /// The sum of the lines. Used when a draft is built or changed — never on
    /// an issued invoice, whose `total` is the figure that was issued.
    pub fn sum_lines(&self) -> AppResult<Money> {
        let mut total = Money::zero(self.currency);
        for line in &self.lines {
            if line.amount.currency != self.currency {
                return Err(AppError::invalid(format!(
                    "line `{}` is in {} and the invoice is in {}: an invoice is in one currency",
                    line.description, line.amount.currency, self.currency
                )));
            }
            total = total.plus(line.amount)?;
        }
        Ok(total)
    }
}

/// An invoice with what the payments say about it. The invoice half is
/// untouched; everything derived is beside it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InvoiceView {
    #[serde(flatten)]
    pub invoice: Invoice,
    pub status: InvoiceStatus,
    /// What has been recorded against it, in its own currency.
    pub paid: Money,
    pub outstanding: Money,
    /// Days past its due date, when it is issued, due and short.
    pub days_overdue: Option<i64>,
    /// The payments recorded against it, newest first.
    pub payments: Vec<Payment>,
}

// ------------------------------------------------------------------- payment

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentState {
    #[default]
    Recorded,
    /// Recorded in error and withdrawn, with a reason. Never deleted.
    Void,
}

/// What was actually received.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Payment {
    pub id: String,
    pub customer_id: String,
    /// The invoice it settles. `None` is a payment on account: real money,
    /// not yet against anything.
    pub invoice_id: Option<String>,
    /// The amount and currency that were handed over.
    pub amount: Money,
    /// The rate it was taken at, when the customer paid in a currency other
    /// than the one they were invoiced in. Stored, never looked up again.
    pub conversion: Option<Conversion>,
    /// What this payment settles, in the invoice's currency — worked out once,
    /// when it was taken, from the rate above. A receipt from March still
    /// reconciles in November because this figure is not recomputed.
    pub settled: Option<Money>,
    /// False when the conversion rounded minor units away.
    pub settled_exact: bool,
    /// Cash, bank transfer, a mobile wallet — the ISP's own words.
    pub method: String,
    pub reference: String,
    pub note: String,
    /// When the money changed hands.
    pub at: i64,
    pub state: PaymentState,
    pub void_reason: String,
    pub voided_at: Option<i64>,
    pub recorded_at: i64,
}

impl Default for Payment {
    fn default() -> Self {
        Payment {
            id: String::new(),
            customer_id: String::new(),
            invoice_id: None,
            amount: Currency::of("USD")
                .expect("USD is in the currency table")
                .zero(),
            conversion: None,
            settled: None,
            settled_exact: true,
            method: String::new(),
            reference: String::new(),
            note: String::new(),
            at: 0,
            state: PaymentState::Recorded,
            void_reason: String::new(),
            voided_at: None,
            recorded_at: 0,
        }
    }
}

// ---------------------------------------------------------------------- link

/// An upstream link or a tower's feed: what it can carry, and what the
/// bandwidth on it costs wholesale.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The device and interface it is, when Nocturne can see it — that is
    /// what makes a measured figure possible beside the sold one.
    pub device_id: Option<String>,
    pub interface: Option<String>,
    /// The label the customers behind it carry in `service.tower`.
    pub tower: Option<String>,
    /// What it can carry, in megabits per second, as somebody wrote it down.
    pub capacity_mbit: i64,
    /// The wholesale cost of one megabit for one `cost_period`.
    pub cost_per_mbit: Option<Money>,
    pub cost_period: Period,
    pub created_at: i64,
    pub updated_at: i64,
}
