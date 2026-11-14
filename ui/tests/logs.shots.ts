/**
 * `pnpm shots` — the Logs screen, photographed in both themes, at the two
 * sizes that matter, with and without the assistant dock.
 *
 * Every line in these pictures is a line a real receiver really took off a
 * socket, and every bucket, count and gap was computed from them by the
 * engine. The lines are sent in several bursts a few seconds apart so that the
 * histogram shows what a histogram is for — busy minutes, quiet minutes, and
 * the stretch before anything was stored, which is a gap and not a zero.
 *
 * The lab and the route harness are duplicated from `logs.spec.ts` on purpose:
 * one Playwright test file may not import another (its `test(...)` calls would
 * be registered twice, in both projects). See NOTES.md.
 */

import { createSocket } from 'node:dgram';
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/logs', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

/** The port the receiver is told to open. High, so it needs no privileges. */
const PORT = 22_000 + Math.floor(Math.random() * 900);

test.describe.configure({ mode: 'serial', timeout: 900_000 });

let backend: Backend;
let harness: { root: string; dist: string };
let pulledAt = 0;

// --------------------------------------------------------------- the harness

const VITE_CONFIG = `
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig({
  base: './',
  plugins: [react()],
  resolve: { alias: { '@': '__SRC__' } },
  define: { __APP_VERSION__: '"0.0.0-logs"' },
  css: { modules: { localsConvention: 'camelCaseOnly' } },
});
`;

const MAIN = `
import '@fontsource-variable/inter';
import '@fontsource-variable/jetbrains-mono';
import '@/design/tokens.css';
import '@/design/base.css';
import '@/design/control.css';

import { ScrollText } from 'lucide-react';
import { lazy, StrictMode, Suspense } from 'react';
import { createRoot } from 'react-dom/client';
import { createHashRouter } from 'react-router';
import { connect } from '@/api/client';
import { bindQueryClient, createQueryClient } from '@/api/queryClient';
import { captureToken } from '@/api/token';
import { App } from '@/app/App';
import { AppError } from '@/app/AppError';
import { AppShell } from '@/app/AppShell';
import { DESTINATIONS } from '@/app/destinations';
import { PageFallback } from '@/app/PageFallback';
import { RouteError } from '@/app/RouteError';
import { bindTheme } from '@/design/theme';

DESTINATIONS.push({
  id: 'logs',
  path: '/logs',
  label: 'Logs',
  icon: ScrollText,
  group: 'operate',
  keywords: ['syslog', 'tail', 'messages'],
});

const element = (load) => {
  const Screen = lazy(load);
  return (
    <Suspense fallback={<PageFallback />}>
      <Screen />
    </Suspense>
  );
};
const screen = (path, load) => ({ path, element: element(load), ErrorBoundary: RouteError });

bindTheme();
captureToken();
const queryClient = createQueryClient();
bindQueryClient(queryClient);
const router = createHashRouter([
  {
    path: '/',
    Component: AppShell,
    ErrorBoundary: AppError,
    children: [
      { index: true, element: element(() => import('@/pages/overview')), ErrorBoundary: RouteError },
      screen('logs', () => import('@/pages/logs')),
      screen('devices/:id?', () => import('@/pages/devices')),
      screen('monitoring/:tab?', () => import('@/pages/monitoring')),
      screen('settings/:section?', () => import('@/pages/settings')),
      screen('*', () => import('@/pages/not-found')),
    ],
  },
]);
connect().catch(() => undefined);
const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root element');
createRoot(root).render(
  <StrictMode>
    <App router={router} queryClient={queryClient} />
  </StrictMode>,
);
`;

async function buildHarness(): Promise<{ root: string; dist: string }> {
  const root = await mkdtemp(join(tmpdir(), 'nocturne-logs-src-'));
  const dist = await mkdtemp(join(tmpdir(), 'nocturne-logs-dist-'));
  await symlink(join(UI_DIR, 'node_modules'), join(root, 'node_modules'), 'dir');
  const html = await readFile(join(UI_DIR, 'index.html'), 'utf8');
  await writeFile(join(root, 'index.html'), html.replace('/src/main.tsx', '/main.tsx'));
  await writeFile(join(root, 'vite.config.ts'), VITE_CONFIG.replace('__SRC__', join(UI_DIR, 'src')));
  await writeFile(join(root, 'main.tsx'), MAIN);
  await run('pnpm', ['exec', 'vite', 'build', root, '--outDir', dist, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 240_000,
  });
  return { root, dist };
}

