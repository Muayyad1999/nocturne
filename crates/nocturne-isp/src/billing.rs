//! Invoices and payments: the part that has to be right.
//!
//! Three rules from `docs/dev/features/isp.md` are implemented here rather
//! than hoped for:
//!
//! 1. **An issued invoice is never recalculated.** [`issue`] writes the
//!    lines, the total, the currency and the due date once. After that the
//!    only writes to an invoice are [`void`] (with a reason) and the link to
//!    the invoice that replaced it. Nothing in this module recomputes the
//!    total of an issued invoice, and nothing reads a tariff to do it.
//! 2. **Nothing marks an invoice paid except a recorded payment.** There is
//!    no "paid" flag. [`InvoiceStatus`] is worked out when somebody reads the
//!    invoice, from the payments recorded against it, so there is no second
//!    copy of the truth to drift.
//! 3. **A payment keeps the rate it was taken at.** When the money came in a
//!    different currency from the invoice, the rate is stored on the payment
//!    and what it settles is worked out once, then. A receipt from March
//!    still reconciles in November, however the rate has moved since.

use nocturne_app::{App, AppError, AppResult, util};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::model::{
    CreditNote, CreditState, Customer, Invoice, InvoiceLine, InvoiceState, InvoiceStatus,
    InvoiceView, LineKind, Payment, PaymentState, Proration, Settings,
};
use crate::money::{Conversion, ConversionInput, Money, Totals};

const DAY_MS: i64 = 86_400_000;

/// Whole days, to the nearest day — so an hour of summer time does not turn
/// a month into "30 days and 23 hours".
fn days_of(ms: i64) -> i64 {
    (ms + DAY_MS / 2) / DAY_MS
}

/// Does a window of this length look like one whole period of this kind?
///
/// A calendar month is 28 to 31 days and nothing here invents an average one:
/// a window that is not one period of a subscription's kind produces no line
/// for it and says why, rather than charging a figure nobody agreed.
fn window_is_one_period(period: crate::model::Period, window_ms: i64) -> bool {
    let days = days_of(window_ms);
    match period {
        crate::model::Period::Monthly => (28..=31).contains(&days),
        crate::model::Period::Quarterly => (89..=92).contains(&days),
        crate::model::Period::Yearly => (365..=366).contains(&days),
    }
}

/// A subscription that was deliberately not charged, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Skipped {
    pub subscription_id: String,
    pub tariff_name: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct DraftedInvoice {
    pub invoice: Invoice,
    /// Subscriptions this invoice does not charge for, each with its reason.
    /// They are not zeroes: nothing was billed and the draft says so.
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<String>,
}

fn require_period(period_start: i64, period_end: i64) -> AppResult<i64> {
    if period_end <= period_start {
        return Err(AppError::invalid(
            "a billing period ends after it starts: give `period_start` and `period_end` in \
             milliseconds since the epoch",
        ));
    }
    Ok(period_end - period_start)
}

/// Build a draft invoice for one customer and one period, out of what their
/// subscriptions say they agreed to — not out of the tariffs — and store it.
pub fn draft(
    app: &App,
    customer: &Customer,
    period_start: i64,
    period_end: i64,
    allow_empty: bool,
) -> AppResult<DraftedInvoice> {
    let drafted = build(app, customer, period_start, period_end, allow_empty)?;
    db::put_invoice(app.store(), &drafted.invoice)?;
    Ok(drafted)
}

