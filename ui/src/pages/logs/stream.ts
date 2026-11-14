/**
 * The list's own model: a page of stored lines, older pages in front of it, a
 * live tail behind it, and what to say where lines are missing.
 *
 * `logs.query` answers newest first, because that is what paging back wants.
 * A list that sticks to the bottom wants the opposite, so everything here is
 * **ascending** — oldest at the top, newest at the bottom, like `tail -f`.
 *
 * Three things the backend does make this more than an array:
 *
 * * **The tail samples.** Over two hundred lines in a quarter of a second it
 *   sends an even sample and says how many it left out (`dropped`, `sampled`).
 *   Dropping that number on the floor would make a storm look like a trickle,
 *   so it becomes a row of its own between the lines it stands for.
 * * **Events overlap the page.** A batch can carry a line the first page
 *   already had; ids are the only reliable identity, and they only ever grow.
 * * **The view is bounded.** The store holds millions; the browser holds
 *   {@link CAP}. When the tail pushes past it the oldest rows leave the view —
 *   and the row that says so stays, because they are still in the store.
 */

import type { LogEntry } from '@/api/types/core';
import type { LogsAppended } from '@/api/types/logs';

/** How many rows the view keeps. Beyond this the oldest leave the window. */
export const CAP = 20_000;

export interface EntryRow {
  kind: 'entry';
  /** The entry's own id, stable across pages and batches. */
  key: string;
  entry: LogEntry;
}

export interface MissingRow {
  kind: 'missing';
  key: string;
  /** How many lines are not here. */
  count: number;
  /** Why they are not: the tail sampled, or the view ran out of room. */
  reason: 'sampled' | 'trimmed';
  /** Where in time the hole is. */
  at: number;
}

export type Row = EntryRow | MissingRow;

export interface Stream {
  rows: Row[];
  /** The smallest id in the view, for `before_id` paging. */
  oldest: number | null;
  /** The largest id in the view, so a batch cannot add a line twice. */
  newest: number | null;
  /** Lines the tail sampled away while this view has been open. */
  sampled: number;
  /** Rows pushed out of the front of the view. */
  trimmed: number;
}

export const EMPTY_STREAM: Stream = {
  rows: [],
  oldest: null,
  newest: null,
  sampled: 0,
  trimmed: 0,
};

function entryRow(entry: LogEntry): EntryRow {
  return { kind: 'entry', key: `e${entry.id}`, entry };
}

function extent(rows: readonly Row[]): { oldest: number | null; newest: number | null } {
  let oldest: number | null = null;
  let newest: number | null = null;
  for (const row of rows) {
    if (row.kind !== 'entry') continue;
    const { id } = row.entry;
    if (oldest === null || id < oldest) oldest = id;
    if (newest === null || id > newest) newest = id;
  }
  return { oldest, newest };
}

/** A fresh view from one `logs.query` page (newest first). */
export function fromPage(entries: readonly LogEntry[]): Stream {
  const rows = [...entries].reverse().map(entryRow);
  return { ...EMPTY_STREAM, rows, ...extent(rows) };
}

/**
 * A newly read page, with anything the tail delivered while it was being read
 * kept on the end.
 *
 * A page is a snapshot of the store at the moment the query ran. Lines that
 * arrive between the request and its answer are in neither — not in the page,
 * because they were not stored yet, and not in the new view, if reading a page
 * simply replaced it. On a busy network that is a hole in the live tail every
 * time the newest page is read.
 */
export function adopt(current: Stream, entries: readonly LogEntry[]): Stream {
  const fresh = fromPage(entries);
  if (fresh.newest === null) return fresh;
  const newer = current.rows.filter(
    (row) => row.kind === 'entry' && row.entry.id > fresh.newest!,
  );
  if (newer.length === 0) return fresh;
  const rows = [...fresh.rows, ...newer];
  return { ...fresh, rows, ...extent(rows) };
}

/**
 * An older page, in front of what is already there. `has_more` is the caller's
 * business; this only joins them up, and never lets a line in twice.
 */
export function prepend(stream: Stream, entries: readonly LogEntry[]): Stream {
  const known = new Set(stream.rows.filter((row) => row.kind === 'entry').map((row) => row.key));
  const older = [...entries]
    .reverse()
    .map(entryRow)
    .filter((row) => !known.has(row.key));
  if (older.length === 0) return stream;
  const rows = [...older, ...stream.rows];
  return { ...stream, rows, ...extent(rows) };
}

/**
 * A tail batch, behind what is already there.
 *
 * Returns the same object when the batch adds nothing, so React can skip the
 * work: an idle tail still delivers an event whenever any other filter's lines
 * arrive.
 */
export function append(stream: Stream, batch: LogsAppended, cap = CAP): Stream {
  const fresh = batch.entries
    .filter((entry) => stream.newest === null || entry.id > stream.newest)
    .sort((a, b) => a.id - b.id);
  const left = batch.dropped;
  if (fresh.length === 0 && left === 0) return stream;

  const added: Row[] = [];
  if (left > 0) {
    // The hole belongs between the lines it stands for: the sample's own
    // window started before the first line in this batch.
    added.push({
      kind: 'missing',
      key: `s${fresh[0]?.id ?? stream.newest ?? 0}-${left}`,
      count: left,
      reason: 'sampled',
      at: fresh[0]?.ts ?? Date.now(),
    });
  }
  added.push(...fresh.map(entryRow));

  let rows = [...stream.rows, ...added];
  let trimmed = stream.trimmed;
  if (rows.length > cap) {
    const cut = rows.length - cap;
    const dropped = rows.slice(0, cut).filter((row) => row.kind === 'entry').length;
    trimmed += dropped;
    rows = rows.slice(cut);
    // One row in front of the window says what is no longer in it. It
    // replaces itself rather than piling up.
    if (trimmed > 0) {
      const first = rows[0];
      rows = [
        {
          kind: 'missing',
          key: 'trimmed',
          count: trimmed,
          reason: 'trimmed',
          at: first?.kind === 'entry' ? first.entry.ts : Date.now(),
        },
        ...rows.filter((row) => !(row.kind === 'missing' && row.reason === 'trimmed')),
      ];
    }
  }
  return {
    rows,
    ...extent(rows),
    sampled: stream.sampled + left,
    trimmed,
  };
}

/** How many entries (not marker rows) the view holds. */
export function entryCount(stream: Stream): number {
  return stream.rows.reduce((count, row) => count + (row.kind === 'entry' ? 1 : 0), 0);
}
