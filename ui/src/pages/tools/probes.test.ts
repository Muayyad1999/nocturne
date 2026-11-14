import { describe, expect, it } from 'vitest';
import type { Job } from '@/api/types/core';
import type { Hop, PingEcho, PortResult, ProbeRun, SweptHost } from '@/api/types/toolbox';
import {
  echoWords,
  hold,
  MAX_HELD,
  take,
  failureWords,
  mergeHop,
  mergeHost,
  mergePort,
  nothingWasSent,
  outcomeOf,
  pingStats,
  rttChart,
} from './probes';

function job(over: Partial<Job> = {}): Job {
  return {
    id: 'j1',
    kind: 'probe.ping',
    title: 'Ping 10.0.0.1',
    state: 'running',
    origin: 'user',
    device_ids: [],
    done: 0,
    total: 5,
    message: 'running',
    started_at: 1_000,
    log: [],
    log_dropped: 0,
    ...over,
  };
}

function run(over: Partial<ProbeRun> = {}): ProbeRun {
  return {
    id: 'r1',
    kind: 'ping',
    title: 'Ping 10.0.0.1',
    target: '10.0.0.1',
    address: '10.0.0.1',
    device: null,
    from: 'this computer',
    method: 'icmp_datagram',
    job_id: 'j1',
    started_at: 1_000,
    finished_at: null,
    ok: null,
    error: null,
    error_code: null,
    summary: '',
    result: null,
    ...over,
  };
}

const echo = (seq: number, rtt: number | null, error: string | null = null): PingEcho => ({
  seq,
  rtt_ms: rtt,
  ttl: rtt === null ? null : 64,
  error,
});

describe('a probe that could not run is never a result', () => {
  it('reports the refusal with the code from the history record, not the job’s sentence alone', () => {
    const outcome = outcomeOf(
      job({ state: 'failed', error: 'raw sockets need CAP_NET_RAW' }),
      run({ ok: false, error: 'raw sockets need CAP_NET_RAW', error_code: 'forbidden' }),
      false,
    );
    expect(outcome).toEqual({
      state: 'failed',
      failure: { code: 'forbidden', message: 'raw sockets need CAP_NET_RAW' },
    });
  });

  it('carries no result on a failure, so there is nothing a chart could draw', () => {
    const outcome = outcomeOf(
      job({ state: 'failed', error: 'no answer', result: { loss_pct: 100 } }),
      run({ ok: false, error: 'no answer', error_code: 'timeout' }),
      false,
    );
    expect(outcome.state).toBe('failed');
    expect(outcome).not.toHaveProperty('result');
  });

  it('says so under a neutral headline while the record has not arrived yet', () => {
    const outcome = outcomeOf(job({ state: 'failed', error: 'it did not work' }), undefined, false);
    expect(outcome).toEqual({
      state: 'failed',
      failure: { code: 'failed', message: 'it did not work' },
    });
    expect(failureWords('failed').headline).toBe('The probe did not finish');
  });

  it('tells "this machine may not" apart from "nothing answered", which is the whole point', () => {
    expect(failureWords('forbidden')).toEqual({
      headline: 'This machine may not run that probe',
      aboutThisMachine: true,
    });
    expect(failureWords('timeout')).toEqual({
      headline: 'Nothing answered in time',
      aboutThisMachine: false,
    });
    expect(failureWords('forbidden').headline).not.toBe(failureWords('timeout').headline);
  });

  it('knows which failures mean no packet ever left the machine', () => {
    expect(nothingWasSent('forbidden')).toBe(true);
    expect(nothingWasSent('not_found')).toBe(true);
    expect(nothingWasSent('timeout')).toBe(false);
    expect(nothingWasSent('unavailable')).toBe(false);
  });
});

describe('a probe that ran', () => {
  it('is running while its job is', () => {
    expect(outcomeOf(job(), run(), false)).toEqual({
      state: 'running',
      jobId: 'j1',
      runId: 'r1',
    });
  });

  it('hands over the job’s own report when it succeeds', () => {
    const report = { sent: 3, received: 3 };
    expect(outcomeOf(job({ state: 'succeeded', result: report }), run({ ok: true }), false)).toEqual({
      state: 'answered',
      result: report,
      partial: false,
    });
  });

  it('marks a report partial when it was the user who stopped the run', () => {
    const outcome = outcomeOf(job({ state: 'succeeded', result: { sent: 2 } }), run({ ok: true }), true);
    expect(outcome).toMatchObject({ state: 'answered', partial: true });
  });

  it('is a refusal when it was stopped before anything came back at all', () => {
    expect(outcomeOf(job({ state: 'cancelled' }), undefined, true)).toEqual({
      state: 'failed',
      failure: { code: 'cancelled', message: 'Stopped before anything came back.' },
    });
  });

  it('is idle before anything has been started', () => {
    expect(outcomeOf(undefined, undefined, false)).toEqual({ state: 'idle' });
  });
});

