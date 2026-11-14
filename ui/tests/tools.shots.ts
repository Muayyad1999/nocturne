/**
 * `pnpm shots` — the Tools screen, photographed in both themes.
 *
 * Everything in these pictures came out of a real engine: the subnet is the
 * engine's arithmetic, the type 6 verdict is `tools.secret.identify`'s own
 * answer, the ping is a real ping of the loopback address (or, on a machine
 * that may not send ICMP, the real refusal), the port check is against a
 * listener this file starts on a port the system chose, and the device probe
 * is a simulated switch really failing to reach 10.0.0.1. A screenshot of an
 * invented result would be worth nothing on a screen whose whole claim is
 * that it never invents one.
 *
 * The backend and the lab are set up the same way as `tools.spec.ts`; one
 * Playwright file may not import another's `test(...)` calls, so the harness
 * is repeated rather than shared.
 */

import { execFile } from 'node:child_process';
import { createServer, type Server } from 'node:net';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/tools', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 600_000 });

let backend: Backend;
let uiDir: string;
let listener: Server;
let openPort = 0;

test.beforeAll(async ({ request }) => {
  test.setTimeout(600_000);
  uiDir = process.env.NOCTURNE_UI_DIST ?? (await mkdtemp(join(tmpdir(), 'nocturne-ui-')));
  if (process.env.NOCTURNE_UI_DIST === undefined) {
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 300_000,
    });
  }
  backend = await startBackend({ uiDir });

  const rpc = rpcFor(request, backend);
  const lab = await rpc<{ id: string }>('labs.create', { name: 'Bench' });
  await rpc('labs.node.add', { lab_id: lab.id, model: 'switch', name: 'SW1' });
  await rpc('labs.start', { id: lab.id });

  listener = createServer((socket) => {
    socket.end('SSH-2.0-NocturneTest\r\n');
  });
  await new Promise<void>((done) => listener.listen(0, '127.0.0.1', done));
  const address = listener.address();
  openPort = typeof address === 'object' && address !== null ? address.port : 0;
});

test.afterAll(async () => {
  await new Promise<void>((done) => listener?.close(() => done()));
  await backend?.stop();
  if (uiDir && process.env.NOCTURNE_UI_DIST === undefined) {
    await rm(uiDir, { recursive: true, force: true });
  }
});

const panel = (page: Page): Locator => page.getByRole('article');

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

async function field(page: Page, name: string, value: string): Promise<void> {
  await panel(page).getByLabel(name, { exact: true }).fill(value);
}

for (const theme of THEMES) {
  test(`tools — ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    // The drawer, and a calculator answering as it is typed into.
    await openApp(page, '/tools/tools.subnet', { backend, theme });
    await field(page, 'Address, prefix or mask', '10.1.2.33/26');
    await expect(panel(page).getByText('255.255.255.192')).toBeVisible();
    await shoot(page, theme, 'subnet');

    // A plan with a table in it.
    await openApp(page, '/tools/tools.vlsm', { backend, theme });
    await field(page, 'Base network', '10.20.0.0/22');
    await field(page, 'What has to fit', 'Staff 500\nVoice 120\nCameras 60\nUplink 2');
    await expect(panel(page).getByText('Allocations, largest first')).toBeVisible();
    await shoot(page, theme, 'vlsm');

    // The two questions about a secret, which are not the same question.
    await openApp(page, '/tools/tools.secret.identify', { backend, theme });
    await field(
      page,
      'The line from the configuration',
      'pre-shared-key local 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB',
    );
    await expect(panel(page).getByText('Nocturne cannot recover it')).toBeVisible();
    await shoot(page, theme, 'secret-type6');

    await openApp(page, '/tools/tools.secret.decode', { backend, theme });
    await field(page, 'The value, or the line it sits in', 'username admin password 7 0822455D0A16');
    await expect(panel(page).getByText('cisco', { exact: true })).toBeVisible();
    await shoot(page, theme, 'secret-decoded');

    // A real ping of the loopback address, or the real refusal.
    await openApp(page, '/tools/probe.ping', { backend, theme });
    await field(page, 'Host', '127.0.0.1');
    await field(page, 'Echoes', '12');
    await field(page, 'Every (ms)', '120');
    await panel(page).getByRole('button', { name: 'Ping', exact: true }).click();
    await expect(
      panel(page)
        .getByText('Every echo')
        .or(panel(page).getByText('This machine may not run that probe')),
    ).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'ping');

    // A probe that could not run: a refusal with its code, and no figures.
    await openApp(page, '/tools/probe.ntp', { backend, theme });
    await field(page, 'NTP server', `127.0.0.1:${openPort + 1}`);
    await panel(page).getByRole('button', { name: 'Ask the server', exact: true }).click();
    await expect(panel(page).getByText('The target could not be reached')).toBeVisible({
      timeout: 60_000,
    });
    await shoot(page, theme, 'probe-refused');

    // Every port that was tried, open and closed.
    await openApp(page, '/tools/probe.ports', { backend, theme });
    await field(page, 'Host', '127.0.0.1');
    await field(page, 'Ports', `${openPort},22,80,443,9`);
    await panel(page).getByRole('button', { name: 'Check ports', exact: true }).click();
    await expect(panel(page).getByText('Every port tried')).toBeVisible({ timeout: 60_000 });
    await shoot(page, theme, 'ports');

    // The device's own ping, through the read-only gate.
    await openApp(page, '/tools/probe.device.ping', { backend, theme });
    await panel(page).getByRole('combobox', { name: 'Device' }).click();
    await page.getByRole('option', { name: /Bench\/SW1/ }).first().click();
    await field(page, 'Target', '10.0.0.1');
    await panel(page).getByRole('button', { name: 'Ping from the device', exact: true }).click();
    await expect(panel(page).getByText('ping 10.0.0.1 repeat 5')).toBeVisible({ timeout: 90_000 });
    await shoot(page, theme, 'device-ping');

    // A reference table.
    await openApp(page, '/tools/tools.reference', { backend, theme });
    await expect(panel(page).getByRole('table')).toBeVisible();
    await shoot(page, theme, 'reference');

    // A server that keeps running, and why it cannot be started from here.
    await openApp(page, '/tools/tftp.start', { backend, theme });
    await expect(panel(page).getByText('Not running')).toBeVisible();
    await shoot(page, theme, 'tftp');

    // What was run, and what became of each run.
    await openApp(page, '/tools/probe.history', { backend, theme });
    await expect(panel(page).getByRole('table', { name: 'Probe runs' })).toBeVisible();
    await shoot(page, theme, 'history');

    // The narrow end of the range, with the assistant dock open: 900px of
    // content, which is where a two-column form has to give way.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/tools/tools.subnet', { backend, theme, dock: true });
    await field(page, 'Address, prefix or mask', '2001:db8:1234::1/64');
    await expect(panel(page).getByText('2001:db8:1234::/64')).toBeVisible();
    await shoot(page, theme, 'subnet-1280-dock');

    await openApp(page, '/tools/probe.history', { backend, theme, dock: true });
    await expect(panel(page).getByRole('table', { name: 'Probe runs' })).toBeVisible();
    await shoot(page, theme, 'history-1280-dock');
    await page.setViewportSize({ width: 1440, height: 900 });
  });
}
