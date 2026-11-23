/**
 * The Logs tab, against a real engine and real traffic.
 *
 * Nothing here is stubbed and no row is written behind the feature's back. A
 * `nocturne-server` is started on its own empty data directory and the
 * application is opened at `#/monitoring/logs`, where the tab lives. What
 * the screens show is then produced the way it is produced on a network:
 *
 * * **syslog** — lines in the shapes a Cisco switch puts on the wire are
 *   sent over UDP, from this test process, to the socket the screen's own
 *   button opened;
 * * **a lab's lines** — a simulated switch is configured from its console
 *   (a real terminal session) and the lab hands what it logs to
 *   `logs.ingest`, which is the only way a lab line arrives and is refused
 *   for hardware;
 * * **traps** — datagrams captured from net-snmp 5.9.4's `snmptrap` (the
 *   same captures `crates/nocturne-logs/tests/traps.rs` replays; net-snmp
 *   sent them, no switch did) are sent to the port the engine reports;
 * * **flows** — NetFlow v5 and v9 datagrams **written by hand** to Cisco's
 *   v5 format and RFC 3954, as `crates/nocturne-logs/tests/flows.rs` builds
 *   them. They are synthetic: no exporter sent them, and no router has ever
 *   met this collector;
 * * **forwarding** — a TCP listener in this process is the collector, and
 *   reads what the engine really sends it;
 * * **a detection** — a console session on a second lab switch, with no
 *   change in the pipeline, is what `unplanned_config_change` fires on.
 */

import { createSocket, type Socket } from 'node:dgram';
import { execFile } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer, type Server, type Socket as Connection } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import { expect, test, type Locator, type Page } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

let backend: Backend;
let uiDir: string;

/** The ports the receivers are told to open. High, so they need no privileges. */
const PORT = 21_000 + Math.floor(Math.random() * 900);
const TRAP_PORT = 22_000 + Math.floor(Math.random() * 900);
const FLOW_PORT = 23_000 + Math.floor(Math.random() * 900);
/** A port this process holds, so that the collector cannot have it. */
const BUSY_PORT = 24_000 + Math.floor(Math.random() * 900);

/**
 * The SNMP community stored for CORE1, and one that is wrong. Both are test
 * values; the first goes to the vault through `credentials.create`. A good
 * part of the trap tests is about neither turning up on a screen.
 */
const COMMUNITY = 'n0cturne-ro';
const WRONG_COMMUNITY = 'gu3ssed-rw';

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

/** Sends datagrams to a receiver the application opened. */
async function sendTo(
  port: number,
  datagrams: readonly (string | Buffer)[],
  options: { from?: string } = {},
): Promise<void> {
  const socket = createSocket('udp4');
  if (options.from) await new Promise<void>((done) => socket.bind({ address: options.from }, done));
  for (const [index, datagram] of datagrams.entries()) {
    socket.send(typeof datagram === 'string' ? Buffer.from(datagram) : datagram, port, '127.0.0.1');
    // Loopback has a finite socket buffer and UDP is allowed to lose a
    // datagram the kernel has no room for — before the application ever sees
    // it, which is why `logs.status` does not pretend to count those. A pause
    // every few dozen lines keeps the loss out of what this test asserts on.
    if (index % 50 === 49) await new Promise((done) => setTimeout(done, 20));
  }
  await new Promise((done) => setTimeout(done, 150));
  socket.close();
}

/** Syslog lines, to the syslog receiver. */
function send(lines: readonly string[], options: { from?: string } = {}): Promise<void> {
  return sendTo(PORT, lines, options);
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

// ------------------------------------------------------------------ the traps

function hex(text: string): Buffer {
  return Buffer.from(text.replace(/\s+/g, ''), 'hex');
}

/**
 * Captured off the wire from net-snmp 5.9.4 — not written by hand, and not
 * sent by a device. Each is the datagram the command above it produced; they
 * are the captures `crates/nocturne-logs/tests/traps.rs` replays.
 *
 * `snmptrap -v 2c -c n0cturne-ro <receiver> 12345 1.3.6.1.6.3.1.1.5.3
 * 1.3.6.1.2.1.2.2.1.1.3 i 3 1.3.6.1.2.1.2.2.1.7.3 i 1 1.3.6.1.2.1.2.2.1.8.3 i 2`
 * — linkDown, ifIndex.3 = 3, ifAdminStatus.3 = up, ifOperStatus.3 = down.
 */
const V2C_LINK_DOWN = hex(`
  307c020101040b6e30637475726e652d726fa76a020460ca908d020100020100305c300e06082b060102
  01010300430230393017060a2b06010603010104010006092b0601060301010503300f060a2b06010201
  0202010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020102`);

/** The linkDown above, sent with `-c gu3ssed-rw`: the wrong community. */
const V2C_LINK_DOWN_WRONG_COMMUNITY = hex(`
  307b020101040a677533737365642d7277a76a020457e98011020100020100305c300e06082b06010201
  010300430230393017060a2b06010603010104010006092b0601060301010503300f060a2b0601020102
  02010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020102`);

/**
 * `snmptrap -v 1 -c n0cturne-ro <receiver> 1.3.6.1.4.1.8072.3.2.10 192.0.2.1 2 0 12345 …`
 * — an SNMPv1 generic trap 2 (linkDown) that claims the agent address 192.0.2.1.
 */
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

/**
 * A NetFlow v5 export datagram, **written by hand** to Cisco's format: a
 * 24-byte header and 48 bytes per record. `sampling` is the header field (two
 * bits of mode, fourteen of interval). No exporter sent this.
 */
function v5(sampling: number, records: readonly V5Record[]): Buffer {
  const header = Buffer.alloc(24);
  header.writeUInt16BE(5, 0);
  header.writeUInt16BE(records.length, 2);
  header.writeUInt32BE(86_400_000, 4); // SysUptime
  header.writeUInt32BE(1_763_000_000, 8); // unix_secs
  header.writeUInt32BE(1, 16); // flow_sequence
  header.writeUInt16BE(sampling, 22);
  const body = records.map((record) => {
    const out = Buffer.alloc(48);
    Buffer.from(record.src).copy(out, 0);
    Buffer.from(record.dst).copy(out, 4);
    out.writeUInt16BE(record.input, 12);
    out.writeUInt16BE(record.output, 14);
    out.writeUInt32BE(record.packets, 16);
    out.writeUInt32BE(record.bytes, 20);
    out.writeUInt32BE(80_000, 24); // First
    out.writeUInt32BE(81_000, 28); // Last
    out.writeUInt16BE(record.srcPort, 32);
    out.writeUInt16BE(record.dstPort, 34);
    out[37] = 0x18; // TCP flags
    out[38] = record.protocol;
    out[39] = record.tos;
    return out;
  });
  return Buffer.concat([header, ...body]);
}

/**
 * A NetFlow v9 datagram holding one data set of template 260 and **no
 * template**, written by hand to RFC 3954: what arrives after an exporter
 * restarts. It cannot be read, and the collector must say so, not guess.
 */
function v9DataWithoutTemplate(): Buffer {
  const header = Buffer.alloc(20);
  header.writeUInt16BE(9, 0);
  header.writeUInt16BE(1, 2);
  header.writeUInt32BE(86_400_000, 4);
  header.writeUInt32BE(1_763_000_000, 8);
  header.writeUInt32BE(1, 12);
  header.writeUInt32BE(7, 16); // source id
  const content = Buffer.alloc(16);
  Buffer.from([10, 0, 0, 1, 192, 0, 2, 80]).copy(content, 0);
  content.writeUInt32BE(4_000, 8);
  content.writeUInt32BE(4, 12);
  const set = Buffer.alloc(4);
  set.writeUInt16BE(260, 0);
  set.writeUInt16BE(4 + content.length, 2);
  return Buffer.concat([header, set, content]);
}

/** Three flows from one exporter: HTTPS, RTP marked EF, and a DNS query. */
const FLOWS: readonly V5Record[] = [
  { src: [10, 0, 0, 5], dst: [192, 0, 2, 80], input: 3, output: 5, packets: 22, bytes: 24_880, srcPort: 51_000, dstPort: 443, protocol: 6, tos: 0 },
  { src: [10, 0, 0, 7], dst: [10, 0, 0, 9], input: 3, output: 5, packets: 50, bytes: 9_400, srcPort: 16_384, dstPort: 16_384, protocol: 17, tos: 0xb8 },
  { src: [10, 0, 0, 5], dst: [192, 0, 2, 53], input: 3, output: 5, packets: 2, bytes: 180, srcPort: 40_000, dstPort: 53, protocol: 17, tos: 0 },
];

/** The header field for "sampling 1 in 100": mode 1 in the top two bits. */
const SAMPLED_1_IN_100 = 0x4000 | 100;

// ------------------------------------------------------------------ the status

interface Status {
  syslog: { listening: string[] };
  traps: { listening: string[]; received: number; stored: number; rejected: number };
  flows: { listening: string[]; error: string | null; records: number; counted: number; exporters: number };
  forwarders: { id: string; name: string; state: string; sent: number }[];
  store: { rows: number };
  unattributed_sources: number;
}

/** The port a receiver is actually bound to, as the engine reports it. */
async function listeningPort(rpc: Rpc, receiver: 'traps' | 'flows'): Promise<number> {
  let port = 0;
  await expect
    .poll(
      async () => {
        const status = await rpc<Status>('logs.status');
        port = Number(status[receiver].listening[0]?.split(':').pop() ?? 0);
        return port;
      },
      { timeout: 30_000, intervals: [100, 250, 500] },
    )
    .toBeGreaterThan(0);
  return port;
}

// -------------------------------------------------------------------- the lab

/** When each lab switch's console last logged a configuration change (see `configureFromConsole`). */
const lastConsoleChange = new Map<string, number>();

/**
 * Configures a simulated switch from its own console — a real terminal
 * session to the simulator, typed into like any other. The switch then logs
 * `%SYS-5-CONFIG_I`, the lab hands the line to `logs.ingest`, and from there
 * it is a log line like a datagram's: stored, matched by the rules, sent on
 * by a forwarder, judged by a detection.
 *
 * Every such line is stored, but not every one is a signal: the rule that
 * turns it into `logs.config_changed` speaks once a minute per device (its
 * throttle) and counts the rest, and the detection is decided from that
 * signal. A test that needs a detection's hit therefore configures a switch
 * nothing has configured for a minute — `LogLab/SW2`, which nothing else
 * touches.
 */
async function configureFromConsole(rpc: Rpc, deviceId: string, description: string): Promise<void> {
  // A detection folds the same line from the same device inside ten seconds
  // into one sighting, and this line is always the same text. Two sessions
  // on one switch therefore wait for that window to pass.
  const wait = (lastConsoleChange.get(deviceId) ?? 0) + 11_000 - Date.now();
  if (wait > 0) await new Promise((done) => setTimeout(done, wait));
  const session = await rpc<{ id: string }>('terminal.open', {
    device_id: deviceId,
    cols: 120,
    rows: 30,
    log: false,
  });
  for (const command of [
    'enable',
    'configure terminal',
    'interface FastEthernet0/1',
    `description ${description}`,
    'end',
  ]) {
    await rpc('terminal.input', { session_id: session.id, data: `${command}\r` });
    await new Promise((done) => setTimeout(done, 300));
  }
  lastConsoleChange.set(deviceId, Date.now());
  await rpc('terminal.close', { session_id: session.id });
}

async function labSwitch(rpc: Rpc, name: 'LogLab/SW1' | 'LogLab/SW2' = 'LogLab/SW1'): Promise<string> {
  const devices = await rpc<{ id: string; name: string }[]>('devices.list');
  const id = devices.find((device) => device.name === name)?.id ?? '';
  expect(id, `${name} is in the inventory`).not.toBe('');
  return id;
}

// -------------------------------------------------------------- the collector

interface Collector {
  port: number;
  /** Everything the engine has sent so far, as text. */
  received: () => string;
  close: () => Promise<void>;
}

/** A TCP listener in this process that keeps what it is sent: the log platform. */
async function collector(): Promise<Collector> {
  const chunks: Buffer[] = [];
  const open = new Set<Connection>();
  const server: Server = createServer((socket) => {
    open.add(socket);
    socket.on('data', (chunk) => chunks.push(chunk));
    socket.on('error', () => undefined);
    socket.on('close', () => open.delete(socket));
  });
  await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('the collector has no port');
  return {
    port: address.port,
    received: () => Buffer.concat(chunks).toString('utf8'),
    // A forwarder keeps its connection open, and a server only finishes
    // closing once every connection has: the collector "goes away" by
    // dropping them, as a crashed one would.
    close: () =>
      new Promise<void>((done) => {
        server.close(() => done());
        for (const socket of open) socket.destroy();
      }),
  };
}

// ------------------------------------------------------------------ the seed

async function seed(rpc: Rpc): Promise<void> {
  const credential = await rpc<{ id: string }>('credentials.create', {
    name: 'Campus SNMP',
    username: 'netops',
    password: 'not-a-real-password',
    snmp_version: 'v2c',
    snmp_community: COMMUNITY,
  });
  await rpc('devices.create', {
    name: 'CORE1',
    // Every datagram this test sends comes from the loopback address, so this
    // is the device the engine attributes them to — by address, as it only
    // ever does.
    address: '127.0.0.1',
    profile: 'cisco_ios',
    site: 'HQ',
    role: 'core',
    credential_id: credential.id,
  });
  await rpc('devices.create', {
    name: 'ACC9',
    address: '10.9.9.9',
    profile: 'mikrotik_routeros',
    site: 'HQ',
    role: 'access',
  });
  // The ports are set but every receiver is left off: the screen's own
  // switches turn them on, which is what the tests are about.
  await rpc('settings.set', {
    section: 'logs',
    value: {
      syslog: { enabled: false, udp_port: PORT, tcp_port: 0 },
      traps: { enabled: false, port: TRAP_PORT, bind: '127.0.0.1' },
      flows: { enabled: false, port: FLOW_PORT, bind: '127.0.0.1' },
    },
  });
}

function hint(page: Page) {
  return page.getByRole('region', { name: 'Log reception' });
}

function rows(page: Page) {
  return page.getByRole('list', { name: 'Log lines' }).getByRole('listitem');
}

/** One receiver's card on the Receivers pane. */
function receiver(page: Page, title: 'Syslog' | 'SNMP traps' | 'Flows'): Locator {
  return page.getByRole('region', { name: `${title} receiver` });
}

/** The value of one of a card's counters, by the engine's own field name. */
function counterOf(card: Locator, id: string): Locator {
  return card.locator(`[data-counter="${id}"] dd`);
}

/** Opens one of the tab's panes by its tab, the way a person does. */
async function openPane(page: Page, name: string): Promise<void> {
  await page.getByRole('tablist', { name: 'Logs' }).getByRole('tab', { name }).click();
}

test.beforeAll(async ({ request }) => {
  test.setTimeout(300_000);
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
  await seed(rpcFor(request, backend));
});

test.afterAll(async () => {
  await backend?.stop();
  if (uiDir && process.env.NOCTURNE_UI_DIST === undefined) {
    await rm(uiDir, { recursive: true, force: true });
  }
});

// ============================================================ the lines pane

test('says the receiver is off and gives the exact command for a real device', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs', { backend });

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
    name: 'Device to show the commands for',
  });
  await picker.fill('CORE1');
  await picker.press('Enter');
  await expect(hint(page)).toContainText(`logging host 127.0.0.1 transport udp port ${PORT}`);
  await expect(hint(page)).toContainText('service timestamps log datetime msec');
  await expect(hint(page)).toContainText('These lines are not applied');
  // And nothing claims the network is quiet.
  await expect(page.getByRole('heading', { name: 'No line has arrived yet' })).toBeVisible();
  await expect(page.getByText(/No line matches this search/)).toHaveCount(0);
  // The engine has a trap receiver and a flow collector, off until somebody
  // switches them on: nothing here may call them absent.
  await expect(hint(page).getByText(/not in this build/)).toHaveCount(0);

  expect(console.messages).toEqual([]);
});

