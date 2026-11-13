import { useVirtualizer } from '@tanstack/react-virtual';
import { ArrowDown, ArrowUp, ChevronsUpDown, Columns3, Ellipsis } from 'lucide-react';
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type RefObject,
} from 'react';
import { cx } from '@/lib/cx';
import { readStored, storageKey, writeStored } from '@/lib/storage';
import { IconButton } from '../Button';
import { Checkbox } from '../Checkbox';
import { contextMenuPoint } from '../ContextMenu';
import { DropdownMenu } from '../DropdownMenu';
import { Icon } from '../Icon';
import { MenuPopup, type MenuEntry } from '../Menu';
import type { AnchorPoint } from '../overlay/useFloating';
import { Skeleton } from '../Skeleton';
import { isSortable, nextSort, sortRows, type SortValue, type TableSort } from './sort';
import styles from './Table.module.css';

export interface TableColumn<T> {
  /** Stable id: the key for remembered sort and column choices. */
  id: string;
  /** The column heading. */
  header: string;
  cell: (row: T) => ReactNode;
  align?: 'left' | 'right' | 'center';
  /** Width in pixels. Columns without one share the remaining space. */
  width?: number;
  /**
   * Makes the column sortable by this value. Numbers sort numerically, text
   * naturally (`Gi1/0/2` before `Gi1/0/10`); missing values go last.
   */
  sortValue?: (row: T) => SortValue;
  /** Makes the column sortable with a custom order, e.g. `compareIp` for addresses. */
  compare?: (a: T, b: T) => number;
  /** Monospace: anything a device said, such as addresses and interface names. */
  mono?: boolean;
  /** Always shown: left out of the column chooser. Use it for the name column. */
  required?: boolean;
  /** Hidden until the user turns it on in the column chooser. */
  defaultHidden?: boolean;
}

export interface TableSelection {
  /** Keys (`rowKey`) of the selected rows. */
  selected: ReadonlySet<string>;
  onChange: (selected: Set<string>) => void;
}

export interface TableProps<T> {
  columns: readonly TableColumn<T>[];
  rows: readonly T[];
  /** A stable, unique key for a row: its id. */
  rowKey: (row: T) => string;
  /** Names the table for assistive technology. */
  'aria-label': string;
  /**
   * What to show when there are no rows: an EmptyState that says what the
   * table is for and offers the action that fills it.
   */
  empty: ReactNode;
  /** Remembers sort and column choices under this id (`localStorage`), and offers the column chooser. */
  id?: string;
  /** `compact` (28px rows) for scanning many rows, `comfortable` (36px) otherwise. */
  density?: 'compact' | 'comfortable';
  /** Shows skeleton rows while the first load is in flight; later loads keep the rows on screen. */
  loading?: boolean;
  /** The order before the user chooses one. */
  defaultSort?: TableSort;
  /** Adds a checkbox column. Space toggles the focused row; Shift+click selects a range. */
  selection?: TableSelection;
  /** Enter or a double-click on a row: open it. */
  onRowActivate?: (row: T) => void;
  /** Actions for one row, offered both by the button at the row's end and as its context menu. */
  rowActions?: (row: T) => readonly MenuEntry[];
  /** Fills the height of its parent and scrolls inside. The parent needs a height. */
  fill?: boolean;
  /** Caps the height (a number is pixels); the table then scrolls inside. */
  maxHeight?: number | string;
  className?: string;
}

/** Above this many rows only the visible ones are rendered. */
const VIRTUALIZE_ABOVE = 200;
const HEADER_HEIGHT = 32;
const ROW_HEIGHT = { compact: 28, comfortable: 36 } as const;
const CHECK_COLUMN_WIDTH = 36;
const ACTIONS_COLUMN_WIDTH = 40;
const MIN_FLEXIBLE_WIDTH = 120;
const SKELETON_ROWS = 8;
const PAGE_ROWS = 10;

interface StoredState {
  sort: TableSort | null;
  hidden: string[];
}

function isStoredState(value: unknown): value is StoredState {
  if (typeof value !== 'object' || value === null) return false;
  const { sort, hidden } = value as Partial<StoredState>;
  const sortOk =
    sort === null ||
    (typeof sort === 'object' &&
      typeof sort.columnId === 'string' &&
      (sort.direction === 'asc' || sort.direction === 'desc'));
  return sortOk && Array.isArray(hidden) && hidden.every((entry) => typeof entry === 'string');
}

