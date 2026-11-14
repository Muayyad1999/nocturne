import { describe, expect, it } from 'vitest';
import type { PrefixRow } from '@/api/types/ipam';
import {
  contentsText,
  descendantsOf,
  groupPrefixes,
  recordState,
  scopeOfSite,
  seenOn,
  siteLabel,
  sitesOf,
  utilisation,
  visibleRows,
} from './tree';

function prefix(over: Partial<PrefixRow> = {}): PrefixRow {
  return {
    id: over.cidr ?? 'p1',
    cidr: '10.0.0.0/24',
    name: '',
    description: '',
    tags: [],
    source: 'manual',
    discovered_from: [],
    created_at: 1_000,
    updated_at: 1_000,
    parent_id: null,
    depth: 0,
    children: 0,
    usable: 254,
    usable_text: '254',
    used: 0,
    reserved: 0,
    free: 254,
    util_pct: 0,
    util_known: true,
    documented: 0,
    observed: 0,
    child_coverage_pct: 0,
    devices: [],
    ...over,
  };
}

describe('scopes', () => {
  it('reads a lab site as a namespace of its own and names the lab when it can', () => {
    const names = new Map([['abc123', 'Campus']]);
    expect(scopeOfSite('lab:abc123', names)).toEqual({
      id: 'abc123',
      kind: 'lab',
      labId: 'abc123',
      label: 'Lab: Campus',
    });
    // No lab feature in the build means no name: the id is shown, not invented.
    expect(scopeOfSite('lab:abc123').label).toBe('Lab: abc123');
    expect(scopeOfSite('HQ').kind).toBe('production');
    expect(scopeOfSite(undefined).kind).toBe('production');
  });

  it('shows a plain site as itself and no site as nothing at all', () => {
    expect(siteLabel('HQ')).toBe('HQ');
    expect(siteLabel(undefined)).toBeNull();
    expect(siteLabel('')).toBeNull();
    expect(siteLabel('lab:x', new Map([['x', 'Bench']]))).toBe('Lab: Bench');
  });

  it('lists the sites in the answer, with lab roots named', () => {
    const rows = [
      prefix({ id: 'a', site: 'HQ' }),
      prefix({ id: 'b', site: 'HQ' }),
      prefix({ id: 'c', site: 'lab:x' }),
      prefix({ id: 'd' }),
    ];
    expect(sitesOf(rows, new Map([['x', 'Bench']]))).toEqual([
      { value: 'HQ', label: 'HQ' },
      { value: 'lab:x', label: 'Lab: Bench' },
    ]);
  });
});

describe('the containment tree', () => {
  it('keeps a lab’s blocks in a root of their own, production first', () => {
    const groups = groupPrefixes([
      prefix({ id: 'lab1', cidr: '10.10.10.0/24', site: 'lab:x', source: 'discovered' }),
      prefix({ id: 'hq', cidr: '10.20.0.0/16', site: 'HQ' }),
      prefix({ id: 'hq-child', cidr: '10.20.1.0/24', site: 'HQ', parent_id: 'hq', depth: 1 }),
    ]);
    expect(groups.map((group) => group.scope.kind)).toEqual(['production', 'lab']);
    expect(groups[0]?.roots.map((root) => root.row.id)).toEqual(['hq']);
    expect(groups[0]?.roots[0]?.children.map((child) => child.row.id)).toEqual(['hq-child']);
    expect(groups[0]?.total).toBe(2);
    expect(groups[0]?.discovered).toBe(0);
    expect(groups[1]?.total).toBe(1);
    expect(groups[1]?.discovered).toBe(1);
  });

  it('shows a prefix whose parent was filtered out as a root rather than dropping it', () => {
    // `ipam.prefixes {flat: true}` answers with matches only, so a child can
    // arrive without its parent. Losing it would hide a block that exists.
    const groups = groupPrefixes([
      prefix({ id: 'child', cidr: '10.20.1.0/24', site: 'HQ', parent_id: 'absent', depth: 1 }),
    ]);
    expect(groups[0]?.roots.map((root) => root.row.id)).toEqual(['child']);
    expect(visibleRows(groups[0]!, new Set())[0]?.depth).toBe(0);
  });

  it('draws nothing under a collapsed prefix, and counts the children it has', () => {
    const groups = groupPrefixes([
      prefix({ id: 'root', cidr: '10.0.0.0/8' }),
      prefix({ id: 'mid', cidr: '10.1.0.0/16', parent_id: 'root', depth: 1 }),
      prefix({ id: 'leaf', cidr: '10.1.1.0/24', parent_id: 'mid', depth: 2 }),
    ]);
    const group = groups[0]!;
    expect(visibleRows(group, new Set()).map((entry) => [entry.row.id, entry.depth])).toEqual([
      ['root', 0],
      ['mid', 1],
      ['leaf', 2],
    ]);
    expect(visibleRows(group, new Set(['mid'])).map((entry) => entry.row.id)).toEqual([
      'root',
      'mid',
    ]);
    expect(visibleRows(group, new Set())[0]?.children).toBe(1);
    expect(descendantsOf(group, 'root')).toEqual(['mid', 'leaf']);
    expect(descendantsOf(group, 'leaf')).toEqual([]);
  });

  it('has no groups at all for an empty answer, rather than an empty production root', () => {
    expect(groupPrefixes([])).toEqual([]);
  });
});

