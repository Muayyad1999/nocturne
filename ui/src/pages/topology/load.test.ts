import { describe, expect, it } from 'vitest';
import type { Link, Node, PortRate, Rates } from '@/api/types/topology';
import {
  busier,
  cadenceS,
  deviceIndex,
  freshness,
  layerState,
  linkLoad,
  loadBand,
  loadDetail,
  loadLabel,
  loadSentence,
  rateIndex,
  ratePorts,
  serverNow,
  worthLabelling,
  staleAfterMs,
  UNKNOWN_CADENCE_S,
  type LoadContext,
  type MonitoredDevice,
} from './load';

const NOW = 1_700_000_000_000;

/**
 * One port's answer from `topology.rates`. `name` is what monitoring calls
 * the port; `interface` is the spelling it was asked for by, which is the
 * link end's — the same word unless a test says otherwise. A `null` is a rate
 * that was not measured, which the backend leaves out of the answer.
 */
function row(
  extra: Pick<PortRate, 'device_id' | 'name'> & {
    interface?: string;
    in_bps?: number | null;
    out_bps?: number | null;
    speed_bps?: number | null;
    at?: number;
  },
): PortRate {
  const { in_bps, out_bps, speed_bps = 1_000_000_000, at = NOW - 10_000, ...rest } = extra;
  return {
    interface: extra.name,
    ...rest,
    ...(in_bps == null ? {} : { in_bps }),
    ...(out_bps == null ? {} : { out_bps }),
    ...(speed_bps == null ? {} : { speed_bps }),
    at,
  };
}

function link(extra: Partial<Link> = {}): Link {
  return {
    id: 'l1',
    a: { node: 'n:a', interface: 'GigabitEthernet0/1' },
    b: { node: 'n:b', interface: 'GigabitEthernet0/2' },
    kind: 'lldp',
    confidence: 'confirmed',
    status: 'up',
    ...extra,
  };
}

function polled(extra: Partial<MonitoredDevice['monitor']> = {}): MonitoredDevice {
  return { monitor: { enabled: true, snmp: true, interval_s: 60, ...extra } };
}

function context(rows: PortRate[], extra: Partial<LoadContext> = {}): LoadContext {
  return {
    deviceOf: new Map([
      ['n:a', 'dev-a'],
      ['n:b', 'dev-b'],
    ]),
    rates: rateIndex(rows),
    devices: new Map([
      ['dev-a', polled()],
      ['dev-b', polled()],
    ]),
    monitoring: { cli_interval_s: 300 },
    now: NOW,
    ...extra,
  };
}

function device(id: string, deviceId?: string): Node {
  return {
    id,
    kind: deviceId ? 'switch' : 'host',
    name: id,
    ...(deviceId ? { device_id: deviceId } : {}),
    lab: false,
    status: 'up',
    unmanaged: !deviceId,
    x: 0,
    y: 0,
    pinned: false,
  };
}

