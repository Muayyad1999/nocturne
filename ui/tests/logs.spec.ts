/**
 * The Logs screen, against a real engine and real log lines.
 *
 * Nothing here is stubbed and no row is written behind the feature's back. A
 * `nocturne-server` is started on its own empty data directory; the screen's
 * own button opens the receiver's socket; the lines are **sent over UDP to
 * that socket** from this test process, in the shapes a Cisco switch really
 * puts on the wire, and the engine parses, attributes, redacts and stores them
 * on the path a real device's lines take. The device-clock test goes the other
 * way: a lab switch is booted and `logs.pull` reads the buffer it keeps for
 * itself, which is dated by its own clock — a clock that, in a fresh
 * simulation, is months away from this machine's. That is the case the screen
 * exists to make visible.
 *
 * The screen is reached at `#/logs`. The route table (`src/app/routes.tsx`)
 * does not list it — Logs is a tab of Monitoring, and that file belongs to the
 * shell, not to this slice — so these tests build the real application with
 * one route added, from the real `src/`. See NOTES.md: when the tab is mounted
 * in `pages/monitoring/index.tsx`, this harness can be replaced with
 * `startBackend({ uiDir: 'dist' })` and `/monitoring/logs`.
 */

import { createSocket } from 'node:dgram';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let harness: { root: string; dist: string };

/** The port the receiver is told to open. High, so it needs no privileges. */
const PORT = 21_000 + Math.floor(Math.random() * 900);

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

/**
 * The real `src/main.tsx`, with two lines added: the `logs` route and the nav
 * destination that goes with it. Everything else — the shell, the theme, the
 * query client, the other screens — is the application as it ships.
 */
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

/**
 * Lines in the shapes a Cisco switch puts on the wire: a priority, the
 * device's own message counter, its own timestamp, then
 * `%FACILITY-SEVERITY-MNEMONIC:`. The severities are the ones the priority
 * encodes — 187 is local7.error, 189 local7.notice, 190 local7.informational.
 */
function iosLine(index: number): string {
  const shapes = [
    `<187>${index}: *Nov 14 00:31:0${index % 10}.123: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down`,
    `<190>${index}: *Nov 14 00:31:0${index % 10}.456: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to up`,
    `<189>${index}: *Nov 14 00:31:0${index % 10}.789: %SYS-5-CONFIG_I: Configured from console by netops on vty0 (10.1.1.9)`,
  ];
  return shapes[index % shapes.length] ?? '';
}

/** Sends datagrams to the receiver the application opened, and waits for them. */
async function send(lines: readonly string[], options: { from?: string } = {}): Promise<void> {
  const socket = createSocket('udp4');
  if (options.from) await new Promise<void>((done) => socket.bind({ address: options.from }, done));
  for (const [index, line] of lines.entries()) {
    socket.send(Buffer.from(line), PORT, '127.0.0.1');
    // Loopback has a finite socket buffer and UDP is allowed to lose a
    // datagram the kernel has no room for — before the application ever sees
    // it, which is why `logs.status` does not pretend to count those. A pause
    // every few dozen lines keeps the loss out of what this test asserts on.
    if (index % 50 === 49) await new Promise((done) => setTimeout(done, 20));
  }
  await new Promise((done) => setTimeout(done, 150));
  socket.close();
}

/** Waits until the engine says it has stored at least `rows` lines. */
async function stored(rpc: Rpc, rows: number): Promise<void> {
  await expect
    .poll(
      async () => (await rpc<{ store: { rows: number } }>('logs.status')).store.rows,
      { timeout: 30_000, intervals: [100, 250, 500] },
    )
    .toBeGreaterThanOrEqual(rows);
}

// ------------------------------------------------------------------ the seed

