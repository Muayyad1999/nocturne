/**
 * A command line on the device under the pointer, without leaving the map.
 *
 * This is the other half of what The Dude is used for all day: see something
 * wrong on the drawing, right-click it, type. Nothing here is a second way
 * into a device — the session is an ordinary `terminal.open` session in the
 * backend and the xterm instance belongs to the registry the Terminal screen
 * owns (`pages/terminal/sessions.ts`), which is why it survives being hidden,
 * keeps its scrollback, and is the *same* session the Terminal screen shows
 * when you send it over there.
 */

import { ExternalLink, Minimize2, TerminalSquare, X } from 'lucide-react';
import { useEffect, useRef, useSyncExternalStore } from 'react';
import { IconButton } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { LabPill } from '@/design/StatusPill';
import { useTerminalAppearance } from '@/pages/terminal/appearance';
import { terminals } from '@/pages/terminal/sessions';
// xterm's own stylesheet and the sixteen ANSI colours it paints with. Both
// are imported by whichever screen shows a terminal; without the first,
// xterm's hidden character-measuring element is painted over the output.
import '@/pages/terminal/tokens.css';
import styles from './SessionDock.module.css';

export interface SessionDockProps {
  sessionId: string;
  /** The device this session is on, as the drawing calls it. */
  name: string;
  lab: boolean;
  /** Leaves the session running in the backend and takes the dock away. */
  onHide: () => void;
  /** Ends the session on the device. */
  onClose: () => void;
  /** Hands this very session over to the Terminal screen. */
  onHandOver: () => void;
}

export function SessionDock({
  sessionId,
  name,
  lab,
  onHide,
  onClose,
  onHandOver,
}: SessionDockProps) {
  const appearance = useTerminalAppearance();
  useEffect(() => terminals.setAppearance(appearance), [appearance]);

  const view = useSyncExternalStore(terminals.subscribe, () => terminals.view(sessionId));
  const hostRef = useRef<HTMLDivElement>(null);

  // The element belongs to the registry and goes back to it on unmount, so
  // the session keeps running and keeps receiving output while this screen is
  // showing something else.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    terminals.adopt(sessionId);
    const unmount = terminals.mount(sessionId, host);
    const observer = new ResizeObserver(() => terminals.refit(sessionId));
    observer.observe(host);
    terminals.focus(sessionId);
    return () => {
      observer.disconnect();
      unmount();
    };
  }, [sessionId]);

  return (
    <section className={styles.dock} aria-label={`Terminal on ${name}`}>
      <header className={styles.head}>
        <Icon icon={TerminalSquare} size={13} />
        <span className={styles.name}>{name}</span>
        {lab && <LabPill />}
        <span className={styles.state}>
          {view.attach === 'attaching' && 'attaching…'}
          {view.attach === 'failed' && <span className={styles.failed}>{view.error}</span>}
          {view.closed && <span className={styles.failed}>{view.closed}</span>}
        </span>
        <IconButton
          size="sm"
          variant="ghost"
          icon={ExternalLink}
          aria-label="Open this session on the Terminal screen"
          title="Open this session on the Terminal screen"
          onClick={onHandOver}
        />
        <IconButton
          size="sm"
          variant="ghost"
          icon={Minimize2}
          aria-label="Hide the terminal, leaving the session open"
          title="Hide the terminal, leaving the session open"
          onClick={onHide}
        />
        <IconButton
          size="sm"
          variant="ghost"
          icon={X}
          aria-label={`Close the session on ${name}`}
          title="End the session on the device"
          onClick={onClose}
        />
      </header>
      <div ref={hostRef} className={styles.surface} data-testid={`map-terminal-${name}`} />
    </section>
  );
}
