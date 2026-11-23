# ISP screens: notes

Owned paths: `ui/src/pages/isp/`, `ui/tests/isp.spec.ts`, `ui/tests/isp.shots.ts`,
`ui/tests/support/isp.ts`. The backend is `crates/nocturne-isp` (specification
`docs/dev/features/isp.md`) and the contract is `ui/src/api/types/isp.ts`,
which mirrors the engine's structs and is brought up to them when they move.

## What is where

| | |
|---|---|
| `index.tsx` | the tabbed screen: customers, invoices, payments, credit notes, arrears, reports, tariffs and links, settings |
| `CustomerRoute.tsx` | one customer, at `/isp/customer/:id` |
| `data.ts` | every read and write, the four events that keep them current, the ISP settings and the business's calendar |
| `calendar.ts` | where a day begins: a typed date to an instant and back, months, all in the business's offset and never the browser's zone |
| `money.ts` | amounts: formatting from `minor` + `digits`, entry as whole minor units, per-currency totals, integer division, sorting within a currency, a typed rate read back |
| `coverage.ts` | the nine reasons there is no usage figure, in words; used-against-sold; a link's own coverage sentence |
| `ageing.ts` | arrears per currency, the age bands, owed-but-not-late, and what stands in customers' favour |
| `billing-rows.ts` | invoice status and what may be done to one; the four figures of a settlement; what is said beside a due date; the run's outcome |
| `credit-notes.ts` | what a credit note is, where it stands, how much an invoice can still be credited, what may be done to a note |
| `records.ts` | account states, units, periods, what marking an account does, what a link save sends, the settings typed as text |
| `Figures.tsx` | `Amount`, `Figures` (one line per currency) and `Words` (a sentence where a figure would be) |
| `AmountField.tsx` | typing an amount, in the currency's own minor unit |
| `CreditDialogs.tsx`, `CreditNoteList.tsx`, `Credits.tsx` | issuing, allocating and voiding a credit note; a customer's or an invoice's notes; the list of all of them |
| `SettingsTab.tsx` | the business's calendar (with "use this computer's offset"), payment terms, number prefix, arrears bands, usage basis |
| `opening.ts` | when a dialog's form is prepared: as it opens, and not again while it is open |
| `seed-dates.ts` | not part of the application: the dates the end-to-end seed writes, counted from the day it runs, and the months and document numbers the spec and the screenshots then expect (`tests/support/isp.ts`). It is here because unit tests are collected from `src/` only |

The React-free modules (`money`, `calendar`, `coverage`, `ageing`,
`billing-rows`, `credit-notes`, `records`) hold every decision that could be
got wrong quietly, and each has unit tests. They are named for what they do
rather than for a component, so that an import of `./Billing` can never
resolve to `billing-rows.ts` on a case-insensitive filesystem
(`scripts/check-case-collisions.sh`) — which is also why the settings screen
is `SettingsTab.tsx` and not `Calendar.tsx`.

## Decisions

* **Eight flat tabs, not nested ones.** Invoices, payments, credit notes and
  arrears are one job in the specification's words ("billing"), but they are
  four different lists with different filters, and a tab bar inside a tab
  bar is harder to reach than eight tabs are. The run and the payment dialog
  are reachable from the invoice list, from a customer's page and from the
  command palette.
* **A day is the business's day.** Every date typed or shown goes through
  `calendar.ts` with an offset: the `utc_offset_minutes` setting for
  anything new (a plan's start, a payment's or a credit note's date, a
  month, a report's window) and the invoice's **own** `utc_offset_minutes`
  for its period, issue date and due date. Nothing reads the browser's zone
  except the button that offers this computer's offset as the setting.
  While the setting is unread or is not a calendar the offset is `null`:
  dates show as a dash and a typed date is refused, rather than either
  falling back to the browser or to UTC. A billing run and a draft send the
  month by name (`month: "2026-10"`) and the engine resolves it. The usage
  report's measurement times are the one exception — they are readings of
  the monitor, compared with device logs, and stay in `lib/time`'s form.
* **Whether an invoice is overdue is the engine's answer.** `status` and
  `days_overdue` are read, never recomputed from `due_at` and a clock. The
  only thing `dueText` works out is which of two phrases to use for an
  invoice the engine says is *not* late ("due today" or "not due yet").
* **A settlement is four figures.** Total, paid, credited, outstanding —
  all four, in that order, in the drawer; what was credited sits under what
  was paid in the list. Paid and credited are never added together, and an
  invoice settled beyond its total says "in their favour" rather than a
  negative outstanding.
* **A credit note is issued where the debt is looked at**: from the invoice
  (against it), from the customer's page (against one of their invoices, or
  on the account) and from the credit notes tab. A note on the account says
  everywhere that it reduces nothing until it is set against an invoice,
  and that is offered once, on the customer's page and in the list. Void
  and void-and-reissue are not offered on an invoice a note stands against;
  the drawer says which note and what to do instead.
