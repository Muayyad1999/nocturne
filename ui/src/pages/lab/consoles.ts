/**
 * Which consoles a lab has open — according to the backend, not to this
 * window.
 *
 * A lab node is an inventory device, so its console is an ordinary
 * `terminal.*` session. That is the whole trick: the sessions a lab has open
 * are whatever `terminal.list` says, filtered to this lab's devices. Close
 * the window, reopen it, and the tabs come back because the sessions never
 * went anywhere — and the scrollback comes back with them through
 * `terminal.attach`.
 */

import type { SessionInfo } from '@/api/types/terminal';
import type { LabView } from '@/api/types/lab';

export interface ConsoleTab {
  sessionId: string;
  /** The lab node this console is on. */
  node: string;
  deviceId: string;
  /** The session has ended but is still listed, so the last output can be read. */
  closed: boolean;
  openedAt: number;
}

/** Node name → device id, from a {@link LabView}. */
export function deviceNodes(lab: Pick<LabView, 'devices'>): Map<string, string> {
  return new Map(Object.entries(lab.devices).map(([node, deviceId]) => [deviceId, node]));
}

/**
 * The lab's console tabs, oldest first — the order the backend lists sessions
 * in, so tabs do not jump about between refreshes.
 */
export function tabsForLab(sessions: readonly SessionInfo[], lab: Pick<LabView, 'devices'>): ConsoleTab[] {
  const nodes = deviceNodes(lab);
  const tabs: ConsoleTab[] = [];
  for (const session of sessions) {
    const node = session.device_id ? nodes.get(session.device_id) : undefined;
    if (!node || !session.device_id) continue;
    tabs.push({
      sessionId: session.id,
      node,
      deviceId: session.device_id,
      closed: session.state === 'closed',
      openedAt: session.opened_at,
    });
  }
  return tabs;
}

/**
 * A session already open on this node, if there is one: opening a second
 * console on a device the user already has one on would be two tabs showing
 * the same device, and the backend allows it, so this is a UI choice. A
 * closed session is not reused — its far end has hung up.
 */
export function openSessionFor(
  sessions: readonly SessionInfo[],
  deviceId: string,
): SessionInfo | undefined {
  return sessions.find((session) => session.device_id === deviceId && session.state === 'open');
}

/**
 * Which tab to show after one is closed: the one before it, else the one
 * after, else none.
 */
export function neighbourTab(tabs: readonly ConsoleTab[], closing: string): string | null {
  const index = tabs.findIndex((tab) => tab.sessionId === closing);
  if (index < 0) return tabs[0]?.sessionId ?? null;
  const before = tabs[index - 1];
  const after = tabs[index + 1];
  return before?.sessionId ?? after?.sessionId ?? null;
}
