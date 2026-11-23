/**
 * `pnpm shots` — the Logs tab (Monitoring → Logs), every pane, photographed in
 * both themes, at the two sizes that matter.
 *
 * Every row in these pictures is something a real receiver took off a socket
 * or the lab handed over, and every count, gap and coverage line was computed
 * from it by the engine:
 *
 * * **syslog** — lines in the shapes a Cisco switch puts on the wire, sent
 *   over UDP in bursts a few seconds apart, so the histogram has a shape;
 * * **traps** — the datagrams net-snmp 5.9.4's `snmptrap` sent, captured and
 *   replayed (the captures `crates/nocturne-logs/tests/traps.rs` replays: no
 *   switch sent them), one of them with the wrong community;
 * * **flows** — NetFlow v5 and v9 datagrams **written by hand** to Cisco's v5
 *   format and RFC 3954, one exporter sampling 1 in 100 and one no device
 *   claims. No exporter sent them;
 * * **a lab switch** configured from its own console, which is a lab line, a
 *   rule's signal and a detection's hit;
 * * **forwarders** — one to a TCP listener in this process, one to an HTTPS
 *   collector that is not there.
 *
 * The trap captures and the v5 builder are duplicated from `logs.spec.ts` on
 * purpose: one Playwright file may not import another (its `test(...)` calls
 * would be registered twice). See `src/pages/logs/NOTES.md`.
 */

