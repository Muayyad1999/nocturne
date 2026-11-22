/**
 * How loaded each link is — the one thing The Dude puts on a line that this
 * drawing did not have.
 *
 * The numbers are monitoring's own: per port, the rate in and the rate out as
 * the poller last worked them out from the device's counters over the
 * *device's* uptime (`nocturne-monitor::rates`), so a reboot is a gap and
 * never a spike. They arrive through `topology.rates`, which is asked about
 * exactly the ports this drawing's links end on and answers under the
 * spelling it was asked with — which two spellings of an interface are one
 * port is decided there, by the function the whole backend uses for it, and
 * never here. Nothing here measures anything; it puts those answers on the
 * ends of a link and decides what may honestly be drawn. Four different
 * facts, four different answers:
 *
 * * **no rate** — monitoring has no measurement for this port (never polled,
 *   the poll failed, or the interval could not be measured). Nothing is drawn
 *   for that direction and the word is "no rate".
 * * **idle** — a rate was measured and it is zero. A channel is drawn, empty.
 *   Zero and unknown are different values everywhere in this codebase, and a
 *   map that paints "no data" as 0 % is the easiest possible way to make an
 *   operator trust a lie.
 * * **no speed** — there is a rate but the port never said how fast it is, so
 *   there are bits per second and no percentage. The rate is shown; the
 *   percentage is not invented.
 * * **stale** — the measurement is older than this device's own measurement
 *   cadence allows, so it is drawn grey and labelled, never as if it were now.
 *   "Older" is reckoned on the server's clock, the one the measurement was
 *   stamped with (`serverNow`): a laptop four minutes fast would otherwise
 *   grey the whole map, and one four minutes slow keep a dead poll alive.
 *
 * Direction matters because a link is rarely symmetrical: each direction is
 * read from the *sending* port's `out_bps` where that port is monitored, and
 * falls back to the receiving port's `in_bps`. The two are different
 * measurements (a frame dropped on the wire is sent and not received), so
 * which one a number came from is carried along and shown in the panel.
 */

import type { MonitorSettings } from '@/api/types/monitor';
import type { Graph, Link, Node, PortRate, RatePort, Rates } from '@/api/types/topology';
import { EMPTY, formatPercent, formatRate } from '@/lib/format';

// ------------------------------------------------------------------ the index

/** One port's last measurement, as monitoring recorded it. */
export interface RateSample {
  deviceId: string;
  /** The name monitoring used, for saying where a number came from. */
  name: string;
  inBps: number | null;
  outBps: number | null;
  speedBps: number | null;
  /** When the poll that produced it ran. */
  at: number | null;
}

export type RateIndex = ReadonlyMap<string, RateSample>;

/**
 * A port, as one string. The interface is taken letter for letter: the
 * backend answers under the spelling it was asked with, so the spelling a
 * link end has is the only one this screen ever needs to know.
 */
function keyOf(deviceId: string, name: string): string {
  return `${deviceId}\u0000${name}`;
}

/**
 * The ports to ask `topology.rates` about: every link end that is an
 * interface of a device, once. In a fixed order, because the list is the
 * question's identity — the same drawing must ask the same question however
 * its links happened to be ordered.
 */
export function ratePorts(graph: Pick<Graph, 'nodes' | 'links'> | undefined): RatePort[] {
  if (!graph) return [];
  const deviceOf = deviceIndex(graph.nodes);
  const ports = new Map<string, RatePort>();
  for (const link of graph.links) {
    for (const end of [link.a, link.b]) {
      const deviceId = deviceOf.get(end.node);
      if (!deviceId || !end.interface) continue;
      ports.set(keyOf(deviceId, end.interface), { device_id: deviceId, interface: end.interface });
    }
  }
  return [...ports.entries()]
    .sort(([one], [other]) => (one < other ? -1 : one > other ? 1 : 0))
    .map(([, port]) => port);
}

/** Every port that was answered for, by device and by the name it was asked with. */
export function rateIndex(ports: readonly PortRate[] | undefined): RateIndex {
  const index = new Map<string, RateSample>();
  for (const port of ports ?? []) {
    index.set(keyOf(port.device_id, port.interface), {
      deviceId: port.device_id,
      name: port.name,
      inBps: port.in_bps ?? null,
      outBps: port.out_bps ?? null,
      speedBps: port.speed_bps ?? null,
      at: port.at ?? null,
    });
  }
  return index;
}