// ------------------------------------------------------------- the log lines

/** The shapes a Cisco switch really puts on the wire, with real severities. */
const SHAPES: readonly ((index: number) => string)[] = [
  (index) =>
    `<187>${index}: *Nov 14 00:31:0${index % 10}.123: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/${(index % 8) + 1}, changed state to down`,
  (index) =>
    `<190>${index}: *Nov 14 00:31:0${index % 10}.456: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/${(index % 8) + 1}, changed state to up`,
  (index) =>
    `<189>${index}: *Nov 14 00:31:0${index % 10}.789: %SYS-5-CONFIG_I: Configured from console by netops on vty0 (10.1.1.9)`,
  (index) =>
    `<188>${index}: *Nov 14 00:31:0${index % 10}.001: %SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: 10.1.1.4${index % 10}] [localport: 22] [Reason: Login Authentication Failed]`,
  (index) =>
    `<186>${index}: *Nov 14 00:31:0${index % 10}.002: %SPANTREE-2-BLOCK_BPDUGUARD: Received BPDU on port GigabitEthernet1/0/${(index % 8) + 1} with BPDU Guard enabled, disabling port`,
  (index) =>
    `<191>${index}: *Nov 14 00:31:0${index % 10}.003: %CDP-4-DUPLEX_MISMATCH: duplex mismatch discovered on GigabitEthernet1/0/${(index % 8) + 1} (not half duplex), with SW9 FastEthernet0/2 (half duplex)`,
];

async function send(count: number, offset: number, from?: string): Promise<void> {
  const socket = createSocket('udp4');
  if (from) await new Promise<void>((done) => socket.bind({ address: from }, done));
  for (let index = 0; index < count; index += 1) {
    const shape = SHAPES[(index + offset) % SHAPES.length];
    socket.send(Buffer.from(shape?.(index + offset) ?? ''), PORT, '127.0.0.1');
    if (index % 50 === 49) await new Promise((done) => setTimeout(done, 20));
  }
  await new Promise((done) => setTimeout(done, 150));
  socket.close();
}

// ------------------------------------------------------------------ the seed

test.beforeAll(async ({ request }) => {
  test.setTimeout(900_000);
  harness = await buildHarness();
  backend = await startBackend({ uiDir: harness.dist });
  const rpc = rpcFor(request, backend);

  for (const device of [
    { name: 'CORE1', address: '127.0.0.1', profile: 'cisco_ios', role: 'core' },
    { name: 'DIST2', address: '10.10.0.2', profile: 'cisco_ios', role: 'distribution' },
    { name: 'EDGE-FW', address: '10.10.0.9', profile: 'mikrotik_routeros', role: 'firewall' },
  ]) {
    await rpc('devices.create', { ...device, site: 'HQ' });
  }
  await rpc('settings.set', {
    section: 'logs',
    value: { syslog: { enabled: false, udp_port: PORT, tcp_port: 0 } },
  });
});