/// Work out what a draft invoice would say, without writing anything. This is
/// what `isp.invoice.run` previews with: a preview that guessed, or that had
/// to write to find out, would be worth nothing.
pub fn build(
    app: &App,
    customer: &Customer,
    period_start: i64,
    period_end: i64,
    allow_empty: bool,
) -> AppResult<DraftedInvoice> {
    let store = app.store();
    let now = util::now_ms();
    let window_ms = require_period(period_start, period_end)?;
    if let Some(existing) = db::invoice_for_period(store, &customer.id, period_start, period_end)? {
        return Err(AppError::conflict(format!(
            "{} already has {} for that period ({}). Billing a period twice is the mistake this \
             refuses; void that invoice first if it is wrong.",
            customer.name,
            existing
                .number
                .clone()
                .unwrap_or_else(|| format!("a draft (`{}`)", existing.id)),
            existing.state.as_str()
        )));
    }

    let subscriptions = db::subscriptions(
        store,
        &db::SubscriptionQuery {
            customer_id: Some(customer.id.clone()),
            ..db::SubscriptionQuery::default()
        },
    )?;
    let mut lines: Vec<InvoiceLine> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for subscription in &subscriptions {
        if !subscription.overlaps(period_start, period_end) {
            continue;
        }
        if subscription.price.currency != customer.currency {
            // Cannot happen through this crate's own writes; if it ever does,
            // the invoice is not quietly made wrong.
            skipped.push(Skipped {
                subscription_id: subscription.id.clone(),
                tariff_name: subscription.tariff_name.clone(),
                reason: format!(
                    "agreed in {} and the customer is billed in {}; nothing here converts between \
                     them",
                    subscription.price.currency, customer.currency
                ),
            });
            continue;
        }
        if !window_is_one_period(subscription.period, window_ms) {
            skipped.push(Skipped {
                subscription_id: subscription.id.clone(),
                tariff_name: subscription.tariff_name.clone(),
                reason: format!(
                    "agreed {} and this invoice covers {} days: nothing here works out what a \
                     part of a {} period costs on its own. Invoice one {} period, or add the \
                     figure by hand as an adjustment.",
                    subscription.period.as_str(),
                    days_of(window_ms),
                    subscription.period.as_str(),
                    subscription.period.as_str()
                ),
            });
            continue;
        }
        let covered_ms = subscription.covered_ms(period_start, period_end);
        if covered_ms <= 0 {
            continue;
        }
        let (amount, proration) = if covered_ms >= window_ms {
            (subscription.price, None)
        } else {
            let scaled = subscription.price.scale_by(covered_ms, window_ms)?;
            (
                scaled.amount,
                Some(Proration {
                    covered_ms,
                    period_ms: window_ms,
                    covered_days: days_of(covered_ms),
                    period_days: days_of(window_ms),
                    exact: scaled.exact,
                    basis: format!(
                        "{} of the period's {} milliseconds, at the agreed {} per {}",
                        covered_ms,
                        window_ms,
                        subscription.price.text(),
                        subscription.period.as_str()
                    ),
                }),
            )
        };
        lines.push(InvoiceLine {
            id: util::new_id(),
            kind: LineKind::Subscription,
            description: format!(
                "{} — {} Mbit/s{}",
                subscription.tariff_name,
                subscription.mbit,
                if proration.is_some() {
                    " (part period)"
                } else {
                    ""
                }
            ),
            subscription_id: Some(subscription.id.clone()),
            tariff_name: Some(subscription.tariff_name.clone()),
            mbit: Some(subscription.mbit),
            unit: Some(subscription.unit),
            unit_price: Some(subscription.unit_price),
            full_period_price: Some(subscription.price),
            period_start: Some(subscription.started_at.max(period_start)),
            period_end: Some(subscription.ended_at.unwrap_or(period_end).min(period_end)),
            prorated: proration,
            amount,
        });
    }
    if lines.is_empty() && !allow_empty {
        return Err(AppError::invalid(format!(
            "{} has nothing to bill for that period: {} subscription(s) on the account, none of \
             them in force in it{}",
            customer.name,
            subscriptions.len(),
            if skipped.is_empty() {
                String::new()
            } else {
                format!(" ({} skipped, each with a reason)", skipped.len())
            }
        )));
    }
    if !skipped.is_empty() {
        warnings.push(format!(
            "{} subscription(s) were not charged on this invoice; each says why",
            skipped.len()
        ));
    }
    let mut invoice = Invoice {
        id: util::new_id(),
        number: None,
        customer_id: customer.id.clone(),
        customer_name: customer.name.clone(),
        currency: customer.currency,
        state: InvoiceState::Draft,
        period_start,
        period_end,
        lines,
        total: Money::zero(customer.currency),
        created_at: now,
        updated_at: now,
        ..Invoice::default()
    };
    invoice.total = invoice.sum_lines()?;
    Ok(DraftedInvoice {
        invoice,
        skipped,
        warnings,
    })
}

/// A line somebody adds by hand to a draft: an installation, a discount, a
/// correction.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineInput {
    pub invoice_id: String,
    pub description: String,
    pub amount: Money,
}

