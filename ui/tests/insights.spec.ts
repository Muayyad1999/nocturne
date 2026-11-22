/**
 * The Insights screen, against a real engine and real simulated devices.
 *
 * A `nocturne-server` is started on its own empty data directory and a lab is
 * imported into it: three IOS switches cabled together, with faults planted in
 * the configurations they boot from — one end of a trunk forced to half duplex
 * against a full-duplex neighbour, and one end of another trunk given a native
 * VLAN the other end does not have. Nothing here is stubbed: the simulator
 * negotiates the duplex, `collect.run` reads `show interfaces` and
 * `show cdp neighbors` through the same path a real switch is read through,
 * `config.backup` stores the running and startup configurations, and
 * `insights.scan` reasons over exactly that. The duplex mismatch on screen is
 * one two simulated switches really disagreed about.
 *
 * The screen is reached at `#/insights`. The route table (`src/app/routes.tsx`)
 * does not list it yet — that file belongs to the shell, not to this slice —
 * so these tests build the real application with one route added, from the
 * real `src/`. See NOTES.md: when the line lands in `routes.tsx`, delete
 * `buildHarness` and use `startBackend({ uiDir: 'dist' })` like the other
 * screen specs.
 */

import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { goTo, openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let harness: { root: string; dist: string };

// --------------------------------------------------------------- the harness

/** `vite.config.ts` for the harness: the real `src/`, under a different root. */
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

/**
 * The real `src/main.tsx`, with two lines added: the `insights` route and the
 * nav destination that goes with it. Everything else — the shell, the theme,
 * the token capture, the query client, the other screens — is the application
 * as it ships.
 */
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
  // Planted: 100 Mb/s full duplex against ACC1's forced half duplex.
  ' speed 100',
  ' duplex full',
  ' no shutdown',
  'interface GigabitEthernet1/0/2',
  ' description To ACC2',
  ' switchport mode trunk',
  ' switchport trunk allowed vlan 10,20',
  // Planted: no native VLAN here, VLAN 10 at the other end.
  ' no shutdown',
  'interface Vlan10',
  ' ip address 10.10.10.2 255.255.255.0',
  ' no shutdown',
  // Planted: the default community, which is a real finding on a real estate.
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
  // Planted: telnet for management.
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

interface Interface {
  name: string;
  oper_up: boolean;
  duplex: string;
  vlan?: number | null;
  mode?: string;
}

async function finished(rpc: Rpc, started: { job_id: string }): Promise<void> {
  try {
    await expect
      .poll(async () => (await rpc<{ state: string }>('jobs.get', { id: started.job_id })).state, {
        timeout: 120_000,
        intervals: [500, 1_000, 2_000],
      })
      .toBe('succeeded');
  } catch (error) {
    // A job that never finishes is the one failure that says nothing about
    // itself, so its own record and the engine's log go into the report.
    const job = await rpc('jobs.get', { id: started.job_id });
    throw new Error(
      `${String(error)}\njob: ${JSON.stringify(job, null, 1)}\nengine log:\n${backend.log().slice(-4000)}`,
    );
  }
}

async function interfaces(rpc: Rpc, device: string): Promise<Interface[]> {
  const row = await rpc<{ rows: Interface[] }>('collect.get', { id: device, kind: 'interfaces' });
  return row.rows;
}

/**
 * Imports the lab, boots it, and collects from it — then checks that the
 * planted faults really are in the collected data before any screen is
 * opened. If the simulator ever stops negotiating half duplex here, this
 * fails with the reason instead of the screen failing with an empty list.
 */
async function seed(rpc: Rpc): Promise<void> {
  // The automatic scan is debounced on collection events; switching it off
  // keeps "nothing has been scanned yet" true until the screen asks for one.
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

  const acc1 = (await interfaces(rpc, 'Campus/ACC1')).find(
    (entry) => entry.name === 'GigabitEthernet0/1',
  );
  expect(acc1, 'ACC1 Gi0/1 was collected').toBeDefined();
  expect(acc1?.oper_up, 'ACC1 Gi0/1 is up').toBe(true);
  expect(acc1?.duplex, 'ACC1 Gi0/1 negotiated half duplex').toBe('half');
  const acc2 = (await interfaces(rpc, 'Campus/ACC2')).find(
    (entry) => entry.name === 'GigabitEthernet0/1',
  );
  expect(acc2?.vlan, 'ACC2 Gi0/1 carries native VLAN 10').toBe(10);
}

// ---------------------------------------------------------------- the screen

const DUPLEX_TITLE = 'Campus/ACC1 GigabitEthernet0/1 is half duplex';

function card(page: Page, titlePart: string) {
  return page.locator('article', { hasText: titlePart }).first();
}

function band(page: Page) {
  return page.getByRole('region', { name: 'Coverage of the last scan' });
}

/**
 * Presses Scan now and waits for the job to have been and gone. What says it
 * has finished is its own toast, which names the coverage rather than claiming
 * success.
 */
async function scan(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Scan now', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Notifications' })).toContainText(
    /checks over 3 devices/,
    { timeout: 60_000 },
  );
}

