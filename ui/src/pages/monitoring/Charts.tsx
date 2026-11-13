/**
 * The Charts tab: pick series, see them drawn.
 *
 * What a device actually has is `metrics.series` — the backend lists only
 * series that have at least one sample, so the picker can never offer a line
 * that does not exist. Chosen series are grouped by metric, one chart per
 * metric, because a chart has one y-axis and a rate is not a percentage.
 */

import { ChartLine as ChartLineIcon, X } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { SeriesSummary } from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { Checkbox } from '@/design/Checkbox';
import { Combobox } from '@/design/Combobox';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Skeleton } from '@/design/Skeleton';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { useDevices } from './data';
import { MetricChart, type ChartLine } from './MetricChart';
import styles from './Monitoring.module.css';
import { DEFAULT_RANGE, range } from './series';
import { RangePicker } from './RangePicker';
import { metricLabel, seriesLabel } from './vocab';

interface Pick {
  device_id: string;
  device: string;
  metric: string;
  instance: string;
}

const key = (pick: Pick) => `${pick.device_id}\u0000${pick.metric}\u0000${pick.instance}`;
/** One chart reads up to eight lines, so each metric takes at most eight. */
const MAX_LINES = 8;

export function ChartsTab() {
  const [deviceId, setDeviceId] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [rangeId, setRangeId] = useState(DEFAULT_RANGE);
  const [picked, setPicked] = useState<Pick[]>([]);

  const devices = useDevices();
  const device = devices.data?.find((entry) => entry.id === deviceId);
  const series = useRpc<SeriesSummary[]>(
    'metrics.series',
    { device_id: deviceId },
    { enabled: Boolean(deviceId), invalidateOn: ['monitor.polled'] },
  );

  const span = range(rangeId);
  const since = Math.floor(Date.now() / 30_000) * 30_000 - span.span;

  const needle = filter.trim().toLowerCase();
  const available = (series.data ?? []).filter(
    (entry) =>
      !needle ||
      `${entry.metric} ${entry.instance} ${metricLabel(entry.metric)}`.toLowerCase().includes(needle),
  );

  const chosen = useMemo(() => new Set(picked.map(key)), [picked]);
  /** How many lines each metric already has: the cap is per chart, not in all. */
  const perMetric = useMemo(() => {
    const counts = new Map<string, number>();
    for (const pick of picked) counts.set(pick.metric, (counts.get(pick.metric) ?? 0) + 1);
    return counts;
  }, [picked]);
  const full = (metric: string) => (perMetric.get(metric) ?? 0) >= MAX_LINES;

  const toggle = (entry: SeriesSummary) => {
    if (!device) return;
    const pick: Pick = {
      device_id: device.id,
      device: device.name,
      metric: entry.metric,
      instance: entry.instance,
    };
    setPicked((current) =>
      current.some((existing) => key(existing) === key(pick))
        ? current.filter((existing) => key(existing) !== key(pick))
        : [...current, pick],
    );
  };

  // One chart per metric: all its lines share a unit and an axis.
  const charts = useMemo(() => {
    const byMetric = new Map<string, ChartLine[]>();
    for (const pick of picked) {
      const lines = byMetric.get(pick.metric) ?? [];
      lines.push({
        device_id: pick.device_id,
        metric: pick.metric,
        instance: pick.instance,
        label: pick.instance ? `${pick.device} · ${pick.instance}` : pick.device,
      });
      byMetric.set(pick.metric, lines);
    }
    return [...byMetric.entries()];
  }, [picked]);

  return (
    <>
      <Toolbar className={styles.filters}>
        <Combobox
          size="sm"
          aria-label="Device"
          className={styles.search}
          placeholder="Pick a device…"
          emptyText="No device matches"
          clearable
          value={deviceId}
          onChange={setDeviceId}
          options={(devices.data ?? []).map((entry) => ({
            value: entry.id,
            label: entry.name,
            description: entry.site || entry.role,
          }))}
        />
        <RangePicker value={rangeId} onChange={setRangeId} />
        <span className={styles.count}>
          {picked.length === 0
            ? 'Nothing picked yet'
            : `${formatCount(picked.length)} ${picked.length === 1 ? 'series' : 'series'} on ${formatCount(charts.length)} ${charts.length === 1 ? 'chart' : 'charts'}`}
        </span>
        <ToolbarSpacer />
        {picked.length > 0 && (
          <Button size="sm" icon={X} onClick={() => setPicked([])}>
            Clear
          </Button>
        )}
      </Toolbar>

      {picked.length > 0 && (
        <div className={styles.inline}>
          {picked.map((pick) => (
            <Badge key={key(pick)} variant="outline">
              {pick.device} · {seriesLabel(pick.metric, pick.instance)}
              <button
                type="button"
                className={styles.chipRemove}
                aria-label={`Remove ${pick.device} ${seriesLabel(pick.metric, pick.instance)}`}
                onClick={() => setPicked((current) => current.filter((entry) => key(entry) !== key(pick)))}
              >
                <X size={11} />
              </button>
            </Badge>
          ))}
        </div>
      )}

      <div className={styles.explorer}>
        <Card className={styles.picker} padding="sm">
          {!deviceId ? (
            <p className={styles.hint}>Pick a device to see which series it has.</p>
          ) : series.isError ? (
            <ErrorState
              size="compact"
              title="Its series could not be listed"
              error={series.error}
              onRetry={() => void series.refetch()}
            />
          ) : series.isPending ? (
            <Skeleton shape="block" height={200} />
          ) : (
            <>
              <SearchInput
                size="sm"
                aria-label="Search series"
                placeholder="Filter series…"
                value={filter}
                onChange={setFilter}
              />
              <p className={styles.hint}>
                {formatCount(series.data?.length ?? 0)} series recorded for {device?.name}. Only what has been
                measured is listed.
              </p>
              <ul className={styles.seriesList}>
                {available.map((entry) => (
                  <li key={`${entry.metric}\u0000${entry.instance}`}>
                    <Checkbox
                      checked={chosen.has(`${deviceId}\u0000${entry.metric}\u0000${entry.instance}`)}
                      disabled={
                        full(entry.metric) &&
                        !chosen.has(`${deviceId}\u0000${entry.metric}\u0000${entry.instance}`)
                      }
                      onChange={() => toggle(entry)}
                    >
                      {seriesLabel(entry.metric, entry.instance)}
                    </Checkbox>
                  </li>
                ))}
              </ul>
              {available.length === 0 && <p className={styles.hint}>Nothing matches that filter.</p>}
              {[...perMetric.keys()].some(full) && (
                <p className={styles.hint}>
                  Eight lines is as many as one chart reads, and{' '}
                  {[...perMetric.keys()].filter(full).map(metricLabel).join(', ')} already has that many.
                  Remove one to add another.
                </p>
              )}
            </>
          )}
        </Card>

        <div className={styles.charts}>
          {charts.length === 0 ? (
            <EmptyState
              icon={ChartLineIcon}
              title="Pick what to plot"
              description="Choose a device, then tick the series you want. They are grouped onto one chart per metric, and anything that was never recorded is left as a hole."
            />
          ) : (
            charts.map(([metric, lines]) => (
              <MetricChart
                key={metric}
                title={metricLabel(metric)}
                subtitle={lines.length === 1 ? lines[0]?.label : `${formatCount(lines.length)} series`}
                lines={lines}
                unitOf={metric}
                since={since}
                step_s={span.step_s}
                height={240}
              />
            ))
          )}
        </div>
      </div>
    </>
  );
}
