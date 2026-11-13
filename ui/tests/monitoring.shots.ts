/**
 * `pnpm shots` — PNGs of the Monitoring screen, in both themes, at 1440×900
 * and 1280×720, with the assistant dock open and closed.
 *
 * Driven by a real engine: the demo campus lab is created and really polled,
 * one node is powered off so a device is really down, and one device's address
 * is changed to one that cannot be resolved so a chart really has a hole in
 * it. Nothing in these pictures is made up. They are written to
 * `tests/shots/<night|day>/` and compared against nothing: they exist to be
 * looked at before a change ships.
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

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots', import.meta.url)));

const THEMES: readonly Theme[] = ['dark', 'light'];
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };

test.describe.configure({ mode: 'serial', timeout: 240_000 });

let backend: Backend;
let uiDir: string;
let devices: Record<string, string> = {};

async function rpc<T = unknown>(method: string, params?: unknown): Promise<T> {
  const response = await fetch(`${backend.url}/api/rpc`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${backend.token}` },
    body: JSON.stringify({ method, params: params ?? null }),
  });
  const body = (await response.json()) as
    | { ok: true; result: T }
    | { ok: false; error: { code: string; message: string } };
  if (!body.ok) throw new Error(`${method} failed: ${body.error.code} — ${body.error.message}`);
  return body.result;
}

const wait = (ms: number) => new Promise<void>((done) => setTimeout(done, ms));

/** `monitor.poll_now` is a job: wait for the answer, not for the clock. */
async function until(what: string, ready: () => Promise<boolean>, timeoutMs = 60_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await ready()) return;
    if (Date.now() > deadline) throw new Error(`the engine never reached: ${what}`);
    await wait(1_000);
  }
}

