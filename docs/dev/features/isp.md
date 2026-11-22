# ISP: subscribers, bandwidth and money

The feature that makes Nocturne worth paying for, and the only one that is
not in the free edition. It is for a wireless or fibre ISP who sells
bandwidth by the megabit — typically over point-to-point links — and needs to
know who their customers are, what each was sold, what each owes, and whether
what they are actually using matches what they are paying for.

It is **bookkeeping, not control.** `nocturne-isp` reads the network through
the other features' RPC methods and keeps its own ledger. It never configures
a device: suspending a customer for non-payment is a decision a person makes
on the router, and Nocturne's job is to tell them who is in arrears, not to
cut anybody off. That is a deliberate limit — a billing bug that disconnects a
paying customer is a worse failure than a late suspension.

## Money

The part that is easiest to get catastrophically wrong, so it is settled here
before anything else.

**Every amount is an integer in the currency's minor unit, with the currency
beside it.** Never a float: `0.1 + 0.2` is not `0.3`, and a rounding error in
a ledger is indistinguishable from theft when somebody audits it.

```rust
pub struct Money {
    /// Whole minor units. Never fractional, never floating point.
    pub minor: i64,
    pub currency: Currency,
}
```

The minor unit is per currency and is **not** always hundredths:

| Currency | Minor digits | 1,000 units is | Why |
|---|---|---|---|
| `USD` | 2 | `minor = 100_000` | cents |
| `IQD` | 0 | `minor = 1_000` | the fils is long out of circulation; dinars are whole |

A currency this table does not know is refused when it is entered, not
guessed at with two digits.

### Customers are billed in their own currency

Each customer carries the currency they are billed in; some in USD, some in
IQD, as the owner asked for. Two consequences, and they are not negotiable:

1. **An amount in one currency is never silently converted to another.** Not
   for a total, not for a chart, not for a sort.
2. **A total across customers of different currencies is therefore not a
   number.** It is one number per currency. Any report that shows "revenue"
   shows it per currency, or it shows a converted figure *and* states the rate
   and the date it used — labelled as a conversion, never as the ledger.

This is the project's "no fabricated data" rule (`CLAUDE.md`) applied to
money. A single mixed total is an invented figure, and an invented figure
about money is the one a person will act on.

### When a conversion really happens

Only when a customer pays in a currency other than the one they were invoiced
in. Then the rate is recorded **on that payment**, at the moment it is taken:

```rust
pub struct Conversion {
    pub from: Currency,
    pub to: Currency,
    /// Minor units of `to` per *one major unit* of `from`, as entered.
    pub rate: i64,
    pub at: i64,
}
```

The rate is stored, never looked up again. A receipt from March must still
reconcile in November, and it cannot if the figure is recomputed at today's
rate. There is no rate feed and no default rate: whoever takes the payment
enters what they actually got.

## What it keeps

| | |
|---|---|
| **Customer** | Who they are, how to reach them, where the service is, what state they are in (`active`, `suspended`, `cancelled`), and — the join to the rest of Nocturne — the device and interface that serves them |
| **Tariff** | A named plan: how many megabits, the price (per megabit or flat), the currency, the billing period |
| **Subscription** | A customer on a tariff from a date, holding **its own copy** of the agreed megabits and price |
| **Invoice** | A period, its lines, its total, its currency, when it is due, and its state |
| **Payment** | What was actually received: amount, currency, date, method, reference, which invoice it settles, and a `Conversion` if the currency differs |
| **Credit note** | An amount a customer no longer owes, with a **required reason**, its own `CN-` number, and the invoice it credits — or the account, until it is set against an invoice. Not a payment and not an edit |

A subscription copies the price it was agreed at rather than pointing at the
tariff. Raising a tariff must not rewrite what somebody was charged last
year, and an issued invoice is never recalculated — if it was wrong, it is
voided and a new one issued, so the trail shows what happened.

## What it works out

The part an ISP cannot get from a spreadsheet, because it needs the network:

* **Used against sold.** A subscription says 10 Mbit; `monitor`'s interface
  rates say what the customer is actually pulling. Both numbers, and the
  period they cover — a customer at 140% of their plan and a customer at 4%
  are two different conversations.
