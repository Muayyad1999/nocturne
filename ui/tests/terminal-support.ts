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
import { startBackend, type Backend } from './support/backend';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

/** A line of the `lines` command: wide enough that a burst is megabytes. */
const PADDING = 'the quick brown fox jumps over the lazy dog';

export interface FakeDevice {
  readonly port: number;
  /** How many consoles are connected right now. */
  readonly connections: number;
  /** Drops every connection, as a device does when it is rebooted. */
  hangUp(): void;
  stop(): Promise<void>;
}

/**
 * A console that echoes what is typed and answers:
 *
 *   show version   two lines of facts
 *   say <text>     prints `[<text>]` once — the brackets keep the answer
 *                  apart from the echo of the command itself
 *   lines <n>      prints `line NNNNN …`, n of them, as fast as it can
 *   anything else  `% Unknown command: …`
 */
export async function startFakeDevice(): Promise<FakeDevice> {
  const sockets = new Set<Socket>();

  const server: Server = createServer((socket) => {
    sockets.add(socket);
    socket.setNoDelay(true);
    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => sockets.delete(socket));
    socket.write('\r\nFakeOS 1.0 console\r\nSW1> ');

    let line = '';
    socket.on('data', (buffer) => {
      for (const byte of buffer) {
        // Telnet commands, and the NUL that follows a carriage return.
        if (byte === 0 || byte === 255) continue;
        const character = String.fromCharCode(byte);
        if (character === '\r' || character === '\n') {
          socket.write('\r\n');
          answer(socket, line.trim());
          line = '';
          socket.write('SW1> ');
        } else if (byte === 127 || byte === 8) {
          if (line !== '') {
            line = line.slice(0, -1);
            socket.write('\b \b');
          }
        } else {
          line += character;
          socket.write(character);
        }
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
