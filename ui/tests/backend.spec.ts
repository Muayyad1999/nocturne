/**
 * The same shell, against a real engine.
 *
 * The foundation does not need a backend, so this spec is opt-in:
 *
 *   NOCTURNE_E2E_BACKEND=1 pnpm e2e
 *
 * It is here so a screen author can copy it: build the UI, start a server on
 * its own empty data directory, open the app from that server, and drive the
 * screen with real RPC answers. Everything is torn down afterwards.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test } from '@playwright/test';
import { openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe('against a real engine', () => {
  test.skip(!process.env.NOCTURNE_E2E_BACKEND, 'set NOCTURNE_E2E_BACKEND=1 to run this');
  test.describe.configure({ mode: 'serial', timeout: 300_000 });

  let backend: Backend;
  let uiDir: string;

  test.beforeAll(async () => {
    // Building the UI and starting the engine take longer than a test.
    test.setTimeout(300_000);
    // The server serves the built UI, so this is the production bundle
    // talking to the engine over HTTP and WebSocket — no mock anywhere.
    //
    // Built outside `ui/dist`: the server crate embeds that directory, so
    // writing to it would make cargo rebuild the whole binary first.
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 180_000,
    });
    backend = await startBackend({ uiDir });
  });

  test.afterAll(async () => {
    await backend?.stop();
    if (uiDir) await rm(uiDir, { recursive: true, force: true });
  });

  test('connects, and says so in the status bar', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/', { backend });

    await expect(page.getByRole('heading', { level: 1, name: 'Overview' })).toBeVisible();
    await expect(page.getByRole('contentinfo').getByText('Engine online')).toBeVisible();
    // The token was taken out of the address, not left for a bookmark.
    expect(page.url()).not.toContain('token=');

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('answers calls, so a spec can set a screen up before opening it', async ({ request }) => {
    // The same endpoint the HTTP transport uses. A screen's spec calls it to
    // put the engine in the state the screen is supposed to show.
    const response = await request.post(`${backend.url}/api/rpc`, {
      headers: { Authorization: `Bearer ${backend.token}` },
      data: { method: 'settings.all' },
    });
    expect(response.ok(), await response.text()).toBe(true);
    expect(await response.json()).toMatchObject({ ok: true });

    // And an unknown method comes back as a refusal, not a crash.
    const unknown = await request.post(`${backend.url}/api/rpc`, {
      headers: { Authorization: `Bearer ${backend.token}` },
      data: { method: 'nothing.at.all' },
    });
    expect(await unknown.json()).toMatchObject({ ok: false });
  });
});