describe('the numbers of a run in flight', () => {
  it('counts an echo with no round trip as lost, and says 100 % only when every one is', () => {
    expect(pingStats([echo(1, null), echo(2, null)])).toMatchObject({
      sent: 2,
      received: 0,
      lossPct: 100,
      min: null,
      avg: null,
      max: null,
    });
    expect(pingStats([echo(1, 1), echo(2, null)]).lossPct).toBe(50);
    expect(pingStats([echo(1, 1), echo(2, 3)]).lossPct).toBe(0);
  });

  it('reports no loss and no figures at all before the first echo', () => {
    expect(pingStats([])).toEqual({
      sent: 0,
      received: 0,
      lossPct: 0,
      min: null,
      avg: null,
      max: null,
      last: null,
    });
  });

  it('averages only the echoes that answered', () => {
    const stats = pingStats([echo(1, 1), echo(2, null), echo(3, 3)]);
    expect(stats).toMatchObject({ min: 1, max: 3, avg: 2, received: 2, sent: 3 });
  });

  it('keeps the last value even when the last echo was lost, as "now" is then nothing', () => {
    expect(pingStats([echo(1, 5), echo(2, null)]).last).toBeNull();
  });
});

describe('the round-trip chart', () => {
  it('gives a lost echo no height at all, so it cannot read as 0 ms', () => {
    const chart = rttChart([echo(1, 10), echo(2, null)]);
    expect(chart.bars).toEqual([
      { seq: 1, rtt: 10, height: 1 },
      { seq: 2, rtt: null, height: 0 },
    ]);
  });

  it('scales to the tallest bar it draws, so a loopback ping is a chart and not a flat line', () => {
    const chart = rttChart([echo(1, 0.08), echo(2, 0.12)]);
    expect(chart.scaleMs).toBe(0.12);
    expect(chart.bars.map((bar) => bar.height)).toEqual([0.08 / 0.12, 1]);
  });

  it('has a scale even when nothing answered, so there is something to label', () => {
    const chart = rttChart([echo(1, null)]);
    expect(chart.scaleMs).toBe(1);
    expect(chart.bars[0]?.height).toBe(0);
  });

  it('draws the newest echoes and says how many older ones are not drawn', () => {
    const many = Array.from({ length: 150 }, (_unused, index) => echo(index + 1, 1));
    const chart = rttChart(many, 120);
    expect(chart.older).toBe(30);
    expect(chart.bars).toHaveLength(120);
    expect(chart.bars[0]?.seq).toBe(31);
  });

  it('draws nothing when nothing has come back yet', () => {
    expect(rttChart([])).toEqual({ bars: [], scaleMs: 1, older: 0 });
  });
});

describe('what one echo says', () => {
  it('quotes the engine’s reason for a lost echo rather than inventing "timeout"', () => {
    expect(echoWords(echo(1, null, 'host unreachable'))).toBe('host unreachable');
    expect(echoWords(echo(1, null))).toBe('no reply');
  });

  it('keeps two decimals under 10 ms, where the difference is the interesting part', () => {
    expect(echoWords(echo(1, 0.083))).toBe('0.08 ms, TTL 64');
    expect(echoWords(echo(2, 42.6))).toBe('42.6 ms, TTL 64');
  });
});

describe('merging what arrives one event at a time', () => {
  const hop = (ttl: number, note: string): Hop => ({
    ttl,
    address: `10.0.0.${ttl}`,
    hostname: null,
    rtts_ms: [1],
    note,
  });

  it('replaces a hop with its newer self instead of listing it twice', () => {
    const merged = mergeHop([hop(1, 'first'), hop(2, 'second')], hop(1, 'again'));
    expect(merged.map((entry) => entry.note)).toEqual(['again', 'second']);
  });

  it('keeps hops in order however they arrive', () => {
    const merged = mergeHop([hop(3, 'three')], hop(1, 'one'));
    expect(merged.map((entry) => entry.ttl)).toEqual([1, 3]);
  });

  const port = (number: number): PortResult => ({
    port: number,
    state: 'open',
    rtt_ms: 1,
    service: '',
    banner: null,
  });

  it('lists an open port once, even if the event arrives twice', () => {
    const merged = mergePort([port(22)], port(22));
    expect(merged).toHaveLength(1);
    expect(mergePort([port(443)], port(22)).map((entry) => entry.port)).toEqual([22, 443]);
  });

  const host = (address: string): SweptHost => ({
    address,
    alive: true,
    rtt_ms: 1,
    via: 'icmp',
    hostname: null,
    mac: null,
    open_ports: [],
  });

  it('keeps swept addresses in the order they answered, once each', () => {
    const merged = mergeHost([host('10.0.0.2')], host('10.0.0.1'));
    expect(merged.map((entry) => entry.address)).toEqual(['10.0.0.2', '10.0.0.1']);
    expect(mergeHost(merged, host('10.0.0.2')).map((entry) => entry.address)).toEqual([
      '10.0.0.1',
      '10.0.0.2',
    ]);
  });
});

describe('events that arrive before the start call answers', () => {
  it('holds them and hands over the ones of that job, in order', () => {
    let held = hold<string>([], { jobId: 'j1', payload: 'first' });
    held = hold(held, { jobId: 'j2', payload: 'someone else’s' });
    held = hold(held, { jobId: 'j1', payload: 'second' });
    expect(take(held, 'j1')).toEqual(['first', 'second']);
    expect(take(held, 'j3')).toEqual([]);
  });

  it('keeps the newest, so a run nobody is watching cannot grow without bound', () => {
    let held: ReturnType<typeof hold<number>> = [];
    for (let index = 0; index < MAX_HELD + 10; index += 1) {
      held = hold(held, { jobId: 'j1', payload: index });
    }
    expect(held).toHaveLength(MAX_HELD);
    expect(held[0]?.payload).toBe(10);
  });
});
