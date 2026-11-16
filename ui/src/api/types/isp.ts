/**
 * ISP (`nocturne-isp`): subscribers, bandwidth and money — customers,
 * tariffs, subscriptions, invoices, payments, arrears, used-against-sold,
 * oversubscription and margin.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, `?` marks a field the backend may leave out (or a
 * parameter it defaults) and `| null` a field it always sends and that may be
 * null. Backend source: `crates/nocturne-isp`. Specification:
 * `docs/dev/features/isp.md`.
 *
 * Five things about this surface decide how the screens have to be built.
 * They are not stylistic.
 *
 * 1. **Money is {@link Money}: whole minor units with a currency.** `minor`
 *    is an integer and the only truth; `digits` and `text` are there so no
 *    screen has to know that `1000` is ten dollars (`USD`, 2 digits) or a
 *    thousand dinars (`IQD`, 0 digits). **Never divide by 100** — use
 *    `digits`, or render `text`. Entering an amount means sending
 *    {@link MoneyInput}: `{currency, minor}` or `{currency, text: "25.00"}`.
 *    A JSON number with a decimal point in it is refused by the backend, so
 *    an input field must not hand a float to `minor`; `isp.currencies`
 *    returns the accepted codes and their minor digits.
 * 2. **A total across currencies is not a number.** {@link Totals} is one
 *    {@link Money} per currency and there is no scalar anywhere. Render it as
 *    several figures. One number requires the caller to supply the rates, and
 *    what comes back is {@link Converted} — `basis: "conversion"`, with the
 *    rates and the date it used. Label it as a conversion on screen; it is
 *    not the ledger.
 * 3. **An issued invoice is frozen.** `state: "issued"` means every edit is
 *    refused with `conflict`. A wrong invoice is voided (with a reason) and
 *    reissued: `isp.invoice.reissue` gives back the void original and a fresh
 *    draft that `replaces` it. {@link InvoiceStatus} (`part_paid`, `paid`,
 *    `overdue`) is *derived* from the payments and the clock at read time —
 *    there is no paid flag to set, and nothing but a recorded payment moves
 *    it.
 * 4. **No usage figure is not a usage of zero.** On a
 *    {@link UsedAgainstSold} row, `used` is null unless
 *    `coverage === 'measured'`, and {@link Coverage} says which of the
 *    several different nothings it is. Show the reason
 *    (`coverage_note`), never a 0 Mbit bar. The same goes for `sold_mbit`,
 *    which is null for a customer with no subscription.
 * 5. **Nothing here touches a device.** `isp.customer.state` records a
 *    decision and returns `device_changed: false`; suspending the service is
 *    something a person does on the router. A screen must not imply
 *    otherwise.
 */

// ---------------------------------------------------------------- vocabulary

/**
 * An ISO 4217 code the backend accepts. The list is fixed in the backend
 * (`money::CURRENCIES`) and an unknown code is refused on entry rather than
 * assumed to have two minor digits; `isp.currencies` is the authority at
 * runtime.
 */
export type CurrencyCode =
  | 'AED'
  | 'AUD'
  | 'BHD'
  | 'CAD'
  | 'CHF'
  | 'EGP'
  | 'EUR'
  | 'GBP'
  | 'INR'
  | 'IQD'
  | 'JOD'
  | 'JPY'
  | 'KES'
  | 'KRW'
  | 'KWD'
  | 'OMR'
  | 'PKR'
  | 'SAR'
  | 'TRY'
  | 'USD'
  | 'ZAR';

/** The state of a customer's *account*. None of these does anything to a device. */
export type CustomerState = 'active' | 'suspended' | 'cancelled';

/** How a tariff's price is quoted. */
export type Unit = 'per_mbit' | 'flat';

export type Period = 'monthly' | 'quarterly' | 'yearly';

export type SubscriptionState = 'active' | 'ended';

/** What has happened to an invoice as a document. Settlement is not in here. */
export type InvoiceState = 'draft' | 'issued' | 'void';

/**
 * What to call an invoice: its state plus what the payments and the clock
 * say. Derived at read time, never stored.
 */
export type InvoiceStatus =
  | 'draft'
  | 'issued'
  | 'part_paid'
  | 'paid'
  | 'overdue'
  | 'void';

export type LineKind = 'subscription' | 'adjustment';

export type PaymentState = 'recorded' | 'void';

/**
 * Why there is, or is not, a figure. Everything but `measured` means there is
 * **no** number — not a zero — and they are different nothings on purpose.
 */
