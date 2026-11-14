/**
 * What was run: every probe, with what came back, and whether it could run.
 *
 * This is the screen's memory, and it is the backend's: `probe.history` holds
 * a record written *before* each probe runs and finished afterwards, so a
 * probe that is still going is in the list, and one that died with the process
 * says so. That is why this panel can be opened an hour later and still answer
 * "what did that traceroute look like before we changed the route?".
 *
 * The column that matters is the outcome, and it has four values, not two:
 * running, answered, could not run (this machine), and no answer (the
 * network). A failed run carries `result: null` — there is no substitute for
 * an answer — so opening one shows the refusal and its code, never an empty
 * report.
 */

import { Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type { ProbeRun } from '@/api/types/toolbox';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount } from '@/lib/format';
import { Refusal } from './Answer';
import { useDebounced, useHistory, useRun, type Form } from './data';
import { KIND_TO_TOOL, sectionsFor } from './facts';
import { failureWords } from './probes';
import { Sections } from './Sections';
import styles from './Tools.module.css';
import resultStyles from './Result.module.css';

const KINDS = [
  { value: '', label: 'Every kind' },
  { value: 'ping', label: 'Ping' },
  { value: 'traceroute', label: 'Traceroute' },
  { value: 'ports', label: 'Port check' },
  { value: 'sweep', label: 'Sweep' },
  { value: 'dns', label: 'DNS' },
  { value: 'whois', label: 'WHOIS' },
  { value: 'tls', label: 'TLS' },
  { value: 'http', label: 'HTTP' },
  { value: 'ntp', label: 'NTP' },
  { value: 'wol', label: 'Wake on LAN' },
  { value: 'device.ping,device.traceroute', label: 'From a device' },
  { value: 'tftp,syslog', label: 'Servers' },
];

/** The outcome of a run, in the four words it can be. */
export function outcomeWords(run: ProbeRun): { tone: 'ok' | 'warn' | 'crit' | 'info'; word: string } {
  if (run.ok === null) return { tone: 'info', word: 'Running' };
  if (run.ok) return { tone: 'ok', word: 'Answered' };
  const { aboutThisMachine } = failureWords(run.error_code ?? '');
  return aboutThisMachine
    ? { tone: 'crit', word: 'Could not run' }
    : { tone: 'warn', word: 'No answer' };
}

type Keyed = ProbeRun & { __key: string };

