/**
 * The prefix tree's reasoning, with no React in it.
 *
 * Two things live here, and both are places where a screen can quietly lie.
 *
 * 1. **A plan and a network are different things.** `ipam.prefixes` returns
 *    blocks a person wrote down (`source: "manual"`) and blocks that exist
 *    only because an address was seen inside them (`source: "discovered"`) in
 *    one list, and the second kind is nobody's plan yet. {@link recordState}
 *    keeps them apart and {@link seenOn} says which device and interface a
 *    discovered block was inferred from, because that is the whole of its
 *    provenance.
 * 2. **A percentage is not always a number.** A block larger than 64 bits of
 *    address space has no meaningful utilisation, and the backend says so with
 *    `util_known: false` and a `usable_text` that is exact where `usable` (a
 *    JSON number) has already lost precision. {@link utilisation} therefore
 *    returns two different shapes, so a caller cannot render "0 %" for a block
 *    whose fullness is unknown, and never formats `usable` itself.
 *
 * Lab blocks carry `site: "lab:<lab id>"` and are kept in a root of their own
 * ({@link groupPrefixes}): a simulated address must never be counted towards
 * production, and the backend already refuses to mix them.
 */

import type { PrefixRow, Sighting } from '@/api/types/ipam';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';

/** The site marker the backend puts on everything inside a lab. */
export const LAB_SITE_PREFIX = 'lab:';

/** Where a block lives: the real estate, or one lab's own namespace. */
export interface Scope {
  /** `''` for production, the lab's id otherwise. */
  id: string;
  kind: 'production' | 'lab';
  /** The lab's id, or null in production. */
  labId: string | null;
  /** What to call this root on screen. */
  label: string;
}

/**
 * Reads a prefix's `site`. A site is only a label in production, but
 * `lab:<id>` is a namespace: nothing in it shares arithmetic with anything
 * outside it.
 */
export function scopeOfSite(
  site: string | null | undefined,
  labNames?: ReadonlyMap<string, string>,
): Scope {
  if (site && site.startsWith(LAB_SITE_PREFIX)) {
    const labId = site.slice(LAB_SITE_PREFIX.length);
    const name = labNames?.get(labId);
    return {
      id: labId,
      kind: 'lab',
      labId,
      // Without the lab feature in the build there is no name to ask for, so
      // the id is shown rather than a prettier guess.
      label: name ? `Lab: ${name}` : `Lab: ${labId}`,
    };
  }
  return { id: '', kind: 'production', labId: null, label: 'Documented network' };
}

/** A site as it should read in a cell: the lab marker spelt out. */
export function siteLabel(
  site: string | null | undefined,
  labNames?: ReadonlyMap<string, string>,
): string | null {
  if (!site) return null;
  if (site.startsWith(LAB_SITE_PREFIX)) return scopeOfSite(site, labNames).label;
  return site;
}

// ------------------------------------------------------------------ the tree

export interface Branch {
  row: PrefixRow;
  children: Branch[];
}

export interface Group {
  scope: Scope;
  roots: Branch[];
  /** Every prefix in this root, at any depth. */
  total: number;
  /** Of those, the ones nobody has documented. */
  discovered: number;
}

/**
 * Builds the containment tree, one root per namespace: production first, then
 * one root per lab in a stable order.
 *
 * `ipam.prefixes` already answers in address order with `parent_id` and
 * `depth` filled in, and keeps the ancestors of a match so a filtered tree
 * stays whole. It does not promise that every parent is present, though — a
 * `flat` call or a site filter can leave a child whose parent was dropped — so
 * a row whose parent is missing from the answer is shown as a root of its own
 * rather than disappearing with it.
 */
export function groupPrefixes(
  rows: readonly PrefixRow[],
  labNames?: ReadonlyMap<string, string>,
): Group[] {
  const present = new Set(rows.map((row) => row.id));
  const branches = new Map<string, Branch>();
  for (const row of rows) branches.set(row.id, { row, children: [] });

  const groups = new Map<string, Group>();
  const groupFor = (row: PrefixRow): Group => {
    const scope = scopeOfSite(row.site, labNames);
    const existing = groups.get(scope.id);
    if (existing) return existing;
    const created: Group = { scope, roots: [], total: 0, discovered: 0 };
    groups.set(scope.id, created);
    return created;
  };

  for (const row of rows) {
    const branch = branches.get(row.id);
    if (!branch) continue;
    const group = groupFor(row);
    group.total += 1;
    if (row.source === 'discovered') group.discovered += 1;
    const parent = row.parent_id !== null && present.has(row.parent_id)
      ? branches.get(row.parent_id)
      : undefined;
    if (parent) parent.children.push(branch);
    else group.roots.push(branch);
  }

  return [...groups.values()].sort((a, b) => {
    if (a.scope.kind !== b.scope.kind) return a.scope.kind === 'production' ? -1 : 1;
    return a.scope.label.localeCompare(b.scope.label);
  });
}

export interface VisibleRow {
  row: PrefixRow;
  /** Depth inside this root, which is not `row.depth` when an ancestor is absent. */
  depth: number;
  /** Children in this answer; `row.children` counts them in the database. */
  children: number;
  expanded: boolean;
}

/**
 * The rows to draw for one root, in address order, with everything under a
 * collapsed prefix left out. Collapsing is the screen's own state, so the set
 * holds the ids the user closed.
 */