// -------------------------------------------------------------- how fresh it is

/** The cadence to assume when nothing says how often a device is measured. */
export const UNKNOWN_CADENCE_S = 300;
/**
 * A measurement survives one missed poll, not two. Below the floor the
 * cadence is so short that a label would flicker between live and stale
 * faster than anybody can read it.
 */
const STALE_FACTOR = 2.5;
const STALE_FLOOR_MS = 90_000;

/** What the screen knows about how a device is monitored. */
export interface MonitoredDevice {
  monitor: { enabled: boolean; snmp: boolean; interval_s: number };
}

/**
 * How often this device's interface counters are measured, in seconds.
 *
 * With SNMP that is the device's own poll interval. Without it, the counters
 * come from a command-line read, which monitoring does no more often than
 * `cli_interval_s` however often the device itself is polled — so the slower
 * of the two is the truth. `null` when the device is not in the inventory
 * (an unmanaged neighbour has no cadence, and no rates either).
 */
export function cadenceS(
  device: MonitoredDevice | undefined,
  monitoring: Pick<MonitorSettings, 'cli_interval_s'> | undefined,
): number | null {
  if (!device) return null;
  const own = Math.max(1, device.monitor.interval_s);
  if (device.monitor.snmp) return own;
  return Math.max(own, monitoring?.cli_interval_s ?? UNKNOWN_CADENCE_S);
}

/** How old a measurement may be before it stops being current. */
export function staleAfterMs(cadence: number | null): number {
  const seconds = cadence ?? UNKNOWN_CADENCE_S;
  return Math.max(STALE_FLOOR_MS, seconds * 1000 * STALE_FACTOR);
}

export type Freshness = 'live' | 'stale' | 'unknown';

export interface Age {
  state: Freshness;
  /** Milliseconds since the measurement, or `null` when there is none. */
  ms: number | null;
}

/**
 * How old a reading is. `at` and `now` have to be on the same clock, and `at`
 * is the server's: pass `serverNow`, not the browser's `Date.now()`.
 */
export function freshness(at: number | null, limitMs: number, now: number): Age {
  if (at == null || !Number.isFinite(at) || !Number.isFinite(now)) {
    return { state: 'unknown', ms: null };
  }
  const ms = Math.max(0, now - at);
  return { state: ms > limitMs ? 'stale' : 'live', ms };
}

/**
 * A moment the server named — the `at` of an answer — and what the browser's
 * own clock read when that answer arrived.
 */
export interface ServerMoment {
  at: number;
  receivedAt: number;
}

/**
 * What time it is on the server, or `null` before it has said.
 *
 * Every timestamp on this screen was stamped by the server, and the browser's
 * clock is somebody's laptop: it can be minutes out either way. So the two
 * are never compared. The server's last word on the time is carried forward
 * by how long ago it arrived, and the browser's clock is only ever asked how
 * long that was — a difference between two of its own readings, in which
 * being wrong by four minutes cancels out.
 *
 * `moments` are the answers that carry a time; the one that arrived last
 * wins. A clock put back while the page is open would make that difference
 * negative: the time then stands still until the next answer rather than
 * going into the past.
 */
export function serverNow(
  moments: readonly (ServerMoment | undefined)[],
  browserNow: number,
): number | null {
  let latest: ServerMoment | undefined;
  for (const moment of moments) {
    if (!moment || !Number.isFinite(moment.at) || !Number.isFinite(moment.receivedAt)) continue;
    if (!latest || moment.receivedAt > latest.receivedAt) latest = moment;
  }
  if (!latest) return null;
  return latest.at + Math.max(0, browserNow - latest.receivedAt);
}

// ----------------------------------------------------------------- one direction

/** What kind of answer there is for one direction of one link. */
export type LoadKind =
  /** A rate and a speed: a percentage may be drawn. */
  | 'scaled'
  /** A rate, but the port never said how fast it is: bits per second only. */
  | 'unscaled'
  /** Nothing was measured. Not zero. */
  | 'unknown';

/** Which counter a number came from: the port that sent, or the one that received. */
export type Sense = 'sent' | 'received';