* **Oversubscription.** What an upstream link or a tower has been sold,
  against what it can carry. Stated as a ratio with both figures, so 3:1 on a
  link that is never busy reads differently from 3:1 on one that saturates
  nightly.
* **Margin.** With a wholesale cost per megabit recorded, what a customer or a
  tower earns after what the bandwidth cost. Per currency, for the reason
  above.
* **Arrears.** Who owes what, and for how long, aged into periods. Per
  currency.

Every one of these is reported with its coverage. A customer whose serving
interface has never been collected has **no** usage figure, not a zero — the
difference between "using nothing" and "we have not looked" is the whole
point, and in a billing dispute it is the difference between a fact and a
guess.

## Rules

* **Nothing here configures a device.** No exceptions; this crate has no
  write path to the network at all.
* **A payment is never invented.** Nothing marks an invoice paid except a
  recorded payment, and nothing adjusts a balance except a recorded entry.
* **Every mutation is audited** (`audit(...)`), because this is money.
* **A customer's personal details are not secrets but are not public either.**
  They never reach a model: `nocturne-ai::redact` is for device
  configurations, so the assistant's tools expose counts and states, never a
  name, a phone number or an address.
* **Deleting a customer does not delete their ledger.** A customer is
  `cancelled`; their invoices, payments and credit notes stay, because
  somebody will ask about them.
* **A check and the write it guards are one transaction.** Two requests can
  arrive at the same instant, and "this period is not billed yet" has to
  still be true when the invoice is written.
* **A day is the business's day.** A billing period is a calendar month, an
  invoice falls due on a day, and it is overdue when that day is over. Where
  midnight is, is a setting of the ISP's — never UTC by default of thought,
  and never the zone of whichever browser is open.

---

# What was built

Everything above is the contract and none of it is weakened below. This part
records what `crates/nocturne-isp` actually does, so the UI and the assistant
have something exact to be written against.

## RPC (contract)

Every method is `App::rpc` only, every mutation calls `audit(...)`, and
nothing here touches a device.

