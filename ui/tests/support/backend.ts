/**
 * Running a real engine for a test.
 *
 * The foundation specs do not need one — the mock transport covers the shell
 * and the showcase — but screen authors do: a screen is only proved by the
 * real `nocturne-server` answering it. `startBackend()` builds the UI if
 * asked, starts the server on a free port with its own empty data directory,
 * and hands back the address and the access token it printed.
 *
 *   const backend = await startBackend();
 *   await openApp(page, '/devices', { backend });
 *   ...
 *   await backend.stop();
 *
 * Each call gets a fresh data directory, so tests never share state. Set
 * `NOCTURNE_SERVER_BIN` to a built binary to skip cargo.
 */

import { spawn, type ChildProcess } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const UI_DIR = resolve(fileURLToPath(new URL('../..', import.meta.url)));
const REPO_DIR = resolve(UI_DIR, '..');

/** The line the server prints when it is up: `… on http://127.0.0.1:PORT/#token=…`. */
const READY = /listening on (https?:\/\/\S+?)\/#token=(\S+)/;

const START_TIMEOUT_MS = 600_000;

export interface Backend {
  /** Where the engine listens, without a trailing slash. */
  readonly url: string;
  /** The access token from the printed link. */
  readonly token: string;
  /** The server's data directory: a fresh temporary one, removed by `stop()`. */
  readonly dataDir: string;
  /** Everything the server wrote to stdout and stderr, for a failing test's report. */
  readonly log: () => string;
  stop(): Promise<void>;
}

export interface BackendOptions {
  /**
   * Serve the web UI from this directory (`--ui-dir`) instead of the one
   * compiled into the binary. Give it `ui/dist` to test the build you just
   * made. Default: the embedded UI.
   */
  uiDir?: string;
  /** Extra command-line arguments for the server. */
  args?: readonly string[];
}

function command(options: BackendOptions, dataDir: string): { bin: string; args: string[] } {
  const args = ['--data-dir', dataDir, '--port', '0'];
  if (options.uiDir) args.push('--ui-dir', resolve(UI_DIR, options.uiDir));
  args.push(...(options.args ?? []));

  const bin = process.env.NOCTURNE_SERVER_BIN;
  if (bin) return { bin, args };
  return { bin: 'cargo', args: ['run', '-q', '-p', 'nocturne-server', '--', ...args] };
}

async function stopProcess(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await new Promise<void>((done) => {
    child.once('exit', () => done());
    child.kill('SIGTERM');
    // A server that will not go quietly must not hang the suite.
    setTimeout(() => child.kill('SIGKILL'), 5_000).unref();
  });
}

/** Starts an engine and waits for it to be ready. */
export async function startBackend(options: BackendOptions = {}): Promise<Backend> {
  const dataDir = await mkdtemp(join(tmpdir(), 'nocturne-e2e-'));
  const { bin, args } = command(options, dataDir);

  const child = spawn(bin, args, {
    cwd: REPO_DIR,
    env: {
      ...process.env,
      // Share the repository's build cache so a test run does not recompile.
      CARGO_TARGET_DIR:
        process.env.CARGO_TARGET_DIR ?? join(process.env.HOME ?? tmpdir(), '.cache/nocturne-target/main'),
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  let output = '';
  const log = () => output;

  const ready = new Promise<{ url: string; token: string }>((fulfil, fail) => {
    const timer = setTimeout(() => {
      fail(
        new Error(
          `nocturne-server did not start within ${START_TIMEOUT_MS / 1000}s. A first run has to ` +
            `build it; set NOCTURNE_SERVER_BIN to a built binary to skip that.\n${output}`,
        ),
      );
    }, START_TIMEOUT_MS);
    timer.unref();

    const read = (chunk: Buffer) => {
      output += chunk.toString();
      const found = READY.exec(output);
      if (!found?.[1] || !found[2]) return;
      clearTimeout(timer);
      fulfil({ url: found[1], token: found[2] });
    };
    child.stdout?.on('data', read);
    child.stderr?.on('data', read);
    child.once('error', (error) => {
      clearTimeout(timer);
      fail(new Error(`could not run ${bin}: ${error.message}`));
    });
    child.once('exit', (code) => {
      clearTimeout(timer);
      fail(new Error(`nocturne-server exited with code ${code}:\n${output}`));
    });
  });

  try {
    const { url, token } = await ready;
    return {
      url,
      token,
      dataDir,
      log,
      async stop() {
        await stopProcess(child);
        await rm(dataDir, { recursive: true, force: true });
      },
    };
  } catch (error) {
    await stopProcess(child);
    await rm(dataDir, { recursive: true, force: true });
    throw error;
  }
}