export interface DirectionLoad {
  /** The node this direction leaves, and the one it arrives at. */
  from: string;
  to: string;
  kind: LoadKind;
  bps: number | null;
  speedBps: number | null;
  /** Percentage of the speed, or `null` when one of the two is missing. */
  pct: number | null;
  /** Where the number was read: whose port, called what, sent or received. */
  read: { deviceId: string; interface: string; sense: Sense } | null;
  at: number | null;
  age: Age;
  /**
   * How often the port this number came from is measured, in seconds. It is
   * that end's cadence and not the link's: a link between a device polled
   * every thirty minutes and one polled every five seconds has two different
   * answers, and the one that matters is the one behind *this* number.
   */
  cadenceS: number | null;
}

export interface LinkLoad {
  aToB: DirectionLoad;
  bToA: DirectionLoad;
  /** Whether either direction has a measurement at all. */
  measured: boolean;
  /** Whether every measurement there is, is older than its device allows. */
  stale: boolean;
  /** The newest measurement behind this link, for "as of" lines. */
  at: number | null;
  /** The direction to label when there is only room for one. */
  busiest: DirectionLoad;
}

/** What `linkLoad` needs to know besides the link itself. */
export interface LoadContext {
  /** Node id → device id, for the nodes that are devices. */
  deviceOf: ReadonlyMap<string, string>;
  rates: RateIndex;
  /** Device id → how it is monitored, from `devices.list`. */
  devices: ReadonlyMap<string, MonitoredDevice>;
  monitoring: Pick<MonitorSettings, 'cli_interval_s'> | undefined;
  /** The time on the server's clock (`serverNow`), which the readings were stamped with. */
  now: number;
}

interface EndRef {
  node: string;
  deviceId: string | undefined;
  sample: RateSample | undefined;
  cadence: number | null;
}

function endRef(end: { node: string; interface?: string }, context: LoadContext): EndRef {
  const deviceId = context.deviceOf.get(end.node);
  const sample =
    deviceId && end.interface ? context.rates.get(keyOf(deviceId, end.interface)) : undefined;
  return {
    node: end.node,
    deviceId,
    sample,
    cadence: deviceId ? cadenceS(context.devices.get(deviceId), context.monitoring) : null,
  };
}

/**
 * One direction, from `from` to `to`.
 *
 * The sending port's `out_bps` is preferred, because it is the only one of
 * the two that cannot have been thinned by the wire in between. The
 * exception is age: a current reading of what arrived beats a stale reading
 * of what was sent, because the question on a map is what is happening now.
 */
function direction(from: EndRef, to: EndRef, link: Link, context: LoadContext): DirectionLoad {
  const candidate = (end: EndRef, bps: number | null | undefined) =>
    bps == null || end.sample === undefined
      ? null
      : {
          end,
          bps,
          sample: end.sample,
          age: freshness(end.sample.at, staleAfterMs(end.cadence), context.now),
        };
  const sent = candidate(from, from.sample?.outBps);
  const received = candidate(to, to.sample?.inBps);
  const chosen =
    sent && received && sent.age.state === 'stale' && received.age.state === 'live'
      ? received
      : (sent ?? received);
  const sense: Sense = chosen === sent ? 'sent' : 'received';

  const base = { from: from.node, to: to.node };
  if (!chosen) {
    // Nothing measured this direction. The age of a non-existent measurement
    // is not "now" and not "long ago": it is unknown.
    return {
      ...base,
      kind: 'unknown',
      bps: null,
      speedBps: speedFor(link, from, to),
      pct: null,
      read: null,
      at: null,
      age: { state: 'unknown', ms: null },
      cadenceS: null,
    };
  }

  const speedBps = speedFor(link, from, to);
  const pct = speedBps != null && speedBps > 0 ? (chosen.bps / speedBps) * 100 : null;
  return {
    ...base,
    kind: pct == null ? 'unscaled' : 'scaled',
    bps: chosen.bps,
    speedBps,
    pct,
    read: {
      deviceId: chosen.end.deviceId ?? '',
      interface: chosen.sample.name,
      sense,
    },
    at: chosen.sample.at,
    age: chosen.age,
    cadenceS: chosen.end.cadence,
  };
}

/**
 * What a rate is a percentage *of*. The link's own speed first — the backend
 * worked that out from both ends, and for a port-channel from its members
 * added together — then the sending port's own, then the receiving port's.
 * `null` means the question has no answer, not that it is slow.
 */
