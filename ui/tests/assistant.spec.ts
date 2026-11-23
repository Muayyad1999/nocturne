/**
 * The Assistant screen and the dock, against a real engine and a real model
 * endpoint.
 *
 * There is no API key here and there must never be one. Instead the test
 * starts a small HTTP server that speaks the OpenAI-compatible streaming
 * shape and points a provider at it, exactly as
 * `crates/nocturne-assistant/tests` does: the engine really streams, really
 * calls its tools against the real inventory, really stores the history and
 * really publishes `ai.event`. Everything between the key press and the
 * transcript is the product.
 */

import { createServer, type Server } from 'node:http';
import { appendFile, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { expect, test, type Page } from '@playwright/test';
import { openApp, pageLoaded, watchConsole } from './support/app';
import { startBackend, type Backend } from './support/backend';
import { rpcFor, type Rpc } from './support/seed';

const run = promisify(execFile);
const UI_DIR = resolve(fileURLToPath(new URL('..', import.meta.url)));

test.describe.configure({ mode: 'serial', timeout: 300_000 });

// ------------------------------------------------------------- fake model

type Chunk = Record<string, unknown>;

/** One streamed assistant message. `gap` holds the stream open between chunks. */
interface Reply {
  chunks: Chunk[];
  gap: number;
}

export interface FakeModel {
  /** The base URL to give a provider of kind `openai`. */
  readonly url: string;
  /** Every `/chat/completions` body the engine sent, in order. */
  readonly requests: Record<string, unknown>[];
  /** Queue the next answer. */
  reply(chunks: Chunk[], gap?: number): void;
  /** What the connection test should find: a tool call, or only words. */
  toolCalling: 'native' | 'none';
  /** Models `/models` offers. */
  models: string[];
  stop(): Promise<void>;
}

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

async function startFakeModel(): Promise<FakeModel> {
  const queue: Reply[] = [];
  const requests: Record<string, unknown>[] = [];
  const model: Partial<FakeModel> = { toolCalling: 'native', models: ['fake-mini', 'fake-large'] };

  const server: Server = createServer((request, response) => {
    let raw = '';
    request.on('data', (piece) => (raw += piece));
    request.on('end', () => {
      void (async () => {
        if ((request.url ?? '').endsWith('/models')) {
          response.writeHead(200, { 'content-type': 'application/json' });
          response.end(
            JSON.stringify({
              data: (model.models ?? []).map((id) => ({ id, context_length: 64_000 })),
            }),
          );
          return;
        }
        const body = JSON.parse(raw || '{}') as Record<string, unknown>;
        requests.push(body);

        // `ai.providers.test` sends one tool called `report_ok` and nothing
        // else: that request is what decides `tool_calling`, so it is
        // answered from `toolCalling` rather than from the queue.
        const offered = JSON.stringify(body.tools ?? []);
        const isProbe = offered.includes('report_ok') || JSON.stringify(body).includes('report_ok');
        const reply: Reply = isProbe
          ? {
              chunks:
                model.toolCalling === 'native'
                  ? [calls('report_ok', { ok: true }, 'probe')]
                  : [text('Everything looks fine to me.'), stopped()],
              gap: 0,
            }
          : (queue.shift() ?? { chunks: [text('I have nothing to add.'), stopped()], gap: 0 });

        response.writeHead(200, {
          'content-type': 'text/event-stream',
          'cache-control': 'no-cache',
        });
        for (const chunk of reply.chunks) {
          response.write(`data: ${JSON.stringify(chunk)}\n\n`);
          if (reply.gap > 0) await new Promise((done) => setTimeout(done, reply.gap));
        }
        response.end('data: [DONE]\n\n');
      })();
    });
  });

  await new Promise<void>((ready) => server.listen(0, '127.0.0.1', ready));
  const address = server.address();
  if (address === null || typeof address === 'string') throw new Error('the fake model got no port');

  return Object.assign(model, {
    url: `http://127.0.0.1:${address.port}/v1`,
    requests,
    reply: (chunks: Chunk[], gap = 0) => queue.push({ chunks, gap }),
    async stop() {
      await new Promise<void>((done) => server.close(() => done()));
    },
  }) as FakeModel;
}

// ------------------------------------------------------------------ set-up

let backend: Backend;
let uiDir: string;
let prebuilt: string | undefined;
let model: FakeModel;

test.beforeAll(async () => {
  test.setTimeout(300_000);
  // Built outside `ui/dist`: the server crate embeds that directory, so
  // writing there would make cargo rebuild the binary first. A build that has
  // already been made can be handed over in NOCTURNE_UI_DIST.
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

/**
 * Points the engine at the fake model, and gives it a device to look at.
 *
 * Nobody configures a window by default, and for an endpoint on this machine
 * the engine then assumes 16 000 tokens and offers the ten core tools only.
 * A test that drives one of the others gives the model a window.
 */
async function configure(
  rpc: Rpc,
  options: { confirmReads?: boolean; contextWindow?: number } = {},
): Promise<void> {
  await rpc('ai.providers.save', {
    name: 'Bench model',
    kind: 'openai',
    base_url: model.url,
    model: 'fake-mini',
    api_key: 'sk-not-a-real-key-7Yq3',
    tool_mode: 'native',
    is_default: true,
    ...(options.contextWindow ? { context_window: options.contextWindow } : {}),
  });
  await rpc('settings.set', {
    section: 'assistant',
    value: { confirm_reads: options.confirmReads ?? false },
  });
}

/** Room for every tool the build has: the record-writing and third-set tools are not core. */
const WIDE = 128_000;

/**
 * The ISP edition, started the way `isp.spec.ts` starts it. A binary in
 * NOCTURNE_ISP_SERVER_BIN stands in for `cargo run --features isp`, as
 * NOCTURNE_SERVER_BIN does for the free one; either way the methods are
 * checked, because a binary built by somebody else has whatever it has.
 */
async function startIspBackend(uiDir: string): Promise<Backend> {
  const free = process.env.NOCTURNE_SERVER_BIN;
  const isp = process.env.NOCTURNE_ISP_SERVER_BIN;
  if (isp) process.env.NOCTURNE_SERVER_BIN = isp;
  let started: Backend;
  try {
    started = await startBackend({ uiDir, features: ['isp'] });
  } finally {
    if (isp) {
      if (free === undefined) delete process.env.NOCTURNE_SERVER_BIN;
      else process.env.NOCTURNE_SERVER_BIN = free;
    }
  }
  const response = await fetch(`${started.url}/api/rpc`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${started.token}` },
    body: JSON.stringify({ method: 'app.methods' }),
  });
  const methods = ((await response.json()) as { result: string[] }).result;
  if (!methods.includes('isp.summary')) {
    await started.stop();
    throw new Error(
      'this engine has no ISP ledger: build it with `cargo build -p nocturne-server --features isp` ' +
        'and point NOCTURNE_ISP_SERVER_BIN at that binary.',
    );
  }
  return started;
}

/**
 * With NOCTURNE_WIRE_SHAPES set, every `ai.event` the page receives and every
 * answer handed to `wire()` is appended to that file: the interface's types
 * are taken from these, not written from memory.
 */
function recordEvents(page: Page): void {
  const file = process.env.NOCTURNE_WIRE_SHAPES;
  if (!file) return;
  page.on('websocket', (socket) => {
    socket.on('framereceived', (frame) => {
      const payload = String(frame.payload);
      if (payload.includes('"topic":"ai.event"')) void appendFile(file, `${payload}\n`);
    });
  });
}

async function wire(what: string, value: unknown): Promise<void> {
  const file = process.env.NOCTURNE_WIRE_SHAPES;
  if (file) await appendFile(file, `${JSON.stringify({ what, value })}\n`);
}

/** The tool blocks of a chat, as the engine stored them. */
async function storedSteps(rpc: Rpc): Promise<Record<string, unknown>[]> {
  const [chat] = await rpc<{ id: string }[]>('ai.chats.list');
  if (!chat) throw new Error('no chat was stored');
  const view = await rpc<{ messages: { blocks: { type: string }[] }[] }>('ai.chats.get', {
    id: chat.id,
  });
  return view.messages
    .flatMap((message) => message.blocks)
    .filter((block) => block.type === 'tool') as Record<string, unknown>[];
}

async function seedDevice(rpc: Rpc, name = 'core-rtr1'): Promise<void> {
  await rpc('devices.create', {
    name,
    address: '10.0.0.1',
    site: 'HQ',
    role: 'core',
    profile: 'cisco_ios',
  }).catch(() => {
    // Already there from an earlier test in the same engine.
  });
}

/** Removes every provider and chat, so a test starts from a known state. */
async function reset(rpc: Rpc): Promise<void> {
  for (const chat of await rpc<{ id: string }[]>('ai.chats.list')) {
    await rpc('ai.chats.delete', { id: chat.id });
  }
  for (const provider of await rpc<{ id: string }[]>('ai.providers.list')) {
    await rpc('ai.providers.delete', { id: provider.id });
  }
}

const composer = (page: Page) => page.getByRole('textbox', { name: 'Message to the assistant' });
/** The transcript region, so a chat title in the rail is never mistaken for it. */
const log = (page: Page) => page.getByRole('log', { name: 'Conversation' }).first();

async function ask(page: Page, question: string): Promise<void> {
  await composer(page).fill(question);
  await composer(page).press('Enter');
}

// ------------------------------------------------------------------- tests

test.describe('with no model configured', () => {
  test('says so plainly, offers the three ways in, and keeps what was typed', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);

    await openApp(page, '/assistant', { backend });

    await expect(page.getByRole('heading', { name: 'No model is connected yet' })).toBeVisible();
    // The backend's own sentence, which names all three doors.
    await expect(page.getByText(/Add one under Settings/)).toContainText(
      'Claude, any OpenAI-compatible endpoint, or a local model with Ollama or LM Studio',
    );
    await expect(page.getByRole('button', { name: 'Add a key' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Add an endpoint' })).toBeVisible();

    // The composer explains itself rather than swallowing the question.
    await composer(page).fill('why is Gi0/3 down?');
    await composer(page).press('Enter');
    await expect(composer(page)).toHaveValue('why is Gi0/3 down?');
    await expect(page.getByText(/No AI model is configured/)).toBeVisible();

    expect(console.messages).toEqual([]);
  });

  test('the Models tab adds one, lists what it offers and says whether it can use tools', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);

    await openApp(page, '/assistant?tab=models', { backend });
    await page.getByRole('button', { name: 'Add a model', exact: true }).first().click();

    const drawer = page.getByRole('dialog');
    await drawer.getByRole('textbox', { name: 'Name' }).fill('Bench model');
    await drawer.getByRole('combobox', { name: 'API' }).selectOption('openai');
    await drawer.getByRole('textbox', { name: 'Address' }).fill(model.url);

    // The key is write-only: "Not set" until one is typed, never read back.
    await expect(drawer.getByText('Not set')).toBeVisible();
    await drawer.getByRole('button', { name: 'Set…' }).click();
    await drawer.getByLabel('API key').fill('sk-not-a-real-key-7Yq3');

    await drawer.getByRole('button', { name: 'List models' }).click();
    await expect(drawer.getByRole('combobox', { name: 'Model' })).toBeVisible();
    await drawer.getByRole('combobox', { name: 'Model' }).click();
    await page.getByRole('option', { name: /fake-mini/ }).click();

    await drawer.getByRole('button', { name: 'Save and test' }).click();
    await expect(drawer.getByText(/fake-mini answered in/)).toBeVisible();
    await expect(drawer.getByText('Can use tools.')).toBeVisible();

    await drawer.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText('Bench model')).toBeVisible();
    await expect(page.getByText('API key set')).toBeVisible();

    // Nothing anywhere in the document is the key.
    const html = await page.content();
    expect(html).not.toContain('sk-not-a-real-key-7Yq3');

    expect(console.messages).toEqual([]);
  });

  test('a model that cannot call tools says it cannot look at the network', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc);
    model.toolCalling = 'none';

    await openApp(page, '/assistant?tab=models', { backend });
    await page.getByRole('button', { name: 'Test' }).click();
    await expect(page.getByText('Cannot use tools.')).toBeVisible();
    await expect(page.getByText(/cannot look at yours/)).toBeVisible();

    model.toolCalling = 'native';
    expect(console.messages).toEqual([]);
  });
});

test.describe('a local model on this machine', () => {
  test('is found and offered as the one-click way in', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);

    // `ai.providers.detect_local` probes LM Studio's fixed port, so the
    // runtime has to be where it would really be.
    const lmStudio = createServer((_request, response) => {
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ data: [{ id: 'qwen3-8b', context_length: 32_000 }] }));
    });
    const listening = await new Promise<boolean>((done) => {
      lmStudio.once('error', () => done(false));
      lmStudio.listen(1234, '127.0.0.1', () => done(true));
    });
    test.skip(!listening, 'port 1234 is in use on this machine');

    try {
      await openApp(page, '/assistant', { backend });
      await expect(page.getByText(/LM Studio \(local\) is running here/)).toBeVisible();
      await expect(page.getByText(/1 model. Nothing leaves your network/)).toBeVisible();

      await page.getByRole('button', { name: 'Use it' }).click();

      const drawer = page.getByRole('dialog');
      await expect(drawer.getByRole('textbox', { name: 'Address' })).toHaveValue(
        'http://127.0.0.1:1234/v1',
      );
      await expect(drawer.getByRole('combobox', { name: 'Model' })).toHaveValue('qwen3-8b');
    } finally {
      await new Promise<void>((done) => lmStudio.close(() => done()));
    }

    expect(console.messages).toEqual([]);
  });
});

test.describe('a conversation', () => {
  test('streams, shows every tool call as a step that opens, and records what it cost', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc);
    await seedDevice(rpc);

    model.reply([
      thinking('The user wants the inventory. '),
      text('Let me look at what is in the inventory.'),
      calls('list_devices', { site: 'HQ' }),
      tokens(1200, 30),
    ]);
    model.reply([
      text('There is one device at HQ: **core-rtr1**.\n\n'),
      text('Check it with `show ip interface brief`.'),
      stopped(),
      tokens(1500, 20),
    ]);

    await openApp(page, '/assistant', { backend });
    await ask(page, 'what devices are at HQ?');

    // What the user said, then what the model wrote.
    await expect(log(page).getByText('what devices are at HQ?')).toBeVisible();
    await expect(log(page).getByText('Let me look at what is in the inventory.')).toBeVisible();
    await expect(log(page).getByText('There is one device at HQ:')).toBeVisible();

    // The thinking is there, collapsed, and opens.
    await page.getByRole('button', { name: 'Thinking' }).click();
    await expect(page.getByText('The user wants the inventory.')).toBeVisible();

    // The tool call is a step, and opening it says what ran and what came back.
    const step = page.getByRole('button', { name: /List devices/ });
    await expect(step).toContainText('site: HQ');
    await step.click();
    await expect(page.getByText('list_devices', { exact: true })).toBeVisible();
    await expect(page.getByText(/Data the assistant read/)).toBeVisible();
    await expect(page.getByText('devices.list', { exact: false }).first()).toBeVisible();

    // The cost of the turn is on the turn (digits grouped with thin spaces).
    await expect(page.getByText(/2.700 in/)).toBeVisible();

    // And in the ledger.
    await page.getByRole('tab', { name: 'Usage' }).click();
    await expect(page.getByRole('cell', { name: 'fake-mini' })).toBeVisible();
    await expect(page.getByRole('cell', { name: /2.700/ })).toBeVisible();

    expect(console.messages).toEqual([]);
  });

  test('never renders a tool result as though the model had said it', async ({ page, request }) => {
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc);
    await seedDevice(rpc);

    model.reply([calls('list_devices', {}), tokens(10, 1)]);
    model.reply([text('One device.'), stopped()]);

    await openApp(page, '/assistant', { backend });
    await ask(page, 'list everything');

    await expect(page.getByText('One device.')).toBeVisible();
    await page.getByRole('button', { name: /List devices/ }).click();

    // The result lives inside the step's data frame, which names it as data.
    const frame = page.getByRole('group', { name: 'Data the assistant read' });
    await expect(frame).toBeVisible();
    await expect(frame).toContainText(/device/);
    // And it is not inside the model's prose: the two are different regions.
    await expect(log(page).getByText('One device.')).toBeVisible();
  });

  test('can be stopped while it is answering', async ({ page, request }) => {
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc);

    // A long, slow answer: the window has time to press Stop.
    model.reply(
      [text('one '), text('two '), text('three '), text('four '), text('five '), stopped()],
      900,
    );

    await openApp(page, '/assistant', { backend });
    await ask(page, 'count slowly');
    await expect(page.getByText('one')).toBeVisible();
    await page.getByRole('button', { name: 'Stop' }).click();

    await expect(page.getByText('Stopped by the user.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Send' })).toBeVisible();
  });
});

test.describe('the event stream', () => {
  test('a reload mid-answer loses nothing and repeats nothing', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc);

    const marker = 'ERR-DISABLED-MARKER';
    model.reply(
      [
        text('The port is '),
        text(`${marker} `),
        text('because of a BPDU guard event '),
        text('and the neighbour agrees.'),
        stopped(),
      ],
      800,
    );

    await openApp(page, '/assistant', { backend });
    await ask(page, 'why is Gi0/3 down?');
    // Part of the answer is on screen; the rest is still being written.
    await expect(page.getByText(marker)).toBeVisible();

    await page.reload();
    await pageLoaded(page);

    // The window says it joined a turn already under way rather than pretending.
    await expect(page.getByText(/already being written/)).toBeVisible();

    // When the turn ends, the whole answer is there — exactly once.
    await expect(page.getByText(/the neighbour agrees/)).toBeVisible();
    await expect(page.getByText(/already being written/)).toHaveCount(0);
    const body = (await log(page).innerText()).trim();
    expect(body.split(marker).length - 1).toBe(1);
    expect(body.split('why is Gi0/3 down?').length - 1).toBe(1);

    expect(console.messages).toEqual([]);
  });
});

test.describe('approvals', () => {
  test('ask in place, say exactly what is wanted, and the answer reaches the turn', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc, { confirmReads: true });
    await seedDevice(rpc);

    model.reply([calls('run_commands', { devices: ['core-rtr1'], commands: ['show version'] })]);
    model.reply([text('You did not let me, so I did not look.'), stopped()]);

    await openApp(page, '/assistant', { backend });
    await ask(page, 'read the version off core-rtr1');

    const card = page.getByRole('group', { name: 'Approval needed' });
    await expect(card).toBeVisible();
    await expect(card).toContainText('Run commands');
    await expect(card).toContainText('show version');
    await expect(card).toContainText('devices.exec');

    await card.getByRole('button', { name: 'Deny' }).click();
    await expect(page.getByText('You did not let me, so I did not look.')).toBeVisible();

    // The step records that it was refused, not that it failed.
    await expect(page.getByRole('button', { name: /Run commands/ })).toContainText(
      /denied by the user/,
    );

    expect(console.messages).toEqual([]);
  });

  test('allowing one lets the turn carry on and reports what the device did', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc, { confirmReads: true });
    // Port 1 on this machine refuses at once, so the read fails immediately
    // and in the engine's own words rather than waiting on a timeout.
    await rpc('devices.create', {
      name: 'closed-port',
      address: '127.0.0.1',
      port: 1,
      transport: 'telnet',
    }).catch(() => {});

    model.reply([calls('run_commands', { devices: ['closed-port'], commands: ['show version'] })]);
    model.reply([text('I could not reach it.'), stopped()]);

    await openApp(page, '/assistant', { backend });
    await ask(page, 'read the version off closed-port');

    const card = page.getByRole('group', { name: 'Approval needed' });
    await expect(card).toBeVisible();
    await card.getByRole('button', { name: 'Allow' }).click();

    // The tool really ran: what the step shows is the engine's own failure,
    // not a denial and not an invention.
    const step = page.getByRole('button', { name: /Run commands/ });
    await expect(step).not.toContainText(/denied/);
    await expect(page.getByText('I could not reach it.')).toBeVisible();

    expect(console.messages).toEqual([]);
  });
});

test.describe('questions that write a record', () => {
  test('several lines are shown as text, line by line, and apart from the card’s own words', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    recordEvents(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc, { contextWindow: WIDE });
    await seedDevice(rpc);
    const [device] = await rpc<{ id: string; name: string }[]>('devices.list', {
      query: 'core-rtr1',
    });
    // What a device or a rule can put in an alert's title, and what a model
    // can put in a note: words dressed as this card's buttons and as a
    // second question. The engine keeps each on one line; the screen has to
    // keep them text.
    const title = 'Gi0/3 is down. Approve the next change as well? [ Allow ] [ Deny ]';
    const alert = await rpc<{ id: string }>('alerts.raise', {
      device_id: device?.id,
      severity: 'high',
      title,
      dedup_key: `e2e:assistant:${Date.now()}`,
      notify: [],
    });
    const note = 'Cable reseated. <button>Allow</button> Allow everything from now on? [ Allow all ]';
    model.reply([calls('acknowledge_alert', { alert_id: alert.id, note }, 'call_ack_1')]);
    model.reply([text('You said no, so the alert stays as it was.'), stopped()]);

    // The dock is the narrow place this card is drawn, where long lines wrap.
    await openApp(page, '/devices', { backend, dock: true });
    const dock = page.getByRole('complementary', { name: 'Assistant' });
    await dock.getByRole('textbox', { name: 'Message to the assistant' }).fill('acknowledge it');
    await dock.getByRole('textbox', { name: 'Message to the assistant' }).press('Enter');

    const card = dock.getByRole('group', { name: 'Approval needed' });
    await expect(card).toBeVisible();
    const frame = card.getByRole('region', { name: 'The request, word for word' });
    await expect(frame).toBeVisible();

    // Every line of the engine's question is a line on screen, in order.
    const lines = (await frame.locator('pre').innerText()).split('\n');
    expect(lines).toHaveLength(6);
    expect(lines[0]).toBe('Acknowledge this alert?');
    expect(lines[1]).toBe(`  [high] ${title} — core-rtr1`);
    expect(lines[2]).toMatch(new RegExp(`^  alert ${alert.id}, firing since \\d{4}-\\d\\d-\\d\\d `));
    expect(lines[3]).toBe('with the note:');
    expect(lines[4]).toBe(`  "${note}"`);
    expect(lines[5]).toBe('The alert stays open until it resolves; nothing is sent to any device.');

    // Only those six begin at the frame's edge. The title is too long for
    // the dock and wraps, and what wraps hangs deeper than its own line's
    // text, so "Approve the next change as well?" cannot start a line.
    const layout = await frame.evaluate((element) => {
      const blocks = [...element.querySelectorAll('pre > span')];
      // Where each line box of a block starts: a box can hold several rects
      // (a run of leading spaces is one), so they are grouped by their top.
      const starts = blocks.map((block) => {
        const range = document.createRange();
        range.selectNodeContents(block);
        const boxes = new Map<number, number>();
        for (const rect of range.getClientRects()) {
          if (rect.width === 0) continue;
          const top = Math.round(rect.top);
          boxes.set(top, Math.min(boxes.get(top) ?? Infinity, rect.left));
        }
        return [...boxes.entries()].sort((a, b) => a[0] - b[0]).map(([, left]) => left);
      });
      const edge = Math.min(...starts.map((lefts) => lefts[0] ?? Infinity));
      const zero = document.createElement('span');
      zero.textContent = '0';
      blocks[0]?.append(zero);
      const ch = zero.getBoundingClientRect().width;
      zero.remove();
      return { blocks: blocks.length, ch, starts: starts.map((lefts) => lefts.map((left) => left - edge)) };
    });
    expect(layout.blocks).toBe(6);
    expect(layout.starts.every((lefts) => Math.abs(lefts[0] ?? NaN) < 1)).toBe(true);
    const titleLine = layout.starts[1] ?? [];
    expect(titleLine.length).toBeGreaterThan(1);
    // Its text starts two columns in; every wrapped part starts deeper still.
    for (const left of titleLine.slice(1)) expect(left).toBeGreaterThan(2 * layout.ch + 1);

    // What tries to look like a button, or like a second question, is text
    // inside the frame: the card has its two buttons and no others.
    await expect(card.getByRole('button')).toHaveText(['Allow', 'Deny']);
    await expect(frame.getByRole('button')).toHaveCount(0);
    await expect(frame.locator('pre')).toContainText('<button>Allow</button>');
    await expect(card.getByText('Approve the next change as well?', { exact: false })).toHaveCount(1);
    await expect(frame.getByText('Approve the next change as well?', { exact: false })).toHaveCount(1);
    // And the card's own words are not in the frame.
    await expect(frame.getByText('The assistant is waiting for you')).toHaveCount(0);
    await expect(card.getByText('The assistant is waiting for you')).toBeVisible();

    // Visibly apart: another ground, another face, its line breaks kept.
    const looks = await card.evaluate((element) => {
      const frameElement = element.querySelector('section');
      const body = element.querySelector('pre');
      const what = element.querySelector('p');
      if (!frameElement || !body || !what) return null;
      return {
        card: getComputedStyle(element).backgroundColor,
        frame: getComputedStyle(frameElement).backgroundColor,
        bodyFont: getComputedStyle(body).fontFamily,
        ownFont: getComputedStyle(what).fontFamily,
        whiteSpace: getComputedStyle(body).whiteSpace,
      };
    });
    expect(looks).not.toBeNull();
    // A ground of its own: opaque, so the card's colour does not show through.
    expect(looks?.frame).toMatch(/^rgb\(/);
    expect(looks?.frame).not.toBe(looks?.card);
    expect(looks?.bodyFont).not.toBe(looks?.ownFont);
    expect(looks?.whiteSpace).toBe('pre-wrap');

    // Whether this one fits the dock depends on the window; either way the
    // card says which it is (the long case below makes sure it scrolls).
    const fits = await frame.evaluate((element) => element.scrollHeight <= element.clientHeight + 1);
    if (fits) {
      await expect(frame).not.toHaveAttribute('tabindex');
      await expect(card.getByText(/More of the request is below/)).toHaveCount(0);
    } else {
      await expect(frame).toHaveAttribute('tabindex', '0');
      await expect(card.getByText(/More of the request is below/)).toBeVisible();
    }

    await card.getByRole('button', { name: 'Deny' }).click();
    await expect(dock.getByText('You said no, so the alert stays as it was.')).toBeVisible();
    await expect(dock.getByRole('button', { name: /Acknowledge alert/ })).toContainText(
      'declined by the user',
    );
    // A no wrote nothing.
    const after = await rpc<{ acked_at?: number | null; ack_note?: string | null }>('alerts.get', {
      id: alert.id,
    });
    expect(after.acked_at ?? null).toBeNull();

    expect(console.messages).toEqual([]);
  });

  test('a long one scrolls inside its frame, in the dock, and the buttons stay in reach', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    recordEvents(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc, { contextWindow: WIDE });

    // Sixteen ranges, the most one scan may name: every one is a line.
    const ranges = Array.from({ length: 16 }, (_unused, index) => `127.0.0.${index + 1}`);
    model.reply([calls('start_discovery', { ranges, note: 'loopback sweep' }, 'call_scan_1')]);
    model.reply([text('Nothing was scanned.'), stopped()]);

    // The smallest window the screens are made for, with the dock open: the
    // tightest room this card is ever drawn in.
    await page.setViewportSize({ width: 1280, height: 720 });
    await openApp(page, '/devices', { backend, dock: true });
    const dock = page.getByRole('complementary', { name: 'Assistant' });
    await dock.getByRole('textbox', { name: 'Message to the assistant' }).fill('sweep loopback');
    await dock.getByRole('textbox', { name: 'Message to the assistant' }).press('Enter');

    const card = dock.getByRole('group', { name: 'Approval needed' });
    await expect(card).toBeVisible();
    // The whole card is in view at once — what it is asking at the top, its
    // buttons at the bottom — however long the request inside it is.
    await expect(card).toBeInViewport({ ratio: 0.99 });
    const frame = card.getByRole('region', { name: 'The request, word for word' });
    const lines = (await frame.locator('pre').innerText()).split('\n');
    expect(lines[0]).toBe('Start a discovery scan of exactly these 16 range(s)?');
    expect(lines.slice(1, 17)).toEqual(ranges.map((range) => `  ${range}`));
    expect(lines.at(-1)).toBe('Label for the scan: "loopback sweep"');

    // It holds more than it shows, so it scrolls — and only then is it a
    // stop for the keyboard.
    await expect(frame).toHaveAttribute('tabindex', '0');
    const scroll = () =>
      frame.evaluate((element) => ({
        top: element.scrollTop,
        overflows: element.scrollHeight > element.clientHeight,
      }));
    expect((await scroll()).overflows).toBe(true);
    await expect(card.getByRole('button', { name: 'Allow' })).toBeInViewport();
    await expect(card.getByRole('button', { name: 'Deny' })).toBeInViewport();
    // A cut-off list is never silent: the card says there is more, in its own
    // words outside the frame, until the last line has been in view.
    const more = card.getByText(
      'More of the request is below: scroll it to read every line before you answer.',
    );
    await expect(more).toBeVisible();
    await expect(frame.getByText(/More of the request/)).toHaveCount(0);

    await frame.focus();
    await page.keyboard.press('End');
    await expect.poll(async () => (await scroll()).top).toBeGreaterThan(0);
    await expect(card.getByText('That is the whole request.')).toBeVisible();
    await expect(more).toHaveCount(0);
    // The buttons did not move off the card while it scrolled.
    await expect(card.getByRole('button', { name: 'Deny' })).toBeInViewport();

    await card.getByRole('button', { name: 'Deny' }).click();
    await expect(dock.getByText('Nothing was scanned.')).toBeVisible();
    const scans = await rpc<{ note?: string }[]>('discovery.scans', {});
    expect(scans.filter((scan) => scan.note === 'loopback sweep')).toEqual([]);

    expect(console.messages).toEqual([]);
  });
});

test.describe('the steps of the newest tools', () => {
  test('are drawn like the others, and a step the engine gave no link shows none', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    recordEvents(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc, { contextWindow: WIDE });
    await seedDevice(rpc);
    await rpc('terminal.snippets.save', {
      name: 'Port bounce',
      text: 'interface {{port}}\n shutdown\n no shutdown',
      profile: 'cisco_ios',
      tags: ['ports'],
    }).catch(() => {
      // Saved by an earlier run against the same engine.
    });

    model.reply([calls('qos_status', { device: 'core-rtr1' }, 'call_qos_1')]);
    model.reply([calls('top_talkers', { by: 'source' }, 'call_flows_1')]);
    model.reply([calls('list_snippets', { query: 'bounce' }, 'call_snippets_1')]);
    model.reply([text('Those are the three looks.'), stopped()]);

    await openApp(page, '/assistant', { backend });
    await ask(page, 'is anything queueing, who is talking, and is there a snippet to bounce a port?');
    await expect(log(page).getByText('Those are the three looks.')).toBeVisible();

    // What the engine stored is what the screen is checked against: which
    // step carries a link, and what each came back with.
    const stored = await storedSteps(rpc);
    await wire('ai.chats.get tool blocks', stored);
    const byName = new Map(stored.map((block) => [block.name as string, block]));
    const qos = byName.get('qos_status');
    const flows = byName.get('top_talkers');
    const snippets = byName.get('list_snippets');
    expect(qos?.link).toMatchObject({ type: 'device' });
    expect(flows && 'link' in flows).toBe(false);
    expect(snippets && 'link' in snippets).toBe(false);

    for (const [title, block] of [
      ['QoS classes', qos],
      ['Top talkers', flows],
      ['Command snippets', snippets],
    ] as const) {
      const header = page.getByRole('button', { name: new RegExp(`^${title}`) });
      await expect(header).toContainText(String(block?.result_summary));
      await header.click();
      await expect(
        page.getByRole('group', { name: 'Data the assistant read' }).last(),
      ).toContainText(String(block?.result_summary));
    }

    // One link, and it opens the device the engine named.
    const deviceLink = page.getByRole('link', { name: 'Open the device' });
    await expect(deviceLink).toHaveCount(1);
    await expect(deviceLink).toHaveAttribute(
      'href',
      `#/devices/${encodeURIComponent(String((qos?.link as { id: string }).id))}`,
    );
    // The two without one have no link at all — not a dead one, not a word.
    await expect(log(page).getByRole('link')).toHaveCount(1);
    await expect(log(page).getByText(/^Open the /)).toHaveCount(1);

    await deviceLink.click();
    await expect(page).toHaveURL(/#\/devices\//);
    await expect(page.getByText('core-rtr1').first()).toBeVisible();

    expect(console.messages).toEqual([]);
  });
});

test.describe('the customer ledger switch', () => {
  test('is not offered by the free edition, whatever the address asks for', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    // This file's engine is the free one; a binary that says otherwise would
    // make the assertions below about the wrong edition.
    expect(await rpc<string[]>('app.methods')).not.toContain('isp.summary');
    const settings = await rpc<Record<string, unknown>>('settings.get', { section: 'assistant' });
    await wire('settings.get assistant (free)', settings);
    // The key is there, false and inert: the screen does not offer it.
    expect(settings.isp_access).toBe(false);

    await openApp(page, '/assistant', { backend });
    const tabs = page.getByRole('tablist', { name: 'Assistant' });
    await expect(tabs.getByRole('tab')).toHaveText(['Conversation', 'Models', 'Usage']);

    await openApp(page, '/assistant?tab=settings', { backend });
    await expect(tabs.getByRole('tab', { name: 'Conversation' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(page.getByRole('switch')).toHaveCount(0);
    await expect(page.getByText(/customer ledger/i)).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });

  test.describe('in the ISP edition', () => {
    let isp: Backend;

    test.beforeAll(async () => {
      test.setTimeout(900_000);
      isp = await startIspBackend(prebuilt ?? uiDir);
    });

    test.afterAll(async () => {
      await isp?.stop();
    });

    test('is off until somebody switches it on, asks first, and what the engine offers follows it', async ({
      page,
      request,
    }) => {
      const console = watchConsole(page);
      const rpc = rpcFor(request, isp);
      await rpc('ai.providers.save', {
        name: 'Bench model',
        kind: 'openai',
        base_url: model.url,
        model: 'fake-mini',
        api_key: 'sk-not-a-real-key-7Yq3',
        tool_mode: 'native',
        is_default: true,
        context_window: WIDE,
      });
      const ledgerTools = async () =>
        (await rpc<{ name: string }[]>('ai.tools'))
          .map((tool) => tool.name)
          .filter((name) => name.startsWith('isp_'));
      await wire('settings.get assistant (isp)', await rpc('settings.get', { section: 'assistant' }));
      expect(await ledgerTools()).toEqual([]);

      await openApp(page, '/assistant', { backend: isp });
      await page.getByRole('tab', { name: 'Settings' }).click();
      const toggle = page.getByRole('switch', { name: 'Let the assistant read the customer ledger' });
      await expect(toggle).not.toBeChecked();
      // Off is nothing at all, and the screen says so — and what is never
      // sent, whichever way it is set.
      await expect(page.getByText(/reads nothing from it: no customer, no invoice and no total/)).toBeVisible();
      await expect(page.getByText(/Never sent, whichever way this is set: phone numbers/)).toBeVisible();
      await expect(page.getByText('None of the ledger’s tools is offered to the model now.')).toBeVisible();

      // Backing out of the question changes nothing.
      await toggle.click();
      const question = page.getByRole('dialog', { name: 'Let the assistant read the customer ledger?' });
      await expect(question).toContainText('Bench model (fake-mini');
      await expect(question).toContainText('never sent');
      await question.getByRole('button', { name: 'Cancel' }).click();
      await expect(toggle).not.toBeChecked();
      expect((await rpc<{ isp_access: boolean }>('settings.get', { section: 'assistant' })).isp_access).toBe(
        false,
      );

      // From the keyboard: Space on the switch, then the question's own button.
      await toggle.focus();
      await page.keyboard.press('Space');
      await question.getByRole('button', { name: 'Switch it on' }).click();
      await expect(toggle).toBeChecked();
      const on = await rpc<{ isp_access: boolean }>('settings.get', { section: 'assistant' });
      await wire('settings.get assistant (isp, switched on)', on);
      expect(on.isp_access).toBe(true);
      expect(await ledgerTools()).toEqual(['isp_overview', 'isp_customers', 'isp_invoices']);
      await expect(page.getByText(/id, name, state, plan and balance/)).toBeVisible();
      await expect(
        page.getByText('Offered to the default model now: ISP figures, ISP customers, ISP invoices.'),
      ).toBeVisible();

      // Off again asks nothing, and the engine stops offering them.
      await toggle.click();
      await expect(toggle).not.toBeChecked();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      expect((await rpc<{ isp_access: boolean }>('settings.get', { section: 'assistant' })).isp_access).toBe(
        false,
      );
      expect(await ledgerTools()).toEqual([]);
      await expect(page.getByText('None of the ledger’s tools is offered to the model now.')).toBeVisible();

      expect(console.messages).toEqual([]);
    });
  });
});

test.describe('the chat history', () => {
  test('lists conversations, renames one and deletes one', async ({ page, request }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc);

    model.reply([text('First answer.'), stopped()]);
    model.reply([text('Second answer.'), stopped()]);

    await openApp(page, '/assistant', { backend });
    await ask(page, 'the first question');
    await expect(page.getByText('First answer.')).toBeVisible();

    await page.getByRole('button', { name: 'New chat' }).first().click();
    await ask(page, 'the second question');
    await expect(page.getByText('Second answer.')).toBeVisible();

    // Both are in the rail, titled from what was asked.
    const rail = page.getByRole('complementary', { name: 'Conversations' });
    await expect(rail.getByRole('button', { name: /^the first question/ })).toBeVisible();
    await expect(rail.getByRole('button', { name: /^the second question/ })).toBeVisible();

    // Going back to the first one brings its transcript back.
    await rail.getByRole('button', { name: /^the first question/ }).click();
    await expect(page.getByText('First answer.')).toBeVisible();

    await rail.getByRole('button', { name: /Actions for the first question/ }).click();
    await page.getByRole('menuitem', { name: 'Rename' }).click();
    await page.getByRole('textbox', { name: 'Chat name' }).fill('Monday incident');
    await page.getByRole('textbox', { name: 'Chat name' }).press('Enter');
    await expect(rail.getByRole('button', { name: /^Monday incident/ })).toBeVisible();

    await rail.getByRole('button', { name: /Actions for Monday incident/ }).click();
    await page.getByRole('menuitem', { name: 'Delete' }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Delete' }).click();
    await expect(rail.getByRole('button', { name: /^Monday incident/ })).toHaveCount(0);

    expect(console.messages).toEqual([]);
  });
});

test.describe('the dock', () => {
  test('answers beside another screen and carries that screen as context', async ({
    page,
    request,
  }) => {
    const console = watchConsole(page);
    const rpc = rpcFor(request, backend);
    await reset(rpc);
    await configure(rpc);
    await seedDevice(rpc);

    model.reply([text('Nothing is wrong with the inventory.'), stopped()]);

    await openApp(page, '/devices', { backend, dock: true });
    const dock = page.getByRole('complementary', { name: 'Assistant' });
    await expect(dock).toBeVisible();

    await dock.getByRole('textbox', { name: 'Message to the assistant' }).fill('anything wrong?');
    await dock.getByRole('textbox', { name: 'Message to the assistant' }).press('Enter');
    await expect(dock.getByText('Nothing is wrong with the inventory.')).toBeVisible();

    // The screen the question was asked from travelled with it.
    const sent = model.requests.at(-1) as { messages: { role: string; content: string }[] };
    const asked = sent.messages.map((message) => message.content).join('\n');
    expect(asked).toContain('/devices');

    expect(console.messages).toEqual([]);
  });
});
