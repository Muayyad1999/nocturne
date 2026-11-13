/**
 * One pane: a strip of tabs with the session's quiet header, and the terminal
 * itself. The xterm instance is not created here — it belongs to
 * `sessions.ts` and is only lent to the pane while the pane is on screen.
 */

import {
  ChevronDown,
  ChevronUp,
  CircleDot,
  Columns2,
  Copy,
  Eraser,
  RotateCw,
  Search,
  Sparkles,
  SquareSplitHorizontal,
  X,
} from 'lucide-react';
import {
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type DragEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import type { SessionInfo } from '@/api/types/terminal';
import { Button, IconButton } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { ContextMenu } from '@/design/ContextMenu';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import type { MenuEntry } from '@/design/Menu';
import { Spinner } from '@/design/Spinner';
import { StatusDot } from '@/design/StatusDot';
import { useTokens } from '@/design/theme';
import { Tooltip } from '@/design/Tooltip';
import { MAX_PANES, type Pane as PaneModel } from './layout';
import styles from './Pane.module.css';
import { terminals } from './sessions';
import { TransportDetails } from './TransportDetails';

export interface PaneActions {
  select: (paneId: string, sessionId: string) => void;
  focusPane: (paneId: string) => void;
  closeTab: (sessionId: string) => void;
  reconnect: (sessionId: string) => void;
  toggleLogging: (sessionId: string) => void;
  split: (sessionId: string) => void;
  moveTo: (sessionId: string, paneId: string) => void;
  closePane: (paneId: string) => void;
  /** A paste the user asked for; it goes through the confirm sheet. */
  paste: (sessionId: string, text: string) => void;
  copySelection: (sessionId: string) => void;
  explain: (sessionId: string) => void;
  clear: (sessionId: string) => void;
  connect: () => void;
}

export interface PaneProps {
  pane: PaneModel;
  sessions: ReadonlyMap<string, SessionInfo>;
  focused: boolean;
  /** How many panes are on screen, so a pane knows if it may split or close. */
  paneCount: number;
  /** Typing goes to every pane that is not held back. */
  broadcasting: boolean;
  actions: PaneActions;
  /** The tab being dragged right now, if any. */
  dragging: string | null;
  /** Whether dropping the dragged tab in a new pane would change anything. */
  draggingCanSplit: boolean;
  onDragging: (sessionId: string | null) => void;
  /** The find bar is open in this pane. Lifted, so a command can open it. */
  finding: boolean;
  onFinding: (finding: boolean) => void;
}

/** The state dot a session wears in its tab. */
function sessionTone(info: SessionInfo | undefined): 'ok' | 'warn' | 'crit' | 'idle' {
  if (!info) return 'idle';
  if (info.state === 'open') return 'ok';
  if (info.state === 'connecting') return 'warn';
  return 'crit';
}

function stateWord(info: SessionInfo | undefined): string {
  if (!info) return 'Unknown';
  if (info.state === 'open') return 'Connected';
  if (info.state === 'connecting') return 'Connecting';
  return info.close_reason ? `Closed — ${info.close_reason}` : 'Closed';
}

export function Pane({
  pane,
  sessions,
  focused,
  paneCount,
  broadcasting,
  actions,
  dragging,
  draggingCanSplit,
  onDragging,
  finding,
  onFinding,
}: PaneProps) {
  const active = pane.active;
  const info = active ? sessions.get(active) : undefined;

  return (
    <section
      className={styles.pane}
      data-focused={focused || undefined}
      aria-label={info ? `Session ${info.title}` : 'Empty pane'}
      onMouseDown={() => actions.focusPane(pane.id)}
      onFocusCapture={() => actions.focusPane(pane.id)}
    >
      <header className={styles.header}>
        <div className={styles.tabs} role="tablist" aria-label="Sessions in this pane">
          {pane.tabs.map((id) => (
            <Tab
              key={id}
              sessionId={id}
              info={sessions.get(id)}
              active={id === active}
              paneId={pane.id}
              actions={actions}
              onDragging={onDragging}
            />
          ))}
        </div>

        <div className={styles.actions}>
          {broadcasting && (
            <span className={styles.flag} data-kind="broadcast">
              TYPING HERE
            </span>
          )}
          {info?.lab && (
            <Tooltip content="A simulated device: whatever happens here happens in a lab.">
              <span className={styles.flag} data-kind="lab">
                LAB
              </span>
            </Tooltip>
          )}
          {info?.info.legacy_crypto && (
            <Tooltip content="Only the legacy algorithm set would negotiate. The device is old; the link is weaker than it should be.">
              <span className={styles.flag} data-kind="legacy">
                LEGACY CRYPTO
              </span>
            </Tooltip>
          )}
          {info?.logging && (
            <Tooltip content={`Writing to ${info.logging}`}>
              <span className={styles.flag} data-kind="log">
                <Icon icon={CircleDot} size={11} />
                LOG
              </span>
            </Tooltip>
          )}
          {info && <TransportDetails info={info} />}
          {info && (
            <IconButton
              icon={Search}
              aria-label="Find in scrollback"
              tooltip="Find in scrollback (Ctrl+Shift+F)"
              size="sm"
              onClick={() => onFinding(true)}
            />
          )}
          {info && (
            <IconButton
              icon={RotateCw}
              aria-label="Reconnect this session"
              tooltip={info.state === 'closed' ? 'Connect again' : 'Close and connect again'}
              size="sm"
              onClick={() => actions.reconnect(info.id)}
            />
          )}
          {active && paneCount < MAX_PANES && pane.tabs.length > 1 && (
            <IconButton
              icon={SquareSplitHorizontal}
              aria-label="Move this session to a new pane"
              tooltip="Move to a new pane"
              size="sm"
              onClick={() => actions.split(active)}
            />
          )}
          {paneCount > 1 && (
            <IconButton
              icon={Columns2}
              aria-label="Close this pane"
              tooltip="Close this pane (its sessions move next door)"
              size="sm"
              onClick={() => actions.closePane(pane.id)}
            />
          )}
        </div>
      </header>

      <div
        className={styles.body}
        role={active ? 'tabpanel' : undefined}
        id={active ? `terminal-panel-${active}` : undefined}
        aria-labelledby={active ? `terminal-tab-${active}` : undefined}
      >
        {active ? (
          <Surface
            key={active}
            sessionId={active}
            actions={actions}
            onFind={() => onFinding(true)}
            finding={finding}
            title={info?.title ?? active}
          />
        ) : (
          <div className={styles.state}>
            <EmptyState
              size="compact"
              icon={SquareSplitHorizontal}
              title="This pane is empty"
              description="Drag a tab onto it, or connect to a device from the list."
              action={
                <Button size="sm" onClick={actions.connect}>
                  Connect…
                </Button>
              }
            />
          </div>
        )}

        {dragging && dragging !== active && (
          <DropZones
            paneId={pane.id}
            sessionId={dragging}
            canSplit={draggingCanSplit}
            actions={actions}
            onDone={() => onDragging(null)}
          />
        )}

        {finding && active && <FindBar sessionId={active} onClose={() => onFinding(false)} />}
      </div>
    </section>
  );
}

// ------------------------------------------------------------------- a tab

interface TabProps {
  sessionId: string;
  info: SessionInfo | undefined;
  active: boolean;
  paneId: string;
  actions: PaneActions;
  onDragging: (sessionId: string | null) => void;
}

function Tab({ sessionId, info, active, paneId, actions, onDragging }: TabProps) {
  const [dragging, setDragging] = useState(false);
  const title = info?.title ?? 'Session';

  return (
    <div
      role="tab"
      id={`terminal-tab-${sessionId}`}
      aria-selected={active}
      aria-controls={active ? `terminal-panel-${sessionId}` : undefined}
      tabIndex={active ? 0 : -1}
      draggable
      data-active={active || undefined}
      data-dragging={dragging || undefined}
      className={styles.tab}
      title={`${title} — ${stateWord(info)}`}
      onClick={() => actions.select(paneId, sessionId)}
      onKeyDown={(event: ReactKeyboardEvent<HTMLDivElement>) => {
        if (event.key === 'Enter' || event.key === ' ') {
          event.preventDefault();
          actions.select(paneId, sessionId);
        }
      }}
      onDragStart={(event: DragEvent<HTMLDivElement>) => {
        event.dataTransfer.setData('text/plain', sessionId);
        event.dataTransfer.effectAllowed = 'move';
        setDragging(true);
        onDragging(sessionId);
      }}
      onDragEnd={() => {
        setDragging(false);
        onDragging(null);
      }}
    >
      <StatusDot tone={sessionTone(info)} label={stateWord(info)} size={8} pulse={info?.state === 'connecting'} />
      <span className={styles.tabLabel}>{title}</span>
      <button
        type="button"
        className={styles.tabClose}
        aria-label={`Close ${title}`}
        onClick={(event) => {
          event.stopPropagation();
          actions.closeTab(sessionId);
        }}
      >
        <Icon icon={X} size={12} />
      </button>
    </div>
  );
}

// --------------------------------------------------------------- the screen

interface SurfaceProps {
  sessionId: string;
  title: string;
  actions: PaneActions;
  onFind: () => void;
  /** While finding, the highlighted match is a match, not a selection. */
  finding: boolean;
}

/**
 * Where the terminal is lent to the pane. The element comes from the registry
 * and goes back to it on unmount, so the session keeps running.
 */
function Surface({ sessionId, title, actions, onFind, finding }: SurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const view = useSyncExternalStore(terminals.subscribe, () => terminals.view(sessionId));

  // The handlers the terminal's own key and paste events reach, kept fresh
  // without re-attaching them on every render.
  const handlers = useRef({ actions, onFind, sessionId });
  handlers.current = { actions, onFind, sessionId };

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    // A pane's own effect runs before the screen's, so the terminal is
    // claimed here too. `adopt` is idempotent: whoever gets there first wins.
    terminals.adopt(sessionId);
    const unmount = terminals.mount(sessionId, host);
    const term = terminals.terminal(sessionId);

    const onPaste = (event: ClipboardEvent) => {
      event.preventDefault();
      const text = event.clipboardData?.getData('text') ?? '';
      if (text !== '') handlers.current.actions.paste(sessionId, text);
    };
    term?.textarea?.addEventListener('paste', onPaste);

    term?.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown') return true;
      const key = event.key.toLowerCase();
      const chord = (event.ctrlKey && event.shiftKey) || (event.metaKey && !event.ctrlKey);
      if (!chord) return true;
      if (key === 'f') {
        event.preventDefault();
        handlers.current.onFind();
        return false;
      }
      if (key === 'c' && term.hasSelection()) {
        event.preventDefault();
        handlers.current.actions.copySelection(sessionId);
        return false;
      }
      // The app's own chords, which the shell cannot see from in here: a
      // terminal keeps every key, including Ctrl+K.
      if (key === 'd') {
        event.preventDefault();
        handlers.current.actions.split(sessionId);
        return false;
      }
      if (key === 'k') {
        event.preventDefault();
        handlers.current.actions.clear(sessionId);
        return false;
      }
      if (key === 'v') {
        event.preventDefault();
        void navigator.clipboard
          .readText()
          .then((text) => {
            if (text !== '') handlers.current.actions.paste(sessionId, text);
          })
          // Without clipboard permission the browser's own paste still works.
          .catch(() => undefined);
        return false;
      }
      return true;
    });

    const observer = new ResizeObserver(() => terminals.refit(sessionId));
    observer.observe(host);
    terminals.focus(sessionId);

    return () => {
      observer.disconnect();
      term?.textarea?.removeEventListener('paste', onPaste);
      term?.attachCustomKeyEventHandler(() => true);
      unmount();
    };
  }, [sessionId]);

  const items: MenuEntry[] = [
    {
      label: 'Copy',
      icon: Copy,
      disabled: view.selection === '',
      onSelect: () => actions.copySelection(sessionId),
    },
    { label: 'Paste…', icon: Copy, onSelect: () => void pasteFromClipboard(sessionId, actions) },
    { type: 'separator' },
    {
      label: 'Explain the selection',
      icon: Sparkles,
      disabled: view.selection === '',
      onSelect: () => actions.explain(sessionId),
    },
    { label: 'Find…', icon: Search, onSelect: onFind },
    { label: 'Clear', icon: Eraser, onSelect: () => actions.clear(sessionId) },
  ];

  return (
    <>
      <ContextMenu items={items} aria-label={`Actions for ${title}`}>
        <div
          ref={hostRef}
          className={styles.surface}
          // The device gets every key: Ctrl+K is its own, not the palette's.
          data-global-shortcuts="off"
        />
      </ContextMenu>

      {view.attach === 'attaching' && (
        <div className={styles.overlay}>
          <Spinner label={`Attaching to ${title}`} />
        </div>
      )}
      {view.attach === 'failed' && (
        <div className={styles.overlay}>
          <ErrorState
            size="compact"
            title="This session could not be attached"
            error={view.error}
            onRetry={() => actions.reconnect(sessionId)}
            retryLabel="Connect again"
          />
        </div>
      )}

      {view.selection !== '' && !finding && (
        <div className={styles.selection}>
          <span className={styles.selectionCount}>
            {view.selection.split('\n').length} line{view.selection.split('\n').length === 1 ? '' : 's'} selected
          </span>
          <Button size="sm" variant="ai" icon={Sparkles} onClick={() => actions.explain(sessionId)}>
            Explain
          </Button>
          <Button size="sm" icon={Copy} onClick={() => actions.copySelection(sessionId)}>
            Copy
          </Button>
        </div>
      )}
    </>
  );
}

