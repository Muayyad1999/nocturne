/**
 * The Tools screen, against a real engine.
 *
 * Nothing here is stubbed: a `nocturne-server` on a fresh data directory
 * serving the production bundle, a simulated switch for the device probe, and
 * — for the probes that need something to answer — listeners this test starts
 * itself on ports the operating system hands out. No internet, no fixed port,
 * no privilege.
 *
 * The suite is built around the two claims the toolbox crate was written to
 * keep, because they are the two a screen can quietly break:
 *
 * 1. **A probe that could not run is not a result.** Whether this machine may
 *    send ICMP is not something a test can decide, so the ping test asserts
 *    *both* branches the way the crate's own test does — a report, or a
 *    refusal naming the reason — and in the refusal case asserts that no
 *    round-trip time and no "100 % loss" is anywhere on screen. The NTP test
 *    makes the failing case deterministic by asking a port nothing listens on.
 * 2. **`reversible` and `decodable` are different questions.** A Cisco type 6
 *    secret is reversible by design and still not recoverable here, and the
 *    screen has to say both.
 *
 * Every test asserts the browser console stayed clean.
 */

import { execFile } from 'node:child_process';
import { createServer, type Server } from 'node:net';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { goTo, openApp, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;
/** A TCP listener on a port the system chose, for the port check. */
let listener: Server;
let openPort = 0;

test.beforeAll(async ({ request }) => {
  test.setTimeout(420_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = process.env.NOCTURNE_UI_DIST ?? (await mkdtemp(join(tmpdir(), 'nocturne-ui-')));
  if (process.env.NOCTURNE_UI_DIST === undefined) {
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 300_000,
    });
  }
  backend = await startBackend({ uiDir });

  // A simulated switch: the device probe builds a real IOS command and sends
  // it through the read-only gate to a device that really answers.
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
  expect(openPort).toBeGreaterThan(0);
});

test.afterAll(async () => {
  await new Promise<void>((done) => listener?.close(() => done()));
  await backend?.stop();
  if (uiDir && process.env.NOCTURNE_UI_DIST === undefined) {
    await rm(uiDir, { recursive: true, force: true });
  }
});

test.beforeEach(({ page }) => {
  page.setDefaultTimeout(20_000);
});

// ------------------------------------------------------------------ helpers

/** The tool on the right: one panel, whatever it is. */
const panel = (page: Page): Locator => page.getByRole('article');

/** The drawer on the left. */
const drawer = (page: Page): Locator => page.getByRole('navigation', { name: 'Tools' });

/** Types into a field by its label; a number field is a spinbutton, not a textbox. */
async function field(page: Page, name: string, value: string): Promise<void> {
  await panel(page).getByLabel(name, { exact: true }).fill(value);
}

async function press(page: Page, name: string): Promise<void> {
  await panel(page).getByRole('button', { name, exact: true }).click();
}

/** The echo lines of a live ping, newest last. */
async function echoLines(page: Page): Promise<string[]> {
  const stream = panel(page).getByLabel('Echoes, newest last');
  if ((await stream.count()) === 0) return [];
  return stream.locator('> div').allInnerTexts();
}

// ------------------------------------------------------------- the drawer

test.describe('the drawer', () => {
  test('is the engine’s own catalogue, and every tool in it has a form', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const catalogue = await rpc<{ id: string; title: string; group: string }[]>('tools.list');
    expect(catalogue.length).toBeGreaterThan(40);

    await openApp(page, '/tools', { backend });
    await expect(page.getByRole('heading', { level: 1, name: 'Tools' })).toBeVisible();

    // The methods that are one panel with another (tftp.stop, the history's
    // get and delete, the reference's table list) are folded into it.
    const rows = drawer(page).getByRole('button');
    await expect(rows).toHaveCount(43);
    for (const title of ['Subnet calculator', 'Ping Probe', 'TFTP server Server', 'What was run Record']) {
      await expect(drawer(page).getByRole('button', { name: title, exact: true })).toBeVisible();
    }
    await expect(drawer(page).getByRole('button', { name: /^Stop the TFTP server/ })).toHaveCount(0);

    // Nothing in the catalogue is left without a form on this screen.
    await expect(page.getByText('No form for this tool yet')).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });

  test('filters on what a tool does, not only on its name, and keeps the groups in order', async ({
    page,
  }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools', { backend });

    // `/` focuses the filter, as on every other screen.
    await page.keyboard.press('/');
    await expect(page.getByRole('searchbox', { name: 'Filter tools' })).toBeFocused();

    await page.getByRole('searchbox', { name: 'Filter tools' }).fill('mss');
    await expect(drawer(page).getByRole('button').first()).toHaveText(/MTU and MSS/);
    // A filter that matched half the drawer would not be one.
    expect(await drawer(page).getByRole('button').count()).toBeLessThan(5);

    await page.getByRole('searchbox', { name: 'Filter tools' }).fill('secret');
    const headings = await drawer(page).getByRole('heading').allInnerTexts();
    expect(headings[0]?.toLowerCase()).toBe('secrets');

    // With nothing typed the shelves are back in their own order.
    await page.getByRole('searchbox', { name: 'Filter tools' }).fill('');
    expect((await drawer(page).getByRole('heading').allInnerTexts())[0]?.toLowerCase()).toBe(
      'addressing',
    );

    await page.getByRole('searchbox', { name: 'Filter tools' }).fill('zzzz');
    await expect(page.getByText('Nothing in the drawer matches')).toBeVisible();

    expect(console.messages).toEqual([]);
  });
});