test('the screen can open the socket, and then says nothing has arrived yet', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs', { backend });

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
  await openApp(page, '/monitoring/logs', { backend });

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
  await openApp(page, '/monitoring/logs', { backend });

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
  await openApp(page, '/monitoring/logs?span=7d', { backend });

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
  await openApp(page, '/monitoring/logs', { backend });

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
  await openApp(page, '/monitoring/logs', { backend });

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

  await openApp(page, '/monitoring/logs', { backend });
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
  // SW2 is for the detection tests further down: a switch nothing else
  // configures, so its first change is not inside another's throttle.
  const lab = await rpc<{ id: string }>('labs.import', {
    spec: {
      name: 'LogLab',
      description: 'two switches: one for its own log buffer and its console, one for a detection',
      nodes: ['SW1', 'SW2'].map((name, index) => ({
        name,
        model: 'switch',
        x: index * 200,
        y: 0,
        startup_config: [`hostname ${name}`, 'service timestamps log datetime msec'].join('\n'),
      })),
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
  await openApp(page, `/monitoring/logs?from=${at - 3_600_000}&to=${at + 3_600_000}`, { backend });

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
  await openApp(page, '/monitoring/logs?span=24h', { backend });

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
  await openApp(page, '/monitoring/logs?q=mnemonic%3ALINK-3-UPDOWN&sev=3&span=24h', { backend });
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

// ============================================================== a lab's lines

test('a lab switch’s own lines arrive through the lab, and say so', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  // The lab of the test above is still running. Its switch is configured from
  // its console; what it logs is handed to `logs.ingest` by the lab, which is
  // the only way a simulated device's line gets here.
  const device = await labSwitch(rpc);
  await configureFromConsole(rpc, device, 'uplink to the core');

  await openApp(page, '/monitoring/logs?q=mnemonic%3ASYS-5-CONFIG_I&span=15m', { backend });
  const line = rows(page).filter({ hasText: 'LogLab/SW1' }).first();
  await expect(line).toContainText('Configured from console', { timeout: 30_000 });
  await expect(line).toContainText('SYS-5-CONFIG_I');

  await line.getByRole('button').click();
  const drawer = page.getByRole('dialog', { name: 'SYS-5-CONFIG_I' });
  await expect(drawer).toContainText('written by a simulated device in a lab');
  // It was sent from no address, and none is made up for it.
  await expect(drawer).toContainText('no address — a simulated device sends from none');
  await expect(drawer).toContainText('%SYS-5-CONFIG_I: Configured from console');
  await page.keyboard.press('Escape');

  // And `logs.ingest` is not a way to write lines for hardware: the engine
  // refuses it for a device that is not a lab's.
  await expect(rpc('logs.ingest', { device_id: 'CORE1', lines: ['made up'] })).rejects.toThrow(
    /forbidden.*is not a lab device/,
  );

  expect(console.messages).toEqual([]);
});

// ========================================================== the receivers pane

test('each receiver has a card that says whether it is listening, and why not', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs', { backend });
  await openPane(page, 'Receivers');

  // Syslog was switched on by an earlier test and has been sent lines.
  const syslog = receiver(page, 'Syslog');
  await expect(syslog).toContainText('Listening');
  await expect(syslog).toContainText(`udp 0.0.0.0:${PORT}`);
  await expect(counterOf(syslog, 'received')).not.toHaveText(/^0\b/);
  // A lab's lines came off no socket and are counted apart.
  await expect(counterOf(syslog, 'from_lab')).not.toHaveText(/^0\b/);

  // The other two are off, and say what switching them on would open.
  const traps = receiver(page, 'SNMP traps');
  await expect(traps).toContainText('Switched off');
  await expect(traps).toContainText(`switching it on opens UDP ${TRAP_PORT} on 127.0.0.1`);
  const flows = receiver(page, 'Flows');
  await expect(flows).toContainText('Switched off');
  await expect(flows).toContainText(`switching it on opens UDP ${FLOW_PORT} on 127.0.0.1`);
  // Neither has received anything, and the engine says so with a counter —
  // which is there because the engine gave it, not because the card made it.
  await expect(counterOf(traps, 'received')).toHaveText(/^0\b/);
  await expect(counterOf(flows, 'records')).toHaveText(/^0\b/);
  // A flow collector stores nothing, so it has no "stored" to show.
  await expect(flows.locator('[data-counter="stored"]')).toHaveCount(0);

  expect(console.messages).toEqual([]);
});

test('the trap receiver is switched on from its card, and says who can forge a trap', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  await openApp(page, '/monitoring/logs?pane=receivers', { backend });

  const traps = receiver(page, 'SNMP traps');
  // The warning belongs where the port is opened, and is not there before.
  await expect(traps).not.toContainText('whoever can reach this port can forge one');
  await traps.getByRole('switch', { name: 'Receive SNMP traps' }).click();

  await expect(traps).toContainText('Listening', { timeout: 30_000 });
  await expect(traps).toContainText(`udp 127.0.0.1:${TRAP_PORT}`);
  await expect(traps).toContainText('whoever can reach this port can forge one');
  await expect(traps).toContainText('never a switch or a router');
  expect(await listeningPort(rpc, 'traps')).toBe(TRAP_PORT);

  expect(console.messages).toEqual([]);
});

test('real traps are counted where they ended, and a refused source is listed with the reason', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const port = await listeningPort(rpc, 'traps');
  await openApp(page, '/monitoring/logs?pane=receivers', { backend });
  const traps = receiver(page, 'SNMP traps');

  // Two notifications with the community stored for CORE1, one with a wrong
  // one, and one datagram that is no notification at all.
  await sendTo(port, [V2C_LINK_DOWN, V1_LINK_DOWN, V2C_LINK_DOWN_WRONG_COMMUNITY, 'not a trap']);

  // The card follows `logs.status_changed` and `logs.appended`: no reload.
  await expect(counterOf(traps, 'received')).toHaveText(/^4\b/, { timeout: 30_000 });
  await expect(counterOf(traps, 'stored')).toHaveText(/^2\b/);
  await expect(counterOf(traps, 'rejected')).toHaveText(/^1\b/);
  await expect(counterOf(traps, 'malformed')).toHaveText(/^1\b/);
  await expect(counterOf(traps, 'unauthenticated')).toHaveText(/^0\b/);

  // Why it was refused is the engine's sentence, and it names the device.
  const refused = traps.getByRole('list', { name: 'Trap sources that were refused' });
  await expect(refused).toContainText('127.0.0.1');
  await expect(refused).toContainText('the community is not the one stored for `CORE1`');
  await expect(refused).toContainText('CORE1');

  // A community is a secret: neither the right one nor the wrong one is
  // anywhere on the screen.
  const text = await page.locator('body').innerText();
  expect(text).not.toContain(COMMUNITY);
  expect(text).not.toContain(WRONG_COMMUNITY);

  expect(console.messages).toEqual([]);
});

