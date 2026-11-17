//! Subscribers, bandwidth and money: the ledger of an ISP that sells
//! megabits.
//!
//! Specification: `docs/dev/features/isp.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! It is **bookkeeping, not control.** This crate reads the network through
//! other features' RPC methods and keeps its own ledger. It never configures
//! a device — there is no code path here that opens a session, creates a
//! change or writes a line of configuration, and suspending a customer for
//! non-payment sets a field on a record and says so. A billing bug that
//! disconnects a paying customer is a worse failure than a late suspension.
//!
//! Three things run through every type and every method, and they are the
//! reason the crate is shaped the way it is.
//!
//! **Money is a whole number of minor units with its currency beside it**
//! ([`money::Money`]). The minor unit is per currency — `USD` has two digits
//! and `IQD` has none, so `1000` is ten dollars or a thousand dinars — and an
//! unknown currency is refused on entry rather than assumed to have two. No
//! float can get in: there is no field for one, no constructor that takes
//! one, and the deserialiser refuses a JSON number with a point in it.
//!
//! **A total across currencies is not a number** ([`money::Totals`]). It is
//! one figure per currency, with no scalar accessor anywhere. A caller who
//! wants a single figure supplies the rates and gets an answer labelled a
//! conversion, carrying those rates and their date. The one place a
//! conversion really happens is a payment in a currency other than the
//! invoice's, and then the rate is recorded on that payment and never looked
//! up again, so a receipt from March still reconciles in November.
//!
//! **Missing is not zero.** A subscription is what was agreed, and holds its
//! own copy of the figures so that changing a tariff cannot rewrite history.
//! An issued invoice is frozen; wrong means voided and reissued. A customer
//! whose serving interface has never been collected has *no* usage figure —
//! [`network::Coverage`] says which of the several different nothings it is —
//! and every derived report states its coverage, because in a billing dispute
//! that is the difference between a fact and a guess.
//!
//! | | written by | lives in | changed by |
//! |---|---|---|---|
//! | **records** | a person | [`model::Customer`], [`model::Tariff`], [`model::Subscription`], [`model::Link`] | `isp.customer.*`, `isp.tariff.*`, `isp.subscription.*`, `isp.link.*` |
//! | **the ledger** | a person, once | [`model::Invoice`], [`model::Payment`], [`model::CreditNote`] | `isp.invoice.*`, `isp.payment.*`, `isp.credit.*` — issue, void, record; never recalculate |
//! | **the network** | devices | nothing here | nothing: it is read through `monitor.*` at the moment a report is asked for |

pub mod api;
pub mod billing;
pub mod db;
pub mod model;
pub mod money;
pub mod network;
pub mod records;
pub mod reports;

/// Register this feature's settings, tables and RPC methods.
///
/// This feature is **not** part of the free edition and is deliberately not
/// registered by `nocturne-suite`; whoever assembles the paid edition calls
/// this.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    api::init(builder);
}
