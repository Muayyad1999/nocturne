/**
 * The two servers: a TFTP server for gear that speaks nothing else, and a
 * staging syslog listener.
 *
 * These are the only things on this screen that are *running* rather than
 * *answering*, and the panel is built around that: the state is the backend's
 * (`tftp.status`, `probe.syslog.status`), not this component's, so leaving the
 * screen does not stop a transfer and coming back shows the socket that is
 * still open, how long it has been open, and what has happened on it. There is
 * no "start" that could be pressed twice: while one is running the panel shows
 * it and offers Stop.
 *
 * Both are refused unless the caller sits at the machine the engine runs on
 * (`app.info` → `local_access`), because they bind a socket on that machine.
 * The panel asks once and says so, rather than offering a button that is
 * always refused — and the refusal, if it happens anyway, is shown in the
 * engine's own words.
 *
 * The syslog listener says, on screen, what the engine says in its `note`:
 * nothing it hears is stored. It is for staging a device, and Nocturne's log
 * collection is a different feature.
 */

import { CircleStop, Play } from 'lucide-react';
import { useState } from 'react';
import { useEvent, useRpcMutation } from '@/api/hooks';
import type {
  Stopped,
  SyslogMessage,
  SyslogStarted,
  SyslogStatus,
  TftpEvent,
  TftpStarted,
} from '@/api/types/toolbox';
import { Button } from '@/design/Button';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { formatBytes, formatCount } from '@/lib/format';
import { Refusal } from './Answer';
import { useLocalAccess, useSyslogMessages, useSyslogStatus, useTftpStatus, type Form } from './data';
import { Fields } from './Fields';
import { Facts } from './Sections';
import type { Spec } from './specs';
import styles from './Tools.module.css';
import resultStyles from './Result.module.css';

/** What a TFTP event says, in one line. */
export function transferWords(event: TftpEvent): { id: string; text: string; tone?: string } {
  if ('started' in event) {
    const { id, peer, file, write, size } = event.started;
    return {
      id: `s${id}`,
      text: `${peer} ${write ? 'is uploading' : 'is fetching'} ${file}${
        size === null ? '' : ` (${formatBytes(size)})`
      }`,
    };
  }
  if ('progress' in event) {
    return {
      id: `p${event.progress.id}`,
      text: `transfer ${event.progress.id}: ${formatBytes(event.progress.bytes)} so far`,
    };
  }
  if ('finished' in event) {
    const { id, bytes, elapsed_ms } = event.finished;
    return { id: `f${id}`, text: `transfer ${id} finished, ${formatBytes(bytes)} in ${elapsed_ms} ms`, tone: 'ok' };
  }
  if ('failed' in event) {
    return { id: `x${event.failed.id}`, text: `transfer ${event.failed.id} failed: ${event.failed.error}`, tone: 'crit' };
  }
  const { peer, file, reason } = event.denied;
  return { id: `d${peer}${file}`, text: `refused ${peer} access to ${file}: ${reason}`, tone: 'warn' };
}

