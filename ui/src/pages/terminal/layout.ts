/**
 * Which sessions are in which pane, and which pane the keyboard is in.
 *
 * Sessions themselves live in the backend; this is only the arrangement, and
 * it is remembered so that leaving the screen — or reloading the window —
 * brings back the same tabs in the same places.
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { storageKey } from '@/lib/storage';

/** Up to four, as the spec says: more and none of them is readable. */
export const MAX_PANES = 4;

export interface Pane {
  id: string;
  /** Session ids, left to right. */
  tabs: string[];
  /** The tab on screen in this pane. */
  active: string | null;
}

interface LayoutState {
  panes: Pane[];
  /** The pane that takes the keyboard and the toolbar's actions. */
  focused: string;
  /** Typing goes to every pane that is not excluded. */
  broadcast: boolean;
  /** Panes held back from a broadcast, by id. */
  excluded: string[];
  sftpOpen: boolean;
}

interface LayoutActions {
  /** Puts a session in a pane (the focused one by default) and selects it. */
  open: (sessionId: string, paneId?: string) => void;
  /** Takes a session out of the arrangement. The pane closes with its last tab. */
  close: (sessionId: string) => void;
  select: (paneId: string, sessionId: string) => void;
  focus: (paneId: string) => void;
  /** Moves a session into a pane of its own. Nothing happens at four panes. */
  split: (sessionId: string) => void;
  /** Moves a session into an existing pane (a tab dragged onto it). */
  moveTo: (sessionId: string, paneId: string) => void;
  closePane: (paneId: string) => void;
  /** Keeps only sessions the backend still has, and adopts ones it has that we do not. */
  reconcile: (sessionIds: readonly string[]) => void;
  setBroadcast: (on: boolean) => void;
  toggleExcluded: (paneId: string) => void;
  setSftpOpen: (open: boolean) => void;
}

export type TerminalLayout = LayoutState & LayoutActions;

let counter = 0;
const paneId = () => `pane-${(counter += 1)}`;

const FIRST: Pane = { id: 'pane-0', tabs: [], active: null };

const initial: LayoutState = {
  panes: [FIRST],
  focused: FIRST.id,
  broadcast: false,
  excluded: [],
  sftpOpen: false,
};

/** The pane holding a session, if any. */
function paneOf(panes: readonly Pane[], sessionId: string): Pane | undefined {
  return panes.find((pane) => pane.tabs.includes(sessionId));
}

function withoutTab(pane: Pane, sessionId: string): Pane {
  if (!pane.tabs.includes(sessionId)) return pane;
  const tabs = pane.tabs.filter((tab) => tab !== sessionId);
  const active =
    pane.active === sessionId
      ? (tabs[Math.min(pane.tabs.indexOf(sessionId), tabs.length - 1)] ?? null)
      : pane.active;
  return { ...pane, tabs, active };
}

/** Drops empty panes, keeping at least one, and keeps `focused` real. */
function tidy(state: LayoutState): LayoutState {
  const kept = state.panes.filter((pane) => pane.tabs.length > 0);
  const panes = kept.length > 0 ? kept : [{ ...(state.panes[0] ?? FIRST), tabs: [], active: null }];
  const focused = panes.some((pane) => pane.id === state.focused) ? state.focused : (panes[0]?.id ?? FIRST.id);
  const ids = new Set(panes.map((pane) => pane.id));
  return { ...state, panes, focused, excluded: state.excluded.filter((id) => ids.has(id)) };
}