test('a trap is a row with no severity, and its body is shown field by field', async ({ page }) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs?stream=trap&span=15m', { backend });

  // Two were stored. The message is the notification's name and its
  // bindings by name, as the engine wrote it.
  await expect(rows(page)).toHaveCount(2, { timeout: 30_000 });
  const first = rows(page).first();
  await expect(first).toContainText('CORE1');
  await expect(first).toContainText('linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)');
  // No severity: not "info", and not the dash of a line that came without
  // one. The cell is empty and says why.
  const severity = first.locator('[data-severity]');
  await expect(severity).toHaveAttribute('data-severity', 'none');
  await expect(severity).toHaveText('');
  await expect(first).not.toContainText('info');
  await expect(page.getByText('Notification', { exact: true })).toBeVisible();

  // The SNMPv1 trap is the newer of the two.
  await rows(page).last().getByRole('button').click();
  const drawer = page.getByRole('dialog', { name: 'linkDown' });
  await expect(drawer).toContainText('none — an SNMP notification carries no severity');
  await expect(drawer).toContainText('SNMPv1');
  // What the agent claims, next to where the datagram really came from.
  await expect(drawer).toContainText('Agent address (claimed)');
  await expect(drawer).toContainText('192.0.2.1');
  await expect(drawer).toContainText('It never decides the device');
  await expect(drawer).toContainText('1.3.6.1.4.1.8072.3.2.10');
  await expect(drawer).toContainText('2 / 0');
  // No monitor has polled CORE1, so the interface is a number and stays one.
  await expect(drawer).toContainText('ifIndex 3');
  await expect(drawer).toContainText('None is made up');

  const bindings = drawer.getByRole('region', { name: 'Bindings' });
  await expect(bindings).toContainText('3 bindings, in the order they were sent');
  await expect(bindings.getByRole('row')).toHaveCount(4);
  await expect(bindings.getByRole('row').nth(1)).toContainText('ifIndex.3');
  await expect(bindings.getByRole('row').nth(1)).toContainText('1.3.6.1.2.1.2.2.1.1.3');
  await expect(bindings.getByRole('row').nth(3)).toContainText('ifOperStatus.3');
  await expect(bindings.getByRole('row').nth(3)).toContainText('down(2)');
  await expect(drawer).not.toContainText(COMMUNITY);
  await page.keyboard.press('Escape');

  expect(console.messages).toEqual([]);
});

test('the trap stream has its own receiver: while it is off the stream says so, with SNMP lines', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  // Syslog stays on. The hint on the trap stream must be about the trap
  // receiver, not about the healthy syslog one.
  await rpc('settings.set', { section: 'logs', value: { traps: { enabled: false } } });
  await openApp(page, '/monitoring/logs?stream=trap&span=15m', { backend });

  await expect(hint(page)).toContainText('The SNMP trap receiver is switched off');
  const picker = hint(page).getByRole('combobox', { name: 'Device to show the commands for' });
  await picker.fill('CORE1');
  await picker.press('Enter');
  const lines = hint(page).getByTestId('setup-traps');
  await expect(lines).toContainText('snmp-server enable traps snmp authentication linkdown linkup');
  await expect(lines).toContainText(
    `snmp-server host 127.0.0.1 version 2c <snmp-community> udp-port ${TRAP_PORT}`,
  );
  // The community in those lines is a placeholder, and the screen says so.
  await expect(lines).toContainText('<snmp-community> is not a community');
  await expect(lines).toContainText('Replace it before the lines are used');
  await expect(lines).toContainText('These lines are not applied, and not complete');
  await expect(lines).toContainText('have not been applied to a device');
  await expect(lines).not.toContainText(COMMUNITY);
  // The syslog lines are for the other stream.
  await expect(hint(page).getByTestId('setup-syslog')).toHaveCount(0);

  // The same switch as on the card.
  await page.getByRole('button', { name: 'Turn the receiver on' }).click();
  await expect(page.getByText('The SNMP trap receiver is switched off')).toHaveCount(0, {
    timeout: 30_000,
  });
  expect(await listeningPort(rpc, 'traps')).toBe(TRAP_PORT);

  expect(console.messages).toEqual([]);
});

test('a receiver that cannot have its port says why, and “Restart receivers” tries again', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);

  // This process holds a port. The collector is told to use it.
  const squatter: Socket = createSocket('udp4');
  await new Promise<void>((done) => squatter.bind({ address: '127.0.0.1', port: BUSY_PORT }, done));
  try {
    await openApp(page, '/monitoring/logs?pane=receivers', { backend });
    const flows = receiver(page, 'Flows');
    await flows.getByRole('switch', { name: 'Collect flows' }).click();
    await expect(flows).toContainText(`udp 127.0.0.1:${FLOW_PORT}`, { timeout: 30_000 });

    // The port is changed through the settings the engine declares.
    const form = flows.getByRole('form', { name: 'Flows port and address' });
    await form.getByLabel('UDP port').fill('70000');
    await expect(form).toContainText('A port is at most 65535');
    await expect(form.getByRole('button', { name: 'Apply' })).toBeDisabled();
    await form.getByLabel('UDP port').fill(String(BUSY_PORT));
    await form.getByRole('button', { name: 'Apply' }).click();

    // Enabled, not listening, and the reason is the engine's.
    await expect(flows).toContainText('Not listening', { timeout: 30_000 });
    await expect(flows.getByRole('alert')).toContainText(`127.0.0.1:${BUSY_PORT}`);
    await expect(flows.getByRole('alert')).toContainText('already using it');
    await expect(page.getByRole('note')).toContainText('“Restart receivers” tries again');
  } finally {
    await new Promise<void>((done) => squatter.close(() => done()));
  }

  // Whatever held the port has gone. A receiver that failed to bind does not
  // notice by itself; the button is what tries again.
  await page.getByRole('button', { name: 'Restart receivers' }).click();
  const flows = receiver(page, 'Flows');
  await expect(flows).toContainText('Listening', { timeout: 30_000 });
  await expect(flows).toContainText(`udp 127.0.0.1:${BUSY_PORT}`);
  await expect(flows.getByRole('alert')).toHaveCount(0);
  expect(await listeningPort(rpc, 'flows')).toBe(BUSY_PORT);
  // The trap receiver was running and was left alone: what it counted stays.
  await expect(counterOf(receiver(page, 'SNMP traps'), 'stored')).toHaveText(/^2\b/);

  expect(console.messages).toEqual([]);
});

test('the receivers pane shows what a device has to be told for both syslog and traps', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs?pane=receivers', { backend });

  const setup = page.getByRole('region', { name: 'What a device has to be told' });
  const picker = setup.getByRole('combobox', { name: 'Device to show the commands for' });
  await picker.fill('CORE1');
  await picker.press('Enter');
  await expect(setup.getByTestId('setup-syslog')).toContainText(
    `logging host 127.0.0.1 transport udp port ${PORT}`,
  );
  await expect(setup.getByTestId('setup-traps')).toContainText(
    `version 2c <snmp-community> udp-port ${TRAP_PORT}`,
  );
  await expect(setup.getByTestId('setup-traps')).toContainText('Placeholder');

  // RouterOS cannot be told a port for traps, and the engine's note says what
  // that means for a receiver that is not on 162.
  await picker.fill('ACC9');
  await picker.press('Enter');
  await expect(setup.getByTestId('setup-traps')).toContainText('trap-community=<snmp-community>');
  await expect(setup.getByTestId('setup-traps')).toContainText('UDP 162');

  expect(console.messages).toEqual([]);
});

// ================================================================= the rules

function ruleRow(page: Page, name: string): Locator {
  return page.getByRole('table', { name: 'Log rules' }).getByRole('row').filter({ hasText: name });
}

