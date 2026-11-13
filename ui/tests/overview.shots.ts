/**
 * `pnpm shots` — the Overview, photographed in both themes, at both sizes,
 * with and without the assistant dock, and in the three states that decide
 * whether this screen is honest:
 *
 *   first-run      an inventory with nothing in it: two doors, no zeroes
 *   nearly-empty   one device and nothing else — the state a new user is in
 *                  for the first hour, and the easiest one to get wrong: the
 *                  alerting is *there and quiet* while the compliance score
 *                  *does not exist yet*, and the two must not look alike
 *   working        a real estate: devices, an alert, a change, a backup, a
 *                  compliance run, findings, a job running
 *   capped         more alerts than the engine will count, so the number on
 *                  the tile is a floor and says so
 *
 * Two engines are started, because "nothing has ever happened here" cannot be
 * staged on an engine where something has. Everything in these pictures was
 * produced by the real backend: no fixtures, no mock transport.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/overview', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 900_000 });

/** The engine with a working estate on it. */
let working: Backend;
/** A second engine where nothing has ever happened. */
let fresh: Backend;
let uiDir: string;
let prebuilt: string | undefined;

async function finished(rpc: Rpc, started: { job_id: string }): Promise<void> {
  await expect
    .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: started.job_id })).state, {
      timeout: 180_000,
      intervals: [500, 1_000, 2_000],
    })
    .toBe('succeeded');
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(900_000);
  if (process.env.NOCTURNE_UI_DIST) {
    prebuilt = process.env.NOCTURNE_UI_DIST;
  } else {
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
  }
  working = await startBackend({ uiDir: prebuilt ?? uiDir });
  fresh = await startBackend({ uiDir: prebuilt ?? uiDir });

  for (const backend of [working, fresh]) {
    const rpc = rpcFor(request, backend);
    // Nothing moves between the shot and the picture.
    await rpc('settings.set', { section: 'monitoring', value: { enabled: false } });
    await rpc('settings.set', { section: 'insights', value: { auto_scan: false } });
  }

  const rpc = rpcFor(request, working);
  for (const [name, address, site, role] of [
    ['core-sw1', '10.0.0.1', 'HQ', 'core'],
    ['dist-sw1', '10.0.0.2', 'HQ', 'distribution'],
    ['access-sw2', '10.0.1.2', 'Branch North', 'access'],
    ['edge-rtr1', '10.0.2.1', 'Branch South', 'edge'],
  ]) {
    await rpc('devices.create', { name, address, site, role, profile: 'cisco_ios' });
  }
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'SW1' });
  await rpc('labs.start', { id: lab.id });

  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  const id = (name: string) => devices.find((device) => device.name === name)?.id;

  await rpc('alerts.raise', {
    device_id: id('core-sw1'),
    severity: 'critical',
    title: 'core-sw1 is not answering',
    rule: 'reachability',
    dedup_key: 'reach:core-sw1',
  });
  await rpc('alerts.raise', {
    device_id: id('access-sw2'),
    severity: 'high',
    title: 'Interface Gi0/3 is flapping',
    rule: 'interface_flap',
    dedup_key: 'flap:access-sw2',
  });
  await rpc('changes.create', {
    title: 'Describe the uplinks',
    ticket: 'NET-412',
    targets: [
      {
        device_id: id('Campus/SW1'),
        intent: { type: 'lines', lines: ['interface FastEthernet0/1', ' description uplink'] },
      },
    ],
  });
  await finished(
    rpc,
    await rpc<{ job_id: string }>('config.backup', {
      device_ids: [id('Campus/SW1')],
      kinds: ['running'],
    }),
  );
  await rpc('compliance.run', {});
  await finished(rpc, await rpc<{ job_id: string }>('insights.scan', {}));
  // One real poll of the simulated switch, so the devices tile shows a state
  // that was measured rather than five devices nobody has ever looked at.
  // Only that one: whether 10.0.0.1 answers depends on the machine the shots
  // are taken on, and a picture must not.
  await finished(
    rpc,
    await rpc<{ job_id: string }>('monitor.poll_now', { device_ids: [id('Campus/SW1')] }),
  );

  // The second engine: one device, and nothing has ever been checked on it.
  await rpcFor(request, fresh)('devices.create', {
    name: 'core-sw1',
    address: '10.0.0.1',
    site: 'HQ',
    role: 'core',
    profile: 'cisco_ios',
  });
});