pub fn add_line(app: &App, input: LineInput) -> AppResult<Invoice> {
    let store = app.store();
    let mut invoice = db::require_invoice(store, &input.invoice_id)?;
    require_draft(&invoice, "add a line to")?;
    if input.description.trim().is_empty() {
        return Err(AppError::invalid("a line needs a description"));
    }
    if input.amount.currency != invoice.currency {
        return Err(AppError::invalid(format!(
            "this invoice is in {} and the line is in {}: an invoice is in one currency",
            invoice.currency, input.amount.currency
        )));
    }
    invoice.lines.push(InvoiceLine {
        id: util::new_id(),
        kind: LineKind::Adjustment,
        description: input.description.trim().to_string(),
        amount: input.amount,
        ..InvoiceLine::default()
    });
    invoice.total = invoice.sum_lines()?;
    invoice.updated_at = util::now_ms();
    db::put_invoice(store, &invoice)?;
    Ok(invoice)
}

pub fn remove_line(app: &App, invoice_id: &str, line_id: &str) -> AppResult<Invoice> {
    let store = app.store();
    let mut invoice = db::require_invoice(store, invoice_id)?;
    require_draft(&invoice, "remove a line from")?;
    let before = invoice.lines.len();
    invoice.lines.retain(|line| line.id != line_id);
    if invoice.lines.len() == before {
        return Err(AppError::not_found(format!("line `{line_id}`")));
    }
    invoice.total = invoice.sum_lines()?;
    invoice.updated_at = util::now_ms();
    db::put_invoice(store, &invoice)?;
    Ok(invoice)
}

fn require_draft(invoice: &Invoice, what: &str) -> AppResult<()> {
    match invoice.state {
        InvoiceState::Draft => Ok(()),
        InvoiceState::Issued => Err(AppError::conflict(format!(
            "invoice {} has been issued, so nothing may {what} it. An issued invoice is never \
             recalculated: if it is wrong, void it and issue a new one, so the trail shows what \
             happened.",
            invoice.number.clone().unwrap_or_else(|| invoice.id.clone())
        ))),
        InvoiceState::Void => Err(AppError::conflict(format!(
            "invoice {} is void{}",
            invoice.number.clone().unwrap_or_else(|| invoice.id.clone()),
            if invoice.void_reason.is_empty() {
                String::new()
            } else {
                format!(" ({})", invoice.void_reason)
            }
        ))),
    }
}

/// Freeze a draft and give it its number. After this the figures are history.
///
/// `issued_at` is for entering an invoice that already exists on paper — an
/// ISP migrating its books into Nocturne has a year of them, and refusing
/// anything with a date in the past would mean typing them in as if they
/// were new. It may not be in the future: an invoice nobody has sent yet is
/// a draft.
pub fn issue(
    app: &App,
    settings: &Settings,
    id: &str,
    due_at: Option<i64>,
    issued_at: Option<i64>,
) -> AppResult<Invoice> {
    let store = app.store();
    let now = util::now_ms();
    let mut invoice = db::require_invoice(store, id)?;
    require_draft(&invoice, "issue")?;
    if invoice.lines.is_empty() {
        return Err(AppError::invalid(
            "an invoice with no lines charges nothing: add what it is for, or delete the draft",
        ));
    }
    // The last arithmetic this invoice will ever have done to it.
    invoice.total = invoice.sum_lines()?;
    let issued = issued_at.unwrap_or(now);
    if issued > now {
        return Err(AppError::invalid(
            "an invoice cannot be issued in the future: until it goes out it is a draft",
        ));
    }
    let due = due_at.unwrap_or(issued + i64::from(settings.payment_terms_days) * DAY_MS);
    if due < issued {
        return Err(AppError::invalid(
            "an invoice cannot fall due before it is issued",
        ));
    }
    let year = chrono::DateTime::from_timestamp_millis(issued)
        .map(|moment| moment.format("%Y").to_string())
        .and_then(|text| text.parse::<i32>().ok())
        .ok_or_else(|| AppError::internal("that moment is outside any calendar year"))?;
    invoice.number = Some(db::next_invoice_number(
        store,
        year,
        &settings.invoice_number_prefix,
    )?);
    invoice.state = InvoiceState::Issued;
    invoice.issued_at = Some(issued);
    invoice.due_at = Some(due);
    invoice.updated_at = now;
    db::put_invoice(store, &invoice)?;
    Ok(invoice)
}