test('the rules say which platform each is for, how many in how long, and what clears them', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs?pane=rules', { backend });

  await page.getByRole('searchbox', { name: 'Search the rules' }).fill('repeated failed logins');
  const ios = ruleRow(page, 'IOS: repeated failed logins from one address');
  await expect(ios).toContainText('Cisco IOS / IOS-XE');
  await expect(ios).toContainText('Alert · high');
  await expect(ios).toContainText('8 in 60 s, per source');
  await expect(ios).toContainText('600 s quiet');
  await expect(ruleRow(page, 'RouterOS: repeated failed logins from one address')).toContainText(
    'MikroTik RouterOS',
  );

  // A built-in rule shows the line it was tested against, and where it is from.
  await ios.getByRole('cell').first().dblclick();
  const drawer = page.getByRole('dialog', { name: 'IOS: repeated failed logins from one address' });
  await expect(drawer).toContainText('This rule ships with Nocturne');
  const tested = drawer.getByRole('region', { name: 'The line it was tested against' });
  await expect(tested).toContainText('%SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: 10.4.2.11]');
  // Written by hand to a documented format, and said to be: never "recorded".
  await expect(tested).toContainText('Synthetic');
  await expect(tested).toContainText('it is not a line a device sent');
  await expect(tested).toContainText('No built-in rule’s line was recorded from a device');

  // One such line matches and raises nothing: the rule wants eight.
  await drawer.getByRole('button', { name: 'Test the rule' }).click();
  const result = drawer.getByTestId('rule-test-result');
  await expect(result).toContainText('this one line raises nothing yet');
  await expect(result).toContainText(
    'One such line raises nothing: the signal is raised when 8 of them arrive inside 60 s for the same device and the same source',
  );
  await expect(result).toContainText('10.4.2.11');
  await page.keyboard.press('Escape');

  expect(console.messages).toEqual([]);
});

test('the test box works for a trap rule: a notification, no severity, and its opposite', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs?pane=rules', { backend });

  await page.getByRole('combobox', { name: 'Reads' }).selectOption('trap');
  const link = ruleRow(page, 'Trap: link down');
  await expect(link).toContainText('Any SNMP agent (SNMPv2-MIB, IF-MIB)');
  await expect(link).toContainText('Every match, per ifindex');
  await expect(link).toContainText('Its opposite event');

  await link.getByRole('cell').first().dblclick();
  const drawer = page.getByRole('dialog', { name: 'Trap: link down' });
  // A trap rule's line is synthetic in its own way: the receiver's message
  // for a datagram net-snmp sent.
  await expect(drawer).toContainText('net-snmp sent it, not a device');

  // The message of the trap the receiver stored a few tests ago.
  const box = drawer.getByLabel('A trap’s message');
  await box.fill('linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)');
  await drawer.getByRole('button', { name: 'Test the rule' }).click();
  const result = drawer.getByTestId('rule-test-result');
  await expect(result).toContainText('The rule matches, and this line raises its signal');
  await expect(result).toContainText('an alert (low): Link down: ifIndex 3');
  await expect(result).toContainText('none — a trap carries no severity');
  // What it is counted under is the ifIndex, which is in every such trap.
  await expect(result.getByText('Counted under').locator('..')).toContainText('3');

  await box.fill('linkUp ifIndex=3 ifAdminStatus=up(1) ifOperStatus=up(1)');
  await drawer.getByRole('button', { name: 'Test the rule' }).click();
  await expect(result).toContainText('This is the rule’s opposite: it clears the alert');
  await expect(result).toContainText(
    'this line is the rule\'s opposite event: it raises nothing and clears the alert the rule has open for this instance',
  );
  await page.keyboard.press('Escape');

  expect(console.messages).toEqual([]);
});

test('a rule with a threshold is saved, and the engine’s refusals stand beside their fields', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  await openApp(page, '/monitoring/logs?pane=rules', { backend });

  await page.getByRole('button', { name: 'New rule' }).click();
  const drawer = page.getByRole('dialog', { name: 'New rule' });
  await drawer.getByLabel('Name').fill('ACL denies');
  await drawer.getByLabel('Pattern').fill('%SEC-6-IPACCESSLOGP: list (?P<acl>\\S+) denied');
  await drawer.getByLabel('Alert title').fill('{acl} is denying a lot');
  await drawer.getByRole('switch', { name: 'Raise only when several matching lines arrive' }).click();
  await drawer.getByLabel('Lines', { exact: true }).fill('3');
  await drawer.getByLabel('Inside (seconds)').fill('60');
  // The pattern's groups are offered where they are chosen.
  await expect(drawer.getByText('Groups of the pattern: acl.')).toBeVisible();
  const counted = drawer.getByLabel('Counted per');
  await counted.fill('nope');
  await counted.press('Enter');

  // A group the pattern does not capture: the engine says so, and its
  // sentence is under the field it is about — not in a toast, not reworded.
  // It is the field's own description, so it is read out with the field.
  await drawer.getByRole('button', { name: 'Save' }).click();
  await expect(counted).toHaveAccessibleDescription('the pattern has no group named `nope` to count by');
  await expect(counted).toHaveAttribute('aria-invalid', 'true');

  await counted.press('Backspace');
  await counted.fill('acl');
  await counted.press('Enter');
  // A quiet period of nought seconds: again the engine's words, by its field.
  const quiet = drawer.getByLabel('Clear after this many quiet seconds');
  await quiet.fill('0');
  await drawer.getByRole('button', { name: 'Save' }).click();
  await expect(
    drawer.getByText('an alert is cleared after between 1 and 2592000 quiet seconds, not 0'),
  ).toBeVisible();
  await expect(quiet).toHaveAttribute('aria-invalid', 'true');
  // The earlier refusal is gone: it is no longer true.
  await expect(drawer.getByText('the pattern has no group named')).toHaveCount(0);

  await quiet.fill('300');
  await drawer.getByRole('button', { name: 'Save' }).click();
  await expect(drawer).toHaveCount(0);

  await page.getByRole('combobox', { name: 'Platform' }).selectOption('own');
  const row = ruleRow(page, 'ACL denies');
  await expect(row).toContainText('Your own');
  await expect(row).toContainText('3 in 60 s, per acl');
  await expect(row).toContainText('300 s quiet');
  const saved = (await rpc<{ name: string; threshold: unknown; clear_after_s: number }[]>('logs.rules.list')).find(
    (rule) => rule.name === 'ACL denies',
  );
  expect(saved?.threshold).toEqual({ count: 3, window_s: 60, group_by: ['acl'] });
  expect(saved?.clear_after_s).toBe(300);

  // Its own switch is on the row, and a rule of one's own can be deleted.
  await row.getByRole('switch').click();
  await expect(row.getByRole('switch')).not.toBeChecked();
  await row.getByRole('cell').first().dblclick();
  await page.getByRole('dialog', { name: 'ACL denies' }).getByRole('button', { name: 'Delete…' }).click();
  await page.getByRole('button', { name: 'Delete the rule' }).click();
  await expect(ruleRow(page, 'ACL denies')).toHaveCount(0);

  expect(console.messages).toEqual([]);
});

// ============================================================== forwarding

function forwarderRow(page: Page, name: string): Locator {
  return page.getByRole('table', { name: 'Log forwarders' }).getByRole('row').filter({ hasText: name });
}

test('a forwarder hands what is stored to a collector, and a test says what happened', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const siem = await collector();
  try {
    await openApp(page, '/monitoring/logs?pane=forwarding', { backend });
    await expect(page.getByRole('heading', { name: 'Nothing is forwarded' })).toBeVisible();

    await page.getByRole('button', { name: 'Add a forwarder' }).first().click();
    const drawer = page.getByRole('dialog', { name: 'New forwarder' });
    await drawer.getByLabel('Name').fill('SIEM');
    await drawer.getByLabel('Kind').selectOption('syslog_tcp');
    await drawer.getByLabel('Collector').fill('127.0.0.1');
    await drawer.getByLabel('Port').fill(String(siem.port));
    // The audit trail is never a default, and ticking it says what it means.
    await expect(drawer.getByRole('checkbox', { name: /Audit trail/ })).not.toBeChecked();
    await expect(drawer.getByTestId('audit-warning')).toHaveCount(0);
    await drawer.getByRole('checkbox', { name: /Audit trail/ }).check();
    await expect(drawer.getByTestId('audit-warning')).toContainText(
      'sends what people did here',
    );
    await drawer.getByRole('checkbox', { name: /Audit trail/ }).uncheck();
    // A lab is the way to try a forwarder out.
    await drawer.getByRole('switch', { name: 'Also what lab devices write' }).click();
    await drawer.getByRole('button', { name: 'Save' }).click();
    await expect(drawer).toHaveCount(0);

    const row = forwarderRow(page, 'SIEM');
    await expect(row).toContainText('Syslog over TCP');
    await expect(row).toContainText(`127.0.0.1:${siem.port}`);
    await expect(row).toContainText('Device logs, SNMP traps, Signals, and labs');
    await expect(row).toContainText('Nothing sent yet');

    // A test: one event that says it is one, on a connection of its own.
    await row.getByRole('button', { name: 'Row actions' }).click();
    await page.getByRole('menuitem', { name: 'Send a test event' }).click();
    await expect(page.getByTestId('forward-test-outcome')).toContainText(
      'Sent: one test event was handed to “SIEM”',
      { timeout: 30_000 },
    );
    await expect(page.getByTestId('forward-test-outcome')).toContainText('it does not mean “arrived”');
    await expect.poll(() => siem.received(), { timeout: 15_000 }).toContain('This is a test');
    // A test is not counted: the forwarder has still sent nothing.
    await expect(row).toContainText('Nothing sent yet');

    // Now a real line: the lab switch is configured from its console again.
    await configureFromConsole(rpc, await labSwitch(rpc), 'forwarded to the collector');
    await expect
      .poll(() => siem.received(), { timeout: 30_000 })
      .toMatch(/<\d+>1 \S+ LogLab\/SW1 .*SYS-5-CONFIG_I.*Configured from console/);
    // The status follows `logs.status_changed`: the row changes by itself.
    await expect(row).toContainText('Sending', { timeout: 30_000 });
    const sent = (await rpc<Status>('logs.status')).forwarders.find((entry) => entry.name === 'SIEM');
    expect(sent?.sent ?? 0).toBeGreaterThan(0);
  } finally {
    await siem.close();
  }

  // The collector is gone. A test now fails, and says so in the engine's words.
  const row = forwarderRow(page, 'SIEM');
  await row.getByRole('button', { name: 'Row actions' }).click();
  await page.getByRole('menuitem', { name: 'Send a test event' }).click();
  const outcome = page.getByTestId('forward-test-outcome');
  await expect(outcome).toContainText('“SIEM” was not sent the test event:', { timeout: 30_000 });
  await expect(outcome).toContainText(/refused/);

  expect(console.messages).toEqual([]);
});

