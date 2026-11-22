/**
 * What the records are called on screen: account states, pricing units,
 * billing periods, and what a subscription says. No React in here.
 *
 * The account states are the part worth being careful about. `suspended` is a
 * decision about an account, and `isp.customer.state` records it and answers
 * `device_changed: false` — nothing in the feature can reach a device
 * (`docs/dev/features/isp.md`: "a billing bug that disconnects a paying
 * customer is a worse failure than a late suspension"). So the words here say
 * what marking an account does, and none of them promises that a service will
 * stop: {@link STATE_WORDS} is the only place those sentences exist, and
 * `records.test.ts` reads every one of them for a promise to cut somebody
 * off.
 */

import type {
  CustomerState,
  Link,
  LinkInput,
  MoneyInput,
  Period,
  SubscriptionRow,
  Unit,
} from '@/api/types/isp';
import type { StatusTone } from '@/design/tone';
import { formatCount } from '@/lib/format';

export interface StateWords {
  label: string;
  tone: StatusTone;
  /** What this state means for the ledger. */
  meaning: string;
  /** What marking it does — and does not do. Shown before it is done. */
  consequence: string;
}

export const STATE_WORDS: Record<CustomerState, StateWords> = {
  active: {
    label: 'Active',
    tone: 'ok',
    meaning: 'Billed as normal. Their plans are counted as sold on the links they are behind.',
    consequence:
      'The account is marked active and will be billed by a run again. Nothing is sent to a device.',
  },
  suspended: {
    label: 'Suspended',
    tone: 'warn',
    meaning:
      'A decision about the account: it is left out of a billing run, and what it was sold is still counted against the link, apart.',
    consequence:
      'This records a decision about the account and nothing else: Nocturne does not configure anything, so the service keeps running until somebody changes the router.',
  },
  cancelled: {
    label: 'Cancelled',
    tone: 'idle',
    meaning:
      'Closed. Their invoices and payments stay, because somebody will ask about them, and no new plan can be sold on the account.',
    consequence:
      'The account is closed for new plans. Its invoices and payments are kept, and nothing is sent to a device.',
  },
};

export const UNIT_WORDS: Record<Unit, string> = {
  per_mbit: 'per megabit',
  flat: 'flat',
};

export const PERIOD_WORDS: Record<Period, string> = {
  monthly: 'a month',
  quarterly: 'a quarter',
  yearly: 'a year',
};

/** How long one period of this kind must be, as the engine requires. */
export const PERIOD_WINDOW: Record<Period, string> = {
  monthly: 'a window of 28 to 31 days',
  quarterly: 'a window of 89 to 92 days',
  yearly: 'a window of 365 or 366 days',
};

/**
 * What a subscription sells, in words: the megabits and how the price was
 * quoted. The figures are the subscription's **own copy** of what was agreed,
 * never today's tariff.
 */
export function termsText(row: Pick<SubscriptionRow, 'mbit' | 'unit' | 'period'>): string {
  return `${formatCount(row.mbit)} Mbit/s, ${UNIT_WORDS[row.unit]}, ${PERIOD_WORDS[row.period]}`;
}

/** Why a subscription's figures do not move when the tariff does. */
export const AGREED_NOTE =
  'A subscription keeps its own copy of the megabits and the price that were agreed. ' +
  'Changing the tariff does not change what anybody already on it pays.';

/** What amending does, which is not editing: the old terms end, the new ones start. */
export const AMEND_NOTE =
  'Amending ends these terms and starts new ones from the date you give. ' +
  'Invoices already issued on the old terms are untouched and still say what they said.';

/** What the link dialog holds when it is saved. */
export interface LinkForm {
  name: string;
  description: string;
  tower: string;
  deviceId: string;
  iface: string;
  /** The capacity field as typed: a whole number, or empty for none. */
  capacity: string;
  /** The cost as parsed, or `null` when the field is empty. */
  cost: MoneyInput | null;
  costPeriod: Period;
}

/**
 * The parameters of `isp.link.save` for a form.
 *
 * `isp.link.save` keeps whatever a save leaves out — that is how a save of
 * the name alone leaves the capacity alone — so an emptied field cannot be
 * sent as an absent one: the dialog would say "updated", reopen with the old
 * figure, and the margin would go on subtracting a cost that somebody had
 * removed. Taking a figure off a link that has one is therefore said
 * outright: `capacity_mbit: 0`, and `clear_cost: true`. A new link, or one
 * that never had the figure, sends neither.
 */
export function linkInput(form: LinkForm, existing: Link | null): LinkInput {
  const capacity = form.capacity.trim();
  return {
    ...(existing ? { id: existing.id } : {}),
    name: form.name.trim(),
    description: form.description.trim(),
    tower: form.tower.trim(),
    device_id: form.deviceId.trim(),
    interface: form.iface.trim(),
    ...(capacity !== ''
      ? { capacity_mbit: Number(capacity) }
      : existing && existing.capacity_mbit !== 0
        ? { capacity_mbit: 0 }
        : {}),
    cost_period: form.costPeriod,
    ...(form.cost !== null
      ? { cost_per_mbit: form.cost }
      : existing?.cost_per_mbit
        ? { clear_cost: true }
        : {}),
  };
}

/**
 * A whole number between `min` and `max` from a settings field, or `null`.
 * Digits only: no sign, no point and no thousands separator, so nothing is
 * read as a different number than the one that was typed.
 */
export function parseWholeNumber(text: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(text.trim())) return null;
  const value = Number(text.trim());
  return Number.isSafeInteger(value) && value >= min && value <= max ? value : null;
}

/**
 * The arrears report's band edges, `30, 60, 90`, or `null` when the text is
 * not a list of whole days greater than zero, each larger than the one
 * before. The engine would sort and de-duplicate a list that was not; it is
 * refused here instead, so that what is saved is what was typed.
 */
export function parseBands(text: string): number[] | null {
  const parts = text
    .split(',')
    .map((part) => part.trim())
    .filter((part) => part !== '');
  if (parts.length === 0) return null;
  const bands: number[] = [];
  for (const part of parts) {
    const days = parseWholeNumber(part, 1, 36_500);
    const last = bands[bands.length - 1];
    if (days === null || (last !== undefined && days <= last)) return null;
    bands.push(days);
  }
  return bands;
}