/// Withdraw an invoice, with a reason. The document stays.
pub fn void(app: &App, id: &str, reason: &str) -> AppResult<Invoice> {
    let store = app.store();
    let mut invoice = db::require_invoice(store, id)?;
    if reason.trim().is_empty() {
        return Err(AppError::invalid(
            "voiding an invoice needs a reason: it is what the trail will say happened",
        ));
    }
    if invoice.state == InvoiceState::Void {
        return Err(AppError::conflict("that invoice is already void"));
    }
    let settled = db::payments(
        store,
        &db::PaymentQuery {
            invoice_id: Some(invoice.id.clone()),
            state: Some("recorded".to_string()),
            ..db::PaymentQuery::default()
        },
    )?;
    if !settled.is_empty() {
        return Err(AppError::conflict(format!(
            "{} payment(s) are recorded against invoice {}, and voiding it would hide money that \
             was actually received. Void the payment first if it was recorded in error.",
            settled.len(),
            invoice.number.clone().unwrap_or_else(|| invoice.id.clone())
        )));
    }
    let now = util::now_ms();
    invoice.state = InvoiceState::Void;
    invoice.voided_at = Some(now);
    invoice.void_reason = reason.trim().to_string();
    invoice.updated_at = now;
    db::put_invoice(store, &invoice)?;
    Ok(invoice)
}

#[derive(Debug, Serialize)]
pub struct Reissued {
    pub voided: Invoice,
    pub draft: Invoice,
    pub note: String,
}

/// Void an invoice and start a draft that replaces it, carrying its lines
/// over so the correction can be made before it is issued.
pub fn reissue(app: &App, id: &str, reason: &str) -> AppResult<Reissued> {
    let store = app.store();
    let now = util::now_ms();
    let original = db::require_invoice(store, id)?;
    let mut voided = void(app, id, reason)?;
    let mut draft = Invoice {
        id: util::new_id(),
        number: None,
        customer_id: original.customer_id.clone(),
        customer_name: original.customer_name.clone(),
        currency: original.currency,
        state: InvoiceState::Draft,
        period_start: original.period_start,
        period_end: original.period_end,
        // The lines are copied, with new ids: the original's are untouched.
        lines: original
            .lines
            .iter()
            .map(|line| InvoiceLine {
                id: util::new_id(),
                ..line.clone()
            })
            .collect(),
        total: original.total,
        replaces: Some(original.id.clone()),
        note: format!(
            "replaces {}{}",
            original
                .number
                .clone()
                .unwrap_or_else(|| original.id.clone()),
            if reason.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", reason.trim())
            }
        ),
        created_at: now,
        updated_at: now,
        ..Invoice::default()
    };
    draft.total = draft.sum_lines()?;
    db::put_invoice(store, &draft)?;
    voided.replaced_by = Some(draft.id.clone());
    voided.updated_at = now;
    db::put_invoice(store, &voided)?;
    Ok(Reissued {
        voided,
        draft,
        note: "the original is void and still readable; the replacement is a draft, so the \
               correction can be made before anybody is asked to pay it"
            .to_string(),
    })
}

// ------------------------------------------------------------------ payments

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaymentInput {
    pub customer_id: String,
    /// The invoice it settles. Leave it out for a payment on account.
    #[serde(default)]
    pub invoice_id: Option<String>,
    pub amount: Money,
    /// Cash, bank transfer, a mobile wallet — the ISP's own words.
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub note: String,
    /// When the money changed hands; now by default.
    #[serde(default)]
    pub at: Option<i64>,
    /// Required when the money is in a currency other than the invoice's.
    #[serde(default)]
    pub conversion: Option<ConversionInput>,
}

#[derive(Debug, Serialize)]
pub struct RecordedPayment {
    pub payment: Payment,
    /// The invoice as it now reads, when the payment settles one.
    pub invoice: Option<InvoiceView>,
    pub warnings: Vec<String>,
}