test.afterAll(async () => {
  await working?.stop();
  await fresh?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

for (const theme of THEMES) {
  test(`overview — ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    // The working estate, at both sizes, with and without the dock.
    await openApp(page, '/', { backend: working, theme });
    await expect(page.getByRole('list', { name: 'Things that need attention' })).toBeVisible();
    await shoot(page, theme, 'working');

    await openApp(page, '/', { backend: working, theme, dock: true });
    await expect(page.getByRole('list', { name: 'Things that need attention' })).toBeVisible();
    await shoot(page, theme, 'working-dock');

    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/', { backend: working, theme });
    await expect(page.getByRole('list', { name: 'Things that need attention' })).toBeVisible();
    await shoot(page, theme, 'working-1280');

    await openApp(page, '/', { backend: working, theme, dock: true });
    await expect(page.getByRole('list', { name: 'Things that need attention' })).toBeVisible();
    await shoot(page, theme, 'working-1280-dock');
    await page.setViewportSize({ width: 1440, height: 900 });

    // A nearly empty installation: the first hour of a new user's life, and
    // the state in which "quiet" and "absent" are easiest to confuse.
    await openApp(page, '/', { backend: fresh, theme });
    await expect(page.locator('[data-tile="alerts"]')).toContainText('Nothing firing');
    await expect(page.locator('[data-tile="compliance"]')).toContainText('Nothing recorded yet');
    await shoot(page, theme, 'nearly-empty');

    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/', { backend: fresh, theme, dock: true });
    await expect(page.locator('[data-tile="compliance"]')).toContainText('Nothing recorded yet');
    await shoot(page, theme, 'nearly-empty-1280-dock');
    await page.setViewportSize({ width: 1440, height: 900 });
  });
}

/**
 * The first run has to be photographed on an engine with an empty inventory,
 * so it comes before the second engine gets its device — which is why these
 * pictures are taken from a third, short-lived engine of their own.
 */
test('overview — the first run', async ({ page }) => {
  test.setTimeout(300_000);
  const empty = await startBackend({ uiDir: prebuilt ?? uiDir });
  try {
    for (const theme of THEMES) {
      await openApp(page, '/', { backend: empty, theme });
      await expect(
        page.getByRole('heading', { name: 'Nothing is in the inventory yet' }),
      ).toBeVisible();
      await shoot(page, theme, 'first-run');

      await page.setViewportSize({ width: 1280, height: 720 });
      await openApp(page, '/', { backend: empty, theme, dock: true });
      await expect(
        page.getByRole('heading', { name: 'Nothing is in the inventory yet' }),
      ).toBeVisible();
      await shoot(page, theme, 'first-run-1280-dock');
      await page.setViewportSize({ width: 1440, height: 900 });
    }
  } finally {
    await empty.stop();
  }
});

test('overview — work in progress', async ({ page, request }) => {
  test.setTimeout(300_000);
  const rpc = rpcFor(request, working);

  for (const theme of THEMES) {
    // A real sweep of a private range: a real job, with real progress on it.
    const scan = await rpc<{ job_id: string }>('discovery.scan', {
      targets: '10.77.0.0/24',
      probe_ssh: true,
    });
    await openApp(page, '/', { backend: working, theme });
    await expect(page.getByRole('list', { name: 'Jobs running now' })).toBeVisible({
      timeout: 30_000,
    });
    await shoot(page, theme, 'running');
    await rpc('jobs.cancel', { id: scan.job_id }).catch(() => undefined);
  }
});

test('overview — a count the engine capped', async ({ page, request }) => {
  test.setTimeout(600_000);
  const rpc = rpcFor(request, working);

  // Past the engine's own limit, so its count is a floor and the tile says
  // so. Taken last: every other picture would otherwise be of this.
  const numbers = Array.from({ length: 520 }, (_value, index) => index);
  let next = 0;
  await Promise.all(
    Array.from({ length: 16 }, async () => {
      for (let index = next++; index < numbers.length; index = next++) {
        await rpc('alerts.raise', {
          severity: 'medium',
          title: `Power supply ${index} reports a fault`,
          rule: 'environment',
          dedup_key: `bulk:${index}`,
        });
      }
    }),
  );

  for (const theme of THEMES) {
    await openApp(page, '/', { backend: working, theme });
    await expect(page.locator('[data-tile="alerts"]')).toContainText('at least');
    await shoot(page, theme, 'capped');
  }
});
