/**
 * What a probe did, worked out with no React in it.
 *
 * This is where the screen keeps the promise the toolbox crate was written
 * around: **a probe that could not run is not a quiet result.** The engine is
 * careful about it (`crates/nocturne-toolbox/src/probe/ping.rs`: "a report of
 * total loss means the network was silent; it never means the probe did not
 * happen"), and the only way for a screen to throw that away is to render a
 * failure as an empty chart with 0 ms in it. So:
 *
 * * {@link outcomeOf} turns a job and its history record into one of five
 *   states, and `failed` carries no result at all — there is nothing to draw,
 *   and {@link failureWords} says what happened instead;
 * * `forbidden` ("this machine may not send that") and `timeout` ("nothing
 *   answered") get different headlines, because they are different facts about
 *   the world and the first one is actionable;
 * * 100 % loss is only ever produced from a report that exists, by
 *   {@link pingStats}, which counts echoes the engine really sent.
 *
 * The second thing here is the arithmetic of a live run. The streamed events
 * (`probe.ping.echo`, `probe.trace.hop`, `probe.port`, `probe.sweep.host`)
 * arrive one at a time and must add up to the same picture the job's own
 * result carries when it ends, so the merging rules — last echo wins, a hop is
 * replaced by its newer self, a port is never duplicated — live in tested
 * functions rather than in a component's state updater.
 */

import type { Job } from '@/api/types/core';
import type { Hop, PingEcho, PortResult, ProbeRun, SweptHost } from '@/api/types/toolbox';

// ----------------------------------------------------------- what happened

export interface Failure {
  /** The machine-stable code: `forbidden`, `timeout`, `not_found`, … */
  code: string;
  /** The engine's own sentence, shown as it was written. */
  message: string;
}

export type Outcome =
  /** Nothing has been asked for yet. */
  | { state: 'idle' }
  | { state: 'running'; jobId: string; runId: string | null }
  /** It ran and there is a report. */
  | { state: 'answered'; result: unknown; partial: boolean }
  /** It could not run, or could not finish. There is no report. */
  | { state: 'failed'; failure: Failure };

/**
 * The state of a job-shaped probe.
 *
 * `run` is the probe's own history record, which is the only place the failure
 * *code* survives: a job carries its error as a sentence (`Job.error`) and
 * drops the code, while `probe.history` keeps `error_code` precisely so that
 * "may not" and "no answer" stay apart. When the record has not arrived yet
 * the sentence is still shown, under a neutral headline, rather than being
 * guessed at.
 */
export function outcomeOf(
  job: Job | undefined,
  run: ProbeRun | undefined,
  stopped: boolean,
): Outcome {
  if (!job) return { state: 'idle' };
  if (job.state === 'running') {
    return { state: 'running', jobId: job.id, runId: run?.id ?? null };
  }
  if (run?.ok === false || job.state === 'failed') {
    return {
      state: 'failed',
      failure: {
        code: run?.error_code ?? 'failed',
        message: run?.error ?? job.error ?? 'The probe did not finish, and said nothing about why.',
      },
    };
  }
  if (job.state === 'cancelled') {
    // The job was stopped before it produced anything at all.
    return {
      state: 'failed',
      failure: { code: 'cancelled', message: job.error ?? 'Stopped before anything came back.' },
    };
  }
  return { state: 'answered', result: job.result, partial: stopped };
}

export interface FailureWords {
  /** The headline: what kind of thing went wrong. */
  headline: string;
  /** Whether this is about the machine the probe runs from, not the target. */
  aboutThisMachine: boolean;
}

/**
 * The headline for a failure code.
 *
 * Deliberately not a single "the probe failed": the whole point of the
 * `error_code` the backend records is that these are different events, and a
 * person reading the screen has to be able to tell at a glance whether to fix
 * their own machine, the name they typed, or nothing at all.
 */
export function failureWords(code: string): FailureWords {
  switch (code) {
    case 'forbidden':
      return { headline: 'This machine may not run that probe', aboutThisMachine: true };
    case 'unsupported':
      return { headline: 'This machine cannot run that probe', aboutThisMachine: true };
    case 'timeout':
      return { headline: 'Nothing answered in time', aboutThisMachine: false };
    case 'not_found':
      return { headline: 'That name has no address', aboutThisMachine: false };
    case 'unavailable':
      return { headline: 'The target could not be reached', aboutThisMachine: false };
    case 'invalid':
      return { headline: 'That request will not do', aboutThisMachine: false };
    case 'conflict':
      return { headline: 'Something is already running', aboutThisMachine: true };
    case 'cancelled':
      return { headline: 'Stopped', aboutThisMachine: true };
    default:
      return { headline: 'The probe did not finish', aboutThisMachine: false };
  }
}

/**
 * Whether a failure means no packet ever left the machine.
 *
 * Used for one sentence on screen, because "nothing was sent" and "nothing
 * came back" look the same on an empty chart and are not the same thing.
 */
export function nothingWasSent(code: string): boolean {
  return code === 'forbidden' || code === 'unsupported' || code === 'not_found' || code === 'invalid';
}

// ------------------------------------------------------------------ a ping

export interface PingStats {
  sent: number;
  received: number;
  lossPct: number;
  min: number | null;
  avg: number | null;
  max: number | null;
  last: number | null;
}

