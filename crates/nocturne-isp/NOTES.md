# nocturne-isp — working notes

Specification: `docs/dev/features/isp.md`. The "What was built" half of that
file is the contract as it now stands; this is the log.

## Done

* `money.rs` — `Currency` (21 codes from a fixed table, per-currency minor
  digits, `IQD` = 0), `Money` (i64 minor units + currency, no float in, no
  float out), exact decimal parsing, `plus`/`minus`/`times`/`scale_by`
  returning `AppResult` instead of operators, `Conversion` (direction-aware,
  stored, never recomputed), `Totals` (one figure per currency, no scalar) and
  `convert_totals` → a `Converted` labelled as a conversion with its rates.
* `model.rs` — `Customer`, `Tariff`, `Subscription` (its own copy of the
  agreed figures), `Invoice`/`InvoiceLine`/`Proration`, `Payment`, `Link`,
  `Settings`.
* `db.rs` — four tables of this feature's own (`CREATE TABLE IF NOT EXISTS`
  via `store.with`; a feature cannot add a core migration), two document
  collections, paged queries, the shortfall join the arrears report is made
  of, and the per-year invoice-number counter in a transaction.
* `records.rs` — customers, tariffs, subscriptions, links. `device_id` is
  checked against the inventory. A customer's currency is fixed once they have
  a ledger. A tariff somebody is on cannot be deleted.
* `billing.rs` — draft (and `build`, which writes nothing, for the preview),
  issue (freezes, numbers), void (reason required, refused with payments
  against it), reissue, lines on drafts, payments with their conversion, and
  the derived `InvoiceStatus`/`paid`/`outstanding`.
* `reports.rs` — arrears aged per currency, revenue per currency with an
  optional caller-supplied conversion, and the personal-detail-free summary.
* `network.rs` — used against sold, oversubscription, margin. Reaches
  `monitor.interfaces` and `metrics.query` **by name** and copes with
  `unknown_method`; `Coverage` keeps nine kinds of "no figure" apart from a
  zero.
* `api.rs` — 34 methods, three events, settings. No background task: nothing
  here needs a clock, and a watcher would only have invented one.
* `ui/src/api/types/isp.ts`; `cargo test`, `clippy -D warnings` and `fmt`
  clean.

## Deliberately not done

* **Not registered in `nocturne-suite`.** The edition split is the lead's; the
  free edition must not contain this crate. `nocturne_isp::init(&mut builder)`
  is the whole of the wiring when the paid edition is assembled.
* **No assistant tool.** `nocturne-assistant` is another crate and not mine to
  edit. `isp.summary` exists to be the only tool target: counts, states and
  per-currency money, no personal detail. A tool that returns a customer row
  would put a name and a phone number in a model's context.
* **No write path to the network, of any kind.** Suspending a customer sets a
  field and returns `device_changed: false`. `tests/boundaries.rs` scans the
  crate's own source for `connect::`, `nocturne_drivers`, `session()`,
  `changes.create` and friends, so a later change cannot quietly add one.
* **No credit notes, no refunds, no tax.** A wrong invoice is voided and
  reissued, and a wrongly recorded payment is voided; a negative payment is
  refused. Tax (VAT, withholding) is not in the contract and inventing a model
  for it would be worse than not having one. Both are the obvious next
  additions and both need a decision from the owner first.
* **No automatic billing run.** `isp.invoice.run` is a method; whatever calls
  it monthly belongs in `nocturne-automation`, which already has a schedule.

## Next, if this is picked up again

* Allocating a payment on account to an invoice afterwards
  (`isp.payment.allocate`), which the data model already allows for.
* A statement of account per customer (invoices and payments interleaved), for
  printing.
* Tax, credit notes and refunds, in that order, once the rules are settled.
* `isp.usage` currently reads `monitor.interfaces` once per device and
  `metrics.query` twice per customer for a windowed report. That is fine for a
  page of 200 customers; a whole estate at once would want a batched
  `metrics.query_many`, which the monitor already has.
