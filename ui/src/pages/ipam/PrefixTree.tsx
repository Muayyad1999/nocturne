/**
 * The prefix tree: the plan and the discoveries in one shape, told apart.
 *
 * Indentation is containment, as the backend computed it. A block nobody
 * documented carries the word "Discovered" and the interface it was inferred
 * from, so it can never be read as part of somebody's plan; a lab's blocks sit
 * under a root of their own with the `LAB` pill, because a simulated address
 * must not be counted towards production.
 *
 * The design system's `Tree` carries one label and one trailing slot per row;
 * this screen needs five columns and a utilisation bar per row, so the tree is
 * drawn here (see NOTES.md). It keeps `Tree`'s keyboard contract: up and down
 * walk the visible rows, right and left open and close a branch, Enter selects.
 */

import { ChevronRight, Network } from 'lucide-react';
import { useRef, type KeyboardEvent } from 'react';
import type { PrefixRow } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { Icon } from '@/design/Icon';
import { LabPill } from '@/design/StatusPill';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, formatPercent } from '@/lib/format';
import styles from './PrefixTree.module.css';
import { contentsText, recordState, seenOn, utilisation, type Group, type VisibleRow } from './tree';

export interface PrefixTreeProps {
  groups: readonly Group[];
  collapsed: ReadonlySet<string>;
  onToggle: (id: string, open: boolean) => void;
  selected: string | null;
  onSelect: (id: string) => void;
  /** Rows to draw for one group, so the page owns the collapse state. */
  rowsOf: (group: Group) => VisibleRow[];
}

