/**
 * The Monitoring screen, against a real engine.
 *
 * A `nocturne-server` is started on its own empty data directory and set up
 * through the same `/api/rpc` endpoint the app uses: the demo campus lab (real
 * simulated devices, really polled), one node powered off so a device is
 * really down, alerts raised through `alerts.raise`, and — for the gap test —
 * a device whose address is changed to one that cannot be resolved, so that
 * monitoring really records nothing for a while. Nothing on screen is faked.
 */

import { createServer, type Server } from 'node:http';
import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { goTo, openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

/** Polls far enough apart that a missing one is unmistakable in a 1-second grid. */
const POLL_GAP_MS = 3_000;

// Serial: one engine, set up once. The long first-run build lives in
// `beforeAll`, which raises its own timeout.
test.describe.configure({ mode: 'serial', timeout: 150_000 });

let backend: Backend;
let uiDir: string;
/** The demo lab's id, and the device ids this file needs by name. */
let labId: string;
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

/**
 * Waits for the backend to agree. `monitor.poll_now` starts a job and returns
 * at once, so a fixed sleep is a guess — on a loaded machine the poll lands
 * after it. This asks until the answer is the one the setup is waiting for.
 */
async function until(what: string, ready: () => Promise<boolean>, timeoutMs = 60_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (await ready()) return;
    if (Date.now() > deadline) throw new Error(`the engine never reached: ${what}`);
    await wait(1_000);
  }
}

/** Names the devices of the lab, so a test can say "AS1" and mean the device. */
async function readInventory(): Promise<void> {
  const rows = await rpc<{ id: string; name: string }[]>('devices.list');
  devices = Object.fromEntries(rows.map((row) => [row.name, row.id]));
}