function speedFor(link: Link, from: EndRef, to: EndRef): number | null {
  const candidates = [link.speed_bps, from.sample?.speedBps, to.sample?.speedBps];
  for (const speed of candidates) {
    if (speed != null && Number.isFinite(speed) && speed > 0) return speed;
  }
  return null;
}

/** Both directions of one link, from the rates monitoring last recorded. */
export function linkLoad(link: Link, context: LoadContext): LinkLoad {
  const a = endRef(link.a, context);
  const b = endRef(link.b, context);
  const aToB = direction(a, b, link, context);
  const bToA = direction(b, a, link, context);

  const measured = aToB.kind !== 'unknown' || bToA.kind !== 'unknown';
  const times = [aToB.at, bToA.at].filter((at): at is number => at != null);
  const states = [aToB.age.state, bToA.age.state].filter((state) => state !== 'unknown');

  return {
    aToB,
    bToA,
    measured,
    stale: states.length > 0 && states.every((state) => state === 'stale'),
    at: times.length > 0 ? Math.max(...times) : null,
    busiest: busier(aToB, bToA),
  };
}

/**
 * The direction worth labelling when there is only room for one. A measured
 * direction always beats an unmeasured one, and a percentage beats a bare
 * rate: they are not comparable, and the percentage is the one that says
 * whether the link is in trouble.
 */
export function busier(one: DirectionLoad, other: DirectionLoad): DirectionLoad {
  const rank = (load: DirectionLoad) => (load.kind === 'unknown' ? 0 : load.kind === 'unscaled' ? 1 : 2);
  if (rank(one) !== rank(other)) return rank(one) > rank(other) ? one : other;
  if (one.pct != null && other.pct != null) return one.pct >= other.pct ? one : other;
  if (one.bps != null && other.bps != null) return one.bps >= other.bps ? one : other;
  return one;
}

// ------------------------------------------------------------------ the words

/**
 * How busy a direction is, in bands. The 90 % line is monitoring's own (its
 * interface table calls a port at or above it one that needs a look), so the
 * two screens agree about what "hot" means.
 */
export type LoadBand = 'idle' | 'light' | 'busy' | 'hot' | 'unknown';

export function loadBand(load: DirectionLoad): LoadBand {
  if (load.kind === 'unknown') return 'unknown';
  if (load.bps === 0) return 'idle';
  if (load.pct == null) return 'light';
  if (load.pct >= 90) return 'hot';
  if (load.pct >= 50) return 'busy';
  return 'light';
}

/**
 * The short label drawn on the line. Each of the four states gets its own
 * word, because they are four different facts:
 *
 * * `42%`      — measured, against a known speed
 * * `<0.1%`    — measured and not zero, but too small to round to anything
 * * `idle`     — measured, and zero
 * * `24 Mb/s`  — measured, with no speed to compare it to
 * * `no rate`  — not measured
 */
export function loadLabel(load: DirectionLoad): string {
  if (load.kind === 'unknown') return 'no rate';
  if (load.bps === 0) return 'idle';
  if (load.pct == null) return formatRate(load.bps);
  if (load.pct > 0 && load.pct < 0.1) return '<0.1%';
  return formatPercent(load.pct);
}

/** The rate on its own, for panels: `24 Mb/s`, or a dash when there is none. */
export function rateText(load: DirectionLoad): string {
  return load.kind === 'unknown' ? EMPTY : formatRate(load.bps);
}

/** One sentence about one direction, for a tooltip and for assistive technology. */
export function loadSentence(
  load: DirectionLoad,
  names: { from: string; to: string },
): string {
  const way = `${names.from} → ${names.to}`;
  if (load.kind === 'unknown') {
    return `${way}: no rate has been measured.`;
  }
  const scale =
    load.pct == null
      ? `${formatRate(load.bps)} (the port did not say how fast it is, so there is no percentage)`
      : `${formatRate(load.bps)} of ${formatRate(load.speedBps)}, ${loadLabel(load)}`;
  const sense = load.read?.sense === 'sent' ? 'sent by' : 'received by';
  const where = load.read ? `, ${sense} ${load.read.interface}` : '';
  const when = load.age.state === 'stale' ? ' — stale, not a current reading' : '';
  return `${way}: ${scale}${where}${when}.`;
}

