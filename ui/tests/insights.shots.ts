/**
 * `pnpm shots` — the Insights screen, photographed in both themes, at the two
 * sizes that matter, with and without the assistant dock, with findings on
 * screen and with nothing examined yet.
 *
 * Taken against the real engine with three simulated switches behind it, so
 * every finding, every piece of evidence and every "not evaluated" reason in
 * these pictures is one the engine actually produced. A screenshot of an
 * invented finding would be worth nothing: this screen exists to be believed.
 *
 * The lab and the route harness are duplicated from `insights.spec.ts` on
 * purpose: one Playwright test file may not import another (its `test(...)`
 * calls would be registered twice, in both projects). See NOTES.md.
 */

import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
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
const SHOTS = resolve(fileURLToPath(new URL('./shots/insights', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 900_000 });

let backend: Backend;
let harness: { root: string; dist: string };

// --------------------------------------------------------------- the harness

const VITE_CONFIG = `
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

export default defineConfig({
  base: './',
  plugins: [react()],
  resolve: { alias: { '@': '__SRC__' } },
  define: { __APP_VERSION__: '"0.0.0-insights"' },
  css: { modules: { localsConvention: 'camelCaseOnly' } },
});
`;

const MAIN = `
import '@fontsource-variable/inter';
import '@fontsource-variable/jetbrains-mono';
import '@/design/tokens.css';
import '@/design/base.css';
import '@/design/control.css';

import { ScanSearch } from 'lucide-react';
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
  id: 'insights',
  path: '/insights',
  label: 'Insights',
  icon: ScanSearch,
  group: 'operate',
  keywords: ['issues', 'findings', 'coverage', 'checks'],
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
      screen('insights', () => import('@/pages/insights')),
      screen('devices/:id?', () => import('@/pages/devices')),
      screen('changes/:id?', () => import('@/pages/changes')),
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
  const root = await mkdtemp(join(tmpdir(), 'nocturne-insights-src-'));
  const dist = await mkdtemp(join(tmpdir(), 'nocturne-insights-dist-'));
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

// ------------------------------------------------------------------- the lab

const DIST1 = [
  'hostname DIST1',
  'vlan 10',
  ' name Staff',
  'vlan 20',
  ' name Guest',
  'ip routing',
  'interface GigabitEthernet1/0/1',
  ' description To ACC1',
  ' switchport mode trunk',
  ' switchport trunk allowed vlan 10,20',
  ' speed 100',
  ' duplex full',
  ' no shutdown',
  'interface GigabitEthernet1/0/2',
  ' description To ACC2',
  ' switchport mode trunk',
  ' switchport trunk allowed vlan 10,20',
  ' no shutdown',
  'interface Vlan10',
  ' ip address 10.10.10.2 255.255.255.0',
  ' no shutdown',
  'snmp-server community public RO',
  'logging host 10.10.99.100',
  'ntp server 10.10.99.100',
  'line vty 0 4',
  ' transport input ssh',
].join('\n');

const ACC1 = [
  'hostname ACC1',
  'vlan 10',
  ' name Staff',
  'vlan 20',
  ' name Guest',
  'interface GigabitEthernet0/1',
  ' description Uplink to DIST1',
  ' switchport mode trunk',
  ' switchport trunk allowed vlan 10,20',
  ' speed 100',
  ' duplex half',
  ' no shutdown',
  'interface FastEthernet0/1',
  ' switchport mode access',
  ' switchport access vlan 10',
  ' no shutdown',
  'logging host 10.10.99.100',
  'ntp server 10.10.99.100',
  'line vty 0 4',
  ' transport input telnet ssh',
].join('\n');

const ACC2 = [
  'hostname ACC2',
  'vlan 10',
  ' name Staff',
  'vlan 20',
  ' name Guest',
  'interface GigabitEthernet0/1',
  ' description Uplink to DIST1',
  ' switchport mode trunk',
  ' switchport trunk allowed vlan 10,20',
  ' switchport trunk native vlan 10',
  ' no shutdown',
  'interface FastEthernet0/1',
  ' switchport mode access',
  ' switchport access vlan 20',
  ' no shutdown',
  'logging host 10.10.99.100',
  'ntp server 10.10.99.100',
  'line vty 0 4',
  ' transport input ssh',
].join('\n');

const LAB_SPEC = {
  name: 'Campus',
  description: 'Three switches with two planted faults',
  nodes: [
    { name: 'DIST1', model: 'switch-l3', x: 0, y: 0, startup_config: DIST1 },
    { name: 'ACC1', model: 'switch', x: -180, y: 160, startup_config: ACC1 },
    { name: 'ACC2', model: 'switch', x: 180, y: 160, startup_config: ACC2 },
  ],
  links: [
    {
      a: { node: 'DIST1', interface: 'GigabitEthernet1/0/1' },
      b: { node: 'ACC1', interface: 'GigabitEthernet0/1' },
    },
    {
      a: { node: 'DIST1', interface: 'GigabitEthernet1/0/2' },
      b: { node: 'ACC2', interface: 'GigabitEthernet0/1' },
    },
  ],
};

const DEVICES = ['Campus/DIST1', 'Campus/ACC1', 'Campus/ACC2'];
const DUPLEX_TITLE = 'Campus/ACC1 GigabitEthernet0/1 is half duplex';

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
  harness = await buildHarness();
  backend = await startBackend({ uiDir: harness.dist });
  const rpc = rpcFor(request, backend);

  // No automatic scan, so the "nothing has been examined yet" pictures are
  // taken before the first scan rather than in a race with it.
  await rpc('settings.set', { section: 'insights', value: { auto_scan: false } });
  const lab = await rpc<{ id: string }>('labs.import', { spec: LAB_SPEC });
  await rpc('labs.start', { id: lab.id });
  await finished(rpc, await rpc<{ job_id: string }>('collect.run', { device_ids: DEVICES }));
  await finished(
    rpc,
    // Running only, deliberately. Reading a *startup* configuration from a
    // simulated node never returns (`show startup-config` through
    // `connect::Cli` hangs on `transport: sim`, reproduced on a bare lab with
    // `devices.exec`), which would hold the device's session for the rest of
    // the suite. It also gives the coverage band an honest "not evaluated":
    // `unsaved_config` cannot compare what it has not got.
    await rpc<{ job_id: string }>('config.backup', {
      device_ids: DEVICES,
      kinds: ['running'],
    }),
  );
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

function card(page: Page, titlePart: string) {
  return page.locator('article', { hasText: titlePart }).first();
}

// Nothing examined yet: both themes, both sizes, before anything is scanned.
test('insights — nothing examined yet', async ({ page }) => {
  for (const theme of THEMES) {
    await openApp(page, '/insights', { backend, theme });
    await expect(page.getByRole('heading', { name: 'No scan has run yet' })).toBeVisible();
    await shoot(page, theme, 'nothing-examined');

    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/insights', { backend, theme, dock: true });
    await expect(page.getByRole('heading', { name: 'No scan has run yet' })).toBeVisible();
    await shoot(page, theme, 'nothing-examined-1280-dock');
    await page.setViewportSize({ width: 1440, height: 900 });
  }
});

test('insights — a real scan, so the rest of the pictures are true', async ({ request }) => {
  const rpc = rpcFor(request, backend);
  await finished(rpc, await rpc<{ job_id: string }>('insights.scan', {}));
  const list = await rpc<{ insights: unknown[] }>('insights.list', { limit: 100 });
  expect(list.insights.length).toBeGreaterThan(5);
});

for (const theme of THEMES) {
  test(`insights — ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    await openApp(page, '/insights', { backend, theme });
    await expect(card(page, DUPLEX_TITLE)).toBeVisible();
    await shoot(page, theme, 'findings');

    // One finding in full, with the check's own blind spots underneath it.
    await card(page, DUPLEX_TITLE).getByRole('button', { name: 'Details' }).click();
    await expect(page.getByRole('dialog', { name: /half duplex/ })).toBeVisible();
    await shoot(page, theme, 'finding');
    await page.keyboard.press('Escape');

    // Dismissing: the reason and the end date.
    await card(page, DUPLEX_TITLE).getByRole('button', { name: 'Dismiss…' }).click();
    const dialog = page.getByRole('dialog', { name: 'Dismiss this finding' });
    await dialog
      .getByRole('textbox', { name: 'Reason' })
      .fill('Known: the patch panel is being replaced on Tuesday');
    await shoot(page, theme, 'dismiss');
    await page.keyboard.press('Escape');

    // Every check accounted for.
    await page.getByRole('tab', { name: 'Coverage' }).click();
    await expect(page.getByRole('table', { name: 'Coverage of every check' })).toBeVisible();
    await shoot(page, theme, 'coverage');

    // The catalogue and the thresholds.
    await page.getByRole('tab', { name: 'Checks' }).click();
    await expect(page.getByRole('searchbox', { name: 'Search checks' })).toBeVisible();
    await shoot(page, theme, 'checks');

    // The narrow end of the range, and the same screens with the dock open.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/insights', { backend, theme });
    await expect(card(page, DUPLEX_TITLE)).toBeVisible();
    await shoot(page, theme, 'findings-1280');

    await openApp(page, '/insights?view=coverage', { backend, theme, dock: true });
    await expect(page.getByRole('table', { name: 'Coverage of every check' })).toBeVisible();
    await shoot(page, theme, 'coverage-1280-dock');

    await page.setViewportSize({ width: 1440, height: 900 });
    await openApp(page, '/insights', { backend, theme, dock: true });
    await expect(card(page, DUPLEX_TITLE)).toBeVisible();
    await shoot(page, theme, 'findings-dock');
  });
}