pub fn record_payment(app: &App, input: PaymentInput) -> AppResult<RecordedPayment> {
    let store = app.store();
    let now = util::now_ms();
    let at = input.at.unwrap_or(now);
    let customer = db::require_customer(store, input.customer_id.trim())?;
    if input.amount.minor <= 0 {
        return Err(AppError::invalid(
            "a payment is what was received, so it is more than nothing. A correction is a void \
             of the payment that was wrong, not a negative one.",
        ));
    }
    if input.method.trim().is_empty() {
        return Err(AppError::invalid(
            "say how the money came in (cash, bank transfer, a wallet): a payment with no method \
             cannot be reconciled against anything",
        ));
    }
    let mut warnings: Vec<String> = Vec::new();
    let invoice = match input
        .invoice_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Some(db::require_invoice(store, id)?),
        None => None,
    };

    let (conversion, settled, settled_exact) = match &invoice {
        None => {
            if input.conversion.is_some() {
                return Err(AppError::invalid(
                    "a rate belongs to a payment that settles an invoice in another currency; \
                     this one is on account, so it is simply the amount that was received",
                ));
            }
            (None, None, true)
        }
        Some(invoice) => {
            if invoice.customer_id != customer.id {
                return Err(AppError::invalid(format!(
                    "invoice {} belongs to another customer",
                    invoice.number.clone().unwrap_or_else(|| invoice.id.clone())
                )));
            }
            match invoice.state {
                InvoiceState::Issued => {}
                InvoiceState::Draft => {
                    return Err(AppError::conflict(
                        "that invoice is still a draft: issue it before taking money against it, \
                         or record the payment on account",
                    ));
                }
                InvoiceState::Void => {
                    return Err(AppError::conflict(format!(
                        "invoice {} is void; record the payment against the invoice that replaced \
                         it, or on account",
                        invoice.number.clone().unwrap_or_else(|| invoice.id.clone())
                    )));
                }
            }
            if input.amount.currency == invoice.currency {
                if input.conversion.is_some() {
                    return Err(AppError::invalid(format!(
                        "this payment and invoice {} are both in {}: there is nothing to convert, \
                         so no rate is recorded",
                        invoice.number.clone().unwrap_or_else(|| invoice.id.clone()),
                        invoice.currency
                    )));
                }
                (None, Some(input.amount), true)
            } else {
                let conversion = input
                    .conversion
                    .as_ref()
                    .ok_or_else(|| {
                        AppError::invalid(format!(
                            "this payment is in {} and the invoice is in {}. Enter the rate you \
                             actually got — there is no rate feed and nothing here will guess one \
                             — as `conversion: {{from, to, rate}}`, where `rate` is whole minor \
                             units of `to` for one `from`.",
                            input.amount.currency, invoice.currency
                        ))
                    })?
                    .resolve(at)?;
                if !conversion.relates(input.amount.currency, invoice.currency) {
                    return Err(AppError::invalid(format!(
                        "that rate is between {} and {}, and this payment settles {} against an \
                         invoice in {}",
                        conversion.from, conversion.to, input.amount.currency, invoice.currency
                    )));
                }
                let applied = conversion.apply(input.amount)?;
                if !applied.exact {
                    warnings.push(format!(
                        "at that rate {} is {} to the nearest minor unit, so the invoice is \
                         settled by a figure that was rounded; the rate and the amount received \
                         are both recorded",
                        input.amount.text(),
                        applied.amount.text()
                    ));
                }
                (Some(conversion), Some(applied.amount), applied.exact)
            }
        }
    };

    let payment = Payment {
        id: util::new_id(),
        customer_id: customer.id.clone(),
        invoice_id: invoice.as_ref().map(|invoice| invoice.id.clone()),
        amount: input.amount,
        conversion,
        settled,
        settled_exact,
        method: input.method.trim().to_string(),
        reference: input.reference.trim().to_string(),
        note: input.note,
        at,
        state: PaymentState::Recorded,
        void_reason: String::new(),
        voided_at: None,
        recorded_at: now,
    };
    db::put_payment(store, &payment)?;

    let view = match invoice {
        Some(invoice) => {
            let view = invoice_view(app, invoice, now)?;
            if view.outstanding.is_negative() {
                warnings.push(format!(
                    "invoice {} is now overpaid by {}; the extra stays recorded against it rather \
                     than being moved anywhere on its own",
                    view.invoice
                        .number
                        .clone()
                        .unwrap_or_else(|| view.invoice.id.clone()),
                    Money::new(-view.outstanding.minor, view.outstanding.currency).text()
                ));
            }
            Some(view)
        }
        None => None,
    };
    Ok(RecordedPayment {
        payment,
        invoice: view,
        warnings,
    })
}

