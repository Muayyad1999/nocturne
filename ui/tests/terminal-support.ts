/**
 * What the Terminal specs need: a device to talk to, and an engine that knows
 * about it.
 *
 * The engine is the real `nocturne-server` (`support/backend.ts`). The device
 * is a small TCP console served from the test process itself — the Telnet
 * client talks to raw TCP services on purpose, which is what console servers
 * are. It answers a few commands, so a spec can make the "device" print
 * exactly what it wants to see on screen, including a few megabytes of it.
 *
 * Not a mock of the UI: everything between the key press and the bytes on
 * screen is the real thing — RPC, the event stream, the attach protocol.
 */

import { connect, createServer, type Server, type Socket } from 'node:net';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { homedir, tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { promisify } from 'node:util';
import type { Page } from '@playwright/test';
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

/** A line of the `lines` command: wide enough that a burst is megabytes. */
const PADDING = 'the quick brown fox jumps over the lazy dog';

export interface FakeDevice {
  readonly port: number;
  /** How many consoles are connected right now. */
  readonly connections: number;
  /**
   * The command lines the console has run, in order, on every connection.
   * What is on screen cannot tell a paste that ran once from one that ran
   * twice and scrolled, or one that ran before it was allowed to; this can.
   *
   * Every connection means the engine's too: it logs in on its own to find
   * out what kind of device this is, and those lines are here as well. Use
   * `said` for what a test sent.
   */
  readonly executed: readonly string[];
  /**
   * The `say …` lines among `executed`. A test only ever sends those, and
   * the engine's own probing never does, so this is what the person at the
   * terminal made the device do.
   */
  readonly said: readonly string[];
  /**
   * Every byte one console was sent, as text, Telnet's own framing removed:
   * the console that was sent `marker`, so the engine's probing on another
   * connection is not mixed in. Null until some console has seen the marker.
   *
   * The console treats a carriage return and a line feed alike, as devices
   * do, so only this can show which of the two — or both — arrived.
   */
  receivedWith(marker: string): string | null;
  /** What arrived inside paste brackets, one entry per bracketed paste. */
  readonly bracketed: readonly string[];
  /** Forgets all of the above, so a test reads only what it did itself. */
  forget(): void;
  /** Drops every connection, as a device does when it is rebooted. */
  hangUp(): void;
  stop(): Promise<void>;
}

/** What a terminal puts around a paste when the far end has asked for it. */
const PASTE_START = '\u001b[200~';
const PASTE_END = '\u001b[201~';

/**
 * A console that echoes what is typed and answers:
 *
 *   show version   two lines of facts
 *   say <text>     prints `[<text>]` once — the brackets keep the answer
 *                  apart from the echo of the command itself
 *   lines <n>      prints `line NNNNN …`, n of them, as fast as it can
 *   link <url>     prints the URL on a line of its own
 *   brackets on    asks the terminal to bracket pastes, as a shell does;
 *                  a bracketed paste is then kept (`bracketed`) and not run
 *   anything else  `% Unknown command: …`
 */
export async function startFakeDevice(): Promise<FakeDevice> {
  const sockets = new Set<Socket>();
  let executed: string[] = [];
  /** What each console was sent; a console is one connection. */
  let received: { text: string }[] = [];
  let bracketed: string[] = [];

  const server: Server = createServer((socket) => {
    sockets.add(socket);
    socket.setNoDelay(true);
    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => sockets.delete(socket));
    socket.write('\r\nFakeOS 1.0 console\r\nSW1> ');

    let line = '';
    let input = { text: '' };
    received.push(input);
    /** An escape sequence that may yet turn out to be a paste bracket. */
    let escape = '';
    /** The paste being collected between its brackets, or null outside one. */
    let paste: string | null = null;

    const take = (character: string) => {
      if (paste !== null) {
        paste += character;
        return;
      }
      if (character === '\r' || character === '\n') {
        socket.write('\r\n');
        const command = line.trim();
        if (command !== '') executed.push(command);
        answer(socket, command);
        line = '';
        socket.write('SW1> ');
      } else if (character === '\u007f' || character === '\b') {
        if (line !== '') {
          line = line.slice(0, -1);
          socket.write('\b \b');
        }
      } else {
        line += character;
        socket.write(character);
      }
    };

    socket.on('data', (buffer) => {
      for (const byte of buffer) {
        // Telnet commands, and the NUL that follows a carriage return.
        if (byte === 0 || byte === 255) continue;
        const character = String.fromCharCode(byte);
        if (!received.includes(input)) {
          // Forgotten between two tests while this console stayed connected.
          input = { text: '' };
          received.push(input);
        }
        input.text += character;

        if (escape !== '' || character === '\u001b') {
          escape += character;
          if (escape === PASTE_START) {
            paste = '';
            escape = '';
          } else if (escape === PASTE_END) {
            if (paste !== null) bracketed.push(paste);
            paste = null;
            escape = '';
          } else if (!PASTE_START.startsWith(escape) && !PASTE_END.startsWith(escape)) {
            // Not a bracket after all: it was ordinary input.
            for (const held of escape) take(held);
            escape = '';
          }
          continue;
        }
        take(character);
      }
    });
  });

  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('the fake device got no port');

  return {
    port: address.port,
    get connections() {
      return sockets.size;
    },
    get executed() {
      return executed;
    },
    get said() {
      return executed.filter((line) => line.startsWith('say '));
    },
    receivedWith(marker) {
      return received.find((input) => input.text.includes(marker))?.text ?? null;
    },
    get bracketed() {
      return bracketed;
    },
    forget() {
      executed = [];
      received = [];
      bracketed = [];
    },
    hangUp() {
      for (const socket of sockets) socket.destroy();
      sockets.clear();
    },
    async stop() {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((done) => server.close(() => done()));
    },
  };
}