// ---------------------------------------------------------- the calculators

test.describe('a calculator', () => {
  test('answers as it is typed into, and refuses in the engine’s own words', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/tools.subnet', { backend });

    await expect(page.getByText('Nothing asked yet')).toBeVisible();
    await field(page, 'Address, prefix or mask', '10.1.2.33/26');

    // No button was pressed.
    await expect(panel(page).getByText('255.255.255.192')).toBeVisible();
    await expect(panel(page).getByText('10.1.2.1 – 10.1.2.62')).toBeVisible();
    await expect(panel(page).getByText('00001010.00000001.00000010.00100001')).toBeVisible();

    // A refusal replaces the answer; it does not sit beside a stale one.
    await field(page, 'Address, prefix or mask', '10.0.300.1/24');
    await expect(panel(page).getByText('That input will not do')).toBeVisible();
    await expect(panel(page).getByText('is not a valid octet (0-255)')).toBeVisible();
    await expect(panel(page).getByText('255.255.255.192')).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });

  test('remembers what was typed, and comes back to it', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/tools.mtu', { backend });
    await field(page, 'Link MTU', '1500');
    await field(page, 'Encapsulations, outermost first', 'gre, ipsec-esp-tunnel');
    await expect(panel(page).getByText('1422')).toBeVisible();

    await goTo(page, '/tools/tools.subnet');
    await goTo(page, '/tools/tools.mtu');
    await expect(panel(page).getByLabel('Link MTU', { exact: true })).toHaveValue('1500');
    await expect(panel(page).getByText('1422')).toBeVisible();

    expect(console.messages).toEqual([]);
  });
});

// --------------------------------------------------------------- the secrets

test.describe('a secret in a configuration', () => {
  test('is reversible and still not recoverable here, and the screen says both', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/tools.secret.identify', { backend });

    // Cisco type 6: AES under the device's own master key.
    await field(
      page,
      'The line from the configuration',
      'pre-shared-key local 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB',
    );
    await expect(panel(page).getByText('Cisco type 6', { exact: false })).toBeVisible();
    await expect(panel(page).getByText('Reversible by design')).toBeVisible();
    await expect(panel(page).getByText('Nocturne cannot recover it')).toBeVisible();
    await expect(panel(page).getByText('the key belongs to the device', { exact: false })).toBeVisible();

    // Type 7: the same question, the other answer.
    await field(page, 'The line from the configuration', 'username admin password 7 0822455D0A16');
    await expect(panel(page).getByText('Nocturne can recover it')).toBeVisible();
    await expect(panel(page).getByText('Reversible by design')).toBeVisible();

    // A hash is neither.
    await field(page, 'The line from the configuration', 'enable secret 5 $1$mERr$3GCGQMcGNJZPAJLtgvmgO0');
    await expect(panel(page).getByText('One way: a hash')).toBeVisible();
    await expect(panel(page).getByText('Nothing can recover it')).toBeVisible();

    expect(console.messages).toEqual([]);
  });

  test('is decoded with the note that says what that is worth, and kept nowhere', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/tools.secret.decode', { backend });

    await field(page, 'The value, or the line it sits in', 'username admin password 7 0822455D0A16');
    await expect(panel(page).getByText('cisco', { exact: true })).toBeVisible();
    await expect(panel(page).getByText('obfuscation with a published key, not encryption', { exact: false })).toBeVisible();
    await expect(panel(page).getByText('Shown here and kept nowhere', { exact: false })).toBeVisible();

    // Nothing of this is written to the machine: not the line, not the secret.
    const stored = await page.evaluate(() =>
      Object.entries({ ...window.localStorage }).map(([key, value]) => `${key}=${String(value)}`).join('\n'),
    );
    expect(stored).not.toContain('0822455D0A16');
    expect(stored).not.toContain('cisco');

    expect(console.messages).toEqual([]);
  });
});

// ---------------------------------------------------------------- the probes