/**
 * The numbers of the echoes that have arrived so far.
 *
 * The same arithmetic the engine does for a finished run, so that the live
 * figures and the report agree; loss is counted from the echoes themselves
 * (an echo with no round-trip time is a lost one), never assumed.
 */
export function pingStats(echoes: readonly PingEcho[]): PingStats {
  const rtts = echoes
    .map((echo) => echo.rtt_ms)
    .filter((rtt): rtt is number => rtt !== null && Number.isFinite(rtt));
  const sent = echoes.length;
  const received = rtts.length;
  const round = (value: number) => Math.round(value * 1000) / 1000;
  const lastEcho = echoes.at(-1);
  return {
    sent,
    received,
    lossPct: sent === 0 ? 0 : round(((sent - received) * 100) / sent),
    min: rtts.length > 0 ? round(Math.min(...rtts)) : null,
    max: rtts.length > 0 ? round(Math.max(...rtts)) : null,
    avg: rtts.length > 0 ? round(rtts.reduce((total, rtt) => total + rtt, 0) / rtts.length) : null,
    last: lastEcho?.rtt_ms ?? null,
  };
}

export interface RttBar {
  seq: number;
  /** `null` is a lost echo: the bar is drawn as a loss, not as a zero. */
  rtt: number | null;
  /** Height as a fraction of the chart, 0…1. A lost echo has no height. */
  height: number;
}

export interface RttChart {
  bars: RttBar[];
  /** The value the tallest bar stands for, in milliseconds. */
  scaleMs: number;
  /** Echoes not drawn because the run is longer than the chart. */
  older: number;
}

/**
 * The last `limit` echoes as bars, with the scale they are drawn against.
 *
 * A continuous ping can run for an hour; drawing three thousand bars two
 * pixels wide says less than drawing the last hundred and saying how many are
 * not shown.
 *
 * The scale is the tallest round trip among the bars drawn, and the chart
 * says what that is underneath. A fixed scale was tried and thrown away: on a
 * loopback ping of 0.08 ms against a 1 ms floor every bar is a sliver one
 * pixel high, which reads as "nothing came back" — the one thing the chart
 * must never say by accident.
 */
export function rttChart(echoes: readonly PingEcho[], limit = 120): RttChart {
  const older = Math.max(0, echoes.length - limit);
  const shown = echoes.slice(older);
  const highest = shown.reduce((high, echo) => Math.max(high, echo.rtt_ms ?? 0), 0);
  // Only when nothing answered at all is there no scale to take from the data.
  const scaleMs = highest > 0 ? highest : 1;
  return {
    older,
    scaleMs,
    bars: shown.map((echo) => ({
      seq: echo.seq,
      rtt: echo.rtt_ms,
      height: echo.rtt_ms === null ? 0 : Math.min(1, echo.rtt_ms / scaleMs),
    })),
  };
}

/** One echo, in the words the live list uses. */
export function echoWords(echo: PingEcho): string {
  if (echo.rtt_ms === null) return echo.error ?? 'no reply';
  const ttl = echo.ttl === null ? '' : `, TTL ${echo.ttl}`;
  return `${echo.rtt_ms.toFixed(echo.rtt_ms < 10 ? 2 : 1)} ms${ttl}`;
}

// ------------------------------------------------------------ merging rules

/**
 * A hop that has just been found, in a list that is filling up.
 *
 * Hops arrive in order but a later event for the same `ttl` carries more
 * (another probe answered), so the newer one replaces the older rather than
 * being appended beside it.
 */
export function mergeHop(hops: readonly Hop[], hop: Hop): Hop[] {
  const next = hops.filter((existing) => existing.ttl !== hop.ttl);
  next.push(hop);
  return next.sort((a, b) => a.ttl - b.ttl);
}

/** An open port, once. The event stream can repeat one after a reconnect. */
export function mergePort(ports: readonly PortResult[], port: PortResult): PortResult[] {
  const next = ports.filter((existing) => existing.port !== port.port);
  next.push(port);
  return next.sort((a, b) => a.port - b.port);
}

/** An address that answered, once, in the order the sweep found them. */
export function mergeHost(hosts: readonly SweptHost[], host: SweptHost): SweptHost[] {
  const next = hosts.filter((existing) => existing.address !== host.address);
  next.push(host);
  return next;
}

// --------------------------------------------------- events before the id

/**
 * An event that arrived before the call that started the run came back.
 *
 * `probe.ping` answers with `{job_id}` *after* the job has begun, so the first
 * echo can reach the screen before it knows which job it belongs to. Dropping
 * it would make the live count disagree with the report by one — the same
 * problem the Terminal screen solves by holding output until the attach
 * answers, and solved the same way here: hold, then take what matches.
 */
export interface Held<T> {
  jobId: string;
  payload: T;
}

/** How many events are held while a start is in flight. */
export const MAX_HELD = 200;

/** Adds one, keeping the newest {@link MAX_HELD}. */
export function hold<T>(held: readonly Held<T>[], entry: Held<T>): Held<T>[] {
  const next = [...held, entry];
  return next.length > MAX_HELD ? next.slice(next.length - MAX_HELD) : next;
}

/** The held events of one job, in the order they arrived. */
export function take<T>(held: readonly Held<T>[], jobId: string): T[] {
  return held.filter((entry) => entry.jobId === jobId).map((entry) => entry.payload);
}