test('an HTTPS forwarder’s URL and token are written and never shown again', async ({ page, request }) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  // Nothing listens on the discard port of this machine: the collector is
  // "not there", which is all this test needs of it.
  const url = 'https://127.0.0.1:9/services/collector/event?channel=nocturne-e2e';
  const token = '0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0';
  await openApp(page, '/monitoring/logs?pane=forwarding', { backend });

  await page.getByRole('button', { name: 'Add a forwarder' }).click();
  const drawer = page.getByRole('dialog', { name: 'New forwarder' });
  await drawer.getByLabel('Name').fill('Lake');
  await drawer.getByLabel('Kind').selectOption('https_json');
  await expect(drawer).toContainText('once saved they are never shown again');
  // Without a destination there is nothing to save.
  await expect(drawer.getByRole('button', { name: 'Save' })).toBeDisabled();

  // The engine checks the destination, and its refusal stands by the URL.
  await drawer.getByRole('textbox', { name: 'URL', exact: true }).fill('ftp://collector.example.net/drop');
  await drawer.getByRole('button', { name: 'Save' }).click();
  await expect(drawer.getByText('the collector\'s URL has to start with https://')).toBeVisible();
  // The refusal does not repeat what was typed.
  await expect(drawer.getByRole('alert')).toHaveCount(0);
  expect(await drawer.locator('p').filter({ hasText: 'has to start with' }).innerText()).not.toContain(
    'collector.example.net',
  );

  await drawer.getByRole('textbox', { name: 'URL', exact: true }).fill(url);
  // What is typed is masked, like a password.
  await expect(drawer.getByRole('textbox', { name: 'URL', exact: true })).toHaveAttribute('type', 'password');
  await drawer.getByRole('textbox', { name: 'Token', exact: true }).fill(token);
  await drawer.getByRole('switch', { name: 'Switched on' }).click();
  await drawer.getByRole('button', { name: 'Save' }).click();
  await expect(drawer).toHaveCount(0);

  const row = forwarderRow(page, 'Lake');
  await expect(row).toContainText('HTTPS, JSON');
  await expect(row).toContainText('A URL in the vault (never shown)');
  await expect(row).toContainText('Switched off');

  // Neither is on the page, in the list the engine returns, or in the audit
  // entry that records the save.
  const pageText = await page.locator('body').innerText();
  expect(pageText).not.toContain('nocturne-e2e');
  expect(pageText).not.toContain(token);
  const listed = JSON.stringify(await rpc('logs.forward.list'));
  expect(listed).not.toContain('nocturne-e2e');
  expect(listed).not.toContain(token);
  const audit = JSON.stringify(await rpc('logs.query', { stream: 'audit', text: 'forwarder', limit: 50 }));
  expect(audit).toContain('Saved the log forwarder `Lake`');
  expect(audit).not.toContain('nocturne-e2e');
  expect(audit).not.toContain(token);

  // Editing it shows that a destination is stored, and nothing of it.
  await row.getByRole('cell').first().dblclick();
  const edit = page.getByRole('dialog', { name: 'Edit Lake' });
  await expect(edit).toContainText('Stored in the vault. It cannot be shown again.');
  await expect(edit).toContainText('Never shown — not even whether one is stored.');
  await expect(edit.getByTestId('token-fate')).toContainText('The token is left as it is');
  expect(await edit.innerText()).not.toContain(token);
  await expect(edit.locator('input[type="password"]')).toHaveCount(0);

  // A new URL does not carry the old token with it, and the form says so
  // before the save.
  await edit.getByRole('button', { name: 'Replace…' }).first().click();
  // The box that was asked for has the focus, not the button that took the
  // place of "Replace…" — which says "Keep what is stored".
  await expect(edit.getByRole('textbox', { name: 'URL', exact: true })).toBeFocused();
  await edit.getByRole('textbox', { name: 'URL', exact: true }).fill('https://127.0.0.1:9/elsewhere');
  await expect(edit.getByTestId('token-fate')).toContainText('A new URL starts without a token');
  await edit.getByRole('button', { name: 'Keep what is stored' }).click();
  await expect(edit.getByTestId('token-fate')).toContainText('The token is left as it is');

  // A test from the drawer: the collector is not there, in the engine's words.
  await edit.getByRole('button', { name: 'Send a test event' }).click();
  await expect(edit.getByRole('alert')).toContainText('The test event was not sent:', { timeout: 60_000 });
  await expect(edit.getByRole('alert')).toContainText(/refused|could not be reached/);
  expect(await edit.getByRole('alert').innerText()).not.toContain('127.0.0.1:9');

  // Deleting it removes it, and its secret with it.
  await edit.getByRole('button', { name: 'Delete…' }).click();
  await page.getByRole('button', { name: 'Delete the forwarder' }).click();
  await expect(forwarderRow(page, 'Lake')).toHaveCount(0);
  expect(JSON.stringify(await rpc('logs.forward.list'))).not.toContain('Lake');

  expect(console.messages).toEqual([]);
});

// ==================================================================== flows

test('the flows pane shows the top talkers with what the answer covers, and multiplies nothing', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const port = await listeningPort(rpc, 'flows');

  await openApp(page, '/monitoring/logs?pane=flows', { backend });
  // Nothing here has met a real exporter, and the pane says so.
  await expect(page.getByTestId('flows-not-verified')).toContainText('never met a router or a switch');
  // Listening, and nothing sent yet: a quiet period, not a broken collector.
  await expect(page.getByRole('heading', { name: 'No flow record in this period' })).toBeVisible();

  // From CORE1's address: three flows from an exporter that says it samples
  // 1 in 100, and a v9 data set whose template never came. From an address
  // no device claims: one more flow, unsampled.
  await sendTo(port, [v5(SAMPLED_1_IN_100, FLOWS), v9DataWithoutTemplate(), 'garbage']);
  await sendTo(port, [v5(0, FLOWS.slice(0, 1))], { from: '127.0.0.2' });
  await expect
    .poll(async () => (await rpc<Status>('logs.status')).flows.counted, { timeout: 30_000 })
    .toBe(4);
  await page.getByRole('button', { name: 'Ask the collector again' }).click();

  const table = page.getByRole('table', { name: 'Top talkers by source' });
  const top = table.getByRole('row').filter({ hasText: '10.0.0.5' });
  // 24 880 + 180 bytes from the sampling exporter and 24 880 from the other:
  // 49 940 bytes, as exported. Not 100 times that.
  await expect(top).toContainText('48.8 KiB');
  await expect(top).toContainText('84%');
  await expect(top).toContainText('46');
  await expect(table.getByRole('row').filter({ hasText: '10.0.0.7' })).toContainText('9.2 KiB');
  await expect(page.getByTestId('flows-sampled')).toContainText('Sampled 1 in 100 — not multiplied');
  await expect(table.getByRole('columnheader', { name: /Bytes, as exported/ })).toBeVisible();

  // Beside the table: what the answer covers.
  const coverage = page.getByRole('complementary', { name: 'What this answer covers' });
  await expect(coverage.locator('[data-coverage="collecting"]')).toContainText('Listening');
  await expect(coverage.locator('[data-coverage="records"]')).toContainText('4');
  await expect(coverage.locator('[data-coverage="exporters"]')).toContainText('2');
  await expect(coverage.locator('[data-coverage="templates"]')).toContainText('1');
  await expect(coverage.locator('[data-coverage="templates"]')).toContainText(
    'is in no row and not in the total',
  );
  await expect(coverage.locator('[data-coverage="sampling"]')).toContainText('1 in 100');
  await expect(coverage.locator('[data-coverage="sampling"]')).toContainText(
    '3 of the 4 records in this answer',
  );
  await expect(coverage.locator('[data-coverage="sampling"]')).toContainText('they are not multiplied here');
  await expect(coverage.locator('[data-coverage="since"]')).toContainText(/\d\d:\d\d:00/);

  // "Other" and the total, as the engine gives them.
  await expect(page.getByTestId('flows-other')).toContainText('Nothing: every byte counted is in a row above');
  await expect(page.getByTestId('flows-total')).toContainText('57.9 KiB · 96 packets · 4 flows');

  // The other five ways of looking.
  const by = page.getByRole('combobox', { name: 'Add up by' });
  await by.selectOption('destination');
  await expect(page.getByRole('table', { name: 'Top talkers by destination' })).toContainText('192.0.2.80');
  await by.selectOption('conversation');
  await expect(page.getByRole('table', { name: 'Top talkers by conversation' })).toContainText(
    '10.0.0.5 <-> 192.0.2.80',
  );
  await by.selectOption('application');
  const applications = page.getByRole('table', { name: 'Top talkers by application' });
  await expect(applications).toContainText('tcp/443 (https)');
  await expect(applications).toContainText('udp/53 (domain)');
  await by.selectOption('dscp');
  await expect(page.getByRole('table', { name: 'Top talkers by dscp' })).toContainText('EF (46)');
  await by.selectOption('interface');
  const interfaces = page.getByRole('table', { name: 'Top talkers by interface' });
  // No monitor knows these indexes, so they are numbers; and over two
  // exporters the label says whose.
  await expect(interfaces).toContainText('CORE1 ifIndex 3 in');
  await expect(interfaces).toContainText('127.0.0.2 ifIndex 5 out');
  // Each flow is counted in and out, so a row is not a part of the total.
  await expect(interfaces.getByRole('row').nth(1)).toContainText('n/a');
  await expect(page.getByText('so the rows add up to twice the total')).toBeVisible();

  // The exporters, with what each sent and why one has no device.
  const exporters = page.getByRole('table', { name: 'Flow exporters' });
  const claimed = exporters.getByRole('row').filter({ hasText: 'CORE1' });
  await expect(claimed).toContainText('NetFlow v9');
  await expect(claimed).toContainText('1 in 100');
  const stranger = exporters.getByRole('row').filter({ hasText: '127.0.0.2' });
  await expect(stranger).toContainText('no device in the inventory has the address 127.0.0.2');
  await expect(stranger).toContainText('NetFlow v5');
  await expect(stranger).toContainText('none said');

  // One exporter alone — which is how one no device claims is looked at.
  await by.selectOption('source');
  await page.getByRole('combobox', { name: 'Exporter or device' }).selectOption('exporter:127.0.0.2');
  const alone = page.getByRole('table', { name: 'Top talkers by source' });
  await expect(alone.getByRole('row')).toHaveCount(2);
  await expect(alone.getByRole('row').nth(1)).toContainText('24.3 KiB');
  // That exporter said no sampling rate, so nothing is labelled sampled.
  await expect(page.getByTestId('flows-sampled')).toHaveCount(0);
  await expect(coverage.locator('[data-coverage="sampling"]')).toHaveCount(0);
  await expect(coverage.locator('[data-coverage="exporters"]')).toContainText('1');

  // The view is in the address.
  expect(page.url()).toContain('by=source');
  expect(page.url()).toContain('scope=exporter');

  expect(console.messages).toEqual([]);
});

