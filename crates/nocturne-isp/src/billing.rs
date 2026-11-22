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
//!
//! Two more are about time, and both are answered by the business's calendar
//! ([`Calendar`]) rather than by UTC or by anybody's browser:
//!
//! 4. **A period is days of the calendar it was billed in.** "Has this been
//!    billed already?" compares those days, so a window shifted by a
//!    millisecond or a day is the same period and is refused, and the same
//!    month named after the calendar was changed is still the same month.
//! 5. **A due date is a day.** An invoice is overdue once that day is over
//!    and not before, and there is one function that says so
//!    ([`Calendar::days_overdue`]): the invoice, the arrears report and the
//!    summary all ask it.
//!
//! And one about how they are implemented. **Every function here that writes
//! does its checks and its writes in one transaction** ([`db::atomically`]).
//! Two requests can arrive at the same instant — two operators, or one
//! request and its retry — and a check that is not still true when the write
//! lands is how a period gets billed twice or an invoice credited beyond its
//! total. The functions that take a `&Connection` are the bodies of those
//! transactions; they are private so that nothing can call one outside.

use std::collections::BTreeMap;

use nocturne_app::{App, AppError, AppResult, util};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::calendar::{Calendar, DAY_MS, WIDEST_SHIFT_MS};
use crate::db;
use crate::model::{
    CreditNote, CreditState, Customer, Invoice, InvoiceLine, InvoiceState, InvoiceStatus,
    InvoiceView, LineKind, Payment, PaymentState, Period, Proration, Settings, Subscription,
};
use crate::money::{Conversion, ConversionInput, Money, Totals};

/// Whole days, to the nearest day — so an hour of summer time does not turn
/// a month into "30 days and 23 hours".
fn days_of(ms: i64) -> i64 {
    // Saturating: a window may be anything a caller typed, up to the whole
    // range of an instant, and the answer for one that size is "not a
    // period", not an overflow.
    ms.saturating_add(DAY_MS / 2) / DAY_MS
}

/// Which kind of period a window of this length is one of, if any.
///
/// A calendar month is 28 to 31 days and nothing here invents an average one:
/// a window that is not one period of a subscription's kind produces no line
/// for it and says why, rather than charging a figure nobody agreed. The
/// three ranges do not meet, so a window is one kind of period at most —
/// which is what lets [`billed_already`] tell a quarter from the three months
/// inside it.
fn period_kind(window_ms: i64) -> Option<Period> {
    match days_of(window_ms) {
        28..=31 => Some(Period::Monthly),
        89..=92 => Some(Period::Quarterly),
        365..=366 => Some(Period::Yearly),
        _ => None,
    }
}

/// Does a window of this length look like one whole period of this kind?
fn window_is_one_period(period: Period, window_ms: i64) -> bool {
    period_kind(window_ms) == Some(period)
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
    match period_end.checked_sub(period_start) {
        Some(window_ms) if window_ms > 0 => Ok(window_ms),
        _ => Err(AppError::invalid(
            "a billing period ends after it starts: give `period_start` and `period_end` in \
             milliseconds since the epoch, or name a `month`",
        )),
    }
}

/// The period a caller asked for: a month of the business's calendar by
/// name, or two instants. One or the other.
///
/// Naming the month is the way that cannot go wrong: the engine knows the
/// business's calendar, and a browser in another zone or a script that
/// counted thirty-one days back from now does not.
pub fn period(
    calendar: &Calendar,
    period_start: Option<i64>,
    period_end: Option<i64>,
    month: Option<&str>,
) -> AppResult<(i64, i64)> {
    let (start, end) = match (month, period_start, period_end) {
        (Some(month), None, None) => calendar.month(month)?,
        (None, Some(start), Some(end)) => (start, end),
        (Some(_), _, _) => {
            return Err(AppError::invalid(
                "name the period once: either `month`, or `period_start` and `period_end`",
            ));
        }
        (None, _, _) => {
            return Err(AppError::invalid(format!(
                "say which period to bill: `month` (`2025-10`, a month of the business's \
                 calendar, {}), or both `period_start` and `period_end` in milliseconds since \
                 the epoch",
                calendar.name()
            )));
        }
    };
    // Checked once, here, rather than once per customer: a run over a period
    // that is not one would otherwise list every customer as left out, each
    // for the same reason that has nothing to do with them.
    require_period(start, end)?;
    Ok((start, end))
}

/// The invoice that already bills this customer for these days, if one
/// does, and whether its period is exactly this one.
///
/// Billing a period twice is the mistake this stops, and two things make
/// "the same period" more than equal milliseconds:
///
/// * **Overlap, not identity.** A window a millisecond or a day away from
///   one already billed covers the same days and would charge them again. A
///   subscription is only ever charged in a window of its own kind
///   ([`period_kind`]), so two windows can only charge the same plan twice
///   when they are the same kind and overlap — which is the test. A quarter
///   overlaps its three months and bills nothing they billed. A window that
///   is no kind of period charges no plan at all, and only an identical one
///   is refused, as it always was.
/// * **Days, not instants.** Each invoice is compared on the wall clock of
///   the calendar it was written in. October billed while the offset was
///   one thing is still October after it is changed — and the November that
///   follows, whose first hours were inside the old October's instants, is
///   not.
fn billed_already(
    sql: &impl db::Sql,
    calendar: &Calendar,
    customer_id: &str,
    period_start: i64,
    period_end: i64,
) -> AppResult<Option<(Invoice, bool)>> {
    let kind = period_kind(period_end - period_start);
    let (start, end) = (calendar.local(period_start), calendar.local(period_end));
    let candidates = db::invoices_touching(
        sql,
        customer_id,
        period_start.saturating_sub(WIDEST_SHIFT_MS),
        period_end.saturating_add(WIDEST_SHIFT_MS),
    )?;
    for existing in candidates {
        let written = Calendar::stored(existing.utc_offset_minutes);
        let (theirs_start, theirs_end) = (
            written.local(existing.period_start),
            written.local(existing.period_end),
        );
        let identical = theirs_start == start && theirs_end == end;
        let overlaps = theirs_start < end && theirs_end > start;
        let same_kind = kind.is_some()
            && period_kind(existing.period_end.saturating_sub(existing.period_start)) == kind;
        if identical || (overlaps && same_kind) {
            return Ok(Some((existing, identical)));
        }
    }
    Ok(None)
}