| Method | Params | Result |
|---|---|---|
| `isp.currencies` | — | the accepted codes, their minor digits, their scale, and what 1,000 minor units of each is |
| `isp.summary` | — | counts, states, per-currency arrears, usage coverage — **and no personal detail**, which makes it the one method fit to put in front of a model |
| `isp.customers` | `{query?, state?, currency?, tower?, device_id?, limit?, cursor?}` | `{customers, matching, next_cursor}` |
| `isp.customer.get` | `{id, invoices?}` | the customer, their subscriptions, their invoices, their balance per currency (`outstanding`, `on_account`, `credit_on_account`, `overpaid` — none netted against another), and their usage row |
| `isp.customer.save` | `{id?, code?, name?, currency?, phone?, email?, address?, site?, device_id?, interface?, tower?, location?, tags?, notes?}` | `{customer, created, warnings}` |
| `isp.customer.state` | `{id, state, reason?}` | `{customer, was, device_changed: false, note}` |
| `isp.customer.delete` | `{id}` | refused (`conflict`) once there is one invoice, payment or credit note, void ones included |
| `isp.tariffs` | — | each tariff with its period price and how many subscriptions were agreed on it |
| `isp.tariff.save` | `{id?, name?, description?, mbit?, unit?, price?, period?}` | `{tariff, period_price, created, subscriptions_unaffected}` |
| `isp.tariff.delete` | `{id}` | refused while an active subscription is on it |
| `isp.subscriptions` | `{customer_id?, tariff_id?, state?}` | each with its monthly-equivalent price, and whether that division was exact |
| `isp.subscription.create` | `{customer_id, tariff_id, mbit?, price?, unit?, period?, started_at?, note?}` | `{subscription, ended: null, note}` — the agreed figures are copied in |
| `isp.subscription.amend` | `{id, mbit?, price?, from_at?, note?}` | `{subscription, ended, note}` — the old terms end, the new ones start. Refused (`invalid`) for a negative price, like a new subscription, and for a `from_at` before the subscription started. With no `from_at`: now, or the plan's start if it has not begun (decision 19) |
| `isp.subscription.end` | `{id, ended_at?, reason?}` | the ended subscription. Refused (`invalid`) for an `ended_at` before it started. With no `ended_at`: now, or the plan's start if it has not begun (decision 19) |
| `isp.links` | — | the upstream links and tower feeds |
| `isp.link.save` | `{id?, name?, description?, device_id?, interface?, tower?, capacity_mbit?, cost_per_mbit?, clear_cost?, cost_period?}` | `{link, created}`. A field left out keeps what is stored, so taking a figure off again is said outright: `capacity_mbit: 0` for the capacity, `clear_cost: true` for the wholesale cost (refused, `invalid`, together with `cost_per_mbit`). No cost is no margin, not a margin against a cost of nothing |
| `isp.link.delete` | `{id}` | `{deleted, name}` |
| `isp.invoices` | `{customer_id?, state?, currency?, since?, until?, limit?, cursor?}` | invoices with their derived settlement. `since`/`until` select invoices whose period overlaps `[since, until)`: a period is half-open, so one month's bounds list that month and not the one before |
| `isp.invoice.get` | `{id}` | the invoice, its status, what is paid, what is outstanding, its payments |
| `isp.invoice.draft` | `{customer_id, month \| period_start + period_end, allow_empty?}` | `{invoice, skipped, warnings}`. `month` is `"2025-10"`, a month of the business's calendar (decision 13) |
| `isp.invoice.run` | `{month \| period_start + period_end, customer_ids?, dry_run?, issue?}` | `{period_start, period_end, utc_offset_minutes, dry_run, considered, drafted, issued, invoices, skipped, totals, warnings, note}`: a per-customer preview (default) or the invoices it wrote, everyone left out and why, and the totals per currency. Every active customer, however many (decision 16) |
| `isp.invoice.line.add` | `{invoice_id, description, amount}` | drafts only |
| `isp.invoice.line.remove` | `{invoice_id, line_id}` | drafts only |
| `isp.invoice.issue` | `{id, due_at?, issued_at?}` | the frozen invoice, with its number. `due_at` is read as a day and may be the day of issue (decision 15). Refused (`invalid`) when the lines come to less than nothing (decision 18) |
| `isp.invoice.void` | `{id, reason}` | the void invoice; the reason is required. Refused (`conflict`) while a recorded payment or an issued credit note stands against it |
| `isp.invoice.reissue` | `{id, reason}` | `{voided, draft, note}`; refused whenever the void would be |
| `isp.payments` | `{customer_id?, invoice_id?, since?, until?, state?, limit?, cursor?}` | the payments, void ones included |
| `isp.payment.record` | `{customer_id, invoice_id?, amount, method, reference?, note?, at?, conversion?}` | `{payment, invoice, warnings}` |
| `isp.payment.void` | `{id, reason}` | the void payment; it is never deleted |
| `isp.credits` | `{customer_id?, invoice_id?, since?, until?, state?, limit?, cursor?}` | the credit notes, newest first |
| `isp.credit.issue` | `{customer_id, invoice_id?, amount, reason, note?, at?}` | `{credit, invoice?, warnings}` |
| `isp.credit.allocate` | `{id, invoice_id}` | `{credit, invoice, warnings}` — a note that was issued on the account, set against one invoice; once only |
| `isp.credit.void` | `{id, reason}` | the void credit note; the debt it cancelled comes back |
| `isp.arrears` | `{customer_id?}` | one aged report **per currency**, plus who owes what, and — never netted against it — `overpaid` per currency and `customers_overpaid`, who is in credit |
| `isp.revenue` | `{since, until?, convert?}` | issued and received per currency, what is outstanding, and a converted figure only if the caller supplied the rates |
| `isp.usage` | `{customer_id? \| filters, since?, until?}` | used against sold, with coverage per row and counted per reason |
| `isp.capacity` | `{link_id?, tower?, since?, until?}` | sold against capacity as a ratio with both figures, and what is measured |
| `isp.margin` | `{link_id?, tower?, customer_id?}` | revenue per currency, the wholesale cost, and the margin **in the cost's currency only** |

