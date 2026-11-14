import { describe, expect, it } from 'vitest';
import type { Address, Disagreement, NextAddresses, Observation } from '@/api/types/ipam';
import {
  addressHits,
  addressScope,
  addressState,
  disagreementText,
  documentedText,
  gridCounts,
  gridScopeText,
  groupDisagreements,
  humaniseBasis,
  nextAddressSummary,
  planMarks,
  seenText,
  skippedLines,
  STATE_WORDS,
  stateMeaning,
} from './addresses';

function observation(over: Partial<Observation> = {}): Observation {
  return {
    ip: '10.10.10.2',
    kind: 'interface',
    device_id: 'd-ds1',
    device: 'Campus/DS1',
    interface: 'Vlan10',
    scope: '',
    first_seen: 1_000,
    last_seen: 2_000,
    collected_at: 2_000,
    collector: 'cli',
    current: true,
    ...over,
  };
}

/** The shape `ipam.prefix.get` sends for an address nothing is known about. */
function address(over: Partial<Address> = {}): Address {
  return {
    ip: '10.10.10.4',
    prefix_id: 'p1',
    status: 'free',
    hostname: null,
    description: null,
    device_id: null,
    interface: null,
    mac: null,
    vendor: null,
    source: 'manual',
    first_seen: null,
    last_seen: null,
    documented_at: null,
    tags: [],
    documented: false,
    observed: false,
    observations: [],
    disagreements: [],
    ...over,
  };
}

describe('the four states of an address', () => {
  it('tells documented, seen, both and neither apart', () => {
    expect(addressState(address())).toBe('unknown');
    expect(addressState(address({ documented: true, status: 'active' }))).toBe('documented');
    expect(addressState(address({ observed: true, status: 'active' }))).toBe('observed');
    expect(addressState(address({ documented: true, observed: true, status: 'active' }))).toBe(
      'both',
    );
    // A conflict is louder than either half of the row.
    expect(addressState(address({ documented: true, observed: true, status: 'conflict' }))).toBe(
      'conflict',
    );
  });

  it('never calls an address with nothing known about it free', () => {
    const words = STATE_WORDS.unknown;
    expect(words.label).not.toMatch(/free|available/i);
    expect(words.meaning).toContain('not the same as knowing it is free');
  });

  it('counts every state, including the ones that are zero', () => {
    const counts = gridCounts([
      address(),
      address({ ip: '10.10.10.2', documented: true, observed: true, status: 'active' }),
      address({ ip: '10.10.10.3', observed: true, status: 'active' }),
      address({ ip: '10.10.10.9', documented: true, status: 'reserved' }),
      address({ ip: '10.10.10.150', documented: true, status: 'dhcp' }),
    ]);
    expect(counts).toEqual({
      total: 5,
      both: 1,
      documented: 2,
      observed: 1,
      unknown: 1,
      conflict: 0,
      reserved: 1,
      dhcp: 1,
    });
    // An empty block still reports each state as a measured zero.
    expect(gridCounts([])).toEqual({
      total: 0,
      both: 0,
      documented: 0,
      observed: 0,
      unknown: 0,
      conflict: 0,
      reserved: 0,
      dhcp: 0,
    });
  });

  it('says whether the list is the whole block or only what is known', () => {
    const counts = gridCounts([address()]);
    expect(gridScopeText(true, counts)).toBe('Every address of the block: 1 rows');
    expect(gridScopeText(false, counts)).toBe('1 addresses with a record or a sighting');
  });
});

describe('a time that never happened is not an old time', () => {
  const now = 10_000_000;

  it('returns nothing for an address that was never seen, rather than 1970', () => {
    expect(seenText(address(), now)).toBeNull();
    expect(seenText(address({ observed: true, last_seen: now - 60_000 }), now)).toBe('1m ago');
  });

  it('returns nothing for an address with no record, rather than 1970', () => {
    expect(documentedText(address({ observed: true }), now)).toBeNull();
    // A record always carries the time it was written; a row that is only a
    // sighting carries null, and must not be read as the epoch.
    expect(documentedText(address({ documented: true, documented_at: null }), now)).toBeNull();
    expect(documentedText(address({ documented: true, documented_at: now - 3_600_000 }), now)).toBe(
      '1h ago',
    );
  });
});