async function pasteFromClipboard(sessionId: string, actions: PaneActions): Promise<void> {
  try {
    const text = await navigator.clipboard.readText();
    if (text !== '') actions.paste(sessionId, text);
  } catch {
    // Nothing to paste, or no permission: the keyboard path still works.
  }
}

// -------------------------------------------------------------- the finding

function FindBar({ sessionId, onClose }: { sessionId: string; onClose: () => void }) {
  const [query, setQuery] = useState('');
  const [caseSensitive, setCaseSensitive] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const view = useSyncExternalStore(terminals.subscribe, () => terminals.view(sessionId));
  const tokens = useTokens(['--bg-4', '--accent']);

  const decorations = {
    matchBackground: tokens['--bg-4'],
    matchOverviewRuler: tokens['--accent'],
    activeMatchBackground: tokens['--accent'],
    activeMatchColorOverviewRuler: tokens['--accent'],
  };

  const find = (direction: 'next' | 'previous', text = query) => {
    const search = terminals.searchAddon(sessionId);
    if (!search || text === '') return;
    const options = { caseSensitive, decorations };
    if (direction === 'next') search.findNext(text, { ...options, incremental: true });
    else search.findPrevious(text, options);
  };

  useEffect(() => {
    inputRef.current?.focus();
    return () => terminals.searchAddon(sessionId)?.clearDecorations();
  }, [sessionId]);

  return (
    <div className={styles.find} role="search">
      <Input
        ref={inputRef}
        size="sm"
        className={styles.findField}
        aria-label="Find in scrollback"
        placeholder="Find in scrollback"
        value={query}
        onChange={(event) => {
          setQuery(event.target.value);
          find('next', event.target.value);
        }}
        onKeyDown={(event) => {
          if (event.key === 'Enter') {
            event.preventDefault();
            find(event.shiftKey ? 'previous' : 'next');
          }
          if (event.key === 'Escape') {
            event.preventDefault();
            onClose();
            terminals.focus(sessionId);
          }
        }}
      />
      <span className={styles.findCount} aria-live="polite">
        {query === '' ? '' : view.find ? `${view.find.index} of ${view.find.count}` : 'No match'}
      </span>
      <IconButton
        icon={ChevronUp}
        size="sm"
        aria-label="Previous match"
        onClick={() => find('previous')}
      />
      <IconButton icon={ChevronDown} size="sm" aria-label="Next match" onClick={() => find('next')} />
      <Checkbox
        className={styles.findCase}
        checked={caseSensitive}
        onChange={(event) => setCaseSensitive(event.target.checked)}
        aria-label="Match case"
      >
        Aa
      </Checkbox>
      <span className={styles.findSpacer} />
      <IconButton icon={X} aria-label="Close find" size="sm" onClick={onClose} />
    </div>
  );
}