describe('joining a link to the ports that were measured', () => {
  it('asks about each port a link ends on once, and about nothing that is not a device’s port', () => {
    const ports = ratePorts({
      nodes: [device('n:b', 'dev-b'), device('n:a', 'dev-a'), device('u:ap'), device('h:laptop')],
      links: [
        link(),
        // The same port of dev-a again, at the end of a second link.
        link({ id: 'l2', a: { node: 'n:a', interface: 'GigabitEthernet0/1' }, b: { node: 'u:ap', interface: 'GigabitEthernet0' } }),
        // An end with no interface named, and a host that is not a device.
        link({ id: 'l3', a: { node: 'n:b' }, b: { node: 'h:laptop', interface: 'eth0' } }),
      ],
    });
    expect(ports).toEqual([
      { device_id: 'dev-a', interface: 'GigabitEthernet0/1' },
      { device_id: 'dev-b', interface: 'GigabitEthernet0/2' },
    ]);
    expect(ratePorts(undefined)).toEqual([]);
  });

  it('asks in the same order whatever order the drawing arrived in', () => {
    // The list is the query's key: a different order would be a different
    // question, asked again for the same drawing.
    const nodes = [device('n:a', 'dev-a'), device('n:b', 'dev-b')];
    const one = link();
    const other = link({ id: 'l2', a: { node: 'n:b', interface: 'Vlan10' }, b: { node: 'n:a', interface: 'Vlan10' } });
    expect(ratePorts({ nodes, links: [one, other] })).toEqual(
      ratePorts({ nodes: [...nodes].reverse(), links: [other, one] }),
    );
  });

  it('finds a port monitoring spells its own way, and quotes monitoring’s name for it', () => {
    // Two IOS XR routers: a link end is `TenGigabitEthernet0/0/0/1`, the
    // rows monitoring keeps are `TenGigE0/0/0/1`. The backend joined them and
    // answered under the spelling it was asked with.
    const xr = link({
      a: { node: 'n:a', interface: 'TenGigabitEthernet0/0/0/1' },
      b: { node: 'n:b', interface: 'TenGigabitEthernet0/0/0/1' },
    });
    const answer = (deviceId: string, outBps: number) =>
      row({
        device_id: deviceId,
        interface: 'TenGigabitEthernet0/0/0/1',
        name: 'TenGigE0/0/0/1',
        out_bps: outBps,
        speed_bps: 10_000_000_000,
      });
    const load = linkLoad(xr, context([answer('dev-a', 4_000_000_000), answer('dev-b', 1_000_000_000)]));
    expect(load.measured).toBe(true);
    expect(load.aToB.pct).toBeCloseTo(40);
    expect(load.bToA.pct).toBeCloseTo(10);
    expect(load.aToB.read).toEqual({ deviceId: 'dev-a', interface: 'TenGigE0/0/0/1', sense: 'sent' });
  });

  it('compares no interface names itself: the answer for another spelling is another port’s', () => {
    // Which two spellings are one port is the backend's to say, with the one
    // function the whole project uses for it. A second opinion here is how
    // `TenGigE` and `TenGigabitEthernet` came to be two ports.
    const load = linkLoad(
      link(),
      context([row({ device_id: 'dev-a', interface: 'Gi0/1', name: 'Gi0/1', out_bps: 1_000_000 })]),
    );
    expect(load.measured).toBe(false);
  });

  it('keeps the ports of two devices apart when they have the same name', () => {
    const index = rateIndex([
      row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 1 }),
      row({ device_id: 'dev-b', name: 'GigabitEthernet0/1', out_bps: 2 }),
    ]);
    expect(index.size).toBe(2);
  });
});

describe('a rate that was measured and a rate that was not', () => {
  it('calls a measured zero idle and an unmeasured port unknown, never both zero', () => {
    const load = linkLoad(
      link(),
      context([
        row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 0, in_bps: null }),
        // dev-b was never polled, so the other direction has no measurement.
      ]),
    );
    expect(load.aToB.kind).toBe('scaled');
    expect(load.aToB.bps).toBe(0);
    expect(load.aToB.pct).toBe(0);
    expect(loadLabel(load.aToB)).toBe('idle');
    expect(loadBand(load.aToB)).toBe('idle');

    expect(load.bToA.kind).toBe('unknown');
    expect(load.bToA.bps).toBeNull();
    expect(load.bToA.pct).toBeNull();
    expect(loadLabel(load.bToA)).toBe('no rate');
    expect(loadBand(load.bToA)).toBe('unknown');
  });

  it('gives an unmeasured direction no age at all rather than the age of now', () => {
    const load = linkLoad(link(), context([]));
    expect(load.measured).toBe(false);
    expect(load.at).toBeNull();
    expect(load.aToB.age).toEqual({ state: 'unknown', ms: null });
    // Nothing was measured, so nothing is stale either: "stale" is a claim
    // about a reading, and there is no reading.
    expect(load.stale).toBe(false);
  });

  it('keeps the two directions apart, because a link is rarely symmetrical', () => {
    const load = linkLoad(
      link(),
      context([
        row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 400_000_000, in_bps: 1_000_000 }),
        row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', out_bps: 2_000_000, in_bps: 390_000_000 }),
      ]),
    );
    expect(load.aToB.pct).toBeCloseTo(40);
    expect(load.bToA.pct).toBeCloseTo(0.2);
    expect(load.busiest).toBe(load.aToB);
  });

  it('prefers the sending port’s own counter and says which end the number came from', () => {
    const both = linkLoad(
      link(),
      context([
        row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 100_000_000 }),
        row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', in_bps: 99_000_000 }),
      ]),
    );
    expect(both.aToB.bps).toBe(100_000_000);
    expect(both.aToB.read).toEqual({
      deviceId: 'dev-a',
      interface: 'GigabitEthernet0/1',
      sense: 'sent',
    });

    // Only the far end is monitored: its receive counter is the next best
    // thing, and the panel has to be able to say so.
    const far = linkLoad(
      link(),
      context([row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', in_bps: 99_000_000 })]),
    );
    expect(far.aToB.bps).toBe(99_000_000);
    expect(far.aToB.read?.sense).toBe('received');
    expect(loadSentence(far.aToB, { from: 'DS1', to: 'AS1' })).toContain('received by');
  });
});

