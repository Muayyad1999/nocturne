/**
 * Reconciliation, read properly — the reasoning behind the screen's hardest
 * claim, with no React in it.
 *
 * `ipam.reconcile` compares the plan with the network and **decides nothing**.
 * Its lists are therefore only as good as what was looked at, which is why it
 * leads with `coverage` and why every "nothing is there" answer carries
 * `had_coverage`. The two sentences this module exists to keep apart:
 *
 * * **"Nobody looked."** No device that could have seen the address has ever
 *   been collected from. The plan is not wrong; it is unchecked.
 * * **"It is not there."** Devices were collected from and none of them
 *   showed the address.
 *
 * Both are rendered by {@link unobservedReading}, and neither is allowed to
 * become the other. {@link coverageReading} does the same for the report as a
 * whole: a reconciliation with no collection behind it is not a clean bill of
 * health, and {@link coverageReading} returns `kind: 'none'` for it rather
 * than a reassuring 0.
 *
 * {@link sections} lists what the report found in a fixed order with a
 * sentence for each, including a sentence for the ones that found nothing —
 * an empty section is a result, not an absence of one.
 */

import type {
  Coverage,
  DeviceCoverage,
  Mismatch,
  Reconciliation,
  Unobserved,
} from '@/api/types/ipam';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';
import { formatRelative } from '@/lib/time';

export type CoverageKind = 'none' | 'partial' | 'full';

export interface CoverageReading {
  kind: CoverageKind;
  tone: StatusTone;
  /** The headline: what was looked at, as a figure that cannot be misread. */
  headline: string;
  /** What that means for everything below it. */
  detail: string;
  devicesTotal: number;
  devicesWithAddresses: number;
  devicesNeverCollected: number;
  /** Null when nothing has ever been collected. */
  oldest: number | null;
  newest: number | null;
}

/**
 * What was looked at, before anything that was found.
 *
 * `devices_total: 0` is not 100 % coverage of nothing: with no inventory there
 * is nothing to compare the plan against, and the report below says nothing at
 * all. That is `kind: 'none'`.
 */
export function coverageReading(coverage: Coverage, now: number = Date.now()): CoverageReading {
  const {
    devices_total: total,
    devices_with_addresses: withAddresses,
    devices_never_collected: never,
    oldest_collection: oldest,
    newest_collection: newest,
  } = coverage;

  const base = {
    devicesTotal: total,
    devicesWithAddresses: withAddresses,
    devicesNeverCollected: never,
    oldest,
    newest,
  };

  if (total === 0) {
    return {
      ...base,
      kind: 'none',
      tone: 'idle',
      headline: 'Nothing has been collected',
      detail:
        'There is no device in the inventory to compare the plan with, so nothing below is evidence of anything.',
    };
  }
  if (withAddresses === 0) {
    return {
      ...base,
      kind: 'none',
      tone: 'warn',
      headline: `No addresses read from ${plural(total, 'device')}`,
      detail:
        'No device has reported an address, so an address the plan claims cannot be confirmed or contradicted. Collect from the devices first.',
    };
  }
  const read = `${formatCount(withAddresses)} of ${plural(total, 'device')} reported addresses`;
  const when = newest === null ? '' : `, most recently ${formatRelative(newest, now)}`;
  if (never > 0) {
    return {
      ...base,
      kind: 'partial',
      tone: 'warn',
      headline: read + when,
      detail: `${plural(never, 'device')} ${
        never === 1 ? 'has' : 'have'
      } never been collected from: anything the plan says about them is unchecked, not confirmed.`,
    };
  }
  return {
    ...base,
    kind: 'full',
    tone: 'ok',
    headline: read + when,
    detail:
      'Every device in the inventory has been collected from, so "nothing answered" below really means nothing answered.',
  };
}

/** The devices that contributed nothing, and the reason the backend gives. */
export function silentDevices(coverage: Coverage): DeviceCoverage[] {
  return coverage.devices.filter(
    (device) => device.interfaces_at === null || device.addresses_configured === 0,
  );
}

export interface UnobservedReading {
  /** Two sentences that must never be confused for each other. */
  headline: 'Nobody looked' | 'Not on the network';
  tone: StatusTone;
  detail: string;
}

/**
 * A documented address the network has not shown. Whether that is evidence
 * depends entirely on `had_coverage`.
 */
export function unobservedReading(row: Unobserved, now: number = Date.now()): UnobservedReading {
  if (!row.had_coverage) {
    return {
      headline: 'Nobody looked',
      tone: 'idle',
      detail:
        'Nothing that could have seen this address has been collected from, so there is no evidence either way.',
    };
  }
  const last =
    row.last_seen === null
      ? 'It has never been seen.'
      : `It was last seen ${formatRelative(row.last_seen, now)}.`;
  return {
    headline: 'Not on the network',
    tone: 'warn',
    detail: `Devices that could have seen this address were collected from and none of them show it. ${last}`,
  };
}

