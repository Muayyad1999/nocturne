/**
 * What one address is, with no React in it — the four states, and the words.
 *
 * The backend keeps a **record** (what a person wrote) and an **observation**
 * (what a device said) in different tables, different types and different
 * fields, and `ipam.prefix.get` hands both halves over without merging them.
 * This module is the only place the screen decides what that pair means, and
 * it holds to three rules:
 *
 * 1. **Four states, not two.** Documented, seen, both, and neither are
 *    different facts about an address ({@link addressState}).
 * 2. **`free` is "nothing is known", never "available".** `Status.free` means
 *    no record and nothing has ever answered — which, in a block nobody has
 *    collected from, says nothing at all. So the word "free" is never used by
 *    itself: {@link STATE_WORDS} spells it out and the backend's own `note`
 *    and `coverage` are shown beside the grid.
 * 3. **An absent time is not an old time.** `first_seen`, `last_seen` and
 *    `documented_at` are `null` when the thing never happened, and `null`
 *    through a relative-time formatter reads as 1970 — "56 years ago" — which
 *    is a lie with a timestamp on it. {@link seenText} and
 *    {@link documentedText} return `null` instead and the caller writes
 *    "never".
 *
 * {@link humaniseBasis} exists for one backend wart: `ipam.next_address`
 * returns a sentence with a raw epoch-millisecond number in it (see NOTES.md),
 * and a person cannot read 1794614077077.
 */

import type { Address, Disagreement, NextAddresses, Observation } from '@/api/types/ipam';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';
import { compareIp } from '@/lib/net';
import { formatDateTime, formatRelative } from '@/lib/time';

/** The four states of one address, from the two halves of its row. */
export type AddressState = 'both' | 'documented' | 'observed' | 'unknown' | 'conflict';

export interface StateWords {
  /** One word for a legend or a badge. */
  label: string;
  /** What that word claims, in full. Shown, not implied. */
  meaning: string;
  tone: StatusTone;
}

export const STATE_WORDS: Record<AddressState, StateWords> = {
  both: {
    label: 'Documented and seen',
    meaning: 'A person wrote this address down and the network shows it where it should be.',
    tone: 'ok',
  },
  documented: {
    label: 'Documented only',
    meaning:
      'A person wrote this address down and nothing on the network has shown it. It may be switched off, or planned, or gone.',
    tone: 'info',
  },
  observed: {
    label: 'Seen only',
    meaning: 'A device has this address, or answered on it, and nobody wrote it down.',
    tone: 'warn',
  },
  unknown: {
    label: 'No record, nothing seen',
    meaning:
      'Nothing documents this address and nothing has ever answered on it. That is not the same as knowing it is free — see what was collected, below.',
    tone: 'idle',
  },
  conflict: {
    label: 'Conflict',
    meaning: 'Two things that cannot both be true are recorded or configured for this address.',
    tone: 'crit',
  },
};

export function addressState(address: Address): AddressState {
  if (address.status === 'conflict') return 'conflict';
  if (address.documented && address.observed) return 'both';
  if (address.documented) return 'documented';
  if (address.observed) return 'observed';
  return 'unknown';
}

/**
 * What the state of *this* address claims. It is the state's general meaning,
 * except for the one case where that meaning would be wrong: an address that
 * is documented **and** seen, where the record and the sighting do not agree
 * about whose it is. "The network shows it where it should be" is exactly
 * what has not happened there.
 */
export function stateMeaning(address: Address): string {
  const state = addressState(address);
  if (state === 'both' && address.disagreements.length > 0) {
    return 'A person wrote this address down and the network shows it — but not as it was written. The disagreements are below.';
  }
  return STATE_WORDS[state].meaning;
}

/** The word for a record's own status. `free` is the absence of a record. */
export const STATUS_LABEL: Record<Address['status'], string> = {
  active: 'In use',
  reserved: 'Reserved',
  dhcp: 'DHCP pool',
  free: 'No record',
  conflict: 'Conflict',
};

export interface GridCounts {
  /** Rows in the answer: the whole block for a grid, the known ones otherwise. */
  total: number;
  both: number;
  documented: number;
  observed: number;
  unknown: number;
  conflict: number;
  reserved: number;
  dhcp: number;
}

/**
 * The legend's numbers. Every state is counted even when it is zero: a
 * missing line and a line reading zero say different things, and a block
 * where nothing is documented is exactly what this screen is for.
 */
export function gridCounts(addresses: readonly Address[]): GridCounts {
  const counts: GridCounts = {
    total: addresses.length,
    both: 0,
    documented: 0,
    observed: 0,
    unknown: 0,
    conflict: 0,
    reserved: 0,
    dhcp: 0,
  };
  for (const address of addresses) {
    counts[addressState(address)] += 1;
    if (address.status === 'reserved') counts.reserved += 1;
    if (address.status === 'dhcp') counts.dhcp += 1;
  }
  return counts;
}