export function TftpPanel({ spec, form }: { spec: Spec; form: Form }) {
  const { local } = useLocalAccess();
  const status = useTftpStatus();
  const start = useRpcMutation<TftpStarted, Record<string, unknown>>('tftp.start', {
    errorToast: false,
    invalidates: ['tftp.', 'probe.history'],
  });
  const stop = useRpcMutation<Stopped, Record<string, never>>('tftp.stop', {
    errorToast: false,
    invalidates: ['tftp.'],
  });
  /** Events seen while this screen is open, on top of what `tftp.status` holds. */
  const [seen, setSeen] = useState<TftpEvent[]>([]);
  useEvent<{ job_id: string } & TftpEvent>('tftp.transfer', (event) => {
    const { job_id: _job, ...rest } = event;
    setSeen((current) => [...current, rest as TftpEvent]);
  });

  const running = status.data?.running === true;
  const transfers = running ? [...(status.data?.transfers ?? []), ...seen] : [];

  return (
    <>
      <Standing
        running={running}
        word={running ? 'Serving' : 'Not running'}
        facts={
          running
            ? [
                { label: 'Listening on', value: status.data?.address ?? '', mono: true, copy: true },
                { label: 'Serving', value: status.data?.root ?? '', mono: true },
                {
                  label: 'Uploads',
                  value: status.data?.allow_write
                    ? status.data?.allow_overwrite
                      ? 'allowed, and may replace a file that is there'
                      : 'allowed, but not over a file that is there'
                    : 'not allowed: downloads only',
                  tone: status.data?.allow_write ? 'warn' : 'ok',
                },
                ...(status.data?.started_at
                  ? [{ label: 'Since', value: new Date(status.data.started_at).toLocaleTimeString() }]
                  : []),
              ]
            : [
                {
                  label: 'Defaults',
                  value: `${status.data?.defaults.bind ?? '—'}:${status.data?.defaults.port ?? '—'}`,
                  mono: true,
                },
                {
                  label: 'Directory',
                  value: status.data?.defaults.root || 'none set — say which directory to serve',
                  mono: Boolean(status.data?.defaults.root),
                  tone: status.data?.defaults.root ? undefined : 'idle',
                },
              ]
        }
      />

      {local === false ? (
        <p className={styles.secretIdle}>
          A TFTP server binds a socket on the machine the engine runs on, so it can only be started
          from the desktop application. This Nocturne is answering over the network.
        </p>
      ) : running ? (
        <div className={styles.actions}>
          <Button variant="danger" icon={CircleStop} loading={stop.isPending} onClick={() => stop.mutate({})}>
            Stop serving
          </Button>
          <span className={styles.actionHint}>
            It keeps serving until it is stopped or Nocturne closes.
          </span>
        </div>
      ) : (
        <>
          <div className={styles.formRow}>
            <Fields fields={spec.fields} values={form.values} onChange={form.set} disabled={start.isPending} />
          </div>
          <div className={styles.actions}>
            <Button
              variant="primary"
              icon={Play}
              loading={start.isPending}
              onClick={() => start.mutate(spec.params(form.values) ?? {})}
            >
              {spec.action ?? 'Start'}
            </Button>
            <span className={styles.actionHint}>
              The port is bound before anything is reported, so a port that cannot be had is an
              error here and not a server that looks as though it started.
            </span>
          </div>
        </>
      )}

      {start.error && <Refusal error={start.error} />}
      {stop.error && <Refusal error={stop.error} />}

      {running && (
        <div className={resultStyles.block}>
          <h4 className={resultStyles.blockTitle}>What has happened on it</h4>
          {transfers.length === 0 ? (
            <p className={resultStyles.empty}>
              Nothing yet. A device fetching a file appears here as it happens.
            </p>
          ) : (
            <div className={resultStyles.stream} aria-label="Transfers, newest last">
              {transfers.map((event, index) => {
                const line = transferWords(event);
                return (
                  <div key={`${line.id}-${index}`} className={resultStyles.streamLine} data-lost={line.tone === 'crit' ? 'true' : undefined}>
                    <span className={resultStyles.streamSeq}>{index + 1}</span>
                    <span>{line.text}</span>
                  </div>
                );
              })}
            </div>
          )}
          {(status.data?.dropped ?? 0) > 0 && (
            <p className={resultStyles.tableNote}>
              {formatCount(status.data?.dropped)} older events are no longer kept.
            </p>
          )}
        </div>
      )}
    </>
  );
}

const MESSAGE_COLUMNS: TableColumn<SyslogMessage & { __key: string }>[] = [
  { id: 'source', header: 'From', mono: true, cell: (row) => row.source },
  {
    id: 'severity',
    header: 'Severity',
    cell: (row) => (row.severity === null ? '—' : String(row.severity)),
    align: 'right',
  },
  { id: 'app', header: 'Program', cell: (row) => row.app ?? row.mnemonic?.facility ?? '—' },
  { id: 'mnemonic', header: 'Tag', mono: true, cell: (row) => row.mnemonic?.name ?? '—' },
  { id: 'message', header: 'Message', cell: (row) => row.message },
];