export type Coverage =
  /** There is a figure, and it was measured. */
  | 'measured'
  /** Nobody has recorded which device serves this customer. */
  | 'no_serving_device'
  /** A device is recorded but not which interface. */
  | 'no_serving_interface'
  /** The device is not in the inventory any more. */
  | 'device_missing'
  /** This build has no monitoring, so no interface rate exists at all. */
  | 'monitor_unavailable'
  /** Monitoring has never recorded that interface: nobody has looked. */
  | 'interface_never_collected'
  /** The rate could not be measured (first poll, reboot, failed poll). */
  | 'no_rate_yet'
  /** Nothing was recorded inside the window asked for. */
  | 'no_samples_in_window'
  /** Monitoring was asked and refused. */
  | 'monitor_failed';

/** Settings section `isp` (`settings.get {section: "isp"}`). */
export interface IspSettings {
  /** Days after issue that an invoice falls due, when no date is given. */
  payment_terms_days: number;
  /** The boundaries, in days overdue, of the arrears report's bands. */
  arrears_buckets: number[];
  /** `greater` (of the two directions), `in` or `out`. */
  usage_basis: string;
  /** How far back a usage figure looks when the caller gives no window. */
  usage_window_hours: number;
  /** Put in front of an invoice number: `ACME-2026-0001`. */
  invoice_number_prefix: string;
}

// --------------------------------------------------------------------- money

/**
 * An amount: whole minor units of one currency.
 *
 * `minor` is the ledger. `digits` is how many decimal places the currency
 * has (2 for `USD`, **0** for `IQD`, 3 for `JOD`) and `text` is the amount
 * written out exactly. A screen formats from `digits` or shows `text`; it
 * never divides by a hundred.
 */
export interface Money {
  currency: CurrencyCode;
  /** Whole minor units. Never fractional, never a float. */
  minor: number;
  /** Decimal places this currency has. */
  digits: number;
  /** The amount as people write it: `25.00`, `1000`, `-1.07`. */
  text: string;
}

/**
 * An amount on the way in: the minor units, or an exact decimal string.
 * Sending a float in `minor` is refused (`invalid`), and so is a string with
 * a thousands separator, a currency symbol or more decimals than the
 * currency has.
 */
export type MoneyInput =
  | { currency: CurrencyCode | string; minor: number }
  | { currency: CurrencyCode | string; text: string };

/**
 * One figure per currency, in code order. There is deliberately no total: a
 * single mixed figure would be invented. Render every entry.
 */
export type Totals = Money[];

/**
 * The rate a payment was actually taken at, recorded on that payment and
 * never looked up again.
 *
 * `rate` is whole minor units of `to` for **one major unit** of `from`:
 * 1,320 for "1,320 dinars to the dollar" (`from: "USD"`, `to: "IQD"`). A rate
 * is kept in the direction it was quoted in — the other direction does not
 * fit in an integer — and the backend divides instead of multiplying when the
 * amount is in the `to` currency.
 */
export interface Conversion {
  from: CurrencyCode;
  to: CurrencyCode;
  /** Minor units of `to` per one major unit of `from`, as entered. */
  rate: number;
  at: number;
}

/** A rate on the way in. `at` defaults to the moment it is recorded. */
export interface ConversionInput {
  from: CurrencyCode | string;
  to: CurrencyCode | string;
  /** A whole number, greater than zero. A float is refused. */
  rate: number;
  at?: number;
}

/**
 * A figure that is **not** the ledger: a mixed total converted at rates the
 * caller supplied. Show it labelled, with `rates` and `at` beside it.
 */
export interface Converted {
  /** Always `"conversion"`. */
  basis: 'conversion';
  amount: Money;
  /** The per-currency ledger it was computed from. */
  of: Totals;
  rates: Conversion[];
  at: number;
  /** False when minor units were rounded away. */
  exact: boolean;
  note: string;
}

/** One row of `isp.currencies`. */
export interface CurrencyRow {
  code: CurrencyCode;
  /** Decimal places in everyday use. `IQD` is 0. */
  digits: number;
  /** Minor units in one major unit: 100 for `USD`, **1** for `IQD`. */
  scale: number;
  name: string;
  /** What 1,000 minor units of it is, written out. */
  thousand_minor_units: string;
}

// ------------------------------------------------------------------ customers

/** Personal details. Never put these in front of a model. */
export interface Contact {
  phone: string;
  email: string;
  address: string;
}

/** The join to the rest of Nocturne. */
export interface Service {
  /** An inventory device id. Null means nobody has said which device serves them. */
  device_id: string | null;
  /** The interface name as the device reports it (`ether3`, `Gi0/2`). */
  interface: string | null;
  /** The label the link that feeds them carries (`Tower A`). */
  tower: string | null;
  /** Where the service is, in the customer's own words. */
  location: string;
}