export function PrefixTree({
  groups,
  collapsed,
  onToggle,
  selected,
  onSelect,
  rowsOf,
}: PrefixTreeProps) {
  const treeRef = useRef<HTMLDivElement>(null);

  // One flat list of what is on screen, so the arrow keys walk what the eye
  // walks rather than the shape of the data.
  const walkable: { id: string; expandable: boolean; expanded: boolean }[] = [];
  for (const group of groups) {
    for (const entry of rowsOf(group)) {
      walkable.push({
        id: entry.row.id,
        expandable: entry.children > 0,
        expanded: entry.expanded,
      });
    }
  }

  const focusRow = (id: string | undefined) => {
    if (id === undefined) return;
    treeRef.current?.querySelector<HTMLElement>(`[data-prefix="${CSS.escape(id)}"]`)?.focus();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const current = (event.target as HTMLElement).closest<HTMLElement>('[data-prefix]');
    const id = current?.dataset.prefix;
    if (!id) return;
    const index = walkable.findIndex((entry) => entry.id === id);
    const row = walkable[index];
    if (!row) return;
    switch (event.key) {
      case 'ArrowDown':
        event.preventDefault();
        focusRow(walkable[index + 1]?.id);
        break;
      case 'ArrowUp':
        event.preventDefault();
        focusRow(walkable[index - 1]?.id);
        break;
      case 'ArrowRight':
        if (row.expandable && !row.expanded) {
          event.preventDefault();
          onToggle(id, true);
        }
        break;
      case 'ArrowLeft':
        if (row.expandable && row.expanded) {
          event.preventDefault();
          onToggle(id, false);
        }
        break;
      case 'Enter':
      case ' ':
        event.preventDefault();
        onSelect(id);
        break;
      default:
        break;
    }
  };

  return (
    <div className={styles.tree} ref={treeRef} onKeyDown={onKeyDown}>
      {groups.map((group) => {
        const rows = rowsOf(group);
        return (
          <section key={group.scope.id || 'production'} className={styles.group}>
            <header className={styles.groupHead}>
              <Icon icon={Network} size={13} />
              <h3 className={styles.groupTitle}>{group.scope.label}</h3>
              {group.scope.kind === 'lab' && <LabPill />}
              <span className={styles.groupCount}>
                {formatCount(group.total)}
                {group.discovered > 0 && (
                  <span className={styles.groupDiscovered}>
                    {' '}
                    · {formatCount(group.discovered)} discovered
                  </span>
                )}
              </span>
            </header>
            <div role="tree" aria-label={`Prefixes in ${group.scope.label}`}>
              {rows.map((entry) => (
                <Row
                  key={entry.row.id}
                  entry={entry}
                  selected={entry.row.id === selected}
                  tabStop={entry.row.id === (selected ?? rows[0]?.row.id)}
                  onToggle={onToggle}
                  onSelect={onSelect}
                  collapsed={collapsed.has(entry.row.id)}
                />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

interface RowProps {
  entry: VisibleRow;
  selected: boolean;
  tabStop: boolean;
  collapsed: boolean;
  onToggle: (id: string, open: boolean) => void;
  onSelect: (id: string) => void;
}

function Row({ entry, selected, tabStop, collapsed, onToggle, onSelect }: RowProps) {
  const row = entry.row;
  const record = recordState(row);
  const used = utilisation(row);
  const provenance = seenOn(row);

  return (
    <div
      role="treeitem"
      aria-level={entry.depth + 1}
      aria-selected={selected}
      aria-expanded={entry.children > 0 ? !collapsed : undefined}
      data-prefix={row.id}
      data-selected={selected || undefined}
      data-source={row.source}
      tabIndex={tabStop ? 0 : -1}
      className={styles.row}
      onClick={() => onSelect(row.id)}
    >
      <div className={styles.cellName} style={{ paddingLeft: `${entry.depth * 14}px` }}>
        {entry.children > 0 ? (
          <button
            type="button"
            className={styles.chevron}
            aria-label={collapsed ? `Open ${row.cidr}` : `Close ${row.cidr}`}
            data-open={!collapsed || undefined}
            tabIndex={-1}
            onClick={(event) => {
              event.stopPropagation();
              onToggle(row.id, collapsed);
            }}
          >
            <Icon icon={ChevronRight} size={13} />
          </button>
        ) : (
          <span className={styles.chevronSpace} />
        )}
        <span className={styles.cidr}>{row.cidr}</span>
        {row.name !== '' && <span className={styles.name}>{row.name}</span>}
        {row.vlan_id !== undefined && (
          <span className={styles.vlan}>VLAN {row.vlan_id}</span>
        )}
      </div>

      <div className={styles.cellRecord}>
        {record.kind === 'discovered' ? (
          <Tooltip content={provenance ? `${record.meaning} ${provenance}.` : record.meaning}>
            <Badge tone="info" variant="outline">
              Discovered
            </Badge>
          </Tooltip>
        ) : (
          <Tooltip content={record.meaning}>
            <Badge tone="ok" variant="outline">
              Documented
            </Badge>
          </Tooltip>
        )}
      </div>

      <Tooltip
        content={
          used.known
            ? `${contentsText(row)} · ${formatCount(used.free)} of ${used.usableText} with no record and nothing seen`
            : `${used.reason} ${formatCount(used.used)} of ${used.usableText} addresses are known to be in use.`
        }
      >
        <div className={styles.cellUse}>
          {used.known ? (
            <>
              <span className={styles.bar} data-tone={used.tone}>
                <span className={styles.fill} style={{ width: `${Math.min(100, used.pct)}%` }} />
              </span>
              <span className={styles.pct}>{formatPercent(used.pct)}</span>
            </>
          ) : (
            <>
              {/* No bar and no percentage, because neither would mean
                  anything: only what is known to be in use. */}
              <span className={styles.noPct}>{formatCount(used.used)} in use</span>
              <span className={styles.pct}>—</span>
            </>
          )}
        </div>
      </Tooltip>

      <div className={styles.cellSeen}>
        {row.devices.length > 0 ? (
          <span className={styles.devices}>
            {formatCount(row.devices.length)}{' '}
            {row.devices.length === 1 ? 'interface' : 'interfaces'}
          </span>
        ) : (
          <span className={styles.none}>none seen</span>
        )}
      </div>
    </div>
  );
}

export interface TreeEmptyProps {
  filtered: boolean;
}

/** Nothing to draw: two different reasons, two different sentences. */
export function treeEmptyText(rows: readonly PrefixRow[], filtered: boolean): string {
  if (filtered && rows.length === 0) {
    return 'No block matches this filter. Clear it to see the rest of the plan.';
  }
  return 'Nothing is documented and nothing has been discovered yet. Add a block, or collect from the devices and let the addresses they report fill this in.';
}
