import { ChartLine, Copy, FileSpreadsheet, ImageDown, Share, Table as TableIcon } from 'lucide-react';
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react';
import {
  CaptureError,
  chartCsv,
  chartToPng,
  copyImageOrSave,
  fileStem,
  savePicture,
  tellImageCopied,
} from '@/lib/capture';
import { cx } from '@/lib/cx';
import { download, tellSaved } from '@/lib/download';
import { formatCompact, formatCount, type Numeric } from '@/lib/format';
import { formatDate, formatDateTime, formatTime } from '@/lib/time';
import { IconButton } from '../Button';
import { DropdownMenu } from '../DropdownMenu';
import { EmptyState } from '../EmptyState';
import type { MenuEntry } from '../Menu';
import { Skeleton } from '../Skeleton';
import { Table, type TableColumn } from '../Table';
import { useTokens } from '../theme';
import type { StatusTone } from '../tone';
import styles from './TimeSeriesChart.module.css';
import uPlot from './uplot';

/** A slot of the categorical palette (`--series-1` … `--series-8`). */
export type SeriesSlot = 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8;

export interface ChartSeries {
  /** Stable id of what is plotted (an interface, a device, a metric). */
  id: string;
  label: string;
  /** One value per timestamp. `null` is a gap: a missing sample is not drawn. */
  values: readonly (number | null)[];
  /**
   * The palette slot. Colour follows the entity: give the same thing the same
   * slot wherever it is plotted, so a filter never repaints the survivors.
   * Defaults to the series' position.
   */
  slot?: SeriesSlot;
  /**
   * A status colour instead of a palette slot, only for a series that *is* a
   * state (errors, drops). Never for telling ordinary series apart.
   */
  tone?: StatusTone;
}

/**
 * The chart's export menu. Off unless a screen asks for it.
 *
 * It offers a PNG of the chart (the plot with its title and legend, in the
 * theme on screen), the same picture on the clipboard, and the numbers as
 * CSV with gaps as empty cells. None of that stores anything. A screen that
 * can also *keep* the chart adds its own entry through `extra` — see
 * `useKeepAsEvidence` in `@/app/KeepAsEvidence`.
 */
export interface ChartExportMenu {
  /** The stem of the file names (`<stem>.png`, `<stem>.csv`). Defaults to the title. */
  filename?: string;
  /**
   * A line under the plot in the picture, for what the picture cannot show by
   * itself: gaps, coverage, where the numbers came from.
   */
  note?: string;
  /** Further entries, under a separator: "Keep as evidence…". */
  extra?: readonly MenuEntry[];
}

export interface TimeSeriesChartProps {
  /** Names what is plotted. A single series has no legend: the title is its name. */
  title: string;
  /** A line under the title: the unit, the device, the time range. */
  subtitle?: string;
  /** Sample times in milliseconds since the epoch, ascending. */
  timestamps: readonly number[];
  /** Up to eight series, all in the same unit: one chart has one y-axis. */
  series: readonly ChartSeries[];
  /** Formats a value for the axis, the tooltip and the table, e.g. `formatRate`. */
  formatValue?: (value: Numeric) => string;
  /** Height of the plot including its time axis, in pixels. */
  height?: number;
  /** Lower bound of the y-axis. Defaults to zero (or the lowest value, if negative). */
  min?: number;
  /** Upper bound of the y-axis, e.g. 100 for a percentage. Defaults to the data. */
  max?: number;
  /** New data is on its way: the current picture is held, dimmed, instead of flashing a placeholder. */
  loading?: boolean;
  /** Label the time axis in UTC instead of local time. */
  utc?: boolean;
  /**
   * Adds an export menu to the header (PNG, copy image, CSV, and whatever
   * the screen adds). Absent by default: no existing chart changes.
   */
  exportMenu?: ChartExportMenu;
  className?: string;
}

const SLOT_TOKENS = [
  '--series-1',
  '--series-2',
  '--series-3',
  '--series-4',
  '--series-5',
  '--series-6',
  '--series-7',
  '--series-8',
] as const;
const TOKENS = [
  ...SLOT_TOKENS,
  '--chart-grid',
  '--chart-axis',
  '--chart-label',
  '--ok',
  '--warn',
  '--crit',
  '--info',
  '--idle',
  '--maint',
] as const;

const PICTURE_TOKENS = ['--text-1', '--text-3'] as const;

