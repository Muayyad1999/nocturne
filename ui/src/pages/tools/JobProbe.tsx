/**
 * The four probes that are jobs: ping, traceroute, port check, address sweep.
 *
 * These are the only tools on this screen that live longer than a call, so
 * they are the only ones with a state machine, and it is deliberately the same
 * one the Terminal screen uses for its sessions: **the run belongs to the
 * backend, not to this component.** Starting one returns `{job_id, run_id}`;
 * everything after that is read from the engine —
 *
 * * progress and the final result from `jobs.get`, kept current by
 *   `jobs.changed`;
 * * what it is finding, live, from the feature's own topics
 *   (`probe.ping.echo`, `probe.trace.hop`, `probe.port`, `probe.sweep.host`),
 *   merged by the rules in `probes.ts`;
 * * whether it could run at all from its probe history record, which is the
 *   only place the failure *code* survives.
 *
 * So leaving the screen does not stop a ping, and coming back finds it: on
 * mount the panel looks for a run of its own kind that is still going
 * (`ok: null` in `probe.history`) and attaches to that rather than starting a
 * second one. The echoes that arrived while the screen was elsewhere were not
 * kept by anyone — the panel says so instead of drawing a gap, and the job's
 * own report has them all when it ends.
 *
 * What this component will not do is draw a result for a run that failed. A
 * ping that this machine may not send has no round-trip time, no bar at zero
 * and no 100 % loss; it has a refusal, and that is what appears.
 */

import { CircleCheck, CircleDashed, CircleSlash, CircleStop, Play } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useEvent, useRpc, useRpcMutation } from '@/api/hooks';
import type { DeviceRow, Job, JobCancelled } from '@/api/types/core';
import type {
  Hop,
  PingEcho,
  PingEchoEvent,
  PingResult,
  PortEvent,
  PortResult,
  PortsResult,
  StartedProbe,
  SweepHostEvent,
  SweepResult,
  SweptHost,
  TraceHopEvent,
  TraceResult,
} from '@/api/types/toolbox';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { ProgressBar } from '@/design/ProgressBar';
import { Spinner } from '@/design/Spinner';
import { useHistory, useJob, useRun, type Form } from './data';
import {
  HOP_COLUMNS,
  hopRow,
  ms,
  sectionsFor,
  SWEEP_COLUMNS,
  sweptRow,
  type Section,
} from './facts';
import { Fields } from './Fields';
import {
  echoWords,
  hold,
  mergeHop,
  mergeHost,
  mergePort,
  outcomeOf,
  pingStats,
  rttChart,
  take,
  type Held,
} from './probes';
import resultStyles from './Result.module.css';
import { Sections } from './Sections';
import type { Spec } from './specs';
import styles from './Tools.module.css';
import { Absence, ProbeFailure } from './Unavailable';

/** The probe kind, as `probe.history` records it, for each job-shaped tool. */
const KIND: Record<string, string> = {
  'probe.ping': 'ping',
  'probe.traceroute': 'traceroute',
  'probe.ports': 'ports',
  'probe.sweep': 'sweep',
};

interface Live {
  echoes: PingEcho[];
  hops: Hop[];
  ports: PortResult[];
  hosts: SweptHost[];
}

const NOTHING: Live = { echoes: [], hops: [], ports: [], hosts: [] };

/** One thing a stream event carries, whichever probe it came from. */
interface Part {
  echo?: PingEcho;
  hop?: Hop;
  port?: PortResult;
  host?: SweptHost;
}

export interface JobProbeProps {
  id: string;
  spec: Spec;
  form: Form;
  devices?: readonly DeviceRow[];
}

