/**
 * The findings: what is wrong, on which device, decided from what.
 *
 * Grouped worst first, filterable by severity, category, check, device and
 * site, and every card carries the evidence it was decided from — a finding
 * with no evidence on screen is an accusation, not a report.
 *
 * Three answers are kept apart here. A finding is a problem. An empty list
 * under a filter is "nothing matches". An empty list with no filter is *not*
 * "the network is clean": it says so, and points at the coverage band.
 */

import { ListFilter, Radar, ScanSearch, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import type { Category, Insight, Severity } from '@/api/types/insights';
import { registerCommand } from '@/app/CommandPalette/commands';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { formatCount, plural } from '@/lib/format';
import { Chip, type FacetValue } from './Facets';
import type { Scan } from './useScan';
import {
  EMPTY_FILTER,
  indexDevices,
  isNarrowed,
  MAX_PAGE,
  PAGE,
  useCoverage,
  useDevices,
  useHasMethod,
  useInsights,
  useInsightsSettings,
  type Filter,
} from './data';
import { DismissDialog } from './DismissDialog';
import { FindingCard } from './FindingCard';
import { FindingDrawer } from './FindingDrawer';
import {
  CATEGORIES,
  categoryLabel,
  coverageTotals,
  groupBySeverity,
  severityLabel,
  SEVERITIES,
} from './insight';
import styles from './Insights.module.css';

const STATE_OPTIONS = [
  { value: 'open', label: 'Open' },
  { value: 'dismissed', label: 'Dismissed' },
  { value: 'resolved', label: 'Resolved' },
  { value: 'any', label: 'Every state' },
];

export interface FindingsTabProps {
  scan: Scan;
}

export function FindingsTab({ scan }: FindingsTabProps) {
  const [filter, setFilter] = useState<Filter>(EMPTY_FILTER);
  const [limit, setLimit] = useState(PAGE);
  const [opened, setOpened] = useState<Insight | null>(null);
  const [dismissing, setDismissing] = useState<Insight | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const list = useInsights(filter, limit);
  const devices = useDevices();
  const settings = useInsightsSettings();
  const coverage = useCoverage();
  // Both are asked for unconditionally: `&&` would short-circuit the second
  // hook away. A fix needs the insight method *and* the change pipeline.
  const hasFix = useHasMethod('insights.fix');
  const hasChanges = useHasMethod('changes.create');
  const canDraftChange = hasFix === true && hasChanges === true;

  const change = (patch: Partial<Filter>) => {
    setFilter((current) => ({ ...current, ...patch }));
    setLimit(PAGE);
  };

  useEffect(
    () =>
      registerCommand({
        id: 'insights.search',
        title: 'Search findings',
        group: 'Insights',
        keywords: ['filter', 'find', 'device', 'interface'],
        shortcut: '/',
        icon: ListFilter,
        run: () => searchRef.current?.focus(),
      }),
    [],
  );

  const index = useMemo(
    () => indexDevices(devices.data ?? [], settings.data?.include_lab ?? true),
    [devices.data, settings.data?.include_lab],
  );

  const matched = list.data?.insights ?? [];
  const rows = useMemo(() => {
    if (filter.site === null) return matched;
    return matched.filter((insight) => index.byId.get(insight.device_id)?.site === filter.site);
  }, [matched, filter.site, index]);

  const counts = list.data?.counts;
  const total = counts
    ? Object.values(counts.by_severity).reduce((sum, value) => sum + value, 0)
    : 0;

  const severityFacets: FacetValue[] = SEVERITIES.map((severity) => ({
    value: severity,
    label: severityLabel(severity),
    count: counts?.by_severity[severity] ?? 0,
  }));
  const categoryFacets: FacetValue[] = CATEGORIES.map((category) => ({
    value: category,
    label: categoryLabel(category),
    count: counts?.by_category[category] ?? 0,
  }));
  const deviceFacets: FacetValue[] = useMemo(() => {
    const tally = new Map<string, FacetValue>();
    for (const insight of matched) {
      const found = tally.get(insight.device_id);
      if (found) found.count += 1;
      else
        tally.set(insight.device_id, {
          value: insight.device_id,
          label: insight.device,
          count: 1,
        });
    }
    return [...tally.values()].sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  }, [matched]);
  const siteFacets: FacetValue[] = useMemo(() => {
    const tally = new Map<string, FacetValue>();
    for (const insight of matched) {
      const site = index.byId.get(insight.device_id)?.site;
      if (site === undefined || site === '') continue;
      const found = tally.get(site);
      if (found) found.count += 1;
      else tally.set(site, { value: site, label: site, count: 1 });
    }
    return [...tally.values()].sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  }, [matched, index]);

  const groups = groupBySeverity(rows);
  const narrowed = isNarrowed(filter) || filter.state !== 'open';
  const totals = coverage.data ? coverageTotals(coverage.data) : null;
  // Nothing to narrow, and nothing narrowing it: a row of filters over an
  // empty screen is furniture.
  const filtering = narrowed || matched.length > 0 || list.isPending;

  return (
    <div className={styles.findings}>
      {filtering && (
        <div className={styles.filters} role="search">
          <SearchInput
            ref={searchRef}
            value={filter.query}
            onChange={(query) => change({ query })}
            aria-label="Search findings"
            placeholder="Search device, interface, check…"
            className={styles.search}
          />
          <Select
            size="sm"
            className={styles.state}
            options={STATE_OPTIONS}
            value={filter.state ?? 'open'}
            aria-label="State"
            onChange={(event) => change({ state: event.target.value as Filter['state'] })}
          />
          <Chip
            label="Severity"
            value={filter.severity ?? null}
            values={severityFacets}
            onSelect={(value) => change({ severity: (value as Severity | null) ?? undefined })}
          />
          <Chip
            label="Category"
            value={filter.category ?? null}
            values={categoryFacets}
            onSelect={(value) => change({ category: (value as Category | null) ?? undefined })}
          />
          <Chip
            label="Device"
            value={filter.deviceId}
            values={deviceFacets}
            onSelect={(deviceId) => change({ deviceId })}
          />
          <Chip
            label="Site"
            value={filter.site}
            values={siteFacets}
            onSelect={(site) => change({ site })}
          />
          {filter.check !== null && (
            <Chip
              label="Check"
              value={filter.check}
              values={[{ value: filter.check, label: filter.check, count: rows.length }]}
              onSelect={(check) => change({ check })}
            />
          )}
          {narrowed && (
            <Button size="sm" variant="ghost" onClick={() => change(EMPTY_FILTER)}>
              Clear filters
            </Button>
          )}
        </div>
      )}

      {filter.severity !== undefined && (
        <p className={styles.note} data-tone="idle">
          Severity is a floor, not an exact match: this is everything{' '}
          {severityLabel(filter.severity).toLowerCase()} and worse.
        </p>
      )}

      {list.isPending ? (
        <div className={styles.cards}>
          <Skeleton shape="block" height={128} />
          <Skeleton shape="block" height={128} />
          <Skeleton shape="block" height={128} />
        </div>
      ) : list.error ? (
        <ErrorState
          title="The findings could not be read"
          error={list.error}
          onRetry={() => void list.refetch()}
        />
      ) : rows.length === 0 ? (
        <Empty
          narrowed={narrowed}
          scanned={coverage.data != null}
          skipped={totals?.skipped ?? 0}
          considered={totals?.considered ?? 0}
          onClear={() => change(EMPTY_FILTER)}
          onScan={() => scan.start()}
          scanning={scan.running}
        />
      ) : (
        <>
          {groups.map((group) => (
            <Panel
              key={group.severity}
              title={severityLabel(group.severity)}
              storageId={`insights.group.${group.severity}`}
              collapsible
              defaultCollapsed={group.severity === 'info'}
              padding="none"
              className={styles.group}
              actions={
                <span className={styles.groupCount}>
                  {plural(group.items.length, 'finding')}
                </span>
              }
            >
              <ul className={styles.cards}>
                {group.items.map((insight) => (
                  <li key={insight.id}>
                    <FindingCard
                      insight={insight}
                      site={index.byId.get(insight.device_id)?.site}
                      canDraftChange={canDraftChange}
                      onOpen={() => setOpened(insight)}
                      onDismiss={() => setDismissing(insight)}
                      onCheck={(check) => change({ check })}
                    />
                  </li>
                ))}
              </ul>
            </Panel>
          ))}

          <div className={styles.footer}>
            <span>
              {rows.length === total
                ? plural(total, 'finding')
                : `Showing ${formatCount(rows.length)} of ${plural(total, 'finding')}`}
              {filter.site !== null && list.data?.next_cursor !== undefined && (
                <>
                  {' '}
                  — the site filter narrows the {formatCount(matched.length)} loaded findings, not
                  all {formatCount(total)}
                </>
              )}
            </span>
            {list.data?.next_cursor !== undefined && limit < MAX_PAGE && (
              <Button
                size="sm"
                variant="secondary"
                onClick={() => setLimit((current) => Math.min(current + PAGE, MAX_PAGE))}
                loading={list.isFetching}
              >
                Show more
              </Button>
            )}
          </div>
        </>
      )}

      <FindingDrawer
        insight={opened}
        site={opened ? index.byId.get(opened.device_id)?.site : undefined}
        canDraftChange={canDraftChange}
        onClose={() => setOpened(null)}
        onDismiss={() => {
          if (opened) setDismissing(opened);
        }}
      />
      <DismissDialog insight={dismissing} onClose={() => setDismissing(null)} />
    </div>
  );
}

interface EmptyProps {
  narrowed: boolean;
  scanned: boolean;
  skipped: number;
  considered: number;
  onClear: () => void;
  onScan: () => void;
  scanning: boolean;
}

/**
 * The three ways this list can be empty, which must not read alike: nothing
 * has been examined, nothing matches the filter, and nothing was found — the
 * last of which is only as good as the coverage above it.
 */
function Empty({ narrowed, scanned, skipped, considered, onClear, onScan, scanning }: EmptyProps) {
  if (!scanned) {
    return (
      <EmptyState
        icon={Radar}
        title="No scan has run yet"
        description="Insights are worked out from what has already been collected from your devices — interface state and counters, neighbours, VLANs, routes, configurations, logs and metrics. Nothing is connected to."
        action={
          <Button variant="primary" icon={Radar} loading={scanning} onClick={onScan}>
            Run the first scan
          </Button>
        }
      />
    );
  }
  if (narrowed) {
    return (
      <EmptyState
        icon={ListFilter}
        title="Nothing matches these filters"
        description="There may still be findings outside them."
        action={
          <Button variant="secondary" onClick={onClear}>
            Clear filters
          </Button>
        }
      />
    );
  }
  return (
    <EmptyState
      icon={TriangleAlert}
      title="No open findings"
      description={
        skipped > 0 ? (
          <>
            Nothing was found — but that is not the same as a clean network.{' '}
            <strong>
              {formatCount(skipped)} of {formatCount(considered)}
            </strong>{' '}
            device-by-check answers were <strong>not evaluated</strong> in the last scan. The
            coverage band above says which, and why.
          </>
        ) : (
          'Every enabled check reached a verdict for every device in the last scan, and none of them found anything.'
        )
      }
      action={
        <Button variant="secondary" icon={ScanSearch} onClick={onScan} loading={scanning}>
          Scan again
        </Button>
      }
    />
  );
}