const MAX_SERIES = SLOT_TOKENS.length;
/** Direct labels at the line ends are only legible for a few series. */
const MAX_DIRECT_LABELS = 4;
const AXIS_FONT = "11px 'Inter Variable', system-ui, sans-serif";
const LABEL_HEIGHT = 14;
const MINUTE_MS = 60_000;
const DAY_MS = 86_400_000;

function withAlpha(hex: string, alpha: number): string {
  const [r, g, b] = [1, 3, 5].map((start) => Number.parseInt(hex.slice(start, start + 2), 16));
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/** The first opaque background behind an element: what the chart is drawn on. */
function surfaceColor(element: Element): string {
  for (let node: Element | null = element; node; node = node.parentElement) {
    const { backgroundColor } = getComputedStyle(node);
    if (backgroundColor && backgroundColor !== 'transparent' && !/,\s*0\)$/.test(backgroundColor)) {
      return backgroundColor;
    }
  }
  return 'transparent';
}

function textWidth(text: string): number {
  const context = document.createElement('canvas').getContext('2d');
  if (!context) return text.length * 6;
  context.font = AXIS_FONT;
  return context.measureText(text).width;
}

interface Hover {
  index: number;
  /** Crosshair position in pixels from the plot container's left edge. */
  left: number;
  flip: boolean;
}

interface EndLabel {
  id: string;
  label: string;
  top: number;
}

/**
 * A time series line chart (uPlot). One y-axis, hairline grid, a crosshair
 * with a tooltip that lists every series, a legend for two or more series,
 * and a table view of the same numbers. The arrow keys move the crosshair.
 *
 * Filters and time ranges belong to the page, above the charts they scope.
 */
