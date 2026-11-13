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
import { mkdtemp, rm } from 'node:fs/promises';
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

/** Points the engine at the fake model, and gives it a device to look at. */
async function configure(rpc: Rpc, options: { confirmReads?: boolean } = {}): Promise<void> {
  await rpc('ai.providers.save', {
    name: 'Bench model',
    kind: 'openai',
    base_url: model.url,
    model: 'fake-mini',
    api_key: 'sk-not-a-real-key-7Yq3',
    tool_mode: 'native',
    is_default: true,
  });
  await rpc('settings.set', {
    section: 'assistant',
    value: { confirm_reads: options.confirmReads ?? false },
  });
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
