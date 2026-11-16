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
  `cancelled`; their invoices and payments stay, because somebody will ask
  about them.

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
| `isp.customer.get` | `{id, invoices?}` | the customer, their subscriptions, their invoices, their balance per currency, and their usage row |
| `isp.customer.save` | `{id?, code?, name?, currency?, phone?, email?, address?, site?, device_id?, interface?, tower?, location?, tags?, notes?}` | `{customer, created, warnings}` |
| `isp.customer.state` | `{id, state, reason?}` | `{customer, was, device_changed: false, note}` |
| `isp.customer.delete` | `{id}` | refused (`conflict`) once there is one invoice or payment |
| `isp.tariffs` | — | each tariff with its period price and how many subscriptions were agreed on it |
| `isp.tariff.save` | `{id?, name?, description?, mbit?, unit?, price?, period?}` | `{tariff, period_price, created, subscriptions_unaffected}` |
| `isp.tariff.delete` | `{id}` | refused while an active subscription is on it |
| `isp.subscriptions` | `{customer_id?, tariff_id?, state?}` | each with its monthly-equivalent price, and whether that division was exact |
| `isp.subscription.create` | `{customer_id, tariff_id, mbit?, price?, unit?, period?, started_at?, note?}` | `{subscription, ended: null, note}` — the agreed figures are copied in |
| `isp.subscription.amend` | `{id, mbit?, price?, from_at?, note?}` | `{subscription, ended, note}` — the old terms end, the new ones start |
| `isp.subscription.end` | `{id, ended_at?, reason?}` | the ended subscription |
| `isp.links` | — | the upstream links and tower feeds |
| `isp.link.save` | `{id?, name?, description?, device_id?, interface?, tower?, capacity_mbit?, cost_per_mbit?, cost_period?}` | `{link, created}` |
| `isp.link.delete` | `{id}` | `{deleted, name}` |
| `isp.invoices` | `{customer_id?, state?, currency?, since?, until?, limit?, cursor?}` | invoices with their derived settlement |
| `isp.invoice.get` | `{id}` | the invoice, its status, what is paid, what is outstanding, its payments |
| `isp.invoice.draft` | `{customer_id, period_start, period_end, allow_empty?}` | `{invoice, skipped, warnings}` |
| `isp.invoice.run` | `{period_start, period_end, customer_ids?, dry_run?, issue?}` | a per-customer preview (default) or the invoices it wrote, plus everyone skipped and why, and the totals per currency |
| `isp.invoice.line.add` | `{invoice_id, description, amount}` | drafts only |
| `isp.invoice.line.remove` | `{invoice_id, line_id}` | drafts only |
| `isp.invoice.issue` | `{id, due_at?, issued_at?}` | the frozen invoice, with its number |
| `isp.invoice.void` | `{id, reason}` | the void invoice; the reason is required |
| `isp.invoice.reissue` | `{id, reason}` | `{voided, draft, note}` |
| `isp.payments` | `{customer_id?, invoice_id?, since?, until?, state?, limit?, cursor?}` | the payments, void ones included |
| `isp.payment.record` | `{customer_id, invoice_id?, amount, method, reference?, note?, at?, conversion?}` | `{payment, invoice, warnings}` |
| `isp.payment.void` | `{id, reason}` | the void payment; it is never deleted |
| `isp.arrears` | `{customer_id?}` | one aged report **per currency**, plus who owes what |
| `isp.revenue` | `{since, until?, convert?}` | issued and received per currency, what is outstanding, and a converted figure only if the caller supplied the rates |
| `isp.usage` | `{customer_id? \| filters, since?, until?}` | used against sold, with coverage per row and counted per reason |
| `isp.capacity` | `{link_id?, tower?, since?, until?}` | sold against capacity as a ratio with both figures, and what is measured |
| `isp.margin` | `{link_id?, tower?, customer_id?}` | revenue per currency, the wholesale cost, and the margin **in the cost's currency only** |

Events: `isp.changed {kinds}` for the records (`customers`, `tariffs`,
`subscriptions`, `links`), `isp.invoices.changed {change, ids, customer_ids}`
(`drafted`, `issued`, `voided`, `updated`; the `issued` one also carries
`currency`, `total_minor` and `due_at`) and
`isp.payments.changed {change, ids, customer_ids, invoice_ids}`
(`recorded`, `voided`). Reads publish nothing.

Settings section `isp`: `payment_terms_days` (14), `arrears_buckets`
(`[30, 60, 90]`), `usage_basis` (`greater`), `usage_window_hours` (24),
`invoice_number_prefix` (`""`).

Storage: `isp_customer`, `isp_subscription`, `isp_invoice` and `isp_payment`
are this feature's own tables, created with `CREATE TABLE IF NOT EXISTS`
through `store.with` (a feature cannot add a core migration). Tariffs and
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
   lie than a backdated record. `due_at` may not precede `issued_at`.
3. **A billing window must be one whole period of a subscription, or that
   subscription is not charged.** A monthly plan is billed in a window of
   28–31 days, a quarterly one in 89–92, a yearly one in 365–366. A
   seven-day window produces no line for a monthly subscription; the draft
   lists it in `skipped` with the reason, and says what to do instead (bill a
   whole period, or add the figure by hand). Nothing here divides a monthly
   price by an invented average month. Within a valid window a part period is
   pro-rated by the milliseconds covered, and the line carries that
   arithmetic (`prorated`) so the figure can be rechecked.
4. **Voiding an invoice that has a recorded payment against it is refused.**
   The void would hide money that really came in. The payment is voided first,
   with its own reason, and both records stay.
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
9. **A customer's currency cannot be changed once they have a ledger.** It
   can be corrected before anything is billed; afterwards it is a new
   account, because otherwise one customer's books would hold two currencies
   with nothing saying which figure was which.
10. **`isp.summary` is the assistant's method.** It carries counts, states and
    per-currency money and no name, code, phone number, email address, street
    or note; a test fails if any of those appear in it. An assistant tool must
    be registered against it (and against nothing else in this feature that
    returns a customer).

## Tests

`cargo test -p nocturne-isp`: 20 unit tests and 29 through `App::rpc`
(`tests/money.rs`, `tests/ledger.rs`, `tests/network.rs`, `tests/rpc.rs`,
`tests/boundaries.rs`). They build real state through the RPC surface, never
by writing to the store. The monitor is reached by name and stubbed in
`tests/common/mod.rs` with the shape the real one answers — including a
`null` rate for an interval that could not be measured — because a feature
crate may not link another feature crate; one fixture deliberately has no
monitor at all. Nothing waits on real time or asserts a duration.