export function SyslogPanel({ spec, form }: { spec: Spec; form: Form }) {
  const { local } = useLocalAccess();
  const status = useSyslogStatus();
  const listening = status.data?.listening === true;
  const messages = useSyslogMessages(listening);
  const start = useRpcMutation<SyslogStarted, Record<string, unknown>>('probe.syslog.start', {
    errorToast: false,
    invalidates: ['probe.syslog.', 'probe.history'],
  });
  const stop = useRpcMutation<Stopped, Record<string, never>>('probe.syslog.stop', {
    errorToast: false,
    invalidates: ['probe.syslog.'],
  });

  return (
    <>
      <Standing
        running={listening}
        word={listening ? 'Listening' : 'Not listening'}
        facts={standingSyslogFacts(status.data)}
      />

      <p className={resultStyles.note} data-tone="info">
        {status.data?.note ??
          'A staging listener: it keeps messages in memory while it runs and stores nothing.'}
      </p>

      {local === false ? (
        <p className={styles.secretIdle}>
          The listener binds a socket on the machine the engine runs on, so it can only be started
          from the desktop application. This Nocturne is answering over the network.
        </p>
      ) : listening ? (
        <div className={styles.actions}>
          <Button variant="danger" icon={CircleStop} loading={stop.isPending} onClick={() => stop.mutate({})}>
            Stop listening
          </Button>
          <span className={styles.actionHint}>
            Messages are dropped when it stops: nothing here is written to the log store.
          </span>
        </div>
      ) : (
        <>
          <div className={styles.formRow}>
            <Fields fields={spec.fields} values={form.values} onChange={form.set} disabled={start.isPending} />
          </div>
          <div className={styles.actions}>
            <Button
              variant="primary"
              icon={Play}
              loading={start.isPending}
              onClick={() => start.mutate(spec.params(form.values) ?? {})}
            >
              {spec.action ?? 'Start'}
            </Button>
          </div>
        </>
      )}

      {start.error && <Refusal error={start.error} />}
      {stop.error && <Refusal error={stop.error} />}

      {listening && (
        <div className={resultStyles.block}>
          <h4 className={resultStyles.blockTitle}>Messages in memory</h4>
          <Table
            columns={MESSAGE_COLUMNS}
            rows={(messages.data?.messages ?? []).map((message, index) => ({
              ...message,
              __key: String(index),
            }))}
            rowKey={(row) => row.__key}
            aria-label="Syslog messages received"
            density="compact"
            maxHeight={360}
            loading={messages.isPending}
            empty={
              <p className={resultStyles.empty}>
                Nothing has arrived yet. Point a device at this address and its messages appear here.
              </p>
            }
          />
          {messages.data && (
            <p className={resultStyles.tableNote}>
              {formatCount(messages.data.total)} in memory
              {messages.data.total > messages.data.messages.length
                ? `, of which the newest ${messages.data.messages.length} are shown`
                : ''}
              .
            </p>
          )}
        </div>
      )}
    </>
  );
}

function standingSyslogFacts(status: SyslogStatus | undefined) {
  if (status?.listening) {
    return [
      { label: 'UDP', value: status.udp_address ?? '', mono: true, copy: true },
      ...(status.tcp_address
        ? [{ label: 'TCP', value: status.tcp_address, mono: true, copy: true }]
        : [{ label: 'TCP', value: 'not listening', tone: 'idle' as const }]),
      { label: 'Messages', value: formatCount(status.received ?? 0), mono: true },
      ...(status.senders && status.senders.length > 0
        ? [
            {
              label: 'Senders',
              value: status.senders
                .map((sender) => `${sender.address}${sender.device ? ` (${sender.device})` : ''} ×${sender.messages}`)
                .join(', '),
              mono: true,
            },
          ]
        : []),
    ];
  }
  return [
    {
      label: 'Defaults',
      value: `${status?.defaults.bind ?? '—'}:${status?.defaults.port ?? '—'}${
        status?.defaults.tcp ? ' (UDP and TCP)' : ' (UDP)'
      }`,
      mono: true,
    },
    {
      label: 'Keeps',
      value: `${formatCount(status?.defaults.keep_messages)} messages in memory`,
    },
  ];
}

/** The standing state of a server: a pill, then the facts about it. */
function Standing({
  running,
  word,
  facts,
}: {
  running: boolean;
  word: string;
  facts: Parameters<typeof Facts>[0]['facts'];
}) {
  return (
    <div className={styles.standing} data-running={running || undefined}>
      <div className={styles.standingHead}>
        <StatusPill tone={running ? 'ok' : 'idle'}>{word}</StatusPill>
        {running && <span className={styles.standingNote}>It keeps running when you leave this screen.</span>}
      </div>
      <Facts facts={facts} />
    </div>
  );
}