test('with the collector off, an empty flows answer says “not collecting”, not “no traffic”', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  await rpc('settings.set', { section: 'logs', value: { flows: { enabled: false } } });
  // A period too short to reach back to the flows of the test above would be
  // an empty answer; what was counted stays in memory, so the default period
  // still holds it. The 5-minute one is used so the pane is asked afresh.
  await openApp(page, '/monitoring/logs?pane=flows&period=5', { backend });

  const coverage = page.getByRole('complementary', { name: 'What this answer covers' });
  await expect(coverage.locator('[data-coverage="collecting"]')).toContainText('Not listening');
  await expect(coverage.locator('[data-coverage="collecting"]')).toContainText('not “no traffic”');
  // What was counted before is still there: the buckets are the application's.
  await expect(page.getByRole('table', { name: 'Top talkers by source' })).toContainText('10.0.0.5');

  expect(console.messages).toEqual([]);
});

// =============================================================== detections

test('the catalogue states what each detection is decided from and what it cannot see', async ({
  page,
}) => {
  const console = watchConsole(page);
  await openApp(page, '/monitoring/logs?pane=detections', { backend });

  const catalogue = page.getByRole('list', { name: 'Detection catalogue' });
  await expect(catalogue.getByRole('listitem')).toHaveCount(7);
  const config = catalogue.locator('[data-detection="unplanned_config_change"]');
  await expect(config).toContainText('Unplanned configuration change');
  await expect(config).toContainText('high');
  await expect(config).toContainText('Watching');
  await expect(config).toContainText('Decided from');
  await expect(config).toContainText('`logs.config_changed`');
  await expect(config).toContainText('Cannot see');
  await expect(config).toContainText('A change on a device that sends no syslog');
  // There are no users, and the screen says so rather than hiding it — in
  // its own words above the catalogue, not only where a detection's blind
  // spot says it.
  await expect(page.locator('p').filter({ hasText: 'A short, fixed catalogue' })).toContainText(
    'Nocturne has one operator identity, not users',
  );
  // Nothing in this run presents a changed SSH host key: that detection has
  // never fired, and says so in words rather than with a date it does not have.
  const hostKey = catalogue.locator('[data-detection="host_key_changed"]');
  await expect(hostKey).toContainText('Watching');
  await expect(hostKey).toContainText('0 hits in 24 h');
  await expect(hostKey).toContainText('never fired');

  expect(console.messages).toEqual([]);
});

test('a lab switch changed from its console is a hit that appears as it happens, with its evidence', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  const device = await labSwitch(rpc, 'LogLab/SW2');
  await openApp(page, '/monitoring/logs?pane=detections', { backend });

  const hits = page.getByRole('table', { name: 'Recent detection hits' });
  // The console sessions and the syslog lines of earlier tests were hits too.
  // Count them, then make one more while the pane is open.
  await expect(hits.getByRole('row').nth(1)).toBeVisible({ timeout: 30_000 });
  const before = await hits.getByRole('row').count();
  await configureFromConsole(rpc, device, 'changed by hand, with no change in the pipeline');

  // `detections.hit` puts it in front of the list: nobody reloads.
  await expect(hits.getByRole('row')).toHaveCount(before + 1, { timeout: 30_000 });
  const newest = hits.getByRole('row').nth(1);
  await expect(newest).toContainText('Unplanned configuration change');
  await expect(newest).toContainText(
    'console changed LogLab/SW2 from console and no approved change was being applied',
  );
  await expect(newest).toContainText('high');
  // A lab device is a device, and its hit says it is a lab's.
  await expect(newest).toContainText('LAB');
  await expect(newest.getByRole('link', { name: 'LogLab/SW2' })).toHaveAttribute(
    'href',
    `#/devices/${device}`,
  );
  await expect(newest.getByRole('link', { name: 'In the alert list' })).toHaveAttribute(
    'href',
    '#/monitoring/alerts',
  );
  // The catalogue's count moved with it, to what the engine now says.
  const listed = await rpc<{ id: string; hits_24h: number }[]>('detections.list');
  const counted = listed.find((entry) => entry.id === 'unplanned_config_change')?.hits_24h ?? -1;
  expect(counted).toBeGreaterThan(0);
  await expect(
    page.locator('[data-detection="unplanned_config_change"]').getByRole('button', { name: /hits? in 24 h/ }),
  ).toHaveText(`${counted} ${counted === 1 ? 'hit' : 'hits'} in 24 h`);

  // The evidence: the records it was decided from, as text.
  await newest.getByRole('cell').first().dblclick();
  const drawer = page.getByRole('dialog', { name: 'Unplanned configuration change' });
  const evidence = drawer.getByRole('region', { name: 'Evidence' });
  await expect(evidence).toContainText('Decided from');
  await expect(evidence).toContainText('Log entry');
  await expect(evidence).toContainText('lab — handed over by a simulated device, in process');
  await expect(evidence).toContainText('By (as the device said)');
  await expect(evidence).toContainText('console');
  await expect(evidence).toContainText('15 min');
  // Who is told about a lab device's alert is the monitor's setting, and the
  // drawer says so rather than promising silence.
  await expect(drawer.getByTestId('hit-lab-note')).toContainText(
    'sent to no notification channel, unless Monitoring is set to notify about lab devices',
  );
  await expect(drawer.getByRole('link', { name: /Raised in the alert list/ })).toBeVisible();
  // The alert really is in the one alert list.
  const alerts = await rpc<{ rule_name: string; lab: boolean }[]>('alerts.list', {});
  expect(
    alerts.some((alert) => alert.rule_name === 'Detection: Unplanned configuration change' && alert.lab),
  ).toBe(true);
  await page.keyboard.press('Escape');

  expect(console.messages).toEqual([]);
});

test('a detection is switched off and on, and the engine’s refusal is shown where the switch is', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  await openApp(page, '/monitoring/logs?pane=detections', { backend });

  const secrets = page.locator('[data-detection="secrets_changed"]');
  const toggle = secrets.getByRole('switch');
  await expect(toggle).toBeChecked();
  await toggle.click();
  await expect(secrets).toContainText('Switched off', { timeout: 15_000 });
  await expect(toggle).not.toBeChecked();
  const settings = await rpc<{ disabled: string[] }>('settings.get', { section: 'detect' });
  expect(settings.disabled).toEqual(['secrets_changed']);

  // Detections are switched off as a whole, somewhere else. The catalogue
  // follows — and switching one on is refused by the engine, in a sentence.
  try {
    await rpc('settings.set', { section: 'detect', value: { enabled: false } });
    await expect(page.getByRole('note')).toContainText('Every detection reads as switched off', {
      timeout: 15_000,
    });
    await toggle.click();
    await expect(secrets.getByRole('alert')).toContainText(
      'detections are switched off as a whole (`enabled` in the `detect` settings); switch them on there before switching one on',
    );
    await expect(toggle).not.toBeChecked();
  } finally {
    await rpc('settings.set', { section: 'detect', value: { enabled: true } });
  }

  // Back on as a whole: the one that was switched off by hand still is.
  await expect(page.locator('[data-detection="unplanned_config_change"]')).toContainText('Watching', {
    timeout: 15_000,
  });
  await expect(secrets).toContainText('Switched off');
  await toggle.click();
  await expect(secrets).toContainText('Watching', { timeout: 15_000 });
  await expect(toggle).toBeChecked();

  // From the keyboard. The switch keeps the focus while its change is on its
  // way — a control that is disabled while it has the focus drops it to the
  // page — so a second press is a second change, not a press on nothing.
  await toggle.focus();
  await page.keyboard.press('Space');
  await expect(secrets).toContainText('Switched off', { timeout: 15_000 });
  await expect(toggle).toBeFocused();
  await page.keyboard.press('Space');
  await expect(secrets).toContainText('Watching', { timeout: 15_000 });
  await expect(toggle).toBeFocused();

  expect(console.messages).toEqual([]);
});

// ==================================================================== gates

