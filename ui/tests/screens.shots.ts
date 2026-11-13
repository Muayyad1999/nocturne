/**
 * `pnpm shots` — PNGs of the Devices and Settings screens, in both themes,
 * at 1440×900 and 1280×720, with the assistant dock open and closed.
 *
 * These are driven by a real engine seeded through `/api/rpc`, so what is in
 * the picture is what the backend answered. They are written to
 * `tests/shots/<night|day>/` and are not compared against anything: they
 * exist to be looked at before a change ships.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { test, type Page } from '@playwright/test';
import { openApp, pageLoaded, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, seedCredential, seedDevices } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots', import.meta.url)));

const THEMES: readonly Theme[] = ['dark', 'light'];
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

test.beforeAll(async ({ request }) => {
  test.setTimeout(300_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 180_000,
  });
  backend = await startBackend({ uiDir });

  const rpc = rpcFor(request, backend);
  await seedCredential(rpc, 'Campus switches');
  await seedCredential(rpc, 'Edge routers');
  await seedDevices(rpc, 90);
  await rpc('devices.create', {
    name: 'core-rtr1',
    address: '10.0.0.1',
    site: 'HQ',
    role: 'core',
    profile: 'cisco_ios',
    tags: ['critical', 'rack-1'],
    protected: true,
    notes: 'The one everything hangs off. Maintenance window is Sunday 02:00–04:00.',
  });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

for (const theme of THEMES) {
  test.describe(`${THEME_FOLDER[theme]} theme`, () => {
    test('the device list', async ({ page }) => {
      await openApp(page, '/devices', { backend, theme });
      await page.getByRole('link', { name: 'core-rtr1' }).waitFor();
      await shoot(page, theme, 'devices-list');

      // Narrow: the window a laptop actually has.
      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'devices-list-1280');
      await page.setViewportSize({ width: 1440, height: 900 });

      // With rows selected: the bulk bar.
      await page.getByRole('checkbox', { name: 'Select all rows' }).click();
      await shoot(page, theme, 'devices-list-selected');
      await page.getByRole('button', { name: 'Clear selection' }).click();
    });

    test('the device list with the assistant dock open', async ({ page }) => {
      await openApp(page, '/devices', { backend, theme, dock: true });
      await page.getByRole('link', { name: 'core-rtr1' }).waitFor();
      await shoot(page, theme, 'devices-list-dock');
    });

    test('adding a device, and what a failed test looks like', async ({ page }) => {
      await openApp(page, '/devices', { backend, theme });
      await page.getByRole('button', { name: 'Add device' }).click();
      // The drawer is renamed once the device exists, so it is located as
      // the one open dialog.
      const drawer = page.getByRole('dialog');
      // A name per theme: both runs share one engine, and a duplicate name
      // would be refused instead of tested.
      await drawer
        .getByRole('textbox', { name: 'Name', exact: true })
        .fill(`new-edge-fw-${THEME_FOLDER[theme]}`);
      await drawer.getByRole('textbox', { name: 'Address', exact: true }).fill('192.0.2.77');
      await shoot(page, theme, 'devices-add');

      await drawer.getByRole('button', { name: 'Save and test connection' }).click();
      await drawer.getByRole('status').waitFor({ timeout: 90_000 });
      await shoot(page, theme, 'devices-test-failed');
    });

    test('importing', async ({ page }) => {
      await openApp(page, '/devices', { backend, theme });
      await page.getByRole('button', { name: 'Import' }).click();
      const dialog = page.getByRole('dialog', { name: 'Import devices' });
      await dialog.getByRole('textbox', { name: 'Devices to import' }).fill(
        [
          'name,address,platform,site,role,tags',
          `shot-sw1,10.80.0.1,ios,Imported,access,from-csv`,
          `shot-sw2,10.80.0.2,routeros,Imported,access,from-csv`,
          `core-sw6,10.80.0.3,nxos,Imported,core,`,
          `nameless,,ios,,,`,
        ].join('\n'),
      );
      await dialog.getByRole('table', { name: 'Devices to import' }).waitFor();
      await shoot(page, theme, 'devices-import');
      await dialog.getByRole('button', { name: 'Cancel' }).click();
    });

    test('one device', async ({ page }) => {
      await openApp(page, '/devices', { backend, theme });
      await page.getByRole('searchbox', { name: 'Search devices' }).fill('core-rtr1');
      await page.getByRole('link', { name: 'core-rtr1' }).click();
      await pageLoaded(page);
      await page.getByRole('tab', { name: 'Summary' }).waitFor();
      await shoot(page, theme, 'device-summary');

      await page.getByRole('tab', { name: 'Data' }).click();
      await shoot(page, theme, 'device-data');

      await page.getByRole('tab', { name: 'Notes' }).click();
      await shoot(page, theme, 'device-notes');

      await page.setViewportSize({ width: 1280, height: 720 });
      await page.getByRole('tab', { name: 'Summary' }).click();
      await shoot(page, theme, 'device-summary-1280');
    });

    test('settings', async ({ page }) => {
      await openApp(page, '/settings/general', { backend, theme });
      await shoot(page, theme, 'settings-general');

      await openApp(page, '/settings/security', { backend, theme });
      await shoot(page, theme, 'settings-security');

      await openApp(page, '/settings/credentials', { backend, theme });
      await page.getByRole('cell', { name: 'Campus switches' }).waitFor();
      await shoot(page, theme, 'settings-credentials');

      await openApp(page, '/settings/connections', { backend, theme });
      await shoot(page, theme, 'settings-connections');

      await openApp(page, '/settings/about', { backend, theme });
      await shoot(page, theme, 'settings-about');
    });

    test('the device-type editor', async ({ page }) => {
      await openApp(page, '/settings/device-types', { backend, theme });
      // Wait for a row, not just the table: the table exists while it is
      // still showing skeletons.
      await page.getByRole('cell', { name: 'cisco_ios', exact: true }).waitFor();
      await shoot(page, theme, 'settings-device-types');

      // The editor takes over the section: a profile is too wide for a drawer.
      await page.getByRole('cell', { name: 'cisco_ios', exact: true }).dblclick();
      const yaml = page.getByRole('textbox', { name: 'Device type YAML' });
      await yaml.waitFor();
      await settle(page);
      await shoot(page, theme, 'settings-device-type-editor');

      await yaml.fill('id: Bad Id\nname: Broken\n');
      await page.getByRole('listitem').filter({ hasText: /^id: / }).waitFor();
      await shoot(page, theme, 'settings-device-type-invalid');
    });

    test('the credential editor', async ({ page }) => {
      await openApp(page, '/settings/credentials', { backend, theme });
      await page.getByRole('cell', { name: 'Campus switches' }).dblclick();
      await page.getByRole('dialog').waitFor();
      await settle(page);
      await shoot(page, theme, 'settings-credential-editor');
    });

    test('settings at 1280', async ({ page }) => {
      await openApp(page, '/settings/connections', { backend, theme });
      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'settings-connections-1280');

      await openApp(page, '/settings/credentials', { backend, theme, dock: true });
      await shoot(page, theme, 'settings-credentials-dock');
    });
  });
}