test.beforeAll(async () => {
  test.setTimeout(300_000);
  harness = await buildHarness();
  backend = await startBackend({ uiDir: harness.dist });
});

test.afterAll(async () => {
  await backend?.stop();
  if (harness) {
    await rm(harness.root, { recursive: true, force: true });
    await rm(harness.dist, { recursive: true, force: true });
  }
});

test('says nothing has been examined yet, rather than that the network is clean', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  await seed(rpc);

  await openApp(page, '/insights', { backend });

  await expect(band(page)).toContainText('Nothing has been examined yet');
  await expect(page.getByRole('heading', { name: 'No scan has run yet' })).toBeVisible();
  // The one place the age of a scan is always on screen.
  await expect(page.getByRole('contentinfo')).toContainText('Never scanned');
  // Nothing claims health: there is no "no findings" or "all clear" here.
  await expect(page.getByText('No open findings')).toHaveCount(0);

  expect(console.messages).toEqual([]);
});

test('a scan finds the planted duplex mismatch and shows what it was decided from', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/insights', { backend });

  await scan(page);

  const found = card(page, DUPLEX_TITLE);
  await expect(found).toBeVisible();
  await expect(found).toContainText('High');
  await expect(found).toContainText('Campus/ACC1');
  await expect(found).toContainText('GigabitEthernet0/1');
  await expect(found).toContainText('duplex_mismatch');
  // The evidence, in the engine's words, on the card itself.
  await expect(found).toContainText('half duplex, 100 Mb/s');
  await expect(found).toContainText('full duplex, 100 Mb/s');
  await expect(found).toContainText('both ends report it');
  // A simulated device can never pass for production.
  await expect(found.getByText('LAB', { exact: true })).toBeVisible();

  // And the scan's age is now on screen in two places.
  await expect(page.getByRole('contentinfo')).toContainText('Scanned');
  await expect(band(page)).toContainText('37 checks over 3 devices');

  expect(console.messages).toEqual([]);
});

test('the other end of a cross-device problem is named, on the end that owns the fix', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/insights', { backend });

  await page.getByRole('searchbox', { name: 'Search findings' }).fill('native');
  const found = card(page, 'Native VLAN 1 on Campus/DIST1');
  await expect(found).toBeVisible();
  // DIST1's end is the one nobody configured, so the finding lives there…
  await expect(found).toContainText('Campus/DIST1');
  // …and ACC2's end is named as the other half of the same problem.
  await expect(found).toContainText('Also at:');
  await expect(found).toContainText('Campus/ACC2');

  expect(console.messages).toEqual([]);
});

