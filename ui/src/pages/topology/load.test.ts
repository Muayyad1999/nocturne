import { describe, expect, it } from 'vitest';
import type { IfStatus } from '@/api/types/monitor';
import type { Link } from '@/api/types/topology';
import {
  busier,
  cadenceS,
  deviceIndex,
  freshness,
  interfaceKey,
  layerState,
  linkLoad,
  loadBand,
  loadDetail,
  loadLabel,
  loadSentence,
  rateIndex,
  worthLabelling,
  staleAfterMs,
  UNKNOWN_CADENCE_S,
  type LoadContext,
  type MonitoredDevice,
} from './load';

const NOW = 1_700_000_000_000;

function row(extra: Partial<IfStatus> & Pick<IfStatus, 'device_id' | 'name'>): IfStatus {
  return {
    device: extra.device_id,
    description: '',
    admin_up: true,
    oper_up: true,
    speed_bps: 1_000_000_000,
    in_bps: null,
    out_bps: null,
    util_pct: null,
    in_err_ps: null,
    out_err_ps: null,
    in_disc_ps: null,
    out_disc_ps: null,
    uplink: true,
    lab: false,
    at: NOW - 10_000,
    ...extra,
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

function context(rows: IfStatus[], extra: Partial<LoadContext> = {}): LoadContext {
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

describe('joining a link to the ports that were measured', () => {
  it('expands a short interface name so a rate is not lost on the way', () => {
    expect(interfaceKey('Gi0/1')).toBe('gigabitethernet0/1');
    expect(interfaceKey('GigabitEthernet0/1')).toBe('gigabitethernet0/1');
    expect(interfaceKey('Te1/1/1')).toBe('tengigabitethernet1/1/1');
    expect(interfaceKey('Po1')).toBe('port-channel1');
    expect(interfaceKey('Vlan10')).toBe('vlan10');
  });

  it('leaves a name that merely starts like a short form alone', () => {
    // `get0` is not `GigabitEthernet0`, and a wrong expansion would join the
    // rate of one port to a different one.
    expect(interfaceKey('get0')).toBe('get0');
    expect(interfaceKey('sfp-sfpplus1')).toBe('sfp-sfpplus1');
  });

  it('finds a port whose name the two features spell differently', () => {
    const load = linkLoad(link(), context([row({ device_id: 'dev-a', name: 'Gi0/1', out_bps: 1_000_000 })]));
    expect(load.aToB.bps).toBe(1_000_000);
    expect(load.aToB.read?.interface).toBe('Gi0/1');
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
  it('says once that nothing is watching, rather than "no rate" on every link', () => {
    expect(layerState({ rows: undefined, pending: false, failed: true })).toBe('absent');
    expect(layerState({ rows: undefined, pending: true, failed: false })).toBe('loading');
    expect(layerState({ rows: [], pending: false, failed: false })).toBe('nothing');
    expect(
      layerState({ rows: [row({ device_id: 'dev-a', name: 'Gi0/1' })], pending: false, failed: false }),
    ).toBe('ready');
  });

  it('joins only the nodes that are devices', () => {
    const index = deviceIndex([
      { id: 'n:a', kind: 'switch', name: 'A', device_id: 'dev-a', lab: false, status: 'up', unmanaged: false, x: 0, y: 0, pinned: false },
      { id: 'h:1', kind: 'host', name: 'laptop', lab: false, status: 'unknown', unmanaged: true, x: 0, y: 0, pinned: false },
    ]);
    expect([...index]).toEqual([['n:a', 'dev-a']]);
  });
});