pub fn void_payment(app: &App, id: &str, reason: &str) -> AppResult<Payment> {
    let store = app.store();
    let mut payment = db::require_payment(store, id)?;
    if reason.trim().is_empty() {
        return Err(AppError::invalid(
            "voiding a payment needs a reason: money that was recorded and then withdrawn has to \
             say why",
        ));
    }
    if payment.state == PaymentState::Void {
        return Err(AppError::conflict("that payment is already void"));
    }
    payment.state = PaymentState::Void;
    payment.void_reason = reason.trim().to_string();
    payment.voided_at = Some(util::now_ms());
    db::put_payment(store, &payment)?;
    Ok(payment)
}

// ----------------------------------------------------------- credit notes

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreditInput {
    pub customer_id: String,
    /// The invoice to credit. Leave it out to credit the account, which is
    /// what to do when what is being made good is not one invoice.
    #[serde(default)]
    pub invoice_id: Option<String>,
    pub amount: Money,
    pub reason: String,
    #[serde(default)]
    pub note: String,
    /// When it was agreed; now by default.
    #[serde(default)]
    pub at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct IssuedCredit {
    pub credit: CreditNote,
    /// The invoice as it now reads, when the note credits one.
    pub invoice: Option<InvoiceView>,
    pub warnings: Vec<String>,
}

/// Write off money a customer owes, as a document.
///
/// Deliberately not a negative payment and not an edit to the invoice: the
/// invoice's figures are what was charged, and this stands beside them. It
/// also records nothing about money leaving the business, because this
/// feature does not know whether any did.
pub fn issue_credit(app: &App, settings: &Settings, input: CreditInput) -> AppResult<IssuedCredit> {
    let store = app.store();
    let now = util::now_ms();
    let at = input.at.unwrap_or(now);
    let customer = db::require_customer(store, input.customer_id.trim())?;
    if input.amount.minor <= 0 {
        return Err(AppError::invalid(
            "a credit note is an amount a customer no longer owes, so it is more than nothing.              To charge them for something, raise an invoice; to undo a credit note, void it.",
        ));
    }
    if input.reason.trim().is_empty() {
        return Err(AppError::invalid(
            "say why this is being credited: an amount a customer stopped owing with no reason              recorded cannot be told apart from a mistake six months later",
        ));
    }
    // No conversion, ever: a credit note moves no money, so there is no rate
    // anybody actually got, and inventing one here would put a made-up figure
    // into the ledger.
    if input.amount.currency != customer.currency {
        return Err(AppError::invalid(format!(
            "{} is billed in {}, and this note is in {}. A credit note is written in the \
             currency the customer is billed in — no money moves, so there is no rate to \
             convert it at.",
            customer.name, customer.currency, input.amount.currency
        )));
    }

    let mut warnings: Vec<String> = Vec::new();
    let invoice = match input
        .invoice_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        Some(id) => {
            let invoice = db::require_invoice(store, id)?;
            if invoice.customer_id != customer.id {
                return Err(AppError::invalid(format!(
                    "invoice {} belongs to another customer",
                    invoice.number.clone().unwrap_or_else(|| invoice.id.clone())
                )));
            }
            match invoice.state {
                InvoiceState::Issued => {}
                InvoiceState::Draft => {
                    return Err(AppError::conflict(
                        "that invoice is still a draft, so nobody owes it yet: change the draft,                          or issue it first and then credit it",
                    ));
                }
                InvoiceState::Void => {
                    return Err(AppError::conflict(format!(
                        "invoice {} is void, so nothing is owed on it to credit",
                        invoice.number.clone().unwrap_or_else(|| invoice.id.clone())
                    )));
                }
            }
            // You cannot credit more than you charged. The cap is the
            // invoice's total rather than what is left outstanding, so that
            // crediting an invoice a customer has already paid is allowed —
            // that is a real case, and it leaves them in credit rather than
            // silently refusing.
            let already = db::credited_of_invoice(store, &invoice)?;
            let after = already.plus(input.amount)?;
            if after.minor > invoice.total.minor {
                return Err(AppError::invalid(format!(
                    "invoice {} is {} and {} of it has already been credited, so this note of {}                      would write off more than was ever charged",
                    invoice.number.clone().unwrap_or_else(|| invoice.id.clone()),
                    invoice.total.text(),
                    already.text(),
                    input.amount.text()
                )));
            }
            Some(invoice)
        }
        None => None,
    };

    let year = chrono::DateTime::from_timestamp_millis(at)
        .map(|moment| moment.format("%Y").to_string())
        .and_then(|text| text.parse::<i32>().ok())
        .ok_or_else(|| AppError::internal("that moment is outside any calendar year"))?;
    let credit = CreditNote {
        id: util::new_id(),
        number: db::next_credit_number(store, year, &settings.invoice_number_prefix)?,
        customer_id: customer.id.clone(),
        invoice_id: invoice.as_ref().map(|invoice| invoice.id.clone()),
        amount: input.amount,
        reason: input.reason.trim().to_string(),
        note: input.note,
        at,
        state: CreditState::Issued,
        void_reason: String::new(),
        voided_at: None,
        recorded_at: now,
    };
    db::put_credit(store, &credit)?;

    let view = match invoice {
        Some(invoice) => {
            let view = invoice_view(app, invoice, now)?;
            if view.outstanding.is_negative() {
                warnings.push(format!(
                    "invoice {} had already been paid, so crediting it leaves {} standing to the                      customer's favour rather than reducing a debt",
                    view.invoice
                        .number
                        .clone()
                        .unwrap_or_else(|| view.invoice.id.clone()),
                    Money::new(-view.outstanding.minor, view.outstanding.currency).text()
                ));
            }
            Some(view)
        }
        None => {
            warnings.push(
                "this note credits the account rather than an invoice, so it reduces nothing                  until it is set against one"
                    .to_string(),
            );
            None
        }
    };
    Ok(IssuedCredit {
        credit,
        invoice: view,
        warnings,
    })
}