test('every pane of this build is offered, each behind the method that fills it', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  // What this server registers decides what the tab offers. (The
  // combinations a full build cannot show — a build without `flows.*`, and
  // so on — are covered where they can be: `panes.test.ts`.)
  const methods = await rpc<string[]>('app.methods');
  const panes: readonly (readonly [label: string, method: string])[] = [
    ['Lines', 'logs.query'],
    ['Receivers', 'logs.status'],
    ['Rules', 'logs.rules.list'],
    ['Forwarding', 'logs.forward.list'],
    ['Flows', 'flows.top'],
    ['Detections', 'detections.list'],
  ];
  const expected = panes.filter(([, method]) => methods.includes(method)).map(([label]) => label);
  expect(expected).toHaveLength(6);

  await openApp(page, '/monitoring/logs', { backend });
  await expect(page.getByRole('tablist', { name: 'Logs' }).getByRole('tab')).toHaveText(expected);

  // The pane is in the address, and survives a reload.
  await openPane(page, 'Flows');
  expect(page.url()).toContain('pane=flows');
  await page.reload();
  await pageLoaded(page);
  await expect(page.getByRole('tab', { name: 'Flows' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByTestId('flows-not-verified')).toBeVisible();

  expect(console.messages).toEqual([]);
});

/**
 * The tab in a build that lacks features. A build without `flows.*` and the
 * rest cannot be started here, so the server is this one, with everything,
 * and its answer to `app.methods` has the absent features' names taken out
 * on its way to the page: the list such a build answers. Nothing else is
 * touched — every other call reaches the engine and comes back as it
 * answered — and a call the page makes to a method "this build" does not
 * have is recorded and fails the test: a control that makes one is dead.
 */
async function asIfWithout(page: Page, absent: (method: string) => boolean): Promise<string[]> {
  const called: string[] = [];
  await page.route('**/api/rpc', async (route) => {
    const { method } = route.request().postDataJSON() as { method: string };
    if (absent(method)) called.push(method);
    if (method !== 'app.methods') return route.continue();
    const response = await route.fetch();
    const json = (await response.json()) as { ok: boolean; result: string[] };
    return route.fulfill({ response, json: { ...json, result: json.result.filter((name) => !absent(name)) } });
  });
  return called;
}

test('a build without flows, detections or forwarding offers none of them, and calls none of them', async ({
  page,
}) => {
  const console = watchConsole(page);
  const called = await asIfWithout(
    page,
    (method) => method.startsWith('flows.') || method.startsWith('detections.') || method.startsWith('logs.forward.'),
  );
  await openApp(page, '/monitoring/logs', { backend });
  await expect(page.getByRole('tablist', { name: 'Logs' }).getByRole('tab')).toHaveText(['Lines', 'Receivers', 'Rules']);
  // A link to a pane this build does not have opens the first one there is.
  for (const pane of ['flows', 'detections', 'forwarding']) {
    await openApp(page, `/monitoring/logs?pane=${pane}`, { backend });
    await expect(page.getByRole('tablist', { name: 'Logs' }).getByRole('tab', { selected: true })).toHaveText('Lines');
  }
  await openPane(page, 'Receivers');
  await expect(receiver(page, 'Syslog')).toBeVisible();
  await openPane(page, 'Rules');
  await page.getByRole('table', { name: 'Log rules' }).getByRole('row').nth(1).getByRole('cell').first().dblclick();
  await expect(page.getByRole('dialog')).toBeVisible();
  await page.keyboard.press('Escape');

  expect(called).toEqual([]);
  expect(console.messages).toEqual([]);
});

test('a build that can list but not change shows every list and no control that would change it', async ({
  page,
}) => {
  const console = watchConsole(page);
  const writes = new Set([
    'logs.forward.save',
    'logs.forward.delete',
    'logs.forward.test',
    'logs.rules.save',
    'logs.rules.delete',
    'logs.rules.test',
    'detections.set_enabled',
    'detections.recent',
    'flows.exporters',
    'logs.restart',
    'settings.set',
    'logs.follow',
    'logs.pull',
    'logs.device_setup',
  ]);
  const called = await asIfWithout(page, (method) => writes.has(method));
  await openApp(page, '/monitoring/logs', { backend });
  await expect(page.getByRole('tablist', { name: 'Logs' }).getByRole('tab')).toHaveText([
    'Lines',
    'Receivers',
    'Rules',
    'Forwarding',
    'Flows',
    'Detections',
  ]);
  await expect(page.getByRole('switch', { name: 'Live tail' })).toHaveCount(0);

  const pane = page.locator('[role="tabpanel"][id^="logs-panel"]');
  await openPane(page, 'Receivers');
  await expect(receiver(page, 'Syslog')).toContainText('Listening');
  await expect(pane.getByRole('switch')).toHaveCount(0);
  await expect(pane.getByRole('button', { name: 'Restart receivers' })).toHaveCount(0);
  await expect(pane.getByRole('form')).toHaveCount(0);

  // Each absence is asserted once the pane has drawn what it does have: before
  // that, "no such button" is true of an empty page and proves nothing.
  await openPane(page, 'Rules');
  await expect(page.getByRole('table', { name: 'Log rules' }).getByRole('row').nth(1)).toBeVisible();
  await expect(page.getByRole('button', { name: 'New rule' })).toHaveCount(0);
  await expect(page.getByRole('table', { name: 'Log rules' }).getByRole('switch')).toHaveCount(0);
  await page.getByRole('table', { name: 'Log rules' }).getByRole('row').nth(1).getByRole('cell').first().dblclick();
  const rule = page.getByRole('dialog');
  await expect(rule).toBeVisible();
  await expect(rule.getByRole('button', { name: 'Save' })).toHaveCount(0);
  await expect(rule.getByRole('button', { name: 'Test the rule' })).toHaveCount(0);
  await page.keyboard.press('Escape');

  await openPane(page, 'Forwarding');
  await expect(page.getByText('Nocturne keeps thirty days of what the network says')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Add a forwarder' })).toHaveCount(0);
  await expect(page.getByRole('table', { name: 'Log forwarders' }).getByRole('button', { name: 'Row actions' })).toHaveCount(0);

  await openPane(page, 'Flows');
  await expect(page.getByRole('table', { name: 'Top talkers by source' })).toBeVisible();
  await expect(page.getByRole('table', { name: 'Flow exporters' })).toHaveCount(0);

  await openPane(page, 'Detections');
  await expect(page.getByRole('list', { name: 'Detection catalogue' }).getByRole('listitem')).toHaveCount(7);
  await expect(page.getByRole('list', { name: 'Detection catalogue' }).getByRole('switch')).toHaveCount(0);
  await expect(page.getByRole('table', { name: 'Recent detection hits' })).toHaveCount(0);

  expect(called).toEqual([]);
  expect(console.messages).toEqual([]);
});

// ======================================================== what is never shown

interface Traffic {
  kind: 'request' | 'answer' | 'event';
  method: string;
  text: string;
}

/** Every RPC call a page makes, every answer it gets and every event it is sent, in order. */
function recordTraffic(page: Page): Traffic[] {
  const seen: Traffic[] = [];
  const methodOf = (body: string | null) => {
    try {
      return (JSON.parse(body ?? '{}') as { method?: string }).method ?? '?';
    } catch {
      return '?';
    }
  };
  page.on('request', (request) => {
    if (request.url().includes('/api/rpc')) {
      seen.push({ kind: 'request', method: methodOf(request.postData()), text: request.postData() ?? '' });
    }
  });
  page.on('response', (response) => {
    if (!response.url().includes('/api/rpc')) return;
    // In the order the answers arrived; the text is filled in once it is read.
    const entry: Traffic = { kind: 'answer', method: methodOf(response.request().postData()), text: '' };
    seen.push(entry);
    response.text().then(
      (text) => {
        entry.text = text;
      },
      () => {
        entry.text = '<unread>';
      },
    );
  });
  page.on('websocket', (socket) => {
    socket.on('framereceived', (frame) => seen.push({ kind: 'event', method: 'event', text: String(frame.payload) }));
  });
  return seen;
}

/** The document's markup, every field's value, both storages and the address. */
function everythingOnThePage(page: Page): Promise<string> {
  return page.evaluate(() => {
    const dump = (store: Storage) =>
      Array.from({ length: store.length }, (_value, index) => {
        const key = store.key(index) ?? '';
        return `${key}=${store.getItem(key) ?? ''}`;
      }).join('\n');
    const fields = [...document.querySelectorAll('input, textarea, select')].map(
      (field) => (field as HTMLInputElement).value,
    );
    return [
      document.documentElement.outerHTML,
      fields.join('\n'),
      dump(window.localStorage),
      dump(window.sessionStorage),
      window.location.href,
    ].join('\n');
  });
}

test('once a forwarder is saved, its URL, its token and any community are nowhere the page can reach', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  // Distinctive, so that finding one is never a coincidence. The two
  // communities are the ones the trap tests sent with.
  const secrets: Record<string, string> = {
    'the token in the URL': 'url-token-Narwhal-9913',
    'the token': 'Tok-Pangolin-4477-ffff',
    'the stored community': COMMUNITY,
    'the wrong community': WRONG_COMMUNITY,
  };
  const found = (text: string) =>
    Object.entries(secrets)
      .filter(([, value]) => text.includes(value))
      .map(([name]) => name)
      .sort();
  const traffic = recordTraffic(page);
  await openApp(page, '/monitoring/logs?pane=forwarding', { backend });

  await page.getByRole('button', { name: 'Add a forwarder' }).first().click();
  const drawer = page.getByRole('dialog', { name: 'New forwarder' });
  await drawer.getByLabel('Name').fill('Vault test');
  await drawer.getByLabel('Kind').selectOption('https_json');
  await drawer
    .getByRole('textbox', { name: 'URL', exact: true })
    .fill(`https://127.0.0.1:9/services/collector/event?token=${secrets['the token in the URL']}`);
  await drawer.getByRole('textbox', { name: 'Token', exact: true }).fill(secrets['the token'] ?? '');
  // While they are being typed the page holds them: the search below is not blind.
  expect(found(await everythingOnThePage(page))).toEqual(['the token', 'the token in the URL']);
  await drawer.getByRole('button', { name: 'Save' }).click();
  await expect(drawer).toHaveCount(0);
  const save = traffic.findIndex((entry) => entry.kind === 'request' && entry.method === 'logs.forward.save');
  // Nor is the recording: the save carried both.
  expect(found(traffic[save]?.text ?? '')).toEqual(['the token', 'the token in the URL']);

  // Every pane, every stream, and every drawer that shows something stored.
  const searched: string[] = [];
  const search = async (where: string) => {
    searched.push(where);
    expect(found(await everythingOnThePage(page)), where).toEqual([]);
  };
  await search('the forwarders, after the save');
  await forwarderRow(page, 'Vault test').getByRole('cell').first().dblclick();
  const edit = page.getByRole('dialog', { name: 'Edit Vault test' });
  await expect(edit).toContainText('Stored in the vault. It cannot be shown again.');
  await search('the edit form of the saved forwarder');
  await edit.getByRole('button', { name: 'Replace…' }).first().click();
  await expect(edit.getByRole('textbox', { name: 'URL', exact: true })).toHaveValue('');
  await search('the edit form, replacing the URL');
  await page.keyboard.press('Escape');
  for (const stream of ['syslog', 'trap', 'event', 'audit', '*']) {
    await openApp(page, `/monitoring/logs?stream=${encodeURIComponent(stream)}&span=24h`, { backend });
    await expect(page.getByTestId('log-scroller')).toBeVisible();
    await search(`the ${stream} stream`);
    const count = Math.min(await rows(page).count(), stream === 'trap' ? 10 : 1);
    for (let index = 0; index < count; index += 1) {
      await rows(page).nth(index).getByRole('button').click();
      await expect(page.getByRole('dialog')).toBeVisible();
      await search(`the ${stream} stream, entry ${index}, in its drawer`);
      await page.keyboard.press('Escape');
    }
  }
  const filled: Record<string, Locator> = {
    receivers: receiver(page, 'Flows'),
    rules: page.getByRole('table', { name: 'Log rules' }).getByRole('row').nth(1),
    flows: page.getByRole('table', { name: 'Flow exporters' }),
    detections: page.getByRole('table', { name: 'Recent detection hits' }).getByRole('row').nth(1),
  };
  for (const [pane, sign] of Object.entries(filled)) {
    await openApp(page, `/monitoring/logs?pane=${pane}`, { backend });
    await expect(sign).toBeVisible();
    await search(`the ${pane} pane`);
  }
  await page.getByRole('table', { name: 'Recent detection hits' }).getByRole('row').nth(1).getByRole('cell').first().dblclick();
  await expect(page.getByRole('dialog')).toBeVisible();
  await search('a hit in its drawer');
  await page.keyboard.press('Escape');
  expect(searched.length).toBeGreaterThan(10);

  // Nothing the page sent or was sent after the save holds either half of
  // the destination, and nothing it sent or was sent at all holds a community.
  await expect.poll(() => traffic.every((entry) => entry.kind !== 'answer' || entry.text !== '')).toBe(true);
  const leaked = traffic
    .slice(save + 1)
    .flatMap((entry) => found(entry.text).map((name) => `${entry.kind} ${entry.method}: ${name}`));
  expect(leaked).toEqual([]);
  const communities = traffic.flatMap((entry) =>
    found(entry.text)
      .filter((name) => name.endsWith('community'))
      .map((name) => `${entry.kind} ${entry.method}: ${name}`),
  );
  expect(communities).toEqual([]);

  const saved = (await rpc<{ id: string; name: string }[]>('logs.forward.list')).find(
    (forwarder) => forwarder.name === 'Vault test',
  );
  if (saved) await rpc('logs.forward.delete', { id: saved.id });
  expect(console.messages).toEqual([]);
});

// ================================================================ hostile text

/** One BER element: its tag, its length, and what it holds. */
function ber(tag: number, content: Buffer): Buffer {
  const length = content.length;
  const head =
    length < 0x80
      ? [length]
      : length < 0x100
        ? [0x81, length]
        : length < 0x10000
          ? [0x82, length >> 8, length & 0xff]
          : [0x83, length >> 16, (length >> 8) & 0xff, length & 0xff];
  return Buffer.concat([Buffer.from([tag, ...head]), content]);
}

function berOid(dotted: string): Buffer {
  const [first = 0, second = 0, ...rest] = dotted.split('.').map(Number);
  const bytes = [first * 40 + second];
  for (const part of rest) {
    const groups = [part & 0x7f];
    for (let value = part >> 7; value > 0; value >>= 7) groups.unshift((value & 0x7f) | 0x80);
    bytes.push(...groups);
  }
  return ber(0x06, Buffer.from(bytes));
}

/** Markup a device could put in its text: it must stay text. */
const MARKUP = '<script>window.__pwned=1</script><img src=x onerror="window.__pwned=2"><b>SW</b> ‮esrever‬';

/**
 * An SNMPv2c linkDown **written by hand** to RFC 3416, with CORE1's community
 * and two string bindings any agent can send: markup, and a run of 50,000
 * characters. No agent sent it; what is tested is what the screen does with
 * what the network says.
 */
function hostileTrap(): Buffer {
  const sequence = (...items: Buffer[]) => ber(0x30, Buffer.concat(items));
  const integer = (value: number) => ber(0x02, Buffer.from([value]));
  const text = (value: string) => ber(0x04, Buffer.from(value, 'utf8'));
  const binding = (oid: string, value: Buffer) => sequence(berOid(oid), value);
  return sequence(
    integer(1),
    text(COMMUNITY),
    ber(
      0xa7,
      Buffer.concat([
        integer(7),
        integer(0),
        integer(0),
        sequence(
          binding('1.3.6.1.2.1.1.3.0', ber(0x43, Buffer.from([0x30, 0x39]))),
          binding('1.3.6.1.6.3.1.1.4.1.0', berOid('1.3.6.1.6.3.1.1.5.3')),
          binding('1.3.6.1.2.1.2.2.1.1.9', integer(9)),
          binding('1.3.6.1.2.1.31.1.1.1.18.9', text(MARKUP)),
          binding('1.3.6.1.4.1.99999.1.1', text('Y'.repeat(50_000))),
        ),
      ]),
    ),
  );
}

/** Whether anything the network said became markup or ran as script. */
async function inert(page: Page, where: string): Promise<void> {
  const state = await page.evaluate(() => ({
    ran: (window as unknown as { __pwned?: unknown }).__pwned ?? null,
    images: document.querySelectorAll('img[src="x"]').length,
    bold: [...document.querySelectorAll('b')].filter((element) => element.textContent === 'SW').length,
    sideways: document.documentElement.scrollWidth - window.innerWidth,
  }));
  expect(state, where).toEqual({ ran: null, images: 0, bold: 0, sideways: 0 });
}

test('a log line, a trap binding and an exporter’s name full of markup stay text, and nothing freezes', async ({
  page,
  request,
}) => {
  const console = watchConsole(page);
  const rpc = rpcFor(request, backend);
  // Seconds, not milliseconds, so that a busy machine does not fail it; what
  // is measured is printed, and a frozen table would take minutes.
  const LIMIT_MS = 10_000;
  const took: Record<string, number> = {};
  const timed = async (what: string, step: () => Promise<void>) => {
    const started = Date.now();
    await step();
    took[what] = Date.now() - started;
  };

  await send([
    `<187>77: *Nov 14 01:02:03.004: %SYS-3-HOSTILE: ${MARKUP} \x1b[31mRED\x1b[0m \x1b]0;owned\x07 ${'Z'.repeat(50_000)}`,
  ]);
  const trapsBefore = (await rpc<Status>('logs.status')).traps.stored;
  await sendTo(await listeningPort(rpc, 'traps'), [hostileTrap()]);
  await expect.poll(async () => (await rpc<Status>('logs.status')).traps.stored, { timeout: 30_000 }).toBe(trapsBefore + 1);
  const exporterName = '<img src=x onerror="window.__pwned=3"><b>SW</b> edge';
  await rpc('devices.create', { name: exporterName, address: '127.0.0.3', profile: 'cisco_ios', site: 'HQ', role: 'edge' });
  await rpc('settings.set', { section: 'logs', value: { flows: { enabled: true } } });
  await sendTo(
    await listeningPort(rpc, 'flows'),
    [v5(0, [{ ...FLOWS[0], src: [10, 3, 3, 3], input: 7, output: 9 } as V5Record])],
    { from: '127.0.0.3' },
  );
  await expect
    .poll(async () => (await rpc<{ device: string | null }[]>('flows.exporters')).some((entry) => entry.device === exporterName), {
      timeout: 30_000,
    })
    .toBe(true);

  // The line: one row, one line high, and its markup is text.
  const line = rows(page).filter({ hasText: 'SYS-3-HOSTILE' }).first();
  await timed('the lines, with the hostile line', async () => {
    await openApp(page, '/monitoring/logs?q=mnemonic%3ASYS-3-HOSTILE&span=24h', { backend });
    await expect(line).toContainText('<script>window.__pwned=1</script>');
  });
  await inert(page, 'the hostile line in the list');
  expect(await line.getByRole('button').evaluate((element) => element.getBoundingClientRect().height)).toBe(22);
  const lineDrawer = page.getByRole('dialog', { name: 'SYS-3-HOSTILE' });
  await timed('the line in its drawer', async () => {
    await line.getByRole('button').click();
    await expect(lineDrawer).toContainText('ZZZZZZZZZZZZZZZZZZZZ');
  });
  await inert(page, 'the hostile line in its drawer');
  // The escapes are characters of the line, not instructions to anything.
  await expect(lineDrawer).toContainText('[31mRED');
  expect(await lineDrawer.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(0);
  await page.keyboard.press('Escape');

  // The trap: its markup is a binding's text, its run is cut where the engine cuts it.
  const trap = rows(page).filter({ hasText: 'ifIndex=9' }).first();
  await timed('the trap stream, with the hostile trap', async () => {
    await openApp(page, '/monitoring/logs?stream=trap&span=24h', { backend });
    await expect(trap).toContainText('window.__pwned=1');
  });
  await inert(page, 'the hostile trap in the list');
  const trapDrawer = page.getByRole('dialog', { name: 'linkDown' });
  await timed('the trap in its drawer', async () => {
    await trap.getByRole('button').click();
    await expect(trapDrawer.getByRole('region', { name: 'Bindings' })).toContainText('YYYYYYYYYYYYYYYYYYYY');
  });
  await inert(page, 'the hostile trap in its drawer');
  await expect(trapDrawer.getByRole('region', { name: 'Bindings' })).toContainText(
    '<script>window.__pwned=1</script><img src=x onerror="window.__pwned=2"><b>SW</b>',
  );
  expect(await trapDrawer.evaluate((element) => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(0);
  await page.keyboard.press('Escape');

  // The exporter: its name is the device's, and the device's name is text.
  await timed('the flows, with the hostile exporter', async () => {
    await openApp(page, '/monitoring/logs?pane=flows&period=60', { backend });
    await expect(page.getByRole('table', { name: 'Flow exporters' })).toContainText(exporterName);
  });
  await inert(page, 'the flows pane');
  await page.getByRole('combobox', { name: 'Add up by' }).selectOption('interface');
  await expect(page.getByRole('table', { name: 'Top talkers by interface' })).toContainText(`${exporterName} ifIndex 7 in`);
  await inert(page, 'the flows by interface');
  expect(await page.getByRole('combobox', { name: 'Exporter or device' }).locator('option').allInnerTexts()).toContain(
    exporterName,
  );

  globalThis.console.log('hostile text, milliseconds:', JSON.stringify(took));
  for (const [what, ms] of Object.entries(took)) expect(ms, what).toBeLessThan(LIMIT_MS);
  expect(console.messages).toEqual([]);
});
