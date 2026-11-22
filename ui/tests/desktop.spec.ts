/**
 * The desktop shell's two differences from a browser, against the real
 * interface and a real engine.
 *
 * Two faults reached an installed application while every test here was
 * green, and both were differences between a browser and Tauri rather than
 * bugs a browser could show:
 *
 *  1. **No envelope.** The HTTP path answers `{ok, result | error}`; the
 *     Tauri command answers with the result itself and rejects with the
 *     error. The transport expected the envelope, so every call in the
 *     desktop application failed and the first screen said "Nocturne sent a
 *     reply the interface could not read".
 *  2. **A frozen `Object.prototype`.** Tauri can freeze it against
 *     prototype pollution. `@xterm/xterm` assigns to an inherited
 *     `toString`, which then throws, and the Terminal screen died with
 *     "Cannot assign to read only property 'toString'".
 *
 * Neither needed a window to reproduce — only the two behaviours. So this
 * file gives the real bundle a `window.__TAURI_INTERNALS__`, which is how
 * the client chooses the desktop transport, and makes `invoke` behave as
 * Tauri does: resolve with the `Ok` value, reject with the `Err`. The
 * freezing is read from `tauri.conf.json`, so the test always simulates the
 * shell that is actually shipped rather than one imagined here.
 *
 * What it is not: the shell itself. A real `tauri-driver` run would also
 * cover the WebView, the window and the packaging. This covers the two
 * things that have actually gone wrong.
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';
import { startBackend, type Backend } from './support/backend';
import { watchConsole } from './support/app';

const CONFIG = fileURLToPath(new URL('../../apps/desktop/src-tauri/tauri.conf.json', import.meta.url));

/** What the shipped desktop configuration does to `Object.prototype`. */
function freezesThePrototype(): boolean {
  const config = JSON.parse(readFileSync(CONFIG, 'utf8')) as {
    app?: { security?: { freezePrototype?: boolean } };
  };
  return config.app?.security?.freezePrototype === true;
}

let backend: Backend;

test.beforeAll(async () => {
  backend = await startBackend({});
});

test.afterAll(async () => {
  await backend?.stop();
});

/**
 * Loads the interface as the desktop shell presents it.
 *
 * `invoke` here is deliberately written to Tauri's contract rather than to
 * ours: it takes the envelope off and resolves with what is inside, so a
 * transport that expects an envelope fails exactly as it did in the
 * installed application.
 */
async function openDesktop(page: import('@playwright/test').Page) {
  await page.addInitScript(
    ({ url, token, freeze }: { url: string; token: string; freeze: boolean }) => {
      if (freeze) Object.freeze(Object.prototype);
      const call = async (command: string, args: Record<string, unknown>) => {
        // The event plugin is not what is under test; keep `listen` quiet.
        if (command.startsWith('plugin:event|')) return 0;
        const reply = await fetch(`${url}/api/rpc`, {
          method: 'POST',
          headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` },
          body: JSON.stringify({ method: args.method, params: args.params ?? null }),
        });
        const envelope = (await reply.json()) as
          | { ok: true; result: unknown }
          | { ok: false; error: unknown };
        // Tauri resolves with the `Ok` value and rejects with the `Err`.
        if (envelope.ok) return envelope.result;
        throw envelope.error;
      };
      Object.defineProperty(window, '__TAURI_INTERNALS__', {
        value: { invoke: call, transformCallback: (fn: unknown) => fn },
        configurable: true,
      });
    },
    { url: backend.url, token: backend.token, freeze: freezesThePrototype() },
  );
  await page.goto(`${backend.url}/#/`);
}

/**
 * Every screen on the rail, with the heading each one shows.
 *
 * The heading is how this test waits. Routing is by hash, so `goto` to a
 * new one does not reload the page and returns before the screen has
 * rendered — the loop would otherwise run ahead and check the *previous*
 * screen for errors. Waiting for the right heading is what makes each step
 * about the screen it names. The first version of this test did not, and
 * passed against a Terminal that was failing.
 */
const SCREENS = [
  ['Overview', '/#/'],
  ['Devices', '/#/devices'],
  ['Terminal', '/#/terminal'],
  ['Topology', '/#/topology'],
  ['Monitoring', '/#/monitoring'],
  ['Config', '/#/config'],
  ['Changes', '/#/changes'],
  ['Automation', '/#/automation'],
  ['Lab', '/#/lab'],
  ['IPAM', '/#/ipam'],
  ['Tools', '/#/tools'],
  ['Operations', '/#/ops'],
] as const;

test('every screen loads over the desktop transport, which carries no envelope', async ({ page }) => {
  const console = watchConsole(page);
  await openDesktop(page);

  for (const [name, route] of SCREENS) {
    await page.goto(`${backend.url}${route}`);
    // This heading is the wait, not a decoration: see the note on SCREENS.
    // A screen that threw shows the error card instead, so failing to find
    // it is itself the report.
    await expect(page.getByRole('heading', { level: 1, name, exact: true }), name).toBeVisible();

    // Then the sentences a failure uses: the error boundary, and a reply
    // the transport could not read, which is what the envelope gave.
    for (const sentence of ['This screen ran into a problem', 'could not be read', 'could not read']) {
      await expect(page.getByText(sentence), `${name}: ${sentence}`).toHaveCount(0);
    }
  }

  expect(console.messages, backend.log()).toEqual([]);
});

test('the engine answers over it, so the interface is not merely drawing itself', async ({ page }) => {
  await openDesktop(page);
  // A number that can only have come from the engine: the methods this
  // build registers. If the transport were broken, the rail would still
  // draw and this would be empty.
  const methods = await page.evaluate(async () => {
    const invoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (c: string, a: unknown) => Promise<unknown> } })
      .__TAURI_INTERNALS__.invoke;
    return (await invoke('rpc', { method: 'app.methods', params: null })) as string[];
  });
  expect(methods.length).toBeGreaterThan(300);
  expect(methods).toContain('devices.list');
  // And the free edition carries none of the ISP surface.
  expect(methods.filter((method) => method.startsWith('isp.'))).toEqual([]);
});