test.afterAll(async () => {
  await backend?.stop();
  if (harness) {
    await rm(harness.root, { recursive: true, force: true });
    await rm(harness.dist, { recursive: true, force: true });
  }
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

// Nothing listening yet: the state most installations start in, with the
// commands for a real device in it.
test('logs — nothing is listening yet', async ({ page }) => {
  for (const theme of THEMES) {
    await openApp(page, '/logs', { backend, theme });
    await expect(page.getByRole('region', { name: 'Log reception' })).toContainText(
      'switched off',
    );
    await shoot(page, theme, 'not-configured');

    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/logs', { backend, theme, dock: true });
    await expect(page.getByRole('region', { name: 'Log reception' })).toBeVisible();
    await shoot(page, theme, 'not-configured-1280-dock');
    await page.setViewportSize({ width: 1440, height: 900 });
  }
});

/**
 * Real lines, in bursts a few seconds apart, so the histogram has a shape:
 * busy twenty seconds, quiet ones, and the stretch before any of it, which is
 * a gap rather than a zero.
 */
test('logs — the lines these pictures are of', async ({ request }) => {
  const rpc = rpcFor(request, backend);
  await rpc('settings.set', { section: 'logs', value: { syslog: { enabled: true } } });
  await expect
    .poll(async () => (await rpc<{ syslog: { listening: string[] } }>('logs.status')).syslog.listening.length, {
      timeout: 30_000,
    })
    .toBeGreaterThan(0);

  for (const [index, count] of [40, 12, 90, 5, 30].entries()) {
    await send(count, index * 7);
    // Two buckets of a fifteen-minute range, so the bars are not one block.
    if (index < 4) await new Promise((done) => setTimeout(done, 22_000));
  }
  // One address nobody claims, which is the other half of attribution.
  await send(3, 61, '127.0.0.2');
});

for (const theme of THEMES) {
  test(`logs — ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    // The screen as a NOC leaves it open: the last quarter of an hour.
    await openApp(page, '/logs?span=15m', { backend, theme });
    await expect(page.getByTestId('log-counts')).toContainText('lines in this range');
    await shoot(page, theme, 'lines');

    // What a severity filter hides, said out loud.
    await page.getByRole('combobox', { name: 'Severity' }).selectOption('4');
    await expect(page.getByTestId('log-counts')).toContainText('hidden by the severity filter');
    await shoot(page, theme, 'severity');
    await page.getByRole('combobox', { name: 'Severity' }).selectOption('');

    // One line in full, with both clocks. A login failure, because that is a
    // line whose drawer has something in every field.
    const list = page.getByRole('list', { name: 'Log lines' });
    const interesting = list.getByRole('button').filter({ hasText: 'Login failed' }).first();
    await (await interesting.count() > 0 ? interesting : list.getByRole('button').first()).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await shoot(page, theme, 'entry');
    await page.keyboard.press('Escape');

    // The live tail, following.
    await page.locator('label', { hasText: 'Live tail' }).click();
    await expect(page.getByText('Following the newest lines')).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'tail');
    await page.locator('label', { hasText: 'Live tail' }).click();

    // A week, where most of the picture is honestly unknown.
    await openApp(page, '/logs?span=7d', { backend, theme });
    await expect(page.getByTestId('log-gaps')).toBeVisible();
    await shoot(page, theme, 'gaps');

    // The addresses no device claims.
    await openApp(page, '/logs?span=15m', { backend, theme });
    await page.getByRole('button', { name: /address no device claims/ }).click();
    await expect(page.getByRole('table', { name: 'Addresses no device claims' })).toBeVisible();
    await shoot(page, theme, 'sources');

    // The narrow end of the range, with the dock open: the filter bar wraps
    // rather than scrolling sideways, and the columns still line up.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/logs?span=15m', { backend, theme, dock: true });
    await expect(page.getByTestId('log-counts')).toBeVisible();
    await shoot(page, theme, 'lines-1280-dock');
    await page.setViewportSize({ width: 1440, height: 900 });
  });
}

/**
 * A device's own log buffer, read over the read-only CLI path. It comes last
 * because it changes what the whole store covers: these lines are dated by
 * the switch's clock, which in a freshly booted simulation is months behind
 * this machine's, and the oldest line in the store is what decides which
 * empty buckets can be called zeros.
 */
test('logs — a device’s own buffer', async ({ request }) => {
  const rpc = rpcFor(request, backend);
  const lab = await rpc<{ id: string }>('labs.import', {
    spec: {
      name: 'LogLab',
      description: 'one switch, for its own log buffer',
      nodes: [
        {
          name: 'SW1',
          model: 'switch',
          x: 0,
          y: 0,
          startup_config: ['hostname SW1', 'service timestamps log datetime msec'].join('\n'),
        },
      ],
      links: [],
    },
  });
  await rpc('labs.start', { id: lab.id });
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  const sw1 = devices.find((device) => device.name === 'LogLab/SW1')?.id;
  const pulled = await rpc<{ stored: number; newest_at: number }>('logs.pull', { device_id: sw1 });
  expect(pulled.stored).toBeGreaterThan(0);
  pulledAt = pulled.newest_at;
});

for (const theme of THEMES) {
  test(`logs — the device's clock, ${theme}`, async ({ page }) => {
    // The hour on either side of the line the switch wrote: before it, a gap,
    // because nothing is stored from then; after it, real zeros.
    await openApp(page, `/logs?from=${pulledAt - 1_800_000}&to=${pulledAt + 1_800_000}`, {
      backend,
      theme,
    });
    await expect(page.getByRole('list', { name: 'Log lines' })).toBeVisible();
    await shoot(page, theme, 'device-clock');

    await page.getByRole('list', { name: 'Log lines' }).getByRole('button').first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await shoot(page, theme, 'device-clock-entry');
    await page.keyboard.press('Escape');
  });
}