* **Money sorts within a currency.** Every sortable money column sorts by
  `moneySortKey`: the currency code, then the amount. Rows of one currency
  stay together in both directions.
* **A month of invoices is asked for by its own bounds, and narrowed by each
  invoice's own calendar.** `isp.invoices {since, until}` selects by
  instants, newest period first, a page at a time. The list sends exactly
  the month's bounds in the business's calendar (`calendar.ts: monthQuery`)
  — months meet, so anything wider puts every invoice of the next month on
  the page ahead of this month's, and with fifty of them a list filtered to
  October showed nothing. What the engine sends is then narrowed by
  `periodInMonth`, in the calendar each invoice was written in, which only
  takes anything away after the setting has been changed: an invoice
  written for September under the old offset overlaps the new October by
  the hours the two disagree about. The count line says how many on the
  page were left out and why (`billing-rows.ts: pageText`).
* **The ISP settings are a tab here, not a section of Settings.** The
  calendar decides what these screens show, the settings section list
  belongs to another slice, and a build without the feature should have no
  trace of it.
* **The customer list joins three reads.** `isp.customers` carries no sold
  megabits and no balance, so the list asks `isp.usage` with the same filters
  and page (both are ordered by name in the engine, and the join is by id,
  not by position) and `isp.arrears` for what each owes. While either is in
  flight the cell is a skeleton — not a dash, which would read as "nothing".
* **Paging is the engine's.** `cursor` is the number of rows already seen as a
  decimal string, so Previous and Next move by one page and the count comes
  from `matching`. Nothing fetches the whole book to count it.
* **A dialog's form is prepared as it opens, and not again.** A save
  reports back only after the lists behind it have been read again
  (`useRpcMutation` awaits the invalidation), and the invoice or the
  customer a dialog was opened on is one of those reads. A form that was
  re-prepared whenever that prop changed was emptied — and the save's answer
  dropped, because resetting a mutation detaches the answer still on its way
  — whenever that read came back before the slowest of the others: the
  credit note or the payment had been written, and the dialog stood open and
  blank, to be filled in a second time. `opening.ts: useOnOpen` prepares on
  the change from closed to open only; the credit note, payment and customer
  dialogs use it, being the three that are given something a read replaces.
  The others are given a row their parent holds in state, which does not
  change under them.
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

One thing in the backend was changed for these screens: `isp.link.save` took
`clear_cost: true`, because a field left out of a save keeps what is stored
and an emptied cost could therefore not be saved at all (`records.ts:
linkInput`). Six things the API does not offer, and what the screens do
instead:

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
   like the customers' one. Nor does it take a `month`, as the draft and the
   run do, so after the business's calendar has been moved **west** a page
   of a month's list can be mostly invoices of the month after it, written
   under the old offset and narrowed out here — the count line says so, and
   the month's own are on the pages that follow. A `month` read in each
   invoice's own calendar would end that.
6. **`isp.credits` has neither the customer's name nor the invoice's
   number.** The names are joined as on the payments list. The number is read
   with `isp.invoice.get` for each note on screen that stands against an
   invoice — at most one page of them — and until it has answered the cell
   says "against an invoice" rather than printing an id. On a customer's
   page the numbers come from the invoices already read; a note against an
   invoice older than those fifty says "against an invoice" and opens it. An
   `invoice_number` on `CreditNote` would remove the read.

Two smaller ones, noted rather than worked around: there is no method that
lists the distinct towers, so every tower filter is built from `isp.links`
(plus the towers on the current page of customers); and `isp.customer.get`
takes no usage window, so a customer's page shows the latest reading and the
Reports tab is where a window is chosen.

## Tests

