/**
 * One chart of one or more series, with the holes left in.
 *
 * `metrics.query_many` answers with only the buckets that have something in
 * them. `alignSeries` puts the answers on one axis and marks what is missing;
 * uPlot lifts the pen there, and the line under the chart says how much of the
 * range was never recorded — a hole in a line is easy to miss, and the
 * difference between "nothing happened" and "nothing was measured" is the
 * whole point.
 */

import { Unplug } from 'lucide-react';
import { useRpc } from '@/api/hooks';
import type { MetricSeries, SeriesRef } from '@/api/types/monitor';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { TimeSeriesChart, type ChartSeries, type SeriesSlot } from '@/design/TimeSeriesChart';
import { formatCount, formatDuration } from '@/lib/format';
import { formatTime } from '@/lib/time';
import styles from './Monitoring.module.css';
import { alignSeries } from './series';
import { metricFormat, metricMax, seriesLabel } from './vocab';

export interface ChartLine extends SeriesRef {
  /** What this line is called in the legend. Defaults to the metric and instance. */
  label?: string;
  /** Keeps a line the same colour wherever it is drawn. */
  slot?: SeriesSlot;
}

export interface MetricChartProps {
  title: string;
  subtitle?: string;
  /** Up to eight lines, all of the same metric family: one chart, one y-axis. */
  lines: readonly ChartLine[];
  /** The metric whose unit the axis is in. Defaults to the first line's. */
  unitOf?: string;
  since: number;
  until?: number;
  step_s: number;
  height?: number;
  /** Refetch when these event topics fire. */
  invalidateOn?: string[];
}

export function MetricChart({
  title,
  subtitle,
  lines,
  unitOf,
  since,
  until,
  step_s,
  height = 200,
  invalidateOn = ['monitor.polled'],
}: MetricChartProps) {
  const params = {
    series: lines.map(({ device_id, metric, instance }) => ({ device_id, metric, instance: instance ?? '' })),
    since,
    until,
    step_s,
  };
  const query = useRpc<MetricSeries[]>('metrics.query_many', params, {
    enabled: lines.length > 0,
    invalidateOn,
  });

  if (query.isError) {
    return (
      <ErrorState
        size="compact"
        title={`${title} could not be read`}
        error={query.error}
        onRetry={() => void query.refetch()}
      />
    );
  }

  const answers = query.data ?? [];
  const { timestamps, values, gaps } = alignSeries(answers);
  const metric = unitOf ?? lines[0]?.metric ?? '';
  const series: ChartSeries[] = lines.map((line, index) => ({
    id: `${line.device_id}:${line.metric}:${line.instance ?? ''}`,
    label: line.label ?? seriesLabel(line.metric, line.instance ?? ''),
    values: values[index] ?? [],
    slot: line.slot,
  }));

  const missing = gaps.reduce((total, gap) => total + (gap.to - gap.from), 0);

  return (
    <div className={styles.chartCard}>
      <TimeSeriesChart
        title={title}
        subtitle={subtitle}
        timestamps={timestamps}
        series={series}
        formatValue={metricFormat(metric)}
        max={metricMax(metric)}
        height={height}
        // Only the first load: a refetch on every poll would make the picture
        // blink once a minute for no reason.
        loading={query.isPending}
      />
      {gaps.length > 0 && (
        <p className={styles.gaps} data-testid="chart-gaps">
          <Icon icon={Unplug} size={12} className={styles.gapsIcon} />
          {gaps.length === 1
            ? `A gap of ${formatDuration(missing)} at ${formatTime(gaps[0]?.from)}: nothing was recorded then.`
            : `${formatCount(gaps.length)} gaps, ${formatDuration(missing)} in all: nothing was recorded then.`}
        </p>
      )}
    </div>
  );
}