describe('a disagreement shows both sides and the evidence', () => {
  const now = 2_000_000;

  it('resolves the device ids the backend puts on both sides of a device disagreement', () => {
    const disagreement: Disagreement = {
      field: 'device',
      documented: 'd-as1',
      observed: 'd-ds1',
      device_id: 'd-ds1',
      device: 'Campus/DS1',
      interface: 'Vlan10',
      collected_at: now - 120_000,
    };
    const names = new Map([
      ['d-as1', 'Campus/AS1'],
      ['d-ds1', 'Campus/DS1'],
    ]);
    expect(disagreementText(disagreement, (id) => names.get(id), now)).toEqual({
      field: 'device',
      documented: 'Campus/AS1',
      observed: 'Campus/DS1',
      evidence: 'Campus/DS1 (Vlan10), read 2m ago',
    });
    // An id with no name left is shown as the id: never as nothing.
    expect(disagreementText(disagreement, () => undefined, now).documented).toBe('d-as1');
  });

  it('leaves a MAC or an interface disagreement exactly as the backend wrote it', () => {
    const disagreement: Disagreement = {
      field: 'mac',
      documented: 'aa:bb:cc:dd:ee:ff',
      observed: '02:4e:43:d0:fd:40',
      device_id: 'd-ds1',
      device: 'Campus/DS1',
      interface: 'Vlan10',
      collected_at: now,
    };
    const text = disagreementText(disagreement, () => 'should not be used', now);
    expect(text.documented).toBe('aa:bb:cc:dd:ee:ff');
    expect(text.observed).toBe('02:4e:43:d0:fd:40');
  });
});

describe('one contradiction, however many devices saw it', () => {
  const now = 2_000_000;

  function mac(device: string, at: number): Disagreement {
    return {
      field: 'mac',
      documented: 'aa:bb:cc:dd:ee:ff',
      observed: '02:4e:43:d0:fd:40',
      device_id: device,
      device,
      interface: 'Vlan10',
      collected_at: at,
    };
  }

  it('groups the same written-versus-seen pair and keeps every piece of evidence', () => {
    // The backend reports one row per *observation*, so three sightings of one
    // wrong MAC arrive as three identical disagreements.
    const grouped = groupDisagreements(
      [mac('DS1', now), mac('DS2', now - 60_000), mac('DS1', now)],
      () => undefined,
      now,
    );
    expect(grouped).toHaveLength(1);
    expect(grouped[0]?.documented).toBe('aa:bb:cc:dd:ee:ff');
    // The repeated sighting from DS1 at the same time is one line, not two.
    expect(grouped[0]?.evidence).toEqual([
      'DS1 (Vlan10), read just now',
      'DS2 (Vlan10), read 1m ago',
    ]);
  });

  it('keeps two different fields apart', () => {
    const device: Disagreement = {
      field: 'device',
      documented: 'd-as1',
      observed: 'd-ds1',
      device_id: 'd-ds1',
      device: 'Campus/DS1',
      interface: 'Vlan10',
      collected_at: now,
    };
    const grouped = groupDisagreements([mac('DS1', now), device], (id) => `name:${id}`, now);
    expect(grouped.map((entry) => entry.field)).toEqual(['mac', 'device']);
    expect(grouped[1]?.documented).toBe('name:d-as1');
  });

  it('says the right thing about an address that is documented, seen and wrong', () => {
    const agreeing = address({ documented: true, observed: true, status: 'active' });
    expect(stateMeaning(agreeing)).toBe(STATE_WORDS.both.meaning);
    const disagreeing = address({
      documented: true,
      observed: true,
      status: 'active',
      disagreements: [mac('DS1', now)],
    });
    expect(stateMeaning(disagreeing)).toContain('but not as it was written');
    expect(stateMeaning(disagreeing)).not.toContain('where it should be');
  });
});