describe('a rate without a speed', () => {
  it('shows the bits per second and does not invent a percentage', () => {
    const load = linkLoad(
      link(),
      context([row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 24_000_000, speed_bps: null })]),
    );
    expect(load.aToB.kind).toBe('unscaled');
    expect(load.aToB.pct).toBeNull();
    expect(load.aToB.speedBps).toBeNull();
    expect(loadLabel(load.aToB)).toBe('24 Mb/s');
    expect(loadSentence(load.aToB, { from: 'DS1', to: 'AS1' })).toContain('no percentage');
  });

  it('takes the link’s own speed over a port that did not say, and ignores a zero', () => {
    const fromLink = linkLoad(
      link({ speed_bps: 100_000_000 }),
      context([row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 50_000_000, speed_bps: null })]),
    );
    expect(fromLink.aToB.speedBps).toBe(100_000_000);
    expect(fromLink.aToB.pct).toBeCloseTo(50);

    const zeroSpeed = linkLoad(
      link(),
      context([row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 50_000_000, speed_bps: 0 })]),
    );
    expect(zeroSpeed.aToB.kind).toBe('unscaled');
  });

  it('writes a rate too small to round as less than a tenth, not as nothing', () => {
    const load = linkLoad(
      link(),
      context([row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 400_000 })]),
    );
    expect(load.aToB.pct).toBeCloseTo(0.04);
    expect(loadLabel(load.aToB)).toBe('<0.1%');
  });
});

describe('how old a measurement may be', () => {
  it('judges a reading against how often the device is actually measured', () => {
    // SNMP: the device's own poll interval.
    expect(cadenceS(polled({ snmp: true, interval_s: 60 }), { cli_interval_s: 300 })).toBe(60);
    // No SNMP: the counters come from a command-line read, which happens no
    // more often than `cli_interval_s` however often the device is pinged.
    expect(cadenceS(polled({ snmp: false, interval_s: 60 }), { cli_interval_s: 300 })).toBe(300);
    expect(cadenceS(polled({ snmp: false, interval_s: 600 }), { cli_interval_s: 300 })).toBe(600);
    // Nothing says: neither the device nor the settings are known.
    expect(cadenceS(undefined, undefined)).toBeNull();
    expect(cadenceS(polled({ snmp: false }), undefined)).toBe(UNKNOWN_CADENCE_S);
  });

  it('allows one missed poll, with a floor so a fast poller does not flicker', () => {
    expect(staleAfterMs(300)).toBe(750_000);
    expect(staleAfterMs(5)).toBe(90_000);
    expect(staleAfterMs(null)).toBe(UNKNOWN_CADENCE_S * 2_500);
  });

  it('carries the cadence of the port each number came from, not the link’s', () => {
    const load = linkLoad(
      link(),
      context(
        [
          row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 1_000 }),
          row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', out_bps: 1_000 }),
        ],
        {
          devices: new Map([
            ['dev-a', polled({ snmp: true, interval_s: 5 })],
            ['dev-b', polled({ snmp: true, interval_s: 600 })],
          ]),
        },
      ),
    );
    expect(load.aToB.cadenceS).toBe(5);
    expect(load.bToA.cadenceS).toBe(600);
  });

  it('calls a reading from twenty minutes ago stale, and one from a minute ago live', () => {
    const limit = staleAfterMs(60);
    expect(freshness(NOW - 60_000, limit, NOW)).toEqual({ state: 'live', ms: 60_000 });
    expect(freshness(NOW - 20 * 60_000, limit, NOW).state).toBe('stale');
    expect(freshness(null, limit, NOW)).toEqual({ state: 'unknown', ms: null });
  });

  it('takes a current reading of what arrived over a stale one of what was sent', () => {
    // dev-a's transmit counter is twenty minutes old; dev-b's receive counter
    // was measured five seconds ago. On a map, what is happening now wins.
    const load = linkLoad(
      link(),
      context([
        row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 1_000_000, at: NOW - 20 * 60_000 }),
        row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', in_bps: 7_000_000, at: NOW - 5_000 }),
      ]),
    );
    expect(load.aToB.bps).toBe(7_000_000);
    expect(load.aToB.read?.sense).toBe('received');
    expect(load.aToB.age.state).toBe('live');
  });

  it('marks the link stale only when every reading it has is stale', () => {
    const rows = [
      row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 1_000_000, at: NOW - 20 * 60_000 }),
      row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', out_bps: 1_000_000, at: NOW - 5_000 }),
    ];
    const half = linkLoad(link(), context(rows));
    expect(half.aToB.age.state).toBe('stale');
    expect(half.bToA.age.state).toBe('live');
    expect(half.stale).toBe(false);
    // The "as of" line quotes the newest reading there is.
    expect(half.at).toBe(NOW - 5_000);

    const bothOld = linkLoad(
      link(),
      context(rows.map((entry) => ({ ...entry, at: NOW - 20 * 60_000 }))),
    );
    expect(bothOld.stale).toBe(true);
  });
});

