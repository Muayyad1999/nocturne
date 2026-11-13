import { cx } from '@/lib/cx';
import type { StatusTone } from '../tone';
import styles from './Sparkline.module.css';

export interface SparklineProps {
  /** The series, oldest first. `null` is a gap: a sample that is missing stays missing. */
  values: readonly (number | null)[];
  /**
   * What the line shows, in words, for assistive technology and as the hover
   * title: "CPU over the last hour: 42% now, between 31% and 67%".
   */
  label: string;
  width?: number;
  height?: number;
  /** A fixed value range, such as `[0, 100]` for a percentage. Defaults to the data's own range. */
  domain?: readonly [min: number, max: number];
  /**
   * Colour of the marker on the latest value. `accent` by default; a status
   * tone only when the level itself is good or bad news.
   */
  tone?: 'accent' | StatusTone;
  /** Adds a faint wash under the line. */
  area?: boolean;
  className?: string;
}

/** Room around the plot so the line and the marker are never clipped. */
const PAD = 3;

interface Point {
  x: number;
  y: number;
}

/**
 * A trend at a glance, small enough for a table cell or a Stat. The history
 * is drawn quietly and only the latest value is marked, because "where is it
 * now" is the question a sparkline answers. It has no axes: put the number
 * next to it.
 */
export function Sparkline({
  values,
  label,
  width = 80,
  height = 24,
  domain,
  tone = 'accent',
  area = false,
  className,
}: SparklineProps) {
  const known = values.filter((value): value is number => value !== null && Number.isFinite(value));
  const low = domain?.[0] ?? Math.min(...known);
  const high = domain?.[1] ?? Math.max(...known);
  const span = high - low;
  const innerWidth = width - 2 * PAD;
  const innerHeight = height - 2 * PAD;

  const point = (value: number, index: number): Point => ({
    x: PAD + (values.length > 1 ? (index / (values.length - 1)) * innerWidth : innerWidth),
    // A flat series sits on the middle line rather than on an edge.
    y: PAD + (span > 0 ? (1 - (Math.min(high, Math.max(low, value)) - low) / span) * innerHeight : innerHeight / 2),
  });

  // One run of points per stretch of consecutive samples; a gap starts a new run.
  const runs: Point[][] = [];
  let run: Point[] = [];
  values.forEach((value, index) => {
    if (value === null || !Number.isFinite(value)) {
      if (run.length > 0) runs.push(run);
      run = [];
    } else {
      run.push(point(value, index));
    }
  });
  if (run.length > 0) runs.push(run);

  const path = (points: Point[]) =>
    points.map(({ x, y }, index) => `${index === 0 ? 'M' : 'L'}${x.toFixed(1)} ${y.toFixed(1)}`).join('');
  const last = runs.at(-1)?.at(-1);
  const baseline = height - PAD;

  return (
    <svg
      className={cx(styles.sparkline, className)}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={known.length > 0 ? label : `${label}: no data`}
      data-tone={tone}
    >
      <title>{known.length > 0 ? label : `${label}: no data`}</title>
      {area &&
        runs.map((points, index) => {
          const first = points[0];
          const end = points.at(-1);
          if (!first || !end || points.length < 2) return null;
          return (
            <path
              key={index}
              className={styles.area}
              d={`${path(points)}L${end.x.toFixed(1)} ${baseline}L${first.x.toFixed(1)} ${baseline}Z`}
            />
          );
        })}
      {runs.map((points, index) => (
        <path key={index} className={styles.line} d={path(points)} />
      ))}
      {last && <circle className={styles.marker} cx={last.x} cy={last.y} r="2.5" />}
    </svg>
  );
}