function answer(socket: Socket, command: string): void {
  if (command === '') return;
  if (command === 'show version') {
    socket.write('FakeOS Software, Version 1.0\r\nUptime is 4 days, 2 hours\r\n');
    return;
  }
  if (command.startsWith('say ')) {
    socket.write(`[${command.slice(4)}]\r\n`);
    return;
  }
  if (command.startsWith('link ')) {
    socket.write(`${command.slice(5)}\r\n`);
    return;
  }
  if (command === 'brackets on') {
    // DECSET 2004, exactly as bash and zsh send it before a prompt.
    socket.write('\u001b[?2004h');
    return;
  }
  if (command.startsWith('lines ')) {
    const count = Number(command.slice(6).trim());
    if (Number.isFinite(count) && count > 0) {
      let out = '';
      for (let index = 0; index < count; index += 1) {
        out += `line ${String(index).padStart(5, '0')} ${PADDING}\r\n`;
      }
      socket.write(out);
      return;
    }
  }
  socket.write(`% Unknown command: ${command}\r\n`);
}

export interface TerminalEngine {
  readonly backend: Backend;
  readonly device: FakeDevice;
  /** The inventory id of the device the console belongs to. */
  readonly deviceId: string;
  /** Calls the engine directly, the way the UI's transport does. */
  rpc: <T>(method: string, params?: unknown) => Promise<T>;
  stop(): Promise<void>;
}

/**
 * Builds the UI, starts the engine on an empty data directory with the fake
 * device in its inventory, and hands back both.
 *
 * The UI is built outside `ui/dist`: the server crate embeds that directory,
 * so writing to it would make cargo rebuild the binary first.
 */