/// One subscription's share of a window: the part of it the plan was in
/// force for.
struct Part<'a> {
    subscription: &'a Subscription,
    start: i64,
    end: i64,
}

impl Part<'_> {
    fn covered_ms(&self) -> i64 {
        self.end - self.start
    }
}

/// What one part costs on its own: the whole price for the whole window, or
/// the price pro-rated by the milliseconds covered.
fn charge_alone(part: &Part<'_>, window_ms: i64) -> AppResult<(Money, Option<Proration>)> {
    let subscription = part.subscription;
    let covered_ms = part.covered_ms();
    if covered_ms >= window_ms {
        return Ok((subscription.price, None));
    }
    let scaled = subscription.price.scale_by(covered_ms, window_ms)?;
    Ok((
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
    ))
}

/// The first subscription of the chain of amendments this one belongs to.
fn first_terms<'a>(
    by_id: &BTreeMap<&'a str, &'a Subscription>,
    subscription: &'a Subscription,
) -> &'a str {
    let mut current = subscription;
    // A chain cannot be longer than the customer has subscriptions. Counting
    // them out, rather than looping until there is no `amends`, means a
    // circle in damaged data cannot hang a billing run.
    for _ in 0..by_id.len() {
        match current
            .amends
            .as_deref()
            .and_then(|previous| by_id.get(previous))
        {
            Some(previous) => current = previous,
            None => break,
        }
    }
    current.id.as_str()
}

/// What each part costs.
///
/// Usually each on its own. The exception is one plan whose megabits were
/// amended part of the way through the window at the same price: it is two
/// subscriptions and two lines, and rounding each alone can make them add up
/// to a minor unit more than the price the customer agreed — 12.51 and 12.51
/// for a plan of 25.01. Those parts are rounded together instead: each is
/// what the plan has cost up to the end of that part, less what the earlier
/// parts were charged, so the lines come to exactly what the plan would have
/// cost had nobody amended it, and no part is ever less than nothing.
///
/// What is rounded together is a **run**: parts of one chain of amendments
/// that follow one another with no time between them, at one price. A part
/// at a different price is a different agreement with no single figure to
/// add up to, and a gap is time nobody was on the plan; either ends the run
/// and the next begins after it. So a plan repriced on the 11th and given
/// more megabits on the 16th is one part at the old price and one run at the
/// new.
fn charges(
    subscriptions: &[Subscription],
    parts: &[Part<'_>],
    window_ms: i64,
) -> AppResult<Vec<(Money, Option<Proration>)>> {
    let mut charges = parts
        .iter()
        .map(|part| charge_alone(part, window_ms))
        .collect::<AppResult<Vec<_>>>()?;
    let by_id: BTreeMap<&str, &Subscription> = subscriptions
        .iter()
        .map(|subscription| (subscription.id.as_str(), subscription))
        .collect();
    let mut plans: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, part) in parts.iter().enumerate() {
        plans
            .entry(first_terms(&by_id, part.subscription))
            .or_default()
            .push(index);
    }
    for members in plans.values_mut() {
        members.sort_by_key(|&index| parts[index].start);
        let runs = members.chunk_by(|&earlier, &later| {
            parts[earlier].subscription.price == parts[later].subscription.price
                && parts[earlier].end == parts[later].start
        });
        for run in runs.filter(|run| run.len() >= 2) {
            let price = parts[run[0]].subscription.price;
            let cost_of = |covered_ms: i64| -> AppResult<Money> {
                if covered_ms >= window_ms {
                    Ok(price)
                } else {
                    Ok(price.scale_by(covered_ms, window_ms)?.amount)
                }
            };
            let together = cost_of(run.iter().map(|&index| parts[index].covered_ms()).sum())?;
            let mut covered_ms = 0;
            let mut charged = Money::zero(price.currency);
            for &index in run {
                covered_ms += parts[index].covered_ms();
                let so_far = cost_of(covered_ms)?;
                charges[index].0 = so_far.minus(charged)?;
                charged = so_far;
                if let Some(proration) = &mut charges[index].1 {
                    proration.basis.push_str(&format!(
                        "; one of {} parts of the same plan, rounded together so that they come \
                         to {} between them and not a minor unit more or less",
                        run.len(),
                        together.text()
                    ));
                }
            }
        }
    }
    Ok(charges)
}

/// Build a draft invoice for one customer and one period, out of what their
/// subscriptions say they agreed to — not out of the tariffs — and store it.
///
/// "Has this period been billed already?" and the write are one transaction:
/// two drafts asked for at the same moment would otherwise both find nothing
/// and both bill the period.
pub fn draft(
    app: &App,
    calendar: &Calendar,
    customer: &Customer,
    period_start: i64,
    period_end: i64,
    allow_empty: bool,
) -> AppResult<DraftedInvoice> {
    db::atomically(app.store(), |conn| {
        // The caller read its copy before this transaction began. The account
        // may have been deleted since, and a draft built from the stale copy
        // would be an invoice for nobody.
        let customer = db::require_customer(conn, &customer.id)?;
        let drafted = build_on(
            conn,
            calendar,
            &customer,
            period_start,
            period_end,
            allow_empty,
        )?;
        db::put_invoice(conn, &drafted.invoice)?;
        Ok(drafted)
    })
}

/// Work out what a draft invoice would say, without writing anything. This is
/// what `isp.invoice.run` previews with: a preview that guessed, or that had
/// to write to find out, would be worth nothing.
pub fn build(
    app: &App,
    calendar: &Calendar,
    customer: &Customer,
    period_start: i64,
    period_end: i64,
    allow_empty: bool,
) -> AppResult<DraftedInvoice> {
    build_on(
        app.store(),
        calendar,
        customer,
        period_start,
        period_end,
        allow_empty,
    )
}

