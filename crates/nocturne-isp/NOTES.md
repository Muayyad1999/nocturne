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
* `calendar.rs` — the business's calendar: a fixed offset from UTC
  (`utc_offset_minutes` in the settings), the month a name like `2025-10`
  means in it, which day a moment falls on, and `days_overdue`, the one
  answer to "is this late?". Daylight-saving rules are not modelled.
* `db.rs` — five tables of this feature's own (`CREATE TABLE IF NOT EXISTS`
  via `store.with`; a feature cannot add a core migration), two document
  collections, paged queries, the shortfall join the arrears report is made
  of and its mirror for invoices settled beyond their total, and
  `atomically`: one transaction around a check and the write it guards. Every
  query takes `&impl Sql` — the store, or the connection of an open
  transaction — so the same query serves both. The per-year invoice and
  credit-note counters take the transaction's connection, so a number is
  only ever taken together with the document that carries it. Every sum of
  a money column is taken in two halves (`halves`, `whole`), so SQLite's
  "integer overflow" cannot reach a caller as an internal error.
* `records.rs` — customers, tariffs, subscriptions, links. `device_id` is
  checked against the inventory. A customer's currency is fixed once they have
  a ledger. A tariff somebody is on cannot be deleted. A plan cannot be ended
  or amended from before it started; with no date given, "now" becomes the
  plan's start when it has not begun, so the screens' End and Change buttons
  (which send no date) still work on a plan sold for next month.
* `billing.rs` — draft (and `build`, which writes nothing, for the preview;
  both refuse a period that overlaps one already billed, compared as days
  of the calendar each invoice was written in, and round the parts of one
  amended plan together), issue (freezes, numbers, reads the due date as a
  day), void (reason required, refused with payments or
  issued credit notes against it), reissue, lines on drafts, payments with
  their conversion, credit notes (issue, allocate to an invoice, void), and
  the derived `InvoiceStatus`/`paid`/`credited`/`outstanding`. Every function
  that writes is one transaction.
* `reports.rs` — arrears aged per currency, revenue per currency with an
  optional caller-supplied conversion, and the personal-detail-free summary.
* `network.rs` — used against sold, oversubscription, margin. Reaches
  `monitor.interfaces` and `metrics.query` **by name** and copes with
  `unknown_method`; `Coverage` keeps nine kinds of "no figure" apart from a
  zero.
* `api.rs` — 38 methods, four events, settings. No background task: nothing
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
* **No refunds, no tax.** Both are the owner's decision, recorded in the
  specification, and not gaps. A wrong invoice is voided and reissued, a
  wrongly recorded payment is voided, and an amount a customer no longer owes
  is a credit note; a negative payment is refused.
* **The store's lock is not re-entrant.** Inside `db::atomically` the closure
  has the connection and nothing else. Anything that goes through the app or
  the store — the inventory, a tariff or link document, settings, `audit`,
  `publish` — happens before the transaction or after it, never inside; a
  second attempt to take the lock on the same thread does not return.
* **No unique index on (customer, period).** The transaction is the guard. An
  index would refuse to be created on a database that already holds two
  drafts for one period from before the guard was atomic, and that would
  stop the application starting. It could not express the rule now in any
  case: "already billed" is overlap between periods of the same kind, read
  in two calendars, not equality of two columns.
* **No named time zone.** `utc_offset_minutes` is a fixed offset. A zone
  would need a zone database and its updates, and the offset on each invoice
  already keeps old documents' dates where they were when the setting is
  changed, which is all a daylight-saving change needs.
* **Megabit sums are not taken in halves.** Megabits are bounded where they
  are entered (`model::MBIT_LIMIT`), which is enough for anything written
  from now on; the capacity and summary reports still add them with plain
  `i64` arithmetic, so a subscription row from before the limit with an
  absurd figure is not handled the way an absurd amount is.
* **The setting is not validated when it is saved.** `settings.set` is the
  core's and has no hook; `Calendar::of` refuses an offset out of range
  where it is used, so nothing is billed on a calendar that was guessed.
  Changing the calendar is not audited either, for the same reason.
* **No automatic billing run.** `isp.invoice.run` is a method; whatever calls
  it monthly belongs in `nocturne-automation`, which already has a schedule.

## Next, if this is picked up again

* Allocating a payment on account to an invoice afterwards
  (`isp.payment.allocate`), which the data model already allows for.
  `isp.credit.allocate` does this for credit notes; a payment needs the rate
  question answered first when its currency is not the invoice's.
* A statement of account per customer (invoices and payments interleaved), for
  printing.
* The screens send their own month bounds and local-midnight dates
  (`ui/src/pages/isp/billing-rows.ts`). They should send `month` for a run
  and a draft, read `utc_offset_minutes` from `settings.get {section:
  "isp"}`, and convert typed and shown dates with it — the invoice's own
  offset for the dates on an existing invoice. The two formulas are in
  decision 13 of the specification. `ui/src/api/types/isp.ts` also still
  documents `since` as "ends at or after".
* A statement or a billing run for quarterly and yearly plans by name
  (`quarter`, `year`), the way `month` names a month.
* The screens know nothing of credit notes yet: `ui/src/api/types/isp.ts` has
  no `CreditNote`, no `credited`/`credits` on an invoice, no `overpaid` on a
  balance or the arrears report, and no `isp.credit.*` method or
  `isp.credits.changed` event. The void and reissue buttons are offered on an
  invoice with a credit note and are now refused by the server with the
  reason.
* `isp.usage` currently reads `monitor.interfaces` once per device and
  `metrics.query` twice per customer for a windowed report. That is fine for a
  page of 200 customers; a whole estate at once would want a batched
  `metrics.query_many`, which the monitor already has.
