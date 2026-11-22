/**
 * Everything these screens read and write.
 *
 * Four events keep them current, and the feature publishes nothing on a
 * read: `isp.changed {kinds}` after a customer, tariff, subscription or link
 * is saved, `isp.invoices.changed` after an invoice is drafted, issued,
 * voided or edited, `isp.payments.changed` after a payment is recorded or
 * voided, and `isp.credits.changed` after a credit note is issued, set
 * against an invoice or voided. Every list here is invalidated by all four
 * rather than by the one that looks relevant: a payment or a credit note
 * changes an invoice's derived status, an issued invoice changes the
 * arrears, and a new subscription changes what a run would bill. Nothing
 * polls — a ledger does not change because a second went by.
 *
 * A fifth event is the core's: `settings.changed`. The `isp` section holds
 * the business's calendar, which decides what is overdue and where a month
 * begins, so a change to it re-reads the ledger ({@link useSettingsWatch}).
 * The calendar itself is read with {@link useCalendar}: every date these
 * screens type or show goes through it (`calendar.ts`), never through the
 * browser's zone.
 *
 * The ISP feature is only in the paid edition, so this build may not have it
 * at all: {@link useIspInstalled} asks `app.methods` once (the answer is
 * cached and shared) and the screen says so in a sentence instead of showing
 * controls that fail. The navigation does the same thing, in
 * `app/destinations.ts`.
 *
 * Two deliberate shapes:
 *
 * * **A billing run has two hooks, not one.** `isp.invoice.run` previews
 *   unless it is told `dry_run: false`, and the preview writes nothing, so it
 *   invalidates nothing either — {@link usePreviewRun} leaves the caches
 *   alone and {@link useRunBilling} refetches everything. One hook with a
 *   flag would quietly refetch the whole screen to show a preview.
 * * **Reports are read on demand.** Revenue, usage, capacity and margin are
 *   readings of a moment and take a window; they are keyed by the window they
 *   were asked for, which is also why changing the window is a new query
 *   rather than a refetch.
 */

import { useQueryClient } from '@tanstack/react-query';
import {
  invalidateMethods,
  useEvent,
  useHasMethod,
  useRpc,
  useRpcMutation,
  type UseRpcOptions,
} from '@/api/hooks';
import type { DeviceRow, SettingsSetParams } from '@/api/types/core';
import type {
  AllocateCreditParams,
  AmendParams,
  Arrears,
  CapacityParams,
  CreditInput,
  CreditNote,
  CreditsParams,
  CurrencyRow,
  CustomerDetail,
  CustomerInput,
  CustomerPage,
  CustomersParams,
  CustomerStateParams,
  Deleted,
  DeletedCustomer,
  DraftedInvoice,
  DraftParams,
  EndSubscriptionParams,
  Invoice,
  InvoicesParams,
  InvoiceView,
  IspSettings,
  IssuedCredit,
  IssueParams,
  Link,
  LinkCapacity,
  LinkInput,
  LineInput,
  LineRemoveParams,
  Margin,
  MarginParams,
  Payment,
  PaymentInput,
  PaymentsParams,
  RecordedPayment,
  Reissued,
  Revenue,
  RevenueParams,
  RunParams,
  RunResult,
  SavedCustomer,
  SavedLink,
  SavedSubscription,
  SavedTariff,
  StateChange,
  Subscription,
  SubscriptionInput,
  SubscriptionRow,
  Summary,
  TariffInput,
  TariffRow,
  UsageParams,
  UsageReport,
  VoidCreditParams,
  VoidParams,
  VoidPaymentParams,
} from '@/api/types/isp';
import { isOffset, type Offset } from './calendar';

/**
 * Every topic that can make anything on these screens out of date.
 *
 * `isp.credits.changed` is here although the engine follows each credit
 * event with one on an older topic: a screen that shows credit notes follows
 * the topic that is about them, rather than depending on the second event.
 */
export const CHANGED = [
  'isp.changed',
  'isp.invoices.changed',
  'isp.payments.changed',
  'isp.credits.changed',
];

/** Method prefixes a mutation invalidates: the whole feature, for the reason above. */
const INVALIDATES = ['isp.'];

const live: UseRpcOptions = { invalidateOn: CHANGED };

/**
 * Is the ISP feature in this build? `undefined` while `app.methods` is in
 * flight — the screen waits for proof rather than saying "not installed"
 * about a feature that is.
 */
