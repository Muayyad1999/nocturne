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
 * The two QoS calculators add a third, of the same family: **a figure is the
 * engine's, and so is a refusal.** Their pickers are the engine's reference
 * tables, the expected figures here are worked out by hand in a comment
 * beside each one, and a refusal is asserted where it has to be read: beside
 * the field it is about, once, with no stale figure left under it.
 *
 * Every test asserts the browser console stayed clean, except the one that
 * cuts a read on the wire to see what the screen says of a read that failed.
 */

import { execFile } from 'node:child_process';
import { createServer, type Server } from 'node:net';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page, type Route } from '@playwright/test';
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

/**
 * The value shown against a label in a tool's answer. A label can stand in
 * two blocks (a figure, and its estimate with VAD), so this is every match.
 */
const shown = (page: Page, label: string): Locator =>
  panel(page).locator(`xpath=.//dt[normalize-space()="${label}"]/following-sibling::dd[1]`);

/**
 * Counts the calls the page makes to one RPC method from here on. "Nothing is
 * asked until the form can be answered" is a statement about the wire: the
 * panel still says "Nothing asked yet" for a moment after any keystroke, so
 * what is on screen cannot settle it.
 */
function callsTo(page: Page, method: string): () => number {
  let count = 0;
  page.on('request', (request) => {
    if (!request.url().endsWith('/api/rpc')) return;
    if ((request.postData() ?? '').includes(`"method":"${method}"`)) count += 1;
  });
  return () => count;
}

/** Longer than a calculator waits after a keystroke before it asks the engine. */
const SETTLED_MS = 700;

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
    // 53 methods in the engine's catalogue, eight of them folded: 45 rows.
    const rows = drawer(page).getByRole('button');
    await expect(rows).toHaveCount(45);
    for (const title of [
      'Subnet calculator',
      'QoS marking',
      'Voice bandwidth',
      'Ping Probe',
      'TFTP server Server',
      'What was run Record',
    ]) {
      await expect(drawer(page).getByRole('button', { name: title, exact: true })).toBeVisible();
    }
    // The engine's `qos` group is a shelf with a name, not an identifier.
    await expect(drawer(page).getByRole('heading', { name: 'Quality of service' })).toBeVisible();
    await expect(drawer(page).getByRole('button', { name: /^Stop the TFTP server/ })).toHaveCount(0);

    // Nothing in the catalogue is left without a form on this screen.
    await expect(page.getByText('No form for this tool yet')).toHaveCount(0);
    // Looked at, not assumed: every row of the drawer is opened in turn. A
    // tool the engine gains without a form here stops this test at its name.
    for (let index = 0; index < 45; index += 1) {
      const row = rows.nth(index);
      const title = await row.locator('span').first().innerText();
      await row.click();
      await expect(row, title).toHaveAttribute('aria-current', 'true');
      await expect(panel(page).getByRole('heading', { level: 2 }), title).toHaveText(title);
      await expect(page.getByText('No form for this tool yet'), title).toHaveCount(0);
    }

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

// ------------------------------------------------------- the QoS calculators

