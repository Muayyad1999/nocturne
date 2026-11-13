/**
 * Shell layout state: whether the nav rail is collapsed, whether the
 * assistant dock is open and how wide it is. Remembered between sessions.
 */

import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { storageKey } from '@/lib/storage';

export const DOCK_MIN_WIDTH = 320;
export const DOCK_MAX_WIDTH = 640;
export const DOCK_DEFAULT_WIDTH = 380;

interface LayoutState {
  navCollapsed: boolean;
  dockOpen: boolean;
  dockWidth: number;
  toggleNav: () => void;
  toggleDock: () => void;
  setDockOpen: (open: boolean) => void;
  setDockWidth: (width: number) => void;
}

function clampDockWidth(width: number): number {
  return Math.round(Math.min(DOCK_MAX_WIDTH, Math.max(DOCK_MIN_WIDTH, width)));
}

export const useLayout = create<LayoutState>()(
  persist(
    (set) => ({
      navCollapsed: false,
      dockOpen: false,
      dockWidth: DOCK_DEFAULT_WIDTH,
      toggleNav: () => set((state) => ({ navCollapsed: !state.navCollapsed })),
      toggleDock: () => set((state) => ({ dockOpen: !state.dockOpen })),
      setDockOpen: (dockOpen) => set({ dockOpen }),
      setDockWidth: (width) => set({ dockWidth: clampDockWidth(width) }),
    }),
    {
      name: storageKey('layout'),
      version: 1,
      partialize: ({ navCollapsed, dockOpen, dockWidth }) => ({ navCollapsed, dockOpen, dockWidth }),
      // Stored state is untrusted: keep only values of the right shape.
      merge: (stored, current) => {
        const saved = (stored ?? {}) as Partial<Record<keyof LayoutState, unknown>>;
        return {
          ...current,
          navCollapsed:
            typeof saved.navCollapsed === 'boolean' ? saved.navCollapsed : current.navCollapsed,
          dockOpen: typeof saved.dockOpen === 'boolean' ? saved.dockOpen : current.dockOpen,
          dockWidth:
            typeof saved.dockWidth === 'number' && Number.isFinite(saved.dockWidth)
              ? clampDockWidth(saved.dockWidth)
              : current.dockWidth,
        };
      },
    },
  ),
);