test('coverage accounts for every check, and never shows "not evaluated" as a pass', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/insights', { backend });

  // The band is above the findings, with the engine's own reasons.
  await expect(band(page)).toContainText('not evaluated');
  await expect(band(page)).toContainText('this metric is not polled for the device');

  await page.getByRole('tab', { name: 'Coverage' }).click();
  await pageLoaded(page);

  const table = page.getByRole('table', { name: 'Coverage of every check' });
  await expect(table).toBeVisible();

  // Three different answers, three different words.
  const notEvaluated = table.getByRole('row', { name: /high_memory/ });
  await expect(notEvaluated).toContainText('Not evaluated');
  const found = table.getByRole('row', { name: /duplex_mismatch/ });
  await expect(found).toContainText('1 finding');
  const clear = table.getByRole('row', { name: /speed_mismatch/ });
  await expect(clear).toContainText('Clear');

  // Every device is accounted for by every check: evaluated + not evaluated.
  await expect(page.getByText('answers missing')).toBeVisible();
  // The same sentence is in the band, in the reason list and in the table:
  // that is the point of it, so one of them is enough here.
  await expect(page.getByText('fewer than two stored configurations').first()).toBeVisible();
  // And a check with nothing to compare says so rather than passing.
  const unsaved = table.getByRole('row', { name: /unsaved_config/ });
  await expect(unsaved).toContainText('Not evaluated');
  await expect(unsaved).toContainText('no startup configuration stored');

  expect(console.messages).toEqual([]);
});

test('a check that is switched off reads as off, not as clear', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/insights', { backend });

  await page.getByRole('tab', { name: 'Checks' }).click();
  await pageLoaded(page);

  await page.getByRole('searchbox', { name: 'Search checks' }).fill('unused_vlan');
  const entry = page.locator('article', { hasText: 'VLAN defined but unused' }).first();
  await expect(entry).toBeVisible();
  // The catalogue says what the check cannot see, in the backend's words.
  await expect(entry).toContainText('Not evaluated when:');
  await expect(entry).toContainText('unused_vlan');

  // The switch's drawn track swallows a click on the input, so what is
  // clicked is its label — which is what a person clicks too. (The word "Off"
  // appears twice once it is off: on the switch and in its verdict.)
  await entry.locator('label').first().click();
  await expect(entry.getByRole('switch')).not.toBeChecked();

  await scan(page);
  await page.getByRole('tab', { name: 'Coverage' }).click();
  await pageLoaded(page);
  const row = page
    .getByRole('table', { name: 'Coverage of every check' })
    .getByRole('row', { name: /unused_vlan/ });
  await expect(row).toContainText('Off');
  await expect(row).not.toContainText('Clear');

  // Put it back, so the rest of the suite sees the estate as it was.
  await page.getByRole('tab', { name: 'Checks' }).click();
  await pageLoaded(page);
  await page.getByRole('searchbox', { name: 'Search checks' }).fill('unused_vlan');
  const again = page.locator('article', { hasText: 'VLAN defined but unused' }).first();
  await again.locator('label').first().click();
  await expect(again.getByRole('switch')).toBeChecked();

  expect(console.messages).toEqual([]);
});

test('a fix drafts a change and does not touch the device', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const labs = await rpc<{ id: string; name: string }[]>('labs.list');
  const lab = labs.find((entry) => entry.name === 'Campus');
  expect(lab, 'the lab is in the inventory').toBeDefined();
  const before = await rpc<{ text: string }>('labs.node.config', {
    lab_id: lab?.id,
    node: 'ACC1',
  });
  expect(before.text).toContain('duplex half');

  await openApp(page, '/insights', { backend });
  const found = card(page, DUPLEX_TITLE);

  // The screen says what this does before it is pressed.
  await expect(found.getByRole('button', { name: 'Draft a change' })).toHaveAttribute(
    'title',
    /Nothing is sent to the device/,
  );
  await found.getByRole('button', { name: 'Draft a change' }).click();

  const link = found.getByRole('link', { name: 'Draft change created — review it' });
  await expect(link).toBeVisible();

  // The device is untouched: the pipeline has the lines, the switch does not.
  const after = await rpc<{ text: string }>('labs.node.config', {
    lab_id: lab?.id,
    node: 'ACC1',
  });
  expect(after.text).toBe(before.text);

  await link.click();
  await pageLoaded(page);
  await expect(page.getByRole('heading', { level: 1 })).toContainText('half duplex');
  const changes = await rpc<{ state: string; origin: string }[]>('changes.list', { limit: 10 });
  expect(changes[0]?.state === 'draft' || changes[0]?.state === 'planned').toBe(true);

  expect(console.messages).toEqual([]);
});

