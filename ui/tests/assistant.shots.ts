/**
 * `pnpm shots` — the Assistant screen and the dock, photographed in both
 * themes, at the two sizes that matter, with the dock open and closed.
 *
 * Driven by a real engine and a real (fake) model endpoint, as
 * `assistant.spec.ts` is: a screenshot of an empty transcript says nothing
 * about how the screen reads, and a made-up one would say something untrue.
 * The conversations are started over RPC so the pictures are the same every
 * time.
 */

import { createServer, type Server } from 'node:http';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, settle, type Theme } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));
const SHOTS = resolve(fileURLToPath(new URL('./shots/assistant', import.meta.url)));
const THEME_FOLDER: Record<Theme, string> = { dark: 'night', light: 'day' };
const THEMES: readonly Theme[] = ['dark', 'light'];

test.describe.configure({ mode: 'serial', timeout: 300_000 });

// A second copy of the fake model from `assistant.spec.ts`, kept small: the
// two files cannot share a helper without a third file in `tests/`.
type Chunk = Record<string, unknown>;

const text = (content: string): Chunk => ({ choices: [{ delta: { content } }] });
const thinking = (reasoning_content: string): Chunk => ({
  choices: [{ delta: { reasoning_content } }],
});
const stopped = (): Chunk => ({ choices: [{ delta: {}, finish_reason: 'stop' }] });
const tokens = (input: number, output: number): Chunk => ({
  usage: { prompt_tokens: input, completion_tokens: output },
  choices: [],
});
const calls = (name: string, args: unknown, id = `call_${name}`): Chunk => ({
  choices: [
    {
      delta: {
        tool_calls: [
          { index: 0, id, type: 'function', function: { name, arguments: JSON.stringify(args) } },
        ],
      },
      finish_reason: 'tool_calls',
    },
  ],
});

interface FakeModel {
  url: string;
  reply(chunks: Chunk[]): void;
  stop(): Promise<void>;
}

async function startFakeModel(): Promise<FakeModel> {
  const queue: Chunk[][] = [];
  const server: Server = createServer((request, response) => {
    let raw = '';
    request.on('data', (piece) => (raw += piece));
    request.on('end', () => {
      if ((request.url ?? '').endsWith('/models')) {
        response.writeHead(200, { 'content-type': 'application/json' });
        response.end(JSON.stringify({ data: [{ id: 'fake-mini', context_length: 64_000 }] }));
        return;
      }
      const probe = raw.includes('report_ok');
      const chunks = probe
        ? [calls('report_ok', { ok: true }, 'probe')]
        : (queue.shift() ?? [text('I have nothing to add.'), stopped()]);
      response.writeHead(200, { 'content-type': 'text/event-stream' });
      for (const chunk of chunks) response.write(`data: ${JSON.stringify(chunk)}\n\n`);
      response.end('data: [DONE]\n\n');
    });
  });
  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('the fake model got no port');
  return {
    url: `http://127.0.0.1:${address.port}/v1`,
    reply: (chunks) => queue.push(chunks),
    stop: () => new Promise<void>((done) => server.close(() => done())),
  };
}

let backend: Backend;
let uiDir: string;
let prebuilt: string | undefined;
let model: FakeModel;

test.beforeAll(async () => {
  test.setTimeout(300_000);
  if (process.env.NOCTURNE_UI_DIST) {
    prebuilt = process.env.NOCTURNE_UI_DIST;
  } else {
    uiDir = await mkdtemp(join(tmpdir(), 'nocturne-ui-'));
    await run('pnpm', ['exec', 'vite', 'build', '--outDir', uiDir, '--emptyOutDir'], {
      cwd: UI_DIR,
      timeout: 180_000,
    });
  }
  backend = await startBackend({ uiDir: prebuilt ?? uiDir });
  model = await startFakeModel();
});

test.afterAll(async () => {
  await model?.stop();
  await backend?.stop();
  if (uiDir) await rm(uiDir, { recursive: true, force: true });
});

async function shoot(page: Page, theme: Theme, name: string): Promise<void> {
  const directory = join(SHOTS, THEME_FOLDER[theme]);
  await mkdir(directory, { recursive: true });
  await settle(page);
  await page.screenshot({ path: join(directory, `${name}.png`), animations: 'disabled' });
}

async function provider(rpc: Rpc): Promise<void> {
  await rpc('ai.providers.save', {
    name: 'Claude (bench)',
    kind: 'openai',
    base_url: model.url,
    model: 'fake-mini',
    api_key: 'sk-not-a-real-key',
    tool_mode: 'native',
    is_default: true,
  });
}

/** Sends a message and waits for the turn to finish, so the picture is settled. */
async function converse(rpc: Rpc, chatId: string, question: string): Promise<void> {
  await rpc('ai.send', { chat_id: chatId, text: question });
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const view = await rpc<{ chat: { pending_turn: string | null } }>('ai.chats.get', {
      id: chatId,
    });
    if (!view.chat.pending_turn) return;
    await new Promise((wait) => setTimeout(wait, 100));
  }
  throw new Error('the turn never finished');
}