import { createSocket } from 'node:dgram';
import { execFile } from 'node:child_process';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { createServer, type Server, type Socket as Connection } from 'node:net';
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
const SHOTS = resolve(fileURLToPath(new URL('./shots/logs', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

/** The ports the receivers are told to open. High, so they need no privileges. */
const PORT = 25_000 + Math.floor(Math.random() * 900);
const TRAP_PORT = 26_000 + Math.floor(Math.random() * 900);
const FLOW_PORT = 27_000 + Math.floor(Math.random() * 900);

/** A test value, stored for CORE1 through `credentials.create`. It is on no screen. */
const COMMUNITY = 'n0cturne-ro';

const TAB = '/monitoring/logs';

test.describe.configure({ mode: 'serial', timeout: 900_000 });

let backend: Backend;
let uiDir: string;
let collector: Server | undefined;
/** The collector's connections: a server only finishes closing once they have. */
const connections = new Set<Connection>();
let pulledAt = 0;

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
    `<188>${index}: *Nov 14 00:31:0${index % 10}.001: %SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: 10.1.1.4${index % 10}] [localport: 22] [Reason: Login Authentication Failed] at 00:31:0${index % 10} UTC Fri Nov 14 2026`,
  (index) =>
    `<186>${index}: *Nov 14 00:31:0${index % 10}.002: %SPANTREE-2-BLOCK_BPDUGUARD: Received BPDU on port GigabitEthernet1/0/${(index % 8) + 1} with BPDU Guard enabled, disabling port`,
  (index) =>
    `<191>${index}: *Nov 14 00:31:0${index % 10}.003: %CDP-4-DUPLEX_MISMATCH: duplex mismatch discovered on GigabitEthernet1/0/${(index % 8) + 1} (not half duplex), with SW9 FastEthernet0/2 (half duplex)`,
];

async function sendTo(port: number, datagrams: readonly (string | Buffer)[], from?: string): Promise<void> {
  const socket = createSocket('udp4');
  if (from) await new Promise<void>((done) => socket.bind({ address: from }, done));
  for (const [index, datagram] of datagrams.entries()) {
    socket.send(typeof datagram === 'string' ? Buffer.from(datagram) : datagram, port, '127.0.0.1');
    // Loopback has a finite buffer, and UDP may lose what it has no room for.
    if (index % 50 === 49) await new Promise((done) => setTimeout(done, 20));
  }
  await new Promise((done) => setTimeout(done, 150));
  socket.close();
}

function lines(count: number, offset: number): string[] {
  return Array.from({ length: count }, (_value, index) => SHAPES[(index + offset) % SHAPES.length]?.(index + offset) ?? '');
}

// ------------------------------------------------------------------ the traps

function hex(text: string): Buffer {
  return Buffer.from(text.replace(/\s+/g, ''), 'hex');
}

/**
 * Captured from net-snmp 5.9.4's `snmptrap -v 2c -c n0cturne-ro … linkDown
 * ifIndex.3 i 3 ifAdminStatus.3 i 1 ifOperStatus.3 i 2`. Not sent by a device.
 */
const V2C_LINK_DOWN = hex(`
  307c020101040b6e30637475726e652d726fa76a020460ca908d020100020100305c300e06082b060102
  01010300430230393017060a2b06010603010104010006092b0601060301010503300f060a2b06010201
  0202010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020102`);

/** The same linkDown sent with `-c gu3ssed-rw`: the wrong community. */
const V2C_LINK_DOWN_WRONG_COMMUNITY = hex(`
  307b020101040a677533737365642d7277a76a020457e98011020100020100305c300e06082b06010201
  010300430230393017060a2b06010603010104010006092b0601060301010503300f060a2b0601020102
  02010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020102`);

/** `snmptrap -v 1 …` — a v1 linkDown that claims the agent address 192.0.2.1. */
const V1_LINK_DOWN = hex(`
  3063020100040b6e30637475726e652d726fa451060a2b06010401bf0803020a4004c000020102010202
  0100430230393033300f060a2b060102010202010103020103300f060a2b060102010202010703020101
  300f060a2b060102010202010803020102`);

// ------------------------------------------------------------------ the flows

interface V5Record {
  src: [number, number, number, number];
  dst: [number, number, number, number];
  input: number;
  output: number;
  packets: number;
  bytes: number;
  srcPort: number;
  dstPort: number;
  protocol: number;
  tos: number;
}

/** A NetFlow v5 datagram, **written by hand** to Cisco's format. No exporter sent it. */
function v5(sampling: number, records: readonly V5Record[]): Buffer {
  const header = Buffer.alloc(24);
  header.writeUInt16BE(5, 0);
  header.writeUInt16BE(records.length, 2);
  header.writeUInt32BE(86_400_000, 4);
  header.writeUInt32BE(1_763_000_000, 8);
  header.writeUInt32BE(1, 16);
  header.writeUInt16BE(sampling, 22);
  const body = records.map((record) => {
    const out = Buffer.alloc(48);
    Buffer.from(record.src).copy(out, 0);
    Buffer.from(record.dst).copy(out, 4);
    out.writeUInt16BE(record.input, 12);
    out.writeUInt16BE(record.output, 14);
    out.writeUInt32BE(record.packets, 16);
    out.writeUInt32BE(record.bytes, 20);
    out.writeUInt32BE(80_000, 24);
    out.writeUInt32BE(81_000, 28);
    out.writeUInt16BE(record.srcPort, 32);
    out.writeUInt16BE(record.dstPort, 34);
    out[37] = 0x18;
    out[38] = record.protocol;
    out[39] = record.tos;
    return out;
  });
  return Buffer.concat([header, ...body]);
}

/** A v9 data set whose template never came, written by hand to RFC 3954. */
function v9DataWithoutTemplate(): Buffer {
  const header = Buffer.alloc(20);
  header.writeUInt16BE(9, 0);
  header.writeUInt16BE(1, 2);
  header.writeUInt32BE(86_400_000, 4);
  header.writeUInt32BE(1_763_000_000, 8);
  header.writeUInt32BE(1, 12);
  header.writeUInt32BE(7, 16);
  const content = Buffer.alloc(16);
  Buffer.from([10, 0, 0, 1, 192, 0, 2, 80]).copy(content, 0);
  content.writeUInt32BE(4_000, 8);
  content.writeUInt32BE(4, 12);
  const set = Buffer.alloc(4);
  set.writeUInt16BE(260, 0);
  set.writeUInt16BE(4 + content.length, 2);
  return Buffer.concat([header, set, content]);
}

/** Talkers of a small office: web, voice marked EF, DNS, a backup, SSH. */
const FLOWS: readonly V5Record[] = [
  { src: [10, 0, 0, 5], dst: [192, 0, 2, 80], input: 3, output: 5, packets: 220, bytes: 248_800, srcPort: 51_000, dstPort: 443, protocol: 6, tos: 0 },
  { src: [10, 0, 0, 7], dst: [10, 0, 0, 9], input: 3, output: 5, packets: 500, bytes: 94_000, srcPort: 16_384, dstPort: 16_384, protocol: 17, tos: 0xb8 },
  { src: [10, 0, 0, 5], dst: [192, 0, 2, 53], input: 3, output: 5, packets: 2, bytes: 180, srcPort: 40_000, dstPort: 53, protocol: 17, tos: 0 },
  { src: [10, 0, 1, 20], dst: [10, 0, 9, 2], input: 4, output: 5, packets: 1_400, bytes: 1_960_000, srcPort: 50_123, dstPort: 873, protocol: 6, tos: 0x28 },
  { src: [10, 0, 0, 12], dst: [10, 0, 0, 1], input: 3, output: 1, packets: 40, bytes: 6_800, srcPort: 53_000, dstPort: 22, protocol: 6, tos: 0x48 },
];

// ------------------------------------------------------------------ the seed

async function labSwitch(rpc: Rpc): Promise<string> {
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  return devices.find((device) => device.name === 'LogLab/SW1')?.id ?? '';
}

/** A real console session to the simulated switch, typed into. */
async function configureFromConsole(rpc: Rpc, deviceId: string, description: string): Promise<void> {
  const session = await rpc<{ id: string }>('terminal.open', { device_id: deviceId, cols: 120, rows: 30, log: false });
  for (const command of ['enable', 'configure terminal', 'interface FastEthernet0/1', `description ${description}`, 'end']) {
    await rpc('terminal.input', { session_id: session.id, data: `${command}\r` });
    await new Promise((done) => setTimeout(done, 300));
  }
  await rpc('terminal.close', { session_id: session.id });
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(900_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing to it would make cargo rebuild the whole binary first.
  uiDir = process.env.NOCTURNE_UI_DIST ?? (await mkdtemp(join(tmpdir(), 'nocturne-ui-')));
  if (process.env.NOCTURNE_UI_DIST === undefined) {
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 240_000,
    });
  }
  backend = await startBackend({ uiDir });
  const rpc = rpcFor(request, backend);

  const credential = await rpc<{ id: string }>('credentials.create', {
    name: 'Campus SNMP',
    username: 'netops',
    password: 'not-a-real-password',
    snmp_version: 'v2c',
    snmp_community: COMMUNITY,
  });
  await rpc('devices.create', {
    name: 'CORE1',
    address: '127.0.0.1',
    profile: 'cisco_ios',
    site: 'HQ',
    role: 'core',
    credential_id: credential.id,
  });
  await rpc('devices.create', { name: 'DIST2', address: '10.10.0.2', profile: 'cisco_ios', site: 'HQ', role: 'distribution' });
  await rpc('devices.create', { name: 'EDGE-FW', address: '10.10.0.9', profile: 'mikrotik_routeros', site: 'HQ', role: 'firewall' });
  await rpc('settings.set', {
    section: 'logs',
    value: {
      syslog: { enabled: false, udp_port: PORT, tcp_port: 0 },
      traps: { enabled: false, port: TRAP_PORT, bind: '127.0.0.1' },
      flows: { enabled: false, port: FLOW_PORT, bind: '127.0.0.1' },
    },
  });
  const lab = await rpc<{ id: string }>('labs.import', {
    spec: {
      name: 'LogLab',
      description: 'one switch, for its own log buffer and its console',
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
});

test.afterAll(async () => {
  await new Promise<void>((done) => {
    if (!collector) return done();
    collector.close(() => done());
    for (const connection of connections) connection.destroy();
  });
  await backend?.stop();
  if (uiDir && process.env.NOCTURNE_UI_DIST === undefined) await rm(uiDir, { recursive: true, force: true });
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

/** A pane is taller than the window: a taller window, so the picture holds all of it. */
async function tall(page: Page, height = 1_500): Promise<void> {
  await page.setViewportSize({ width: 1440, height });
}

async function normal(page: Page): Promise<void> {
  await page.setViewportSize({ width: 1440, height: 900 });
}

// Nothing listening yet: the state most installations start in, with the
// commands for a real device in it.
test('logs — nothing is listening yet', async ({ page }) => {
  for (const theme of THEMES) {
    await openApp(page, TAB, { backend, theme });
    await expect(page.getByRole('region', { name: 'Log reception' })).toContainText('switched off');
    await shoot(page, theme, 'not-configured');

    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, TAB, { backend, theme, dock: true });
    await expect(page.getByRole('region', { name: 'Log reception' })).toBeVisible();
    await shoot(page, theme, 'not-configured-1280-dock');

    await tall(page);
    await openApp(page, `${TAB}?pane=receivers`, { backend, theme });
    await expect(page.getByRole('region', { name: 'SNMP traps receiver' })).toContainText('Switched off');
    await shoot(page, theme, 'receivers-off');
    await normal(page);

    await openApp(page, `${TAB}?pane=forwarding`, { backend, theme });
    await expect(page.getByRole('heading', { name: 'Nothing is forwarded' })).toBeVisible();
    await shoot(page, theme, 'forwarding-empty');
  }
});

/**
 * The traffic these pictures are of. Lines in bursts a few seconds apart, so
 * the histogram has busy and quiet buckets and a stretch before anything was
 * stored, which is a gap and not a zero.
 */
test('logs — the traffic these pictures are of', async ({ request }) => {
  const rpc = rpcFor(request, backend);
  const lake = createServer((socket) => {
    connections.add(socket);
    socket.on('data', () => undefined);
    socket.on('error', () => undefined);
    socket.on('close', () => connections.delete(socket));
  });
  collector = lake;
  await new Promise<void>((done) => lake.listen(0, '127.0.0.1', done));
  const address = lake.address();
  const collectorPort = address !== null && typeof address === 'object' ? address.port : 0;

  await rpc('settings.set', {
    section: 'logs',
    value: { syslog: { enabled: true }, traps: { enabled: true }, flows: { enabled: true } },
  });
  await expect
    .poll(
      async () => {
        const status = await rpc<Record<'syslog' | 'traps' | 'flows', { listening: string[] }>>('logs.status');
        return [status.syslog, status.traps, status.flows].every((receiver) => receiver.listening.length > 0);
      },
      { timeout: 30_000 },
    )
    .toBe(true);

  // Two forwarders: one that a collector here reads, one whose collector is gone.
  await rpc('logs.forward.save', {
    name: 'SIEM',
    kind: 'syslog_tcp',
    host: '127.0.0.1',
    port: collectorPort,
    format: 'rfc5424',
    streams: ['syslog', 'trap', 'event'],
    lab: true,
  });
  await rpc('logs.forward.save', {
    name: 'Data lake',
    kind: 'https_json',
    url: 'https://127.0.0.1:9/services/collector/event',
    token: 'a-test-token-that-is-on-no-screen',
    streams: ['syslog', 'trap', 'event', 'audit'],
    lab: true,
  });
  // A rule of one's own, with a threshold and a quiet period.
  await rpc('logs.rules.save', {
    name: 'ACL denies',
    pattern: '%SEC-6-IPACCESSLOGP: list (?P<acl>\\S+) denied',
    threshold: { count: 20, window_s: 60, group_by: ['acl'] },
    clear_after_s: 600,
    emit: { kind: 'alert', severity: 'medium', title: '{acl} is denying a lot' },
    test_line: '%SEC-6-IPACCESSLOGP: list EDGE-IN denied tcp 10.0.0.5(1234) -> 10.0.9.9(22), 1 packet',
    test_line_source: 'Synthetic: written by hand for these pictures',
  });

  for (const [index, count] of [40, 12, 90, 5, 30].entries()) {
    await sendTo(PORT, lines(count, index * 7));
    if (index === 1) {
      await sendTo(TRAP_PORT, [V2C_LINK_DOWN, V1_LINK_DOWN, V2C_LINK_DOWN_WRONG_COMMUNITY, 'not a trap']);
      await sendTo(FLOW_PORT, [v5(0x4000 | 100, FLOWS), v9DataWithoutTemplate(), 'garbage']);
    }
    if (index === 3) await sendTo(FLOW_PORT, [v5(0, FLOWS.slice(0, 2))], '127.0.0.2');
    if (index < 4) await new Promise((done) => setTimeout(done, 22_000));
  }
  // One address nobody claims, which is the other half of attribution.
  await sendTo(PORT, lines(3, 61), '127.0.0.2');

  // A lab switch configured from its console: a lab line, a signal, a hit.
  await configureFromConsole(rpc, await labSwitch(rpc), 'uplink to the core');
  await expect
    .poll(async () => (await rpc<unknown[]>('detections.recent', { limit: 10 })).length, { timeout: 30_000 })
    .toBeGreaterThan(0);
  await expect
    .poll(
      async () =>
        (await rpc<{ forwarders: { state: string }[] }>('logs.status')).forwarders.map((entry) => entry.state).sort(),
      { timeout: 60_000 },
    )
    .toEqual(['failing', 'ok']);
});

for (const theme of THEMES) {
  test(`logs — the lines, ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    // The screen as a NOC leaves it open: the last quarter of an hour.
    await openApp(page, `${TAB}?span=15m`, { backend, theme });
    await expect(page.getByTestId('log-counts')).toContainText('lines in this range');
    // The list sits at its newest line once everything above it has loaded.
    await expect(page.getByRole('button', { name: 'Jump to the newest line' })).toHaveCount(0);
    await shoot(page, theme, 'lines');

    // What a severity filter hides, said out loud.
    await page.getByRole('combobox', { name: 'Severity' }).selectOption('4');
    await expect(page.getByTestId('log-counts')).toContainText('hidden by the severity filter');
    await shoot(page, theme, 'severity');
    await page.getByRole('combobox', { name: 'Severity' }).selectOption('');

    // One line in full, with both clocks.
    const list = page.getByRole('list', { name: 'Log lines' });
    const interesting = list.getByRole('button').filter({ hasText: 'Login failed' }).first();
    await ((await interesting.count()) > 0 ? interesting : list.getByRole('button').first()).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await shoot(page, theme, 'entry');
    await page.keyboard.press('Escape');

    // The live tail, following.
    await page.locator('label', { hasText: 'Live tail' }).click();
    await expect(page.getByText('Following the newest lines')).toBeVisible({ timeout: 30_000 });
    await shoot(page, theme, 'tail');
    await page.locator('label', { hasText: 'Live tail' }).click();

    // A week, where most of the picture is honestly unknown.
    await openApp(page, `${TAB}?span=7d`, { backend, theme });
    await expect(page.getByTestId('log-gaps')).toBeVisible();
    await shoot(page, theme, 'gaps');

    // The addresses no device claims.
    await openApp(page, `${TAB}?span=15m`, { backend, theme });
    await page.getByRole('button', { name: /address no device claims/ }).click();
    await expect(page.getByRole('table', { name: 'Addresses no device claims' })).toBeVisible();
    await shoot(page, theme, 'sources');

    // The trap stream: no severity, the notification by name.
    await openApp(page, `${TAB}?stream=trap&span=15m`, { backend, theme });
    await expect(list.getByRole('button').first()).toContainText('linkDown');
    await shoot(page, theme, 'traps');
    await list.getByRole('button').filter({ hasText: 'linkDown' }).last().click();
    await expect(page.getByRole('dialog', { name: 'linkDown' })).toBeVisible();
    await shoot(page, theme, 'trap-entry');
    await page.keyboard.press('Escape');

    // The narrow end of the range, with the dock open.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, `${TAB}?span=15m`, { backend, theme, dock: true });
    await expect(page.getByTestId('log-counts')).toBeVisible();
    await shoot(page, theme, 'lines-1280-dock');
    await normal(page);
  });

  test(`logs — receivers, rules and forwarding, ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    await tall(page);
    await openApp(page, `${TAB}?pane=receivers`, { backend, theme });
    const traps = page.getByRole('region', { name: 'SNMP traps receiver' });
    await expect(traps.getByRole('list', { name: 'Trap sources that were refused' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Flows receiver' })).toContainText('Listening');
    await shoot(page, theme, 'receivers');
    const setup = page.getByRole('region', { name: 'What a device has to be told' });
    await setup.getByRole('combobox', { name: 'Device to show the commands for' }).fill('CORE1');
    await setup.getByRole('combobox', { name: 'Device to show the commands for' }).press('Enter');
    await expect(setup.getByTestId('setup-traps')).toContainText('<snmp-community>');
    await setup.scrollIntoViewIfNeeded();
    await shoot(page, theme, 'receivers-setup');
    await normal(page);

    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, `${TAB}?pane=receivers`, { backend, theme, dock: true });
    await expect(page.getByRole('region', { name: 'Syslog receiver' })).toContainText('Listening');
    await shoot(page, theme, 'receivers-1280-dock');
    await normal(page);

    // The rules, and one built-in rule with the line it was tested against.
    await openApp(page, `${TAB}?pane=rules`, { backend, theme });
    await expect(page.getByRole('table', { name: 'Log rules' })).toContainText('Cisco IOS / IOS-XE');
    await shoot(page, theme, 'rules');
    await page.getByRole('searchbox', { name: 'Search the rules' }).fill('repeated failed logins');
    await page
      .getByRole('table', { name: 'Log rules' })
      .getByRole('row')
      .filter({ hasText: 'IOS: repeated failed logins from one address' })
      .getByRole('cell')
      .first()
      .dblclick();
    const builtin = page.getByRole('dialog', { name: 'IOS: repeated failed logins from one address' });
    await builtin.getByRole('button', { name: 'Test the rule' }).click();
    await expect(builtin.getByTestId('rule-test-result')).toBeVisible();
    await builtin.getByTestId('rule-test-result').scrollIntoViewIfNeeded();
    await shoot(page, theme, 'rule-builtin');
    await page.keyboard.press('Escape');

    // A rule of one's own, refused by the engine beside the field it is about.
    await page.getByRole('button', { name: 'New rule' }).click();
    const fresh = page.getByRole('dialog', { name: 'New rule' });
    await fresh.getByLabel('Name').fill('BGP flaps from one neighbour');
    await fresh.getByLabel('Pattern').fill('%BGP-5-ADJCHANGE: neighbor (?P<neighbor>\\S+) Down');
    await fresh.getByLabel('Alert title').fill('{neighbor} keeps going down');
    await fresh.getByRole('switch', { name: 'Raise only when several matching lines arrive inside a window' }).click();
    await fresh.getByLabel('Counted per').fill('peer');
    await fresh.getByLabel('Counted per').press('Enter');
    await fresh.getByRole('button', { name: 'Save' }).click();
    await expect(fresh.getByText('the pattern has no group named `peer` to count by')).toBeVisible();
    await shoot(page, theme, 'rule-refused');
    await page.keyboard.press('Escape');

    // Forwarding: one sending, one failing, and what the failure was.
    await openApp(page, `${TAB}?pane=forwarding`, { backend, theme });
    await expect(page.getByRole('table', { name: 'Log forwarders' })).toContainText('Failing');
    await shoot(page, theme, 'forwarding');
    await page
      .getByRole('table', { name: 'Log forwarders' })
      .getByRole('row')
      .filter({ hasText: 'Data lake' })
      .getByRole('cell')
      .first()
      .dblclick();
    await expect(page.getByRole('dialog', { name: 'Edit Data lake' })).toContainText('Stored in the vault');
    await shoot(page, theme, 'forwarder-https');
    await page.keyboard.press('Escape');

    // The narrow end: whether a forwarder is failing stays in view.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, `${TAB}?pane=forwarding`, { backend, theme, dock: true });
    await expect(page.getByRole('table', { name: 'Log forwarders' })).toContainText('Failing');
    await shoot(page, theme, 'forwarding-1280-dock');
    await normal(page);
  });

  test(`logs — flows and detections, ${theme}`, async ({ page }) => {
    test.setTimeout(300_000);

    await tall(page, 1_600);
    await openApp(page, `${TAB}?pane=flows&period=60`, { backend, theme });
    await expect(page.getByRole('table', { name: 'Top talkers by source' })).toContainText('10.0.1.20');
    await shoot(page, theme, 'flows');
    await page.getByRole('combobox', { name: 'Add up by' }).selectOption('interface');
    await expect(page.getByRole('table', { name: 'Top talkers by interface' })).toContainText('ifIndex');
    await shoot(page, theme, 'flows-interface');
    await page.getByRole('combobox', { name: 'Add up by' }).selectOption('application');
    await expect(page.getByRole('table', { name: 'Top talkers by application' })).toContainText('tcp/443');
    await shoot(page, theme, 'flows-application');
    await normal(page);

    // The narrow end: the key column keeps what the numbers do not need.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, `${TAB}?pane=flows&period=60`, { backend, theme, dock: true });
    await expect(page.getByRole('table', { name: 'Top talkers by source' })).toContainText('10.0.1.20');
    await shoot(page, theme, 'flows-1280-dock');
    await normal(page);

    await tall(page);
    await openApp(page, `${TAB}?pane=detections`, { backend, theme });
    await expect(page.getByRole('table', { name: 'Recent detection hits' }).getByRole('row').nth(1)).toBeVisible({
      timeout: 30_000,
    });
    await shoot(page, theme, 'detections');
    await page.getByRole('region', { name: 'Recent hits' }).scrollIntoViewIfNeeded();
    await shoot(page, theme, 'detections-hits');
    await normal(page);
    await openApp(page, `${TAB}?pane=detections`, { backend, theme });
    await page
      .getByRole('table', { name: 'Recent detection hits' })
      .getByRole('row')
      .filter({ hasText: 'LogLab/SW1' })
      .first()
      .getByRole('cell')
      .first()
      .dblclick();
    await expect(page.getByRole('dialog', { name: 'Unplanned configuration change' })).toBeVisible();
    await shoot(page, theme, 'detection-hit');
    await page.keyboard.press('Escape');

    // Nothing on any of these screens holds the community.
    expect(await page.locator('body').innerText()).not.toContain(COMMUNITY);
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
  const sw1 = await labSwitch(rpc);
  // The switch may be busy with the console session of a moment ago.
  await expect
    .poll(
      async () => {
        try {
          pulledAt = (await rpc<{ newest_at: number }>('logs.pull', { device_id: sw1 })).newest_at;
          return pulledAt > 0;
        } catch {
          return false;
        }
      },
      { timeout: 60_000, intervals: [1_000, 3_000] },
    )
    .toBe(true);
});

for (const theme of THEMES) {
  test(`logs — the device's clock, ${theme}`, async ({ page }) => {
    // The hour on either side of the line the switch wrote: before it, a gap,
    // because nothing is stored from then; after it, real zeros.
    await openApp(page, `${TAB}?from=${pulledAt - 1_800_000}&to=${pulledAt + 1_800_000}`, { backend, theme });
    await expect(page.getByRole('list', { name: 'Log lines' })).toBeVisible();
    await shoot(page, theme, 'device-clock');

    await page.getByRole('list', { name: 'Log lines' }).getByRole('button').first().click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await shoot(page, theme, 'device-clock-entry');
    await page.keyboard.press('Escape');
  });
}