export function JobProbe({ id, spec, form, devices }: JobProbeProps) {
  const [started, setStarted] = useState<StartedProbe | null>(null);
  /** True once Stop was pressed: a partial report is then expected, not a failure. */
  const [stopped, setStopped] = useState(false);
  /** Attached to a run that was already going when this screen opened. */
  const [adopted, setAdopted] = useState(false);
  const [live, setLive] = useState<Live>(NOTHING);

  const start = useRpcMutation<StartedProbe, Record<string, unknown>>(id, {
    errorToast: false,
    invalidates: ['probe.history'],
  });
  const cancel = useRpcMutation<JobCancelled, { id: string }>('jobs.cancel', { errorToast: false });

  const job = useJob(started?.job_id ?? null);
  const run = useRun(started?.run_id ? started.run_id : null);

  /**
   * A run of this kind that is still going: the one this panel re-attaches to.
   *
   * Found through `jobs.list` rather than through the probe history, because
   * a history record does not carry its `job_id` until the run *ends* — the
   * crate sets it on the job's own copy and saves that copy only when the run
   * finishes (`crates/nocturne-toolbox/src/probe/ping.rs`, and the same in
   * `trace.rs`, `ports.rs` and `sweep.rs`; noted in NOTES.md). The job's kind
   * is the method name, so one list answers "is a ping of mine still going?".
   * The history record is then paired with it by being the only one of this
   * kind that has not finished; when that is ambiguous the panel attaches to
   * the job alone and does without the record's failure code.
   */
  const kind = KIND[id] ?? '';
  const runningJobs = useRpc<Job[]>(
    'jobs.list',
    { kind: id, state: 'running', limit: 5 },
    { invalidateOn: ['jobs.changed'] },
  );
  const unfinished = useHistory({ kind, limit: 5 }).data?.runs.filter((entry) => entry.ok === null);
  const runningJob = runningJobs.data?.[0];

  useEffect(() => {
    if (started !== null || runningJob === undefined) return;
    setStarted({
      job_id: runningJob.id,
      run_id: unfinished?.length === 1 ? (unfinished[0]?.id ?? '') : '',
    });
    setAdopted(true);
    setStopped(false);
    setLive(NOTHING);
  }, [started, runningJob?.id, unfinished?.length]);

  const jobId = started?.job_id ?? null;

  /**
   * Events that arrived before the start call came back with the job's id.
   *
   * `probe.ping` begins the job and *then* answers, so the first echo can
   * reach this screen before it knows which job it belongs to. Dropping it
   * would leave the live count one short of the report, so it is held — the
   * same rule the Terminal screen's attach follows — and taken as soon as the
   * id is known.
   */
  const held = useRef<Held<Part>[]>([]);

  const apply = (part: Part) =>
    setLive((current) => {
      if (part.echo) return { ...current, echoes: [...current.echoes, part.echo] };
      if (part.hop) return { ...current, hops: mergeHop(current.hops, part.hop) };
      if (part.port) return { ...current, ports: mergePort(current.ports, part.port) };
      if (part.host) return { ...current, hosts: mergeHost(current.hosts, part.host) };
      return current;
    });

  const receive = (eventJobId: string, part: Part) => {
    if (jobId === null || eventJobId !== jobId) {
      held.current = hold(held.current, { jobId: eventJobId, payload: part });
      return;
    }
    apply(part);
  };

  useEvent<PingEchoEvent>('probe.ping.echo', (event) => receive(event.job_id, { echo: event.echo }));
  useEvent<TraceHopEvent>('probe.trace.hop', (event) => receive(event.job_id, { hop: event.hop }));
  useEvent<PortEvent>('probe.port', (event) => receive(event.job_id, { port: event.port }));
  useEvent<SweepHostEvent>('probe.sweep.host', (event) =>
    receive(event.job_id, { host: event.host }),
  );

  // The id has arrived: take what was held for this job, in order.
  useEffect(() => {
    if (jobId === null) return;
    const waiting = take(held.current, jobId);
    held.current = [];
    for (const part of waiting) apply(part);
  }, [jobId]);

  const outcome = outcomeOf(job.data, run.data, stopped);
  const params = spec.params(form.values);
  const what = describe(id, form);

  const begin = () => {
    if (params === null) return;
    setLive(NOTHING);
    setStopped(false);
    setAdopted(false);
    setStarted(null);
    start.mutate(params, { onSuccess: (answer) => setStarted(answer) });
  };

  const stop = () => {
    if (!jobId) return;
    setStopped(true);
    cancel.mutate({ id: jobId });
  };

  const isRunning = outcome.state === 'running';

  return (
    <>
      <div className={styles.formRow}>
        <Fields
          fields={spec.fields}
          values={form.values}
          devices={devices}
          onChange={form.set}
          onSubmit={begin}
          disabled={isRunning}
        />
      </div>

      <div className={styles.actions}>
        {isRunning ? (
          <Button variant="danger" icon={CircleStop} loading={cancel.isPending} onClick={stop}>
            Stop
          </Button>
        ) : (
          <Button
            variant="primary"
            icon={Play}
            loading={start.isPending}
            disabled={params === null}
            onClick={begin}
          >
            {spec.action ?? 'Run'}
          </Button>
        )}
        {isRunning && <Running job={job.data} />}
        {params === null && !isRunning && (
          <span className={styles.actionHint}>Say what to probe, above.</span>
        )}
      </div>

      {adopted && isRunning && (
        <Absence
          reason="attached"
          message="This run was already going when the screen opened, so it was picked up rather than started again. The echoes from before are not kept by anyone, but the report has them all when it ends."
        />
      )}

      {start.error && (
        <ProbeFailure
          failure={{ code: start.error.code, message: start.error.message }}
          what={what}
        />
      )}

      {outcome.state === 'failed' && <ProbeFailure failure={outcome.failure} what={what} />}

      {outcome.state === 'idle' && !start.error && (
        <Absence
          reason="nothing_yet"
          message="Nothing has been probed yet. A run streams what it finds and is recorded in the probe history — including a run that could not start."
        />
      )}

      {(isRunning || outcome.state === 'answered') && (
        <LiveView
          id={id}
          live={live}
          result={outcome.state === 'answered' ? outcome.result : undefined}
          partial={outcome.state === 'answered' && outcome.partial}
        />
      )}
    </>
  );
}