export interface Customer {
  id: string;
  /** The ISP's own account number; `''` when they do not use one. Unique when set. */
  code: string;
  name: string;
  contact: Contact;
  site: string;
  /** Every subscription and invoice of theirs is in this currency. */
  currency: CurrencyCode;
  state: CustomerState;
  service: Service;
  tags: string[];
  notes: string;
  created_at: number;
  updated_at: number;
}

/**
 * `isp.customer.save` params. Absent fields are left as they are on an
 * update. `currency` is required when creating and refused once they have a
 * ledger. The state is **not** here: it goes through `isp.customer.state`.
 */
export interface CustomerInput {
  /** Omit to create. */
  id?: string;
  code?: string;
  /** Required when creating. */
  name?: string;
  /** Required when creating; there is no default. */
  currency?: CurrencyCode | string;
  phone?: string;
  email?: string;
  address?: string;
  site?: string;
  /** An inventory device id; it must exist. */
  device_id?: string;
  interface?: string;
  tower?: string;
  location?: string;
  tags?: string[];
  notes?: string;
}

export interface SavedCustomer {
  customer: Customer;
  created: boolean;
  /** Worth showing, not errors: e.g. an interface recorded with no device. */
  warnings: string[];
}

/** `isp.customers` params. */
export interface CustomersParams {
  /** Free text over the name, code, contact details, site and tower. */
  query?: string;
  state?: CustomerState;
  currency?: CurrencyCode | string;
  tower?: string;
  device_id?: string;
  /** Default 200, at most 2,000. */
  limit?: number;
  /** The number of rows already received, as a decimal string. */
  cursor?: string;
}

export interface CustomerPage {
  customers: Customer[];
  /** How many match the filter in all. */
  matching: number;
  /** Pass back as `cursor`; null on the last page. */
  next_cursor: string | null;
}

/** `isp.customer.state` params. */
export interface CustomerStateParams {
  id: string;
  state: CustomerState;
  reason?: string;
}

export interface StateChange {
  customer: Customer;
  was: CustomerState;
  /** Always false: this feature has no write path to the network. */
  device_changed: boolean;
  note: string;
}

export interface DeletedCustomer {
  deleted: boolean;
  name: string;
}

/** Result of `isp.customer.get`. */
export interface CustomerDetail {
  customer: Customer;
  currency: CurrencyCode;
  subscriptions: SubscriptionRow[];
  /** Newest first, as many as `invoices` asked for (default 20). */
  invoices: InvoiceView[];
  balance: Balance;
  usage: UsedAgainstSold;
}

/** `isp.customer.get` params. */
export interface CustomerGetParams {
  id: string;
  /** How many invoices to send back, newest first. Default 20. */
  invoices?: number;
}

/** What one customer owes and what they have paid in advance. */
export interface Balance {
  customer_id: string;
  /** Still owed on issued invoices, in the invoices' currency. */
  outstanding: Totals;
  /** Received and not yet against an invoice, as it was received. */
  on_account: Totals;
  invoices_outstanding: number;
  oldest_due_at: number | null;
  note: string;
}

// -------------------------------------------------------------------- tariffs

export interface Tariff {
  id: string;
  name: string;
  description: string;
  /** What the plan sells, in Mbit/s. */
  mbit: number;
  unit: Unit;
  /** Per megabit for `per_mbit`, the whole plan for `flat`. */
  price: Money;
  period: Period;
  created_at: number;
  updated_at: number;
}

/** A row of `isp.tariffs`: the tariff, flattened, plus what it comes to. */
export interface TariffRow extends Tariff {
  /** `price × mbit` for `per_mbit`, `price` for `flat`. */
  period_price: Money;
  /** How many subscriptions were agreed on these terms, ever. */
  subscriptions: number;
  active_subscriptions: number;
}

/** `isp.tariff.save` params. */
export interface TariffInput {
  /** Omit to create. */
  id?: string;
  name?: string;
  description?: string;
  /** Required, and greater than zero. */
  mbit?: number;
  unit?: Unit;
  price?: MoneyInput;
  period?: Period;
}

export interface SavedTariff {
  tariff: Tariff;
  period_price: Money;
  created: boolean;
  /**
   * Running subscriptions that keep their own copy of the old figures and are
   * therefore untouched by this save. Worth showing: raising a tariff does
   * not raise what anybody already on it pays.
   */
  subscriptions_unaffected: number;
}

export interface Deleted {
  deleted: boolean;
  name: string;
}

// --------------------------------------------------------------- subscriptions

/**
 * A customer on a tariff from a date, holding **its own copy** of the agreed
 * megabits and price. `tariff_id` is provenance only — nothing reads the
 * tariff to work these figures out again, and the tariff may since have
 * changed or gone.
 */