/// Withdraw a credit note, with a reason. The debt it cancelled comes back.
pub fn void_credit(app: &App, id: &str, reason: &str) -> AppResult<CreditNote> {
    let store = app.store();
    let mut credit = db::require_credit(store, id)?;
    if reason.trim().is_empty() {
        return Err(AppError::invalid(
            "voiding a credit note needs a reason: it puts back money a customer had been told              they no longer owed",
        ));
    }
    if credit.state == CreditState::Void {
        return Err(AppError::conflict("that credit note is already void"));
    }
    credit.state = CreditState::Void;
    credit.void_reason = reason.trim().to_string();
    credit.voided_at = Some(util::now_ms());
    db::put_credit(store, &credit)?;
    Ok(credit)
}

// -------------------------------------------------------------------- views

/// What to call an invoice, from its state, its payments and the clock. Not
/// stored: there is one copy of this truth and it is the payments.
pub fn status_of(invoice: &Invoice, paid: Money, credited: Money, now: i64) -> InvoiceStatus {
    match invoice.state {
        InvoiceState::Draft => InvoiceStatus::Draft,
        InvoiceState::Void => InvoiceStatus::Void,
        InvoiceState::Issued => {
            // Nothing more is owed. Which of the two words applies turns on
            // whether a credit note is part of the reason, because `Paid` is
            // a claim that the customer's money arrived for all of it and a
            // write-off is not that claim.
            if paid.minor + credited.minor >= invoice.total.minor {
                if credited.minor > 0 {
                    InvoiceStatus::Credited
                } else {
                    InvoiceStatus::Paid
                }
            } else if invoice.due_at.is_some_and(|due| due < now) {
                InvoiceStatus::Overdue
            } else if paid.minor > 0 {
                InvoiceStatus::PartPaid
            } else {
                // A part-credited invoice with no payment is still simply
                // issued or overdue: money is owed, and nobody has paid any
                // of it. The credit shows in the figures, not in the word.
                InvoiceStatus::Issued
            }
        }
    }
}

