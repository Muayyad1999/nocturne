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
