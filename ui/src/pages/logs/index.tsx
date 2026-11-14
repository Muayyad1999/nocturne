/**
 * Logs — what the network says about itself.
 *
 * Everything on screen comes from `nocturne-logs` (`docs/dev/features/logs.md`,
 * types in `src/api/types/logs.ts`): `logs.query` for the lines,
 * `logs.histogram` for the shape of them, `logs.follow` for the live tail,
 * `logs.status` for what is listening and what has arrived, and
 * `logs.device_setup` for the lines that would point a device here. Nothing is
 * polled: the status follows `logs.status_changed` and the counts follow
 * `logs.appended`.
 *
 * Logs is the Monitoring screen's **Logs** tab (`spec-ui-pages.md` heads that
 * section `monitor.md`, `logs.md`, `insights.md`), so the component that
 * matters here is {@link Logs}: a plain component with no props, no router of
 * its own, no `Page` and no page header, which renders inside another screen's
 * tab panel. Its view lives in search parameters — `q`, `sev`, `stream`, `dev`,
 * `span`, `from`/`to`, `tail`, `entry` — none of which Monitoring or Insights
 * use, so a shared link opens the same search.
 *
 * The default export mounts the same component as a standalone screen with its
 * own `Page` and title. That is what the Playwright specs drive, and what a
 * route would render if Logs ever gets one.
 *
 * Two rules from CLAUDE.md shape it. **A gap is a gap**: an empty histogram
 * bucket is only drawn as a zero where the store and the receiver can prove it
 * was one, and a severity filter says out loud how much it is hiding.
 * **Nothing arriving is not nothing happening**: when the receiver is off, has
 * failed to bind, or has never been sent a line, the screen says which of those
 * it is and shows the exact commands for a real device instead of a shrug.
 */

import { Radio, RefreshCw, ScrollText, X } from 'lucide-react';
import { useCallback, useEffect, useEffectEvent, useMemo, useRef, useState } from 'react';
import { useSearchParams } from 'react-router';
import { useHasMethod } from '@/api/hooks';
import type { LogEntry } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Switch } from '@/design/Switch';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { formatCount, plural } from '@/lib/format';
import { formatTime, formatTimestamp } from '@/lib/time';
import { emptyReason, histogramView, receiverVerdict, windowOf } from './coverage';
import { useDevices, useHistogram, useLogSources, useLogsStatus } from './data';
import { EntryDrawer } from './EntryDrawer';
import { Histogram } from './Histogram';
import { LogList } from './LogList';
import styles from './Logs.module.css';
import {
  DEFAULT_SPAN,
  SEVERITY_CHOICES,
  SPANS,
  STREAMS,
  filterFrom,
  isNarrowed,
  parseQuery,
  severityPhrase,
  span as spanOf,
  withoutSeverity,
} from './query';
import { SetupHint, SourcesPanel } from './SetupHint';
import { useLogStream } from './useStream';

/**
 * What this part of the application is called, for the standalone page title
 * and the tab that mounts it. Logs is not a nav destination of its own.
 */
export const AREA = { label: 'Logs', icon: ScrollText } as const;

/** How often the time range is re-anchored while the tail is running. */
const REANCHOR_MS = 60_000;

/**
 * The screen's body, for mounting inside another screen's tab panel. No props:
 * everything it needs is RPC, and the search it is showing is in the URL.
 */
export function Logs() {
  const installed = useHasMethod('logs.query');
  // `undefined` is "the method list has not answered yet". Waiting for proof
  // is right here: a screen that flashes "not in this build" and then draws
  // itself is worse than a skeleton for one round trip.
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <LogsBody />;
}

/** Logs as a screen of its own: the same component, with a page around it. */
export default function LogsPage() {
  usePageHeader({ title: AREA.label });
  return (
    <Page>
      <Logs />
    </Page>
  );
}

function Loading() {
  return (
    <div className={styles.screen}>
      <Skeleton shape="block" height={32} width={520} />
      <Skeleton shape="block" height={120} />
      <Skeleton shape="block" height={320} />
    </div>
  );
}

function NotInstalled() {
  return (
    <EmptyState
      icon={AREA.icon}
      title="Log collection is not in this build"
      description="This copy of Nocturne was assembled without the logs feature, so no syslog is received, stored or searched. The rest of the application is unaffected."
    />
  );
}

