/**
 * The roads text can take to a device, counted in the source.
 *
 * The paste funnel was rebuilt after text reached a device before the person
 * had confirmed it. What keeps that from coming back is not only the funnel's
 * own tests but that nothing goes around it — and that can only be seen by
 * looking at who calls the methods that type. This test reads every source
 * file of the interface and holds the list:
 *
 *   terminal.input          the registry's own send (`sessions.ts`), which
 *                           takes keystrokes and what the funnel hands it
 *   terminal.input_many     the Terminal screen's broadcast, handed to the
 *                           registry as its way to send to several sessions
 *   terminal.snippets.send  `snippetRoad.ts`, used only through
 *                           `sendSnippet`, which asks first
 *
 * A new caller is not necessarily wrong. It has to be added here, by
 * someone who has read `docs/dev/features/terminal.md` ("one road for
 * pasted text").
 */

import { describe, expect, it } from 'vitest';

const sources = import.meta.glob<string>('/src/**/*.{ts,tsx}', { query: '?raw', import: 'default', eager: true });

/** The files that mention `needle` in code: not tests, not the type declarations, not comments. */
function callers(needle: string): string[] {
  return Object.entries(sources)
    .filter(([path]) => !/\.test\.tsx?$/.test(path) && !path.includes('/api/types/') && !path.includes('/dev/'))
    .filter(([, source]) =>
      source
        .split('\n')
        .filter((line) => !/^\s*(\/\/|\*|\/\*)/.test(line))
        .some((line) => line.includes(needle)),
    )
    .map(([path]) => path.replace(/^.*\/src\//, ''))
    .sort();
}

describe('the ways text reaches a device', () => {
  it('is typed by one caller of terminal.input', () => {
    expect(callers("rpc('terminal.input'")).toEqual(['pages/terminal/sessions.ts']);
    // No other spelling of the call anywhere.
    expect(callers("'terminal.input'")).toEqual(['pages/terminal/sessions.ts']);
  });

  it('is broadcast by one caller of terminal.input_many', () => {
    expect(callers("'terminal.input_many'")).toEqual(['pages/terminal/index.tsx']);
  });

  it('is sent as a snippet by one caller of terminal.snippets.send, behind the question', () => {
    expect(callers("rpc<SnippetSendResult>('terminal.snippets.send'")).toEqual(['pages/terminal/snippetRoad.ts']);
    // The only other mention asks whether the method exists.
    expect(callers("'terminal.snippets.send'")).toEqual([
      'pages/terminal/SnippetsDrawer.tsx',
      'pages/terminal/snippetRoad.ts',
    ]);
    const drawer = sources['/src/pages/terminal/SnippetsDrawer.tsx'] ?? '';
    expect(drawer).toContain("useHasMethod('terminal.snippets.send')");
    expect(drawer).not.toContain("rpc<SnippetSendResult>('terminal.snippets.send'");
  });

  it('uses the snippet road only through sendSnippet, which asks before it sends', () => {
    const users = callers('snippetRoad').filter((path) => path !== 'pages/terminal/snippetRoad.ts');
    expect(users).toEqual(['pages/terminal/SnippetUse.tsx', 'pages/terminal/SnippetsDrawer.tsx']);
    for (const path of users) {
      const source = sources[`/src/${path}`] ?? '';
      // Handed whole to `sendSnippet`; never picked apart to call `send` directly.
      expect(source, path).toContain('sendSnippet(snippetRoad,');
      expect(source, path).not.toMatch(/snippetRoad\s*\./);
    }
  });

  it('hands pasted text to the funnel and never to the registry’s own input', () => {
    // `terminals.input` is for keystrokes: xterm's `onData`, inside the registry.
    expect(callers('terminals.input(')).toEqual([]);
    expect(callers('terminals.paste(')).toEqual(['pages/terminal/Pane.tsx', 'pages/terminal/index.tsx']);
  });
});