export function visibleRows(group: Group, collapsed: ReadonlySet<string>): VisibleRow[] {
  const out: VisibleRow[] = [];
  const walk = (branches: readonly Branch[], depth: number): void => {
    for (const branch of branches) {
      const expanded = !collapsed.has(branch.row.id);
      out.push({ row: branch.row, depth, children: branch.children.length, expanded });
      if (expanded) walk(branch.children, depth + 1);
    }
  };
  walk(group.roots, 0);
  return out;
}

/** Every prefix id under `id` (not including it), for collapsing a whole branch. */
export function descendantsOf(group: Group, id: string): string[] {
  const found: string[] = [];
  const collect = (branch: Branch): void => {
    for (const child of branch.children) {
      found.push(child.row.id);
      collect(child);
    }
  };
  const find = (branches: readonly Branch[]): boolean =>
    branches.some((branch) => {
      if (branch.row.id === id) {
        collect(branch);
        return true;
      }
      return find(branch.children);
    });
  find(group.roots);
  return found;
}

// --------------------------------------------------------------- utilisation

/**
 * How full a block is — or, for a block too large for that to mean anything,
 * the fact that there is no answer. Two shapes on purpose: `0 %` and "no
 * percentage" must not be able to render as the same thing.
 */
export type Utilisation =
  | {
      known: true;
      pct: number;
      used: number;
      reserved: number;
      free: number;
      usableText: string;
      tone: StatusTone;
    }
  | {
      known: false;
      used: number;
      usableText: string;
      /** Why there is no percentage. */
      reason: string;
    };

/** Above this share of a block, being nearly full is news. */
export const BUSY_PCT = 75;
export const FULL_PCT = 90;

export function utilisation(row: PrefixRow): Utilisation {
  if (!row.util_known) {
    return {
      known: false,
      used: row.used,
      // `usable` is a JSON number and a /64 has already lost precision in it;
      // `usable_text` is the backend's exact figure.
      usableText: row.usable_text,
      reason: 'The block is too large for a percentage to mean anything, so only what is known to be in use is counted.',
    };
  }
  const pct = row.util_pct;
  const tone: StatusTone =
    pct >= FULL_PCT ? 'crit' : pct >= BUSY_PCT ? 'warn' : row.used + row.reserved === 0 ? 'idle' : 'ok';
  return {
    known: true,
    pct,
    used: row.used,
    reserved: row.reserved,
    free: row.free,
    usableText: row.usable_text,
    tone,
  };
}

/**
 * What the two halves of a block's contents are, in words: how much of it a
 * person wrote down, and how much of it the network has shown. They are
 * different counts of different things and the screen says both.
 */
export function contentsText(row: PrefixRow): string {
  const parts = [
    `${formatCount(row.documented)} documented`,
    `${formatCount(row.observed)} seen on the network`,
  ];
  if (row.reserved > 0) parts.push(`${formatCount(row.reserved)} reserved`);
  return parts.join(' · ');
}

// ------------------------------------------------------------ record or not

export interface RecordState {
  /** `documented`: somebody wrote it down. `discovered`: nobody has. */
  kind: 'documented' | 'discovered';
  label: string;
  tone: StatusTone;
  /** One sentence for a tooltip or a detail panel. */
  meaning: string;
}

export function recordState(row: PrefixRow): RecordState {
  if (row.source === 'discovered') {
    return {
      kind: 'discovered',
      label: 'Discovered',
      tone: 'info',
      meaning:
        'This block exists because an address was seen inside it. Nobody has documented it, so it is not part of anybody’s plan yet.',
    };
  }
  return {
    kind: 'documented',
    label: 'Documented',
    tone: 'ok',
    meaning: 'Somebody wrote this block down. The network is checked against it, not the other way round.',
  };
}

/** `Campus/DS1 (Gi1/0/1)`, for a sighting or an interface in a block. */
export function sightingText(sighting: Pick<Sighting, 'device' | 'interface'>): string {
  return sighting.interface ? `${sighting.device} (${sighting.interface})` : sighting.device;
}

/**
 * Where a discovered block came from. The provenance is the whole of its
 * standing, so it is never summarised away to "discovered".
 */
export function seenOn(row: PrefixRow, limit = 3): string | null {
  if (row.discovered_from.length === 0) return null;
  const named = row.discovered_from.slice(0, limit).map(sightingText);
  const rest = row.discovered_from.length - named.length;
  return `Seen on ${named.join(', ')}${rest > 0 ? ` and ${formatCount(rest)} more` : ''}`;
}

/** The devices with an interface configured inside a block. */
export function devicesText(row: PrefixRow): string | null {
  if (row.devices.length === 0) return null;
  return `${plural(row.devices.length, 'interface')} in it`;
}

/** Which sites the answer contains, for the filter. Lab roots are named. */
export function sitesOf(
  rows: readonly PrefixRow[],
  labNames?: ReadonlyMap<string, string>,
): { value: string; label: string }[] {
  const seen = new Map<string, string>();
  for (const row of rows) {
    if (!row.site) continue;
    if (seen.has(row.site)) continue;
    seen.set(row.site, siteLabel(row.site, labNames) ?? row.site);
  }
  return [...seen.entries()]
    .map(([value, label]) => ({ value, label }))
    .sort((a, b) => a.label.localeCompare(b.label));
}
