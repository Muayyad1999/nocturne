/**
 * How much the network said, when, and how severe it was.
 *
 * Stacked bars, one per bucket, in four severity bands plus the lines that
 * never said how severe they were. Three decisions are worth the words:
 *
 * * **A bucket that cannot be proved empty is drawn as a gap**, not as a zero
 *   — hatched, at full height, and counted in a sentence underneath.
 *   `coverage.ts` decides which is which; the chart only draws it. This follows
 *   `pages/monitoring`, where a missing sample lifts the pen and the line under
 *   the chart says how much is missing.
 * * **What the severity filter hides is drawn as an outline**, in the band's
 *   own colour, rather than left out. A filter that silently shrinks the
 *   picture teaches the wrong thing about the network.
 * * **It is a row of buttons, not a canvas.** One is in the tab order and the
 *   arrow keys move along them, as in the design system's chart; pressing one
 *   narrows the range to that bucket, which is how a spike is read.
 *
 * It is drawn with flex boxes and design tokens rather than SVG: the bars are
 * rectangles, the widths are proportions of the container, and nothing here
 * needs to measure the viewport to stay crisp at any width.
 */

import { Unplug } from 'lucide-react';
import { useRef, useState, type KeyboardEvent } from 'react';
import { Icon } from '@/design/Icon';
import { Skeleton } from '@/design/Skeleton';
import { formatCount } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { gapNote, type Bar, type HistogramView, type Window } from './coverage';
import styles from './Logs.module.css';
import { SEVERITY_BANDS, severityPhrase, timeLabel } from './query';

const REACH_WORD: Record<Bar['reach'], string> = {
  counted: '',
  future: 'not yet',
  'before-store': 'nothing stored this far back',
  'not-listening': 'not listening',
};

export interface HistogramProps {
  view: HistogramView;
  window: Window;
  since: number;
  until: number;
  spanMs: number;
  maxSeverity: number | undefined;
  loading: boolean;
  /** Narrow the range to one bucket. */
  onPick: (from: number, until: number) => void;
}

function barTitle(bar: Bar, spanMs: number): string {
  const when = `${timeLabel(bar.t, spanMs)}–${timeLabel(bar.until, spanMs)}`;
  if (bar.reach !== 'counted') {
    return `${when}: ${REACH_WORD[bar.reach]} — this bucket is a gap, not a zero`;
  }
  if (bar.total === 0) return `${when}: no line matched`;
  const bands = bar.segments
    .map((segment) => {
      const band = SEVERITY_BANDS.find((entry) => entry.id === segment.band);
      const hidden = segment.hidden > 0 ? ` (${formatCount(segment.hidden)} hidden by the filter)` : '';
      return `${formatCount(segment.count)} ${band?.label.toLowerCase() ?? segment.band}${hidden}`;
    })
    .join(', ');
  return `${when}: ${formatCount(bar.total)} lines — ${bands}`;
}

export function Histogram({
  view,
  window,
  since,
  until,
  spanMs,
  maxSeverity,
  loading,
  onPick,
}: HistogramProps) {
  const [focus, setFocus] = useState(0);
  const plot = useRef<HTMLDivElement>(null);

  const move = (event: KeyboardEvent<HTMLDivElement>) => {
    const step =
      event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : event.key === 'Home' ? -view.bars.length : event.key === 'End' ? view.bars.length : 0;
    if (step === 0) return;
    event.preventDefault();
    const next = Math.min(view.bars.length - 1, Math.max(0, focus + step));
    setFocus(next);
    plot.current?.querySelectorAll<HTMLButtonElement>('[data-bar]')[next]?.focus();
  };

  const note = gapNote(view, window);
  const { counts } = view;
  const hiding = maxSeverity !== undefined && counts.total > counts.shown;

  return (
    <div className={styles.chart}>
      <div className={styles.chartHead}>
        <span className={styles.peak}>
          {view.peak === 0 ? '' : `peak ${formatCount(view.peak)} / bucket`}
        </span>
        <ul className={styles.legend}>
          {SEVERITY_BANDS.map((band) => (
            <li key={band.id}>
              <span className={styles.swatch} data-tone={band.tone} data-band={band.id} />
              {band.label}
            </li>
          ))}
          <li>
            <span className={styles.swatch} data-band="gap" />
            Not known
          </li>
        </ul>
      </div>

      {loading ? (
        <Skeleton shape="block" height={104} />
      ) : (
        <div
          className={styles.plot}
          ref={plot}
          role="group"
          aria-label={`Lines per ${Math.round(view.width / 1000)} seconds, ${formatCount(counts.total)} in the range`}
          onKeyDown={move}
        >
          {view.bars.map((bar, index) => (
            <button
              key={bar.t}
              type="button"
              data-bar=""
              data-reach={bar.reach}
              className={styles.bar}
              tabIndex={index === focus ? 0 : -1}
              onFocus={() => setFocus(index)}
              onClick={() => onPick(bar.t, bar.until)}
              title={barTitle(bar, spanMs)}
              aria-label={barTitle(bar, spanMs)}
            >
              {bar.reach === 'counted' && bar.total === 0 ? (
                <span className={styles.zero} />
              ) : bar.reach === 'counted' ? (
                <span
                  className={styles.stack}
                  style={{ height: `${view.peak === 0 ? 0 : (bar.total / view.peak) * 100}%` }}
                >
                  {bar.segments.map((segment) => (
                    <span key={segment.band} className={styles.segment} style={{ flexGrow: segment.count }}>
                      {segment.kept > 0 && (
                        <span
                          className={styles.fill}
                          data-band={segment.band}
                          style={{ flexGrow: segment.kept }}
                        />
                      )}
                      {segment.hidden > 0 && (
                        <span
                          className={styles.fill}
                          data-band={segment.band}
                          data-hidden=""
                          style={{ flexGrow: segment.hidden }}
                        />
                      )}
                    </span>
                  ))}
                </span>
              ) : (
                <span className={styles.gap} data-reach={bar.reach} />
              )}
            </button>
          ))}
        </div>
      )}

      <div className={styles.axis} aria-hidden="true">
        <span>{timeLabel(since, spanMs)}</span>
        <span>{timeLabel((since + until) / 2, spanMs)}</span>
        <span>{timeLabel(until, spanMs)}</span>
      </div>

      <p className={styles.counts} data-testid="log-counts">
        <span title={`${formatTimestamp(since)} to ${formatTimestamp(until)}`}>
          {formatCount(counts.total)} {counts.total === 1 ? 'line' : 'lines'} in this range
        </span>
        {hiding && (
          <>
            {' · '}
            <strong>{formatCount(counts.shown)} shown</strong> ({severityPhrase(maxSeverity)}).{' '}
            {counts.lessSevere > 0 && `${formatCount(counts.lessSevere)} less severe`}
            {counts.lessSevere > 0 && counts.noSeverity > 0 && ' and '}
            {counts.noSeverity > 0 &&
              `${formatCount(counts.noSeverity)} that never said how severe ${counts.noSeverity === 1 ? 'it was' : 'they were'}`}
            {' are hidden by the severity filter.'}
          </>
        )}
      </p>

      {note && (
        <p className={styles.gapNote} data-testid="log-gaps">
          <Icon icon={Unplug} size={12} />
          {note}
        </p>
      )}
    </div>
  );
}
