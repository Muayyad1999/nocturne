/**
 * Consoles on lab devices, in the same terminal the Terminal screen uses.
 *
 * A lab node is an inventory device, so its console is an ordinary
 * `terminal.open` / `terminal.attach` session and the registry in
 * `pages/terminal/sessions.ts` owns the xterm instance — outside React, so
 * output never costs a render and a session survives leaving the screen.
 * This file only decides which sessions belong to this lab and lends the
 * terminal's element to a pane.
 *
 * Which tabs are open is not remembered here: it is read back from
 * `terminal.list` every time, so closing the window and opening it again
 * shows exactly the consoles the backend still holds, with their scrollback.
 */

import { Terminal as TerminalIcon, X } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { rpc } from '@/api/client';
import { RpcError } from '@/api/errors';
import { useRpc } from '@/api/hooks';
import type { LabView, NodeView } from '@/api/types/lab';
import type { SessionInfo } from '@/api/types/terminal';
import { IconButton } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Icon } from '@/design/Icon';
import { toast } from '@/design/Toast';
import { useTerminalAppearance } from '@/pages/terminal/appearance';
import { terminals } from '@/pages/terminal/sessions';
// xterm's own stylesheet. It is imported by whichever screen shows a
// terminal, and without it xterm's hidden character-measuring element is
// painted over the output as a row of repeated glyphs.
// The sixteen ANSI colours the terminal paints with. They are tokens that
// belong in `design/tokens.css`; until they move, the Terminal screen's file
// is the one place they exist, and a lab console needs them just as much.
import '@/pages/terminal/tokens.css';
import styles from './ConsoleDock.module.css';
import { neighbourTab, openSessionFor, tabsForLab, type ConsoleTab } from './consoles';

/** The size a console is opened at; the fit addon corrects it as soon as it is on screen. */
const OPEN_COLS = 120;
const OPEN_ROWS = 32;

export interface LabConsoles {
  tabs: ConsoleTab[];
  active: string | null;
  setActive: (sessionId: string) => void;
  open: (node: NodeView) => void;
  close: (sessionId: string) => void;
  opening: string | null;
}

/**
 * The lab's consoles. `open` reuses the session a node already has rather
 * than opening a second one on the same device.
 */
export function useLabConsoles(lab: LabView | undefined): LabConsoles {
  const sessions = useRpc<SessionInfo[]>('terminal.list', undefined, {
    invalidateOn: ['terminal.opened', 'terminal.closed', 'terminal.removed'],
  });
  const [active, setActive] = useState<string | null>(null);
  const [opening, setOpening] = useState<string | null>(null);
  /**
   * A session this screen has just opened. `terminal.list` does not know
   * about it until the refetch lands, and without this the reconciling
   * effect below would pull the user straight back to the previous tab.
   */
  const pending = useRef<string | null>(null);
  /** Sessions being closed: gone from the tab bar before the refetch says so. */
  const [closing, setClosing] = useState<ReadonlySet<string>>(() => new Set());

  const tabs = useMemo(
    () =>
      lab ? tabsForLab(sessions.data ?? [], lab).filter((tab) => !closing.has(tab.sessionId)) : [],
    [sessions.data, lab, closing],
  );

  // Forget a closing session once the backend has: the set must not grow.
  useEffect(() => {
    if (closing.size === 0) return;
    const listed = new Set((sessions.data ?? []).map((session) => session.id));
    const left = [...closing].filter((id) => listed.has(id));
    if (left.length !== closing.size) setClosing(new Set(left));
  }, [sessions.data, closing]);

  // The active tab follows what the backend still has: a session closed from
  // somewhere else must not leave an empty pane behind.
  useEffect(() => {
    if (pending.current !== null && tabs.some((tab) => tab.sessionId === pending.current)) {
      pending.current = null;
    }
    if (tabs.length === 0) {
      if (active !== null && pending.current === null) setActive(null);
      return;
    }
    if (active !== null && (pending.current === active || tabs.some((tab) => tab.sessionId === active))) {
      return;
    }
    setActive(tabs[tabs.length - 1]?.sessionId ?? null);
  }, [tabs, active]);

  const open = useCallback(
    (node: NodeView) => {
      if (!lab) return;
      const deviceId = lab.devices[node.name];
      if (!deviceId) {
        toast.error(`${node.name} has no command line, so there is no console to open.`);
        return;
      }
      const existing = openSessionFor(sessions.data ?? [], deviceId);
      if (existing) {
        setActive(existing.id);
        return;
      }
      setOpening(node.name);
      void (async () => {
        try {
          const session = await rpc<SessionInfo>('terminal.open', {
            device_id: deviceId,
            cols: OPEN_COLS,
            rows: OPEN_ROWS,
          });
          terminals.adopt(session.id);
          pending.current = session.id;
          setActive(session.id);
          await sessions.refetch();
        } catch (error) {
          const message =
            error instanceof RpcError ? error.message : 'the console could not be opened';
          toast.error(`No console on ${node.name}`, { description: message });
        } finally {
          setOpening(null);
        }
      })();
    },
    [lab, sessions],
  );

  const close = useCallback(
    (sessionId: string) => {
      setActive(neighbourTab(tabs, sessionId));
      setClosing((current) => new Set(current).add(sessionId));
      void (async () => {
        try {
          await rpc('terminal.close', { session_id: sessionId });
        } catch {
          // A session the backend has already forgotten is closed enough.
        }
        terminals.forget(sessionId);
        await sessions.refetch();
      })();
    },
    [tabs, sessions],
  );

  return { tabs, active, setActive, open, close, opening };
}