describe('utilisation', () => {
  it('reports a measured zero as a zero', () => {
    const reading = utilisation(prefix({ used: 0, free: 254, util_pct: 0 }));
    expect(reading).toMatchObject({ known: true, pct: 0, used: 0, free: 254, tone: 'idle' });
  });

  it('refuses to turn an unknowable percentage into 0 %', () => {
    const reading = utilisation(
      prefix({
        cidr: '2001:db8::/64',
        util_known: false,
        // What the backend sends for a /64: `usable` has already lost
        // precision as a JSON number, `usable_text` has not.
        usable: 18_446_744_073_709_551_615,
        usable_text: '18446744073709551615',
        util_pct: 0,
        used: 3,
      }),
    );
    expect(reading.known).toBe(false);
    expect(reading).not.toHaveProperty('pct');
    expect(reading.usableText).toBe('18446744073709551615');
    expect(reading.used).toBe(3);
  });

  it('calls a full block full and a busy block busy', () => {
    // The tone only exists where a percentage does, which is the point of the
    // two shapes: reading it has to go through the `known` branch.
    const toneOf = (row: PrefixRow) => {
      const reading = utilisation(row);
      return reading.known ? reading.tone : null;
    };
    expect(toneOf(prefix({ used: 254, util_pct: 100, free: 0 }))).toBe('crit');
    expect(toneOf(prefix({ used: 200, util_pct: 78.7 }))).toBe('warn');
    expect(toneOf(prefix({ used: 7, util_pct: 2.8 }))).toBe('ok');
    expect(toneOf(prefix({ util_known: false, usable_text: '18446744073709551615' }))).toBeNull();
  });

  it('counts what is documented and what was seen as two different numbers', () => {
    expect(contentsText(prefix({ documented: 0, observed: 7 }))).toBe(
      '0 documented · 7 seen on the network',
    );
    expect(contentsText(prefix({ documented: 2, observed: 0, reserved: 1 }))).toBe(
      '2 documented · 0 seen on the network · 1 reserved',
    );
  });
});

describe('a record and a discovery are not the same thing', () => {
  it('says which one a block is, and what that claims', () => {
    expect(recordState(prefix({ source: 'manual' })).kind).toBe('documented');
    const discovered = recordState(prefix({ source: 'discovered' }));
    expect(discovered.kind).toBe('discovered');
    expect(discovered.meaning).toContain('Nobody has documented it');
  });

  it('names the interfaces a discovered block was inferred from, and caps the list', () => {
    const sighting = (device: string, iface: string) => ({
      device_id: device,
      device,
      interface: iface,
      cidr: '10.0.0.1/30',
      collected_at: 5,
    });
    expect(
      seenOn(
        prefix({
          source: 'discovered',
          discovered_from: [sighting('DS1', 'Gi1/0/1'), sighting('EDGE', 'Gi0/1')],
        }),
      ),
    ).toBe('Seen on DS1 (Gi1/0/1), EDGE (Gi0/1)');
    expect(
      seenOn(
        prefix({
          source: 'discovered',
          discovered_from: [
            sighting('a', 'Gi0'),
            sighting('b', 'Gi1'),
            sighting('c', 'Gi2'),
            sighting('d', 'Gi3'),
          ],
        }),
      ),
    ).toContain('and 1 more');
    // Nothing to show is null, never "Seen on".
    expect(seenOn(prefix())).toBeNull();
  });
});