export interface Subscription {
  id: string;
  customer_id: string;
  tariff_id: string;
  /** The tariff's name *as it was* when this was agreed. */
  tariff_name: string;
  mbit: number;
  unit: Unit;
  /** The agreed unit price (per megabit, or the flat figure). */
  unit_price: Money;
  /** The agreed figure for one whole period, worked out once. */
  price: Money;
  period: Period;
  started_at: number;
  ended_at: number | null;
  state: SubscriptionState;
  /** The subscription this one replaced, when terms were amended. */
  amends: string | null;
  note: string;
  created_at: number;
  updated_at: number;
}

/** A row of `isp.subscriptions`. */
export interface SubscriptionRow extends Subscription {
  /** The agreed figure as a monthly one, for comparing plans. Derived. */
  monthly_price: Money;
  /** False when that division rounded. */
  monthly_exact: boolean;
}

/** `isp.subscription.create` params. */
export interface SubscriptionInput {
  customer_id: string;
  tariff_id: string;
  /** Override the tariff's megabits for this customer. */
  mbit?: number;
  /** Override the tariff's unit price. Must be in the customer's currency. */
  price?: MoneyInput;
  unit?: Unit;
  period?: Period;
  /** When it starts; now by default. */
  started_at?: number;
  note?: string;
}

export interface SavedSubscription {
  subscription: Subscription;
  /** The subscription this one replaced, when terms were amended. */
  ended: Subscription | null;
  note: string;
}

/**
 * `isp.subscription.amend` params: change what a customer pays from a date.
 * The old subscription ends and a new one starts, so invoices already issued
 * on the old terms stay as they were. At least one of `mbit`/`price`.
 */
export interface AmendParams {
  id: string;
  mbit?: number;
  price?: MoneyInput;
  /** When the new terms start; now by default. */
  from_at?: number;
  note?: string;
}

/** `isp.subscription.end` params. */
export interface EndSubscriptionParams {
  id: string;
  ended_at?: number;
  reason?: string;
}

// ----------------------------------------------------------------------- links

/** An upstream link or a tower's feed. */
export interface Link {
  id: string;
  name: string;
  description: string;
  /** The device and interface it is, when Nocturne can see it. */
  device_id: string | null;
  interface: string | null;
  /** The label the customers behind it carry in `service.tower`. */
  tower: string | null;
  /** What it can carry, as somebody wrote it down. */
  capacity_mbit: number;
  /** The wholesale cost of one megabit for one `cost_period`. */
  cost_per_mbit: Money | null;
  cost_period: Period;
  created_at: number;
  updated_at: number;
}

/** `isp.link.save` params. */
export interface LinkInput {
  id?: string;
  name?: string;
  description?: string;
  device_id?: string;
  interface?: string;
  tower?: string;
  capacity_mbit?: number;
  cost_per_mbit?: MoneyInput;
  cost_period?: Period;
}

export interface SavedLink {
  link: Link;
  created: boolean;
}

// -------------------------------------------------------------------- invoices

/** Why a line is not the whole period's price, and the arithmetic. */
export interface Proration {
  covered_ms: number;
  period_ms: number;
  covered_days: number;
  period_days: number;
  /** False when minor units were rounded away. */
  exact: boolean;
  basis: string;
}

export interface InvoiceLine {
  id: string;
  kind: LineKind;
  description: string;
  subscription_id: string | null;
  tariff_name: string | null;
  mbit: number | null;
  unit: Unit | null;
  unit_price: Money | null;
  /** What one whole period at these terms would have cost. */
  full_period_price: Money | null;
  period_start: number | null;
  period_end: number | null;
  prorated: Proration | null;
  /** Always in the invoice's currency. */
  amount: Money;
}

export interface Invoice {
  id: string;
  /** Allocated when it is issued, never before, never reused. */
  number: string | null;
  customer_id: string;
  /** The customer's name as it was when the invoice was issued. */
  customer_name: string;
  currency: CurrencyCode;
  state: InvoiceState;
  period_start: number;
  period_end: number;
  lines: InvoiceLine[];
  total: Money;
  issued_at: number | null;
  due_at: number | null;
  voided_at: number | null;
  void_reason: string;
  /** The invoice issued in this one's place. */
  replaced_by: string | null;
  /** The invoice this one replaces. */
  replaces: string | null;
  note: string;
  created_at: number;
  updated_at: number;
}

/**
 * An invoice with what the payments say about it. The invoice's own fields are
 * flattened in and are untouched; everything derived is beside them.
 */
export interface InvoiceView extends Invoice {
  status: InvoiceStatus;
  /** Recorded against it, in its own currency. */
  paid: Money;
  /** Negative when it has been overpaid. */
  outstanding: Money;
  /** Days past its due date, when it is issued, due and short. */
  days_overdue: number | null;
  /** Newest first. Void payments are here too, with `state: "void"`. */
  payments: Payment[];
}