pub fn invoice_view(app: &App, invoice: Invoice, now: i64) -> AppResult<InvoiceView> {
    let store = app.store();
    let paid = db::settled_of_invoice(store, &invoice)?;
    let payments = db::payments(
        store,
        &db::PaymentQuery {
            invoice_id: Some(invoice.id.clone()),
            limit: Some(500),
            ..db::PaymentQuery::default()
        },
    )?;
    let credited = db::credited_of_invoice(store, &invoice)?;
    let credits = db::credits(
        store,
        &db::CreditQuery {
            invoice_id: Some(invoice.id.clone()),
            limit: Some(500),
            ..db::CreditQuery::default()
        },
    )?;
    let outstanding = invoice.total.minus(paid)?.minus(credited)?;
    let status = status_of(&invoice, paid, credited, now);
    let days_overdue = match (invoice.due_at, status) {
        (Some(due), InvoiceStatus::Overdue) => Some(days_of(now - due)),
        _ => None,
    };
    Ok(InvoiceView {
        invoice,
        status,
        paid,
        credited,
        outstanding,
        days_overdue,
        payments,
        credits,
    })
}

/// What one customer owes and what they have paid in advance — per currency,
/// because a customer is billed in one currency but may have paid in another.
#[derive(Debug, Serialize)]
pub struct Balance {
    pub customer_id: String,
    /// What is still owed on issued invoices, in the invoices' currency.
    pub outstanding: Totals,
    /// Money received and not yet against an invoice, as it was received.
    pub on_account: Totals,
    /// Credit notes standing against the account rather than one invoice.
    /// Not money: nobody can be paid out of this.
    pub credit_on_account: Totals,
    pub invoices_outstanding: u32,
    pub oldest_due_at: Option<i64>,
    pub note: String,
}

pub fn balance(app: &App, customer_id: &str) -> AppResult<Balance> {
    let store = app.store();
    let shortfalls = db::shortfalls(store, Some(customer_id))?;
    let mut outstanding = Totals::new();
    for shortfall in &shortfalls {
        outstanding.add(shortfall.outstanding())?;
    }
    let mut on_account = Totals::new();
    for (code, minor) in db::on_account(store, Some(customer_id))? {
        on_account.add(Money::new(minor, crate::money::Currency::of(&code)?))?;
    }
    let mut credit_on_account = Totals::new();
    for (code, minor) in db::credit_on_account(store, Some(customer_id))? {
        credit_on_account.add(Money::new(minor, crate::money::Currency::of(&code)?))?;
    }
    Ok(Balance {
        customer_id: customer_id.to_string(),
        outstanding,
        on_account,
        credit_on_account,
        invoices_outstanding: u32::try_from(shortfalls.len()).unwrap_or(u32::MAX),
        oldest_due_at: shortfalls.iter().filter_map(|row| row.due_at).min(),
        note: "one figure per currency: an amount owed in one currency is not converted into \
               another to make a single number"
            .to_string(),
    })
}

/// The rate a conversion was taken at, for a receipt.
pub fn conversion_note(conversion: &Conversion, amount: Money, settled: Money) -> String {
    format!(
        "{} at {} {} per 1 {} = {} {}",
        amount.text(),
        conversion.rate,
        conversion.to,
        conversion.from,
        settled.text(),
        settled.currency
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Period;

    #[test]
    fn a_window_has_to_be_one_whole_period_before_anything_is_charged_for_it() {
        assert!(window_is_one_period(Period::Monthly, 31 * DAY_MS));
        assert!(window_is_one_period(Period::Monthly, 28 * DAY_MS));
        // A summer-time hour does not turn February into a part period.
        assert!(window_is_one_period(
            Period::Monthly,
            28 * DAY_MS - 3_600_000
        ));
        assert!(!window_is_one_period(Period::Monthly, 7 * DAY_MS));
        assert!(!window_is_one_period(Period::Monthly, 365 * DAY_MS));
        assert!(window_is_one_period(Period::Quarterly, 92 * DAY_MS));
        assert!(window_is_one_period(Period::Yearly, 366 * DAY_MS));
        assert!(!window_is_one_period(Period::Yearly, 31 * DAY_MS));
    }

    #[test]
    fn an_issued_invoice_is_refused_every_kind_of_edit() {
        let issued = Invoice {
            state: InvoiceState::Issued,
            number: Some("2026-0001".to_string()),
            ..Invoice::default()
        };
        let err = require_draft(&issued, "add a line to").unwrap_err();
        assert_eq!(err.code, "conflict");
        assert!(
            err.message.contains("never recalculated"),
            "{}",
            err.message
        );
        assert!(require_draft(&Invoice::default(), "issue").is_ok());
    }
}