function Running({ job }: { job: Job | undefined }) {
  if (!job) {
    return (
      <span className={styles.actionHint}>
        <Spinner size={13} label={null} /> starting…
      </span>
    );
  }
  return (
    <span className={styles.progress}>
      <Spinner size={13} label={null} />
      <span className={styles.progressWord}>{job.message || 'running'}</span>
      {job.total > 0 && (
        <ProgressBar
          value={job.done}
          max={job.total}
          aria-label="How far the probe has got"
          valueText={`${job.done} of ${job.total}`}
          className={styles.progressBar}
        />
      )}
    </span>
  );
}

// ------------------------------------------------------------- the live views

interface LiveViewProps {
  id: string;
  live: Live;
  /** The job's own report, once it has one. */
  result: unknown;
  /** The run was stopped by hand: the report covers only what it managed. */
  partial: boolean;
}

function LiveView({ id, live, result, partial }: LiveViewProps) {
  if (id === 'probe.ping') {
    return <PingView live={live} result={result as PingResult | undefined} partial={partial} />;
  }
  if (id === 'probe.traceroute') {
    return <TraceView live={live} result={result as TraceResult | undefined} />;
  }
  if (id === 'probe.ports') {
    return <PortsView live={live} result={result as PortsResult | undefined} />;
  }
  if (id === 'probe.sweep') {
    return <SweepView live={live} result={result as SweepResult | undefined} />;
  }
  return result === undefined ? null : <Sections sections={sectionsFor(id, result)} />;
}