/** `isp.invoices` params. */
export interface InvoicesParams {
  customer_id?: string;
  state?: InvoiceState;
  currency?: CurrencyCode | string;
  /** Invoices whose period ends at or after this moment. */
  since?: number;
  /** Invoices whose period starts before this moment. */
  until?: number;
  /** Default 100, at most 2,000. */
  limit?: number;
  cursor?: string;
}

/** `isp.invoice.draft` params. */
export interface DraftParams {
  customer_id: string;
  period_start: number;
  period_end: number;
  /** Make a draft with no lines, to add them by hand. */
  allow_empty?: boolean;
}

/** A subscription this invoice deliberately does not charge for. */
export interface Skipped {
  subscription_id: string;
  tariff_name: string;
  /**
   * Why. Most often: the window is not one whole period of that
   * subscription's kind, and nothing here guesses what a part of one costs.
   */
  reason: string;
}

export interface DraftedInvoice {
  invoice: Invoice;
  /** Not zeroes: nothing was billed for these, and each says why. */
  skipped: Skipped[];
  warnings: string[];
}

/** `isp.invoice.issue` params. */
export interface IssueParams {
  id: string;
  /** Default: issued + `payment_terms_days`. Never before it was issued. */
  due_at?: number;
  /**
   * For entering an invoice that already exists on paper (a migration). It
   * may not be in the future.
   */
  issued_at?: number;
}

/** `isp.invoice.void` and `isp.invoice.reissue` params. The reason is required. */
export interface VoidParams {
  id: string;
  reason: string;
}

export interface Reissued {
  /** The original, now void and still readable, with its number kept. */
  voided: Invoice;
  /** Its replacement: a draft, so the correction can be made first. */
  draft: Invoice;
  note: string;
}

/** `isp.invoice.line.add` params. Drafts only. */
export interface LineInput {
  invoice_id: string;
  description: string;
  /** Must be in the invoice's currency. */
  amount: MoneyInput;
}

/** `isp.invoice.line.remove` params. Drafts only. */
export interface LineRemoveParams {
  invoice_id: string;
  line_id: string;
}

/** `isp.invoice.run` params: bill a period. Previews unless `dry_run: false`. */
export interface RunParams {
  period_start: number;
  period_end: number;
  /** Only these customers; every active one by default. */
  customer_ids?: string[];
  /** Nothing is written unless this is explicitly `false`. Default true. */
  dry_run?: boolean;
  /** Issue each draft as it is made instead of leaving it to be checked. */
  issue?: boolean;
}

export interface RunRow {
  customer_id: string;
  /** Null in a dry run. */
  invoice_id: string | null;
  number: string | null;
  total: Money;
  lines: number;
  skipped_subscriptions: Skipped[];
}

export interface RunSkip {
  customer_id: string;
  /** e.g. nothing to bill, or that period is already invoiced. */
  reason: string;
}

export interface RunResult {
  period_start: number;
  period_end: number;
  dry_run: boolean;
  drafted: number;
  issued: number;
  invoices: RunRow[];
  /** Customers deliberately not billed, each with a reason. */
  skipped: RunSkip[];
  /** What the run comes to, per currency. Never one number. */
  totals: Totals;
  note: string;
}

// -------------------------------------------------------------------- payments

export interface Payment {
  id: string;
  customer_id: string;
  /** Null for a payment on account: real money, not yet against anything. */
  invoice_id: string | null;
  /** What was handed over. */
  amount: Money;
  /** The rate it was taken at, when the currency differed. Never recomputed. */
  conversion: Conversion | null;
  /**
   * What it settles, in the invoice's currency — worked out once, when it was
   * taken. Null for a payment on account.
   */
  settled: Money | null;
  /** False when that conversion rounded minor units away. */
  settled_exact: boolean;
  method: string;
  reference: string;
  note: string;
  /** When the money changed hands. */
  at: number;
  state: PaymentState;
  void_reason: string;
  voided_at: number | null;
  recorded_at: number;
}

/** `isp.payments` params. */
export interface PaymentsParams {
  customer_id?: string;
  invoice_id?: string;
  since?: number;
  until?: number;
  state?: PaymentState;
  /** Default 100, at most 2,000. */
  limit?: number;
  cursor?: string;
}

/**
 * `isp.payment.record` params.
 *
 * `amount` is what was actually received. Leave `invoice_id` out for a
 * payment on account. `conversion` is **required** when the money is in a
 * currency other than the invoice's, and refused when it is not: there is no
 * rate feed, so whoever takes the payment enters what they got.
 */
export interface PaymentInput {
  customer_id: string;
  invoice_id?: string;
  amount: MoneyInput;
  /** Cash, bank transfer, a wallet — the ISP's own words. Required. */
  method: string;
  reference?: string;
  note?: string;
  /** When the money changed hands; now by default. */
  at?: number;
  conversion?: ConversionInput;
}