test.beforeAll(async () => {
  test.setTimeout(420_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 240_000,
  });
  backend = await startBackend({ uiDir });

  // A quarter of a second is long enough for a loopback echo and short enough
  // that a device that is not there fails the poll quickly.
  await rpc('settings.set', { section: 'monitoring', value: { ping_timeout_ms: 250 } });

  const lab = await rpc<{ id: string }>('labs.create', { example: 'demo' });
  labId = lab.id;
  await rpc('labs.start', { id: labId });
  await readInventory();

  // Built-in rules leave lab devices alone unless a rule opts in, which is
  // exactly what the scope switch in the rule editor does.
  await rpc('alerts.rules.save', { id: 'builtin.device_down', name: 'Device down', scope: { include_lab: true } });

  const network = ['Campus/AS1', 'Campus/AS2', 'Campus/AS3', 'Campus/DS1', 'Campus/DS2', 'Campus/EDGE'].map(
    (name) => devices[name] as string,
  );
  // Two reads of the counters far enough apart to produce rates. The
  // simulator reports its uptime in whole minutes, and a rate is a counter
  // delta over an *uptime* delta, so two reads inside the same simulated
  // minute measure no interval at all and are recorded as a gap. Waiting for
  // the minute to turn over is what makes the interface table show rates.
  await rpc('monitor.poll_now', { device_ids: network });
  await wait(65_000);
  await rpc('monitor.poll_now', { device_ids: network });

  // One node really off: two failed polls is what `down_after` asks for.
  await rpc('labs.node.update', { lab_id: labId, node: 'AS1', powered: false });
  await until('Campus/AS1 is down', async () => {
    await rpc('monitor.poll_now', { device_id: devices['Campus/AS1'] });
    await wait(1_500);
    const view = await rpc<{ down: boolean }>('monitor.device', { device_id: devices['Campus/AS1'] });
    return view.down;
  });
  // …and the rule that says so has fired.
  await until('the device-down alert is open', async () => {
    const alerts = await rpc<{ title: string }[]>('alerts.list', { state: 'open', limit: 100 });
    return alerts.some((alert) => alert.title.includes('Device down'));
  });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

function alertRows(page: Page): Locator {
  return page.getByRole('table', { name: 'Alerts' }).locator('tbody tr');
}

test.describe('what is up and what is not', () => {
  test('the strip counts what the backend counts, and new alerts arrive on their own', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/monitoring', { backend });

    await expect(page.getByRole('heading', { level: 1, name: 'Monitoring' })).toBeVisible();

    // Six monitored lab devices, one of them really down.
    const tiles = page.getByText('Devices answering');
    await expect(tiles).toBeVisible();
    await expect(page.getByText('5 / 6')).toBeVisible();
    await expect(page.getByText('1 down')).toBeVisible();

    // The device-down rule fired by itself while the test was setting up.
    await expect(alertRows(page)).toHaveCount(1);
    await expect(page.getByRole('cell', { name: /Device down — Campus\/AS1/ })).toBeVisible();
    await expect(page.getByRole('table', { name: 'Alerts' }).getByText('Critical')).toBeVisible();

    // Something else raises an alert: the table follows the event stream.
    await rpc('alerts.raise', {
      rule: 'Compliance',
      device_id: devices['Campus/DS1'],
      severity: 'medium',
      title: 'Configuration drifts from intended — Campus/DS1',
      detail: 'Two lines differ from the intended configuration.',
      dedup_key: 'test:drift:ds1',
    });
    await expect(alertRows(page)).toHaveCount(2);
    await expect(page.getByRole('cell', { name: /drifts from intended/ })).toBeVisible();

    // The rail carries the same count the summary gave.
    await expect(
      page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: /2 open alerts/ }),
    ).toBeVisible();

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('an alert can be acknowledged and resolved from the list', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/monitoring', { backend });

    const row = alertRows(page).filter({ hasText: 'drifts from intended' });
    await expect(row).toHaveCount(1);
    await row.getByRole('button', { name: 'Row actions' }).click();
    await page.getByRole('menuitem', { name: 'Acknowledge' }).click();

    // Acknowledged is still open: the alert stays on the list, differently.
    await expect(row.getByText('Acknowledged')).toBeVisible();
    await expect(alertRows(page)).toHaveCount(2);

    await row.getByRole('button', { name: 'Row actions' }).click();
    await page.getByRole('menuitem', { name: 'Resolve…' }).click();
    // A resolve says what it will do before it does it.
    const question = page.getByRole('dialog');
    await expect(question).toContainText('notification channel');
    await question.getByRole('button', { name: 'Resolve' }).click();

    await expect(alertRows(page)).toHaveCount(1);
    await page.getByRole('button', { name: 'Resolved' }).click();
    await expect(alertRows(page).filter({ hasText: 'drifts from intended' })).toHaveCount(1);

    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('one device', () => {
  test('a device that is down reads as one cause, not thirty', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/monitoring/health', { backend });

    const table = page.getByRole('table', { name: 'Device health' });
    await expect(table.locator('tbody tr')).toHaveCount(6);
    // Problems first, so the device that is down is the first row.
    await expect(table.locator('tbody tr').first()).toContainText('Campus/AS1');

    await table.getByRole('link', { name: 'Campus/AS1' }).click();
    await pageLoaded(page);

    await expect(page.getByRole('heading', { level: 2, name: 'Campus/AS1' })).toBeVisible();
    await expect(page.getByText('Device down — Campus/AS1').first()).toBeVisible();
    // The sentence that keeps the page honest about the backend's suppression.
    await expect(page.getByText(/interface and threshold alerts are held back/)).toBeVisible();

    // Exactly one open alert for it, the one cause.
    const alerts = page.getByRole('region', { name: 'Open alerts' });
    await expect(alerts).toContainText('Device down');

    // Its ports are still listed, as last read.
    await expect(page.getByRole('table', { name: 'Interfaces' }).locator('tbody tr').first()).toBeVisible();
    // A lab device is never pinged, and the page says so rather than showing 0 ms.
    await expect(page.getByText('Where the numbers come from')).toBeVisible();

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('an interface with no measured rate says so instead of showing zero', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/monitoring/interfaces', { backend });

    const table = page.getByRole('table', { name: 'Interfaces' });
    await expect(table.locator('tbody tr').first()).toBeVisible();

    // A switched virtual interface has no speed, so it can have no utilisation:
    // that cell reads "Not measured", never 0 %.
    await page.getByRole('searchbox', { name: 'Search interfaces' }).fill('Vlan');
    await expect(table.locator('tbody tr').first()).toBeVisible();
    await expect(table.getByText('Not measured').first()).toBeVisible();

    // "Needs a look" asks the backend for the problem rows only.
    await page.getByRole('searchbox', { name: 'Search interfaces' }).fill('');
    await page.getByText('Needs a look').click();
    await expect(table.locator('tbody tr').first()).toBeVisible();
    await expect(table.getByText('Down', { exact: true }).first()).toBeVisible();

    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('charts', () => {
  test('a stretch with nothing recorded is drawn as a gap, not a line across it', async ({ page }) => {
    const console = watchConsole(page);

    // A device of its own, polled only when this test says so: its interval is
    // a day, so the scheduler cannot add a sample in the middle of the hole.
    const probe = await rpc<{ id: string; name: string }>('devices.create', {
      name: 'echo-probe',
      address: '127.0.0.1',
      transport: 'none',
      monitor: { enabled: true, icmp: true, snmp: false, interval_s: 86_400 },
    });
    const pollOnce = async () => {
      await rpc('monitor.poll_now', { device_id: probe.id });
      await wait(POLL_GAP_MS);
    };
    const address = (value: string) => rpc('devices.update', { id: probe.id, address: value });

    for (let index = 0; index < 3; index += 1) await pollOnce();
    // `.invalid` can never resolve (RFC 2606), so the echo cannot even be sent:
    // monitoring records that the device is down and no round-trip time at all.
    await address('nothing-answers-here.invalid');
    await pollOnce();
    await pollOnce();
    await address('127.0.0.1');
    await pollOnce();
    await pollOnce();

    const series = await rpc<{ points: [number, number][] }>('metrics.query', {
      device_id: probe.id,
      metric: 'icmp.rtt_ms',
      since: Date.now() - 300_000,
      step_s: 1,
    });
    // The backend really did leave the failed polls out.
    expect(series.points.length).toBeGreaterThanOrEqual(5);

    await openApp(page, '/monitoring/charts', { backend });
    await page.getByRole('combobox', { name: 'Device' }).fill('echo-probe');
    await page.getByRole('option', { name: /echo-probe/ }).click();
    await page.getByRole('combobox', { name: 'Time range' }).selectOption('5m');
    await page.getByRole('checkbox', { name: 'Round-trip time' }).check();

    const chart = page.getByRole('figure').filter({ hasText: 'Round-trip time' });
    await expect(chart).toBeVisible();

    // Said in words…
    await expect(page.getByTestId('chart-gaps')).toContainText('nothing was recorded then');

    // …and present in the numbers: the same chart as a table has a row with no
    // value in it, where a line across the hole would have invented one.
    await chart.getByRole('button', { name: 'Show as table' }).click();
    const values = chart.getByRole('table').locator('tbody tr td:nth-child(2)');
    await expect(values.filter({ hasText: '—' }).first()).toBeVisible();

    expect(console.messages, backend.log()).toEqual([]);
  });
});

test.describe('rules, windows and channels', () => {
  test('a rule can be tried against what is stored without raising anything', async ({ page }) => {
    const console = watchConsole(page);
    const before = await rpc<unknown[]>('alerts.list', { limit: 2000 });

    await openApp(page, '/monitoring/rules', { backend });
    await expect(page.getByRole('table', { name: 'Alert rules' }).locator('tbody tr')).toHaveCount(10);

    await page.getByRole('button', { name: 'New rule' }).click();
    const drawer = page.getByRole('dialog', { name: 'New alert rule' });
    await drawer.getByRole('textbox', { name: 'Name', exact: true }).fill('Anything moving at all');
    // An <input list> is a combobox, not a textbox.
    await drawer.getByRole('combobox', { name: 'Metric' }).fill('if.in_bps');
    await drawer.getByRole('spinbutton', { name: 'Value' }).fill('1000');
    // The built-in scope leaves the lab out; this rule opts in, as the backend wants.
    await drawer.getByText('Include simulated lab devices').click();
    await drawer.getByRole('button', { name: 'Test against what is stored' }).click();

    const result = drawer.getByRole('status');
    await expect(result).toContainText(/Would fire on \d+ of \d+ devices in scope/);
    await expect(result).toContainText('GigabitEthernet');

    // Trying a rule changes nothing: the alert list is exactly as it was.
    const after = await rpc<unknown[]>('alerts.list', { limit: 2000 });
    expect(after.length).toBe(before.length);

    await drawer.getByRole('button', { name: 'Save' }).click();
    await expect(page.getByRole('table', { name: 'Alert rules' }).locator('tbody tr')).toHaveCount(11);
    await expect(page.getByRole('cell', { name: 'Anything moving at all', exact: true })).toBeVisible();

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('a built-in rule can be switched off but not deleted', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/monitoring/rules', { backend });

    const row = page
      .getByRole('table', { name: 'Alert rules' })
      .locator('tbody tr')
      .filter({ hasText: 'Temperature above 70' });
    await row.getByRole('button', { name: 'Row actions' }).click();
    await expect(page.getByRole('menuitem', { name: 'Built-in rules cannot be deleted' })).toBeDisabled();
    await page.keyboard.press('Escape');

    // `design/Switch` draws a track over its real <input>, so a click never
    // lands when the switch has no label text next to it (as in a table cell).
    // The keyboard is the honest way in, and the way a NOC engineer works.
    await row.getByRole('switch').focus();
    await page.keyboard.press(' ');
    await expect
      .poll(async () => {
        const rules = await rpc<{ id: string; enabled: boolean }[]>('alerts.rules.list');
        return rules.find((rule) => rule.id === 'builtin.temperature_high')?.enabled;
      })
      .toBe(false);

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('a maintenance window is recorded and shows as open', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/monitoring/rules', { backend });

    await page.getByRole('button', { name: 'New window' }).click();
    const drawer = page.getByRole('dialog', { name: 'New maintenance window' });
    await drawer.getByRole('textbox', { name: 'Name', exact: true }).fill('Tonight’s patching');
    const now = new Date(Date.now() - 60_000);
    const local = (at: Date) =>
      `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, '0')}-${String(at.getDate()).padStart(2, '0')}T${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`;
    await drawer.getByLabel('Starts').fill(local(now));
    await drawer.getByLabel('Ends').fill(local(new Date(Date.now() + 3_600_000)));
    await drawer.getByRole('button', { name: 'Save' }).click();

    const row = page
      .getByRole('table', { name: 'Maintenance windows' })
      .locator('tbody tr')
      .filter({ hasText: 'patching' });
    await expect(row).toContainText('Open now');

    // The strip picks it up from `monitor.status`, which the save invalidated.
    await expect(page.getByText(/maintenance window is open/)).toBeVisible();

    expect(console.messages, backend.log()).toEqual([]);
  });

  test('a channel keeps its secret and still proves it works', async ({ page }) => {
    const console = watchConsole(page);
    const posts: string[] = [];
    const hook: Server = createServer((request, response) => {
      let body = '';
      request.on('data', (chunk: Buffer) => {
        body += chunk.toString();
      });
      request.on('end', () => {
        posts.push(body);
        response.writeHead(200, { 'Content-Type': 'application/json' });
        response.end('{}');
      });
    });
    await new Promise<void>((ready) => hook.listen(0, '127.0.0.1', ready));
    const port = (hook.address() as { port: number }).port;

    try {
      await openApp(page, '/monitoring/rules', { backend });
      await page.getByRole('button', { name: 'New channel' }).click();
      const drawer = page.getByRole('dialog', { name: 'New notification channel' });
      await drawer.getByRole('textbox', { name: 'Name', exact: true }).fill('Test hook');
      await drawer.getByRole('combobox', { name: 'Kind' }).selectOption('webhook');

      // Write-only: the field knows only whether a secret is set.
      await expect(drawer.getByText('Not set')).toBeVisible();
      await drawer.getByRole('button', { name: 'Set…' }).click();
      await drawer.getByLabel('Webhook URL').fill(`http://127.0.0.1:${port}/alerts`);
      await drawer.getByRole('button', { name: 'Save' }).click();

      const row = page
        .getByRole('table', { name: 'Notification channels' })
        .locator('tbody tr')
        .filter({ hasText: 'Test hook' });
      await expect(row).toContainText('Set');

      // Reopening it shows "Set", never the URL — the backend does not have it
      // to give, and the page does not pretend otherwise.
      await row.getByRole('cell', { name: /Test hook/ }).dblclick();
      const again = page.getByRole('dialog', { name: 'Edit Test hook' });
      await expect(again.getByText('Set', { exact: true })).toBeVisible();
      await expect(again.getByText(`127.0.0.1:${port}`)).toHaveCount(0);

      await again.getByRole('button', { name: 'Send a test message' }).click();
      await expect(again.getByText(/Test message delivered/)).toBeVisible();
      expect(posts.length).toBeGreaterThan(0);
      expect(posts.join('')).toContain('test');
      // The URL is a secret, so it is nowhere on the page either.
      expect(await page.content()).not.toContain(`http://127.0.0.1:${port}`);

      await page.keyboard.press('Escape');
      expect(console.messages, backend.log()).toEqual([]);
    } finally {
      await new Promise<void>((done) => hook.close(() => done()));
    }
  });
});

test.describe('when monitoring is switched off', () => {
  test('the screen says so instead of showing stale numbers', async ({ page }) => {
    const console = watchConsole(page);
    await rpc('settings.set', { section: 'monitoring', value: { enabled: false } });
    try {
      await openApp(page, '/monitoring', { backend });
      await expect(page.getByText('Monitoring is switched off.')).toBeVisible();
      await expect(page.getByText(/no alert can fire/)).toBeVisible();
    } finally {
      await rpc('settings.set', { section: 'monitoring', value: { enabled: true } });
    }
    await goTo(page, '/monitoring/health');
    expect(console.messages, backend.log()).toEqual([]);
  });
});
