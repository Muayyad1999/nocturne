/**
 * Session logs: what devices printed into sessions that were being logged,
 * read back a page at a time.
 *
 * A log on disk is what the device printed, like a stored configuration, so
 * it is read redacted. Reading it as it was written is possible and is not
 * a setting: the person asks for it, is told before the read is made that
 * the engine records it in the audit trail, and it lasts for the log that
 * is open and no longer.
 *
 * What is on a page is device output: it is shown as text, in a `<pre>`,
 * and never written into a terminal.
 */

import { ArrowLeft, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, Eye, EyeOff, FileCheck2, FileText, Trash } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { rpc } from '@/api/client';
import { useHasMethod, useRpc } from '@/api/hooks';
import type {
  SessionLogInfo,
  TerminalLogsDeleteResult,
  TerminalLogsReadResult,
} from '@/api/types/terminal';
import { useKeepAsEvidence } from '@/app/KeepAsEvidence';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { formatBytes, plural } from '@/lib/format';
import type { InventoryDevice } from './inventory';
import {
  captureRange,
  hasAfter,
  hasBefore,
  listCoverage,
  logDevice,
  logPeriod,
  pageLines,
  pageRange,
  readAt,
  readBefore,
  type ReadLog,
} from './logs';
import styles from './LogsDrawer.module.css';
import { useFocusWhenShown } from './viewFocus';

/** Logs asked for at first, and how many more each "Show more" asks for. The engine gives at most 1000. */
const LIST_STEP = 100;
const LIST_MOST = 1000;
/** How long the search waits after a key before asking the engine. */
const SEARCH_AFTER_MS = 250;

const read: ReadLog = (params) => rpc<TerminalLogsReadResult>('terminal.logs.read', params);

export interface LogsDrawerProps {
  open: boolean;
  onClose: () => void;
  /** The inventory, to name the device a log is of. Undefined while it has not arrived. */
  devices: readonly InventoryDevice[] | undefined;
}

export function LogsDrawer({ open, onClose, devices }: LogsDrawerProps) {
  const [query, setQuery] = useState('');
  const [asked, setAsked] = useState('');
  const [deviceId, setDeviceId] = useState('');
  const [limit, setLimit] = useState(LIST_STEP);
  const [reading, setReading] = useState<SessionLogInfo | null>(null);
  /** The log last opened: the list gives its row the focus when it is shown again. */
  const [lastRead, setLastRead] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  useFocusWhenShown(listRef, reading ? 'reader' : 'list');

  useEffect(() => {
    const timer = setTimeout(() => setAsked(query.trim()), SEARCH_AFTER_MS);
    return () => clearTimeout(timer);
  }, [query]);

  const filtered = asked !== '' || deviceId !== '';
  const logs = useRpc<SessionLogInfo[]>(
    'terminal.logs.list',
    {
      limit,
      ...(asked === '' ? {} : { query: asked }),
      ...(deviceId === '' ? {} : { device_id: deviceId }),
    },
    {
      enabled: open,
      // A log appears when a session starts being logged and grows while it runs.
      invalidateOn: ['terminal.logs.changed', 'terminal.opened', 'terminal.changed', 'terminal.closed'],
    },
  );

  const close = () => {
    setReading(null);
    setLastRead(null);
    onClose();
  };

  const rows = logs.data ?? [];

  return (
    <Drawer
      open={open}
      onClose={close}
      size="lg"
      title="Session logs"
      description="What devices printed into sessions that were being logged. Read back with secrets hidden."
    >
      {reading ? (
        <LogReader
          key={reading.name}
          log={reading}
          device={logDevice(reading, devices)}
          onBack={() => setReading(null)}
          onDeleted={() => {
            setReading(null);
            void logs.refetch();
          }}
        />
      ) : (
        <div ref={listRef} className={styles.body}>
          <div className={styles.filters}>
            <SearchInput
              aria-label="Search logs by title or file name"
              placeholder="Title or file name"
              value={query}
              onChange={setQuery}
            />
            <Select
              aria-label="Device"
              value={deviceId}
              onChange={(event) => setDeviceId(event.target.value)}
              className={styles.device}
            >
              <option value="">Every device</option>
              {(devices ?? []).map((device) => (
                <option key={device.id} value={device.id}>
                  {device.name}
                </option>
              ))}
            </Select>
          </div>

          {logs.isError ? (
            <ErrorState
              size="compact"
              title="The logs could not be listed"
              error={logs.error}
              onRetry={() => void logs.refetch()}
            />
          ) : logs.isPending ? (
            <>
              <Skeleton height={48} />
              <Skeleton height={48} />
            </>
          ) : rows.length === 0 ? (
            <EmptyState
              size="compact"
              icon={FileText}
              title={filtered ? 'No log matches' : 'No session has been logged'}
              description={
                filtered
                  ? 'Nothing matches the words and the device that are picked.'
                  : '“Log session” on the toolbar writes what a session prints to a file from then on; the setting terminal.log_sessions logs every session.'
              }
            />
          ) : (
            <>
              <ul className={styles.rows} aria-label="Session logs">
                {rows.map((log) => (
                  <li key={log.name}>
                    <button
                      type="button"
                      className={styles.row}
                      data-autofocus={log.name === lastRead || undefined}
                      onClick={() => {
                        setLastRead(log.name);
                        setReading(log);
                      }}
                    >
                      <span className={styles.rowTitle}>
                        {log.title}
                        <Badge variant="outline">{log.kind}</Badge>
                        {log.active && <Badge tone="info">being written</Badge>}
                      </span>
                      <span className={styles.rowFacts}>
                        {logDevice(log, devices)} · {logPeriod(log)} · {formatBytes(log.size)}
                      </span>
                      <span className={styles.rowName}>{log.name}</span>
                    </button>
                  </li>
                ))}
              </ul>
              <p className={styles.note} data-testid="logs-coverage">
                {listCoverage(rows.length, limit, filtered)}
              </p>
              {rows.length >= limit && limit < LIST_MOST && (
                <div>
                  <Button size="sm" loading={logs.isFetching} onClick={() => setLimit(Math.min(LIST_MOST, limit + LIST_STEP))}>
                    Show older logs
                  </Button>
                </div>
              )}
              {rows.length >= LIST_MOST && (
                <p className={styles.note}>
                  That is as many as one list holds. Narrow it with words or a device to reach older logs.
                </p>
              )}
            </>
          )}
        </div>
      )}
    </Drawer>
  );
}

