// @vitest-environment happy-dom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AppEvent } from '@/api/events';
import type { SessionInfo, TerminalLineClass } from '@/api/types/terminal';
import { ConfirmHost } from '@/design/Dialog';

/**
 * The registry against a backend that is a few lines of test: what it asks
 * for and what it sends are recorded, and the test says what comes back.
 * xterm is the real one — it parses and keeps its modes without a screen.
 */
const backend = vi.hoisted(() => ({
  calls: [] as [string, Record<string, unknown>][],
  handlers: new Map<string, (event: AppEvent<never>) => void>(),
  answer: (_method: string, _params: Record<string, unknown>): unknown => ({}),
}));

vi.mock('@/api/client', () => ({
  rpc: (method: string, params: Record<string, unknown>) => {
    backend.calls.push([method, params]);
    return Promise.resolve().then(() => backend.answer(method, params));
  },
  subscribe: (topic: string, handler: (event: AppEvent<never>) => void) => {
    backend.handlers.set(topic, handler);
    return () => backend.handlers.delete(topic);
  },
  useConnection: { subscribe: () => () => undefined },
}));

const { terminals } = await import('./sessions');

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
const base64 = (text: string) => btoa(text);
const called = (method: string) => backend.calls.filter(([name]) => name === method);
const sentTo = (id: string) =>
  called('terminal.input')
    .filter(([, params]) => params.session_id === id)
    .map(([, params]) => params.data);

function info(id: string, over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id,
    title: 'SW1',
    kind: 'telnet',
    device_id: 'dev-1',
    lab: false,
    opened_at: 0,
    state: 'open',
    cols: 80,
    rows: 24,
    info: { peer: '10.0.0.1:23', legacy_crypto: false, via: [] },
    bytes_in: 0,
    bytes_out: 0,
    ...over,
  };
}

/** A backend that holds `seq` events of scrollback for every session. */
function backendAt(seq: number, classify?: (text: string) => TerminalLineClass[]) {
  backend.answer = (method, params) => {
    if (method === 'terminal.attach') {
      return { info: info(String(params.session_id)), scrollback: base64('SW1> '), seq };
    }
    if (method === 'terminal.classify') {
      const text = String(params.text);
      return classify
        ? classify(text)
        : text.split('\n').map((line) => ({ line, class: 'read_only', reason: 'read-only command' }));
    }
    return {};
  };
}