test.describe('a probe from this machine', () => {
  test('streams its echoes in order and ends in a report — or says it could not run', async ({
    page,
  }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/probe.ping', { backend });

    await field(page, 'Host', '127.0.0.1');
    await field(page, 'Echoes', '6');
    await field(page, 'Every (ms)', '250');
    await press(page, 'Ping');

    // While it runs the echoes are listed as they come back. They are
    // collected here as the run goes on, so what is asserted is the order
    // they *arrived* in and not only the order of the finished report.
    const seen: string[][] = [];
    const refusal = panel(page).getByText('This machine may not run that probe');
    const report = panel(page).getByText('Every echo');
    for (let tick = 0; tick < 60; tick += 1) {
      if (await refusal.isVisible()) break;
      if (await report.isVisible()) break;
      const lines = await echoLines(page);
      if (lines.length > 0 && lines.length !== seen.at(-1)?.length) seen.push(lines);
      await page.waitForTimeout(120);
    }
    await expect(refusal.or(report)).toBeVisible({ timeout: 60_000 });

    if (await refusal.isVisible()) {
      // The refusal must not be dressed as a measurement.
      const text = await panel(page).innerText();
      expect(text).toContain('forbidden');
      expect(text).not.toMatch(/\d+(\.\d+)? ms/);
      expect(text).not.toContain('100%');
      await expect(panel(page).getByText('Full height is')).toHaveCount(0);
      test.info().annotations.push({
        type: 'note',
        description: 'This machine may not send ICMP; the refusal branch was asserted instead.',
      });
    } else {
      // Every snapshot is the one before it with more on the end, numbered
      // from 1 without gaps: the events arrived in order.
      expect(seen.length).toBeGreaterThan(1);
      for (const lines of seen) {
        const seqs = lines.map((line) => Number(line.split(/\s/)[0]));
        expect(seqs).toEqual(seqs.map((_value, index) => index + 1));
        expect(lines.every((line) => / ms/.test(line) || /no reply/.test(line))).toBe(true);
      }
      const first = seen[0] ?? [];
      const last = seen.at(-1) ?? [];
      expect(last.length).toBeGreaterThan(first.length);
      expect(last.slice(0, first.length)).toEqual(first);

      // The report that follows agrees with what was streamed.
      const text = await panel(page).innerText();
      expect(text).toContain('of 6 answered');
      expect(text).toContain('Ran from');
      await expect(panel(page).getByRole('table', { name: 'Every echo' }).getByRole('row')).toHaveCount(7);
    }

    expect(console.messages).toEqual([]);
  });

  test('accounts for every port it tried, not only the open one', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/probe.ports', { backend });

    await field(page, 'Host', '127.0.0.1');
    await field(page, 'Ports', `${openPort},9`);
    await press(page, 'Check ports');

    await expect(panel(page).getByText('Every port tried')).toBeVisible({ timeout: 60_000 });
    const text = await panel(page).innerText();
    // The listener this test started answered; the other port did not.
    expect(text).toContain(String(openPort));
    expect(text).toContain('open');
    expect(text).toContain('closed');
    expect(text).toContain('SSH-2.0-NocturneTest');
    // "Closed" and "not tried" are different things, so both are in the table.
    await expect(panel(page).getByRole('row')).toHaveCount(3);

    expect(console.messages).toEqual([]);
  });

  test('that could not run shows the refusal and its code, and no figures at all', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/probe.ntp', { backend });

    // Nothing listens on this port of the loopback address, so the engine
    // answers `unavailable` — a refusal, not a reading.
    await field(page, 'NTP server', `127.0.0.1:${openPort + 1}`);
    await press(page, 'Ask the server');

    await expect(panel(page).getByText('The target could not be reached')).toBeVisible({
      timeout: 60_000,
    });
    const text = await panel(page).innerText();
    expect(text).toContain('The engine called this');
    expect(text).toContain('unavailable');
    expect(text).toContain('This is not 100 % loss');
    // No reading of any kind: no "Ran from" line, no stratum, no timing, no
    // zero pretending to be one. (The word "Stratum" is in the tool's own
    // description above the form, which is why this looks for the shapes a
    // *result* has rather than for the word.)
    await expect(panel(page).getByText('Ran from')).toHaveCount(0);
    await expect(panel(page).getByText('Offset from this clock')).toHaveCount(0);
    expect(text).not.toMatch(/\d+(\.\d+)? ms/);

    expect(console.messages).toEqual([]);
  });
});

// --------------------------------------------------------- probes on a device

