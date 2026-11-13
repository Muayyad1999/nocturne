/**
 * The terminal's looks, resolved from design tokens.
 *
 * xterm paints its own glyphs, so it cannot be given `var(--bg-0)`: it needs
 * the resolved values. `useTokens` hands them over and recomputes them when
 * the theme changes, which is what keeps the terminal in step with the rest of
 * the app.
 *
 * The sixteen ANSI colours are tokens too (`--term-*`, in `terminal.css`).
 * They belong in `design/tokens.css` with the others; they are here because
 * that file belongs to another slice while this screen is being written.
 */

import type { ITheme } from '@xterm/xterm';
import { useMemo } from 'react';
import { useTokens } from '@/design/theme';

const ANSI = [
  'black',
  'red',
  'green',
  'yellow',
  'blue',
  'magenta',
  'cyan',
  'white',
  'bright-black',
  'bright-red',
  'bright-green',
  'bright-yellow',
  'bright-blue',
  'bright-magenta',
  'bright-cyan',
  'bright-white',
] as const;

const KEYS = [
  '--bg-0',
  '--text-1',
  '--text-3',
  '--accent',
  '--selection',
  '--font-mono',
  ...ANSI.map((name) => `--term-${name}` as const),
] as const;

/** The ANSI entries of xterm's theme: every colour key, none of the lists. */
type AnsiKey = Exclude<
  { [Key in keyof ITheme]-?: ITheme[Key] extends string | undefined ? Key : never }[keyof ITheme],
  undefined
>;

/** The camelCase name xterm knows each ANSI colour by. */
function themeKey(name: (typeof ANSI)[number]): AnsiKey {
  return name.replace(/-(.)/g, (_, letter: string) => letter.toUpperCase()) as AnsiKey;
}

export interface TerminalAppearance {
  theme: ITheme;
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
}

/** 13px is the app's base size; a terminal reads best a touch looser than UI text. */
export const TERMINAL_FONT_SIZE = 13;
export const TERMINAL_LINE_HEIGHT = 1.3;

export function useTerminalAppearance(): TerminalAppearance {
  const tokens = useTokens(KEYS);
  // `useTokens` only changes with the theme, and so must this: applying it
  // walks every terminal on screen.
  return useMemo(() => build(tokens), [tokens]);
}

function build(tokens: Record<(typeof KEYS)[number], string>): TerminalAppearance {
  const theme: ITheme = {
    background: tokens['--bg-0'],
    foreground: tokens['--text-1'],
    cursor: tokens['--accent'],
    cursorAccent: tokens['--bg-0'],
    selectionBackground: tokens['--selection'],
    scrollbarSliderBackground: tokens['--selection'],
    scrollbarSliderHoverBackground: tokens['--accent'],
    scrollbarSliderActiveBackground: tokens['--accent'],
  };
  for (const name of ANSI) {
    const value = tokens[`--term-${name}`];
    if (value) theme[themeKey(name)] = value;
  }

  return {
    theme,
    fontFamily: tokens['--font-mono'] || 'monospace',
    fontSize: TERMINAL_FONT_SIZE,
    lineHeight: TERMINAL_LINE_HEIGHT,
  };
}