// ------------------------------------------------------------- the dropping

interface DropZonesProps {
  paneId: string;
  sessionId: string;
  canSplit: boolean;
  actions: PaneActions;
  onDone: () => void;
}

/** Where a dragged tab may land: into this pane, or into a new one beside it. */
function DropZones({ paneId, sessionId, canSplit, actions, onDone }: DropZonesProps) {
  const [over, setOver] = useState<'move' | 'split' | null>(null);

  const zone = (kind: 'move' | 'split', label: string, run: () => void) => (
    <div
      className={styles.dropZone}
      data-kind={kind}
      data-over={over === kind || undefined}
      onDragOver={(event: DragEvent<HTMLDivElement>) => {
        event.preventDefault();
        event.dataTransfer.dropEffect = 'move';
        setOver(kind);
      }}
      onDragLeave={() => setOver((current) => (current === kind ? null : current))}
      onDrop={(event: DragEvent<HTMLDivElement>) => {
        event.preventDefault();
        setOver(null);
        run();
        onDone();
      }}
    >
      {label}
    </div>
  );

  return (
    <div className={styles.drop}>
      {zone('move', 'Move here', () => actions.moveTo(sessionId, paneId))}
      {canSplit && zone('split', 'New pane', () => actions.split(sessionId))}
    </div>
  );
}