export const useTerminalLayout = create<TerminalLayout>()(
  persist(
    (set) => ({
      ...initial,

      open: (sessionId, target) =>
        set((state) => {
          const wanted = target ?? state.focused;
          const existing = paneOf(state.panes, sessionId);
          if (existing) {
            return {
              panes: state.panes.map((pane) =>
                pane.id === existing.id ? { ...pane, active: sessionId } : pane,
              ),
              focused: existing.id,
            };
          }
          const panes = state.panes.map((pane) =>
            pane.id === wanted ? { ...pane, tabs: [...pane.tabs, sessionId], active: sessionId } : pane,
          );
          return { panes, focused: wanted };
        }),

      close: (sessionId) =>
        set((state) => tidy({ ...state, panes: state.panes.map((pane) => withoutTab(pane, sessionId)) })),

      select: (paneId_, sessionId) =>
        set((state) => ({
          panes: state.panes.map((pane) =>
            pane.id === paneId_ && pane.tabs.includes(sessionId) ? { ...pane, active: sessionId } : pane,
          ),
          focused: paneId_,
        })),

      focus: (paneId_) =>
        set((state) => (state.panes.some((pane) => pane.id === paneId_) ? { focused: paneId_ } : {})),

      split: (sessionId) =>
        set((state) => {
          if (state.panes.length >= MAX_PANES) return {};
          const from = paneOf(state.panes, sessionId);
          // A pane with one tab would only move its tab to an identical pane.
          if (!from || from.tabs.length < 2) return {};
          const pane: Pane = { id: paneId(), tabs: [sessionId], active: sessionId };
          const panes = state.panes.map((each) => withoutTab(each, sessionId));
          const at = panes.findIndex((each) => each.id === from.id);
          panes.splice(at + 1, 0, pane);
          return { panes, focused: pane.id };
        }),

      moveTo: (sessionId, target) =>
        set((state) => {
          const from = paneOf(state.panes, sessionId);
          if (!from || from.id === target) return {};
          const panes = state.panes.map((pane) => {
            if (pane.id === target) return { ...pane, tabs: [...pane.tabs, sessionId], active: sessionId };
            return withoutTab(pane, sessionId);
          });
          return tidy({ ...state, panes, focused: target });
        }),

      closePane: (paneId_) =>
        set((state) => {
          if (state.panes.length < 2) return {};
          const closing = state.panes.find((pane) => pane.id === paneId_);
          if (!closing) return {};
          // The sessions are not closed with the pane: they move next door.
          const rest = state.panes.filter((pane) => pane.id !== paneId_);
          const first = rest[0];
          if (!first) return {};
          const panes = rest.map((pane) =>
            pane.id === first.id
              ? { ...pane, tabs: [...pane.tabs, ...closing.tabs], active: closing.active ?? pane.active }
              : pane,
          );
          return tidy({ ...state, panes, focused: first.id });
        }),

      reconcile: (sessionIds) =>
        set((state) => {
          const live = new Set(sessionIds);
          const panes = state.panes.map((pane) => {
            const tabs = pane.tabs.filter((tab) => live.has(tab));
            const active = pane.active && tabs.includes(pane.active) ? pane.active : (tabs[0] ?? null);
            return { ...pane, tabs, active };
          });
          const known = new Set(panes.flatMap((pane) => pane.tabs));
          const missing = sessionIds.filter((id) => !known.has(id));
          if (missing.length > 0) {
            const first = panes[0] ?? FIRST;
            panes[0] = { ...first, tabs: [...first.tabs, ...missing], active: first.active ?? missing[0] ?? null };
          }
          return tidy({ ...state, panes });
        }),

      setBroadcast: (broadcast) => set({ broadcast }),

      toggleExcluded: (paneId_) =>
        set((state) => ({
          excluded: state.excluded.includes(paneId_)
            ? state.excluded.filter((id) => id !== paneId_)
            : [...state.excluded, paneId_],
        })),

      setSftpOpen: (sftpOpen) => set({ sftpOpen }),
    }),
    {
      name: storageKey('terminal.layout'),
      version: 1,
      partialize: ({ panes, focused, sftpOpen }) => ({ panes, focused, sftpOpen }),
      // Stored state is untrusted: keep only what has the right shape, and
      // never start with a broadcast nobody asked for.
      merge: (stored, current) => {
        const saved = (stored ?? {}) as Partial<Record<keyof LayoutState, unknown>>;
        const panes = Array.isArray(saved.panes)
          ? saved.panes
              .filter((pane): pane is Pane => isPane(pane))
              .slice(0, MAX_PANES)
              .map((pane) => ({
                id: pane.id,
                tabs: [...new Set(pane.tabs)],
                active: pane.active && pane.tabs.includes(pane.active) ? pane.active : (pane.tabs[0] ?? null),
              }))
          : [];
        // Keep the id counter ahead of anything that was restored.
        for (const pane of panes) {
          const number = Number(/^pane-(\d+)$/.exec(pane.id)?.[1] ?? 0);
          if (Number.isFinite(number) && number > counter) counter = number;
        }
        const next: LayoutState = {
          ...current,
          panes: panes.length > 0 ? panes : current.panes,
          focused: typeof saved.focused === 'string' ? saved.focused : current.focused,
          sftpOpen: typeof saved.sftpOpen === 'boolean' ? saved.sftpOpen : false,
          broadcast: false,
          excluded: [],
        };
        return { ...current, ...tidyKeepEmpty(next) };
      },
    },
  ),
);

/** Like `tidy`, but a restored empty pane is kept: the tabs arrive a moment later. */
function tidyKeepEmpty(state: LayoutState): LayoutState {
  const focused = state.panes.some((pane) => pane.id === state.focused)
    ? state.focused
    : (state.panes[0]?.id ?? FIRST.id);
  return { ...state, focused };
}

function isPane(value: unknown): value is Pane {
  if (typeof value !== 'object' || value === null) return false;
  const pane = value as Partial<Pane>;
  return (
    typeof pane.id === 'string' &&
    Array.isArray(pane.tabs) &&
    pane.tabs.every((tab) => typeof tab === 'string') &&
    (pane.active === null || typeof pane.active === 'string')
  );
}

/** The session the toolbar's actions apply to: the focused pane's tab. */
export function activeSessionId(state: TerminalLayout): string | null {
  return state.panes.find((pane) => pane.id === state.focused)?.active ?? null;
}

/** The sessions a broadcast would type into. */
export function broadcastTargets(state: TerminalLayout): string[] {
  return state.panes
    .filter((pane) => !state.excluded.includes(pane.id))
    .map((pane) => pane.active)
    .filter((id): id is string => id !== null);
}
