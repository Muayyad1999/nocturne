/**
 * The lines themselves: a virtualised list that holds tens of thousands of
 * rows, oldest at the top, newest at the bottom.
 *
 * **The tail sticks to the bottom until the reader scrolls up, and then stops
 * fighting them.** That is the whole behaviour contract. Pinning is derived
 * from how far the viewport is from the bottom and which way the last scroll
 * went (`scrollVerdict`): scrolling up unpins, scrolling back down pins again,
 * and the list never yanks a reader who is in the middle of something back to
 * the end. The direction matters because a scroll event is read when it is
 * dispatched: the list's own jump to its end, read after the tab above it
 * grew, looks like "not at the end" and must not unpin a tail nobody touched.
 * While unpinned the arrived lines still go into the view; they just wait
 * there.
 *
 * Every row is exactly one line high and the widths are in `ch` of the
 * monospace face, so the columns line up and nothing reflows as the list fills.
 * A line too long for the row is clipped with an ellipsis and read in full in
 * the drawer, because wrapping would make every row a different height and the
 * virtualiser's arithmetic a guess.
 */

import { useVirtualizer } from '@tanstack/react-virtual';
import { ArrowDown, ChevronsUp, Scissors } from 'lucide-react';
import {
  useEffect,
  useEffectEvent,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import type { LogEntry } from '@/api/types/core';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { formatCount } from '@/lib/format';
import { formatTime } from '@/lib/time';
import { PAGE } from './data';
import { clockOf, interfaceOf, severityCell, tagOf } from './entry';
import styles from './Logs.module.css';
import { bandOf, severityName } from './query';
import { AT_END_PX, scrollVerdict, type Row, type Stream } from './stream';

/** One line, one row. Must match `--log-row` in the stylesheet. */
const ROW = 22;

/** The short word for a severity in a dense row: `err`, `warn`, `dbg`. */
const SHORT: Record<number, string> = {
  0: 'emrg',
  1: 'alrt',
  2: 'crit',
  3: 'err',
  4: 'warn',
  5: 'note',
  6: 'info',
  7: 'dbg',
};

export interface LogListProps {
  stream: Stream;
  /** The live tail is on: the list should sit at the bottom to begin with. */
  tail: boolean;
  /** What the middle column holds for the stream on screen: `Mnemonic`, `Notification`, … */
  tagHeader: string;
  /** Device id → name, for the device column. */
  nameOf: (id: string | null | undefined) => string;
  /** The id of the row the drawer is showing, if any. */
  openId: number | null;
  onOpen: (entry: LogEntry) => void;
  hasOlder: boolean;
  loadingOlder: boolean;
  loadOlder: () => void;
  /** Whether the list is following the bottom, reported for the toolbar. */
  onPinnedChange: (pinned: boolean) => void;
  /** Shown instead of the rows when there are none. */
  empty: ReactNode;
}

export function LogList({
  stream,
  tail,
  tagHeader,
  nameOf,
  openId,
  onOpen,
  hasOlder,
  loadingOlder,
  loadOlder,
  onPinnedChange,
  empty,
}: LogListProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const [pinned, setPinned] = useState(true);
  const { rows } = stream;

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller.current,
    estimateSize: () => ROW,
    overscan: 24,
  });

  // Prepending older lines grows the list upwards. Without this the content
  // under the reader's eyes jumps by however many rows arrived.
  const previous = useRef<{ length: number; first: string | null }>({ length: 0, first: null });
  useLayoutEffect(() => {
    const before = previous.current;
    const first = rows[0]?.key ?? null;
    previous.current = { length: rows.length, first };
    const added = rows.length - before.length;
    if (before.first !== null && first !== before.first && added > 0 && scroller.current) {
      scroller.current.scrollTop += added * ROW;
    }
  }, [rows]);

  /**
   * Keep the view and the pin in agreement.
   *
   * Pinned: sit at the bottom — the scroll event that follows measures zero
   * distance, so nothing has to know the scroll was ours. Unpinned: measure,
   * because the content above the list can change height (a gap note
   * appears, a panel collapses) without anyone scrolling, and a list that is
   * at its end must not go on offering to take the reader there.
   */
  const pinnedRef = useRef(pinned);
  pinnedRef.current = pinned;
  const sync = useEffectEvent(() => {
    const element = scroller.current;
    if (!element) return;
    if (pinnedRef.current) {
      element.scrollTop = element.scrollHeight;
      return;
    }
    const distance = element.scrollHeight - element.scrollTop - element.clientHeight;
    if (distance <= AT_END_PX) setPinned(true);
  });

  useEffect(() => sync(), [rows.length, pinned]);

  // The list's own box changes size with the window, the dock and whatever is
  // above it; both halves of `sync` care.
  useEffect(() => {
    const element = scroller.current;
    if (!element || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => sync());
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  // Switching the tail on means "take me to the end".
  useEffect(() => {
    if (tail) setPinned(true);
  }, [tail]);

  useEffect(() => onPinnedChange(pinned), [pinned, onPinnedChange]);

  /** Where the list was at the previous scroll event: which way a scroll went. */
  const lastTop = useRef(0);
  const onScroll = () => {
    const element = scroller.current;
    if (!element) return;
    const verdict = scrollVerdict({
      distance: element.scrollHeight - element.scrollTop - element.clientHeight,
      top: element.scrollTop,
      lastTop: lastTop.current,
      pinned: pinnedRef.current,
    });
    lastTop.current = element.scrollTop;
    if (verdict === 'pin') setPinned(true);
    else if (verdict === 'unpin') setPinned(false);
    else if (verdict === 'restick') element.scrollTop = element.scrollHeight;
    if (element.scrollTop === 0 && hasOlder && !loadingOlder) loadOlder();
  };

  const toBottom = () => {
    setPinned(true);
    const element = scroller.current;
    if (element) element.scrollTop = element.scrollHeight;
  };

  return (
    <div className={styles.listFrame} data-empty={rows.length === 0 || undefined}>
      <div className={styles.listHead} aria-hidden="true">
        <span className={styles.colTime}>Received</span>
        <span className={styles.colSeverity}>Sev</span>
        <span className={styles.colDevice}>Device</span>
        <span className={styles.colTag}>{tagHeader}</span>
        <span className={styles.colMessage}>Message</span>
      </div>

      <div className={styles.scroller} ref={scroller} onScroll={onScroll} data-testid="log-scroller">
        {rows.length === 0 ? (
          <div className={styles.listEmpty}>{empty}</div>
        ) : (
          <>
            {hasOlder && (
              <div className={styles.older}>
                <Button size="sm" variant="ghost" icon={ChevronsUp} loading={loadingOlder} onClick={loadOlder}>
                  Load {formatCount(PAGE)} older lines
                </Button>
              </div>
            )}
            <div
              className={styles.rows}
              style={{ height: virtualizer.getTotalSize() }}
              role="list"
              aria-label="Log lines"
            >
              {virtualizer.getVirtualItems().map((item) => {
                const row = rows[item.index];
                if (!row) return null;
                return (
                  <div
                    key={row.key}
                    role="listitem"
                    className={styles.rowSlot}
                    style={{ transform: `translateY(${item.start}px)` }}
                  >
                    {row.kind === 'entry' ? (
                      <LineRow
                        row={row}
                        nameOf={nameOf}
                        open={row.entry.id === openId}
                        onOpen={onOpen}
                      />
                    ) : (
                      <MissingNote row={row} />
                    )}
                  </div>
                );
              })}
            </div>
          </>
        )}
      </div>

      {!pinned && rows.length > 0 && (
        <div className={styles.jump}>
          <Button size="sm" icon={ArrowDown} onClick={toBottom}>
            Jump to the newest line
          </Button>
        </div>
      )}
    </div>
  );
}

interface LineRowProps {
  row: Extract<Row, { kind: 'entry' }>;
  nameOf: LogListProps['nameOf'];
  open: boolean;
  onOpen: LogListProps['onOpen'];
}

function LineRow({ row, nameOf, open, onOpen }: LineRowProps) {
  const { entry } = row;
  const clock = clockOf(entry);
  const band = bandOf(entry.severity);
  const device = entry.device_id == null ? null : nameOf(entry.device_id);
  const port = interfaceOf(entry);
  const severity = severityCell(entry);
  return (
    <button
      type="button"
      className={styles.row}
      data-band={band}
      data-stream={entry.stream}
      data-open={open || undefined}
      onClick={() => onOpen(entry)}
      title={clock.note}
    >
      <span className={styles.colTime} data-clock={clock.shown}>
        {formatTime(entry.ts)}
        {clock.shown === 'device' && <span className={styles.clockMark}>d</span>}
      </span>
      {/* Three different things: a severity, a line that gave none (a dash),
          and a trap, which has none to give (nothing at all). */}
      <span
        className={styles.colSeverity}
        data-band={band}
        data-severity={severity.kind}
        title={severity.kind === 'none' ? 'A trap carries no severity' : undefined}
      >
        {severity.kind === 'given'
          ? (SHORT[severity.severity ?? -1] ?? severityName(severity.severity))
          : severity.kind === 'unsaid'
            ? '—'
            : ''}
      </span>
      <span className={styles.colDevice} data-unattributed={device === null || undefined}>
        {device ?? entry.source}
      </span>
      <span className={styles.colTag}>{tagOf(entry)}</span>
      <span className={styles.colMessage}>
        {port && <span className={styles.port}>{port}</span>}
        {entry.message}
      </span>
    </button>
  );
}

function MissingNote({ row }: { row: Extract<Row, { kind: 'missing' }> }) {
  return (
    <p className={styles.missing}>
      <Icon icon={Scissors} size={12} />
      {row.reason === 'sampled'
        ? `${formatCount(row.count)} lines are not shown here: the tail sampled them away. Search this range to read them all.`
        : `${formatCount(row.count)} older lines have left this view. They are still stored — search or page back for them.`}
    </p>
  );
}