export function HistoryPanel({ form }: { form: Form }) {
  const kind = form.values['kind'] ?? '';
  const query = useDebounced(form.values['query'] ?? '');
  const history = useHistory({
    kind: kind || undefined,
    query: query.trim() || undefined,
    limit: 200,
  });
  const [open, setOpen] = useState<string | null>(null);
  const forget = useRpcMutation<{ removed: number }, { all: true }>('probe.history.delete', {
    invalidates: ['probe.history'],
    successToast: 'The probe history was cleared',
  });

  const columns: TableColumn<Keyed>[] = [
    {
      id: 'outcome',
      header: 'Outcome',
      width: 130,
      cell: (run) => {
        const { tone, word } = outcomeWords(run);
        return <StatusPill tone={tone}>{word}</StatusPill>;
      },
      sortValue: (run) => outcomeWords(run).word,
      required: true,
    },
    { id: 'title', header: 'What', cell: (run) => run.title, sortValue: (run) => run.title, required: true },
    { id: 'summary', header: 'Result', cell: (run) => run.summary, sortValue: (run) => run.summary },
    {
      id: 'from',
      header: 'Ran from',
      width: 180,
      cell: (run) => run.from,
      sortValue: (run) => run.from,
    },
    {
      id: 'method',
      header: 'Sent as',
      mono: true,
      cell: (run) => run.method ?? '—',
      sortValue: (run) => run.method,
      defaultHidden: true,
    },
    {
      id: 'started_at',
      header: 'When',
      width: 120,
      cell: (run) => <TimeAgo at={run.started_at} />,
      sortValue: (run) => run.started_at,
    },
  ];

  const rows: Keyed[] = (history.data?.runs ?? []).map((run) => ({ ...run, __key: run.id }));

  return (
    <>
      <div className={styles.formRow}>
        <div className={styles.form}>
          <div className={styles.formField} data-span={1}>
            <Field label="Kind">
              <Select
                value={kind}
                options={KINDS}
                onChange={(event) => form.set('kind', event.currentTarget.value)}
              />
            </Field>
          </div>
          <div className={styles.formField} data-span={1}>
            <Field label="Search" hint="Over what was asked for, the target and the result line.">
              <SearchInput
                value={form.values['query'] ?? ''}
                placeholder="10.0.0.1, core-sw1…"
                aria-label="Search the probe history"
                onChange={(value) => form.set('query', value)}
              />
            </Field>
          </div>
        </div>
      </div>

      <div className={styles.actions}>
        <Button
          variant="secondary"
          icon={Trash2}
          loading={forget.isPending}
          disabled={(history.data?.total ?? 0) === 0}
          onClick={() => {
            void confirm({
              title: 'Forget every finished probe run?',
              description:
                'The record of every probe that has finished is deleted, here and for the assistant. A run that is still going is kept. This cannot be undone.',
              confirmLabel: 'Forget them',
              tone: 'danger',
            }).then((yes) => {
              if (yes) forget.mutate({ all: true });
            });
          }}
        >
          Forget finished runs
        </Button>
        <span className={styles.actionHint}>
          {history.data
            ? `${formatCount(history.data.total)} ${history.data.total === 1 ? 'run' : 'runs'} recorded${
                history.data.cursor === null ? '' : `, the newest ${rows.length} shown`
              }.`
            : 'Reading the history…'}
        </span>
      </div>

      {history.error ? (
        <Refusal error={history.error} />
      ) : (
        <div className={styles.answer}>
          <Table
            id="tools.history"
            columns={columns}
            rows={rows}
            rowKey={(run) => run.__key}
            aria-label="Probe runs"
            density="compact"
            loading={history.isPending}
            maxHeight={420}
            defaultSort={{ columnId: 'started_at', direction: 'desc' }}
            onRowActivate={(run) => setOpen(run.id)}
            rowActions={(run) => [{ id: 'open', label: 'Open this run', onSelect: () => setOpen(run.id) }]}
            empty={
              <p className={resultStyles.empty}>
                Nothing has been probed yet from here. A ping, a traceroute, a port check or a
                lookup is recorded the moment it starts.
              </p>
            }
          />
          <p className={resultStyles.tableNote}>
            Enter on a row, or the row’s menu, opens the whole record.
          </p>
        </div>
      )}

      {open !== null && <OneRun id={open} onClose={() => setOpen(null)} />}
    </>
  );
}

function OneRun({ id, onClose }: { id: string; onClose: () => void }) {
  const run = useRun(id);
  const record = run.data;

  return (
    <section className={styles.openRun} aria-label="One probe run">
      <header className={styles.openRunHead}>
        <h4 className={styles.openRunTitle}>{record?.title ?? 'Reading the run…'}</h4>
        <Button variant="ghost" size="sm" onClick={onClose}>
          Close
        </Button>
      </header>

      {run.error && <Refusal error={run.error} />}

      {record && (
        <>
          <Sections
            sections={[
              {
                kind: 'facts',
                facts: [
                  { label: 'Asked for', value: record.target, mono: true },
                  ...(record.address ? [{ label: 'Which was', value: record.address, mono: true }] : []),
                  { label: 'Outcome', value: outcomeWords(record).word, tone: outcomeWords(record).tone },
                  { label: 'Result line', value: record.summary },
                  { label: 'Ran from', value: record.from },
                  ...(record.method ? [{ label: 'Sent as', value: record.method, mono: true }] : []),
                  ...(record.device
                    ? [{ label: 'The device', value: `${record.device.name}${record.device.lab ? ' (lab)' : ''}` }]
                    : []),
                ],
              },
            ]}
          />

          {record.ok === false ? (
            <div className={resultStyles.note} data-tone="crit">
              <strong>{failureWords(record.error_code ?? '').headline}.</strong>{' '}
              {record.error ?? 'No reason was recorded.'}
              <br />
              This run has no result at all — the engine recorded the refusal rather than a figure
              that would look like one. The code was <code>{record.error_code ?? 'unknown'}</code>.
            </div>
          ) : record.ok === null ? (
            <p className={resultStyles.note} data-tone="info">
              Still going. Its report appears here when it finishes.
            </p>
          ) : record.result === null || record.result === undefined ? (
            <p className={resultStyles.note} data-tone="idle">
              This run finished but kept no report.
            </p>
          ) : (
            <Sections sections={sectionsFor(KIND_TO_TOOL[record.kind] ?? '', record.result)} />
          )}
        </>
      )}
    </section>
  );
}