/** When the network last showed this address, or null when it never has. */
export function seenText(address: Address, now: number = Date.now()): string | null {
  if (address.last_seen === null) return null;
  return formatRelative(address.last_seen, now);
}

/** When the record was last written, or null when there is no record. */
export function documentedText(address: Address, now: number = Date.now()): string | null {
  if (!address.documented || address.documented_at === null) return null;
  return formatRelative(address.documented_at, now);
}

/** `interface` or `arp`, as a word with its weight. */
export const OBSERVATION_KIND_LABEL: Record<Observation['kind'], string> = {
  interface: 'Configured on an interface',
  arp: 'Answered in an ARP table',
};

/**
 * One sighting as a sentence: who saw it, where, when the table was read, and
 * whether it is still in that table.
 */
export function observationText(observation: Observation, now: number = Date.now()): string {
  const where = observation.interface
    ? `${observation.device} (${observation.interface})`
    : observation.device;
  const when = formatRelative(observation.collected_at, now);
  const state = observation.current ? '' : ', no longer in that table';
  return `${where}, read ${when}${state}`;
}

/** The devices that have shown an address, newest sighting first. */
export function sightingsNewestFirst(address: Address): Observation[] {
  return [...address.observations].sort((a, b) => b.collected_at - a.collected_at);
}

export interface DisagreementText {
  field: string;
  /** What the record says. A device id is resolved to its name where possible. */
  documented: string;
  /** What the device says. */
  observed: string;
  /** Who said it, and when the table was read. */
  evidence: string;
}

/**
 * A disagreement in words, with both sides and the evidence.
 *
 * For `field: "device"` the backend puts **device ids** on both sides (see
 * NOTES.md), so a name resolver is taken: an id with no name left is shown as
 * the id rather than as nothing.
 */
export function disagreementText(
  disagreement: Disagreement,
  deviceName: (id: string) => string | undefined,
  now: number = Date.now(),
): DisagreementText {
  const resolve = (value: string): string =>
    disagreement.field === 'device' ? (deviceName(value) ?? value) : value;
  const where = disagreement.interface
    ? `${disagreement.device} (${disagreement.interface})`
    : disagreement.device;
  return {
    field: disagreement.field,
    documented: resolve(disagreement.documented),
    observed: resolve(disagreement.observed),
    evidence: `${where}, read ${formatRelative(disagreement.collected_at, now)}`,
  };
}

export interface GroupedDisagreement {
  field: string;
  documented: string;
  observed: string;
  /** Every device that showed this, with when its table was read. */
  evidence: string[];
}

/**
 * The same contradiction, once.
 *
 * The backend reports a disagreement per *observation*, so one wrong MAC in a
 * record becomes three identical rows when three devices saw the address. The
 * contradiction is one fact with three pieces of evidence, and is shown that
 * way — nothing is dropped, the evidence is listed under it.
 */
export function groupDisagreements(
  disagreements: readonly Disagreement[],
  deviceName: (id: string) => string | undefined,
  now: number = Date.now(),
): GroupedDisagreement[] {
  const groups = new Map<string, GroupedDisagreement>();
  for (const disagreement of disagreements) {
    const text = disagreementText(disagreement, deviceName, now);
    const key = `${text.field}\u0000${text.documented}\u0000${text.observed}`;
    const group = groups.get(key);
    if (group) {
      if (!group.evidence.includes(text.evidence)) group.evidence.push(text.evidence);
    } else {
      groups.set(key, { ...text, evidence: [text.evidence] });
    }
  }
  return [...groups.values()];
}

// ------------------------------------------------------------ search hits

/**
 * Which namespace an address row belongs to.
 *
 * An {@link Address} carries no scope of its own: a sighting has one, and
 * otherwise the block the address falls in does. A row with neither — the
 * production-namespace row `ipam.search` adds for a whole typed address — is
 * production, which is what the backend asked about.
 */
export function addressScope(
  address: Address,
  siteOf: (prefixId: string) => string | null | undefined,
): string {
  const seen = address.observations[0]?.scope;
  if (seen !== undefined) return seen;
  const site = address.prefix_id === null ? undefined : siteOf(address.prefix_id);
  return site && site.startsWith('lab:') ? site : '';
}

export interface AddressHit {
  address: Address;
  /** `''` for production, `lab:<id>` for a lab. */
  scope: string;
  /** A record, a sighting, or both: something is known here. */
  known: boolean;
}

/**
 * The address hits of a search, one per address **and namespace**.
 *
 * `ipam.search` answers a whole typed address with a production-namespace row
 * whether or not anything is known there, on top of the rows its text search
 * really found. Since the engine learned to find an address properly
 * (`db.rs`, commit c5fe67c), that extra row usually arrives *beside* the real
 * one, and the same address would otherwise be listed twice — once with its
 * record and its sightings, once saying nothing is known about it.
 *
 * So: an address known in one namespace is listed there, and the empty row is
 * dropped. An address nothing knows anywhere keeps its single empty row,
 * because "nothing is known about this address" is the answer to that search,
 * not an artefact. An address known in two namespaces is two hits, and the
 * caller labels each with its namespace — a lab and production are different
 * places, not a duplicate.
 */