// ------------------------------------------------------------- one log

interface LogReaderProps {
  log: SessionLogInfo;
  device: string;
  onBack: () => void;
  onDeleted: () => void;
}

type Page =
  | { state: 'reading' }
  | { state: 'read'; page: TerminalLogsReadResult }
  | { state: 'failed'; why: string };

function LogReader({ log, device, onBack, onDeleted }: LogReaderProps) {
  const [page, setPage] = useState<Page>({ state: 'reading' });
  /** Read as written, for this log, since the person asked. */
  const [raw, setRaw] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [refused, setRefused] = useState<string | null>(null);
  /** Only the latest request may put its answer on screen. */
  const request = useRef(0);
  const rootRef = useRef<HTMLDivElement>(null);
  useFocusWhenShown(rootRef);

  const canKeep = useHasMethod('evidence.capture_log') === true;
  const canDelete = useHasMethod('terminal.logs.delete') === true;
  const keep = useKeepAsEvidence();

  const show = (reading: Promise<TerminalLogsReadResult>) => {
    request.current += 1;
    const mine = request.current;
    setPage({ state: 'reading' });
    reading.then(
      (answer) => {
        if (request.current === mine) setPage({ state: 'read', page: answer });
      },
      (error: unknown) => {
        if (request.current !== mine) return;
        setPage({ state: 'failed', why: error instanceof Error ? error.message : 'The log could not be read.' });
      },
    );
  };

  // The log opens at its start, redacted.
  useEffect(() => {
    show(readAt(read, log.name, 0, false));
    // Only when another log is opened: the reader is keyed by the log's name.
  }, [log.name]); // eslint-disable-line react-hooks/exhaustive-deps

  const current = page.state === 'read' ? page.page : null;

  const reveal = async () => {
    if (!current) return;
    const confirmed = await confirm({
      title: `Read ${log.title} without redaction?`,
      description:
        'Passwords, keys and community strings in this log will be on screen as the device printed them. The engine records every page read this way in the audit trail — which log and which bytes, never the text — starting with this page.',
      confirmLabel: 'Show unredacted',
    });
    if (!confirmed) return;
    setRaw(true);
    show(readAt(read, log.name, current.offset, true));
  };

  const hide = () => {
    setRaw(false);
    if (current) show(readAt(read, log.name, current.offset, false));
  };

  const remove = async () => {
    const confirmed = await confirm({
      title: `Delete the log of ${log.title}?`,
      description: `${log.name} is removed from the disk. Evidence that was kept from it stays.`,
      confirmLabel: 'Delete log',
      tone: 'danger',
    });
    if (!confirmed) return;
    setDeleting(true);
    setRefused(null);
    try {
      const result = await rpc<TerminalLogsDeleteResult>('terminal.logs.delete', { names: [log.name] });
      const failure = result.failed.find((entry) => entry.name === log.name) ?? result.failed[0];
      if (result.deleted === 0 || failure) {
        setRefused(failure?.message ?? 'The engine did not delete it and did not say why.');
        return;
      }
      toast.success('Log deleted', { description: log.name });
      onDeleted();
    } catch (error) {
      setRefused(error instanceof Error ? error.message : 'The log could not be deleted.');
    } finally {
      setDeleting(false);
    }
  };

  const range = current ? captureRange(current) : null;

  return (
    <div ref={rootRef} className={styles.reader} data-testid="log-reader">
      <div className={styles.readerHead}>
        <Button size="sm" variant="ghost" icon={ArrowLeft} onClick={onBack}>
          All logs
        </Button>
        <span className={styles.spacer} />
        {canKeep && keep.can('log') && (
          <Button
            size="sm"
            icon={FileCheck2}
            disabled={!range || 'why' in range}
            title={range && 'why' in range ? range.why : 'The engine reads this page again, redacted, and keeps it'}
            onClick={() => {
              if (!range || 'why' in range) return;
              keep.keep({ kind: 'log', name: log.name, offset: range.offset, limit: range.limit, log: log.title });
            }}
          >
            Keep these lines as evidence…
          </Button>
        )}
        {raw ? (
          <Button size="sm" icon={EyeOff} onClick={hide}>
            Hide secrets again
          </Button>
        ) : (
          <Button size="sm" icon={Eye} disabled={!current} onClick={() => void reveal()}>
            Show unredacted…
          </Button>
        )}
        {canDelete && (
          <IconButton
            icon={Trash}
            size="sm"
            aria-label={`Delete the log of ${log.title}`}
            tooltip="Delete this log"
            loading={deleting}
            onClick={() => void remove()}
          />
        )}
      </div>

      <h3 className={styles.readerTitle}>
        {log.title}
        <Badge variant="outline">{log.kind}</Badge>
        {log.active && <Badge tone="info">being written</Badge>}
      </h3>
      <p className={styles.note}>
        {device} · {logPeriod(log)} · {log.name}
      </p>

      {refused && (
        <p className={styles.refused} role="alert" data-testid="log-refused">
          <b>The log was not deleted.</b> {refused}
        </p>
      )}

      {raw && (
        <p className={styles.raw} role="status" data-testid="log-raw">
          Unredacted. Every page read this way is recorded in the audit trail.
        </p>
      )}

      {page.state === 'failed' ? (
        <ErrorState
          size="compact"
          title="The log could not be read"
          error={page.why}
          onRetry={() => show(readAt(read, log.name, 0, raw))}
        />
      ) : page.state === 'reading' ? (
        <Skeleton shape="block" height={320} />
      ) : (
        <>
          <div className={styles.pager}>
            <span
              className={styles.range}
              data-testid="log-range"
              data-offset={page.page.offset}
              data-next={page.page.next_offset}
            >
              {pageRange(page.page)}
              {page.page.text !== '' && ` · ${plural(pageLines(page.page.text), 'line')}`}
            </span>
            <span className={styles.spacer} />
            <IconButton
              icon={ChevronsLeft}
              size="sm"
              aria-label="The start of the log"
              disabled={!hasBefore(page.page)}
              onClick={() => show(readAt(read, log.name, 0, raw))}
            />
            <IconButton
              icon={ChevronLeft}
              size="sm"
              aria-label="The page before"
              disabled={!hasBefore(page.page)}
              onClick={() => show(readBefore(read, log.name, page.page.offset, raw))}
            />
            <IconButton
              icon={ChevronRight}
              size="sm"
              aria-label="The page after"
              disabled={!hasAfter(page.page)}
              onClick={() => show(readAt(read, log.name, page.page.next_offset, raw))}
            />
            <IconButton
              icon={ChevronsRight}
              size="sm"
              aria-label="The end of the log"
              disabled={!hasAfter(page.page)}
              onClick={() => show(readBefore(read, log.name, page.page.size, raw))}
            />
          </div>
          {page.page.text === '' ? (
            <p className={styles.note}>There is nothing on this page.</p>
          ) : (
            // No copy button on a page that was read as written: what was
            // revealed leaves by select-and-copy, as a revealed configuration does.
            <CodeBlock code={page.page.text} copyable={!raw} maxHeight="calc(100vh - 360px)" />
          )}
          {!raw && (
            <p className={styles.note}>
              {page.page.redacted
                ? 'Secrets on this page are replaced.'
                : 'The engine found nothing to replace on this page.'}
            </p>
          )}
        </>
      )}
      {keep.dialog}
    </div>
  );
}