function LogsBody() {
  const [search, setSearch] = useSearchParams();

  // --------------------------------------------------------------- the view
  const queryParam = search.get('q') ?? '';
  const stream = STREAMS.some((entry) => entry.value === search.get('stream'))
    ? (search.get('stream') as string)
    : 'syslog';
  const severityParam = search.get('sev');
  const maxSeverity =
    severityParam !== null && /^[0-7]$/.test(severityParam) ? Number(severityParam) : undefined;
  const deviceId = search.get('dev');
  const spanId = search.get('span') ?? DEFAULT_SPAN;
  const from = Number(search.get('from'));
  const to = Number(search.get('to'));
  const absolute = Number.isFinite(from) && from > 0 && Number.isFinite(to) && to > from;
  const tail = search.get('tail') === '1';

  const update = useEffectEvent((changes: Record<string, string | null>) => {
    const next = new URLSearchParams(search);
    for (const [key, value] of Object.entries(changes)) {
      if (value === null || value === '') next.delete(key);
      else next.set(key, value);
    }
    setSearch(next, { replace: true });
  });

  // The search box types faster than a database answers, so the URL and the
  // query follow it a beat behind.
  const [typed, setTyped] = useState(queryParam);
  const typedRef = useRef(queryParam);
  useEffect(() => {
    if (typed === typedRef.current) return;
    const timer = setTimeout(() => {
      typedRef.current = typed;
      update({ q: typed });
    }, 300);
    return () => clearTimeout(timer);
  }, [typed]);
  // A query changed from outside (a link, the back button) wins.
  useEffect(() => {
    if (queryParam !== typedRef.current) {
      typedRef.current = queryParam;
      setTyped(queryParam);
    }
  }, [queryParam]);

  // The time range. A fixed anchor keeps the histogram's buckets still while
  // lines arrive; the counts in them are refreshed by `logs.appended`.
  const [anchor, setAnchor] = useState(() => Date.now());
  useEffect(() => {
    if (!tail) return;
    const timer = setInterval(() => setAnchor(Date.now()), REANCHOR_MS);
    return () => clearInterval(timer);
  }, [tail]);
  const chosenSpan = spanOf(spanId);
  // The range ends at the end of the bucket that is running, not at the
  // instant the screen was opened: a line that arrives a second later belongs
  // in this picture, and a bucket that has not happened yet is drawn as "not
  // yet" rather than as a quiet minute.
  const width = Math.max(1_000, Math.round(chosenSpan.ms / chosenSpan.buckets));
  const since = absolute ? from : Math.ceil(anchor / width) * width - chosenSpan.ms;
  const until = absolute ? to : Math.ceil(anchor / width) * width;
  const spanMs = until - since;

  // ---------------------------------------------------------------- the data
  const status = useLogsStatus();
  const devices = useDevices();
  // The list and the live tail are deliberately **open-ended** on a relative
  // range. `logs.follow` applies this same filter to every line as it
  // arrives, so an `until` of "when the screen was opened" would be a tail
  // that silently stops delivering; and a search for "the last hour" means
  // up to now, whenever now is.
  const filter = useMemo(
    () =>
      filterFrom({
        query: queryParam,
        stream,
        deviceId,
        maxSeverity,
        since,
        ...(absolute ? { until } : {}),
      }),
    [queryParam, stream, deviceId, maxSeverity, since, until, absolute],
  );
  const counted = useMemo(
    () => withoutSeverity(filterFrom({ query: queryParam, stream, deviceId, maxSeverity: undefined, since, until })),
    [queryParam, stream, deviceId, since, until],
  );
  const histogram = useHistogram(counted, since, until, chosenSpan.buckets, true);
  const logs = useLogStream(filter, tail, true);
  const sources = useLogSources((status.data?.unattributed_sources ?? 0) > 0);

  // `until` is this view's "now": a bucket beyond the range does not exist,
  // and anchoring the judgement to the range keeps it stable between renders.
  const cover = useMemo(() => windowOf(status.data, until), [status.data, until]);
  const view = useMemo(
    () => histogramView(histogram.data ?? [], maxSeverity, cover),
    [histogram.data, maxSeverity, cover],
  );
  const statusData = status.data;
  const verdict = statusData ? receiverVerdict(statusData) : null;

  const byId = useMemo(
    () => new Map((devices.data ?? []).map((device) => [device.id, device])),
    [devices.data],
  );
  // Called while rendering a row, so a plain callback and not an effect event.
  const nameOf = useCallback(
    (id: string | null | undefined) => (id == null ? '' : (byId.get(id)?.name ?? id)),
    [byId],
  );

  const [openEntry, setOpenEntry] = useState<LogEntry | null>(null);
  const [pinned, setPinned] = useState(true);

  // The one line about logging that is on screen whatever tab is open.
  useStatusItem(
    status.data && verdict
      ? {
          id: 'logs.receiver',
          order: 40,
          label:
            verdict.kind === 'receiving'
              ? `${formatCount(status.data.syslog.rate_per_minute)} lines/min`
              : verdict.kind === 'off'
                ? 'Syslog off'
                : verdict.kind === 'failed'
                  ? 'Syslog not listening'
                  : 'No syslog yet',
          tone:
            verdict.kind === 'receiving'
              ? 'ok'
              : verdict.kind === 'failed'
                ? 'crit'
                : verdict.kind === 'off'
                  ? 'idle'
                  : 'warn',
          detail: `${verdict.title}. ${plural(status.data.store.rows, 'line')} stored${
            status.data.syslog.dropped > 0
              ? `, ${formatCount(status.data.syslog.dropped)} dropped under load since the start`
              : ''
          }.`,
        }
      : null,
  );

  const reload = useEffectEvent(() => {
    setAnchor(Date.now());
    logs.refresh();
  });

  /**
   * Switching the tail on means "show me what is happening now", so it reads
   * the newest page again first: a follow only ever delivers lines that
   * arrive *after* it was opened, and starting a live tail on top of a list
   * that was read ten minutes ago would leave a hole nobody could see.
   */
  const setTail = useEffectEvent((on: boolean) => {
    update({ tail: on ? '1' : null });
    if (on) reload();
  });

  const toggleTail = useEffectEvent(() => setTail(!tail));
  useEffect(
    () =>
      registerCommand({
        id: 'logs.tail',
        title: 'Follow the logs live',
        group: AREA.label,
        keywords: ['syslog', 'tail', 'follow', 'live', 'logs'],
        icon: Radio,
        run: () => toggleTail(),
      }),
    [],
  );

  const parsed = parseQuery(queryParam);
  const narrowed = isNarrowed(filter);
  const reason = verdict
    ? emptyReason(verdict, { narrowed, rangeTotal: view.counts.total })
    : 'range';

  const pick = (pickedFrom: number, pickedUntil: number) =>
    update({ from: String(Math.round(pickedFrom)), to: String(Math.round(pickedUntil)), span: null });

  return (
    <div className={styles.screen}>
      <Toolbar className={styles.toolbar}>
        <SearchInput
          className={styles.search}
          value={typed}
          onChange={setTyped}
          aria-label="Search the logs"
          placeholder="Words, &quot;a phrase&quot;, -not this, if:Gi1/0/3"
        />
        <Select
          className={styles.filterControl}
          size="sm"
          aria-label="Severity"
          value={maxSeverity === undefined ? '' : String(maxSeverity)}
          onChange={(event) => update({ sev: event.target.value })}
        >
          <option value="">Every severity</option>
          {SEVERITY_CHOICES.map((choice) => (
            <option key={choice.value} value={choice.value}>
              {choice.label}
            </option>
          ))}
        </Select>
        <Select
          className={styles.filterControl}
          size="sm"
          aria-label="Stream"
          value={stream}
          options={STREAMS}
          onChange={(event) => update({ stream: event.target.value })}
        />
        <Select
          className={styles.filterControl}
          size="sm"
          aria-label="Device"
          value={deviceId ?? ''}
          onChange={(event) => update({ dev: event.target.value })}
        >
          <option value="">Every device</option>
          {(devices.data ?? []).map((device) => (
            <option key={device.id} value={device.id}>
              {device.name}
            </option>
          ))}
        </Select>
        <Select
          className={styles.filterControl}
          size="sm"
          aria-label="Time range"
          value={absolute ? 'custom' : chosenSpan.id}
          onChange={(event) => update({ span: event.target.value, from: null, to: null })}
        >
          {absolute && <option value="custom">{`${formatTime(since)} – ${formatTime(until)}`}</option>}
          {SPANS.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.label}
            </option>
          ))}
        </Select>
        <ToolbarSpacer />
        <Switch
          checked={tail}
          onChange={(event) => setTail(event.target.checked)}
          className={styles.tailSwitch}
        >
          Live tail
        </Switch>
        <IconButton
          icon={RefreshCw}
          size="sm"
          aria-label="Read the newest lines again"
          loading={logs.loading}
          onClick={reload}
        />
      </Toolbar>

      {(parsed.unknown.length > 0 || absolute || tail) && (
        <div className={styles.chips}>
          {tail && (
            <Badge tone={logs.following ? 'ok' : 'warn'} variant="outline">
              {logs.following
                ? pinned
                  ? 'Following the newest lines'
                  : 'Tail running — scrolled back, so it is not jumping'
                : (logs.followError ?? 'Opening the live tail…')}
            </Badge>
          )}
          {absolute && (
            <Badge tone="accent" variant="outline">
              <button
                type="button"
                className={styles.chipClear}
                onClick={() => update({ from: null, to: null, span: DEFAULT_SPAN })}
                title={`${formatTimestamp(since)} to ${formatTimestamp(until)}`}
              >
                Zoomed to {formatTime(since)} – {formatTime(until)} <X size={11} aria-hidden="true" />
              </button>
            </Badge>
          )}
          {parsed.unknown.map((unknown) => (
            <Badge key={unknown} tone="warn" variant="outline">
              <code>{unknown}</code> is not a field — the fields are mnemonic, if, from, facility and
              is:unattributed
            </Badge>
          ))}
        </div>
      )}

      {status.isError ? (
        <ErrorState
          size="compact"
          title="The log receiver could not be reached"
          error={status.error}
          onRetry={() => void status.refetch()}
        />
      ) : verdict?.setup && statusData ? (
        <SetupHint
          verdict={verdict}
          status={statusData}
          devices={devices.data ?? []}
          deviceId={deviceId}
        />
      ) : null}

      {sources.data && <SourcesPanel sources={sources.data} onPick={(address) => setTyped(`from:${address}`)} />}

      {histogram.isError ? (
        <ErrorState
          size="compact"
          title="The histogram could not be read"
          error={histogram.error}
          onRetry={() => void histogram.refetch()}
        />
      ) : (
        <Histogram
          view={view}
          window={cover}
          since={since}
          until={until}
          spanMs={spanMs}
          maxSeverity={maxSeverity}
          loading={histogram.isPending}
          onPick={pick}
        />
      )}

      {logs.behind && (
        <div className={styles.behind}>
          <span>New lines have arrived. This list is the search as it was read.</span>
          <Button size="sm" icon={RefreshCw} onClick={reload}>
            Read it again
          </Button>
        </div>
      )}

      {logs.error ? (
        <ErrorState
          title="The lines could not be read"
          error={logs.error}
          onRetry={logs.retry}
          size="compact"
        />
      ) : logs.loading ? (
        <Skeleton shape="block" height={320} />
      ) : (
        <LogList
          stream={logs.stream}
          tail={tail}
          nameOf={nameOf}
          openId={openEntry?.id ?? null}
          onOpen={setOpenEntry}
          hasOlder={logs.hasOlder}
          loadingOlder={logs.loadingOlder}
          loadOlder={logs.loadOlder}
          onPinnedChange={setPinned}
          empty={
            reason === 'setup' ? (
              <EmptyState
                size="compact"
                icon={AREA.icon}
                title="No line has arrived yet"
                description="Nothing is wrong with the search: nothing has been sent here. The panel above has the exact commands for one of your devices, and can read a device's own buffer instead."
              />
            ) : reason === 'filter' ? (
              <EmptyState
                size="compact"
                icon={AREA.icon}
                title="No line matches this search"
                description={
                  maxSeverity !== undefined && view.counts.total > 0
                    ? `This range holds ${plural(view.counts.total, 'line')}; none of them is ${severityPhrase(maxSeverity)}.`
                    : 'The range holds lines, but none that match. Widen the search or clear a filter.'
                }
                action={
                  <Button
                    onClick={() =>
                      update({ q: null, sev: null, dev: null, stream: null, from: null, to: null })
                    }
                  >
                    Clear the filters
                  </Button>
                }
              />
            ) : (
              <EmptyState
                size="compact"
                icon={AREA.icon}
                title="No lines in this range"
                description={`Nothing was stored between ${formatTime(since)} and ${formatTime(until)}. A wider range may have what you are looking for.`}
                action={<Button onClick={() => update({ span: '7d', from: null, to: null })}>Look back a week</Button>}
              />
            )
          }
        />
      )}

      <EntryDrawer
        entry={openEntry}
        deviceName={openEntry?.device_id == null ? null : nameOf(openEntry.device_id)}
        onClose={() => setOpenEntry(null)}
        onContext={(entry) => {
          setOpenEntry(null);
          update({ tail: null });
          pick(entry.ts - 30_000, entry.ts + 30_000);
        }}
      />
    </div>
  );
}