describe('which direction gets the one number there is room for', () => {
  it('prefers a measurement to none, and a percentage to a bare rate', () => {
    const measured = linkLoad(
      link(),
      context([row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', out_bps: 10 })]),
    );
    expect(measured.busiest).toBe(measured.bToA);

    const scaled = linkLoad(
      link({ speed_bps: undefined }),
      context([
        row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 900_000_000, speed_bps: null }),
        row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', out_bps: 1_000, speed_bps: 1_000_000_000 }),
      ]),
    );
    // A huge number with nothing to compare it to loses to a real percentage:
    // the percentage is the one that says whether the link is in trouble.
    expect(scaled.busiest.kind).toBe('scaled');
  });

  it('is stable when the two directions are equally loaded', () => {
    const load = linkLoad(
      link(),
      context([
        row({ device_id: 'dev-a', name: 'GigabitEthernet0/1', out_bps: 5_000_000 }),
        row({ device_id: 'dev-b', name: 'GigabitEthernet0/2', out_bps: 5_000_000 }),
      ]),
    );
    expect(busier(load.aToB, load.bToA)).toBe(load.aToB);
    expect(busier(load.bToA, load.aToB)).toBe(load.bToA);
  });
});

describe('bands, so colour is never the only thing that says it', () => {
  it('uses monitoring’s own ninety per cent line for a port in trouble', () => {
    const banded = (pct: number) =>
      loadBand({
        from: 'a',
        to: 'b',
        kind: 'scaled',
        bps: pct * 10_000_000,
        speedBps: 1_000_000_000,
        pct,
        read: null,
        at: NOW,
        age: { state: 'live', ms: 0 },
        cadenceS: 60,
      });
    expect(banded(0.5)).toBe('light');
    expect(banded(49.9)).toBe('light');
    expect(banded(50)).toBe('busy');
    expect(banded(89.9)).toBe('busy');
    expect(banded(90)).toBe('hot');
    expect(banded(140)).toBe('hot');
  });
});

describe('what shows at what zoom', () => {
  it('draws nothing at all when a card is a smudge', () => {
    expect(loadDetail(0.1, 400)).toBe('none');
    expect(loadDetail(0.19, 4_000)).toBe('none');
  });

  it('draws the bars before the writing, because a bar is readable when tiny', () => {
    expect(loadDetail(0.3, 400)).toBe('bars');
    expect(loadDetail(0.59, 400)).toBe('bars');
  });

  it('writes one number when there is room for one and two when there is room for two', () => {
    expect(loadDetail(0.7, 400)).toBe('busiest');
    expect(loadDetail(1.2, 400)).toBe('both');
  });

  it('keeps a number off a link that is too short on screen to carry it', () => {
    expect(loadDetail(1.5, 60)).toBe('bars');
    expect(loadDetail(1.5, 100)).toBe('busiest');
  });

  it('draws nothing at all on a line too short to put a bar on', () => {
    // Two cards almost touching: the wire between them is forty pixels long,
    // and a channel either side of it is a smudge. Zoom in and it comes back.
    expect(loadDetail(1.5, 40)).toBe('none');
    expect(loadDetail(3, 40)).toBe('none');
  });
});

describe('which numbers earn their ink', () => {
  const direction = (pct: number | null, kind: 'scaled' | 'unscaled' | 'unknown' = 'scaled') => ({
    from: 'a',
    to: 'b',
    kind,
    bps: pct == null ? null : pct * 10_000_000,
    speedBps: kind === 'unscaled' ? null : 1_000_000_000,
    pct,
    read: null,
    at: NOW,
    age: { state: 'live' as const, ms: 0 },
    cadenceS: 60,
  });

  it('never writes a percentage too small to write as a number', () => {
    // A campus at rest is mostly links carrying a few hundred kilobits on a
    // gigabit port. Seven of those wrote `<0.1%` over each other and over the
    // port names, and two more overlapped at the zoom where both directions
    // are written; the bar already says it, and the panel says it exactly.
    expect(worthLabelling(direction(0.04), 'busiest')).toBe(false);
    expect(worthLabelling(direction(0.04), 'both')).toBe(false);
    expect(worthLabelling(direction(0.4), 'busiest')).toBe(true);
    expect(worthLabelling(direction(42), 'busiest')).toBe(true);
  });

  it('writes a rate with no speed, which has no bar to speak for it', () => {
    expect(worthLabelling(direction(null, 'unscaled'), 'busiest')).toBe(true);
  });

  it('writes idle and no rate, which are the two a reader must not confuse', () => {
    expect(worthLabelling(direction(0), 'busiest')).toBe(true);
    expect(worthLabelling(direction(null, 'unknown'), 'busiest')).toBe(true);
  });

  it('writes nothing at all where there are only bars', () => {
    expect(worthLabelling(direction(42), 'bars')).toBe(false);
    expect(worthLabelling(direction(42), 'none')).toBe(false);
  });
});

