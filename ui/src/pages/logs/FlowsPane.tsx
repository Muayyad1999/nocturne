/**
 * Flows: who is talking, as the flow collector has it in memory.
 *
 * `flows.top` adds up the one-minute buckets of a chosen period — over every
 * exporter, the exporters of one device, or one exporter by its address —
 * under one of six keys. No flow is stored anywhere, so the answer is all
 * there is, and the pane's job is to keep it from being read as more than
 * it is (`flows.ts`):
 *
 * * the **coverage** stands beside the table, not under a fold: whether the
 *   collector is listening, since when the window holds anything, how many
 *   records, how many data sets could not be read for want of a template;
 * * a **sampled** exporter's figures are labelled sampled, with the rate it
 *   said, and are **not multiplied** — the bytes on screen are the bytes the
 *   exporter reported;
 * * `other` and `total` are shown as the engine gives them: `total` exact,
 *   each row a floor, `other` what is under no key.
 *
 * The collector publishes nothing per flow, so the pane asks again every
 * thirty seconds and on demand. Nothing here has met a router or a switch,
 * and the pane says that too.
 */

import { RefreshCw, Waves } from 'lucide-react';
import { useSearchParams } from 'react-router';
import type { FlowExporter, FlowRow, LogsStatus } from '@/api/types/logs';
import { Badge } from '@/design/Badge';
import { IconButton } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatBytes, formatCount, formatPercent, plural } from '@/lib/format';
import { useFlowExporters, useFlowTop } from './data';
import {
  FLOW_BYS,
  FLOW_PERIODS,
  NOT_VERIFIED,
  coverageLines,
  emptyKind,
  hasOther,
  isFlowBy,
  parseScope,
  periodOf,
  ratesPhrase,
  samplingNote,
  scopeChoices,
  scopeValue,
  shareOf,
  topParams,
  versionLabel,
} from './flows';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';

export interface FlowsPaneProps {
  capabilities: Capabilities;
  status: LogsStatus | undefined;
}

