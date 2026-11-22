/**
 * The interface where the WebView reports a locale that is not a language.
 *
 * WebKitGTK gives `navigator.language` the process locale, so a Linux desktop
 * started with `LANG` unset, `C`, `C.UTF-8` or `POSIX` reports `"C"`. uPlot
 * hands that to `new Intl.NumberFormat(...)` while its module is evaluated,
 * `Intl` throws a RangeError, and the Monitoring screen — whose code is what
 * loads uPlot — showed "This screen ran into a problem" instead of itself.
 *
 * Chromium never reports such a tag on its own, which is how every test here
 * stayed green. So this spec makes it: the browser context is given the
 * locale `C`, and Chromium's own `navigator.language` then answers what
 * WebKitGTK's does — nothing in the page is replaced to get there. The test
 * checks that premise before it relies on it.
 *
 * It runs the real bundle, against a real engine, because the bundle is where
 * it is decided: the repair has to be evaluated before uPlot is, and a build
 * that put uPlot in a chunk of its own would evaluate it first while the
 * sources still read correctly. The unit test beside the chart covers the
 * sources; only this covers what ships.
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

test.describe.configure({ mode: 'serial', timeout: 120_000 });
// What a Linux desktop with `LANG` unset hands its WebView.
test.use({ locale: 'C' });

let backend: Backend;
let uiDir: string | undefined;

test.beforeAll(async () => {
  test.setTimeout(600_000);
  let dist = process.env.NOCTURNE_UI_DIST;
  if (!dist) {
    // Built outside `ui/dist`: the server crate embeds that directory, so
    // writing there would make cargo rebuild the binary first.
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
    dist = uiDir;
  }
  backend = await startBackend({ uiDir: dist });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

test('Monitoring opens where navigator.language is "C"', async ({ page }) => {
  const seen = watchConsole(page);

  await openApp(page, '/', { backend });
  // The premise, in this browser: the tag is reported, and Intl refuses it.
  // Asked of the browser's own accessor, so it holds wherever a repair runs.
  expect(
    await page.evaluate(() => {
      const reported = Object.getOwnPropertyDescriptor(Navigator.prototype, 'language')?.get?.call(navigator);
      try {
        new Intl.NumberFormat(reported as string);
        return `${String(reported)} accepted`;
      } catch (error) {
        return `${String(reported)} refused: ${(error as Error).name}`;
      }
    }),
  ).toBe('C refused: RangeError');

  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Monitoring' }).click();

  // One value to wait on, so a failure shows the error card and not only a missing heading.
  await expect
    .poll(async () => ({
      screen: await page.getByRole('heading', { level: 1 }).allTextContents(),
      problem: await page.getByText('This screen ran into a problem').count(),
    }))
    .toEqual({ screen: ['Monitoring'], problem: 0 });
  await expect(page.getByRole('tab', { name: 'Charts' })).toBeVisible();
  // What a library reads now is a tag Intl takes.
  expect(await page.evaluate(() => new Intl.NumberFormat(navigator.language).format(1234.5))).toBe('1,234.5');
  expect(seen.messages).toEqual([]);
});