export function useIspInstalled(): boolean | undefined {
  return useHasMethod('isp.customers');
}

// ------------------------------------------------------------------- settings

/** The settings section the engine declares (`api.rs: SETTINGS_SECTION`). */
export const SETTINGS_SECTION = 'isp';

/** The ISP settings, read with the core's `settings.get`. */
export function useIspSettings() {
  return useRpc<IspSettings>(
    'settings.get',
    { section: SETTINGS_SECTION },
    { invalidateOn: ['settings.changed'] },
  );
}

/**
 * Saves some of the ISP settings (`settings.set` merges key by key). The
 * ledger is re-read afterwards as well as the settings: the calendar decides
 * which invoices are overdue and the payment terms decide nothing already
 * issued, but the screen cannot know which key was written.
 */
export function useSaveIspSettings() {
  return useRpcMutation<IspSettings, SettingsSetParams>('settings.set', {
    invalidates: ['settings.', ...INVALIDATES],
    errorToast: false,
  });
}

/**
 * The business's calendar: its offset from UTC in minutes, or `null` while
 * the setting has not been read or does not hold a calendar. With `null`
 * there is no date to show and none to accept — the helpers in `calendar.ts`
 * answer with nothing rather than fall back to the browser's zone.
 */
export function useCalendar(): Offset {
  const minutes = useIspSettings().data?.utc_offset_minutes;
  return isOffset(minutes) ? minutes : null;
}

/**
 * Re-reads the ledger when the ISP settings change, from this window or
 * another. Mounted once by each ISP route; a list's own `invalidateOn`
 * cannot do it, because `settings.changed` is every section's event and a
 * change of theme is no reason to read the invoices again.
 */
export function useSettingsWatch(): void {
  const queryClient = useQueryClient();
  useEvent<{ section?: string }>('settings.changed', (payload) => {
    if (payload.section === SETTINGS_SECTION) void invalidateMethods(queryClient, INVALIDATES);
  });
}

// ------------------------------------------------------------------ reference

/**
 * The currencies the backend accepts, with their minor digits. A fixed table
 * in the engine, so it is read once and never invalidated; every amount field
 * needs it to know whether a decimal point is allowed at all.
 */
export function useCurrencies() {
  return useRpc<CurrencyRow[]>('isp.currencies');
}

export function useSummary() {
  return useRpc<Summary>('isp.summary', undefined, live);
}

// ------------------------------------------------------------------ customers

export function useCustomers(params: CustomersParams) {
  return useRpc<CustomerPage>('isp.customers', params, live);
}

export function useCustomer(id: string | null, invoices = 20) {
  return useRpc<CustomerDetail>('isp.customer.get', id ? { id, invoices } : undefined, {
    ...live,
    enabled: id !== null,
  });
}

