/**
 * The drawer's left side: every tool this build has, grouped, filterable.
 *
 * Built from `tools.list`, so it is the engine's own catalogue rather than a
 * list maintained here (`catalogue.ts` says how the groups and the folding
 * work). The filter is the command palette's fuzzy match over the title, the
 * description and the group, with the matched characters picked out, because
 * the honest way to find "the one that does MSS" is to type `mss`.
 *
 * A row says which of the three families its tool belongs to — arithmetic
 * here, packets from here, something that runs on a device or keeps running —
 * with a word and a colour, never a colour alone.
 */

import { Search } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { Highlight } from '@/design/Highlight';
import { SearchInput } from '@/design/SearchInput';
import { Skeleton } from '@/design/Skeleton';
import { FAMILY, KIND_BADGE, type Panel, type PanelGroup } from './catalogue';
import styles from './Tools.module.css';

export interface ToolListProps {
  groups: readonly PanelGroup[];
  selected: string | undefined;
  query: string;
  onQuery: (query: string) => void;
  onSelect: (panel: Panel) => void;
  /** How many tools there are in all, for the filter's own count. */
  total: number;
  loading?: boolean;
}

export function ToolList({
  groups,
  selected,
  query,
  onQuery,
  onSelect,
  total,
  loading,
}: ToolListProps) {
  const shown = groups.reduce((count, group) => count + group.entries.length, 0);
  const filter = useRef<HTMLInputElement>(null);

  // `/` focuses the screen's search, as on every other screen.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== '/' || event.ctrlKey || event.metaKey || event.altKey) return;
      const target = event.target;
      if (target instanceof HTMLElement && target.closest('input, textarea')) return;
      event.preventDefault();
      filter.current?.focus();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  return (
    <div className={styles.list}>
      <div className={styles.listFilter}>
        <SearchInput
          ref={filter}
          value={query}
          placeholder="Filter tools"
          aria-label="Filter tools"
          onChange={onQuery}
        />
        <p className={styles.listCount}>
          {loading
            ? 'Reading the catalogue…'
            : query.trim()
              ? `${shown} of ${total} tools`
              : `${total} tools`}
        </p>
      </div>

      <nav className={styles.listScroll} aria-label="Tools">
        {loading && (
          <div className={styles.listSkeleton} aria-hidden="true">
            {Array.from({ length: 8 }, (_unused, index) => (
              <Skeleton key={index} shape="block" height={32} />
            ))}
          </div>
        )}

        {!loading && shown === 0 && (
          <p className={styles.listEmpty}>
            <Search size={14} aria-hidden="true" /> Nothing in the drawer matches “{query.trim()}”.
          </p>
        )}

        {groups.map((group) => (
          <section key={group.group} className={styles.group}>
            <h3 className={styles.groupLabel}>{group.label}</h3>
            <ul className={styles.groupItems}>
              {group.entries.map(({ panel, indices }) => (
                <li key={panel.tool.id}>
                  <button
                    type="button"
                    className={styles.row}
                    data-family={FAMILY[panel.kind]}
                    aria-current={panel.tool.id === selected ? 'true' : undefined}
                    title={panel.tool.description}
                    onClick={() => onSelect(panel)}
                  >
                    <span className={styles.rowTitle}>
                      <Highlight text={panel.tool.title} indices={indices} />
                    </span>
                    {FAMILY[panel.kind] !== 'offline' && (
                      <span className={styles.rowBadge}>{KIND_BADGE[panel.kind]}</span>
                    )}
                  </button>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </nav>
    </div>
  );
}