export async function startTerminalEngine(): Promise<TerminalEngine> {
  const device = await startFakeDevice();
  const uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
  let backend: Backend | undefined;
  try {
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 180_000,
    });
    backend = await startBackend({ uiDir });

    const engine = backend;
    const rpc = async <T>(method: string, params?: unknown): Promise<T> => {
      const response = await fetch(`${engine.url}/api/rpc`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${engine.token}` },
        body: JSON.stringify({ method, params: params ?? null }),
      });
      const envelope = (await response.json()) as
        | { ok: true; result: T }
        | { ok: false; error: { code: string; message: string } };
      if (!envelope.ok) throw new Error(`${method}: ${envelope.error.code} — ${envelope.error.message}`);
      return envelope.result;
    };

    const created = await rpc<{ id: string }>('devices.create', {
      name: 'SW1-LAB',
      address: '127.0.0.1',
      port: device.port,
      transport: 'telnet',
      site: 'Lab',
      role: 'access',
    });

    return {
      backend: engine,
      device,
      deviceId: created.id,
      rpc,
      async stop() {
        await engine.stop();
        await device.stop();
        await rm(uiDir, { recursive: true, force: true });
      },
    };
  } catch (error) {
    await backend?.stop();
    await device.stop();
    await rm(uiDir, { recursive: true, force: true });
    throw error;
  }
}

// ------------------------------------------------------------ lost output

interface OutputLoss {
  losing: boolean;
  lost: number;
  /** Hands the interface a frame as if the engine had sent it. */
  deliver: ((frame: string) => void) | null;
}

/**
 * Lets a test lose `terminal.output` events on their way to the interface,
 * on a connection that stays open. That is what the engine does to a client
 * that has fallen too far behind: it drops events, says `events.lagged` on
 * the same socket and carries on. Making a browser fall 1024 events behind
 * takes a stalled network; losing the frames at the browser's door is the
 * same thing as far as the interface can tell, and it happens every time.
 *
 * Call before the application is opened.
 */
export async function loseOutputOnRequest(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const loss: OutputLoss = { losing: false, lost: 0, deliver: null };
    Object.defineProperty(window, '__output', { value: loss });
    const own = Object.getOwnPropertyDescriptor(WebSocket.prototype, 'onmessage');
    if (!own?.get || !own.set) throw new Error('this browser keeps WebSocket.onmessage somewhere else');
    const { get, set } = own;
    Object.defineProperty(WebSocket.prototype, 'onmessage', {
      configurable: true,
      get(this: WebSocket) {
        return get.call(this) as unknown;
      },
      set(this: WebSocket, handler: ((message: MessageEvent) => void) | null) {
        if (!handler) {
          set.call(this, handler);
          return;
        }
        loss.deliver = (frame) => handler.call(this, new MessageEvent('message', { data: frame }));
        set.call(this, (message: MessageEvent<unknown>) => {
          if (loss.losing && typeof message.data === 'string') {
            const topic = (JSON.parse(message.data) as { topic?: unknown }).topic;
            if (topic === 'terminal.output') {
              loss.lost += 1;
              return;
            }
          }
          handler.call(this, message);
        });
      },
    });
  });
}

/** Starts or stops losing output events; answers how many have been lost so far. */
export function losingOutput(page: Page, losing: boolean): Promise<number> {
  return page.evaluate((value) => {
    const loss = (window as unknown as { __output: OutputLoss }).__output;
    loss.losing = value;
    return loss.lost;
  }, losing);
}

/**
 * Waits until output has been lost and no more is being lost: what the device
 * printed has arrived at the browser's door and gone. A fixed sleep would be
 * a guess at how long the machine takes; a late frame after losing has been
 * switched off would arrive as a hole and be repaired, and the test would
 * then be about the wrong thing.
 */
export async function outputLossSettled(page: Page, moreThan = 0): Promise<number> {
  const deadline = Date.now() + 15_000;
  let before = -1;
  while (Date.now() < deadline) {
    const lost = await page.evaluate(() => (window as unknown as { __output: OutputLoss }).__output.lost);
    if (lost > moreThan && lost === before) return lost;
    before = lost;
    await page.waitForTimeout(250);
  }
  throw new Error(`no output was lost beyond the ${moreThan} events lost before: the device printed nothing, or losing is off`);
}

/** The engine's own notice that it dropped events for this client. */
export async function announceLag(page: Page, missed: number): Promise<void> {
  await page.evaluate((count) => {
    const loss = (window as unknown as { __output: OutputLoss }).__output;
    if (!loss.deliver) throw new Error('the event stream is not open');
    loss.deliver(JSON.stringify({ topic: 'events.lagged', payload: { missed: count }, at: Date.now() }));
  }, missed);
}

// ------------------------------------------------------- the desktop shell

/** What the stand-in shell was asked for, and what the page tried on its own. */
export interface ShellRecord {
  /** The text `plugin:clipboard-manager|read_text` answers with; null is an empty clipboard. */
  clipboard: string | null;
  /** Addresses handed to `plugin:opener|open_url`. */
  opened: string[];
  /** Times the page read the WebView's own clipboard, which refuses. */
  pageClipboardReads: number;
  /** Times the page called `window.open`, which goes nowhere. */
  windowOpens: number;
}

/**
 * Loads the interface as the desktop shell presents it, in the respects a
 * terminal can tell apart from a browser:
 *
 *   * `window.__TAURI_INTERNALS__` exists, so calls go through `invoke('rpc')`
 *     and events arrive from the event plugin — both are passed on to the
 *     real engine, so the session is as real as in every other test here;
 *   * a script may not read the clipboard (WebKitGTK refuses outright);
 *   * `window.open` returns nothing and opens nothing;
 *   * the shell lends two plugin commands, `plugin:clipboard-manager|read_text`
 *     and `plugin:opener|open_url`, and rejects any other, as its capability
 *     file does.
 *
 * Not the shell itself — that needs a WebView and a display — but the three
 * differences that left Paste and links dead in the installed program while
 * every browser test passed. What the page asked for is in `window.__shell`.
 */
export async function openInDesktopShell(page: Page, backend: Backend, route: string): Promise<void> {
  await page.addInitScript(
    ({ url, token }: { url: string; token: string }) => {
      const shell: ShellRecord = { clipboard: null, opened: [], pageClipboardReads: 0, windowOpens: 0 };
      Object.defineProperty(window, '__shell', { value: shell });

      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: {
          readText: () => {
            shell.pageClipboardReads += 1;
            return Promise.reject(new DOMException('The request is not allowed.', 'NotAllowedError'));
          },
          writeText: () => Promise.resolve(),
        },
      });
      window.open = () => {
        shell.windowOpens += 1;
        return null;
      };

      const invoke = async (command: string, args: Record<string, unknown>): Promise<unknown> => {
        if (command === 'plugin:event|listen') {
          // The shell forwards the engine's events on one channel; here they
          // come off the engine's own stream.
          const handler = args.handler as (message: { event: unknown; id: number; payload: unknown }) => void;
          const socket = new WebSocket(`${url.replace(/^http/, 'ws')}/api/events?token=${token}`);
          socket.onmessage = (message: MessageEvent<string>) => {
            handler({ event: args.event, id: 1, payload: JSON.parse(message.data) });
          };
          await new Promise((opened) => {
            socket.onopen = opened;
          });
          return 1;
        }
        if (command.startsWith('plugin:event|')) return null;
        if (command === 'plugin:clipboard-manager|read_text') {
          // The plugin rejects with its reason, as a sentence.
          if (shell.clipboard === null) {
            throw 'The clipboard contents were not available in the requested format or the clipboard is empty.';
          }
          return shell.clipboard;
        }
        if (command === 'plugin:opener|open_url') {
          shell.opened.push(String(args.url));
          return null;
        }
        if (command !== 'rpc') throw `${command} not allowed by the capability file`;

        const reply = await fetch(`${url}/api/rpc`, {
          method: 'POST',
          headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` },
          body: JSON.stringify({ method: args.method, params: args.params ?? null }),
        });
        const envelope = (await reply.json()) as { ok: true; result: unknown } | { ok: false; error: unknown };
        // Tauri resolves with the `Ok` value and rejects with the `Err`.
        if (envelope.ok) return envelope.result;
        throw envelope.error;
      };
      Object.defineProperty(window, '__TAURI_INTERNALS__', {
        value: { invoke, transformCallback: (callback: unknown) => callback },
        configurable: true,
      });
    },
    { url: backend.url, token: backend.token },
  );
  await page.goto(`${backend.url}/#${route}`);
}