export interface RecordedPayment {
  payment: Payment;
  /** The invoice as it now reads, when the payment settles one. */
  invoice: InvoiceView | null;
  /** e.g. the conversion rounded, or the invoice is now overpaid. */
  warnings: string[];
}

/** `isp.payment.void` params. The reason is required. */
export interface VoidPaymentParams {
  id: string;
  reason: string;
}

// -------------------------------------------------------------------- arrears

/** One age band of what is owed, in one currency. */
export interface Bucket {
  /** `not yet due`, `1–30 days`, …, `over 90 days`. */
  label: string;
  /** Days overdue this band starts at; `-1` for "not yet due". */
  from_days: number;
  /** Null on the last band. */
  to_days: number | null;
  amount: Money;
  invoices: number;
}

/** What is owed in one currency, aged. Never a row in a bigger sum. */
export interface CurrencyArrears {
  currency: CurrencyCode;
  total: Money;
  /** Owed but not yet due, kept out of `overdue`. */
  not_yet_due: Money;
  overdue: Money;
  buckets: Bucket[];
  customers: number;
  invoices: number;
}

/** What one customer owes, in one currency. */
export interface CustomerArrears {
  customer_id: string;
  /** `(deleted customer)` when the ledger outlived the record. */
  customer_name: string;
  state: CustomerState;
  outstanding: Money;
  /** Days overdue of their oldest unpaid invoice; null when nothing is due yet. */
  oldest_days: number | null;
  invoices: number;
}

export interface Arrears {
  as_of: number;
  /**
   * One aged report per currency. **There is no total across them** and
   * nowhere to put one: render them side by side.
   */
  by_currency: CurrencyArrears[];
  /** Worst first within each currency. One row per customer per currency. */
  customers: CustomerArrears[];
  note: string;
}

/** `isp.arrears` params. */
export interface ArrearsParams {
  customer_id?: string;
}

// -------------------------------------------------------------------- revenue

/** Ask for one figure instead of several, at rates you supply. */
export interface ConvertRequest {
  to: CurrencyCode | string;
  /** A rate for every currency in the total, or the call is refused. */
  rates: ConversionInput[];
  /** When these rates are from; now by default. */
  at?: number;
}

export interface ConvertedRevenue {
  issued: Converted;
  received: Converted;
}

/** `isp.revenue` params. */
export interface RevenueParams {
  since: number;
  /** Default: just after now, so an invoice issued this instant is included. */
  until?: number;
  convert?: ConvertRequest;
}

export interface Revenue {
  since: number;
  until: number;
  /** Invoiced in the window, per currency. */
  issued: Totals;
  /** Received in the window, in the currency it was received in. */
  received: Totals;
  /** Owed right now, per currency. */
  outstanding: Totals;
  invoices_issued: number;
  /** Only when the caller supplied the rates. Labelled a conversion. */
  converted: ConvertedRevenue | null;
  note: string;
}

// ----------------------------------------------------------- used against sold

/** The window a figure covers, and which resolution it came from. */
export interface UsageWindow {
  since: number;
  until: number;
  /** `raw`, `5m` or `1h`, as the monitor answered. */
  rollup: string;
}

/** What is actually passing, in Mbit/s. */
export interface UsageFigure {
  in_mbit: number | null;
  out_mbit: number | null;
  /** The figure a plan is compared against, per the `usage_basis` setting. */
  used_mbit: number;
  /** Which direction that is, in words. Show it. */
  basis: string;
  /** The moment the figure is from, for a latest reading. */
  at: number | null;
  /** The window it averages, for a windowed reading. */
  window: UsageWindow | null;
  peak_mbit: number | null;
  samples: number | null;
  /** What the interface itself says it can carry. */
  speed_mbit: number | null;
}

/** What one customer was sold, and what they are using. */
export interface UsedAgainstSold {
  customer_id: string;
  customer_name: string;
  state: CustomerState;
  /**
   * The sum of their active subscriptions. **Null means they have none** —
   * not a plan of zero megabits.
   */
  sold_mbit: number | null;
  subscriptions: number;
  device_id: string | null;
  interface: string | null;
  /** Null whenever `coverage` is not `measured`. */
  used: UsageFigure | null;
  coverage: Coverage;
  /** Why, in a sentence fit to show somebody. */
  coverage_note: string;
  /** Only when both numbers exist. 140 and 4 are both worth knowing. */
  of_plan_pct: number | null;
}

export interface CoverageCount {
  coverage: Coverage;
  customers: number;
  note: string;
}