test('dismissing asks for a reason and an end date, and reopening undoes it', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/insights', { backend });

  await card(page, DUPLEX_TITLE).getByRole('button', { name: 'Dismiss…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Dismiss this finding' });
  await expect(dialog).toContainText('Campus/ACC1 GigabitEthernet0/1');
  await dialog.getByRole('combobox', { name: 'For how long' }).selectOption('7');
  await dialog
    .getByRole('textbox', { name: 'Reason' })
    .fill('Known: the patch panel is being replaced on Tuesday');
  await expect(dialog).toContainText('not as a new discovery');
  await dialog.getByRole('button', { name: 'Dismiss', exact: true }).click();

  await expect(card(page, DUPLEX_TITLE)).toHaveCount(0);

  // It is not gone, it is dismissed — with the reason that was given.
  await page.getByRole('combobox', { name: 'State' }).selectOption('dismissed');
  const dismissed = card(page, DUPLEX_TITLE);
  await expect(dismissed).toContainText('Dismissed');
  await dismissed.getByRole('button', { name: 'Details' }).click();
  const drawer = page.getByRole('dialog', { name: /half duplex/ });
  await expect(drawer).toContainText('the patch panel is being replaced on Tuesday');
  // Drawer has two "Close" controls, so Escape is the reliable way out.
  await page.keyboard.press('Escape');

  await dismissed.getByRole('button', { name: 'Reopen' }).click();
  await expect(card(page, DUPLEX_TITLE)).toHaveCount(0);
  await page.getByRole('combobox', { name: 'State' }).selectOption('open');
  await expect(card(page, DUPLEX_TITLE)).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('a dismissal that has run out comes back as a repeat, not as a discovery', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  const list = await rpc<{ insights: { id: string }[] }>('insights.list', {
    check: 'duplex_mismatch',
    include_lab: true,
  });
  const id = list.insights[0]?.id;
  expect(id, 'the duplex mismatch is in the ledger').toBeDefined();
  // A dismissal that is over as soon as it is made: the next scan finds the
  // problem again, which is exactly the case the "Repeat" badge is for.
  await rpc('insights.dismiss', { id, days: 0, note: 'expired on purpose' });

  await openApp(page, '/insights', { backend });
  await scan(page);

  const found = card(page, DUPLEX_TITLE);
  await expect(found).toBeVisible();
  await expect(found).toContainText('Repeat · 2');
  await found.getByRole('button', { name: 'Details' }).click();
  const drawer = page.getByRole('dialog', { name: /half duplex/ });
  await expect(drawer).toContainText('2 separate times');
  await page.keyboard.press('Escape');

  expect(console.messages).toEqual([]);
});

test('the findings and the coverage survive a reload, and the command palette can scan', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/insights', { backend });
  await expect(card(page, DUPLEX_TITLE)).toBeVisible();

  await page.reload();
  await pageLoaded(page);
  await goTo(page, '/insights');
  await expect(card(page, DUPLEX_TITLE)).toBeVisible();

  await page.keyboard.press('ControlOrMeta+k');
  await page.getByRole('combobox', { name: /command/i }).fill('scan the network');
  await page.getByRole('option', { name: 'Scan the network for issues' }).click();
  // The scan itself takes milliseconds, so what is waited for is its outcome:
  // the toast, which names the coverage rather than claiming success.
  await expect(page.getByRole('region', { name: 'Notifications' })).toContainText(
    '37 checks over 3 devices',
    { timeout: 30_000 },
  );

  expect(console.messages).toEqual([]);
});