/** What the page has asked the stand-in shell for so far. */
export function shellRecord(page: Page): Promise<ShellRecord> {
  return page.evaluate(() => ({ ...(window as unknown as { __shell: ShellRecord }).__shell }));
}

/** Puts text on the clipboard the stand-in shell reads; null empties it. */
export async function setShellClipboard(page: Page, text: string | null): Promise<void> {
  await page.evaluate((value) => {
    (window as unknown as { __shell: ShellRecord }).__shell.clipboard = value;
  }, text);
}

// ------------------------------------------------------------------- sshd

export interface Sshd {
  readonly port: number;
  readonly user: string;
  /** The private key that gets in, in OpenSSH format. */
  readonly privateKey: string;
  /** Where a session lands: the account's own home directory. */
  readonly home: string;
  /** A file put there, so a listing has something of ours to show. */
  readonly file: string;
  stop(): Promise<void>;
}

/**
 * A private OpenSSH daemon on a free port, with its own host key, its own
 * `authorized_keys` and its own home directory — the only way to talk real
 * SFTP. Where one cannot be started (no binary, no privilege-separation
 * directory, a locked-down container) this says why and the SFTP test skips:
 * it is an interoperability check, not a reason to fail a build.
 */
export async function startSshd(): Promise<Sshd | string> {
  const sshd = ['/usr/sbin/sshd', '/usr/local/sbin/sshd', '/sbin/sshd'].find((path) => existsSync(path));
  const subsystem = [
    '/usr/lib/openssh/sftp-server',
    '/usr/libexec/openssh/sftp-server',
    '/usr/libexec/sftp-server',
    '/usr/lib/ssh/sftp-server',
  ].find((path) => existsSync(path));
  if (!sshd) return 'no sshd on this machine';
  if (!subsystem) return 'no sftp-server on this machine';
  if (!existsSync('/run/sshd')) {
    try {
      await mkdir('/run/sshd', { recursive: true });
    } catch {
      return '/run/sshd does not exist and cannot be created (sshd needs it)';
    }
  }

  const root = await mkdtemp(join(tmpdir(), 'nocturne-sshd-'));
  const keygen = async (name: string) => {
    await run('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-C', 'nocturne-ui-test', '-f', join(root, name)]);
    return readFile(join(root, name), 'utf8');
  };

  try {
    await keygen('host_ed25519');
    const privateKey = await keygen('client_ed25519');
    const publicKey = await readFile(join(root, 'client_ed25519.pub'), 'utf8');
    await writeFile(join(root, 'authorized_keys'), publicKey);

    // sshd puts a session in the account's own home directory, so the file
    // the test looks for has to be there. It is named for what it is and
    // removed again by `stop()`.
    const home = homedir();
    const file = `nocturne-ui-test-${process.pid}.cfg`;
    await writeFile(join(home, file), 'hostname SW1\n!\n');

    const port = await freePort();
    await writeFile(
      join(root, 'sshd_config'),
      [
        `Port ${port}`,
        'ListenAddress 127.0.0.1',
        `HostKey ${join(root, 'host_ed25519')}`,
        'PidFile none',
        'StrictModes no',
        'UsePAM no',
        'PasswordAuthentication no',
        'KbdInteractiveAuthentication no',
        'PubkeyAuthentication yes',
        'PermitRootLogin prohibit-password',
        `AuthorizedKeysFile ${join(root, 'authorized_keys')}`,
        `Subsystem sftp ${subsystem}`,
        'LogLevel ERROR',
        '',
      ].join('\n'),
    );

    const child = spawn(sshd, ['-D', '-e', '-f', join(root, 'sshd_config')], { stdio: ['ignore', 'ignore', 'pipe'] });
    let complaints = '';
    child.stderr?.on('data', (chunk: Buffer) => {
      complaints += chunk.toString();
    });

    const listening = await waitForPort(port, child);
    if (!listening) {
      child.kill('SIGKILL');
      await rm(root, { recursive: true, force: true });
      return `sshd would not listen: ${complaints.trim().replace(/\n/g, '; ') || 'no reason given'}`;
    }

    return {
      port,
      user: process.env.USER ?? 'root',
      privateKey,
      home,
      file,
      async stop() {
        child.kill('SIGTERM');
        await rm(join(home, file), { force: true });
        await rm(root, { recursive: true, force: true });
      },
    };
  } catch (error) {
    await rm(root, { recursive: true, force: true });
    return `sshd could not be set up: ${error instanceof Error ? error.message : String(error)}`;
  }
}

async function freePort(): Promise<number> {
  const server = createServer();
  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const address = server.address();
  const port = address !== null && typeof address !== 'string' ? address.port : 0;
  await new Promise<void>((done) => server.close(() => done()));
  if (port === 0) throw new Error('no free port');
  return port;
}

async function waitForPort(port: number, child: ChildProcess): Promise<boolean> {
  const deadline = Date.now() + 10_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) return false;
    const open = await new Promise<boolean>((resolve) => {
      const probe = connect({ port, host: '127.0.0.1' });
      probe.once('connect', () => {
        probe.destroy();
        resolve(true);
      });
      probe.once('error', () => resolve(false));
    });
    if (open) return true;
    await new Promise((wait) => setTimeout(wait, 50));
  }
  return false;
}