describe('the address hits of a search', () => {
  const inLab = (ip: string, over: Partial<Address> = {}) =>
    address({
      ip,
      documented: true,
      observed: true,
      status: 'active',
      hostname: 'gw-staff',
      prefix_id: 'lab-block',
      observations: [observation({ ip, scope: 'lab:x' })],
      ...over,
    });
  /** What `ipam.search` adds for a whole typed address: production, empty. */
  const typedInProduction = (ip: string) => address({ ip, prefix_id: null });
  const siteOf = (id: string) => (id === 'lab-block' ? 'lab:x' : 'HQ');

  it('reads a row’s namespace from its sighting, then from its block, then production', () => {
    expect(addressScope(inLab('10.10.10.2'), siteOf)).toBe('lab:x');
    // Documented in a lab with nothing seen yet: the block says where it is.
    expect(
      addressScope(
        address({ ip: '10.10.10.9', documented: true, prefix_id: 'lab-block' }),
        siteOf,
      ),
    ).toBe('lab:x');
    expect(addressScope(typedInProduction('10.10.10.2'), siteOf)).toBe('');
    // A production block is still production: only `lab:` is a namespace.
    expect(
      addressScope(address({ ip: '10.20.1.50', documented: true, prefix_id: 'hq' }), siteOf),
    ).toBe('');
  });

  it('drops the empty production row the backend adds beside a real hit', () => {
    // Since the engine learned to find an address (db.rs, c5fe67c) both rows
    // arrive for one typed address; listing them both says the same address
    // is known and unknown at once.
    const hits = addressHits([inLab('10.10.10.2'), typedInProduction('10.10.10.2')], (row) =>
      addressScope(row, siteOf),
    );
    expect(hits).toHaveLength(1);
    expect(hits[0]?.scope).toBe('lab:x');
    expect(hits[0]?.known).toBe(true);
  });

  it('keeps the empty row when nothing knows the address anywhere', () => {
    // "Nothing is known about this address" is the answer to that search.
    const hits = addressHits([typedInProduction('192.0.2.9')], (row) => addressScope(row, siteOf));
    expect(hits).toHaveLength(1);
    expect(hits[0]?.known).toBe(false);
  });

  it('lists one address known in two namespaces as two hits', () => {
    const hits = addressHits(
      [
        inLab('10.10.10.2'),
        address({
          ip: '10.10.10.2',
          documented: true,
          status: 'active',
          hostname: 'production gw',
          prefix_id: 'hq',
        }),
      ],
      (row) => addressScope(row, siteOf),
    );
    expect(hits.map((hit) => hit.scope).sort()).toEqual(['', 'lab:x']);
  });

  it('collapses the same address and namespace, keeping the row that knows something', () => {
    const hits = addressHits(
      [address({ ip: '10.10.10.2', prefix_id: 'lab-block' }), inLab('10.10.10.2')],
      (row) => addressScope(row, siteOf),
    );
    expect(hits).toHaveLength(1);
    expect(hits[0]?.address.hostname).toBe('gw-staff');
  });

  it('leaves unrelated addresses alone', () => {
    const hits = addressHits(
      [inLab('10.10.10.2'), typedInProduction('10.20.1.50')],
      (row) => addressScope(row, siteOf),
    );
    expect(hits.map((hit) => hit.address.ip)).toEqual(['10.10.10.2', '10.20.1.50']);
  });
});