test.beforeAll(async () => {
  test.setTimeout(600_000);
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 240_000,
  });
  backend = await startBackend({ uiDir });

  await rpc('settings.set', { section: 'monitoring', value: { ping_timeout_ms: 250 } });
  const lab = await rpc<{ id: string }>('labs.create', { example: 'demo' });
  await rpc('labs.start', { id: lab.id });
  const rows = await rpc<{ id: string; name: string }[]>('devices.list');
  devices = Object.fromEntries(rows.map((row) => [row.name, row.id]));

  // Built-in rules leave lab devices alone unless a rule opts in.
  await rpc('alerts.rules.save', {
    id: 'builtin.device_down',
    name: 'Device down',
    scope: { include_lab: true },
  });

  const network = ['Campus/AS1', 'Campus/AS2', 'Campus/AS3', 'Campus/DS1', 'Campus/DS2', 'Campus/EDGE'].map(
    (name) => devices[name] as string,
  );
  // The simulator reports its uptime in whole minutes, and a rate is a counter
  // delta over an uptime delta: two reads inside the same simulated minute
  // measure nothing. Waiting for the minute to turn is what fills the table.
  await rpc('monitor.poll_now', { device_ids: network });
  await wait(65_000);
  await rpc('monitor.poll_now', { device_ids: network });

  // A device that is really not answering, and the one alert it earns.
  await rpc('labs.node.update', { lab_id: lab.id, node: 'AS1', powered: false });
  await until('Campus/AS1 is down', async () => {
    await rpc('monitor.poll_now', { device_id: devices['Campus/AS1'] });
    await wait(1_500);
    const view = await rpc<{ down: boolean }>('monitor.device', { device_id: devices['Campus/AS1'] });
    return view.down;
  });

  // Alerts other features raise, so the list shows more than one severity.
  await rpc('alerts.raise', {
    rule: 'Compliance',
    device_id: devices['Campus/DS1'],
    severity: 'medium',
    title: 'Configuration drifts from intended — Campus/DS1',
    detail: 'Two lines differ from the intended configuration: the NTP server and one ACL remark.',
    dedup_key: 'shots:drift:ds1',
  });
  await rpc('alerts.raise', {
    rule: 'Insights',
    device_id: devices['Campus/EDGE'],
    severity: 'high',
    instance: 'GigabitEthernet0/1',
    title: 'Duplex mismatch — Campus/EDGE Gi0/1',
    detail: 'The neighbour reports full duplex while this end is half: expect late collisions under load.',
    dedup_key: 'shots:duplex:edge',
  });
  await rpc('alerts.raise', {
    rule: 'Backups',
    device_id: devices['Campus/AS2'],
    severity: 'info',
    title: 'Configuration changed — Campus/AS2',
    detail: 'The running configuration changed since the last backup.',
    dedup_key: 'shots:changed:as2',
  });

  // Something to show in the Rules tab beyond the built-ins.
  await rpc('notify.channels.save', {
    name: 'NOC Slack',
    kind: 'slack',
    url: 'https://hooks.invalid/T000/B000/not-a-real-webhook',
    min_severity: 'medium',
  });
  const soon = Date.now() + 36 * 3_600_000;
  await rpc('maintenance.save', {
    name: 'Saturday patching',
    starts_at: soon,
    ends_at: soon + 4 * 3_600_000,
    recurring: 'weekly',
    note: 'Access-switch firmware, one stack at a time.',
    scope: { sites: ['Lab: Campus'], include_lab: true },
  });

  // A series with a real hole in it: polled, then pointed at an address that
  // cannot be resolved, then back. The failed polls record that the device is
  // down and no round-trip time at all.
  const probe = await rpc<{ id: string }>('devices.create', {
    name: 'echo-probe',
    address: '127.0.0.1',
    transport: 'none',
    monitor: { enabled: true, icmp: true, snmp: false, interval_s: 86_400 },
  });
  devices['echo-probe'] = probe.id;
  const pollProbe = async () => {
    await rpc('monitor.poll_now', { device_id: probe.id });
    await wait(3_000);
  };
  for (let index = 0; index < 3; index += 1) await pollProbe();
  await rpc('devices.update', { id: probe.id, address: 'nothing-answers-here.invalid' });
  await pollProbe();
  await pollProbe();
  await rpc('devices.update', { id: probe.id, address: '127.0.0.1' });
  await pollProbe();
  await pollProbe();
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
    test('the alert list', async ({ page }) => {
      await openApp(page, '/monitoring', { backend, theme });
      await page.getByRole('table', { name: 'Alerts' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'monitoring-alerts');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'monitoring-alerts-1280');
      await page.setViewportSize({ width: 1440, height: 900 });

      // One alert in full, with the series it was measured on.
      await page.getByRole('cell', { name: /Duplex mismatch/ }).dblclick();
      await page.getByRole('dialog').waitFor();
      await shoot(page, theme, 'monitoring-alert');
      await page.keyboard.press('Escape');
    });

    test('the alert list with the assistant dock open', async ({ page }) => {
      await openApp(page, '/monitoring', { backend, theme, dock: true });
      await page.getByRole('table', { name: 'Alerts' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'monitoring-alerts-dock');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'monitoring-alerts-dock-1280');
    });

    test('health, and one device', async ({ page }) => {
      await openApp(page, '/monitoring/health', { backend, theme });
      await page.getByRole('table', { name: 'Device health' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'monitoring-health');

      await page.getByRole('link', { name: 'Campus/AS1' }).click();
      await pageLoaded(page);
      await page.getByRole('heading', { level: 2, name: 'Campus/AS1' }).waitFor();
      await shoot(page, theme, 'monitoring-device-down');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'monitoring-device-down-1280');
      await page.setViewportSize({ width: 1440, height: 900 });

      // A device that is answering, with its charts and its ports.
      await openApp(page, `/monitoring/health?device=${devices['Campus/DS1']}`, { backend, theme });
      await page.getByRole('heading', { level: 2, name: 'Campus/DS1' }).waitFor();
      await shoot(page, theme, 'monitoring-device');
    });

    test('the interface table', async ({ page }) => {
      await openApp(page, '/monitoring/interfaces', { backend, theme });
      await page.getByRole('table', { name: 'Interfaces' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'monitoring-interfaces');

      await page.getByRole('cell', { name: 'GigabitEthernet1/0/11', exact: true }).first().dblclick();
      await page.getByRole('dialog').waitFor();
      await shoot(page, theme, 'monitoring-interface');
      await page.keyboard.press('Escape');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'monitoring-interfaces-1280');
    });

    test('a chart with a gap in it', async ({ page }) => {
      await openApp(page, '/monitoring/charts', { backend, theme });
      await page.getByRole('combobox', { name: 'Device' }).fill('echo-probe');
      await page.getByRole('option', { name: /echo-probe/ }).click();
      await page.getByRole('combobox', { name: 'Time range' }).selectOption('5m');
      await page.getByRole('checkbox', { name: 'Round-trip time' }).check();
      await page.getByTestId('chart-gaps').waitFor();
      await shoot(page, theme, 'monitoring-charts-gap');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'monitoring-charts-gap-1280');
    });

    test('rules, windows and channels', async ({ page }) => {
      await openApp(page, '/monitoring/rules', { backend, theme });
      await page.getByRole('table', { name: 'Alert rules' }).locator('tbody tr').first().waitFor();
      await shoot(page, theme, 'monitoring-rules');

      await page.setViewportSize({ width: 1280, height: 720 });
      await shoot(page, theme, 'monitoring-rules-1280');
      await page.setViewportSize({ width: 1440, height: 900 });

      // A rule being edited, tried against what is stored.
      // The row's switch is named after the rule too, so take the name cell.
      await page.getByRole('row', { name: /CPU above 90/ }).locator('td').nth(1).dblclick();
      const rule = page.getByRole('dialog');
      await rule.waitFor();
      await rule.getByText('Include simulated lab devices').click();
      await rule.getByRole('button', { name: 'Test against what is stored' }).click();
      await rule.getByRole('status').waitFor();
      // The answer is at the foot of a long form: show it, not the top of it.
      await rule.getByRole('status').scrollIntoViewIfNeeded();
      await shoot(page, theme, 'monitoring-rule');
      await page.keyboard.press('Escape');

      // A channel, whose secret can only be set or replaced.
      await page.getByRole('cell', { name: /NOC Slack/ }).dblclick();
      await page.getByRole('dialog').waitFor();
      await shoot(page, theme, 'monitoring-channel');
      await page.keyboard.press('Escape');

      // A maintenance window.
      await page.getByRole('cell', { name: /Saturday patching/ }).dblclick();
      await page.getByRole('dialog').waitFor();
      await shoot(page, theme, 'monitoring-maintenance');
      await page.keyboard.press('Escape');
    });
  });
}