export interface UsageReport {
  rows: UsedAgainstSold[];
  customers: number;
  /** How many have a usage figure at all. */
  measured: number;
  without_usage: number;
  /** The reasons the rest have none, counted. Put this on the screen. */
  coverage: CoverageCount[];
  window: UsageWindow | null;
  note: string;
}

/** `isp.usage` params: one customer, or the page the filters describe. */
export interface UsageParams {
  customer_id?: string;
  query?: string;
  state?: CustomerState;
  currency?: CurrencyCode | string;
  tower?: string;
  device_id?: string;
  limit?: number;
  cursor?: string;
  /** Average over a window instead of taking the latest reading. */
  since?: number;
  until?: number;
}

// ------------------------------------------------------------------- capacity

export interface LinkCapacity {
  link_id: string;
  link_name: string;
  tower: string | null;
  device_id: string | null;
  interface: string | null;
  capacity_mbit: number;
  /** `declared` or `interface_speed` — say which on screen. */
  capacity_basis: 'declared' | 'interface_speed';
  /** The active plans of the active customers behind it. */
  sold_mbit: number;
  /** Sold to suspended accounts: still sold, counted apart. */
  suspended_mbit: number;
  customers: number;
  suspended_customers: number;
  /** Sold ÷ capacity. Null when no capacity is recorded. */
  ratio: number | null;
  /** `3.0:1`, or a sentence when there is no ratio to show. */
  ratio_text: string;
  /** What it is actually carrying, when that can be measured. */
  measured: UsageFigure | null;
  coverage: Coverage;
  coverage_note: string;
  note: string;
}

/** `isp.capacity` params. */
export interface CapacityParams {
  link_id?: string;
  tower?: string;
  /** Average over a window instead of taking the latest reading. */
  since?: number;
  until?: number;
}

// --------------------------------------------------------------------- margin

export interface Margin {
  of: 'link' | 'tower' | 'customer';
  id: string;
  name: string;
  /** Revenue for one month, per currency — never summed across them. */
  revenue: Totals;
  /** What the bandwidth costs for one month. Null when no cost is recorded. */
  cost: Money | null;
  /** Revenue minus cost, **in the cost's currency only**. */
  margin: Money | null;
  /** Revenue in currencies the cost is not in: left exactly as it is. */
  unconverted: Totals;
  cost_mbit: number | null;
  /** False when a figure here came out of a division that rounded. */
  exact: boolean;
  note: string;
}

/** `isp.margin` params: a link, a tower, or one customer. */
export interface MarginParams {
  link_id?: string;
  tower?: string;
  customer_id?: string;
}

// -------------------------------------------------------------------- summary

export interface StateCount {
  state: CustomerState | string;
  customers: number;
}

export interface CurrencyCount {
  currency: CurrencyCode;
  customers: number;
}

export interface InvoiceCounts {
  draft: number;
  issued: number;
  void: number;
  /** Issued, due and still short. */
  overdue: number;
}

export interface UsageCoverage {
  with_device_and_interface: number;
  with_device_only: number;
  without_device: number;
  /** Without monitoring, no customer has a usage figure. */
  monitoring_present: boolean;
  note: string;
}

/**
 * Result of `isp.summary` (no params): counts, states and per-currency money.
 *
 * This is the only result in the feature with **no personal detail in it** —
 * no name, no phone number, no email, no address — and it is therefore the
 * only one that may be put in front of a model.
 */
export interface Summary {
  customers: number;
  by_state: StateCount[];
  by_currency: CurrencyCount[];
  subscriptions_active: number;
  /** The megabits of every active subscription. */
  sold_mbit: number;
  tariffs: number;
  links: number;
  invoices: InvoiceCounts;
  /** Per currency, with no total across them. */
  arrears: CurrencyArrears[];
  /** The last 30 days, per currency as received. */
  received_30d: Totals;
  usage_coverage: UsageCoverage;
  note: string;
}

// --------------------------------------------------------------------- events

/** `isp.changed`: records a screen should refetch. */
export interface IspChanged {
  /** Some of `customers`, `tariffs`, `subscriptions`, `links`. */
  kinds: string[];
}

/** `isp.invoices.changed`. The `issued` event carries the figure too. */
export interface IspInvoicesChanged {
  change: 'drafted' | 'issued' | 'voided' | 'updated';
  ids: string[];
  customer_ids: string[];
  /** Only on `issued`. */
  currency?: CurrencyCode;
  /** Only on `issued`. Whole minor units of `currency`. */
  total_minor?: number;
  /** Only on `issued`. */
  due_at?: number | null;
}

/** `isp.payments.changed`. */
export interface IspPaymentsChanged {
  change: 'recorded' | 'voided';
  ids: string[];
  customer_ids: string[];
  invoice_ids: string[];
}

// -------------------------------------------------------------------- methods