describe('what the plan marks on the grid', () => {
  it('marks the documented gateway and the documented DHCP range, and nothing else', () => {
    const mark = planMarks({
      gateway: '10.20.1.1',
      dhcp: { start: '10.20.1.100', end: '10.20.1.200' },
    });
    expect(mark('10.20.1.1')).toBe('gateway');
    expect(mark('10.20.1.100')).toBe('dhcp');
    expect(mark('10.20.1.150')).toBe('dhcp');
    expect(mark('10.20.1.200')).toBe('dhcp');
    expect(mark('10.20.1.201')).toBeNull();
    expect(mark('10.20.1.99')).toBeNull();
    expect(mark('10.20.1.2')).toBeNull();
  });

  it('marks nothing at all for a block that documents neither', () => {
    const mark = planMarks({});
    expect(mark('10.20.1.1')).toBeNull();
    expect(planMarks({ gateway: '', dhcp: null })('10.20.1.1')).toBeNull();
  });

  it('works on IPv6, where a range is not a string comparison', () => {
    const mark = planMarks({
      gateway: '2001:db8::1',
      dhcp: { start: '2001:db8::10', end: '2001:db8::100' },
    });
    expect(mark('2001:db8::1')).toBe('gateway');
    // ::20 is above ::100 as text and below it as an address.
    expect(mark('2001:db8::20')).toBe('dhcp');
    expect(mark('2001:db8::101')).toBeNull();
  });
});

describe('the basis of a next-free answer', () => {
  function answer(over: Partial<NextAddresses> = {}): NextAddresses {
    return {
      prefix_id: 'p1',
      cidr: '10.10.10.0/24',
      addresses: ['10.10.10.4'],
      exhausted: false,
      skipped: {
        network: 1,
        broadcast: 1,
        anycast: 0,
        gateway: 0,
        dhcp_range: 0,
        documented: 0,
        seen_on_an_interface: 2,
        seen_in_arp: 1,
        asked_to_skip: 0,
      },
      basis: '16 sighting(s) in this prefix',
      ...over,
    };
  }

  it('turns the epoch milliseconds the backend embeds into a time a person can read', () => {
    const at = 1_794_614_077_077;
    const text = humaniseBasis(
      `16 sighting(s) in this prefix, last collected at ${at}; an address nothing has answered on is not known to be free`,
      at + 60_000,
    );
    expect(text).not.toContain('1794614077077');
    expect(text).toContain('1m ago');
    // The claim itself is quoted, not paraphrased.
    expect(text).toContain('is not known to be free');
  });

  it('leaves a basis with no timestamp in it alone', () => {
    const basis =
      'nothing has ever been seen in this prefix: these addresses are not documented, which is not the same as known to be free';
    expect(humaniseBasis(basis, 1_000)).toBe(basis);
  });

  it('lists only the exclusions that applied, so nothing reads as a reason when it is zero', () => {
    expect(skippedLines(answer()).map((line) => [line.label, line.count])).toEqual([
      ['network address', 1],
      ['broadcast address', 1],
      ['configured on an interface', 2],
      ['answered in ARP recently', 1],
    ]);
    expect(
      skippedLines(
        answer({
          skipped: {
            network: 0,
            broadcast: 0,
            anycast: 0,
            gateway: 0,
            dhcp_range: 0,
            documented: 0,
            seen_on_an_interface: 0,
            seen_in_arp: 0,
            asked_to_skip: 0,
          },
        }),
      ),
    ).toEqual([]);
  });

  it('says when the block ran out instead of pretending the count was the ask', () => {
    expect(nextAddressSummary(answer({ addresses: ['a', 'b'] }))).toBe('2 addresses');
    expect(nextAddressSummary(answer({ addresses: ['a'], exhausted: true }))).toContain(
      'the block ran out',
    );
  });
});

describe('sightings', () => {
  it('keeps the kind of each sighting, because an interface outweighs an ARP entry', () => {
    const row = address({
      observed: true,
      status: 'active',
      observations: [observation(), observation({ kind: 'arp', device: 'Campus/DS2' })],
    });
    expect(row.observations.map((entry) => entry.kind)).toEqual(['interface', 'arp']);
  });
});