async function seed(rpc: Rpc): Promise<void> {
  await rpc('devices.create', {
    name: 'CORE1',
    // Every datagram this test sends comes from the loopback address, so this
    // is the device the engine attributes them to — by address, as it only
    // ever does.
    address: '127.0.0.1',
    profile: 'cisco_ios',
    site: 'HQ',
    role: 'core',
  });
  await rpc('devices.create', {
    name: 'ACC9',
    address: '10.9.9.9',
    profile: 'mikrotik_routeros',
    site: 'HQ',
    role: 'access',
  });
  // The port is set but the receiver is left off: the screen's own button
  // turns it on, which is what the first test is about.
  await rpc('settings.set', {
    section: 'logs',
    value: { syslog: { enabled: false, udp_port: PORT, tcp_port: 0 } },
  });
}

function hint(page: Page) {
  return page.getByRole('region', { name: 'Log reception' });
}

function rows(page: Page) {
  return page.getByRole('list', { name: 'Log lines' }).getByRole('listitem');
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(300_000);
  harness = await buildHarness();
  backend = await startBackend({ uiDir: harness.dist });
  await seed(rpcFor(request, backend));
});

test.afterAll(async () => {
  await backend?.stop();
  if (harness) {
    await rm(harness.root, { recursive: true, force: true });
    await rm(harness.dist, { recursive: true, force: true });
  }
});

test('says the receiver is off and gives the exact command for a real device', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/logs', { backend });

  await expect(hint(page)).toContainText('The syslog receiver is switched off');
  await expect(hint(page)).toContainText('opens a listening port on this machine');

  // Whichever device is offered, the lines are that platform's own: the
  // RouterOS device gets RouterOS commands.
  await expect(hint(page)).toContainText('/system logging action set [find name=remote]');
  await expect(hint(page)).toContainText(`remote-port=${PORT}`);

  // And for the switch that will be sending: the real command, for its
  // platform, naming the port this installation would actually listen on and
  // the address of this machine as that switch would reach it.
  const picker = hint(page).getByRole('combobox', {
    name: 'Device to show the syslog commands for',
  });
  await picker.fill('CORE1');
  await picker.press('Enter');
  await expect(hint(page)).toContainText(`logging host 127.0.0.1 transport udp port ${PORT}`);
  await expect(hint(page)).toContainText('service timestamps log datetime msec');
  await expect(hint(page)).toContainText('These lines are not applied');
  // And nothing claims the network is quiet.
  await expect(page.getByRole('heading', { name: 'No line has arrived yet' })).toBeVisible();
  await expect(page.getByText(/No line matches this search/)).toHaveCount(0);

  // A build without the trap receiver says so rather than offering a switch.
  await expect(hint(page)).toContainText('SNMP traps and flow collection are not in this build');

  expect(console.messages).toEqual([]);
});

test('the screen can open the socket, and then says nothing has arrived yet', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/logs', { backend });

  await page.getByRole('button', { name: 'Turn the receiver on' }).click();

  // `logs.status_changed` arrives over the WebSocket and the hint changes its
  // mind about what is wrong: the port is open, and no device has sent to it.
  await expect(hint(page)).toContainText(`Listening on udp 0.0.0.0:${PORT}`, { timeout: 30_000 });
  await expect(hint(page)).toContainText('Devices do not send syslog until they are told where to');
  await expect(page.getByRole('contentinfo')).toContainText('No syslog yet');

  expect(console.messages).toEqual([]);
});

test('real lines arrive, are attributed to the device, and are counted honestly', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  await openApp(page, '/logs', { backend });

  await send([iosLine(0), iosLine(1), iosLine(2), iosLine(3), iosLine(4), iosLine(5)]);
  await stored(rpc, 6);

  // The tail is off, so the screen says lines have arrived rather than
  // changing under the reader.
  await page.getByRole('button', { name: 'Read it again' }).click();

  const first = rows(page).first();
  await expect(first).toContainText('CORE1');
  await expect(first).toContainText('LINK-3-UPDOWN');
  await expect(first).toContainText('changed state to down');
  await expect(first).toContainText('err');
  await expect(rows(page)).toHaveCount(6);

  // The histogram counted the same lines.
  await expect(page.getByTestId('log-counts')).toContainText('6 lines in this range');

  // The setup hint is gone: something is arriving now.
  await expect(page.getByText('The syslog receiver is switched off')).toHaveCount(0);
  await expect(page.getByRole('contentinfo')).toContainText('lines/min');

  expect(console.messages).toEqual([]);
});

