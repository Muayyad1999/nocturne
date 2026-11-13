import { compareNatural } from '@/lib/net';

export type SortDirection = 'asc' | 'desc';

export interface TableSort {
  columnId: string;
  direction: SortDirection;
}

export type SortValue = string | number | null | undefined;

/** What sorting needs to know about a column. */
export interface SortableColumn<T> {
  id: string;
  sortValue?: (row: T) => SortValue;
  compare?: (a: T, b: T) => number;
}

function isMissing(value: SortValue): value is null | undefined {
  return value == null || (typeof value === 'number' && Number.isNaN(value));
}

/**
 * Orders two cell values: numbers numerically, everything else naturally, so
 * `Gi1/0/2` comes before `Gi1/0/10` and `10.0.0.9` before `10.0.0.10`.
 */
export function compareSortValues(a: string | number, b: string | number): number {
  if (typeof a === 'number' && typeof b === 'number') return a - b;
  return compareNatural(String(a), String(b));
}

/** Whether a column can be sorted at all. */
export function isSortable<T>(column: SortableColumn<T>): boolean {
  return Boolean(column.sortValue ?? column.compare);
}

/**
 * Returns the rows in sorted order (a new array; the input is untouched).
 * Rows with a missing value go last in both directions: flipping the order
 * should not bury the data under a page of dashes.
 */
export function sortRows<T>(
  rows: readonly T[],
  columns: readonly SortableColumn<T>[],
  sort: TableSort | null,
): readonly T[] {
  const column = sort && columns.find((candidate) => candidate.id === sort.columnId);
  if (!sort || !column || !isSortable(column)) return rows;
  const sign = sort.direction === 'asc' ? 1 : -1;
  const { compare, sortValue } = column;

  return [...rows].sort((a, b) => {
    if (compare) return sign * compare(a, b);
    const left = sortValue?.(a);
    const right = sortValue?.(b);
    const leftMissing = isMissing(left);
    const rightMissing = isMissing(right);
    if (leftMissing || rightMissing) return Number(leftMissing) - Number(rightMissing);
    return sign * compareSortValues(left, right);
  });
}

/** The next state when a column's header is pressed: ascending, descending, then unsorted. */
export function nextSort(current: TableSort | null, columnId: string): TableSort | null {
  if (current?.columnId !== columnId) return { columnId, direction: 'asc' };
  return current.direction === 'asc' ? { columnId, direction: 'desc' } : null;
}