`pnpm --dir ui test` covers the React-free modules: the money rules (minor
units per currency, entry that cannot produce a float, per-currency totals
with no sum, integer division that says when it rounded, a sort that never
ranks one currency against another — through the table's own `sortRows`),
the calendar (run with the process in `Pacific/Auckland`, because under UTC
a helper that read the browser's zone would pass), the nine coverage reasons
and the rule that a row with no figure has no digit in it, the ageing bands
and owed-but-not-late, what may be done to an invoice in each state and with
what standing against it, the four figures of a settlement, the rules of a
credit note, what a link save sends when a field is emptied, and the words
that must not promise to disconnect anybody. `opening.test.tsx` is the one
that renders anything: a dialog's form is prepared when it opens and is left
alone when what the dialog was given changes under it.

`pnpm --dir ui e2e tests/isp.spec.ts` runs against a real
`nocturne-server --features isp` on a fresh data directory, seeded through
`/api/rpc` (`tests/support/isp.ts`). A free-edition engine is simulated by
taking the `isp.*` methods out of `app.methods` at the transport; that the
crate is absent from the binary is the backend's own guarantee
(`docs/dev/spec-editions.md`), and the interface keys off the method list,
which is what that test asserts. The calendar tests open their own browser
contexts in `Asia/Baghdad` and `Pacific/Auckland`: the point of them is that
the zone of the browser changes nothing.

Two tests read the seed's invoices against the real clock ("overdue", "not
due yet"), and dates typed into the seed could satisfy them for seven weeks
of 2026 and no longer. So the seed's dates are counted from the moment it
runs, in the calendar the engine has then (`seed-dates.ts`): the month it
bills is the last whole one, the overdue invoice was issued on the 2nd of it
and fell due on its 10th, the open one is issued on the 1st of this month and
falls due three weeks from today, the part-period plan starts on the 16th of
the billed month, and the two plans billed whole began two months before it.
`seed-dates.test.ts` holds that to the engine's rules and to the screens'
own formatting over every midnight of five years in six calendars: the
overdue invoice is at least two days late and the open one at least two days
from due in whichever calendar it is read in, no invoice is issued in the
future or before its plan began, every period is one whole month and no two
overlap.

The spec and the screenshots type in no date, month or document number: the
month a run previews, the months the reissued invoice and the hand-made
draft are for, the words on a part-period line and the numbers of the two
seeded invoices all come from `seeded.dates`. A date typed in beside them is
right on the day it is typed, so the spec would only notice once the month
had turned: `seed-dates.test.ts` therefore also reads the seed, the spec and
the screenshots and fails, the same day, if the code of one holds a year
with a month, a day or a document number, a `Date.UTC` of a figure or a
month by name. It cannot see a date that is fixed without being written as
one — the wrong field of `seeded.dates`, a start counted by hand — and for
those the spec is still the only check, after the month turns.

A number carries the year its document was issued in and each year counts
from one, so in January the seed's two invoices are the first of two
different years. What a test issues itself — the credit notes, the invoice
that falls due today — is numbered in the year the test runs in; a run still
going when the year ends would expect the old year's sequence on notes
issued after midnight, which is not guarded against. The steps added for the
calendar, the months billed from the screens and the month filter take their
dates from the clock in the test itself, and their arithmetic (`dayIn`,
`monthIn`, `monthsBefore`) is the same module's.

What a helper decides is tested in the unit tests, and that a screen
actually uses it is tested here, one screen at a time: the plan's start
date, the due date, a payment's date, the run's month and the draft's month
from a browser in Auckland while the business is in Baghdad; the link dialog
emptied and saved; the rate echoed under the revenue's conversion; the
invoice list sorted by a money column and filtered to a month. Each of
those was checked by putting the old behaviour back in the component alone
and seeing the spec fail.

One test holds a read back on purpose (`page.route`, 1.5 s on one `isp.*`
method) while a credit note, a payment and a customer edit are saved: the
order in which the reads behind a save come back is otherwise decided by the
machine, and the defect it guards — a dialog left open and blank after a
save that went through — showed up without it only as a test that failed
one run in ten under load.

The engine has to be built with the feature. Set `NOCTURNE_SERVER_BIN` to a
binary built with `cargo build -p nocturne-server --features isp`, or let the
harness build one (`startBackend({ features: ['isp'] })`). The spec fails with
that sentence rather than with forty assertions about a screen that says the
edition is not installed.