/** The browser's own paste, landing where xterm's textarea is: inside the terminal's element. */
function browserPaste(id: string, text: string): { reachedXterm: boolean; prevented: boolean } {
  const host = [...document.querySelectorAll<HTMLElement>('.nocturne-terminal-host')].at(-1);
  if (!host) throw new Error(`no terminal element for ${id}`);
  // Stands where xterm's own listeners are: on an element inside the host.
  const inner = document.createElement('textarea');
  host.append(inner);
  let reachedXterm = false;
  inner.addEventListener('paste', () => {
    reachedXterm = true;
  });
  const event = new Event('paste', { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'clipboardData', { value: { getData: () => text } });
  inner.dispatchEvent(event);
  inner.remove();
  return { reachedXterm, prevented: event.defaultPrevented };
}

let next = 0;
/** A session with a terminal, attached. */
async function adopt(): Promise<string> {
  next += 1;
  const id = `sess-${next}`;
  terminals.adopt(id);
  await settle();
  return id;
}

beforeEach(() => {
  backend.calls.length = 0;
  backendAt(0);
});

afterEach(() => {
  for (const id of terminals.ids()) terminals.forget(id);
  terminals.setBroadcast(null);
  terminals.setPasteAsker(null);
});

describe('a paste the browser raises', () => {
  it('never reaches xterm, which would send it before anyone was asked', async () => {
    const id = await adopt();
    const answers: ((confirmed: boolean) => void)[] = [];
    terminals.setPasteAsker(() => new Promise((resolve) => answers.push(resolve)));

    const paste = browserPaste(id, 'conf t\r\nreload\r\n');
    await settle();

    expect(paste).toEqual({ reachedXterm: false, prevented: true });
    // The question is open: the policy was asked, the device was sent nothing.
    expect(called('terminal.classify')).toEqual([
      ['terminal.classify', { device_id: 'dev-1', text: 'conf t\nreload\n' }],
    ]);
    expect(answers).toHaveLength(1);
    expect(sentTo(id)).toEqual([]);

    answers[0]?.(true);
    await settle();
    expect(sentTo(id)).toEqual(['conf t\rreload\r']);
  });

  it('is dropped whole when the person says no', async () => {
    const id = await adopt();
    terminals.setPasteAsker(() => Promise.resolve(false));

    browserPaste(id, 'conf t\nreload\n');
    await settle();

    expect(sentTo(id)).toEqual([]);
  });

  it('goes to every session of a broadcast, once, and only after the question', async () => {
    const first = await adopt();
    const second = await adopt();
    const broadcast: string[] = [];
    terminals.setBroadcast({ targets: [first, second], send: (data) => broadcast.push(data) });
    const asked: number[] = [];
    terminals.setPasteAsker((question) => {
      asked.push(question.targets);
      return Promise.resolve(true);
    });

    browserPaste(second, 'show version');
    await settle();

    // One read-only line would not be asked about for one session; for two it is.
    expect(asked).toEqual([2]);
    expect(broadcast).toEqual(['show version']);
    expect(called('terminal.input')).toEqual([]);
  });

  it('is bracketed once the program at the far end has asked for that', async () => {
    const id = await adopt();
    terminals.setPasteAsker(() => Promise.resolve(true));
    // DECSET 2004, as a shell sends it before its prompt.
    backend.handlers.get('terminal.output')?.({
      topic: 'terminal.output',
      at: 0,
      payload: { session_id: id, seq: 1, data: base64('\u001b[?2004h$ ') } as never,
    });
    // xterm parses what it is written a moment later.
    await new Promise((resolve) => setTimeout(resolve, 20));

    terminals.paste(id, 'ls\npwd\n');
    await settle();

    expect(sentTo(id)).toEqual(['\u001b[200~ls\rpwd\r\u001b[201~']);
  });
});

describe('a screen that lends a terminal without a paste sheet of its own', () => {
  // A lab's consoles and the session docked on the map: they mount the
  // terminal and nothing else, and used to get xterm's unguarded paste.
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

  const button = (name: string) =>
    [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')].find((element) =>
      (element.textContent ?? '').includes(name),
    );

  async function withConfirmHost(run: () => Promise<void>): Promise<void> {
    const container = document.createElement('div');
    document.body.append(container);
    const root = createRoot(container);
    await act(async () => root.render(<ConfirmHost />));
    try {
      await run();
    } finally {
      await act(async () => root.unmount());
      container.remove();
    }
  }

  it('is asked with the plain dialog, and the device gets nothing until it is answered', async () => {
    const id = await adopt();

    await withConfirmHost(async () => {
      await act(async () => {
        browserPaste(id, 'conf t\nreload\n');
        await settle();
      });

      const dialog = document.querySelector('[role="dialog"]');
      expect(dialog?.textContent).toContain('Paste into SW1?');
      expect(dialog?.textContent).toContain('2 lines will be sent to the device');
      expect(sentTo(id)).toEqual([]);

      await act(async () => {
        button('Paste anyway')?.click();
        await settle();
      });
      expect(sentTo(id)).toEqual(['conf t\rreload\r']);
    });
  });

  it('sends nothing when the plain dialog is cancelled', async () => {
    const id = await adopt();

    await withConfirmHost(async () => {
      await act(async () => {
        browserPaste(id, 'conf t\nreload\n');
        await settle();
      });
      await act(async () => {
        button('Cancel')?.click();
        await settle();
      });

      expect(document.querySelector('[role="dialog"]')).toBeNull();
      expect(sentTo(id)).toEqual([]);
    });
  });
});

describe('typing', () => {
  it('goes straight out when no paste is in the way', async () => {
    const id = await adopt();

    terminals.input(id, 'show version\r');
    await settle();

    expect(sentTo(id)).toEqual(['show version\r']);
  });

  it('follows a paste it was typed after, even when the policy is slow to answer', async () => {
    const id = await adopt();
    let answerPolicy: (classes: TerminalLineClass[]) => void = () => undefined;
    backend.answer = (method) =>
      method === 'terminal.classify'
        ? new Promise<TerminalLineClass[]>((resolve) => {
            answerPolicy = resolve;
          })
        : {};

    terminals.paste(id, 'show version');
    await settle();
    terminals.input(id, '\r');
    await settle();
    expect(sentTo(id)).toEqual([]);

    answerPolicy([{ line: 'show version', class: 'read_only', reason: 'read-only command' }]);
    await settle();
    await settle();

    expect(sentTo(id).join('')).toBe('show version\r');
  });
});

describe('when the server says it dropped events', () => {
  it('asks for the scrollback of every session again', async () => {
    const first = await adopt();
    const second = await adopt();
    backend.calls.length = 0;

    backend.handlers.get('events.lagged')?.({ topic: 'events.lagged', at: 0, payload: { missed: 2800 } as never });
    await settle();

    expect(called('terminal.attach').map(([, params]) => params.session_id)).toEqual([first, second]);
  });

  it('asks again by itself when an output event is not the next one', async () => {
    const id = await adopt();
    backend.calls.length = 0;
    const output = backend.handlers.get('terminal.output');

    output?.({ topic: 'terminal.output', at: 0, payload: { session_id: id, seq: 1, data: base64('a') } as never });
    expect(called('terminal.attach')).toEqual([]);
    // 2 to 8 never arrived; the backend has them in its scrollback.
    backendAt(9);
    output?.({ topic: 'terminal.output', at: 0, payload: { session_id: id, seq: 9, data: base64('b') } as never });
    await settle();

    expect(called('terminal.attach')).toEqual([['terminal.attach', { session_id: id }]]);
  });

  it('does not ask in a tight loop when the answers keep falling short of the hole', async () => {
    // A backend that says "nothing since the start" however often it is
    // asked, while events far ahead of that keep arriving.
    const id = await adopt();
    backend.calls.length = 0;
    const output = backend.handlers.get('terminal.output');

    output?.({ topic: 'terminal.output', at: 0, payload: { session_id: id, seq: 9, data: base64('b') } as never });
    await new Promise((resolve) => setTimeout(resolve, 60));

    // Once at once; the second try is waiting its turn, not spinning.
    expect(called('terminal.attach')).toHaveLength(1);
    expect(terminals.view(id).attach).toBe('live');
  });

  it('asks once more after an answer that was already on its way, and not once per notice', async () => {
    const id = await adopt();
    backend.calls.length = 0;
    let answerAttach: (value: unknown) => void = () => undefined;
    backend.answer = (method) =>
      method === 'terminal.attach'
        ? new Promise((resolve) => {
            answerAttach = resolve;
          })
        : {};
    const lagged = () =>
      backend.handlers.get('events.lagged')?.({ topic: 'events.lagged', at: 0, payload: { missed: 1 } as never });

    lagged();
    lagged();
    lagged();
    await settle();
    // Each answer carries the whole scrollback: three notices are not three of them.
    expect(called('terminal.attach')).toHaveLength(1);

    // The answer in flight may predate the last notice, so one more follows it.
    answerAttach({ info: info(id), scrollback: base64('SW1> '), seq: 0 });
    await settle();
    expect(called('terminal.attach')).toHaveLength(2);
    answerAttach({ info: info(id), scrollback: base64('SW1> '), seq: 0 });
    await settle();
    expect(called('terminal.attach')).toHaveLength(2);
  });
});