test('a severity filter says what it is hiding, including the lines that never said', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/logs', { backend });

  // One line with no priority at all: its severity is unknown, which is not
  // the same as debug, and `severity <= 3` in SQL hides it too.
  await send(['a line with no priority %LINK-3-UPDOWN: Interface Gi1/0/9, changed state to down']);
  await page.getByRole('button', { name: 'Read the newest lines again' }).click();
  await expect(page.getByTestId('log-counts')).toContainText('7 lines in this range');

  await page.getByRole('combobox', { name: 'Severity' }).selectOption('3');

  const counts = page.getByTestId('log-counts');
  await expect(counts).toContainText('7 lines in this range');
  await expect(counts).toContainText('2 shown');
  await expect(counts).toContainText('4 less severe');
  await expect(counts).toContainText('1 that never said how severe it was');
  await expect(counts).toContainText('hidden by the severity filter');
  await expect(rows(page)).toHaveCount(2);

  // A filter that hides everything is still a filter, not an empty network.
  await page.getByRole('combobox', { name: 'Severity' }).selectOption('2');
  await expect(page.getByRole('heading', { name: 'No line matches this search' })).toBeVisible();
  await expect(page.getByText('none of them is critical and worse')).toBeVisible();
  await page.getByRole('button', { name: 'Clear the filters' }).click();
  await expect(rows(page)).toHaveCount(7);

  expect(console.messages).toEqual([]);
});

test('an empty bucket is drawn as a gap where nothing was stored, not as a zero', async ({
  page,
}) => {
  const console = watchConsole(page);
  // A week's range over a store that is minutes old: most of it is a gap,
  // because "we have nothing from then" is not "nothing happened then".
  await openApp(page, '/logs?span=7d', { backend });

  const note = page.getByTestId('log-gaps');
  await expect(note).toContainText('drawn as a gap, not as a zero');
  await expect(note).toContainText('older than the oldest line still stored');

  const plot = page.getByRole('group', { name: /Lines per/ });
  // Most of the week is unprovable; the last bucket is the one with the lines.
  expect(await plot.locator('button[data-reach="before-store"]').count()).toBeGreaterThan(60);
  await expect(plot.locator('button').last()).toHaveAttribute('aria-label', /lines —/);

  // Nothing in the picture claims a zero it cannot prove: every bucket is
  // either counted or a gap, and the gaps carry the reason.
  expect(await plot.locator('button[data-reach="not-listening"]').count()).toBe(0);

  expect(console.messages).toEqual([]);
});