function PingView({
  live,
  result,
  partial,
}: {
  live: Live;
  result: PingResult | undefined;
  partial: boolean;
}) {
  // Once the report is in, it is the whole truth: it holds every echo the
  // engine saw, including the ones that arrived before this screen attached.
  const echoes = result?.echoes ?? live.echoes;
  const stats = useMemo(() => pingStats(echoes), [echoes]);
  const chart = useMemo(() => rttChart(echoes), [echoes]);
  const tail = echoes.slice(-60);
  // While it runs, the figures and the rolling list are the only account of
  // it; once the report is in they would be a second copy of what the report
  // says, so they give way to it. The chart stays either way: it is the one
  // view of the run as a shape rather than as numbers.
  const finished = result !== undefined;
  const stream = useRef<HTMLDivElement>(null);
  useEffect(() => {
    // The newest echo is the one being waited for: keep it in view.
    const element = stream.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [tail.length]);

  return (
    <div className={resultStyles.live}>
      {!finished && (
      <dl className={resultStyles.liveHead}>
        <div className={resultStyles.liveStat}>
          <dt>Sent</dt>
          <dd>{stats.sent}</dd>
        </div>
        <div className={resultStyles.liveStat} data-tone={stats.received > 0 ? 'ok' : undefined}>
          <dt>Answered</dt>
          <dd>{stats.received}</dd>
        </div>
        <div
          className={resultStyles.liveStat}
          data-tone={stats.lossPct === 0 ? 'ok' : stats.lossPct >= 100 ? 'crit' : 'warn'}
        >
          <dt>Loss</dt>
          <dd>{stats.sent === 0 ? '—' : `${stats.lossPct}%`}</dd>
        </div>
        <div className={resultStyles.liveStat}>
          <dt>Last</dt>
          <dd>{ms(stats.last) ?? '—'}</dd>
        </div>
        <div className={resultStyles.liveStat}>
          <dt>Average</dt>
          <dd>{ms(stats.avg) ?? '—'}</dd>
        </div>
        <div className={resultStyles.liveStat}>
          <dt>Shortest</dt>
          <dd>{ms(stats.min) ?? '—'}</dd>
        </div>
        <div className={resultStyles.liveStat}>
          <dt>Longest</dt>
          <dd>{ms(stats.max) ?? '—'}</dd>
        </div>
      </dl>
      )}

      {chart.bars.length > 0 && (
        <>
          <div
            className={resultStyles.chart}
            role="img"
            aria-label={`Round-trip time of ${chart.bars.length} echoes, up to ${chart.scaleMs} ms, with ${
              stats.sent - stats.received
            } lost`}
          >
            {chart.bars.map((bar) => (
              <span
                key={bar.seq}
                className={resultStyles.bar}
                data-lost={bar.rtt === null ? 'true' : undefined}
                style={{ ['--bar-height' as string]: `${(bar.height * 100).toFixed(1)}%` }}
                title={bar.rtt === null ? `#${bar.seq}: no reply` : `#${bar.seq}: ${ms(bar.rtt)}`}
              />
            ))}
          </div>
          <p className={resultStyles.chartFoot}>
            <span>Full height is {ms(chart.scaleMs)}</span>
            <span>A red mark at the bottom is an echo that never came back</span>
            {chart.older > 0 && <span>{chart.older} older echoes are not drawn</span>}
          </p>
        </>
      )}

      {tail.length > 0 && !finished && (
        <div className={resultStyles.stream} ref={stream} aria-label="Echoes, newest last">
          {tail.map((echo) => (
            <div
              key={echo.seq}
              className={resultStyles.streamLine}
              data-lost={echo.rtt_ms === null ? 'true' : undefined}
            >
              <span className={resultStyles.streamSeq}>{echo.seq}</span>
              <span>{echoWords(echo)}</span>
            </div>
          ))}
        </div>
      )}

      {partial && result && (
        <p className={resultStyles.note} data-tone="info">
          Stopped by you after {result.sent} {result.sent === 1 ? 'echo' : 'echoes'}. What it saw up
          to then is the report below.
        </p>
      )}

      {result && <Sections sections={sectionsFor('probe.ping', result)} />}
    </div>
  );
}

function TraceView({ live, result }: { live: Live; result: TraceResult | undefined }) {
  if (result) return <Sections sections={sectionsFor('probe.traceroute', result)} />;
  const sections: Section[] = [
    {
      kind: 'table',
      title: 'The path, as it is found',
      columns: HOP_COLUMNS,
      rows: live.hops.map(hopRow),
      empty: 'No hop has answered yet.',
      note: 'A hop appears as soon as a router answers for it.',
    },
  ];
  return <Sections sections={sections} />;
}

function PortsView({ live, result }: { live: Live; result: PortsResult | undefined }) {
  // While it runs only the open ports are streamed; the closed ones arrive
  // with the report, which is why the grid says what it is showing.
  const ports = result?.ports ?? live.ports;
  return (
    <div className={resultStyles.live}>
      <div className={resultStyles.ports} aria-label="Ports">
        {ports.map((port) => (
          <span
            key={port.port}
            className={resultStyles.port}
            data-state={port.state}
            title={`${port.port}: ${port.state}${port.banner ? ` — ${port.banner}` : ''}`}
          >
            {/* The state is a shape as well as a colour: a grid of chips that
                differ only in hue says nothing to a colour-blind reader. */}
            {port.state === 'open' ? (
              <Icon icon={CircleCheck} size={12} label="open" />
            ) : port.state === 'filtered' ? (
              <Icon icon={CircleDashed} size={12} label="filtered" />
            ) : (
              <Icon icon={CircleSlash} size={12} label="closed" />
            )}
            {port.port}
            {port.service && <span className={resultStyles.portService}>{port.service}</span>}
          </span>
        ))}
      </div>
      <p className={resultStyles.chartFoot}>
        {result
          ? 'Every port that was tried. A closed port is one that answered "no"; a filtered one said nothing.'
          : 'Open ports appear as they are found. The closed and filtered ones arrive with the report.'}
      </p>
      {result && <Sections sections={sectionsFor('probe.ports', result)} />}
    </div>
  );
}

function SweepView({ live, result }: { live: Live; result: SweepResult | undefined }) {
  if (result) return <Sections sections={sectionsFor('probe.sweep', result)} />;
  const sections: Section[] = [
    {
      kind: 'table',
      title: 'What has answered so far',
      columns: SWEEP_COLUMNS,
      rows: live.hosts.map(sweptRow),
      empty: 'Nothing has answered yet.',
      note: 'Only addresses that answer are streamed; the rest are counted in the report.',
    },
  ];
  return <Sections sections={sections} />;
}

/** What a failure sentence calls this run. */
function describe(id: string, form: Form): string {
  const target =
    form.values['host'] || form.values['device'] || form.values['targets'] || 'that target';
  switch (id) {
    case 'probe.ping':
      return `the ping of ${target}`;
    case 'probe.traceroute':
      return `the traceroute to ${target}`;
    case 'probe.ports':
      return `the port check of ${target}`;
    case 'probe.sweep':
      return `the sweep of ${target}`;
    default:
      return 'this probe';
  }
}
