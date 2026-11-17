/**
 * The Lab screen, against a real engine and a real simulator.
 *
 * Nothing here is mocked. A `nocturne-server` runs on a fresh data
 * directory, the UI is the production bundle it serves, and the lab is built
 * the way a person would build it: devices dragged out of the palette,
 * cables drawn between them, a console opened and IOS typed into it.
 *
 * The long test is the one that matters — two switches, a router, two hosts,
 * addresses configured at the consoles, `show ip interface brief` read back
 * from the device, and then the simulator asked whether one host can reach
 * the other. Everything else is a corner of that.
 */

import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

test.beforeAll(async () => {
  test.setTimeout(420_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
    cwd: UI_DIR,
    timeout: 300_000,
  });
  backend = await startBackend({ uiDir });
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

// ------------------------------------------------------------------ helpers

/** The text on the visible terminal, row by row. */
async function screenText(page: Page): Promise<string> {
  return page.evaluate(() => {
    const rows = [...document.querySelectorAll('.xterm-rows')].find(
      (element) => (element as HTMLElement).offsetParent !== null,
    );
    if (!rows) return '';
    return [...rows.children]
      .map((row) => (row.textContent ?? '').replace(/ /g, ' ').trimEnd())
      .join('\n');
  });
}

async function expectOnScreen(page: Page, text: string): Promise<void> {
  await expect
    .poll(async () => (await screenText(page)).includes(text), {
      message: `waiting for “${text}” on the console`,
      timeout: 20_000,
    })
    .toBe(true);
}

/** Types into the console that is on screen, as a person would. */
async function typeConsole(page: Page, ...lines: string[]): Promise<void> {
  await page.locator('.xterm-screen:visible').first().click();
  for (const line of lines) {
    // A gap between keystrokes: without one they can arrive out of order
    // on a busy machine. See `typeInTerminal` in terminal.spec.ts.
    await page.keyboard.type(`${line}\r`, { delay: 15 });
    await page.waitForTimeout(90);
  }
}

function node(page: Page, name: string) {
  return page.getByRole('button', { name: new RegExp(`^${name},`) });
}

/** Adds a device from the palette and waits for it to appear on the canvas. */
async function addDevice(page: Page, model: string, expected: string): Promise<void> {
  await page.getByRole('button', { name: `Add ${model}` }).click();
  await expect(node(page, expected)).toBeVisible({ timeout: 15_000 });
}

/**
 * Draws a cable between two devices with the cable tool: one click each. The
 * tool stays armed after a cable, so it is only turned on when it is off.
 */
async function cable(page: Page, a: string, b: string): Promise<void> {
  const tool = page.getByRole('button', { name: 'Cable', exact: true });
  if ((await tool.getAttribute('aria-pressed')) !== 'true') await tool.click();
  await node(page, a).click();
  await node(page, b).click();
  await expect(page.getByText(new RegExp(`Cabled ${a} `)).first()).toBeVisible({ timeout: 15_000 });
}

/** Opens a lab and leaves the screen on its canvas. */
async function openLab(page: Page, id: string): Promise<void> {
  await openApp(page, `/lab/${id}`, { backend });
  await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
}

// -------------------------------------------------------------------- tests

test.describe('the labs', () => {
  test('creates a lab from an example, starts it and stops it again', async ({ page }) => {
    const consoleWatch = watchConsole(page);
    await openApp(page, '/lab', { backend });

    await expect(page.getByRole('heading', { level: 1, name: 'Lab' })).toBeVisible();
    // An installation with no labs says what a lab is for.
    await expect(page.getByText('No labs yet')).toBeVisible();

    await page.getByRole('button', { name: 'New lab' }).first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    // The examples are the simulator's own, with what each one teaches.
    await expect(page.getByRole('radio', { name: /Campus/ })).toBeVisible();
    await page.getByRole('radio', { name: /OSPF triangle/ }).click();
    await page.getByRole('textbox', { name: 'Name' }).fill('Triangle');
    await page.getByRole('button', { name: 'Create' }).click();

    // Creating opens the lab, and the canvas is the simulator's, not a guess.
    await expect(page.getByRole('heading', { level: 1, name: 'Triangle' })).toBeVisible();
    await expect(node(page, 'R1')).toBeVisible();
    await expect(page.getByText('Stopped')).toBeVisible();

    await page.getByRole('button', { name: 'Start' }).click();
    await expect(page.getByText('Running')).toBeVisible({ timeout: 30_000 });

    // The list shows the running state the backend holds, not what we clicked.
    await page.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('link', { name: 'Lab' }).click();
    await pageLoaded(page);
    const card = page.getByRole('listitem').filter({ hasText: 'Triangle' });
    await expect(card.getByText('Running')).toBeVisible();

    await card.getByRole('button', { name: 'Stop' }).click();
    await expect(card.getByText('Stopped')).toBeVisible({ timeout: 30_000 });

    expect(consoleWatch.messages, backend.log()).toEqual([]);
  });

  test('an empty lab is empty, and says so', async ({ page, request }) => {
    const consoleWatch = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const lab = await rpc<{ id: string }>('labs.create', { name: 'Nothing here' });

    await openLab(page, lab.id);

    await expect(page.getByRole('heading', { name: 'This lab is empty' })).toBeVisible();
    await expect(page.getByRole('heading', { name: 'Nothing selected' })).toBeVisible();

    // Reachability refuses to invent an answer about a lab with nothing in it.
    await page.getByRole('button', { name: 'Reachability' }).click();
    await expect(page.getByRole('heading', { name: 'The lab is not running' })).toBeVisible();

    expect(consoleWatch.messages, backend.log()).toEqual([]);
  });
});

test.describe('a lab built by hand', () => {
  /** The lab this group builds; the later tests reuse it. */
  let labId = '';

  test('builds two switches and a router, runs it, and the console answers', async ({
    page,
    request,
  }) => {
    test.setTimeout(240_000);
    const consoleWatch = watchConsole(page);
    const rpc = rpcFor(request, backend);
    const lab = await rpc<{ id: string }>('labs.create', { name: 'Two sites' });
    labId = lab.id;

    await openLab(page, labId);
    await expect(page.getByRole('heading', { name: 'This lab is empty' })).toBeVisible();

    // 1. The devices, out of the catalog the backend answered with.
    await addDevice(page, 'Router (4 × GE)', 'R1');
    await addDevice(page, 'Switch (24 × FE + 2 × GE)', 'SW1');
    await addDevice(page, 'Switch (24 × FE + 2 × GE)', 'SW2');
    await addDevice(page, 'PC', 'PC1');
    await addDevice(page, 'PC', 'PC2');

    // 2. The cables. One click on each end; the simulator picks the ports.
    await cable(page, 'R1', 'SW1');
    await cable(page, 'R1', 'SW2');
    await cable(page, 'PC1', 'SW1');
    await cable(page, 'PC2', 'SW2');
    await page.keyboard.press('Escape');

    // The topology is the backend's: four cables, as the simulator stored them.
    const stored = await rpc<{ snapshot: { links: unknown[] } }>('labs.get', { id: labId });
    expect(stored.snapshot.links).toHaveLength(4);

    // 3. Run it.
    await page.getByRole('button', { name: 'Start' }).click();
    await expect(page.getByText('Running')).toBeVisible({ timeout: 60_000 });

    // 4. A console on the router, and real IOS typed into it.
    await node(page, 'R1').dblclick();
    await expect(page.getByRole('tab', { name: /R1/ })).toBeVisible({ timeout: 30_000 });
    await expectOnScreen(page, 'R1>');

    await typeConsole(
      page,
      'enable',
      'configure terminal',
      'interface GigabitEthernet0/0',
      'ip address 10.0.1.1 255.255.255.0',
      'no shutdown',
      'interface GigabitEthernet0/1',
      'ip address 10.0.2.1 255.255.255.0',
      'no shutdown',
      'end',
      'terminal length 0',
      'show ip interface brief',
    );

    // What the device printed, read back off the screen.
    await expectOnScreen(page, '10.0.1.1');
    const brief = await screenText(page);
    expect(brief, brief).toContain('GigabitEthernet0/0');
    expect(brief, brief).toMatch(/GigabitEthernet0\/0\s+10\.0\.1\.1\s+YES/);
    expect(brief, brief).toMatch(/GigabitEthernet0\/1\s+10\.0\.2\.1\s+YES/);

    // 5. The hosts, each at its own console.
    await node(page, 'PC1').dblclick();
    await expect(page.getByRole('tab', { name: /PC1/ })).toBeVisible({ timeout: 30_000 });
    await expectOnScreen(page, 'C:\\>');
    await typeConsole(page, 'ip 10.0.1.10 255.255.255.0 10.0.1.1');
    // The host says it took the address; waiting for the device rather than
    // for a clock is what keeps this test honest about what it proved.
    await expectOnScreen(page, 'Static IP configuration applied.');

    await node(page, 'PC2').dblclick();
    await expect(page.getByRole('tab', { name: /PC2/ })).toBeVisible({ timeout: 30_000 });
    await expectOnScreen(page, 'C:\\>');
    await typeConsole(page, 'ip 10.0.2.10 255.255.255.0 10.0.2.1');
    await expectOnScreen(page, 'Static IP configuration applied.');

    // What was typed at the three consoles is in the simulator, not just on
    // the screen: the addresses are in the lab the backend holds.
    const configured = await rpc<{
      snapshot: { nodes: { name: string; interfaces: { ip?: string | null }[] }[] };
    }>('labs.get', { id: labId });
    const addresses = Object.fromEntries(
      configured.snapshot.nodes.map((item) => [
        item.name,
        item.interfaces.map((port) => port.ip).filter(Boolean),
      ]),
    );
    const shown = JSON.stringify(addresses);
    expect(addresses.R1, shown).toEqual(['10.0.1.1/24', '10.0.2.1/24']);
    expect(addresses.PC1, shown).toEqual(['10.0.1.10/24']);
    expect(addresses.PC2, shown).toEqual(['10.0.2.10/24']);

    // Spanning tree holds a port that has just come up in listening and
    // learning before it forwards, so the path is not up the instant the
    // addresses are typed. Wait for the simulator itself to say it is, then
    // ask the screen — what is being tested is what the screen does with the
    // answer, not how fast the simulated network converges.
    await expect
      .poll(
        async () =>
          (await rpc<{ ok: boolean }>('labs.probe', { lab_id: labId, from: 'PC1', to: 'PC2' })).ok,
        { message: 'the path from PC1 to PC2 to come up', timeout: 60_000 },
      )
      .toBe(true);

    // 6. And now the question the whole screen exists to answer.
    await page.getByRole('button', { name: 'Reachability' }).click();
    const drawer = page.getByRole('dialog', { name: 'Reachability' });
    await drawer.getByRole('combobox', { name: 'From' }).selectOption('PC1');
    await drawer.getByRole('combobox', { name: 'To' }).selectOption('PC2');
    await drawer.getByRole('button', { name: 'Check this pair' }).click();

    await expect(drawer.getByText('PC1 reaches PC2')).toBeVisible({ timeout: 30_000 });
    // The path is the point: every hop, with what that device did and why.
    const hops = drawer.getByRole('list').first().getByRole('listitem');
    await expect(hops).toHaveCount(5);
    await expect(hops.nth(2)).toContainText('R1');
    await expect(hops.nth(2)).toContainText('routing table');

    // The matrix agrees, and the simulator's percentage is shown as it is.
    await drawer.getByRole('button', { name: 'Check every pair' }).click();
    await expect(drawer.getByText(/of the \d+ ordered pairs get through/)).toBeVisible({
      timeout: 30_000,
    });
    await drawer.getByRole('button', { name: 'PC1 to PC2: reachable' }).click();
    await expect(drawer.getByText('PC1 → PC2:')).toBeVisible();
    await page.keyboard.press('Escape');

    expect(consoleWatch.messages, backend.log()).toEqual([]);
  });

  test('comes back as the backend has it after the window is closed and reopened', async ({
    page,
    request,
  }) => {
    const consoleWatch = watchConsole(page);
    expect(labId, 'the lab from the previous test').not.toBe('');

    await openLab(page, labId);

    // The canvas, the running state and the consoles are all read back: this
    // window has never seen this lab before.
    await expect(page.getByText('Running')).toBeVisible({ timeout: 30_000 });
    await expect(node(page, 'R1')).toBeVisible();
    await expect(node(page, 'PC2')).toBeVisible();
    await expect(page.getByRole('tab', { name: /R1/ })).toBeVisible({ timeout: 30_000 });
    await expect(page.getByRole('tab', { name: /PC1/ })).toBeVisible();

    // And the console is the same session, with everything it had printed.
    await page.getByRole('tab', { name: /R1/ }).click();
    await expectOnScreen(page, '10.0.1.1');

    // Still live, not a picture of a session.
    await typeConsole(page, 'show ip route connected');
    await expectOnScreen(page, '10.0.2.0');

    // A node the user drags stays where it was put, in the backend.
    const rpc = rpcFor(request, backend);
    const before = await rpc<{ snapshot: { nodes: { name: string; x: number; y: number }[] } }>(
      'labs.get',
      { id: labId },
    );
    const pc1 = before.snapshot.nodes.find((item) => item.name === 'PC1');
    expect(pc1).toBeDefined();

    await node(page, 'PC1').hover();
    await page.mouse.down();
    await page.mouse.move(0, 0, { steps: 2 });
    const box = await node(page, 'PC1').boundingBox();
    await page.mouse.move((box?.x ?? 0) + 120, (box?.y ?? 0) + 60, { steps: 8 });
    await page.mouse.up();

    await expect
      .poll(
        async () => {
          const after = await rpc<{ snapshot: { nodes: { name: string; x: number }[] } }>(
            'labs.get',
            { id: labId },
          );
          return after.snapshot.nodes.find((item) => item.name === 'PC1')?.x;
        },
        { message: 'the new position reaching the backend', timeout: 15_000 },
      )
      .not.toBe(pc1?.x);

    expect(consoleWatch.messages, backend.log()).toEqual([]);
  });

  test('refuses a cable the simulator cannot make, in the simulator’s own words', async ({
    page,
  }) => {
    const consoleWatch = watchConsole(page);
    expect(labId, 'the lab from the previous test').not.toBe('');
    await openLab(page, labId);

    // A PC has one port and it is cabled: there is nowhere to put another.
    const tool = page.getByRole('button', { name: 'Cable', exact: true });
    if ((await tool.getAttribute('aria-pressed')) !== 'true') await tool.click();
    await node(page, 'PC1').click();
    await expect(page.getByText(/PC1 has no free interface/)).toBeVisible();

    // And a cable from a node to itself is not a cable.
    await node(page, 'R1').click();
    await node(page, 'R1').click();
    await expect(page.getByText('A cable needs two different nodes.')).toBeVisible();
    await page.keyboard.press('Escape');

    expect(consoleWatch.messages, backend.log()).toEqual([]);
  });

  test('shows the device tables the simulator keeps', async ({ page }) => {
    const consoleWatch = watchConsole(page);
    await openLab(page, labId);

    await node(page, 'R1').click();
    await page.getByRole('button', { name: 'Tables' }).click();
    const drawer = page.getByRole('dialog', { name: 'R1 tables' });
    await expect(drawer.getByRole('table', { name: 'Routing table' })).toBeVisible({
      timeout: 30_000,
    });
    await expect(drawer.getByRole('cell', { name: '10.0.1.0/24' })).toBeVisible();

    await drawer.getByRole('tab', { name: 'ARP' }).click();
    await expect(drawer.getByRole('table', { name: 'ARP table' })).toBeVisible();
    await page.keyboard.press('Escape');

    expect(consoleWatch.messages, backend.log()).toEqual([]);
  });
});

test.describe('the twin', () => {
  test('says what it cannot copy instead of copying it badly', async ({ page, request }) => {
    const consoleWatch = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await rpc('devices.create', {
      name: 'core-sw1',
      address: '10.9.9.9',
      profile: 'cisco_ios',
      site: 'HQ',
    });

    await openApp(page, '/lab', { backend });
    await page.getByRole('button', { name: 'Twin the real network…' }).click();
    const drawer = page.getByRole('dialog', { name: 'Build a lab from the real network' });

    // Simulated devices are not offered: twinning one is twinning a twin.
    await expect(drawer.getByText('core-sw1')).toBeVisible();
    await expect(drawer.getByText('Two sites/R1')).toHaveCount(0);

    await drawer.getByRole('checkbox').first().check();
    await drawer.getByRole('button', { name: 'Build the twin' }).click();

    // Nothing has been collected from this device, so nothing is invented:
    // the backend's own reason is what the user reads.
    await expect(drawer.getByText('No twin was built')).toBeVisible({ timeout: 60_000 });
    await expect(drawer.getByText(/collect from them and back them up first/)).toBeVisible();
    await expect(drawer.getByText(/no interfaces have been collected/)).toBeVisible();

    await page.keyboard.press('Escape');
    expect(consoleWatch.messages, backend.log()).toEqual([]);
  });
});