test('the live tail follows the newest line, and scrolling up stops it following', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  await openApp(page, '/logs', { backend });

  // Enough lines that the list scrolls at all.
  await send(Array.from({ length: 400 }, (_value, index) => iosLine(index)));
  // Not "all 400": UDP may lose a datagram on the way and this test is about
  // the tail, not about datagram arithmetic.
  await stored(rpc, 300);

  await page.locator('label', { hasText: 'Live tail' }).click();
  await expect(page.getByRole('switch', { name: 'Live tail' })).toBeChecked();
  await expect(page.getByText('Following the newest lines')).toBeVisible({ timeout: 30_000 });

  // A line sent now is streamed in by `logs.follow`, and the list is at the
  // bottom, so it is on screen without anybody scrolling.
  await send(['<187>901: *Nov 14 00:41:11.001: %SYS-5-RESTART: System restarted by a tail test']);
  const newest = page.getByRole('listitem').filter({ hasText: 'restarted by a tail test' });
  await expect(newest).toBeVisible({ timeout: 30_000 });
  await expect(newest).toBeInViewport();

  // Now scroll up: the tail must stop dragging the reader to the bottom.
  const scroller = page.getByTestId('log-scroller');
  await scroller.evaluate((element) => {
    element.scrollTop = Math.max(0, element.scrollHeight - element.clientHeight - 600);
  });
  await expect(page.getByText('Tail running — scrolled back, so it is not jumping')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Jump to the newest line' })).toBeVisible();

  const before = await scroller.evaluate((element) => ({
    top: element.scrollTop,
    height: element.scrollHeight,
  }));
  await send(['<187>902: *Nov 14 00:41:12.001: %SYS-5-RESTART: System restarted while reading']);
  // The line goes into the list — the list grew by a row — and the reader has
  // not been moved an inch. (The row itself is not in the document: it is
  // below the viewport, and only what is in view is rendered.)
  await expect
    .poll(async () => scroller.evaluate((element) => element.scrollHeight), { timeout: 30_000 })
    .toBeGreaterThan(before.height);
  expect(await scroller.evaluate((element) => element.scrollTop)).toBe(before.top);
  const later = page.getByRole('listitem').filter({ hasText: 'restarted while reading' });
  await expect(later).toHaveCount(0);

  // Asking to go back to the end works, following resumes, and what arrived
  // while they were reading is there.
  await page.getByRole('button', { name: 'Jump to the newest line' }).click();
  await expect(page.getByText('Following the newest lines')).toBeVisible();
  await expect(later).toBeInViewport();

  // The list is virtualised: tens of thousands of rows would be the same
  // handful of elements as these hundreds are.
  const rendered = await rows(page).count();
  expect(rendered).toBeGreaterThan(5);
  expect(rendered).toBeLessThan(120);

  // Switching it off closes the follow in the engine rather than leaving it
  // filtering every line for a screen nobody is looking at.
  await page.locator('label', { hasText: 'Live tail' }).click();
  await expect(page.getByRole('switch', { name: 'Live tail' })).not.toBeChecked();

  expect(console.messages).toEqual([]);
});

test('paging back loads older lines without moving what the reader is reading', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  // One page is 500 lines, so there has to be more than that to page back
  // through. (UDP loses a few on the way, which is why this is not exact.)
  await send(Array.from({ length: 400 }, (_value, index) => iosLine(index)));
  await stored(rpc, 600);
  await openApp(page, '/logs', { backend });

  await expect(page.getByRole('button', { name: /Load \d+ older lines/ })).toBeVisible();

  const scroller = page.getByTestId('log-scroller');
  /** How far the viewport is from the end of the list: the reader's place. */
  const place = () =>
    scroller.evaluate(
      (element) => element.scrollHeight - element.scrollTop - element.clientHeight,
    );
  const height = await scroller.evaluate((element) => {
    element.scrollTop = 0;
    return element.scrollHeight;
  });
  const before = await place();
  // Scrolling to the very top loads the page before this one…
  await expect
    .poll(async () => scroller.evaluate((element) => element.scrollHeight), { timeout: 20_000 })
    .toBeGreaterThan(height);
  // …and the reader is left looking at the same lines: the list grew upwards
  // and the viewport came with it, so the distance to the end has not moved.
  // (Within a row: the "load older" button leaves the top of the list when
  // the last page has been read, and it is a row and a half tall.)
  expect(Math.abs((await place()) - before)).toBeLessThan(40);

  expect(console.messages).toEqual([]);
});

test('an address no device claims is listed with the engine’s own reason', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // Another loopback address: a real datagram from a source the inventory
  // does not know. Its lines are kept and the address is listed, because a
  // setting that makes a device invisible has to be visible itself.
  await send(
    ['<188>401: *Nov 14 00:51:00.001: %SPANTREE-2-BLOCK_BPDUGUARD: Received BPDU on Gi0/2'],
    { from: '127.0.0.2' },
  );
  await expect
    .poll(async () => (await rpc<{ unattributed_sources: number }>('logs.status')).unattributed_sources, {
      timeout: 30_000,
    })
    .toBe(1);

  await openApp(page, '/logs', { backend });
  await page.getByRole('button', { name: /address no device claims/ }).click();
  const table = page.getByRole('table', { name: 'Addresses no device claims' });
  await expect(table).toContainText('127.0.0.2');
  await expect(table).toContainText('no device in the inventory has the address 127.0.0.2');

  expect(console.messages).toEqual([]);
});