export function addressHits(
  addresses: readonly Address[],
  scopeOf: (address: Address) => string,
): AddressHit[] {
  const hits = new Map<string, AddressHit>();
  for (const address of addresses) {
    const scope = scopeOf(address);
    const known = address.documented || address.observed;
    const key = `${scope}\u0000${address.ip}`;
    const existing = hits.get(key);
    // The same address and namespace twice: keep whichever row knows something.
    if (!existing || (known && !existing.known)) hits.set(key, { address, scope, known });
  }
  const informed = new Set(
    [...hits.values()].filter((hit) => hit.known).map((hit) => hit.address.ip),
  );
  return [...hits.values()].filter((hit) => hit.known || !informed.has(hit.address.ip));
}

// ----------------------------------------------------- what the plan marks

/** What the *plan* says about one address, which is not what anything saw. */
export type PlanMark = 'gateway' | 'dhcp';

/**
 * The gateway and the DHCP range a block documents, as a lookup for the grid.
 *
 * These are records, not sightings: a cell is marked because somebody wrote
 * the block down that way, and the mark says so. Returns `null` for every
 * address when the block documents neither, so a grid with no plan behind it
 * shows no marks rather than guessing where a gateway "should" be.
 */
export function planMarks(prefix: {
  gateway?: string | null;
  dhcp?: { start: string; end: string } | null;
}): (ip: string) => PlanMark | null {
  const gateway = prefix.gateway ?? '';
  const dhcp = prefix.dhcp ?? null;
  return (ip: string) => {
    if (gateway !== '' && ip === gateway) return 'gateway';
    if (dhcp && compareIp(ip, dhcp.start) >= 0 && compareIp(ip, dhcp.end) <= 0) return 'dhcp';
    return null;
  };
}

export const PLAN_MARK_WORDS: Record<PlanMark, string> = {
  gateway: 'the gateway this block documents',
  dhcp: 'inside the DHCP range this block documents',
};

// ------------------------------------------------------- next free addresses

/** Matches a run of digits long enough to be an epoch in milliseconds. */
const EPOCH_MS = /\b1[0-9]{12}\b/g;

/**
 * The backend's `basis` sentence, with any epoch-millisecond stamp in it
 * turned into a time a person can read. The sentence itself is kept word for
 * word — it is the backend's claim about what its answer rests on, and this
 * screen is not in the business of paraphrasing it.
 */
export function humaniseBasis(basis: string, now: number = Date.now()): string {
  return basis.replace(EPOCH_MS, (digits) => {
    const at = Number(digits);
    if (!Number.isFinite(at)) return digits;
    return `${formatDateTime(at)} (${formatRelative(at, now)})`;
  });
}

export interface SkippedLine {
  label: string;
  count: number;
}

/**
 * Why addresses were passed over, in the order a person would ask. Only the
 * reasons that applied are listed — a zero here is noise, because the answer
 * itself is the point — but the list is never empty-but-silent: the caller
 * says "nothing was skipped" when it comes back empty.
 */
export function skippedLines(answer: NextAddresses): SkippedLine[] {
  const skipped = answer.skipped;
  const lines: SkippedLine[] = [
    { label: 'network address', count: skipped.network },
    { label: 'broadcast address', count: skipped.broadcast },
    { label: 'subnet-router anycast address', count: skipped.anycast },
    { label: 'the documented gateway', count: skipped.gateway },
    { label: 'inside the DHCP range', count: skipped.dhcp_range },
    { label: 'already documented', count: skipped.documented },
    { label: 'configured on an interface', count: skipped.seen_on_an_interface },
    { label: 'answered in ARP recently', count: skipped.seen_in_arp },
    { label: 'asked to be skipped', count: skipped.asked_to_skip },
  ];
  return lines.filter((line) => line.count > 0);
}

/** "3 addresses, and the block has more" / "only 2 were left". */
export function nextAddressSummary(answer: NextAddresses): string {
  const found = plural(answer.addresses.length, 'address', 'addresses');
  if (answer.exhausted) return `${found} — the block ran out before the rest could be found`;
  return found;
}

/** The DHCP range of a block as a phrase, or null when it has none. */
export function dhcpText(range: { start: string; end: string } | null | undefined): string | null {
  if (!range) return null;
  return `${range.start} – ${range.end}`;
}

/** How many of a block's addresses the answer actually carries. */
export function gridScopeText(grid: boolean, counts: GridCounts): string {
  return grid
    ? `Every address of the block: ${formatCount(counts.total)} rows`
    : `${formatCount(counts.total)} addresses with a record or a sighting`;
}