export function FlowsPane({ capabilities, status }: FlowsPaneProps) {
  const [search, setSearch] = useSearchParams();
  const byParam = search.get('by');
  const by = isFlowBy(byParam) ? byParam : 'source';
  const minutes = periodOf(search.get('period'));
  const scope = parseScope(search.get('scope'));

  const update = (changes: Record<string, string | null>) => {
    const next = new URLSearchParams(search);
    for (const [key, value] of Object.entries(changes)) {
      if (value === null || value === '') next.delete(key);
      else next.set(key, value);
    }
    setSearch(next, { replace: true });
  };

  const top = useFlowTop(topParams(by, minutes, scope));
  const exporters = useFlowExporters(capabilities.exporters);
  const way = FLOW_BYS.find((entry) => entry.value === by) ?? FLOW_BYS[0];
  const choices = scopeChoices(exporters.data ?? []);
  // A scope from a link whose exporter has since gone quiet is still a
  // scope: it stays selectable rather than silently becoming "everything".
  const value = scopeValue(scope);
  if (value !== '' && !choices.some((choice) => choice.value === value)) {
    choices.push({ value, label: scope.kind === 'exporter' ? scope.address : value.slice(7) });
  }

  const data = top.data;
  const sampling = data ? samplingNote(data.coverage) : null;
  const empty = data ? emptyKind(data) : null;
  const flows = status?.flows;

  // A sortable header takes 52 px beside its text (padding, its sort control
  // and the arrow); the numeric widths are their headers' and no more, so the
  // key column keeps the rest in a narrow window.
  const columns: readonly TableColumn<FlowRow>[] = [
    {
      id: 'label',
      header: way?.column ?? 'Key',
      required: true,
      // Addresses and names an exporter reported: text, in the monospace face.
      cell: (row) => <span title={row.key}>{row.label}</span>,
      mono: true,
      sortValue: (row) => row.label,
    },
    {
      id: 'bytes',
      header: sampling ? 'Bytes, as exported' : 'Bytes',
      align: 'right',
      width: 164,
      cell: (row) => formatBytes(row.bytes),
      sortValue: (row) => row.bytes,
    },
    {
      id: 'share',
      header: 'Share',
      align: 'right',
      width: 88,
      cell: (row) => {
        const share = data ? shareOf(row, data.total, by) : null;
        return share === null ? (
          <span className={styles.muted} title="By interface every flow is counted twice, so a row is not a part of the total.">
            n/a
          </span>
        ) : (
          formatPercent(share, { fromRatio: true })
        );
      },
      sortValue: (row) => row.bytes,
    },
    {
      id: 'packets',
      header: 'Packets',
      align: 'right',
      width: 100,
      cell: (row) => formatCount(row.packets),
      sortValue: (row) => row.packets,
    },
    {
      id: 'flows',
      header: 'Flows',
      align: 'right',
      width: 88,
      cell: (row) => formatCount(row.flows),
      sortValue: (row) => row.flows,
    },
  ];

  return (
    <div className={styles.paneBody}>
      <p className={styles.caution} data-testid="flows-not-verified">
        {NOT_VERIFIED}
      </p>

      <Toolbar className={styles.toolbar}>
        <Select
          className={styles.filterControl}
          size="sm"
          aria-label="Add up by"
          value={by}
          onChange={(event) => update({ by: event.target.value })}
        >
          {FLOW_BYS.map((entry) => (
            <option key={entry.value} value={entry.value}>
              By {entry.label.toLowerCase()}
            </option>
          ))}
        </Select>
        <Select
          className={styles.filterControl}
          size="sm"
          aria-label="Period"
          value={String(minutes)}
          onChange={(event) => update({ period: event.target.value })}
        >
          {FLOW_PERIODS.map((period) => (
            <option key={period.minutes} value={String(period.minutes)}>
              {period.label}
            </option>
          ))}
        </Select>
        <Select
          className={styles.filterControlWide}
          size="sm"
          aria-label="Exporter or device"
          value={value}
          onChange={(event) => update({ scope: event.target.value })}
        >
          {choices.map((choice) => (
            <option key={choice.value} value={choice.value}>
              {choice.label}
            </option>
          ))}
        </Select>
        {sampling && (
          <Badge tone="warn" variant="outline" data-testid="flows-sampled">
            {sampling.label} — not multiplied
          </Badge>
        )}
        <ToolbarSpacer />
        <IconButton
          icon={RefreshCw}
          size="sm"
          aria-label="Ask the collector again"
          loading={top.isFetching}
          onClick={() => {
            void top.refetch();
            if (capabilities.exporters) void exporters.refetch();
          }}
        />
      </Toolbar>
      {way && <p className={styles.muted}>{way.help}</p>}

      {top.isError ? (
        <ErrorState title="The top talkers could not be read" error={top.error} onRetry={() => void top.refetch()} />
      ) : !data ? (
        <Skeleton shape="block" height={280} />
      ) : (
        <div className={styles.flowLayout}>
          <div className={styles.flowTable}>
            <Table
              id={`logs.flows.${by}`}
              columns={columns}
              rows={data.rows}
              rowKey={(row) => row.key}
              defaultSort={{ columnId: 'bytes', direction: 'desc' }}
              maxHeight={420}
              empty={
                <EmptyState
                  size="compact"
                  icon={Waves}
                  title={
                    empty === 'not-collecting'
                      ? 'The flow collector is not listening'
                      : empty === 'no-rows'
                        ? 'No row for this way of looking'
                        : 'No flow record in this period'
                  }
                  description={
                    empty === 'not-collecting'
                      ? flows?.enabled
                        ? `It is switched on and could not open its port${flows.error ? `: ${flows.error}` : ''}. This is “not collecting”, not “no traffic”.`
                        : 'This is “not collecting”, not “no traffic”: nothing can arrive until the collector is switched on in the Receivers pane and an exporter is pointed at it.'
                      : empty === 'no-rows'
                        ? 'The period holds flow records, but none under a key of this kind.'
                        : 'The collector is listening and no exporter has reported anything in this period. A longer period may hold something; flows are kept for a day, in memory.'
                  }
                />
              }
              aria-label={`Top talkers by ${way?.label.toLowerCase() ?? by}`}
            />
            <dl className={styles.totals} aria-label="Totals">
              <div data-testid="flows-other">
                <dt>Other</dt>
                <dd>
                  {hasOther(data.other)
                    ? `${formatBytes(data.other.bytes)} · ${plural(data.other.packets, 'packet')} · ${plural(data.other.flows, 'flow')}`
                    : 'Nothing: every byte counted is in a row above'}
                </dd>
              </div>
              <div data-testid="flows-total">
                <dt>Total</dt>
                <dd>
                  {formatBytes(data.total.bytes)} · {plural(data.total.packets, 'packet')} ·{' '}
                  {plural(data.total.flows, 'flow')}
                </dd>
              </div>
            </dl>
            <p className={styles.muted}>
              The total is everything the exporters reported in the period, exact. A row is a floor —
              never more than was seen — and “other” is what is under no key.
              {by === 'interface'
                ? ' By interface each flow is counted twice, in and out, so the rows add up to twice the total.'
                : ''}
            </p>
          </div>

          <aside className={styles.coverage} aria-label="What this answer covers">
            <h3 className={styles.cardTitle}>What this covers</h3>
            <dl className={styles.coverageList}>
              {coverageLines(data.coverage).map((line) => (
                <div key={line.id} data-coverage={line.id}>
                  <dt>{line.label}</dt>
                  <dd>
                    {line.tone ? <StatusPill tone={line.tone}>{line.value}</StatusPill> : line.value}
                    {line.note && <p className={styles.coverageNote}>{line.note}</p>}
                  </dd>
                </div>
              ))}
            </dl>
            {flows?.lab && <p className={styles.muted}>{flows.lab}</p>}
          </aside>
        </div>
      )}

      {capabilities.exporters && (
        <Exporters
          exporters={exporters}
          onPick={(exporter) =>
            update({
              scope:
                exporter.device_id != null && exporter.device_id !== ''
                  ? `device:${exporter.device_id}`
                  : `exporter:${exporter.address}`,
            })
          }
        />
      )}
    </div>
  );
}