export function useSaveCustomer() {
  return useRpcMutation<SavedCustomer, CustomerInput>('isp.customer.save', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useSetCustomerState() {
  return useRpcMutation<StateChange, CustomerStateParams>('isp.customer.state', {
    invalidates: INVALIDATES,
  });
}

export function useDeleteCustomer() {
  return useRpcMutation<DeletedCustomer, { id: string }>('isp.customer.delete', {
    invalidates: INVALIDATES,
  });
}

// -------------------------------------------------------------------- tariffs

export function useTariffs() {
  return useRpc<TariffRow[]>('isp.tariffs', undefined, live);
}

export function useSaveTariff() {
  return useRpcMutation<SavedTariff, TariffInput>('isp.tariff.save', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useDeleteTariff() {
  return useRpcMutation<Deleted, { id: string }>('isp.tariff.delete', {
    invalidates: INVALIDATES,
  });
}

// -------------------------------------------------------------- subscriptions

export function useSubscriptions(params: { customer_id?: string; tariff_id?: string; state?: string }) {
  return useRpc<SubscriptionRow[]>('isp.subscriptions', params, live);
}

export function useCreateSubscription() {
  return useRpcMutation<SavedSubscription, SubscriptionInput>('isp.subscription.create', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useAmendSubscription() {
  return useRpcMutation<SavedSubscription, AmendParams>('isp.subscription.amend', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useEndSubscription() {
  return useRpcMutation<Subscription, EndSubscriptionParams>('isp.subscription.end', {
    invalidates: INVALIDATES,
  });
}

// ---------------------------------------------------------------------- links

export function useLinks() {
  return useRpc<Link[]>('isp.links', undefined, live);
}

export function useSaveLink() {
  return useRpcMutation<SavedLink, LinkInput>('isp.link.save', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useDeleteLink() {
  return useRpcMutation<Deleted, { id: string }>('isp.link.delete', { invalidates: INVALIDATES });
}

// ------------------------------------------------------------------- invoices

export function useInvoices(params: InvoicesParams, enabled = true) {
  return useRpc<InvoiceView[]>('isp.invoices', params, { ...live, enabled });
}

export function useInvoice(id: string | null) {
  return useRpc<InvoiceView>('isp.invoice.get', id ? { id } : undefined, {
    ...live,
    enabled: id !== null,
  });
}

export function useDraftInvoice() {
  return useRpcMutation<DraftedInvoice, DraftParams>('isp.invoice.draft', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

/**
 * A preview. `dry_run` defaults to true in the backend and is passed
 * explicitly here so that the request itself says what it is; nothing is
 * written, so nothing is invalidated.
 */
export function usePreviewRun() {
  return useRpcMutation<RunResult, RunParams>('isp.invoice.run', { errorToast: false });
}

/** The run that writes. The caller passes `dry_run: false` and means it. */
export function useRunBilling() {
  return useRpcMutation<RunResult, RunParams>('isp.invoice.run', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useAddLine() {
  return useRpcMutation<Invoice, LineInput>('isp.invoice.line.add', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useRemoveLine() {
  return useRpcMutation<Invoice, LineRemoveParams>('isp.invoice.line.remove', {
    invalidates: INVALIDATES,
  });
}

export function useIssueInvoice() {
  return useRpcMutation<Invoice, IssueParams>('isp.invoice.issue', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useVoidInvoice() {
  return useRpcMutation<Invoice, VoidParams>('isp.invoice.void', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useReissueInvoice() {
  return useRpcMutation<Reissued, VoidParams>('isp.invoice.reissue', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

// ------------------------------------------------------------------- payments

export function usePayments(params: PaymentsParams) {
  return useRpc<Payment[]>('isp.payments', params, live);
}

export function useRecordPayment() {
  return useRpcMutation<RecordedPayment, PaymentInput>('isp.payment.record', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useVoidPayment() {
  return useRpcMutation<Payment, VoidPaymentParams>('isp.payment.void', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

// --------------------------------------------------------------- credit notes

export function useCredits(params: CreditsParams, enabled = true) {
  return useRpc<CreditNote[]>('isp.credits', params, { ...live, enabled });
}

export function useIssueCredit() {
  return useRpcMutation<IssuedCredit, CreditInput>('isp.credit.issue', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

/** Sets a note that is on the account against one invoice. Once only. */
export function useAllocateCredit() {
  return useRpcMutation<IssuedCredit, AllocateCreditParams>('isp.credit.allocate', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

export function useVoidCredit() {
  return useRpcMutation<CreditNote, VoidCreditParams>('isp.credit.void', {
    invalidates: INVALIDATES,
    errorToast: false,
  });
}

// -------------------------------------------------------------------- reports

export function useArrears(customerId?: string) {
  return useRpc<Arrears>('isp.arrears', customerId ? { customer_id: customerId } : {}, live);
}

export function useRevenue(params: RevenueParams | null) {
  return useRpc<Revenue>('isp.revenue', params ?? undefined, {
    ...live,
    enabled: params !== null,
  });
}

export function useUsage(params: UsageParams) {
  return useRpc<UsageReport>('isp.usage', params, live);
}

export function useCapacity(params: CapacityParams) {
  return useRpc<LinkCapacity[]>('isp.capacity', params, live);
}

export function useMargin(params: MarginParams) {
  return useRpc<Margin[]>('isp.margin', params, live);
}

/**
 * The devices a customer's service or a link can be joined to.
 *
 * `devices.list` is another feature's method, so it is asked for only when
 * the build answers it, and its absence is not an error here: the screen then
 * takes the device id as text, which is what the backend stores anyway.
 */
export function useDeviceNames(): DeviceRow[] {
  const present = useHasMethod('devices.list') === true;
  const devices = useRpc<DeviceRow[]>('devices.list', undefined, { enabled: present });
  return devices.data ?? [];
}