test.describe('a QoS calculator', () => {
  test('writes one marking in every form, and refuses beside the value what the form chosen cannot hold', async ({
    page,
  }) => {
    const console = watchConsole(page);
    await openApp(page, '/tools/tools.qos.marking', { backend });
    await expect(page.getByText('No form for this tool yet')).toHaveCount(0);
    const value = panel(page).getByLabel('Marking', { exact: true });
    const as = panel(page).getByLabel('What the value is', { exact: true });

    // A value alone is not a question: 46 is EF as a DSCP and something else
    // as a ToS byte, so nothing is chosen for the reader and nothing is asked.
    const asked = callsTo(page, 'tools.qos.marking');
    await value.fill('af41');
    await expect(as).toHaveValue('');
    await page.waitForTimeout(SETTLED_MS);
    await expect(panel(page).getByText('Nothing asked yet')).toBeVisible();
    expect(asked()).toBe(0);

    // AF41 is class 4, drop precedence 1: 100 01 0 = 34, and with two ECN
    // bits clear behind it the byte is 1000 1000 = 136 = 0x88.
    await as.selectOption('dscp');
    for (const [label, text] of [
      ['Name', 'AF41'],
      ['DSCP', '34'],
      ['DSCP, binary', '100010'],
      ['DSCP, hex', '0x22'],
      ['ToS byte', '136'],
      ['ToS byte, binary', '10001000'],
      ['ToS byte, hex', '0x88'],
      ['IP precedence', '4 — Flash Override'],
      ['ECN bits', '00 (0) — Not-ECT'],
      ['Per-hop behaviour', 'Assured Forwarding class 4'],
      ['Drop precedence', 'low'],
      ['Defined in', 'RFC 2597'],
      ['Typically for', 'Multimedia conferencing: interactive video'],
    ] as const) {
      await expect(shown(page, label), label).toHaveText(text);
    }

    // 184 does not fit in six bits. The engine says so, and its sentence is
    // beside the value — once — with nothing of the last answer left under it.
    await value.fill('184');
    await expect(value).toHaveAttribute('aria-invalid', 'true');
    await expect(value).toHaveAccessibleDescription(/invalid DSCP `184`: a DSCP is six bits, 0 to 63/);
    await expect(panel(page).getByText('a DSCP is six bits', { exact: false })).toHaveCount(1);
    await expect(
      panel(page).getByText('The engine refused what is in “Marking”', { exact: false }),
    ).toBeVisible();
    await expect(shown(page, 'DSCP')).toHaveCount(0);
    await expect(as).not.toHaveAttribute('aria-invalid', 'true');

    // The sentence says what to do about it: the whole byte is a `tos`.
    // 184 = 1011 1000: DSCP 101110 = 46, which is EF.
    await as.selectOption('tos');
    await expect(shown(page, 'Name')).toHaveText('EF');
    await expect(shown(page, 'DSCP')).toHaveText('46');
    await expect(shown(page, 'ToS byte, hex')).toHaveText('0xB8');
    await expect(shown(page, 'Defined in')).toHaveText('RFC 3246');
    await expect(shown(page, 'Drop precedence')).toHaveCount(0);
    await expect(value).not.toHaveAttribute('aria-invalid', 'true');

    // A value the registry has not named says so; it is not given a name, a
    // use or a source it does not have.
    await value.fill('47');
    await as.selectOption('dscp');
    await expect(shown(page, 'Name')).toContainText('the registry has not named this value');
    await expect(shown(page, 'Typically for')).toContainText('no standard use');
    await expect(shown(page, 'Per-hop behaviour')).toContainText('pool 2');
    await expect(shown(page, 'Defined in')).toHaveCount(0);

    // A ToS byte keeps its two ECN bits, and they are read out: 185 is EF
    // (1011 10) with 01 behind it, which is ECT(1) — the field's value 1.
    await value.fill('185');
    await as.selectOption('tos');
    await expect(shown(page, 'ECN bits')).toHaveText('01 (1) — ECT(1)');
    await expect(shown(page, 'What ECN says')).toContainText('L4S');
    await expect(shown(page, 'Name')).toHaveText('EF');

    // Each form refuses what it cannot hold, in the engine's words, beside
    // the value — and what was typed comes back as text, whatever it was.
    for (const [form, wrong, says] of [
      ['name', '46', /invalid DSCP name `46`: that is a number, not a name/],
      ['tos', '300', /invalid ToS byte `300`: the byte is eight bits, 0 to 255/],
      ['precedence', '9', /invalid IP precedence `9`: IP precedence is three bits, 0 to 7/],
      ['dscp', '<img src=x onerror=alert(1)>', /invalid DSCP `<img src=x onerror=alert\(1\)>`: neither a number/],
    ] as const) {
      await as.selectOption(form);
      await value.fill(wrong);
      await expect(value, wrong).toHaveAttribute('aria-invalid', 'true');
      await expect(value, wrong).toHaveAccessibleDescription(says);
      await expect(panel(page).getByText('The engine refused what is in “Marking”', { exact: false })).toBeVisible();
      await expect(shown(page, 'DSCP'), wrong).toHaveCount(0);
    }
    await expect(panel(page).locator('img')).toHaveCount(0);

    // The DSCP reference table leads here, with the row's value, rather than
    // growing these columns itself.
    await goTo(page, '/tools/tools.reference');
    await panel(page).getByLabel('Table', { exact: true }).selectOption('dscp');
    const row = panel(page)
      .getByRole('table', { name: 'DSCP and IP precedence' })
      .getByRole('row', { name: /^CS3\b/ });
    await row.click();
    await row.press('Enter');
    await expect(page).toHaveURL(/tools\.qos\.marking$/);
    await expect(value).toHaveValue('24');
    await expect(as).toHaveValue('dscp');
    await expect(shown(page, 'Name')).toHaveText('CS3');
    await expect(shown(page, 'Per-hop behaviour')).toHaveText('Class Selector 3');

    expect(console.messages).toEqual([]);
  });

  test('prices voice calls from the engine’s own tables, with its arithmetic, its sources and its refusals', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await openApp(page, '/tools/tools.qos.voice', { backend });
    await expect(page.getByText('No form for this tool yet')).toHaveCount(0);
    const codec = panel(page).getByLabel('Codec', { exact: true });
    const link = panel(page).getByLabel('Link type', { exact: true });
    const steps = panel(page).locator('ol > li');
    const values = (select: Locator) =>
      select.locator('option').evaluateAll((options) => options.map((option) => (option as HTMLOptionElement).value));

    // The pickers are the engine's two reference tables, in its order, after
    // "nothing chosen yet"; `custom` is the one choice that is not a row.
    const codecs = await rpc<{ rows: { id: string }[] }>('tools.reference', { table: 'voice_codecs' });
    const links = await rpc<{ rows: { id: string }[] }>('tools.reference', { table: 'voice_links' });
    expect(codecs.rows.length).toBeGreaterThan(5);
    expect(links.rows.length).toBeGreaterThan(5);
    await expect(codec.locator('option')).toHaveCount(codecs.rows.length + 1);
    expect(await values(codec)).toEqual(['', ...codecs.rows.map((entry) => entry.id)]);
    expect(await values(link)).toEqual(['', ...links.rows.map((entry) => entry.id), 'custom']);

    // Nothing is chosen for the reader, least of all the number of calls.
    await expect(codec).toHaveValue('');
    await expect(link).toHaveValue('');
    const asked = callsTo(page, 'tools.qos.voice');
    await codec.selectOption('g729');
    await link.selectOption('mlppp');
    await page.waitForTimeout(SETTLED_MS);
    await expect(panel(page).getByText('Nothing asked yet')).toBeVisible();
    expect(asked()).toBe(0);
    await field(page, 'Simultaneous calls', '10');

    // G.729 puts 20 bytes in a packet every 20 ms. Over Multilink PPP that is
    // 6 + 1 of layer 2 and 40 of IP/UDP/RTP: 67 bytes × 8 × 50 = 26 800 bps.
    await expect(shown(page, 'Per call')).toHaveText('26.8 kbps (26 800 bps)');
    await expect(shown(page, 'Total')).toHaveText('268 kbps (268 000 bps) for 10 calls');
    await expect(shown(page, 'Compressed RTP')).toHaveText('Not counted');

    // With compressed RTP the 40 bytes are 2: 29 bytes × 8 × 50 = 11 600 bps.
    await panel(page).getByRole('switch', { name: 'Compressed RTP (cRTP) on the link' }).check();
    await expect(shown(page, 'Per call')).toHaveText('11.6 kbps (11 600 bps)');
    await expect(shown(page, 'Total')).toHaveText('116 kbps (116 000 bps) for 10 calls');
    await expect(shown(page, 'Each packet')).toHaveText('29 bytes, 50 of them a second');
    await expect(shown(page, 'Link')).toHaveText('Multilink PPP: 7 bytes (MP header 6 + End-of-frame flag 1)');

    // The arithmetic is the engine's, line by line and in its order.
    await expect(steps).toHaveText([
      'Voice payload: 20 ms ÷ 10 ms per sample × 10 bytes = 20 bytes',
      'Packet: 6 (MP header) + 1 (End-of-frame flag) + 2 (cRTP-compressed IP/UDP/RTP header) + 20 (Voice payload) = 29 bytes',
      'Packet rate: 1000 ms ÷ 20 ms = 50 packets per second',
      'Per call: 29 bytes × 8 bits × 50 packets per second = 11600 bps',
      'Total: 10 × 11600 bps = 116000 bps',
    ]);
    await expect(
      panel(page).getByRole('table', { name: 'Where the bytes go' }).getByRole('row'),
    ).toHaveCount(5);
    // And so are the sources: the document every figure was copied from.
    await expect(panel(page).getByRole('heading', { name: 'Where the figures come from' })).toBeVisible();
    await expect(panel(page).getByText('Document ID 7934', { exact: false })).toBeVisible();

    // Voice-activity detection is an estimate beside the full figure, never
    // in its place: 65 % of 11 600 is 7 540, and ten calls are too few for it.
    await panel(page).getByRole('switch', { name: 'Also estimate voice-activity detection (VAD)' }).check();
    await expect(shown(page, 'Per call')).toHaveCount(2);
    await expect(shown(page, 'Per call').first()).toHaveText('11.6 kbps (11 600 bps)');
    await expect(shown(page, 'Per call').nth(1)).toHaveText('7.54 kbps (7 540 bps)');
    await expect(panel(page).getByText('with 10 it must not be counted on', { exact: false })).toBeVisible();
    await expect(steps).toHaveCount(6);

    // Compressed RTP on Ethernet is refused by the engine, and its sentence
    // is beside the link — once — with no figure left on screen.
    await link.selectOption('ethernet');
    await expect(link).toHaveAttribute('aria-invalid', 'true');
    await expect(link).toHaveAccessibleDescription(
      /invalid link type `ethernet`: compressed RTP is not counted on Ethernet/,
    );
    await expect(
      panel(page).getByText('compressed RTP is not counted on Ethernet', { exact: false }),
    ).toHaveCount(1);
    await expect(
      panel(page).getByText('The engine refused what is in “Link type”', { exact: false }),
    ).toBeVisible();
    await expect(shown(page, 'Per call')).toHaveCount(0);
    await expect(steps).toHaveCount(0);
    await expect(codec).not.toHaveAttribute('aria-invalid', 'true');

    // The way out the engine names: a custom link, which waits for its bytes.
    // 10 + 2 + 20 = 32 bytes × 8 × 50 = 12 800 bps.
    const before = asked();
    await link.selectOption('custom');
    await expect(link).not.toHaveAttribute('aria-invalid', 'true');
    await expect(panel(page).getByText('Nothing asked yet')).toBeVisible();
    await page.waitForTimeout(SETTLED_MS);
    expect(asked()).toBe(before);
    await field(page, 'Layer-2 bytes on every packet', '10');
    await expect(shown(page, 'Per call').first()).toHaveText('12.8 kbps (12 800 bps)');
    await expect(shown(page, 'Link')).toHaveText('Custom layer 2: 10 bytes (Layer 2, as given 10)');

    // More than a link header can be: refused beside the bytes, not the link.
    await field(page, 'Layer-2 bytes on every packet', '300');
    const bytes = panel(page).getByLabel('Layer-2 bytes on every packet', { exact: true });
    await expect(bytes).toHaveAttribute('aria-invalid', 'true');
    await expect(bytes).toHaveAccessibleDescription(/more than 256 bytes in front of every packet/);
    await expect(link).not.toHaveAttribute('aria-invalid', 'true');

    // The engine's other refusals each land beside the input they are about,
    // with no figure under them; put right, the answer is back.
    await link.selectOption('ppp');
    await expect(shown(page, 'Per call').first()).toHaveText('11.2 kbps (11 200 bps)');
    for (const [name, wrong, says, right] of [
      ['Simultaneous calls', '0', /invalid number of calls `0`: there has to be at least one call/, '10'],
      ['Simultaneous calls', '1000001', /1000001 simultaneous calls is more than this calculator works out/, '10'],
      ['Voice per packet (ms)', '25', /invalid voice per packet `25 ms`: G\.729 produces 10 bytes every 10 ms/, ''],
      ['Voice per packet (ms)', '0', /invalid voice per packet `0 ms`: a packet has to hold some voice/, ''],
      ['Voice per packet (ms)', '250', /invalid voice per packet `250 ms`: more than 240 ms/, ''],
    ] as const) {
      const input = panel(page).getByLabel(name, { exact: true });
      await input.fill(wrong);
      await expect(input, wrong).toHaveAttribute('aria-invalid', 'true');
      await expect(input, wrong).toHaveAccessibleDescription(says);
      await expect(panel(page).locator('[aria-invalid="true"]'), wrong).toHaveCount(1);
      await expect(shown(page, 'Per call'), wrong).toHaveCount(0);
      await input.fill(right);
      await expect(input, wrong).not.toHaveAttribute('aria-invalid', 'true');
      await expect(shown(page, 'Per call').first(), wrong).toHaveText('11.2 kbps (11 200 bps)');
    }

    // A number the method's type cannot hold is refused before the engine
    // sees it, in a sentence that names no input. It is shown whole where the
    // answer would be: not blank, not beside a field guessed at, and with no
    // figure left over from the calculation before it.
    await field(page, 'Simultaneous calls', '-3');
    await expect(panel(page).getByText('That input will not do')).toBeVisible();
    await expect(
      panel(page).getByText('invalid parameters for `tools.qos.voice`', { exact: false }),
    ).toHaveCount(1);
    await expect(panel(page).getByText('integer `-3`', { exact: false })).toBeVisible();
    await expect(panel(page).locator('[aria-invalid="true"]')).toHaveCount(0);
    await expect(shown(page, 'Per call')).toHaveCount(0);
    await field(page, 'Simultaneous calls', '10');
    await expect(shown(page, 'Per call').first()).toHaveText('11.2 kbps (11 200 bps)');

    // A row of the codec table leads here with that codec chosen — through
    // the row's menu this time — and what the form remembered is kept.
    await goTo(page, '/tools/tools.reference');
    await panel(page).getByLabel('Table', { exact: true }).selectOption('voice_codecs');
    const g711 = panel(page).getByRole('table', { name: 'Voice codecs' }).getByRole('row', { name: /^g711\b/ });
    await g711.getByRole('button', { name: 'Row actions' }).click();
    await page.getByRole('menuitem', { name: 'Open in Voice bandwidth' }).click();
    await expect(page).toHaveURL(/tools\.qos\.voice$/);
    await expect(codec).toHaveValue('g711');
    await expect(link).toHaveValue('ppp');
    await expect(panel(page).getByLabel('Simultaneous calls', { exact: true })).toHaveValue('10');
    // G.711 has 160 bytes in a packet; with PPP's 6 and cRTP's 2 that is
    // 168 bytes × 8 × 50 = 67 200 bps.
    await expect(shown(page, 'Per call').first()).toHaveText('67.2 kbps (67 200 bps)');

    expect(console.messages).toEqual([]);
  });

  test('says that the engine’s table of codecs could not be read, and offers nothing in its place', async ({
    page,
  }) => {
    // The one read is cut on the wire. No answer is made up for it: the
    // browser reports the failure, which is why this test, alone in the file,
    // does not ask for a silent console.
    const cut = async (route: Route) => {
      if ((route.request().postData() ?? '').includes('"voice_codecs"')) await route.abort('connectionfailed');
      else await route.continue();
    };
    await page.route('**/api/rpc', cut);
    const asked = callsTo(page, 'tools.qos.voice');
    await openApp(page, '/tools/tools.qos.voice', { backend });
    const codec = panel(page).getByLabel('Codec', { exact: true });
    const link = panel(page).getByLabel('Link type', { exact: true });

    // Not "still reading", and not a list of codecs kept here for the day
    // the engine's cannot be had: that list could disagree with the engine.
    await expect(codec).toHaveAccessibleDescription(
      /The engine’s table of these could not be read, so there is nothing to choose from: \S/,
    );
    await expect(codec).toBeDisabled();
    await expect(codec.locator('option')).toHaveText(['Nothing to choose from']);

    // The other picker is the engine's as ever, and without a codec nothing
    // is asked however much else is filled in.
    await expect(link.locator('option')).not.toHaveCount(1);
    await link.selectOption('ppp');
    await field(page, 'Simultaneous calls', '10');
    await page.waitForTimeout(SETTLED_MS);
    await expect(panel(page).getByText('Nothing asked yet')).toBeVisible();
    expect(asked()).toBe(0);

    // A form that already holds a codec is another matter: that codec is
    // still asked about, so it is still what the list shows. The table is
    // read this once, a codec chosen, and then the read is cut again.
    await page.unroute('**/api/rpc', cut);
    await openApp(page, '/tools/tools.qos.voice', { backend });
    await codec.selectOption('g729');
    // 6 of PPP + 40 of IP/UDP/RTP + 20 of voice: 66 bytes × 8 × 50 = 26 400 bps.
    await expect(shown(page, 'Per call')).toHaveText('26.4 kbps (26 400 bps)');
    await page.route('**/api/rpc', cut);
    await openApp(page, '/tools/tools.qos.voice', { backend });
    await expect(codec).toHaveAccessibleDescription(/could not be read, so there is nothing to choose from/);
    await expect(codec).toBeDisabled();
    // Not "Nothing to choose from" over an answer for G.729: the answer on
    // screen and the list above it name the same codec.
    await expect(codec).toHaveValue('g729');
    await expect(codec.locator('option')).toHaveText(['g729']);
    await expect(shown(page, 'Codec')).toHaveText(/^G\.729: /);
    await expect(shown(page, 'Per call')).toHaveText('26.4 kbps (26 400 bps)');
  });

  test('is filled and answered from the keyboard alone, with the focus in sight', async ({ page }) => {
    const console = watchConsole(page);
    /** Tab until `target` has the focus: the only way in is the way a keyboard gets there. */
    const tabTo = async (target: Locator) => {
      for (let presses = 0; presses < 200; presses += 1) {
        if (await target.evaluate((element) => element === document.activeElement)) return;
        await page.keyboard.press('Tab');
      }
      throw new Error('the field was never reached with Tab');
    };
    /** The focus ring is drawn on the control's frame, or on a switch's track. */
    const ringed = (target: Locator) =>
      target.evaluate((element) => {
        const frame = element.closest('[data-control]') ?? element.nextElementSibling ?? element;
        return [element, frame].some((one) => getComputedStyle(one).outlineStyle !== 'none');
      });

    // One marking. A native list moves through its choices with the arrows.
    await openApp(page, '/tools/tools.qos.marking', { backend });
    const value = panel(page).getByLabel('Marking', { exact: true });
    const as = panel(page).getByLabel('What the value is', { exact: true });
    await tabTo(value);
    expect(await ringed(value)).toBe(true);
    await page.keyboard.type('ef');
    await page.keyboard.press('Tab');
    await expect(as).toBeFocused();
    expect(await ringed(as)).toBe(true);
    await page.keyboard.press('ArrowDown');
    await expect(as).toHaveValue('dscp');
    await expect(shown(page, 'DSCP')).toHaveText('46');
    await expect(shown(page, 'ToS byte, hex')).toHaveText('0xB8');

    // Voice calls: every field in the order Tab reaches them, a list by the
    // first letters of a choice, the two switches with Space. 10 × G.729 over PPP with cRTP is 28 bytes a packet:
    // 28 × 8 × 50 = 11 200 bps.
    await openApp(page, '/tools/tools.qos.voice', { backend });
    const codec = panel(page).getByLabel('Codec', { exact: true });
    const calls = panel(page).getByLabel('Simultaneous calls', { exact: true });
    const link = panel(page).getByLabel('Link type', { exact: true });
    const crtp = panel(page).getByRole('switch', { name: 'Compressed RTP (cRTP) on the link' });
    const vad = panel(page).getByRole('switch', { name: 'Also estimate voice-activity detection (VAD)' });
    await expect(codec.locator('option')).not.toHaveCount(1);
    await tabTo(codec);
    await page.keyboard.type('G.729');
    await expect(codec).toHaveValue('g729');
    await page.keyboard.press('Tab');
    await expect(calls).toBeFocused();
    await page.keyboard.type('10');
    await page.keyboard.press('Tab');
    await expect(link).toBeFocused();
    await page.keyboard.type('PPP');
    await expect(link).toHaveValue('ppp');
    await expect(shown(page, 'Per call')).toHaveText('26.4 kbps (26 400 bps)');
    await page.keyboard.press('Tab');
    await expect(panel(page).getByLabel('Voice per packet (ms)', { exact: true })).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(crtp).toBeFocused();
    expect(await ringed(crtp)).toBe(true);
    await page.keyboard.press('Space');
    await expect(crtp).toBeChecked();
    await expect(shown(page, 'Per call')).toHaveText('11.2 kbps (11 200 bps)');
    await page.keyboard.press('Tab');
    await expect(vad).toBeFocused();
    await page.keyboard.press('Space');
    await expect(shown(page, 'Per call')).toHaveCount(2);

    // A refusal is reached the same way: back to the link, and Ethernet.
    await page.keyboard.press('Shift+Tab');
    await page.keyboard.press('Shift+Tab');
    await page.keyboard.press('Shift+Tab');
    await expect(link).toBeFocused();
    await page.keyboard.press('ArrowUp');
    await page.keyboard.press('ArrowUp');
    await expect(link).toHaveValue('ethernet');
    await expect(link).toHaveAttribute('aria-invalid', 'true');
    await expect(link).toHaveAccessibleDescription(/compressed RTP is not counted on Ethernet/);
    await expect(link).toBeFocused();

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