fn build_on(
    sql: &impl db::Sql,
    calendar: &Calendar,
    customer: &Customer,
    period_start: i64,
    period_end: i64,
    allow_empty: bool,
) -> AppResult<DraftedInvoice> {
    let now = util::now_ms();
    let window_ms = require_period(period_start, period_end)?;
    if let Some((existing, identical)) =
        billed_already(sql, calendar, &customer.id, period_start, period_end)?
    {
        let written = Calendar::stored(existing.utc_offset_minutes);
        return Err(AppError::conflict(format!(
            "{} already has {} for {} ({} to {}, {}). Billing a period twice is the mistake this \
             refuses; void that invoice first if it is wrong.",
            customer.name,
            existing
                .number
                .clone()
                .unwrap_or_else(|| format!("a draft (`{}`)", existing.id)),
            if identical {
                "that period"
            } else {
                "a period that overlaps this one"
            },
            written.date_text(existing.period_start),
            // The last day it covers: the period ends where the next begins.
            written.date_text(existing.period_end.saturating_sub(1)),
            existing.state.as_str()
        ))
        .with_details(json!({"invoice_id": existing.id, "number": existing.number})));
    }

    let subscriptions = db::subscriptions(
        sql,
        &db::SubscriptionQuery {
            customer_id: Some(customer.id.clone()),
            ..db::SubscriptionQuery::default()
        },
    )?;
    let mut parts: Vec<Part<'_>> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    for subscription in &subscriptions {
        if !subscription.overlaps(period_start, period_end) {
            continue;
        }
        // Asked before anything is said about it: a plan that ended at the
        // moment it started never ran, and listing it below as one this
        // invoice "could not charge" would tell the operator a plan was in
        // force that never was.
        let start = subscription.started_at.max(period_start);
        let end = subscription.ended_at.unwrap_or(period_end).min(period_end);
        if end <= start {
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
        parts.push(Part {
            subscription,
            start,
            end,
        });
    }
    let lines: Vec<InvoiceLine> = parts
        .iter()
        .zip(charges(&subscriptions, &parts, window_ms)?)
        .map(|(part, (amount, proration))| {
            let subscription = part.subscription;
            InvoiceLine {
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
                period_start: Some(part.start),
                period_end: Some(part.end),
                prorated: proration,
                amount,
            }
        })
        .collect();
    if lines.is_empty() && !allow_empty {
        return Err(nothing_to_bill(customer, subscriptions.len(), &skipped));
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
        utc_offset_minutes: calendar.offset_minutes(),
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

/// Why a customer has no invoice for a period, said so that somebody can act
/// on it.
///
/// Two different things end here and they are not to be confused. A customer
/// with no plan in force in the period has nothing to bill. A customer whose
/// plan *is* in force and could not be charged — a yearly plan in a monthly
/// window, a price in another currency — has something to bill that this
/// window cannot, and the reasons are the whole of what the operator needs:
/// they are in the message, because a billing run shows nothing else, and in
/// `details.skipped` for a caller that wants them apart.
fn nothing_to_bill(customer: &Customer, subscriptions: usize, skipped: &[Skipped]) -> AppError {
    if skipped.is_empty() {
        return AppError::invalid(format!(
            "{} has nothing to bill for that period: {} subscription(s) on the account, none of \
             them in force in it",
            customer.name, subscriptions
        ));
    }
    AppError::invalid(format!(
        "{} has nothing this invoice can charge for that period, though {} subscription(s) \
         were in force in it: {}",
        customer.name,
        skipped.len(),
        skipped
            .iter()
            .map(|skip| format!("{} — {}", skip.tariff_name, skip.reason))
            .collect::<Vec<_>>()
            .join("; ")
    ))
    .with_details(json!({"skipped": skipped}))
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
    db::atomically(app.store(), |conn| {
        let mut invoice = db::require_invoice(conn, &input.invoice_id)?;
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
            amount: input.amount.enterable("a line")?,
            ..InvoiceLine::default()
        });
        // Each line is within the limit; enough of them together need not be.
        invoice.total = invoice.sum_lines()?.enterable("an invoice's total")?;
        invoice.updated_at = util::now_ms();
        db::put_invoice(conn, &invoice)?;
        Ok(invoice)
    })
}