test('a line read from a device’s buffer keeps the device’s clock, and says so', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // A simulated switch is an inventory device like any other. Its own log
  // buffer is read over the read-only CLI path, and a freshly booted
  // simulation thinks it is March — which is exactly the wrong-clock case a
  // real estate is full of.
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
  const labSwitch = devices.find((device) => device.name === 'LogLab/SW1')?.id ?? '';
  expect(labSwitch, 'the lab switch is in the inventory').not.toBe('');

  const pulled = await rpc<{ stored: number; dated_by_device: number; newest_at: number }>(
    'logs.pull',
    { device_id: labSwitch },
  );
  expect(pulled.stored).toBeGreaterThan(0);
  expect(pulled.dated_by_device, 'the device dated its own lines').toBeGreaterThan(0);

  // The line is months from now, so it is read at the time the device says —
  // which is the point.
  const at = pulled.newest_at;
  await openApp(page, `/logs?from=${at - 3_600_000}&to=${at + 3_600_000}`, { backend });

  const line = page.getByRole('listitem').filter({ hasText: 'System restarted' }).first();
  await expect(line).toBeVisible();
  await expect(line).toContainText('LogLab/SW1');
  await expect(line.locator('[data-clock="device"]')).toBeVisible();

  // The same window is the one place in this suite where an empty bucket can
  // be *proved* to be a zero: the device's own line is now the oldest thing
  // in the store, so the quarter of an hour after it is covered by the store
  // and really was quiet.
  const proven = page
    .getByRole('group', { name: /Lines per/ })
    .locator('button[data-reach="counted"][aria-label*="no line matched"]');
  await expect(proven.first()).toBeVisible();
  expect(await proven.count()).toBeGreaterThan(5);
  // And the same picture carries both answers: the hour *before* that line is
  // not a quiet hour, it is an hour nothing is stored from.
  await expect(page.getByTestId('log-gaps')).toContainText(
    'older than the oldest line still stored',
  );
  expect(
    await page
      .getByRole('group', { name: /Lines per/ })
      .locator('button[data-reach="before-store"]')
      .count(),
  ).toBeGreaterThan(5);

  await line.getByRole('button').click();
  const drawer = page.getByRole('dialog', { name: 'SYS-5-RESTART' });
  await expect(drawer).toContainText('Time (device’s clock)');
  await expect(drawer).toContainText('read out of its log buffer');
  await expect(drawer).toContainText('read out of the device’s own log buffer');
  // And the raw line, as the device wrote it.
  await expect(drawer).toContainText('%SYS-5-RESTART: System restarted');
  await page.keyboard.press('Escape');

  expect(console.messages).toEqual([]);
});

test('the search box understands fields, and says so when it does not', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/logs?span=24h', { backend });

  const box = page.getByRole('searchbox', { name: 'Search the logs' });
  await box.fill('mnemonic:SYS-5-CONFIG_I');
  await expect(rows(page).first()).toContainText('Configured from console', { timeout: 15_000 });
  await expect(rows(page).first()).toContainText('SYS-5-CONFIG_I');

  // A phrase and an exclusion go to the engine, which is where they are
  // understood — the browser does not have a second search language.
  await box.fill('"changed state to down" -LINEPROTO');
  await expect(rows(page).first()).toContainText('changed state to down', { timeout: 15_000 });
  await expect(page.getByText('Line protocol on Interface')).toHaveCount(0);

  // A field that does not exist is named rather than searched for.
  await box.fill('severity:3');
  await expect(page.getByText('is not a field')).toBeVisible();

  expect(console.messages).toEqual([]);
});

test('the view survives a reload, because it is in the address', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/logs?q=mnemonic%3ALINK-3-UPDOWN&sev=3&span=24h', { backend });
  await expect(rows(page).first()).toContainText('LINK-3-UPDOWN');
  await expect(page.getByRole('combobox', { name: 'Severity' })).toHaveValue('3');

  await page.reload();
  await pageLoaded(page);
  await expect(page.getByRole('searchbox', { name: 'Search the logs' })).toHaveValue(
    'mnemonic:LINK-3-UPDOWN',
  );
  await expect(rows(page).first()).toContainText('LINK-3-UPDOWN');

  expect(console.messages).toEqual([]);
});
