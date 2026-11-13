/**
 * "Can A reach B?" — and, when it cannot, where the packet died.
 *
 * `labs.probe` walks one packet through the same forwarding code the timed
 * simulation uses and returns every hop with the device's own reasoning;
 * `labs.connectivity` does that for every pair. Both are worth showing in
 * full: the sentence the simulator writes about an access list or a missing
 * route is more useful than any verdict this screen could invent, so it is
 * never reduced to a red cross.
 */

import { CircleCheck, OctagonExclamationPoint, Play, Route } from 'lucide-react';
import { useState } from 'react';
import { rpc } from '@/api/client';
import { RpcError } from '@/api/errors';
import type { Connectivity, LabView, ProbeResult } from '@/api/types/lab';
import { Button } from '@/design/Button';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Spinner } from '@/design/Spinner';
import { formatPercent } from '@/lib/format';
import { addressableNodes, nodeAddress } from './model';
import styles from './Reachability.module.css';

export interface ReachabilityProps {
  open: boolean;
  onClose: () => void;
  lab: LabView;
  /** Starts the lab: nothing can be probed until it runs. */
  onStart: () => void;
  starting: boolean;
}

type Kind = 'ping' | 'tcp';

export function Reachability({ open, onClose, lab, onStart, starting }: ReachabilityProps) {
  const nodes = addressableNodes(lab.snapshot.nodes);
  const [from, setFrom] = useState('');
  const [to, setTo] = useState('');
  const [kind, setKind] = useState<Kind>('ping');
  const [port, setPort] = useState('80');

  const [probe, setProbe] = useState<ProbeResult | null>(null);
  const [matrix, setMatrix] = useState<Connectivity | null>(null);
  const [cell, setCell] = useState<{ from: string; to: string; reason: string } | null>(null);
  const [busy, setBusy] = useState<'probe' | 'matrix' | null>(null);
  const [error, setError] = useState<unknown>(null);

  const source = from || nodes[0]?.name || '';
  const target = to || nodes[1]?.name || '';

  const run = async (what: 'probe' | 'matrix') => {
    setBusy(what);
    setError(null);
    try {
      if (what === 'probe') {
        setMatrix(null);
        setCell(null);
        const result = await rpc<ProbeResult>('labs.probe', {
          lab_id: lab.id,
          from: source,
          to: target,
          kind,
          ...(kind === 'tcp' ? { port: Number(port) || 80 } : {}),
        });
        setProbe(result);
      } else {
        setProbe(null);
        setCell(null);
        const result = await rpc<Connectivity>('labs.connectivity', { lab_id: lab.id });
        setMatrix(result);
      }
    } catch (failure) {
      setProbe(null);
      setMatrix(null);
      setError(failure instanceof RpcError ? failure : new Error(String(failure)));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      size="lg"
      title="Reachability"
      description={`Walk a packet through ${lab.name} and read what each device did with it.`}
    >
      {!lab.running ? (
        <EmptyState
          icon={Play}
          title="The lab is not running"
          description="The simulator only answers probes while it runs; nothing about reachability can be said about a stopped lab."
          action={
            <Button variant="primary" icon={Play} loading={starting} onClick={onStart}>
              Start the lab
            </Button>
          }
        />
      ) : nodes.length < 2 ? (
        <EmptyState
          icon={Route}
          title="Nothing to probe yet"
          description={
            nodes.length === 0
              ? 'No powered device in this lab has an address, so there is nowhere to send a packet from or to. Configure an address on a device, or add a host.'
              : `Only ${nodes[0]?.name} has an address, so there is nothing to reach. Add a second addressed device.`
          }
        />
      ) : (
        <div className={styles.body}>
          <div className={styles.form}>
            <Field label="From">
              <Select
                value={source}
                onChange={(event) => setFrom(event.target.value)}
                options={nodes.map((node) => ({
                  value: node.name,
                  label: `${node.name} (${nodeAddress(node) ?? ''})`,
                }))}
              />
            </Field>
            <Field label="To">
              <Select
                value={target}
                onChange={(event) => setTo(event.target.value)}
                options={nodes.map((node) => ({
                  value: node.name,
                  label: `${node.name} (${nodeAddress(node) ?? ''})`,
                }))}
              />
            </Field>
            <Field label="Probe">
              <Select
                value={kind}
                onChange={(event) => setKind(event.target.value as Kind)}
                options={[
                  { value: 'ping', label: 'ICMP ping' },
                  { value: 'tcp', label: 'TCP connect' },
                ]}
              />
            </Field>
            {kind === 'tcp' && (
              <Field label="Port">
                <Input
                  value={port}
                  inputMode="numeric"
                  onChange={(event) => setPort(event.target.value)}
                />
              </Field>
            )}
            <div className={styles.run}>
              <Button
                variant="primary"
                icon={Route}
                loading={busy === 'probe'}
                disabled={busy !== null || source === target}
                onClick={() => void run('probe')}
              >
                Check this pair
              </Button>
              <Button
                loading={busy === 'matrix'}
                disabled={busy !== null}
                onClick={() => void run('matrix')}
              >
                Check every pair
              </Button>
            </div>
            {source === target && (
              <p className={styles.note}>Choose two different devices to send a packet between.</p>
            )}
          </div>

          {error != null && <ErrorState title="The probe did not run" error={error} size="compact" />}

          {busy !== null && !probe && !matrix && (
            <p className={styles.waiting}>
              <Spinner size={14} /> Walking the packet…
            </p>
          )}

          {probe && <ProbePanel result={probe} from={source} to={target} />}

          {matrix && (
            <MatrixPanel
              matrix={matrix}
              selected={cell}
              onSelect={(selection) => setCell(selection)}
            />
          )}
        </div>
      )}
    </Drawer>
  );
}

function ProbePanel({ result, from, to }: { result: ProbeResult; from: string; to: string }) {
  return (
    <section className={styles.result} aria-label="Probe result">
      <header className={styles.verdict} data-ok={result.ok || undefined} data-failed={!result.ok || undefined}>
        <Icon icon={result.ok ? CircleCheck : OctagonExclamationPoint} size={16} />
        <strong>
          {result.ok ? `${from} reaches ${to}` : `${from} cannot reach ${to}`}
        </strong>
        {result.ok && <span className={styles.rtt}>{(result.rtt_us / 1000).toFixed(2)} ms</span>}
      </header>

      {!result.ok && result.reason !== '' && <p className={styles.reason}>{result.reason}</p>}

      <ol className={styles.hops}>
        {result.hops.map((hop, index) => (
          <li key={`${hop.node}-${index}`} className={styles.hop}>
            <div className={styles.hopHead}>
              <span className={styles.hopIndex}>{index + 1}</span>
              <span className={styles.hopName}>{hop.name}</span>
              <span className={styles.hopPorts}>
                {hop.in_if && <span className={styles.mono}>in {hop.in_if}</span>}
                {hop.out_if && <span className={styles.mono}>out {hop.out_if}</span>}
              </span>
            </div>
            <p className={styles.hopNote}>{hop.note}</p>
          </li>
        ))}
      </ol>
      {result.hops.length === 0 && (
        <p className={styles.reason}>The simulator reported no hops: the packet was never sent.</p>
      )}
    </section>
  );
}

interface MatrixPanelProps {
  matrix: Connectivity;
  selected: { from: string; to: string; reason: string } | null;
  onSelect: (cell: { from: string; to: string; reason: string } | null) => void;
}

function MatrixPanel({ matrix, selected, onSelect }: MatrixPanelProps) {
  return (
    <section className={styles.result} aria-label="Connectivity matrix">
      <header className={styles.verdict} data-ok={matrix.reachable_pct === 100 || undefined}>
        <Icon icon={matrix.reachable_pct === 100 ? CircleCheck : Route} size={16} />
        <strong>
          {formatPercent(matrix.reachable_pct / 100, { fromRatio: true, digits: 1 })} of the{' '}
          {matrix.nodes.length * (matrix.nodes.length - 1)} ordered pairs get through
        </strong>
      </header>

      <div className={styles.matrixScroll}>
        <table className={styles.matrix}>
          <caption className={styles.caption}>
            A row is the sender, a column the destination. Select a cell to read what the simulator
            said.
          </caption>
          <thead>
            <tr>
              <th scope="col" className={styles.corner}>
                from \ to
              </th>
              {matrix.nodes.map((node) => (
                <th key={node} scope="col" className={styles.columnHead}>
                  <span>{node}</span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {matrix.nodes.map((source, row) => (
              <tr key={source}>
                <th scope="row" className={styles.rowHead}>
                  {source}
                </th>
                {matrix.nodes.map((target, column) => {
                  const result = matrix.matrix[row]?.[column];
                  const self = row === column;
                  const ok = result?.ok ?? false;
                  return (
                    <td key={target} className={styles.cell}>
                      {self ? (
                        <span className={styles.self} aria-hidden="true">
                          ·
                        </span>
                      ) : (
                        <button
                          type="button"
                          className={styles.mark}
                          data-ok={ok || undefined}
                          data-selected={
                            (selected?.from === source && selected.to === target) || undefined
                          }
                          aria-label={`${source} to ${target}: ${ok ? 'reachable' : 'not reachable'}`}
                          onClick={() =>
                            onSelect({ from: source, to: target, reason: result?.reason ?? '' })
                          }
                        >
                          {ok ? '●' : '✕'}
                        </button>
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {selected && (
        <p className={styles.reason} role="status">
          <strong>
            {selected.from} → {selected.to}:
          </strong>{' '}
          {selected.reason === '' ? 'the packet got through.' : selected.reason}
        </p>
      )}
    </section>
  );
}