export function TimeSeriesChart({
  title,
  subtitle,
  timestamps,
  series: allSeries,
  formatValue = formatCompact,
  height = 220,
  min,
  max,
  loading = false,
  utc = false,
  exportMenu,
  className,
}: TimeSeriesChartProps) {
  const series = allSeries.slice(0, MAX_SERIES);
  const notShown = allSeries.length - series.length;
  const tokens = useTokens(TOKENS);
  // The picture's own text. Read apart from TOKENS, which the plot is built from.
  const ink = useTokens(PICTURE_TOKENS);
  const colors = series.map((entry, index) =>
    entry.tone ? tokens[`--${entry.tone}`] : tokens[SLOT_TOKENS[(entry.slot ?? index + 1) - 1] ?? '--series-1'],
  );

  const [view, setView] = useState<'chart' | 'table'>('chart');
  const [hidden, setHidden] = useState<ReadonlySet<string>>(new Set());
  const [hover, setHover] = useState<Hover | null>(null);
  const [endLabels, setEndLabels] = useState<EndLabel[]>([]);
  const [focused, setFocused] = useState(false);

  const plotRef = useRef<HTMLDivElement>(null);
  const chart = useRef<uPlot | null>(null);
  const format = useRef(formatValue);
  useEffect(() => {
    format.current = formatValue;
  });

  const data: uPlot.AlignedData = [
    timestamps as number[],
    ...series.map((entry) => entry.values as (number | null)[]),
  ];
  /** The columns the plot currently shows, to tell new data from a mere re-render. */
  const plotted = useRef<uPlot.AlignedData | null>(null);

  const empty = timestamps.length === 0;
  const directLabels = series.length >= 2 && series.length <= MAX_DIRECT_LABELS;
  // What the plot is built from. When this changes the plot is rebuilt;
  // when only the data changes it is updated in place.
  const structure = JSON.stringify({
    series: series.map((entry) => [entry.id, entry.label]),
    colors,
    tokens,
    height,
    min,
    max,
    utc,
    directLabels,
  });

  useLayoutEffect(() => {
    const container = plotRef.current;
    if (!container || view !== 'chart' || empty) return;

    const surface = surfaceColor(container);
    const single = series.length === 1;
    const gutter = directLabels
      ? Math.ceil(Math.max(...series.map((entry) => textWidth(entry.label)))) + 14
      : 12;

    const plot = new uPlot(
      {
        width: container.clientWidth,
        height,
        ms: 1,
        padding: [10, gutter, 0, 0],
        legend: { show: false },
        tzDate: utc ? (timestamp) => uPlot.tzDate(new Date(timestamp), 'Etc/UTC') : undefined,
        cursor: {
          x: true,
          y: false,
          drag: { x: false, y: false },
          points: {
            size: 8,
            width: 2,
            stroke: surface,
            fill: (_self, seriesIndex) => colors[seriesIndex - 1] ?? 'transparent',
          },
        },
        scales: {
          x: { time: true },
          y: {
            range: (_self, dataMin, dataMax) => {
              const low = min ?? Math.min(0, dataMin ?? 0);
              const high = max ?? (dataMax ?? low + 1);
              // A little headroom, so the highest line never touches the frame.
              return [low, max ?? high + (high - low || 1) * 0.1];
            },
          },
        },
        series: [
          {},
          ...series.map((entry, index) => {
            const color = colors[index] ?? 'transparent';
            return {
              label: entry.label,
              stroke: color,
              width: 2,
              cap: 'round' as const,
              // A lone series gets a faint wash; several would only muddy each other.
              fill: single ? withAlpha(color, 0.1) : undefined,
              points: { size: 8, width: 2, stroke: surface, fill: color },
            };
          }),
        ],
        axes: [
          {
            stroke: tokens['--chart-label'],
            font: AXIS_FONT,
            // Room for two lines: the time, and the date where the day changes.
            size: 42,
            gap: 6,
            grid: { show: false },
            ticks: { show: false },
            border: { show: true, stroke: tokens['--chart-axis'], width: 1 },
            values: (_self, splits, _axisIndex, _space, increment) => {
              let previousDay = '';
              return splits.map((timestamp) => {
                const day = formatDate(timestamp, { utc });
                if (increment >= DAY_MS) return day.slice(5);
                const time = formatTime(timestamp, { utc }).slice(0, increment < MINUTE_MS ? 8 : 5);
                const dayChanged = day !== previousDay;
                previousDay = day;
                return dayChanged ? `${time}\n${day}` : time;
              });
            },
          },
          {
            stroke: tokens['--chart-label'],
            font: AXIS_FONT,
            gap: 8,
            // Solid hairlines, one step off the surface: the data is the only loud thing.
            grid: { stroke: tokens['--chart-grid'], width: 1 },
            ticks: { show: false },
            values: (_self, splits) => splits.map((value) => format.current(value)),
            // Wide enough for the longest tick label, measured.
            size: (self, values, axisIndex, cycle) => {
              const axis = self.axes[axisIndex] as uPlot.Axis & { _size?: number };
              if (cycle > 1 && axis._size !== undefined) return axis._size;
              const longest = (values ?? []).reduce((a, b) => (b.length > a.length ? b : a), '');
              return Math.ceil(textWidth(longest)) + 14;
            },
          },
        ],
        hooks: {
          setCursor: [
            (self) => {
              const { idx, left } = self.cursor;
              if (idx == null || left == null || left < 0) {
                setHover(null);
                return;
              }
              const ratio = window.devicePixelRatio || 1;
              setHover({
                index: idx,
                left: self.bbox.left / ratio + left,
                flip: left > self.bbox.width / ratio / 2,
              });
            },
          ],
          draw: [
            (self) => {
              if (!directLabels) return;
              const ratio = window.devicePixelRatio || 1;
              const labels: EndLabel[] = [];
              series.forEach((entry, index) => {
                if (self.series[index + 1]?.show === false) return;
                const last = entry.values.findLast((value) => value !== null);
                if (last == null) return;
                labels.push({
                  id: entry.id,
                  label: entry.label,
                  top: self.bbox.top / ratio + self.valToPos(last, 'y') - LABEL_HEIGHT / 2,
                });
              });
              // Labels that would overlap are left out altogether: stacked or
              // nudged labels drift away from their lines. The legend remains.
              const sorted = [...labels].sort((a, b) => a.top - b.top);
              const collide = sorted.some((label, index) => {
                const previous = sorted[index - 1];
                return previous !== undefined && label.top - previous.top < LABEL_HEIGHT;
              });
              const next = collide ? [] : labels;
              setEndLabels((current) => (JSON.stringify(current) === JSON.stringify(next) ? current : next));
            },
          ],
        },
      },
      data,
      container,
    );
    chart.current = plot;
    plotted.current = data;

    const observer = new ResizeObserver(() => {
      if (container.clientWidth > 0) plot.setSize({ width: container.clientWidth, height });
    });
    observer.observe(container);
    // Axis text is drawn on a canvas: draw again once the font has loaded.
    void document.fonts.ready.then(() => chart.current === plot && plot.redraw());

    return () => {
      observer.disconnect();
      plot.destroy();
      chart.current = null;
      setHover(null);
      setEndLabels([]);
    };
    // `structure` stands for every value the plot is built from.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [structure, view, empty]);

  // New data updates the plot in place; the frame, zoom and cursor stay.
  useEffect(() => {
    const plot = chart.current;
    const previous = plotted.current;
    if (!plot || !previous) return;
    const unchanged = previous.length === data.length && previous.every((column, index) => column === data[index]);
    if (unchanged) return;
    plot.setData(data);
    plotted.current = data;
  });

  useEffect(() => {
    series.forEach((entry, index) => {
      chart.current?.setSeries(index + 1, { show: !hidden.has(entry.id) });
    });
    // `structure` covers the list of series.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hidden, structure, view]);

  const moveCursor = (index: number | null) => {
    const plot = chart.current;
    if (!plot) return;
    const timestamp = index === null ? undefined : timestamps[index];
    if (timestamp === undefined) {
      plot.setCursor({ left: -10, top: -10 });
      return;
    }
    const ratio = window.devicePixelRatio || 1;
    plot.setCursor({ left: plot.valToPos(timestamp, 'x'), top: plot.bbox.height / ratio / 2 });
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const lastIndex = timestamps.length - 1;
    const current = hover?.index ?? lastIndex;
    const step = event.shiftKey ? 10 : 1;
    if (event.key === 'ArrowLeft') moveCursor(Math.max(0, current - step));
    else if (event.key === 'ArrowRight') moveCursor(Math.min(lastIndex, hover ? current + step : current));
    else if (event.key === 'Home') moveCursor(0);
    else if (event.key === 'End') moveCursor(lastIndex);
    else if (event.key === 'Escape' && hover) moveCursor(null);
    else return;
    event.preventDefault();
  };

  const firstTime = timestamps[0];
  const lastTime = timestamps.at(-1);
  const hoverTime = hover ? timestamps[hover.index] : undefined;

  // The table view: the same numbers, one row per sample. A row is its index.
  const tableColumns: TableColumn<number>[] = [
    {
      id: 'time',
      header: utc ? 'Time (UTC)' : 'Time',
      width: 170,
      cell: (index) => formatDateTime(timestamps[index], { utc }),
      sortValue: (index) => timestamps[index],
    },
    ...series.map((entry) => ({
      id: entry.id,
      header: entry.label,
      align: 'right' as const,
      cell: (index: number) => formatValue(entry.values[index]),
      sortValue: (index: number) => entry.values[index],
    })),
  ];
  const tableRows = useMemo(() => timestamps.map((_, index) => index), [timestamps]);

  /**
   * The chart as a picture: the plot's own canvas, as it is on screen, with
   * the title and the legend of the series that are drawn. Made when asked
   * for, from what is plotted at that moment.
   */
  const picture = (): Promise<Blob> => {
    const plot = chart.current;
    const container = plotRef.current;
    if (!plot || !container) {
      return Promise.reject(new CaptureError('Show the chart rather than the table to make a picture of it.'));
    }
    return chartToPng({
      plot: plot.ctx.canvas,
      title,
      subtitle,
      legend:
        series.length >= 2
          ? series.flatMap((entry, index) =>
              hidden.has(entry.id) ? [] : [{ label: entry.label, color: colors[index] ?? ink['--text-1'] }],
            )
          : undefined,
      note: exportMenu?.note,
      colors: { background: surfaceColor(container), text: ink['--text-1'], muted: ink['--text-3'] },
    });
  };

  const stem = fileStem(exportMenu?.filename ?? title);
  const exportItems: MenuEntry[] = exportMenu
    ? [
        {
          label: 'Save as PNG',
          icon: ImageDown,
          disabled: view !== 'chart',
          onSelect: () => void savePicture(picture(), `${stem}.png`).then(tellSaved),
        },
        {
          label: 'Copy image',
          icon: Copy,
          disabled: view !== 'chart',
          // Called inside the click, with the promise of the picture: see `copyImageOrSave`.
          onSelect: () => void copyImageOrSave(picture(), `${stem}.png`).then(tellImageCopied),
        },
        {
          label: 'Save numbers as CSV',
          icon: FileSpreadsheet,
          // Every series, hidden or not: the file is the numbers, not the view.
          onSelect: () => download(`${stem}.csv`, chartCsv({ timestamps, series }), 'text/csv;charset=utf-8'),
        },
        ...(exportMenu.extra && exportMenu.extra.length > 0
          ? [{ type: 'separator' } as const, ...exportMenu.extra]
          : []),
      ]
    : [];

  return (
    <figure className={cx(styles.chart, className)}>
      <div className={styles.header}>
        <figcaption className={styles.caption}>
          <span className={styles.title}>{title}</span>
          {subtitle && <span className={styles.subtitle}>{subtitle}</span>}
        </figcaption>
        {!empty && exportMenu && (
          <DropdownMenu
            aria-label={`Export ${title}`}
            placement="bottom-end"
            items={exportItems}
            trigger={<IconButton icon={Share} size="sm" aria-label="Export this chart" tooltipPlacement="left" />}
          />
        )}
        {!empty && (
          <IconButton
            icon={view === 'chart' ? TableIcon : ChartLine}
            size="sm"
            aria-label={view === 'chart' ? 'Show as table' : 'Show as chart'}
            tooltipPlacement="left"
            onClick={() => setView(view === 'chart' ? 'table' : 'chart')}
          />
        )}
      </div>

      {series.length >= 2 && view === 'chart' && !empty && (
        <ul className={styles.legend} aria-label="Series">
          {series.map((entry, index) => (
            <li key={entry.id}>
              <button
                type="button"
                className={styles.legendItem}
                aria-pressed={!hidden.has(entry.id)}
                title={hidden.has(entry.id) ? `Show ${entry.label}` : `Hide ${entry.label}`}
                onClick={() => {
                  const next = new Set(hidden);
                  if (!next.delete(entry.id)) next.add(entry.id);
                  setHidden(next);
                }}
              >
                {/* A line key, like the mark it stands for. The colour is on the key, never on the text. */}
                <span className={styles.key} style={{ background: colors[index] }} />
                {entry.label}
              </button>
            </li>
          ))}
          {notShown > 0 && (
            <li className={styles.overflow}>
              {formatCount(notShown)} more not shown: a chart reads up to {MAX_SERIES} series. Narrow the
              selection or split it into several charts.
            </li>
          )}
        </ul>
      )}

      {empty ? (
        <div className={styles.placeholder} style={{ height }}>
          {loading ? (
            <Skeleton shape="block" height="100%" />
          ) : (
            <EmptyState
              size="compact"
              icon={ChartLine}
              title="No data in this range"
              description="Nothing was recorded for the selected time range."
            />
          )}
        </div>
      ) : view === 'table' ? (
        <Table
          aria-label={title}
          columns={tableColumns}
          rows={tableRows}
          rowKey={String}
          maxHeight={height}
          defaultSort={{ columnId: 'time', direction: 'desc' }}
          empty={null}
        />
      ) : (
        <div
          ref={plotRef}
          className={styles.plot}
          style={{ height }}
          data-stale={loading || undefined}
          role="group"
          tabIndex={0}
          aria-label={`${title}: line chart, ${formatCount(series.length)} series, ${formatCount(timestamps.length)} points from ${formatDateTime(firstTime, { utc })} to ${formatDateTime(lastTime, { utc })}. Use the arrow keys to read values, or show it as a table.`}
          onKeyDown={onKeyDown}
          onFocus={() => {
            setFocused(true);
            if (!hover) moveCursor(timestamps.length - 1);
          }}
          onBlur={() => {
            setFocused(false);
            moveCursor(null);
          }}
        >
          {endLabels.map((label) => (
            <span key={label.id} className={styles.endLabel} style={{ top: label.top }} aria-hidden="true">
              {label.label}
            </span>
          ))}
          {hover && hoverTime !== undefined && (
            <div
              className={styles.tooltip}
              style={{ left: hover.left }}
              data-flip={hover.flip || undefined}
              // Read out while navigating with the keyboard, silent under a moving pointer.
              aria-live={focused ? 'polite' : 'off'}
            >
              <div className={styles.tooltipTime}>
                {formatDateTime(hoverTime, { utc })}
                {utc && ' UTC'}
              </div>
              {series.map((entry, index) =>
                hidden.has(entry.id) ? null : (
                  <div key={entry.id} className={styles.tooltipRow}>
                    <span className={styles.key} style={{ background: colors[index] }} />
                    {/* The value leads; the reader already knows which series. */}
                    <span className={styles.tooltipValue}>{formatValue(entry.values[hover.index])}</span>
                    <span className={styles.tooltipLabel}>{entry.label}</span>
                  </div>
                ),
              )}
            </div>
          )}
        </div>
      )}
    </figure>
  );
}