/**
 * Every method of this feature, with its parameter and result types.
 *
 * | Method | Params | Result |
 * |---|---|---|
 * | `isp.currencies` | — | {@link CurrencyRow}[] |
 * | `isp.summary` | — | {@link Summary} |
 * | `isp.customers` | {@link CustomersParams} | {@link CustomerPage} |
 * | `isp.customer.get` | {@link CustomerGetParams} | {@link CustomerDetail} |
 * | `isp.customer.save` | {@link CustomerInput} | {@link SavedCustomer} |
 * | `isp.customer.state` | {@link CustomerStateParams} | {@link StateChange} |
 * | `isp.customer.delete` | `{id: string}` | {@link DeletedCustomer} |
 * | `isp.tariffs` | — | {@link TariffRow}[] |
 * | `isp.tariff.save` | {@link TariffInput} | {@link SavedTariff} |
 * | `isp.tariff.delete` | `{id: string}` | {@link Deleted} |
 * | `isp.subscriptions` | `{customer_id?, tariff_id?, state?}` | {@link SubscriptionRow}[] |
 * | `isp.subscription.create` | {@link SubscriptionInput} | {@link SavedSubscription} |
 * | `isp.subscription.amend` | {@link AmendParams} | {@link SavedSubscription} |
 * | `isp.subscription.end` | {@link EndSubscriptionParams} | {@link Subscription} |
 * | `isp.links` | — | {@link Link}[] |
 * | `isp.link.save` | {@link LinkInput} | {@link SavedLink} |
 * | `isp.link.delete` | `{id: string}` | {@link Deleted} |
 * | `isp.invoices` | {@link InvoicesParams} | {@link InvoiceView}[] |
 * | `isp.invoice.get` | `{id: string}` | {@link InvoiceView} |
 * | `isp.invoice.draft` | {@link DraftParams} | {@link DraftedInvoice} |
 * | `isp.invoice.run` | {@link RunParams} | {@link RunResult} |
 * | `isp.invoice.line.add` | {@link LineInput} | {@link Invoice} |
 * | `isp.invoice.line.remove` | {@link LineRemoveParams} | {@link Invoice} |
 * | `isp.invoice.issue` | {@link IssueParams} | {@link Invoice} |
 * | `isp.invoice.void` | {@link VoidParams} | {@link Invoice} |
 * | `isp.invoice.reissue` | {@link VoidParams} | {@link Reissued} |
 * | `isp.payments` | {@link PaymentsParams} | {@link Payment}[] |
 * | `isp.payment.record` | {@link PaymentInput} | {@link RecordedPayment} |
 * | `isp.payment.void` | {@link VoidPaymentParams} | {@link Payment} |
 * | `isp.arrears` | {@link ArrearsParams} | {@link Arrears} |
 * | `isp.revenue` | {@link RevenueParams} | {@link Revenue} |
 * | `isp.usage` | {@link UsageParams} | {@link UsageReport} |
 * | `isp.capacity` | {@link CapacityParams} | {@link LinkCapacity}[] |
 * | `isp.margin` | {@link MarginParams} | {@link Margin}[] |
 *
 * The events: `isp.changed` ({@link IspChanged}), `isp.invoices.changed`
 * ({@link IspInvoicesChanged}) and `isp.payments.changed`
 * ({@link IspPaymentsChanged}).
 *
 * Error codes worth handling by name: `invalid` (the request is wrong — a
 * float amount, an unknown currency, a missing rate), `conflict` (it clashes
 * with what exists — an issued invoice, a period already billed, a customer
 * with a ledger) and `not_found`. Every message is written to be shown.
 */
export type IspMethod =
  | 'isp.currencies'
  | 'isp.summary'
  | 'isp.customers'
  | 'isp.customer.get'
  | 'isp.customer.save'
  | 'isp.customer.state'
  | 'isp.customer.delete'
  | 'isp.tariffs'
  | 'isp.tariff.save'
  | 'isp.tariff.delete'
  | 'isp.subscriptions'
  | 'isp.subscription.create'
  | 'isp.subscription.amend'
  | 'isp.subscription.end'
  | 'isp.links'
  | 'isp.link.save'
  | 'isp.link.delete'
  | 'isp.invoices'
  | 'isp.invoice.get'
  | 'isp.invoice.draft'
  | 'isp.invoice.run'
  | 'isp.invoice.line.add'
  | 'isp.invoice.line.remove'
  | 'isp.invoice.issue'
  | 'isp.invoice.void'
  | 'isp.invoice.reissue'
  | 'isp.payments'
  | 'isp.payment.record'
  | 'isp.payment.void'
  | 'isp.arrears'
  | 'isp.revenue'
  | 'isp.usage'
  | 'isp.capacity'
  | 'isp.margin';