export interface ConsoleDockProps {
  consoles: LabConsoles;
  labName: string;
}

export function ConsoleDock({ consoles, labName }: ConsoleDockProps) {
  const appearance = useTerminalAppearance();
  useEffect(() => terminals.setAppearance(appearance), [appearance]);

  const { tabs, active } = consoles;
  const current = tabs.find((tab) => tab.sessionId === active);

  return (
    <section className={styles.dock} aria-label="Lab consoles">
      <div className={styles.tabs} role="tablist" aria-label="Open consoles">
        {tabs.map((tab) => (
          <div key={tab.sessionId} className={styles.tab} data-active={tab.sessionId === active || undefined}>
            <button
              type="button"
              role="tab"
              aria-selected={tab.sessionId === active}
              className={styles.tabButton}
              onClick={() => consoles.setActive(tab.sessionId)}
            >
              <Icon icon={TerminalIcon} size={12} />
              {tab.node}
              {tab.closed && <span className={styles.closed}>closed</span>}
            </button>
            <IconButton
              icon={X}
              size="sm"
              variant="ghost"
              aria-label={`Close the console on ${tab.node}`}
              onClick={() => consoles.close(tab.sessionId)}
            />
          </div>
        ))}
      </div>

      <div className={styles.body}>
        {current ? (
          <Surface key={current.sessionId} sessionId={current.sessionId} node={current.node} />
        ) : (
          <EmptyState
            icon={TerminalIcon}
            size="compact"
            title="No console open"
            description={`Select a device on the canvas and press Console, or double-click it, to log in to a ${labName} device.`}
          />
        )}
      </div>
    </section>
  );
}

interface SurfaceProps {
  sessionId: string;
  node: string;
}

/**
 * Where the terminal is lent to this screen. The element belongs to the
 * registry and goes back to it on unmount, so the session keeps running and
 * keeps receiving output while the user is somewhere else.
 */
function Surface({ sessionId, node }: SurfaceProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const view = useSyncExternalStore(terminals.subscribe, () => terminals.view(sessionId));

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    terminals.adopt(sessionId);
    const unmount = terminals.mount(sessionId, host);
    const observer = new ResizeObserver(() => terminals.refit(sessionId));
    observer.observe(host);
    return () => {
      observer.disconnect();
      unmount();
    };
  }, [sessionId]);

  return (
    <div className={styles.surface}>
      <div className={styles.status}>
        <span className={styles.statusNode}>{node}</span>
        {view.attach === 'attaching' && <span>attaching…</span>}
        {view.attach === 'failed' && <span className={styles.failed}>{view.error}</span>}
        {view.closed && <span className={styles.failed}>{view.closed}</span>}
      </div>
      <div ref={hostRef} className={styles.terminal} data-testid={`console-${node}`} />
    </div>
  );
}
