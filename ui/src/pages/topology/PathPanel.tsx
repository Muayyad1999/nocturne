/**
 * Path mode: `topology.path` between two points, drawn on the graph and
 * written out hop by hop.
 *
 * The walk reads collected routing tables, then ARP and MAC tables for the
 * last hop, and it stops rather than guessing. Those stops are the valuable
 * part — "no route data collected for R3", "the next hop 203.0.113.1 is not an
 * address of any device Nocturne manages" — so this panel gives the reason the
 * same weight as the hops, quotes it as the backend wrote it, and never calls
 * an incomplete walk a failure.
 *
 * Both ends are plain fields with the drawing's device names offered as
 * suggestions, because `topology.path` takes a device id, a name *or* an IP
 * address, and tracing to an address nobody manages is one of the questions
 * worth asking.
 */

import { CircleSlash, Flag, Route } from 'lucide-react';
import type { Graph, Trace } from '@/api/types/topology';
import type { RpcError } from '@/api/errors';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Skeleton } from '@/design/Skeleton';
import { plural } from '@/lib/format';
import styles from './PathPanel.module.css';
import { viaLabel } from './trace';

export interface PathPanelProps {
  graph: Graph;
  from: string;
  to: string;
  onFrom: (value: string) => void;
  onTo: (value: string) => void;
  /** Whether a trace has been asked for at all. */
  asked: boolean;
  trace: Trace | undefined;
  pending: boolean;
  error: RpcError | null;
  onTrace: () => void;
  onClear: () => void;
}

const SUGGESTIONS = 'topology-path-devices';

export function PathPanel({
  graph,
  from,
  to,
  onFrom,
  onTo,
  asked,
  trace,
  pending,
  error,
  onTrace,
  onClear,
}: PathPanelProps) {
  const names = [...new Set(graph.nodes.map((node) => node.name))].sort((one, other) =>
    one.localeCompare(other),
  );

  return (
    <section className={styles.panel} aria-label="Trace a path">
      <datalist id={SUGGESTIONS}>
        {names.map((name) => (
          <option key={name} value={name} />
        ))}
      </datalist>

      <form
        className={styles.pickers}
        onSubmit={(event) => {
          event.preventDefault();
          onTrace();
        }}
      >
        <Field label="From" hint="A device name, or an address">
          <Input
            value={from}
            list={SUGGESTIONS}
            placeholder="core-sw1"
            onChange={(event) => onFrom(event.target.value)}
          />
        </Field>
        <Field label="To" hint="A device name, or an address">
          <Input
            value={to}
            list={SUGGESTIONS}
            placeholder="10.10.20.10"
            onChange={(event) => onTo(event.target.value)}
          />
        </Field>
        <div className={styles.buttons}>
          <Button
            type="submit"
            size="sm"
            variant="primary"
            icon={Route}
            disabled={from.trim() === '' || to.trim() === ''}
            loading={pending}
          >
            Trace
          </Button>
          {asked && (
            <Button size="sm" variant="ghost" onClick={onClear}>
              Clear
            </Button>
          )}
        </div>
      </form>

      {!asked && (
        <EmptyState
          icon={Route}
          title="Two points, and what is between them"
          description="Nocturne walks the routing, ARP and MAC tables it has already collected. It never guesses a next hop: where the data runs out, it says so."
        />
      )}

      {asked && pending && (
        <div className={styles.loading} aria-busy="true">
          <Skeleton />
          <Skeleton width="70%" />
          <Skeleton width="85%" />
        </div>
      )}

      {error && <p className={styles.error}>{error.message}</p>}

      {trace && !pending && <TraceResult trace={trace} />}
    </section>
  );
}

function TraceResult({ trace }: { trace: Trace }) {
  return (
    <div className={styles.result}>
      <header className={styles.head} data-complete={trace.complete || undefined}>
        <Icon icon={trace.complete ? Flag : CircleSlash} size={14} />
        <span className={styles.ends}>
          {trace.from} → {trace.to}
          {trace.destination && trace.destination !== trace.to && (
            <span className={styles.aim}> (aiming at {trace.destination})</span>
          )}
        </span>
        <b>
          {trace.complete
            ? `arrived in ${plural(trace.hops.length, 'hop')}`
            : 'did not arrive'}
        </b>
      </header>

      <ol className={styles.hops}>
        {trace.hops.map((hop, index) => (
          <li key={`${index}-${hop.node}`}>
            <span className={styles.number}>{index + 1}</span>
            <div>
              <p className={styles.hop}>
                <b>{hop.node}</b>
                {hop.in_interface && <span className="mono">in {hop.in_interface}</span>}
                {hop.out_interface && <span className="mono">out {hop.out_interface}</span>}
                <span className={styles.via}>{viaLabel(hop.via)}</span>
              </p>
              {/* The device's own reasoning, exactly as the backend put it. */}
              <p className={styles.detail}>{hop.detail}</p>
            </div>
          </li>
        ))}
      </ol>

      {/*
        Not a failure: the walk reached the edge of what has been collected and
        said which edge. That sentence is the answer.
      */}
      {!trace.complete && trace.stopped_because && (
        <section className={styles.stopped}>
          <h3>Why it stops here</h3>
          <p>{trace.stopped_because}</p>
        </section>
      )}

      {trace.notes.length > 0 && (
        <section className={styles.notes}>
          <h3>{plural(trace.notes.length, 'note')}</h3>
          <ul>
            {trace.notes.map((note) => (
              <li key={note}>{note}</li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