Events: `isp.changed {kinds}` for the records (`customers`, `tariffs`,
`subscriptions`, `links`), `isp.invoices.changed {change, ids, customer_ids}`
(`drafted`, `issued`, `voided`, `updated`; the `issued` one also carries
`currency`, `total_minor` and `due_at`),
`isp.payments.changed {change, ids, customer_ids, invoice_ids}`
(`recorded`, `voided`) and
`isp.credits.changed {change, ids, customer_ids, invoice_ids}`
(`issued`, `allocated`, `voided`). Reads publish nothing.

A real billing run publishes `isp.invoices.changed` once for the drafts it
left (`drafted`) and once for the invoices it issued (`issued`), and neither
for an empty list.

A credit note changes what an invoice or a balance reads, so each of its
three mutations publishes a second event on a topic the screens already
follow: `isp.invoices.changed {change: "updated"}` naming the invoice the
note stands against, or — for a note on the account, where no invoice moved —
`isp.changed {kinds: ["credits"]}`. A screen that refetches on `isp.changed`,
`isp.invoices.changed` and `isp.payments.changed` therefore needs nothing
new to stay right.

Settings section `isp`: `payment_terms_days` (14), `arrears_buckets`
(`[30, 60, 90]`), `usage_basis` (`greater`), `usage_window_hours` (24),
`invoice_number_prefix` (`""`), `utc_offset_minutes` (`0`, `-720..=840`: the
business's calendar, decision 13). They are read and saved with the core's
`settings.get` / `settings.set {section: "isp"}`, which takes any value; the
offset's range is therefore checked where it is used, and while it is out of
range every method that bills, ages or reads an invoice is refused
(`invalid`) with a message naming the setting. In the interface they are the
ISP screen's own Settings tab, where the calendar comes first and one button
sets it to the offset of the computer the page is open on.

Storage: `isp_customer`, `isp_subscription`, `isp_invoice`, `isp_payment` and
`isp_credit` are this feature's own tables, created with `CREATE TABLE IF NOT
EXISTS` through `store.with` (a feature cannot add a core migration). Tariffs and
links are document collections. **No money column is a REAL**, every amount
is an INTEGER of minor units with its currency in a column beside it, and a
test reads the schema back to keep it that way. TypeScript:
`ui/src/api/types/isp.ts`.

## Decisions this needed, that the contract did not settle

1. **A rate is applied in whichever direction it was quoted.** `Conversion`
   is exactly as specified, and `rate` is minor units of `to` per one major
   unit of `from`. A dinar payment against a dollar invoice is therefore
   recorded as `{from: USD, to: IQD, rate: 1320}` — the rate a person
   actually quotes — and the settled amount is worked out by **dividing**:
   `33,000 IQD × 100 ÷ 1,320 = 2,500` minor USD, exactly. Storing the
   opposite direction instead would need 0.00076 USD per dinar, which in an
   integer rate is zero, and the payment would settle nothing. So
   `Conversion::apply` multiplies when the amount is in `from` and divides
   when it is in `to`, and refuses an amount in a third currency. What the
   payment settles is computed once, at that moment, and stored on it
   (`settled`, `settled_exact`); nothing recomputes it.
2. **An invoice may be issued with a date in the past** (`issued_at`), never
   in the future. An ISP migrating a year of paper invoices into Nocturne
   would otherwise have to enter them as if they were new, which is a worse
   lie than a backdated record. `due_at` may not fall on a day before the
   day of issue (decision 15).
3. **A billing window must be one whole period of a subscription, or that
   subscription is not charged.** A monthly plan is billed in a window of
   28–31 days, a quarterly one in 89–92, a yearly one in 365–366. A
   seven-day window produces no line for a monthly subscription; the draft
   lists it in `skipped` with the reason, and says what to do instead (bill a
   whole period, or add the figure by hand). Nothing here divides a monthly
   price by an invented average month. Within a valid window a part period is
   pro-rated by the milliseconds covered, and the line carries that
   arithmetic (`prorated`) so the figure can be rechecked. **The parts of one
   plan are rounded together.** A plan whose megabits were amended part of
   the way through a window at the same price is two subscriptions and two
   lines, and rounding each alone can make them come to a minor unit more
   than the price that was agreed (12.51 + 12.51 for a plan of 25.01). Each
   such part is instead what the plan has cost up to the end of that part,
   less what the earlier parts were charged: the lines add up to exactly what
   the plan would have cost un-amended, no part is more than one minor unit
   from its own share, none is negative, and each line's `basis` says so.
   Parts at different prices are different agreements, with no single figure
   to add up to, and are each rounded alone. What is rounded together is a
   *run*: parts of one plan that follow one another with no time between
   them, at one price. A change of price or a gap ends the run and the next
   begins after it, so a plan repriced on the 11th and given more megabits
   on the 16th is one part at the old price and one run at the new.
4. **Voiding an invoice that has a recorded payment or an issued credit note
   against it is refused**, and so is reissuing it, which begins with the
   void. With a payment, the void would hide money that really came in. With
   a credit note, it would leave a numbered document the customer was given
   issued against nothing: what it wrote off would stop counting in every
   figure, with no reason recorded. Each is voided first, with its own
   reason, and every record stays. A credit that should carry over to a
   replacement invoice is issued against the replacement once that is
   issued; nothing moves it there silently.
5. **A payment with no invoice is money on account**, kept in the currency it
   arrived in and reported as `on_account` in the customer's balance. A rate
   on such a payment is refused: there is nothing to convert it towards yet.
6. **Which direction is "usage" is a setting** (`usage_basis`: `greater`,
   `in`, `out`), because it depends on which side of the link the serving
   interface is, and every figure says in `basis` which it used. A usage
   figure is `null` unless `coverage == "measured"`, and `Coverage`
   distinguishes nine states, including the one that matters most in a
   dispute: `interface_never_collected` ("nobody has looked") against
   `no_rate_yet` ("the interval could not be measured").
7. **Margin is only worked out in the currency the wholesale cost is in.**
   Revenue behind a tower can be in several; the others are returned
   untouched in `unconverted`, with a note saying so, rather than converted
   at a rate nobody entered.
8. **`IQD` has 0 minor digits**, as this specification settles, which is a
   deliberate departure from ISO 4217's 3. The currency table
   (`money::CURRENCIES`) is a fixed list of 21 codes including three-digit
   ones (`JOD`, `KWD`, `BHD`, `OMR`) so that nothing can assume hundredths;
   currencies whose everyday practice and ISO's minor unit disagree the way
   the dinar's does are deliberately absent rather than guessed at. Adding one
   is a line in that table and a decision recorded beside it.
9. **A customer's currency cannot be changed once they have a ledger or a
   plan.** The ledger is their invoices, payments and credit notes — a
   credit note on the account is a numbered document in the old currency
   like any other. It can be corrected before anything is billed or agreed;
   afterwards it is a new account, because otherwise one customer's books
   would hold two currencies with nothing saying which figure was which. A
   subscription, running or ended, blocks the change as well: a plan is
   invoiced in the currency it was agreed in, so a customer whose currency
   moved from under their plan would be left out of every billing run. A
   customer who has a plan and no ledger can be deleted and created again.
10. **`isp.summary` is the assistant's method.** It carries counts, states and
    per-currency money and no name, code, phone number, email address, street
    or note; a test fails if any of those appear in it. An assistant tool must
    be registered against it (and against nothing else in this feature that
    returns a customer).
11. **Every check-then-write is one transaction** (`db::atomically`, over the
    core's `Store::transaction`). `App::rpc` runs handlers on several threads,
    so two operators — or one request and its retry — can land at the same
    instant. Drafting (and each customer of a billing run), adding or
    removing a line, issuing, voiding, reissuing, recording or voiding a
    payment, issuing, allocating or voiding a credit note, deleting a
    customer, changing their currency or state, and creating, amending or
    ending a subscription each read what they check and write what they
    decide with nothing able to run in between; the store has one connection
    and the transaction holds it. An invoice or credit note number is taken
    inside the same transaction as the document that carries it, so a call
    that is refused takes no number with it and the sequences have no holes.
    Of two calls that collide, one succeeds and the other is refused with
    the ordinary message (`conflict`: already billed, already issued, already
    void; `invalid`: more than was ever charged) — the second is never told a
    number that no document has. Tariffs and links are documents in the
    core's collection and are not covered: a subscription copies a tariff's
    figures when it is created, so nothing in the ledger depends on one
    afterwards.
12. **Money in a customer's favour is reported, and never netted.** An issued
    invoice settled beyond its total — a payment larger than the invoice, or
    a credit note on one already paid — leaves the difference standing to the
    customer's favour. That figure is `overpaid` in the customer's balance
    (with `invoices_overpaid`) and in `isp.arrears` (`overpaid` per currency,
    `customers_overpaid` per customer), in the invoice's currency. It is not
    subtracted from `outstanding` anywhere: nothing has set it against
    another invoice, and a report that netted it off would be asserting an
    allocation nobody made. The same holds for the two on-account figures.

13. **The business has one calendar, and it is a setting.**
    `utc_offset_minutes` is how far the business's clock is ahead of UTC, in
    minutes: `180` for Baghdad, `-240` for UTC−4, `0` by default. It is a
    **fixed offset, not a zone: daylight-saving rules are not modelled.** A
    business that moves its clocks keeps its books on standard time all
    year, or changes the setting when the clocks change. It decides four
    things, and nothing else in this feature asks "which day?" of anything
    but it (`calendar::Calendar`):
    * **where a month begins and ends.** `month: "2025-10"` on
      `isp.invoice.draft` and `isp.invoice.run` is October from the first
      moment of the 1st in that calendar to the first moment of 1 November.
      Naming the month is the way that cannot go wrong; a caller who sends
      `period_start`/`period_end` instead is taken at their word, and those
      have to be instants of the same calendar or a plan that "starts on the
      1st" is billed three hours of the month before (east of UTC) or loses
      the first hours of its own (west);
    * **which day a due date falls on**, when an invoice is issued;
    * **what today is**, when something asks whether that day is over
      (decision 15);
    * **which year a number is counted in**: an invoice issued, or a credit
      note agreed, in the first hours of 1 January is numbered in the new
      year wherever UTC still is.

    **An invoice remembers the calendar it was written in**
    (`utc_offset_minutes` on the invoice, set when it is drafted; `0` on
    invoices from before the setting, which is how they were billed). Its
    period, its lines' periods, its issue date and its due date are days of
    *that* calendar for as long as it exists, so changing the setting moves
    no date on a document a customer has already been given. A reissued
    invoice's replacement keeps the original's.

    For the interface, which must use the same offset for what is typed and
    what is shown: a typed date `Y-M-D` is the instant
    `Date.UTC(Y, M-1, D) - utc_offset_minutes * 60000`; an instant is shown
    by adding `utc_offset_minutes * 60000` and reading the UTC fields. The
    offset is the setting for anything new (a subscription's start, a
    payment's date, a report's window, a month) and the invoice's own for
    the dates on an existing invoice.
14. **"Already billed" is the same days, not the same milliseconds.** A
    draft is refused (`conflict`, naming the invoice and its dates) when the
    customer already has an invoice that is not void whose period is
    identical, or is **the same kind of period and overlaps** — a window a
    millisecond, a day or a fortnight away from one already billed covers
    the same days. The kind is what decision 3 already settles (monthly
    28–31 days, quarterly 89–92, yearly 365–366; the ranges do not meet):
    a subscription is only ever charged in a window of its own kind, so two
    windows can charge the same plan twice only when they are the same kind
    and overlap. A quarter therefore overlaps its three monthly invoices
    freely, and bills only the quarterly plan. A window that is no kind of
    period charges no plan at all, and only an identical one is refused.
    Both periods are compared **as days of the calendar each was written
    in** (decision 13): October billed under one offset is still October
    after the offset is changed, and the November that follows — whose first
    hours were, in milliseconds, inside the old October — is not, and is
    billed in full. The check and the write are still one transaction
    (decision 11).
15. **A due date is a day, and an invoice is overdue when that day is
    over.** Not at the instant in `due_at`, and not on the due day at any
    hour: an invoice due on the 15th reads `issued` throughout the 15th and
    is `overdue`, by one day, throughout the 16th. The due day is the day
    `due_at` falls on in the invoice's own calendar; "today" is the
    business's calendar as it is now set. One function answers the question
    (`Calendar::days_overdue`) and the invoice's `status` and
    `days_overdue`, the arrears report's `overdue`, `not_yet_due`, bands and
    `oldest_days`, and `isp.summary`'s `invoices.overdue` all ask it, so
    they cannot disagree about whether an invoice is late or by how many
    days. `due_at` defaults to whole days after `issued_at`
    (`payment_terms_days`); a caller's `due_at` may be any instant of the
    day meant, may be the day of issue, and is refused (`invalid`) on a day
    before it.
16. **A billing run reaches every customer or says whom it did not.** With
    no `customer_ids` it reads every active customer — not a page of them —
    and each is either in `invoices` or in `skipped` with the reason:
    `considered` is how many it looked at and the two lists always add up to
    it. A customer whose plans were all in force and could not be charged
    (a yearly plan in a monthly window) is told *that* reason, in the
    message and as `skipped_subscriptions`, and never that "none of them
    [were] in force"; that sentence is kept for an account with no plan in
    force in the period. Once the first draft is written nothing stops the
    run before its audit entry: a draft that could not be issued stays in
    `invoices` as a draft with `not_issued` saying why, and anything about
    the run as a whole is in `warnings`. A customer named twice in
    `customer_ids` is billed once. Suspended and cancelled accounts are not
    considered unless named.
17. **A figure nobody could have meant is refused where it is entered.** One
    entry may be at most 1,000,000,000,000,000 minor units either side of
    zero (`money::ENTRY_LIMIT_MINOR`: ten million million dollars), and one
    plan or link at most 1,000,000,000 Mbit/s (`model::MBIT_LIMIT`). That
    covers a payment and what it settles after conversion, a credit note, a
    line and the invoice total it makes, a tariff's price, an agreed price
    and the period price it multiplies out to, and a wholesale cost;
    refusals are `invalid`. The reason is what such a row does afterwards:
    balances, arrears and revenue are sums over the ledger. Those sums are
    also taken so that they **cannot overflow in the database** — each in
    two halves that are put back together in Rust (`db::halves`,
    `db::whole`) — so that rows from before the limit, or more rows than a
    ledger has, end in the ledger's own refusal ("overflows the ledger's
    64-bit minor units") and never in an internal database error. The limit
    applies to what comes in, never to what is read back.
18. **An invoice never reads `paid` without a payment, and cannot be issued
    for less than nothing.** `paid` needs money to have arrived, so an
    issued invoice whose lines come to exactly nothing (a month given away
    as a line on the invoice) is `issued`, is never overdue, and is in no
    arrears figure; `credited` likewise needs a credit note. A draft whose
    lines come to *less* than nothing is refused at issue (`invalid`): an
    invoice asks for money, and what a customer is owed is a credit note,
    which carries its reason.
19. **A plan never covers less than no time.** `isp.subscription.end` with
    an `ended_at` before the plan's `started_at`, and `isp.subscription.amend`
    with a `from_at` before it, are refused (`invalid`); the first moment of
    the plan is allowed in both — a plan that never ran, and terms corrected
    from the very start. An amended price obeys the rule a new one does and
    cannot be negative. **With no date given, both mean now, or the plan's
    start if that is still to come.** A plan sold today to start on the
    first of next month has not begun: ending it "now" would end it before
    it started, so with no `ended_at` it ends where it would have begun, and
    with no `from_at` the new terms replace the old from that same moment.
    Only a date the caller actually gave is refused for being too early. A
    plan that ended at the moment it started is not in force in any period:
    a draft neither charges it nor lists it in `skipped`, whatever kind of
    plan it was, so a billing run does not say of it that a plan "in force"
    could not be charged (decision 16).

## Tax, refunds and credit notes

Three gaps were left open deliberately rather than guessed at. Two of them
are now settled, and the settlement is as much a part of the contract as
anything above.

**There is no tax, and that is the decision — not a gap waiting to be
filled.** An invoice's total is its lines, and that is the amount owed.
Nothing here computes, holds or reports a tax figure, and nothing should be
added that does without the owner asking for it: a tax model invented on
somebody's behalf is worse than no tax model, because an operator cannot
tell by looking at a total whether the number was reasoned about or made up.
Where tax applies it is handled in whatever the ISP files their returns
with, and an invoice from here is an input to that.

**There are no refunds.** Nothing in this crate says money went back out of
the business, because it has no way to know whether it did and no business
asserting it. A payment recorded in error is voided, with a reason.

**Credit notes exist** (`isp.credit.*`), and they are a document rather than
a correction:

* An issued invoice's figures are still the figures. A credit note stands
  beside them and reduces what is owed; it never rewrites a total.
* **A reason is required.** An amount a customer stopped owing with no reason
  recorded cannot be told from a mistake six months later.
* It is written in the currency the customer is billed in, always. No money
  moves, so there is no rate anybody actually got, and converting here would
  put an invented figure in the ledger.
* It is **not money.** It lives in its own table rather than as a negative
  payment, because a negative payment would be counted as revenue by every
  report that sums payments. `isp.revenue` does not see it; `isp.payments`
  does not list it; the balance keeps `credit_on_account` apart from
  `on_account`, and nobody can be paid out of the former.
* An invoice that a credit note was needed to clear reads **`credited`, not
  `paid`**. `Paid` is a claim that the customer's money arrived for all of
  it, and a write-off is not that claim. The word turns on the money alone:
  an invoice whose payments cover its total reads `paid` whatever was
  credited besides, so a goodwill credit on a settled invoice does not take
  `paid` away from it. An invoice part credited with nothing paid is still
  `issued` or `overdue`: money is owed and nobody has paid any of it.
* You cannot credit more than was charged. The cap is the invoice's total,
  not what is left outstanding, so crediting an invoice that has already been
  paid is allowed — a real case — and leaves the customer in credit, with a
  warning saying so and the amount reported as `overpaid` (decision 12).
* **A note on the account reduces nothing until it is set against an
  invoice**, and `isp.credit.allocate {id, invoice_id}` is how. It is the
  same document — same number, amount, reason and date — now standing
  against one issued invoice of the same customer, under the same cap as a
  note issued against it directly; `allocated_at` on the note records when,
  so the note does not read as if the invoice had been credited on the day
  the credit was agreed. It happens once. Moving a note from one invoice to
  another is a void, with its reason, and a new note, so the trail shows
  both. (Nothing allocates a *payment* on account to an invoice yet.)
* It is never deleted. Voiding one needs its own reason, and puts the debt
  back.
* An invoice cannot be voided or reissued while an issued credit note stands
  against it (decision 4).

## Tests

`cargo test -p nocturne-isp`: unit tests beside the code they test, and the
rest through `App::rpc` (`tests/money.rs`, `tests/ledger.rs`,
`tests/credits.rs`, `tests/calendar.rs`, `tests/billing.rs`,
`tests/concurrency.rs`, `tests/network.rs`, `tests/rpc.rs`,
`tests/boundaries.rs`). They build real state through the RPC surface, never
by writing to the store. `tests/concurrency.rs` makes two calls at the same
instant from two OS threads released by a barrier — which is what two HTTP
requests are; two tasks on one runtime worker take turns and prove nothing —
and runs each race forty times, because a losing interleaving is not certain
on any one attempt. The monitor is reached by name and stubbed in
`tests/common/mod.rs` with the shape the real one answers — including a
`null` rate for an interval that could not be measured — because a feature
crate may not link another feature crate; one fixture deliberately has no
monitor at all. Nothing waits on real time or asserts a duration. The tests
of what is overdue run against the real clock, so they first set a calendar
in which it is midday: no test can then straddle a midnight, and the offset
is rarely zero. The sums that must not overflow are tested in `db.rs` against
rows written straight to the tables, because the RPC surface now refuses to
create them.