test.describe('a probe run by a device', () => {
  test('sends the platform’s own command and reads its summary back', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/probe.device.ping', { backend });

    await panel(page).getByRole('combobox', { name: 'Device' }).click();
    await page.getByRole('option', { name: /Bench\/SW1/ }).first().click();
    await field(page, 'Target', '10.0.0.1');
    await press(page, 'Ping from the device');

    await expect(panel(page).getByText('ping 10.0.0.1 repeat 5')).toBeVisible({ timeout: 90_000 });
    const text = await panel(page).innerText();
    expect(text).toContain('ios-like');
    expect(text).toContain('Bench/SW1 itself');
    // The simulated switch has no route to 10.0.0.1: a real 100 % loss, which
    // is a reading and is shown as one.
    expect(text).toContain('100% — 0 of 5 answered');
    expect(text).toContain('Success rate is 0 percent');
    // Simulated devices are labelled wherever they appear.
    expect(text).toContain('simulated device');

    expect(console.messages).toEqual([]);
  });

  test('refuses a target that is not one, by name, and sends nothing', async ({ page }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/probe.device.ping', { backend });

    await panel(page).getByRole('combobox', { name: 'Device' }).click();
    await page.getByRole('option', { name: /Bench\/SW1/ }).first().click();
    await field(page, 'Target', '8.8.8.8; reload');
    await press(page, 'Ping from the device');

    await expect(panel(page).getByText('That request will not do')).toBeVisible({ timeout: 60_000 });
    const text = await panel(page).innerText();
    expect(text).toContain('is not a host name or an address');
    expect(text).not.toContain('reload\n');
    expect(text).not.toContain('Success rate');

    expect(console.messages).toEqual([]);
  });
});

// --------------------------------------------------------------- the servers

test('a server that binds a socket says why it cannot be started from a browser', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/tools/tftp.start', { backend });

  await expect(panel(page).getByText('Not running')).toBeVisible();
  await expect(
    panel(page).getByText('it can only be started from the desktop application', { exact: false }),
  ).toBeVisible();
  // No control that would only ever be refused.
  await expect(panel(page).getByRole('button', { name: 'Start serving' })).toHaveCount(0);
  // The state it does have is the engine's, not this screen's.
  await expect(panel(page).getByText('0.0.0.0:6969')).toBeVisible();

  expect(console.messages).toEqual([]);
});

// --------------------------------------------------------------- the history

test('the history holds every run, and a run that failed has no report', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/tools/probe.history', { backend });

  const table = panel(page).getByRole('table', { name: 'Probe runs' });
  await expect(table).toBeVisible();
  // The runs the tests above made, with what became of each.
  await expect(table.getByText('Answered').first()).toBeVisible();
  await expect(table.getByRole('row', { name: /NTP query/ })).toBeVisible();

  // Opening the failed one: the refusal, its code, and no figures.
  await table.getByRole('row', { name: /NTP query/ }).click();
  await table.getByRole('row', { name: /NTP query/ }).press('Enter');
  const opened = page.getByRole('region', { name: 'One probe run' });
  await expect(opened).toBeVisible();
  await expect(opened.getByText('The target could not be reached')).toBeVisible();
  const text = await opened.innerText();
  expect(text).toContain('no result at all');
  expect(text).toContain('unavailable');
  // A failed run carries `result: null`: there is nothing to render, and
  // nothing is rendered.
  expect(text).not.toMatch(/\d+(\.\d+)? ms/);

  expect(console.messages).toEqual([]);
});

// ------------------------------------------------------- leaving and coming back

test('a run that is still going is picked up again, not started twice', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc: Rpc = rpcFor(request, backend);

  await openApp(page, '/tools/probe.ping', { backend });
  await field(page, 'Host', '127.0.0.1');
  await field(page, 'Echoes', '0'); // until it is stopped
  await field(page, 'Every (ms)', '400');
  await press(page, 'Ping');

  const refusal = panel(page).getByText('This machine may not run that probe');
  const running = panel(page).getByRole('button', { name: 'Stop' });
  await expect(refusal.or(running)).toBeVisible({ timeout: 60_000 });
  if (await refusal.isVisible()) {
    test.info().annotations.push({
      type: 'note',
      description: 'This machine may not send ICMP, so there was no run to come back to.',
    });
    expect(console.messages).toEqual([]);
    return;
  }

  // Leave the screen entirely and come back: the run is the backend's.
  await goTo(page, '/devices');
  await expect(page.getByRole('heading', { level: 1, name: 'Devices' })).toBeVisible();
  await goTo(page, '/tools/probe.ping');

  await expect(panel(page).getByRole('button', { name: 'Stop' })).toBeVisible();
  await expect(panel(page).getByText('Picked up where it was')).toBeVisible();
  await expect(
    panel(page).getByText('already going when the screen opened', { exact: false }),
  ).toBeVisible();
  // One run, not two: the screen attached rather than starting another.
  const history = await rpc<{ runs: { ok: boolean | null }[] }>('probe.history', { kind: 'ping' });
  expect(history.runs.filter((entry) => entry.ok === null)).toHaveLength(1);

  await press(page, 'Stop');
  await expect(panel(page).getByText('Stopped by you after', { exact: false })).toBeVisible({
    timeout: 30_000,
  });

  expect(console.messages).toEqual([]);
});