test('the assistant, with and without a model', async ({ page, request }) => {
  test.setTimeout(300_000);
  const rpc = rpcFor(request, backend);

  // ---------------------------------------------------- nothing set up yet
  for (const theme of THEMES) {
    await openApp(page, '/assistant', { backend, theme });
    await expect(page.getByRole('heading', { name: 'No model is connected yet' })).toBeVisible();
    // The local probe takes about a second; shoot the settled card, not the
    // spinner it shows on the way.
    await expect(page.getByRole('button', { name: 'Look again' })).toBeVisible();
    await shoot(page, theme, 'no-model');
  }

  // --------------------------------------------------------- a real estate
  await provider(rpc);
  for (const device of [
    { name: 'core-rtr1', address: '10.0.0.1', site: 'HQ', role: 'core', profile: 'cisco_ios' },
    { name: 'dist-sw1', address: '10.0.1.1', site: 'HQ', role: 'distribution', profile: 'cisco_ios' },
    { name: 'access-sw7', address: '10.0.7.12', site: 'HQ', role: 'access', profile: 'cisco_ios' },
  ]) {
    await rpc('devices.create', device).catch(() => {});
  }

  const chat = await rpc<{ id: string }>('ai.chats.create', {});
  model.reply([
    thinking(
      'The question is about a port on an access switch. Start with the inventory so I am talking about the right device, then look at the port itself.',
    ),
    text('Let me find the switch first.'),
    calls('list_devices', { site: 'HQ', role: 'access' }),
    tokens(2_140, 48),
  ]);
  model.reply([
    text(
      'That is **access-sw7** at HQ (10.0.7.12).\n\nI could not reach the device itself — it has no credential in the vault yet — so this is as far as the evidence goes:\n\n- the inventory has the switch, with the access role at HQ\n- nothing has been collected from it, so there are no interface counters to compare\n\nTo go further, add a credential and let me read it:\n\n```cisco\nshow interfaces GigabitEthernet1/0/7\nshow logging | include 1/0/7\n```\n\nI have not run those: they would need a session to the device.',
    ),
    stopped(),
    tokens(3_900, 220),
  ]);
  await converse(rpc, chat.id, 'why is Gi1/0/7 on the access switch at HQ down?');

  for (const theme of THEMES) {
    await openApp(page, '/assistant', { backend, theme });
    await expect(page.getByText('That is')).toBeVisible();
    await shoot(page, theme, 'conversation');

    // The step opened: what ran, what came back, and what it may reach.
    await page.getByRole('button', { name: /List devices/ }).click();
    await expect(page.getByRole('group', { name: 'Data the assistant read' })).toBeVisible();
    await shoot(page, theme, 'tool-step-open');

    await page.getByRole('tab', { name: 'Models' }).click();
    await expect(page.getByText('Claude (bench)')).toBeVisible();
    await shoot(page, theme, 'models');

    await page.getByRole('tab', { name: 'Usage' }).click();
    await expect(page.getByRole('cell', { name: 'fake-mini' })).toBeVisible();
    await shoot(page, theme, 'usage');

    // The dock, beside another screen.
    await openApp(page, '/devices', { backend, theme, dock: true });
    await expect(
      page.getByRole('complementary', { name: 'Assistant' }).getByText('That is'),
    ).toBeVisible();
    await shoot(page, theme, 'dock-on-devices');

    // The narrow window, with and without the dock.
    await page.setViewportSize({ width: 1280, height: 720 });
    await shoot(page, theme, 'dock-1280');
    await openApp(page, '/assistant', { backend, theme });
    await expect(page.getByText('That is')).toBeVisible();
    await shoot(page, theme, 'conversation-1280');
    // The tightest case the specification names: 1280 with the dock open, so
    // the screen has about 680px. The chat rail steps aside.
    await openApp(page, '/assistant', { backend, theme, dock: true });
    await expect(page.getByRole('complementary', { name: 'Assistant' })).toBeVisible();
    await shoot(page, theme, 'assistant-and-dock-1280');
    await page.setViewportSize({ width: 1440, height: 900 });
  }
});

test('an approval, waiting for a person', async ({ page, request }) => {
  test.setTimeout(300_000);
  const rpc = rpcFor(request, backend);
  await provider(rpc);
  await rpc('settings.set', { section: 'assistant', value: { confirm_reads: true } });
  await rpc('devices.create', {
    name: 'edge-fw1',
    address: '10.0.0.254',
    site: 'HQ',
    role: 'firewall',
  }).catch(() => {});

  for (const theme of THEMES) {
    const chat = await rpc<{ id: string }>('ai.chats.create', {});
    model.reply([
      text('I will read the interface state off the firewall.'),
      calls('run_commands', {
        devices: ['edge-fw1'],
        commands: ['show interfaces description'],
      }),
    ]);

    // The window has to be open and attached before the turn starts: an
    // approval request is only ever an event, so one made while nothing was
    // listening cannot be shown afterwards (see NOTES.md).
    await openApp(page, `/assistant?chat=${chat.id}`, { backend, theme });
    await expect(page.getByRole('textbox', { name: 'Message to the assistant' })).toBeVisible();
    // The turn stops inside the tool gate and waits; no need to let it finish.
    await rpc('ai.send', { chat_id: chat.id, text: 'is anything down on edge-fw1?' });

    await expect(page.getByRole('group', { name: 'Approval needed' })).toBeVisible();
    await shoot(page, theme, 'approval');
    await rpc('ai.cancel', { chat_id: chat.id });
  }
});