// ------------------------------------------------------- what shows at what zoom

/**
 * How much of a link's load to draw. A number on every line at every zoom is
 * a drawing of overlapping text — the port labels learnt that already — so
 * the bars, which stay readable when they are three pixels long, show first
 * and the writing comes in as there is room for it.
 */
export type LoadDetail =
  /** Nothing: at this zoom a card is a smudge and a bar is a pixel. */
  | 'none'
  /** The bars only. */
  | 'bars'
  /** The bars and one number: the direction that matters. */
  | 'busiest'
  /** The bars and both numbers. */
  | 'both';

/** Below this zoom the whole load layer is left off. */
const BARS_FROM = 0.2;
/**
 * A bar needs a line to live on. Between two cards that are all but touching
 * there are fifty pixels of wire, and three lines in that gap — the wire and
 * a channel either side — is a smudge with the words that say how the link is
 * known written over it. Zooming in gives the same link more room and its
 * bars come back.
 */
const BARS_LENGTH_PX = 52;
/** A number needs this much line on screen, in pixels, to sit beside. */
const LABEL_LENGTH_PX = 90;
const BOTH_LENGTH_PX = 150;

export function loadDetail(zoom: number, screenLengthPx: number): LoadDetail {
  if (!Number.isFinite(zoom) || zoom < BARS_FROM || screenLengthPx < BARS_LENGTH_PX) return 'none';
  if (zoom < 0.6 || screenLengthPx < LABEL_LENGTH_PX) return 'bars';
  if (zoom < 1 || screenLengthPx < BOTH_LENGTH_PX) return 'busiest';
  return 'both';
}

/**
 * Whether a direction's number earns the ink it costs.
 *
 * Looking at the drawing decided this one. A campus at rest is mostly links
 * carrying a few hundred kilobits on a gigabit port, and every one of them
 * wrote `<0.1%`: seven of them around one pair of distribution switches, over
 * each other and over the port names, and two more overlapping at the zoom
 * where both directions are written. A percentage too small to write as a
 * number says nothing a bar one pixel long has not already said — so it is
 * not written at any zoom, and the exact figure stays one click away in the
 * panel. Everything else is: a real percentage, `idle`, a bare rate, and
 * `no rate`, which are the four a reader must be able to tell apart.
 */
export function worthLabelling(load: DirectionLoad, detail: LoadDetail): boolean {
  if (detail === 'none' || detail === 'bars') return false;
  return !(load.kind === 'scaled' && load.pct != null && load.pct > 0 && load.pct < 0.1);
}

// --------------------------------------------------------------- the whole layer

/**
 * Whether there are rates to draw at all, which is not the same question as
 * whether a particular link has one. A build without the monitor feature
 * says so (`monitoring: false`); drawing every link as "no rate" in that case
 * would be true but useless, so the layer says once that nothing is watching
 * and draws nothing.
 */
export type LayerState =
  /** Monitoring is not in this build, or refused. */
  | 'absent'
  /** The first answer has not arrived yet. */
  | 'loading'
  /** Monitoring is there and has measured none of this drawing's ports yet. */
  | 'nothing'
  | 'ready';

export function layerState(query: {
  rates: Rates | undefined;
  pending: boolean;
  failed: boolean;
}): LayerState {
  if (query.failed) return 'absent';
  if (query.rates === undefined) return query.pending ? 'loading' : 'absent';
  if (!query.rates.monitoring) return 'absent';
  return query.rates.ports.length === 0 ? 'nothing' : 'ready';
}

/** What the layer says about itself, under the drawing. */
export function layerNote(state: LayerState, newest: number | null): string {
  switch (state) {
    case 'absent':
      return 'Link load needs monitoring, which is not answering in this build.';
    case 'loading':
      return 'Reading the interface rates…';
    case 'nothing':
      return 'No interface rates have been measured yet, so no link shows a load.';
    case 'ready':
      return newest == null ? 'No interface rates have been measured yet.' : 'load';
  }
}

/** The node ids that are devices, for the join. */
export function deviceIndex(nodes: readonly Node[]): Map<string, string> {
  const index = new Map<string, string>();
  for (const node of nodes) {
    if (node.device_id) index.set(node.id, node.device_id);
  }
  return index;
}
