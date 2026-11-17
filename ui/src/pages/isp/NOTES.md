# ISP screens: notes

Owned paths: `ui/src/pages/isp/`, `ui/tests/isp.spec.ts`, `ui/tests/isp.shots.ts`,
`ui/tests/support/isp.ts`. The backend is `crates/nocturne-isp` (specification
`docs/dev/features/isp.md`), the contract is `ui/src/api/types/isp.ts`, and
nothing here changes any of them.

## What is where

| | |
|---|---|
| `index.tsx` | the tabbed screen: customers, invoices, payments, arrears, reports, tariffs and links |
| `CustomerRoute.tsx` | one customer, at `/isp/customer/:id` |
| `data.ts` | every read and write, and the three events that keep them current |
| `money.ts` | amounts: formatting from `minor` + `digits`, entry as whole minor units, per-currency totals, integer division |
| `coverage.ts` | the nine reasons there is no usage figure, in words; used-against-sold; a link's own coverage sentence |
| `ageing.ts` | arrears per currency, the age bands, owed-but-not-late |
| `billing-rows.ts` | invoice status and what may be done to one; the run's outcome; the period to bill |
| `records.ts` | account states, units, periods, and what marking an account does |
| `Figures.tsx` | `Amount`, `Figures` (one line per currency) and `Words` (a sentence where a figure would be) |
| `AmountField.tsx` | typing an amount, in the currency's own minor unit |

The React-free modules (`money`, `coverage`, `ageing`, `billing-rows`,
`records`) hold every decision that could be got wrong quietly, and each has
unit tests. They are named for what they do rather than for a component, so
that an import of `./Billing` can never resolve to `billing-rows.ts` on a
case-insensitive filesystem (`scripts/check-case-collisions.sh`).

## Decisions

* **Six flat tabs, not nested ones.** Invoices, payments and arrears are one
  job in the specification's words ("billing"), but they are three different
  lists with three different filters, and a tab bar inside a tab bar is
  harder to reach than six tabs are. The run and the payment dialog are
  reachable from the invoice list, from a customer's page and from the command
  palette.
* **The customer list joins three reads.** `isp.customers` carries no sold
  megabits and no balance, so the list asks `isp.usage` with the same filters
  and page (both are ordered by name in the engine, and the join is by id,
  not by position) and `isp.arrears` for what each owes. While either is in
  flight the cell is a skeleton — not a dash, which would read as "nothing".
* **Paging is the engine's.** `cursor` is the number of rows already seen as a
  decimal string, so Previous and Next move by one page and the count comes
  from `matching`. Nothing fetches the whole book to count it.
* **Issuing asks for a typed confirmation**; drafting does not. An issued
  invoice is frozen for good — the correction is a void with a reason and a
  reissue, which is a trail and not an undo — so the preview is the first
  gate and the typed word is the second. Creating drafts is reversible by
  voiding them, and gets a plain confirmation.
* **A conversion is only ever shown as one.** The only single figure across
  currencies on these screens comes from `isp.revenue {convert}`, with the
  rates the person entered, and it is drawn under the per-currency ledger with
  the rates and the moment beside it. There is no code in `money.ts` that adds
  two currencies, so no screen can grow one by accident.
* **Rates are entered in the direction they were quoted**, with a swap
  control. A rate is minor units of the second currency for one unit of the
  first, because the other direction does not fit in an integer (0.00076
  dollars per dinar is zero). The field says which way round it is reading it.
* **Nothing offers to disconnect anybody.** Marking an account suspended says,
  before it is done, that it records a decision and that the service keeps
  running until somebody changes the router. `records.test.ts` reads every one
  of those sentences for a promise to cut a customer off, which is the
  interface half of what `crates/nocturne-isp/tests/boundaries.rs` enforces in
  the engine.

## Worked around in the browser

Nothing in the backend was changed for these screens. Five things the API does
not offer, and what the screens do instead:

1. **`isp.payments` has no customer name.** Every payment carries
   `customer_id` only, while an invoice carries `customer_name` (as it was
   when the invoice was written). The payments list therefore joins against a
   page of up to 2 000 customers and shows the id when a name is not in that
   page. The same goes for a billing run's preview rows, which carry
   `customer_id` and nothing else. A `customer_name` on `Payment` and on
   `RunRow` — the invoice's own convention — would remove both joins.
2. **`isp.capacity` describes a link with a customer's words.** `coverage_note`
   on a `LinkCapacity` row is `Coverage::note()`, which is written about a
   customer: on a tower feed it reads "no serving device is recorded for this
   customer, so there is nothing to measure *their usage* on". The
   oversubscription report shows `coverage.ts: linkCoverageNote` instead, which
   says the same thing about the link. The engine could carry a link-worded
   note and the screen would use it.
3. **`isp.margin` answers `not_found` when no link is recorded at all**
   ("record one with `isp.link.save` so there is a capacity and a wholesale
   cost to work a margin out from"). That is an empty report rather than a
   failure, so the panel shows it as an empty state with the engine's own
   sentence. An empty list would be easier to tell from a real error.
4. **`isp.margin` does not aggregate by tower.** `of` is `link` unless
   `customer_id` is given, and a tower is resolved to its links, so the
   report's tower control filters rather than groups. A customer's own margin
   is on their page, where the id is known.
5. **`isp.invoices` has no count and no text search.** There is no `matching`
   (the customers' page has one), so paging is "a full page means there may be
   more", and there is no `query` over the customer's name, so the filters are
   state, currency and period; one customer's invoices are on their own page.
   `matching` on `isp.invoices` and `isp.payments` would make those lists read
   like the customers' one.

Two smaller ones, noted rather than worked around: there is no method that
lists the distinct towers, so every tower filter is built from `isp.links`
(plus the towers on the current page of customers); and `isp.customer.get`
takes no usage window, so a customer's page shows the latest reading and the
Reports tab is where a window is chosen.

## Tests

`pnpm --dir ui test` covers the React-free modules: the money rules (minor
units per currency, entry that cannot produce a float, per-currency totals
with no sum, integer division that says when it rounded), the nine coverage
reasons and the rule that a row with no figure has no digit in it, the ageing
bands and owed-but-not-late, what may be done to an invoice in each state, and
the words that must not promise to disconnect anybody.

`pnpm --dir ui e2e tests/isp.spec.ts` runs against a real
`nocturne-server --features isp` on a fresh data directory, seeded through
`/api/rpc` (`tests/support/isp.ts`). A free-edition engine is simulated by
taking the `isp.*` methods out of `app.methods` at the transport; that the
crate is absent from the binary is the backend's own guarantee
(`docs/dev/spec-editions.md`), and the interface keys off the method list,
which is what that test asserts.

The engine has to be built with the feature. Set `NOCTURNE_SERVER_BIN` to a
binary built with `cargo build -p nocturne-server --features isp`, or let the
harness build one (`startBackend({ features: ['isp'] })`). The spec fails with
that sentence rather than with forty assertions about a screen that says the
edition is not installed.
