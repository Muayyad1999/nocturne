import { Search, SearchX } from 'lucide-react';
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { EmptyState } from '@/design/EmptyState';
import { Highlight } from '@/design/Highlight';
import { Icon } from '@/design/Icon';
import { Kbd } from '@/design/Kbd';
import { useFocusScope } from '@/design/overlay/focus';
import { useLayer } from '@/design/overlay/layers';
import { Portal } from '@/design/overlay/Portal';
import { fuzzyFilter } from '@/lib/fuzzy';
import { allowsGlobalShortcuts, matchesShortcut } from '@/lib/keyboard';
import styles from './CommandPalette.module.css';
import { runCommand, useCommands, type Command } from './commands';
import { usePalette } from './palette';

export const PALETTE_SHORTCUT = 'mod+k';

interface Row {
  command: Command;
  /** Matched characters of the title, for highlighting. */
  indices: number[];
}

interface Section {
  title: string;
  rows: Row[];
}

/**
 * What the palette lists. Without a query: recently used commands first, then
 * every command under its group. With one: the best matches, ranked.
 */
function buildSections(commands: Command[], recent: string[], query: string): Section[] {
  if (query.trim()) {
    const rows = fuzzyFilter(commands, query, {
      text: (command) => command.title,
      keywords: (command) => [command.group, ...(command.keywords ?? [])],
    }).map(({ item, indices }) => ({ command: item, indices }));
    return rows.length > 0 ? [{ title: 'Results', rows }] : [];
  }

  const byId = new Map(commands.map((command) => [command.id, command]));
  const recentRows = recent.flatMap((id) => {
    const command = byId.get(id);
    return command ? [{ command, indices: [] }] : [];
  });
  const recentIds = new Set(recentRows.map((row) => row.command.id));

  const groups = new Map<string, Row[]>();
  for (const command of commands) {
    if (recentIds.has(command.id)) continue;
    const rows = groups.get(command.group) ?? [];
    rows.push({ command, indices: [] });
    groups.set(command.group, rows);
  }

  return [
    ...(recentRows.length > 0 ? [{ title: 'Recent', rows: recentRows }] : []),
    ...[...groups].map(([title, rows]) => ({ title, rows })),
  ];
}

function PaletteDialog() {
  const close = usePalette((state) => state.close);
  const commands = useCommands((state) => state.commands);
  const recent = useCommands((state) => state.recent);
  const [query, setQuery] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const dialogRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();

  useLayer(true, dialogRef, { modal: true, onEscape: close, onPressOutside: close });
  useFocusScope(true, dialogRef);

  const sections = useMemo(
    () => buildSections(Object.values(commands), recent, query),
    [commands, recent, query],
  );
  const rows = sections.flatMap((section) => section.rows);
  const active = rows[Math.min(activeIndex, rows.length - 1)];
  const optionId = (command: Command) => `${listId}-${command.id}`;

  useEffect(() => {
    listRef.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' });
  }, [active]);

  const run = (command: Command) => {
    close();
    // After the palette is gone and focus is back where it was, so a command
    // that opens a dialog or moves focus starts from the right place.
    setTimeout(() => runCommand(command.id), 0);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    const move = (delta: number) => {
      if (rows.length === 0) return;
      const current = Math.min(activeIndex, rows.length - 1);
      setActiveIndex((current + delta + rows.length) % rows.length);
    };
    if (event.key === 'ArrowDown') move(1);
    else if (event.key === 'ArrowUp') move(-1);
    else if (event.key === 'PageDown') setActiveIndex(Math.min(activeIndex + 8, rows.length - 1));
    else if (event.key === 'PageUp') setActiveIndex(Math.max(activeIndex - 8, 0));
    else if (event.key === 'Enter' && !event.nativeEvent.isComposing) {
      if (active) run(active.command);
    } else return;
    event.preventDefault();
  };

  return (
    <Portal>
      <div className={styles.scrim}>
        <div ref={dialogRef} className={styles.palette} role="dialog" aria-modal="true" aria-label="Command palette">
          <div className={styles.inputRow}>
            <Icon icon={Search} className={styles.inputIcon} />
            <input
              className={styles.input}
              data-autofocus
              role="combobox"
              aria-expanded="true"
              aria-controls={listId}
              aria-activedescendant={active ? optionId(active.command) : undefined}
              aria-autocomplete="list"
              aria-label="Search commands and screens"
              placeholder="Search commands and screens"
              autoComplete="off"
              spellCheck={false}
              value={query}
              onChange={(event) => {
                setQuery(event.target.value);
                setActiveIndex(0);
              }}
              onKeyDown={onKeyDown}
            />
          </div>

          <div ref={listRef} id={listId} className={styles.list} role="listbox" aria-label="Commands">
            {rows.length === 0 && (
              <EmptyState
                size="compact"
                icon={SearchX}
                title="No matching command"
                description={`Nothing matches “${query.trim()}”. Try the name of a screen or an action.`}
              />
            )}
            {sections.map((section) => (
              <div key={section.title} role="group" aria-label={section.title}>
                <div className={styles.sectionTitle} aria-hidden="true">
                  {section.title}
                </div>
                {section.rows.map(({ command, indices }) => (
                  <div
                    key={command.id}
                    id={optionId(command)}
                    role="option"
                    aria-selected={command === active?.command}
                    className={styles.option}
                    onClick={() => run(command)}
                    onPointerMove={() => {
                      const index = rows.findIndex((row) => row.command === command);
                      if (index !== activeIndex) setActiveIndex(index);
                    }}
                  >
                    <span className={styles.optionIcon}>
                      {command.icon && <Icon icon={command.icon} />}
                    </span>
                    <span className={styles.optionTitle}>
                      <Highlight text={command.title} indices={indices} />
                    </span>
                    {section.title === 'Results' || section.title === 'Recent' ? (
                      <span className={styles.optionGroup}>{command.group}</span>
                    ) : null}
                    {command.shortcut && <Kbd shortcut={command.shortcut} />}
                  </div>
                ))}
              </div>
            ))}
          </div>

          <div className={styles.footer} aria-hidden="true">
            <span>
              <Kbd>↑</Kbd> <Kbd>↓</Kbd> to move
            </span>
            <span>
              <Kbd>Enter</Kbd> to run
            </span>
            <span>
              <Kbd>Esc</Kbd> to close
            </span>
          </div>
        </div>
      </div>
    </Portal>
  );
}

/** The command palette. Mounted once by the shell; opens with Ctrl/⌘ K. */
export function CommandPalette() {
  const visible = usePalette((state) => state.visible);
  const toggle = usePalette((state) => state.toggle);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!matchesShortcut(event, PALETTE_SHORTCUT) || !allowsGlobalShortcuts(event.target)) return;
      event.preventDefault();
      toggle();
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [toggle]);

  return visible ? <PaletteDialog /> : null;
}