function Exporters({
  exporters,
  onPick,
}: {
  exporters: ReturnType<typeof useFlowExporters>;
  onPick: (exporter: FlowExporter) => void;
}) {
  const columns: readonly TableColumn<FlowExporter>[] = [
    { id: 'address', header: 'Exporter', cell: (row) => row.address, mono: true, required: true, width: 132 },
    {
      id: 'device',
      header: 'Device',
      cell: (row) =>
        row.device ? (
          row.device
        ) : (
          // The engine's own reason no device claims the address.
          <span className={styles.muted} title={row.reason}>
            {row.reason === '' ? 'no device' : row.reason}
          </span>
        ),
      sortValue: (row) => row.device ?? '',
    },
    { id: 'version', header: 'Speaks', width: 100, cell: (row) => versionLabel(row.version) },
    {
      id: 'datagrams',
      header: 'Datagrams',
      align: 'right',
      width: 116,
      cell: (row) => formatCount(row.datagrams),
      sortValue: (row) => row.datagrams,
    },
    {
      id: 'records',
      // Counted, beside "Not counted": the panel's note says what of.
      header: 'Records',
      align: 'right',
      width: 100,
      cell: (row) => formatCount(row.records),
      sortValue: (row) => row.records,
    },
    {
      id: 'dropped',
      header: 'Not counted',
      align: 'right',
      width: 124,
      cell: (row) => formatCount(row.dropped),
      sortValue: (row) => row.dropped,
    },
    {
      id: 'templates',
      header: 'Templates',
      align: 'right',
      width: 112,
      cell: (row) => formatCount(row.templates),
      sortValue: (row) => row.templates,
    },
    {
      id: 'unread',
      header: 'Sets without template',
      align: 'right',
      width: 178,
      cell: (row) => formatCount(row.sets_without_template),
      sortValue: (row) => row.sets_without_template,
    },
    {
      id: 'sampling',
      header: 'Sampling',
      width: 112,
      cell: (row) =>
        row.sampling.length === 0 ? <span className={styles.muted}>none said</span> : ratesPhrase(row.sampling),
    },
    {
      id: 'last',
      header: 'Last heard',
      width: 116,
      cell: (row) => <TimeAgo at={row.last_at} />,
      sortValue: (row) => row.last_at,
    },
  ];

  return (
    <Panel
      title="Exporters"
      storageId="logs.flows.exporters"
      collapsible
      padding="none"
      actions={
        exporters.data ? <span className={styles.muted}>{plural(exporters.data.length, 'exporter')}</span> : undefined
      }
    >
      <p className={styles.panelNote}>
        Every address that has sent flow export, most recently heard first; at most 256 are followed and
        one goes when it has been silent for a day. An exporter is attributed by the address its datagrams
        come from. “Records” are the flow records counted from it, and “Not counted” those left out
        because no device claims it and unknown sources are dropped. “Templates” is the templates the
        collector knows for it now; a data set that arrives before its template cannot be read, and is
        counted instead. Open a row to look at that exporter alone.
      </p>
      {exporters.isError ? (
        <ErrorState
          size="compact"
          title="The exporters could not be read"
          error={exporters.error}
          onRetry={() => void exporters.refetch()}
        />
      ) : (
        <Table
          id="logs.flows.exporters"
          columns={columns}
          rows={exporters.data ?? []}
          rowKey={(row) => row.address}
          loading={exporters.isPending}
          maxHeight={320}
          onRowActivate={onPick}
          empty="No exporter has sent anything since the collector started"
          aria-label="Flow exporters"
        />
      )}
    </Panel>
  );
}
