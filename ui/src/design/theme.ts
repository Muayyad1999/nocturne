/**
 * The theme: Night (default) or Day. The choice is remembered and reflected
 * as `data-theme` on <html>, which is what tokens.css keys on.
 */

import { useMemo } from 'react';
import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import { storageKey } from '@/lib/storage';

export type Theme = 'dark' | 'light';

export const THEME_NAME: Record<Theme, string> = { dark: 'Night', light: 'Day' };

interface ThemeState {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  toggleTheme: () => void;
}

function isTheme(value: unknown): value is Theme {
  return value === 'dark' || value === 'light';
}

export const useTheme = create<ThemeState>()(
  persist(
    (set) => ({
      theme: 'dark',
      setTheme: (theme) => set({ theme }),
      toggleTheme: () => set((state) => ({ theme: state.theme === 'dark' ? 'light' : 'dark' })),
    }),
    {
      name: storageKey('theme'),
      version: 1,
      partialize: ({ theme }) => ({ theme }),
      merge: (stored, current) => {
        const theme = (stored as Partial<ThemeState> | undefined)?.theme;
        return { ...current, theme: isTheme(theme) ? theme : current.theme };
      },
    },
  ),
);

/** Applies the theme to the document now and on every change. Call once at start-up. */
export function bindTheme(): () => void {
  const apply = (theme: Theme) => {
    document.documentElement.dataset.theme = theme;
  };
  apply(useTheme.getState().theme);
  return useTheme.subscribe((state) => apply(state.theme));
}

/**
 * Resolved values of design tokens, for code that cannot use `var()`: canvas
 * drawing (charts, the terminal). Recomputed when the theme changes.
 *
 *   const { '--bg-0': background } = useTokens(['--bg-0', '--text-1']);
 */
export function useTokens<Name extends string>(names: readonly Name[]): Record<Name, string> {
  const theme = useTheme((state) => state.theme);
  const key = names.join(' ');
  return useMemo(() => {
    const computed = getComputedStyle(document.documentElement);
    const values = {} as Record<Name, string>;
    for (const name of key.split(' ') as Name[]) {
      values[name] = computed.getPropertyValue(name).trim();
    }
    return values;
    // `theme` is the trigger: the same names resolve differently per theme.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, theme]);
}