pub fn remove_line(app: &App, invoice_id: &str, line_id: &str) -> AppResult<Invoice> {
    db::atomically(app.store(), |conn| {
        let mut invoice = db::require_invoice(conn, invoice_id)?;
        require_draft(&invoice, "remove a line from")?;
        let before = invoice.lines.len();
        invoice.lines.retain(|line| line.id != line_id);
        if invoice.lines.len() == before {
            return Err(AppError::not_found(format!("line `{line_id}`")));
        }
        invoice.total = invoice.sum_lines()?;
        invoice.updated_at = util::now_ms();
        db::put_invoice(conn, &invoice)?;
        Ok(invoice)
    })
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
///
/// **A due date is a day**, in the calendar the invoice was drafted in — the
/// same one its period is days of. It may be the day of issue and may not be
/// a day before it, whatever the hour of either; and the year in the number
/// is that calendar's year, so an invoice issued in the first hours of 1
/// January is numbered in the new year wherever UTC still is.
///
/// The number is taken in the same transaction that freezes the invoice, so
/// a draft issued twice at once is issued once, and the call that lost takes
/// no number with it.
pub fn issue(
    app: &App,
    settings: &Settings,
    id: &str,
    due_at: Option<i64>,
    issued_at: Option<i64>,
) -> AppResult<Invoice> {
    db::atomically(app.store(), |conn| {
        let now = util::now_ms();
        let mut invoice = db::require_invoice(conn, id)?;
        require_draft(&invoice, "issue")?;
        if invoice.lines.is_empty() {
            return Err(AppError::invalid(
                "an invoice with no lines charges nothing: add what it is for, or delete the draft",
            ));
        }
        // The last arithmetic this invoice will ever have done to it.
        invoice.total = invoice.sum_lines()?.enterable("an invoice's total")?;
        if invoice.total.is_negative() {
            return Err(AppError::invalid(format!(
                "this invoice comes to {}, which is less than nothing: an invoice asks a customer \
                 for money and cannot owe them any. What they are owed is a credit note \
                 (`isp.credit.issue`), which carries its reason; take the line that is too large \
                 off this draft, or void it.",
                invoice.total.text()
            )));
        }
        let issued = issued_at.unwrap_or(now);
        if issued > now {
            return Err(AppError::invalid(
                "an invoice cannot be issued in the future: until it goes out it is a draft",
            ));
        }
        let calendar = Calendar::stored(invoice.utc_offset_minutes);
        let due = due_at.unwrap_or_else(|| {
            // Whole days on from the moment of issue: the same time of day,
            // so many days later, which in a calendar with a fixed offset is
            // that many days on.
            issued.saturating_add(i64::from(settings.payment_terms_days) * DAY_MS)
        });
        if calendar.day(due) < calendar.day(issued) {
            return Err(AppError::invalid(format!(
                "an invoice cannot fall due before the day it is issued: it would be issued on \
                 {} and due on {}",
                calendar.date_text(issued),
                calendar.date_text(due)
            )));
        }
        let year = calendar.year(issued)?;
        invoice.number = Some(db::next_invoice_number(
            conn,
            year,
            &settings.invoice_number_prefix,
        )?);
        invoice.state = InvoiceState::Issued;
        invoice.issued_at = Some(issued);
        invoice.due_at = Some(due);
        invoice.updated_at = now;
        db::put_invoice(conn, &invoice)?;
        Ok(invoice)
    })
}

/// Withdraw an invoice, with a reason. The document stays.
///
/// Refused while anything still stands against it. A recorded payment would
/// be money that came in for an invoice that no longer exists; an issued
/// credit note would be a numbered document the customer was given, left
/// writing off nothing, with no reason recorded for why it stopped counting.
/// Each is withdrawn first, with its own reason, and then the invoice can be.
pub fn void(app: &App, id: &str, reason: &str) -> AppResult<Invoice> {
    db::atomically(app.store(), |conn| void_on(conn, id, reason))
}

fn void_on(conn: &Connection, id: &str, reason: &str) -> AppResult<Invoice> {
    let mut invoice = db::require_invoice(conn, id)?;
    if reason.trim().is_empty() {
        return Err(AppError::invalid(
            "voiding an invoice needs a reason: it is what the trail will say happened",
        ));
    }
    if invoice.state == InvoiceState::Void {
        return Err(AppError::conflict("that invoice is already void"));
    }
    let settled = db::payments(
        conn,
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
    let credited = db::credits(
        conn,
        &db::CreditQuery {
            invoice_id: Some(invoice.id.clone()),
            state: Some("issued".to_string()),
            ..db::CreditQuery::default()
        },
    )?;
    if !credited.is_empty() {
        return Err(AppError::conflict(format!(
            "{} credit note(s) stand against invoice {} ({}), and voiding it would leave them \
             issued against nothing: what they wrote off would stop counting anywhere, with no \
             reason recorded. Void each credit note first, with its own reason. If this invoice \
             is being reissued, credit the replacement once it has been issued.",
            credited.len(),
            invoice.number.clone().unwrap_or_else(|| invoice.id.clone()),
            credited
                .iter()
                .map(|note| note.number.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let now = util::now_ms();
    invoice.state = InvoiceState::Void;
    invoice.voided_at = Some(now);
    invoice.void_reason = reason.trim().to_string();
    invoice.updated_at = now;
    db::put_invoice(conn, &invoice)?;
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
///
/// One transaction: the void, the replacement and the link between them all
/// happen or none does, so there is never a void invoice whose replacement
/// was not written. Whatever [`void`] refuses, this refuses.
pub fn reissue(app: &App, id: &str, reason: &str) -> AppResult<Reissued> {
    db::atomically(app.store(), |conn| {
        let now = util::now_ms();
        let original = db::require_invoice(conn, id)?;
        let mut voided = void_on(conn, id, reason)?;
        let mut draft = Invoice {
            id: util::new_id(),
            number: None,
            customer_id: original.customer_id.clone(),
            customer_name: original.customer_name.clone(),
            currency: original.currency,
            state: InvoiceState::Draft,
            period_start: original.period_start,
            period_end: original.period_end,
            // The same days, so the same calendar: the lines' periods came
            // across with it.
            utc_offset_minutes: original.utc_offset_minutes,
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
        db::put_invoice(conn, &draft)?;
        voided.replaced_by = Some(draft.id.clone());
        voided.updated_at = now;
        db::put_invoice(conn, &voided)?;
        Ok(Reissued {
            voided,
            draft,
            note: "the original is void and still readable; the replacement is a draft, so the \
                   correction can be made before anybody is asked to pay it"
                .to_string(),
        })
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

pub fn record_payment(
    app: &App,
    calendar: &Calendar,
    input: PaymentInput,
) -> AppResult<RecordedPayment> {
    db::atomically(app.store(), |conn| {
        let now = util::now_ms();
        let at = input.at.unwrap_or(now);
        let customer = db::require_customer(conn, input.customer_id.trim())?;
        if input.amount.minor <= 0 {
            return Err(AppError::invalid(
                "a payment is what was received, so it is more than nothing. A correction is a \
                 void of the payment that was wrong, not a negative one.",
            ));
        }
        input.amount.enterable("a payment")?;
        if input.method.trim().is_empty() {
            return Err(AppError::invalid(
                "say how the money came in (cash, bank transfer, a wallet): a payment with no \
                 method cannot be reconciled against anything",
            ));
        }
        let mut warnings: Vec<String> = Vec::new();
        let invoice = match input
            .invoice_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
        {
            Some(id) => Some(db::require_invoice(conn, id)?),
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
                            "that invoice is still a draft: issue it before taking money against \
                             it, or record the payment on account",
                        ));
                    }
                    InvoiceState::Void => {
                        return Err(AppError::conflict(format!(
                            "invoice {} is void; record the payment against the invoice that \
                             replaced it, or on account",
                            invoice.number.clone().unwrap_or_else(|| invoice.id.clone())
                        )));
                    }
                }
                if input.amount.currency == invoice.currency {
                    if input.conversion.is_some() {
                        return Err(AppError::invalid(format!(
                            "this payment and invoice {} are both in {}: there is nothing to \
                             convert, so no rate is recorded",
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
                                "this payment is in {} and the invoice is in {}. Enter the rate \
                                 you actually got — there is no rate feed and nothing here will \
                                 guess one — as `conversion: {{from, to, rate}}`, where `rate` is \
                                 whole minor units of `to` for one `from`.",
                                input.amount.currency, invoice.currency
                            ))
                        })?
                        .resolve(at)?;
                    if !conversion.relates(input.amount.currency, invoice.currency) {
                        return Err(AppError::invalid(format!(
                            "that rate is between {} and {}, and this payment settles {} against \
                             an invoice in {}",
                            conversion.from, conversion.to, input.amount.currency, invoice.currency
                        )));
                    }
                    let applied = conversion.apply(input.amount)?;
                    // What it settles is added up as well, in the invoice's
                    // currency, and a rate can make a sensible amount absurd.
                    applied.amount.enterable("what this payment settles")?;
                    if !applied.exact {
                        warnings.push(format!(
                            "at that rate {} is {} to the nearest minor unit, so the invoice is \
                             settled by a figure that was rounded; the rate and the amount \
                             received are both recorded",
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
        db::put_payment(conn, &payment)?;

        let view = match invoice {
            Some(invoice) => {
                let view = invoice_view(conn, calendar, invoice, now)?;
                if view.outstanding.is_negative() {
                    warnings.push(format!(
                        "invoice {} is now overpaid by {}; the extra stays recorded against it \
                         rather than being moved anywhere on its own",
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
    })
}

pub fn void_payment(app: &App, id: &str, reason: &str) -> AppResult<Payment> {
    db::atomically(app.store(), |conn| {
        let mut payment = db::require_payment(conn, id)?;
        if reason.trim().is_empty() {
            return Err(AppError::invalid(
                "voiding a payment needs a reason: money that was recorded and then withdrawn has \
                 to say why",
            ));
        }
        if payment.state == PaymentState::Void {
            return Err(AppError::conflict("that payment is already void"));
        }
        payment.state = PaymentState::Void;
        payment.void_reason = reason.trim().to_string();
        payment.voided_at = Some(util::now_ms());
        db::put_payment(conn, &payment)?;
        Ok(payment)
    })
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
pub fn issue_credit(
    app: &App,
    settings: &Settings,
    calendar: &Calendar,
    input: CreditInput,
) -> AppResult<IssuedCredit> {
    db::atomically(app.store(), |conn| {
        let now = util::now_ms();
        let at = input.at.unwrap_or(now);
        let customer = db::require_customer(conn, input.customer_id.trim())?;
        if input.amount.minor <= 0 {
            return Err(AppError::invalid(
                "a credit note is an amount a customer no longer owes, so it is more than \
                 nothing. To charge them for something, raise an invoice; to undo a credit note, \
                 void it.",
            ));
        }
        input.amount.enterable("a credit note")?;
        if input.reason.trim().is_empty() {
            return Err(AppError::invalid(
                "say why this is being credited: an amount a customer stopped owing with no \
                 reason recorded cannot be told apart from a mistake six months later",
            ));
        }
        // No conversion, ever: a credit note moves no money, so there is no
        // rate anybody actually got, and inventing one here would put a
        // made-up figure into the ledger.
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
            Some(id) => Some(creditable_invoice(conn, &customer.id, id, input.amount)?),
            None => None,
        };

        // The business's year, as an invoice's number is: a note agreed in
        // the first hours of 1 January is this year's.
        let year = calendar.year(at)?;
        let credit = CreditNote {
            id: util::new_id(),
            number: db::next_credit_number(conn, year, &settings.invoice_number_prefix)?,
            customer_id: customer.id.clone(),
            invoice_id: invoice.as_ref().map(|invoice| invoice.id.clone()),
            amount: input.amount,
            reason: input.reason.trim().to_string(),
            note: input.note,
            at,
            allocated_at: None,
            state: CreditState::Issued,
            void_reason: String::new(),
            voided_at: None,
            recorded_at: now,
        };
        db::put_credit(conn, &credit)?;

        let view = match invoice {
            Some(invoice) => {
                let view = invoice_view(conn, calendar, invoice, now)?;
                warnings.extend(in_credit_warning(&view));
                Some(view)
            }
            None => {
                warnings.push(
                    "this note credits the account rather than an invoice, so it reduces nothing \
                     until it is set against one with `isp.credit.allocate`"
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
    })
}

/// The invoice a credit note of `amount` may stand against: this customer's,
/// issued, and with that much of its total not yet credited. One rule for a
/// note issued against an invoice and for one set against it afterwards.
fn creditable_invoice(
    conn: &Connection,
    customer_id: &str,
    invoice_id: &str,
    amount: Money,
) -> AppResult<Invoice> {
    let invoice = db::require_invoice(conn, invoice_id)?;
    let name = invoice.number.clone().unwrap_or_else(|| invoice.id.clone());
    if invoice.customer_id != customer_id {
        return Err(AppError::invalid(format!(
            "invoice {name} belongs to another customer"
        )));
    }
    match invoice.state {
        InvoiceState::Issued => {}
        InvoiceState::Draft => {
            return Err(AppError::conflict(
                "that invoice is still a draft, so nobody owes it yet: change the draft, or \
                 issue it first and then credit it",
            ));
        }
        InvoiceState::Void => {
            return Err(AppError::conflict(format!(
                "invoice {name} is void, so nothing is owed on it to credit"
            )));
        }
    }
    if amount.currency != invoice.currency {
        return Err(AppError::invalid(format!(
            "invoice {name} is in {} and this note is in {}: a credit note moves no money, so \
             there is no rate to convert it at",
            invoice.currency, amount.currency
        )));
    }
    // You cannot credit more than you charged. The cap is the invoice's total
    // rather than what is left outstanding, so that crediting an invoice a
    // customer has already paid is allowed — that is a real case, and it
    // leaves them in credit rather than silently refusing.
    let already = db::credited_of_invoice(conn, &invoice)?;
    let after = already.plus(amount)?;
    if after.minor > invoice.total.minor {
        return Err(AppError::invalid(format!(
            "invoice {name} is {} and {} of it has already been credited, so this note of {} \
             would write off more than was ever charged",
            invoice.total.text(),
            already.text(),
            amount.text()
        )));
    }
    Ok(invoice)
}

/// Said when a credit note leaves an invoice settled beyond its total.
fn in_credit_warning(view: &InvoiceView) -> Option<String> {
    view.outstanding.is_negative().then(|| {
        format!(
            "with this credit, invoice {} is settled {} beyond its total. That stands in the \
             customer's favour rather than reducing a debt: it is reported as `overpaid` in \
             their balance and in the arrears report, and is not taken off anything else they \
             owe.",
            view.invoice
                .number
                .clone()
                .unwrap_or_else(|| view.invoice.id.clone()),
            Money::new(-view.outstanding.minor, view.outstanding.currency).text()
        )
    })
}

/// Set a credit note that was issued on the account against one invoice.
///
/// It is the same document — same number, same amount, same reason — now
/// reducing a particular debt, under the same rule as a note issued against
/// that invoice in the first place. It happens once: moving a note from one
/// invoice to another is a void, with its reason, and a new note, so that
/// the trail shows both.
pub fn allocate_credit(
    app: &App,
    calendar: &Calendar,
    id: &str,
    invoice_id: &str,
) -> AppResult<IssuedCredit> {
    db::atomically(app.store(), |conn| {
        let now = util::now_ms();
        let mut credit = db::require_credit(conn, id.trim())?;
        if credit.state == CreditState::Void {
            return Err(AppError::conflict(format!(
                "credit note {} is void{}, so it credits nothing. Issue a new one against the \
                 invoice instead.",
                credit.number,
                if credit.void_reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", credit.void_reason)
                }
            )));
        }
        if let Some(current) = &credit.invoice_id {
            let against = db::invoice(conn, current)?
                .and_then(|invoice| invoice.number)
                .unwrap_or_else(|| current.clone());
            return Err(AppError::conflict(format!(
                "credit note {} is already set against invoice {against}. To credit a different \
                 invoice, void this note with a reason and issue another, so the trail shows \
                 both.",
                credit.number
            )));
        }
        let invoice_id = invoice_id.trim();
        if invoice_id.is_empty() {
            return Err(AppError::invalid(
                "say which invoice to set this credit note against",
            ));
        }
        let invoice = creditable_invoice(conn, &credit.customer_id, invoice_id, credit.amount)?;
        credit.invoice_id = Some(invoice.id.clone());
        credit.allocated_at = Some(now);
        db::put_credit(conn, &credit)?;
        let view = invoice_view(conn, calendar, invoice, now)?;
        let warnings: Vec<String> = in_credit_warning(&view).into_iter().collect();
        Ok(IssuedCredit {
            credit,
            invoice: Some(view),
            warnings,
        })
    })
}

/// Withdraw a credit note, with a reason. The debt it cancelled comes back.
pub fn void_credit(app: &App, id: &str, reason: &str) -> AppResult<CreditNote> {
    db::atomically(app.store(), |conn| {
        let mut credit = db::require_credit(conn, id)?;
        if reason.trim().is_empty() {
            return Err(AppError::invalid(
                "voiding a credit note needs a reason: it puts back money a customer had been \
                 told they no longer owed",
            ));
        }
        if credit.state == CreditState::Void {
            return Err(AppError::conflict("that credit note is already void"));
        }
        credit.state = CreditState::Void;
        credit.void_reason = reason.trim().to_string();
        credit.voided_at = Some(util::now_ms());
        db::put_credit(conn, &credit)?;
        Ok(credit)
    })
}

// -------------------------------------------------------------------- views

/// What to call an invoice, from its state, its payments and the clock. Not
/// stored: there is one copy of this truth and it is the payments.
///
/// `today` is the business's calendar, which is what says whether the due
/// day is over ([`Calendar::days_overdue`]).
pub fn status_of(
    invoice: &Invoice,
    paid: Money,
    credited: Money,
    today: &Calendar,
    now: i64,
) -> InvoiceStatus {
    match invoice.state {
        InvoiceState::Draft => InvoiceStatus::Draft,
        InvoiceState::Void => InvoiceStatus::Void,
        InvoiceState::Issued => {
            // Wider than the ledger's own figures, so that a row from before
            // amounts were bounded cannot make this arithmetic wrap.
            let total = i128::from(invoice.total.minor);
            let paid = i128::from(paid.minor);
            let credited = i128::from(credited.minor);
            let settled = paid + credited >= total;
            // Nothing more is owed. Which word applies turns on what made it
            // so. `Paid` is a claim that the customer's money arrived for all
            // of it, so there has to have been some: an invoice that comes
            // to nothing is not paid by nobody paying it. `Credited` is for
            // an invoice that a write-off was needed to clear — asked second,
            // because asking it first would take `Paid` away from an invoice
            // settled in full the day somebody adds a goodwill credit to it.
            if paid >= total && paid > 0 {
                InvoiceStatus::Paid
            } else if settled && credited > 0 {
                InvoiceStatus::Credited
            } else if settled {
                // Nothing was ever owed on it, and nothing settled it. It is
                // an issued invoice, and it can never be late.
                InvoiceStatus::Issued
            } else if overdue_by(invoice, today, now).is_some() {
                InvoiceStatus::Overdue
            } else if paid > 0 {
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

/// How many days past its due day an invoice is, or `None` while that day is
/// not over. The due day is read in the calendar the invoice was written in.
fn overdue_by(invoice: &Invoice, today: &Calendar, now: i64) -> Option<i64> {
    today.days_overdue(
        invoice.due_at?,
        Calendar::stored(invoice.utc_offset_minutes),
        now,
    )
}

pub fn invoice_view(
    sql: &impl db::Sql,
    today: &Calendar,
    invoice: Invoice,
    now: i64,
) -> AppResult<InvoiceView> {
    let paid = db::settled_of_invoice(sql, &invoice)?;
    let payments = db::payments(
        sql,
        &db::PaymentQuery {
            invoice_id: Some(invoice.id.clone()),
            limit: Some(500),
            ..db::PaymentQuery::default()
        },
    )?;
    let credited = db::credited_of_invoice(sql, &invoice)?;
    let credits = db::credits(
        sql,
        &db::CreditQuery {
            invoice_id: Some(invoice.id.clone()),
            limit: Some(500),
            ..db::CreditQuery::default()
        },
    )?;
    let outstanding = invoice.total.minus(paid)?.minus(credited)?;
    let status = status_of(&invoice, paid, credited, today, now);
    let days_overdue = match status {
        InvoiceStatus::Overdue => overdue_by(&invoice, today, now),
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
    /// What has been settled beyond the total of an issued invoice — a
    /// payment larger than the invoice, or a credit note on one already paid
    /// — in the invoices' currency. It stands in the customer's favour, and
    /// it is **not** taken off `outstanding`: nothing here moves it to
    /// another invoice on its own, so the two are shown side by side.
    pub overpaid: Totals,
    pub invoices_outstanding: u32,
    pub invoices_overpaid: u32,
    pub oldest_due_at: Option<i64>,
    pub note: String,
}

pub fn balance(app: &App, customer_id: &str) -> AppResult<Balance> {
    let store = app.store();
    let shortfalls = db::shortfalls(store, Some(customer_id))?;
    let mut outstanding = Totals::new();
    for shortfall in &shortfalls {
        outstanding.add(shortfall.outstanding()?)?;
    }
    let mut on_account = Totals::new();
    for (code, minor) in db::on_account(store, Some(customer_id))? {
        on_account.add(Money::new(minor, crate::money::Currency::of(&code)?))?;
    }
    let mut credit_on_account = Totals::new();
    for (code, minor) in db::credit_on_account(store, Some(customer_id))? {
        credit_on_account.add(Money::new(minor, crate::money::Currency::of(&code)?))?;
    }
    let overpayments = db::overpayments(store, Some(customer_id))?;
    let mut overpaid = Totals::new();
    for overpayment in &overpayments {
        overpaid.add(overpayment.overpaid()?)?;
    }
    Ok(Balance {
        customer_id: customer_id.to_string(),
        outstanding,
        on_account,
        credit_on_account,
        overpaid,
        invoices_outstanding: u32::try_from(shortfalls.len()).unwrap_or(u32::MAX),
        invoices_overpaid: u32::try_from(overpayments.len()).unwrap_or(u32::MAX),
        oldest_due_at: shortfalls.iter().filter_map(|row| row.due_at).min(),
        note: "one figure per currency: an amount owed in one currency is not converted into \
               another to make a single number. Nothing is netted: what is owed, what was paid \
               or credited on account and what was overpaid are each reported as they stand."
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

    fn utc() -> Calendar {
        Calendar::stored(0)
    }

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
        // No window is two kinds of period, which is what the "already
        // billed" test leans on.
        for days in 1..=400 {
            let kinds = [Period::Monthly, Period::Quarterly, Period::Yearly]
                .into_iter()
                .filter(|period| window_is_one_period(*period, days * DAY_MS))
                .count();
            assert!(kinds <= 1, "{days} days is {kinds} kinds of period");
        }
    }

    #[test]
    fn the_word_for_a_settled_invoice_turns_on_whether_the_money_alone_covers_it() {
        let usd = crate::money::Currency::of("USD").expect("a currency");
        let invoice = Invoice {
            state: InvoiceState::Issued,
            total: Money::new(10_000, usd),
            due_at: Some(1_000),
            ..Invoice::default()
        };
        let status = |paid: i64, credited: i64, now: i64| {
            status_of(
                &invoice,
                Money::new(paid, usd),
                Money::new(credited, usd),
                &utc(),
                now,
            )
        };
        // Their money arrived for all of it; a credit on top changes nothing.
        assert_eq!(status(10_000, 0, 0), InvoiceStatus::Paid);
        assert_eq!(status(10_000, 2_000, 0), InvoiceStatus::Paid);
        assert_eq!(status(12_000, 0, 0), InvoiceStatus::Paid);
        // It took a write-off to clear it.
        assert_eq!(status(0, 10_000, 0), InvoiceStatus::Credited);
        assert_eq!(status(6_000, 4_000, 0), InvoiceStatus::Credited);
        assert_eq!(status(9_999, 2_000, 2 * DAY_MS), InvoiceStatus::Credited);
        // Still short: a credit is not a payment, and does not stop the clock.
        assert_eq!(status(0, 4_000, 0), InvoiceStatus::Issued);
        assert_eq!(status(1_000, 4_000, 0), InvoiceStatus::PartPaid);
        // Later on the day it fell due it is still not late; the next day it is.
        assert_eq!(status(1_000, 4_000, DAY_MS - 1), InvoiceStatus::PartPaid);
        assert_eq!(status(1_000, 4_000, DAY_MS), InvoiceStatus::Overdue);
        assert_eq!(status(0, 0, DAY_MS), InvoiceStatus::Overdue);
    }

    #[test]
    fn an_invoice_for_nothing_is_not_paid_by_nobody_paying_it() {
        let usd = crate::money::Currency::of("USD").expect("a currency");
        let word = |total: i64, paid: i64, credited: i64| {
            let invoice = Invoice {
                state: InvoiceState::Issued,
                total: Money::new(total, usd),
                due_at: Some(0),
                ..Invoice::default()
            };
            // A year after it fell due: it would be overdue if anything were owed.
            status_of(
                &invoice,
                Money::new(paid, usd),
                Money::new(credited, usd),
                &utc(),
                365 * DAY_MS,
            )
        };
        assert_eq!(word(0, 0, 0), InvoiceStatus::Issued);
        // Money that did arrive is still money that arrived.
        assert_eq!(word(0, 500, 0), InvoiceStatus::Paid);
        // An invoice from before a negative total was refused at issue.
        assert_eq!(word(-500, 0, 0), InvoiceStatus::Issued);
        // Figures from before amounts were bounded do not wrap round.
        assert_eq!(word(i64::MAX, i64::MAX, i64::MAX), InvoiceStatus::Paid);
        assert_eq!(word(i64::MAX, 0, i64::MAX), InvoiceStatus::Credited);
        assert_eq!(word(i64::MAX, 1, 1), InvoiceStatus::Overdue);
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

    fn plan(
        id: &str,
        amends: Option<&str>,
        price: i64,
        from: i64,
        to: Option<i64>,
    ) -> Subscription {
        let usd = crate::money::Currency::of("USD").expect("a currency");
        Subscription {
            id: id.to_string(),
            amends: amends.map(str::to_string),
            price: Money::new(price, usd),
            unit_price: Money::new(price, usd),
            started_at: from,
            ended_at: to,
            ..Subscription::default()
        }
    }

    /// The amounts charged for each plan over a window of `days`, in the
    /// order the plans were given.
    fn amounts(plans: &[Subscription], days: i64) -> Vec<i64> {
        let window_ms = days * DAY_MS;
        let parts: Vec<Part<'_>> = plans
            .iter()
            .map(|subscription| Part {
                subscription,
                start: subscription.started_at.max(0),
                end: subscription.ended_at.unwrap_or(window_ms).min(window_ms),
            })
            .collect();
        charges(plans, &parts, window_ms)
            .expect("the charges")
            .into_iter()
            .map(|(amount, _)| amount.minor)
            .collect()
    }

    #[test]
    fn the_parts_of_one_plan_come_to_what_the_plan_costs_however_often_it_was_amended() {
        // 25.01, amended half way: 12.505 twice is not 12.51 twice.
        let half = 15 * DAY_MS;
        let twice = [
            plan("a", None, 2_501, -DAY_MS, Some(half)),
            plan("b", Some("a"), 2_501, half, None),
        ];
        assert_eq!(amounts(&twice, 30), [1_251, 1_250]);
        // Whatever order the subscriptions come in.
        let reversed = [twice[1].clone(), twice[0].clone()];
        assert_eq!(amounts(&reversed, 30), [1_250, 1_251]);

        // Amended every day of a thirty-day month, at 1.00: thirty parts of
        // 3.33 cents each. Each alone rounds to 3 and they would come to 90.
        let daily: Vec<Subscription> = (0..30)
            .map(|day| {
                let id = format!("d{day}");
                let previous = format!("d{}", day - 1);
                plan(
                    &id,
                    (day > 0).then_some(previous.as_str()),
                    100,
                    day * DAY_MS,
                    (day < 29).then_some((day + 1) * DAY_MS),
                )
            })
            .collect();
        let parts = amounts(&daily, 30);
        assert_eq!(parts.iter().sum::<i64>(), 100);
        assert!(
            parts.iter().all(|amount| (3..=4).contains(amount)),
            "no part is more than a minor unit from its own share: {parts:?}"
        );

        // A plan that began part of the way through and was then amended
        // comes to what the part it ran for would have cost: 20 days of 30
        // at 25.01 is 16.67, not 8.34 twice.
        let late = [
            plan("a", None, 2_501, 10 * DAY_MS, Some(20 * DAY_MS)),
            plan("b", Some("a"), 2_501, 20 * DAY_MS, None),
        ];
        assert_eq!(amounts(&late, 30), [834, 833]);
    }

    #[test]
    fn a_plan_repriced_and_then_amended_is_rounded_together_from_the_new_price_on() {
        let day = DAY_MS;
        // 25.01 for ten days, then 30.03 — and at 30.03 the megabits are
        // amended twice more. The first part is its own agreement. The other
        // three are one plan at one price for twenty days: 5.005 + 5.005 +
        // 10.01, which is 20.02 and not the 20.03 that 5.01 twice makes it.
        let repriced_then_amended = [
            plan("a", None, 2_501, -day, Some(10 * day)),
            plan("b", Some("a"), 3_003, 10 * day, Some(15 * day)),
            plan("c", Some("b"), 3_003, 15 * day, Some(20 * day)),
            plan("d", Some("c"), 3_003, 20 * day, None),
        ];
        assert_eq!(amounts(&repriced_then_amended, 30), [834, 501, 500, 1_001]);
        // The same after a gap: the plan was off for two days, and what ran
        // after it at one price is still one run of it.
        let off_and_on = [
            plan("a", None, 3_003, -day, Some(8 * day)),
            plan("b", Some("a"), 3_003, 10 * day, Some(15 * day)),
            plan("c", Some("b"), 3_003, 15 * day, Some(20 * day)),
        ];
        // 8 days is 8.008 on its own; then 5.005 and 5.005 come to 10.01.
        assert_eq!(amounts(&off_and_on, 30), [801, 501, 500]);
    }

    #[test]
    fn parts_that_are_not_one_plan_at_one_price_are_each_rounded_alone() {
        let half = 15 * DAY_MS;
        // A price change is two agreements.
        let repriced = [
            plan("a", None, 2_501, -DAY_MS, Some(half)),
            plan("b", Some("a"), 3_001, half, None),
        ];
        assert_eq!(amounts(&repriced, 30), [1_251, 1_501]);
        // Two plans that merely follow one another are two plans.
        let unrelated = [
            plan("a", None, 2_501, -DAY_MS, Some(half)),
            plan("b", None, 2_501, half, None),
        ];
        assert_eq!(amounts(&unrelated, 30), [1_251, 1_251]);
        // A gap between the old terms and the new is time nobody was on the
        // plan, so the parts are not one run of it: 14.5 days is 1,208.82 on
        // its own, where rounded with the first part it would be 1,208.
        let gapped = [
            plan("a", None, 2_501, -DAY_MS, Some(half)),
            plan("b", Some("a"), 2_501, half + DAY_MS / 2, None),
        ];
        assert_eq!(amounts(&gapped, 30), [1_251, 1_209]);
        // Amendments that point at each other in a circle have no first
        // terms to belong to. Looking for one ends, and each is charged alone.
        let circular = [
            plan("a", Some("b"), 2_501, -DAY_MS, Some(half)),
            plan("b", Some("a"), 2_501, half, None),
        ];
        assert_eq!(amounts(&circular, 30), [1_251, 1_251]);
    }
}
