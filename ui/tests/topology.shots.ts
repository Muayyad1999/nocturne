/**
 * `pnpm shots` — the Topology screen, photographed in both themes, at both
 * sizes, with the assistant dock open and closed.
 *
 * Taken against a real engine with a real network behind it: the demo campus
 * lab, collected from, so every line in the picture is a line some device
 * really reported. The seed is the same one `topology.spec.ts` uses, and it is
 * built to contain all three kinds of link — both ends reported it, one end
 * did, it was worked out — plus a link a person drew and a node that is not in
 * the inventory, because those are exactly the cases a screenshot has to be
 * checked for.
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

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/topology', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 600_000 });

let backend: Backend;
let uiDir: string;
/** The address of the one host the MAC tables know about. */
let hostAddress = '';

async function rpc<T>(method: string, params?: unknown): Promise<T> {
  const response = await fetch(`${backend.url}/api/rpc`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${backend.token}` },
    body: JSON.stringify({ method, params: params ?? null }),
  });
  const envelope = (await response.json()) as
    | { ok: true; result: T }
    | { ok: false; error: { code: string; message: string } };
  if (!envelope.ok) throw new Error(`${method}: ${envelope.error.code} — ${envelope.error.message}`);
  return envelope.result;
}

async function waitForJob(id: string): Promise<void> {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const job = await rpc<{ state: string }>('jobs.get', { id });
    if (job.state !== 'running') return;
    await new Promise((done) => setTimeout(done, 1_000));
  }
  throw new Error(`job ${id} did not finish`);
}

async function collectFrom(ids: string[]): Promise<void> {
  const started = await rpc<{ job_id: string }>('collect.run', { device_ids: ids });
  await waitForJob(started.job_id);
}

test.beforeAll(async () => {
  test.setTimeout(900_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 300_000,
  });
  backend = await startBackend({ uiDir });

  const lab = await rpc<{ id: string }>('labs.create', { name: 'Campus', example: 'demo' });
  await rpc('labs.start', { id: lab.id });
  // The drawing is to be built from what the devices reported, as a real
  // network's would be, rather than from the lab's own wiring.
  await rpc('settings.set', {
    section: 'topology',
    value: {
      stale_after_hours: 24,
      endpoint_max_macs: 3,
      fold_port_channels: true,
      max_snapshots: 200,
      default_algorithm: 'layered',
      use_lab_links: false,
      use_monitor_utilisation: true,
    },
  });

  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  await collectFrom(devices.map((device) => device.id));

  // A cable only one end has been collected from since it was plugged in.
  await rpc('labs.link.add', {
    lab_id: lab.id,
    a: { node: 'AS1', interface: 'FastEthernet0/3' },
    b: { node: 'AS2', interface: 'FastEthernet0/3' },
  });
  const as1 = devices.find((device) => device.name.endsWith('/AS1'));
  if (as1) await collectFrom([as1.id]);

  await rpc('topology.manual.save', {
    a: { device: 'Campus/EDGE', interface: 'GigabitEthernet0/0' },
    b: { name: 'ISP uplink', kind: 'cloud' },
    label: '100 Mb/s fibre',
  });

  const graph = await rpc<{ nodes: { id: string; address?: string }[] }>('topology.get', {
    include_endpoints: true,
  });
  hostAddress = graph.nodes.find((entry) => entry.id.startsWith('h:'))?.address ?? '';
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

function node(page: Page, name: string) {
  return page.getByRole('button', { name: new RegExp(`^${name},`) });
}

async function openTopology(page: Page, theme: Theme, dock = false): Promise<void> {
  await openApp(page, '/topology', { theme, backend, dock });
  await expect(node(page, 'Campus/DS1')).toBeVisible({ timeout: 60_000 });
}

for (const theme of THEMES) {
  test(`topology screens — ${theme}`, async ({ page }) => {
    test.setTimeout(420_000);

    // ------------------------------------------------------------- the drawing
    await openTopology(page, theme);
    await shoot(page, theme, 'canvas');

    // A node: what it is, and what it says it is connected to.
    await node(page, 'Campus/DS1').click();
    await expect(page.getByRole('heading', { name: 'Campus/DS1' })).toBeVisible();
    await shoot(page, theme, 'node');

    // The link only one end knows about — the claim that matters most.
    await page.locator('g[data-confidence="one_sided"] path').first().click({ force: true });
    // Scoped: the link's own `<title>` carries the same sentence.
    const selection = page.getByRole('complementary', { name: 'Selection' });
    await expect(selection.getByText(/Only one end reported this link/)).toBeVisible();
    await shoot(page, theme, 'link-one-sided');

    // The link a person drew.
    await page.locator('g[data-source="drawn"] path').first().click({ force: true });
    await expect(
      selection.getByText('A person drew this link. No device reported it.'),
    ).toBeVisible();
    await shoot(page, theme, 'link-drawn');

    // The caveats, open.
    await page
      .getByRole('region', { name: 'What this drawing cannot tell you' })
      .getByRole('button', { name: /cannot tell you/ })
      .click();
    await shoot(page, theme, 'warnings');
    await page
      .getByRole('region', { name: 'What this drawing cannot tell you' })
      .getByRole('button', { name: /cannot tell you/ })
      .click();

    // ------------------------------------------------------------ 1280 x 720
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.getByRole('button', { name: 'Fit to view' }).click();
    await shoot(page, theme, 'canvas-1280');
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.getByRole('button', { name: 'Fit to view' }).click();

    // ------------------------------------------------------------- the layers
    await page.getByRole('combobox', { name: 'Layer' }).selectOption('l3');
    await expect(page.locator('g[data-confidence="inferred"]').first()).toBeVisible({
      timeout: 30_000,
    });
    await shoot(page, theme, 'layer3');
    await page.getByRole('combobox', { name: 'Layer' }).selectOption('physical');

    await page.getByRole('button', { name: 'Endpoints' }).click();
    await expect(page.locator('g[data-kind="host"][data-unmanaged]').first()).toBeVisible({
      timeout: 30_000,
    });
    await shoot(page, theme, 'endpoints');
    await page.getByRole('button', { name: 'Endpoints' }).click();

    // --------------------------------------------------------------- the path
    await page.getByRole('tab', { name: 'Path' }).click();
    await shoot(page, theme, 'path-empty');
    await page.getByLabel('From', { exact: true }).fill('Campus/DS1');
    await page.getByLabel('To', { exact: true }).fill('8.8.8.8');
    await page.getByRole('button', { name: 'Trace', exact: true }).click();
    await expect(page.getByText('Why it stops here')).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'path');

    // A walk with two equal-cost choices, which the backend notes.
    await page.getByLabel('From', { exact: true }).fill('Campus/EDGE');
    await page.getByLabel('To', { exact: true }).fill('10.10.20.10');
    await page.getByRole('button', { name: 'Trace', exact: true }).click();
    await expect(page.getByText(/equal-cost paths/)).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'path-ecmp');

    // ---------------------------------------------------------- finding a host
    if (hostAddress !== '') {
      await page
        .getByRole('searchbox', { name: 'Search for a device, an IP or a MAC address' })
        .fill(hostAddress);
      await page.keyboard.press('Enter');
      await expect(page.getByText('The port it decided on')).toBeVisible({ timeout: 30_000 });
      await shoot(page, theme, 'host');
    }

    // ------------------------------------------------------------ the exports
    await page.getByRole('button', { name: 'Export this drawing' }).click();
    const dialog = page.getByRole('dialog', { name: 'Export this drawing' });
    await expect(dialog.getByText(/<svg xmlns=/).first()).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'export');
    await dialog.getByRole('combobox', { name: 'Format' }).selectOption('mermaid');
    await expect(dialog.getByText(/graph TD/).first()).toBeVisible();
    await shoot(page, theme, 'export-mermaid');
    await page.keyboard.press('Escape');

    // ------------------------------------------------------- drawn by a person
    await page.getByRole('button', { name: 'Links drawn by hand' }).click();
    await expect(page.getByRole('dialog', { name: 'Links drawn by hand' })).toBeVisible();
    await shoot(page, theme, 'manual');
    await page.keyboard.press('Escape');

    // -------------------------------------------------- with the assistant dock
    await openTopology(page, theme, true);
    await node(page, 'Campus/AS1').click();
    await shoot(page, theme, 'canvas-dock');

    await page.setViewportSize({ width: 1280, height: 720 });
    await page.getByRole('button', { name: 'Fit to view' }).click();
    await shoot(page, theme, 'canvas-dock-1280');
    await page.setViewportSize({ width: 1440, height: 900 });
  });
}