interface RowMenu<T> {
  row: T;
  anchor: RefObject<Element | null> | AnchorPoint;
}

/**
 * The data table: sticky header, sortable columns, row selection, keyboard
 * navigation, a row actions menu, and loading and empty states. Rows have a
 * fixed height and long cells are cut with an ellipsis, so the grid stays
 * scannable; above 200 rows only the visible ones are in the DOM.
 */
export function Table<T>({
  columns,
  rows,
  rowKey,
  empty,
  id,
  density = 'compact',
  loading = false,
  defaultSort,
  selection,
  onRowActivate,
  rowActions,
  fill = false,
  maxHeight,
  className,
  ...rest
}: TableProps<T>) {
  const storeKey = id ? storageKey(`table.${id}`) : null;
  const [stored] = useState(() => (storeKey ? readStored(storeKey, isStoredState) : undefined));
  const [sort, setSort] = useState<TableSort | null>(stored ? stored.sort : (defaultSort ?? null));
  const [hidden, setHidden] = useState<ReadonlySet<string>>(
    () => new Set(stored?.hidden ?? columns.filter((column) => column.defaultHidden).map((column) => column.id)),
  );
  const remember = (next: StoredState) => {
    if (storeKey) writeStored(storeKey, next);
  };

  const visibleColumns = columns.filter((column) => column.required || !hidden.has(column.id));
  const sortedRows = useMemo(() => sortRows(rows, columns, sort), [rows, columns, sort]);
  const keys = useMemo(() => sortedRows.map(rowKey), [sortedRows, rowKey]);

  // Virtualisation: only the rows in view (plus a margin) are rendered.
  const scrollRef = useRef<HTMLDivElement>(null);
  const rowHeight = ROW_HEIGHT[density];
  const virtual = sortedRows.length > VIRTUALIZE_ABOVE;
  const virtualizer = useVirtualizer({
    count: sortedRows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => rowHeight,
    overscan: 12,
    scrollMargin: HEADER_HEIGHT,
    enabled: virtual,
  });
  const virtualItems = virtual ? virtualizer.getVirtualItems() : [];
  const firstIndex = virtual ? (virtualItems[0]?.index ?? 0) : 0;
  const lastIndex = virtual ? (virtualItems.at(-1)?.index ?? -1) : sortedRows.length - 1;

  // One row is in the tab order; arrow keys move between rows.
  const [focusedKey, setFocusedKey] = useState<string | null>(null);
  const tabStop = focusedKey !== null && keys.includes(focusedKey) ? focusedKey : keys[0];
  const pendingFocus = useRef<string | null>(null);
  useEffect(() => {
    const key = pendingFocus.current;
    if (key === null) return;
    const row = scrollRef.current?.querySelector<HTMLElement>(`[data-row="${CSS.escape(key)}"]`);
    if (!row) return;
    pendingFocus.current = null;
    row.focus();
  });

  const focusRow = (index: number) => {
    const target = Math.min(keys.length - 1, Math.max(0, index));
    const key = keys[target];
    if (key === undefined) return;
    setFocusedKey(key);
    pendingFocus.current = key;
    if (virtual) virtualizer.scrollToIndex(target);
  };

  // Selection
  const selected = selection?.selected;
  const selectedCount = selected ? keys.filter((key) => selected.has(key)).length : 0;
  const lastToggled = useRef<number | null>(null);
  const toggleRow = (index: number, range: boolean) => {
    const key = keys[index];
    if (!selection || key === undefined) return;
    const next = new Set(selection.selected);
    const select = !next.has(key);
    const from = range && lastToggled.current !== null ? lastToggled.current : index;
    for (let i = Math.min(from, index); i <= Math.max(from, index); i++) {
      const each = keys[i];
      if (each === undefined) continue;
      if (select) next.add(each);
      else next.delete(each);
    }
    lastToggled.current = index;
    selection.onChange(next);
  };
  const toggleAll = () => {
    if (!selection) return;
    selection.onChange(selectedCount === keys.length ? new Set() : new Set(keys));
  };

  const [menu, setMenu] = useState<RowMenu<T> | null>(null);

  const onRowKeyDown = (event: KeyboardEvent<HTMLTableRowElement>, row: T, index: number) => {
    // Keys pressed inside a cell's own control belong to that control.
    if (event.target !== event.currentTarget) return;
    switch (event.key) {
      case 'ArrowDown':
        focusRow(index + 1);
        break;
      case 'ArrowUp':
        focusRow(index - 1);
        break;
      case 'PageDown':
        focusRow(index + PAGE_ROWS);
        break;
      case 'PageUp':
        focusRow(index - PAGE_ROWS);
        break;
      case 'Home':
        focusRow(0);
        break;
      case 'End':
        focusRow(keys.length - 1);
        break;
      case ' ':
        if (!selection) return;
        toggleRow(index, event.shiftKey);
        break;
      case 'Enter':
        if (!onRowActivate) return;
        onRowActivate(row);
        break;
      default:
        return;
    }
    event.preventDefault();
  };

  const onRowContextMenu = (event: MouseEvent<HTMLTableRowElement>, row: T) => {
    if (!rowActions || rowActions(row).length === 0) return;
    event.preventDefault();
    setMenu({ row, anchor: contextMenuPoint(event) });
  };

  const choosable = columns.filter((column) => !column.required);
  const showChooser = id !== undefined && choosable.length > 0;
  const hasTrailing = Boolean(rowActions) || showChooser;
  const columnCount = visibleColumns.length + (selection ? 1 : 0) + (hasTrailing ? 1 : 0);
  const minWidth =
    visibleColumns.reduce((total, column) => total + (column.width ?? MIN_FLEXIBLE_WIDTH), 0) +
    (selection ? CHECK_COLUMN_WIDTH : 0) +
    (hasTrailing ? ACTIONS_COLUMN_WIDTH : 0);

  const showSkeleton = loading && rows.length === 0;
  const showEmpty = !loading && rows.length === 0;

  return (
    <div
      className={cx(styles.table, className)}
      data-density={density}
      data-fill={fill || undefined}
      // Many rows need a bounded height to scroll in; give them one if the screen did not.
      data-bounded={(virtual && !fill && maxHeight === undefined) || undefined}
      style={{ maxHeight }}
    >
      <div ref={scrollRef} className={styles.scroll}>
        <table
          className={styles.grid}
          style={{ minWidth }}
          aria-label={rest['aria-label']}
          aria-busy={loading || undefined}
          aria-rowcount={virtual ? sortedRows.length + 1 : undefined}
        >
          <colgroup>
            {selection && <col style={{ width: CHECK_COLUMN_WIDTH }} />}
            {visibleColumns.map((column) => (
              <col key={column.id} style={{ width: column.width }} />
            ))}
            {hasTrailing && <col style={{ width: ACTIONS_COLUMN_WIDTH }} />}
          </colgroup>

          <thead>
            <tr>
              {selection && (
                <th scope="col" className={styles.checkCell}>
                  <Checkbox
                    aria-label="Select all rows"
                    checked={keys.length > 0 && selectedCount === keys.length}
                    indeterminate={selectedCount > 0 && selectedCount < keys.length}
                    disabled={keys.length === 0}
                    onChange={toggleAll}
                  />
                </th>
              )}
              {visibleColumns.map((column) => {
                const sorted = sort?.columnId === column.id ? sort.direction : null;
                return (
                  <th
                    key={column.id}
                    scope="col"
                    data-align={column.align}
                    aria-sort={
                      !isSortable(column)
                        ? undefined
                        : sorted === 'asc'
                          ? 'ascending'
                          : sorted === 'desc'
                            ? 'descending'
                            : 'none'
                    }
                  >
                    {isSortable(column) ? (
                      <button
                        type="button"
                        className={styles.sort}
                        onClick={() => {
                          const next = nextSort(sort, column.id);
                          setSort(next);
                          remember({ sort: next, hidden: [...hidden] });
                        }}
                      >
                        <span className={styles.headerText}>{column.header}</span>
                        <Icon
                          icon={sorted === 'asc' ? ArrowUp : sorted === 'desc' ? ArrowDown : ChevronsUpDown}
                          size={12}
                          className={styles.sortIcon}
                        />
                      </button>
                    ) : (
                      <span className={styles.headerText}>{column.header}</span>
                    )}
                  </th>
                );
              })}
              {hasTrailing && (
                <th scope="col" className={styles.actionsCell}>
                  {showChooser ? (
                    <DropdownMenu
                      aria-label="Columns"
                      placement="bottom-end"
                      trigger={<IconButton icon={Columns3} size="sm" aria-label="Choose columns" tooltipPlacement="left" />}
                      items={choosable.map((column) => ({
                        label: column.header,
                        checked: !hidden.has(column.id),
                        onSelect: () => {
                          const next = new Set(hidden);
                          if (!next.delete(column.id)) next.add(column.id);
                          setHidden(next);
                          remember({ sort, hidden: [...next] });
                        },
                      }))}
                    />
                  ) : (
                    <span className="visually-hidden">Actions</span>
                  )}
                </th>
              )}
            </tr>
          </thead>

          <tbody>
            {showSkeleton &&
              Array.from({ length: SKELETON_ROWS }, (_, rowIndex) => (
                <tr key={rowIndex} aria-hidden="true">
                  {Array.from({ length: columnCount }, (_, cellIndex) => (
                    <td key={cellIndex}>
                      {/* Varied widths read as content, not as a striped block. */}
                      <Skeleton width={`${40 + ((rowIndex * 7 + cellIndex * 13) % 45)}%`} />
                    </td>
                  ))}
                </tr>
              ))}

            {showEmpty && (
              <tr>
                <td colSpan={columnCount} className={styles.emptyCell}>
                  {empty}
                </td>
              </tr>
            )}

            {firstIndex > 0 && (
              <tr aria-hidden="true">
                <td colSpan={columnCount} className={styles.spacer} style={{ height: firstIndex * rowHeight }} />
              </tr>
            )}
            {sortedRows.slice(firstIndex, lastIndex + 1).map((row, offset) => {
              const index = firstIndex + offset;
              const key = keys[index] ?? '';
              const isSelected = selected?.has(key) ?? false;
              return (
                <tr
                  key={key}
                  data-row={key}
                  className={styles.row}
                  tabIndex={key === tabStop ? 0 : -1}
                  aria-selected={selection ? isSelected : undefined}
                  aria-rowindex={virtual ? index + 2 : undefined}
                  onFocus={(event) => {
                    if (event.target === event.currentTarget) setFocusedKey(key);
                  }}
                  onKeyDown={(event) => onRowKeyDown(event, row, index)}
                  onDoubleClick={onRowActivate ? () => onRowActivate(row) : undefined}
                  onContextMenu={(event) => onRowContextMenu(event, row)}
                >
                  {selection && (
                    <td className={styles.checkCell}>
                      <Checkbox
                        aria-label="Select row"
                        checked={isSelected}
                        onChange={(event) => toggleRow(index, (event.nativeEvent as PointerEvent).shiftKey)}
                      />
                    </td>
                  )}
                  {visibleColumns.map((column) => (
                    <td key={column.id} data-align={column.align} className={column.mono ? 'mono' : undefined}>
                      {column.cell(row)}
                    </td>
                  ))}
                  {hasTrailing && (
                    <td className={styles.actionsCell}>
                      {rowActions && rowActions(row).length > 0 && (
                        <IconButton
                          icon={Ellipsis}
                          size="sm"
                          aria-label="Row actions"
                          aria-haspopup="menu"
                          tooltip={false}
                          onClick={(event) => setMenu({ row, anchor: { current: event.currentTarget } })}
                        />
                      )}
                    </td>
                  )}
                </tr>
              );
            })}
            {virtual && lastIndex < sortedRows.length - 1 && (
              <tr aria-hidden="true">
                <td
                  colSpan={columnCount}
                  className={styles.spacer}
                  style={{ height: (sortedRows.length - 1 - lastIndex) * rowHeight }}
                />
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {menu && rowActions && (
        <MenuPopup
          anchor={menu.anchor}
          placement="bottom-end"
          items={rowActions(menu.row)}
          onClose={() => setMenu(null)}
          aria-label="Row actions"
        />
      )}
    </div>
  );
}