describe('the layer as a whole', () => {
  const answer = (extra: Partial<Rates> = {}): Rates => ({
    at: NOW,
    monitoring: true,
    ports: [],
    ...extra,
  });

  it('says once that nothing is watching, rather than "no rate" on every link', () => {
    expect(layerState({ rates: undefined, pending: false, failed: true })).toBe('absent');
    expect(layerState({ rates: undefined, pending: true, failed: false })).toBe('loading');
    // A build without the monitor feature answers, and says so: that is not
    // the same as monitoring being there and having measured nothing.
    expect(layerState({ rates: answer({ monitoring: false }), pending: false, failed: false })).toBe(
      'absent',
    );
    expect(layerState({ rates: answer(), pending: false, failed: false })).toBe('nothing');
    expect(
      layerState({
        rates: answer({ ports: [row({ device_id: 'dev-a', name: 'GigabitEthernet0/1' })] }),
        pending: false,
        failed: false,
      }),
    ).toBe('ready');
  });

  it('joins only the nodes that are devices', () => {
    const index = deviceIndex([device('n:a', 'dev-a'), device('h:1')]);
    expect([...index]).toEqual([['n:a', 'dev-a']]);
  });
});

describe('whose clock a reading is aged against', () => {
  // A device polled every sixty seconds: a reading is current for 150 s.
  const limit = staleAfterMs(60);

  it('calls a five-second-old reading live however fast the browser’s clock runs', () => {
    // The laptop is four minutes ahead of the server. The answer said NOW and
    // arrived when the laptop's clock read four minutes later; the drawing is
    // reckoning its ages one tick after that.
    const skew = 240_000;
    const now = serverNow([{ at: NOW, receivedAt: NOW + skew }], NOW + skew + 15_000);
    expect(now).toBe(NOW + 15_000);
    expect(freshness(NOW - 5_000, limit, now ?? Number.NaN)).toEqual({ state: 'live', ms: 20_000 });
  });

  it('lets a reading whose polling stopped go stale however slow the browser’s clock runs', () => {
    // Four minutes behind, and the last poll was five minutes ago: against
    // the laptop's clock that reading was one minute old, and live.
    const skew = -240_000;
    const now = serverNow([{ at: NOW, receivedAt: NOW + skew }], NOW + skew);
    expect(freshness(NOW - 300_000, limit, now ?? Number.NaN)).toEqual({
      state: 'stale',
      ms: 300_000,
    });
  });

  it('goes by the answer that arrived last, whichever one it is', () => {
    const earlier = { at: NOW, receivedAt: 5_000 };
    const later = { at: NOW + 60_000, receivedAt: 65_500 };
    expect(serverNow([earlier, later], 70_500)).toBe(NOW + 65_000);
    expect(serverNow([later, undefined, earlier], 70_500)).toBe(NOW + 65_000);
  });

  it('does not run backwards when the browser’s clock is put back', () => {
    // Somebody corrects the laptop's clock while the page is open. Until the
    // next answer arrives the time stands still; it never goes into the past.
    expect(serverNow([{ at: NOW, receivedAt: 900_000 }], 100_000)).toBe(NOW);
  });

  it('has no time to give before the server has said one', () => {
    expect(serverNow([], NOW)).toBeNull();
    expect(serverNow([undefined], NOW)).toBeNull();
    expect(serverNow([{ at: Number.NaN, receivedAt: 0 }], NOW)).toBeNull();
  });

  it('gives a reading with no time no age, rather than the age of now', () => {
    const load = linkLoad(
      link(),
      context([{ device_id: 'dev-a', interface: 'GigabitEthernet0/1', name: 'GigabitEthernet0/1', out_bps: 5 }]),
    );
    expect(load.aToB.bps).toBe(5);
    expect(load.aToB.at).toBeNull();
    expect(load.aToB.age).toEqual({ state: 'unknown', ms: null });
  });
});