// --------------------------------------------------------------- mismatches

export interface GroupedMismatch {
  ip: string;
  field: string;
  documented: string;
  observed: string;
  /** Every device that shows it, with when its table was read. */
  evidence: string[];
}

/**
 * One contradiction per address and field, with its evidence listed under it.
 *
 * The report carries a row per *observation*, so one wrong MAC seen by three
 * devices is three rows that read as three faults. And for `field: "device"`
 * both sides are device **ids** (the same backend wart as
 * {@link module:addresses.disagreementText}), so a name resolver is taken and
 * an id with no name is shown as the id.
 */
export function groupMismatches(
  mismatches: readonly Mismatch[],
  deviceName: (id: string) => string | undefined,
  now: number = Date.now(),
): GroupedMismatch[] {
  const groups = new Map<string, GroupedMismatch>();
  for (const row of mismatches) {
    const resolve = (value: string) =>
      row.field === 'device' ? (deviceName(value) ?? value) : value;
    const documented = resolve(row.documented);
    const observed = resolve(row.observed);
    const key = `${row.scope}\u0000${row.ip}\u0000${row.field}\u0000${documented}\u0000${observed}`;
    const where = row.interface ? `${row.device} (${row.interface})` : row.device;
    const evidence = `${where} · read ${formatRelative(row.collected_at, now)}`;
    const group = groups.get(key);
    if (group) {
      if (!group.evidence.includes(evidence)) group.evidence.push(evidence);
    } else {
      groups.set(key, { ip: row.ip, field: row.field, documented, observed, evidence: [evidence] });
    }
  }
  return [...groups.values()];
}

// ----------------------------------------------------------------- sections

export type SectionId =
  | 'undocumented'
  | 'unobserved'
  | 'mismatches'
  | 'mask_mismatches'
  | 'prefixes_not_documented'
  | 'prefixes_without_evidence'
  | 'conflicts';

export interface Section {
  id: SectionId;
  title: string;
  /** What this section is, in one sentence, whether or not it found anything. */
  about: string;
  /** What it means that this section found nothing. */
  empty: string;
  count: number;
  /** The backend capped this list at 500 rows. */
  truncated: boolean;
  tone: StatusTone;
}

const SECTION_ORDER: readonly {
  id: SectionId;
  title: string;
  about: string;
  empty: string;
  tone: StatusTone;
}[] = [
  {
    id: 'undocumented',
    title: 'On the network, not in the plan',
    about: 'Addresses a device is configured with, or that answered, that nobody wrote down.',
    empty: 'Every address the network showed has a record.',
    tone: 'warn',
  },
  {
    id: 'unobserved',
    title: 'In the plan, not on the network',
    about: 'Address records nothing has shown — which, where nobody looked, is not evidence.',
    empty: 'Every documented address has been seen.',
    tone: 'info',
  },
  {
    id: 'mismatches',
    title: 'The record and the device disagree',
    about: 'One address, two answers about whose it is, which interface it is on, or its MAC.',
    empty: 'Nothing recorded contradicts what the devices report.',
    tone: 'crit',
  },
  {
    id: 'mask_mismatches',
    title: 'An interface mask that is not the block’s',
    about: 'An interface configured with a different prefix length from the block it sits in.',
    empty: 'Every interface mask matches the block it is in.',
    tone: 'warn',
  },
  {
    id: 'prefixes_not_documented',
    title: 'Blocks only the network knows',
    about: 'Prefixes that exist because something was seen inside them; nobody has documented them.',
    empty: 'Every block on record was put there by a person.',
    tone: 'info',
  },
  {
    id: 'prefixes_without_evidence',
    title: 'Blocks nothing has been seen in',
    about: 'Documented prefixes no device has ever shown an address from.',
    empty: 'Something has been seen in every documented block.',
    tone: 'idle',
  },
  {
    id: 'conflicts',
    title: 'Conflicts',
    about: 'Things that cannot be true at the same time. Reported, never resolved.',
    empty: 'Nothing contradicts itself.',
    tone: 'crit',
  },
];

export function sections(report: Reconciliation): Section[] {
  return SECTION_ORDER.map((entry) => ({
    ...entry,
    count: report[entry.id].length,
    truncated: report.truncated.includes(entry.id),
  }));
}

/** The lists that hit the backend's cap, so a count reads as a floor. */
export function truncatedNote(report: Reconciliation): string | null {
  if (report.truncated.length === 0) return null;
  return `These lists were cut off at the backend's limit, so their counts are a floor and not a total: ${report.truncated.join(', ')}.`;
}

/** Is there anything at all to show, apart from coverage? */
export function foundSomething(report: Reconciliation): boolean {
  return sections(report).some((section) => section.count > 0);
}
