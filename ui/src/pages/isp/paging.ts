/**
 * Paging that continues after a row (`docs/dev/features/isp.md`, decision
 * 20).
 *
 * The invoices, payments and credit notes are listed newest first, so a
 * billing run's drafts or a payment recorded while somebody reads the list
 * sort in front of the page they have already seen; a page asked for by a
 * count of rows would hand part of that page back. The customers are listed
 * by name, and one deleted from the first page would make a count skip a row.
 * So a page begins after the last row of the one before: the engine's
 * `next_cursor` for the customers and the usage report, and the last row's
 * `id` for the three lists that are arrays.
 *
 * A cursor only goes forwards, so the screen keeps the cursors of the pages
 * it walked — the last one is the page on screen — and Previous goes back
 * exactly one. Where a page is in the list is how many pages came before it.
 * No React here except the one hook.
 */

import { useState } from 'react';

/** The cursors of the pages walked so far, as the screen holds them. */
export type Walked = readonly string[];

/** The page after this one. */
export function forward(walked: Walked, cursor: string): string[] {
  return [...walked, cursor];
}

/** The page before this one; the first page stays the first. */
export function back(walked: Walked): string[] {
  return walked.slice(0, -1);
}

/** Where the first row of the page on screen is in the list, counting from one. */
export function firstRow(walked: Walked, pageSize: number): number {
  return walked.length * pageSize + 1;
}

/**
 * The cursor that follows a page of a list the engine sends as an array: its
 * last row's id, when the page is full. Such a list says there may be more by
 * being full, so a page that is not is the last one.
 */
export function cursorAfter(rows: readonly { id: string }[], pageSize: number): string | null {
  if (rows.length < pageSize) return null;
  return rows.at(-1)?.id ?? null;
}

/** `{cursor}` for a request, or nothing on the first page. */
export function cursorParam(walked: Walked): { cursor?: string } {
  const cursor = walked.at(-1);
  return cursor === undefined ? {} : { cursor };
}

export interface Pages {
  walked: Walked;
  /** Where the first row of the page on screen is in the list, from one. */
  first: number;
  /** Whether there is a page before this one. */
  hasPrevious: boolean;
  next: (cursor: string) => void;
  previous: () => void;
  /** Back to the first page: after a filter changed, the cursors mean nothing. */
  reset: () => void;
}

export function usePages(pageSize: number): Pages {
  const [walked, setWalked] = useState<string[]>([]);
  return {
    walked,
    first: firstRow(walked, pageSize),
    hasPrevious: walked.length > 0,
    next: (cursor) => setWalked((current) => forward(current, cursor)),
    previous: () => setWalked((current) => back(current)),
    reset: () => setWalked((current) => (current.length === 0 ? current : [])),
  };
}
