// @vitest-environment happy-dom
/**
 * "Keep as evidence…" for the three kinds that are text: the last lines of
 * a terminal session, a page of a session log, and text a person holds.
 *
 * What is held here: for the first two the dialog sends *what to read* and
 * shows the engine's own redacted read of it before anything is stored; for
 * the third it says, before it is sent, that the item will be the
 * operator's, and afterwards shows what the engine stored. The real engine
 * is driven by `tests/terminal.spec.ts`.
 *
 * Every engine answer below was recorded from a running `nocturne-server`.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RpcError } from '@/api/errors';
import type { EvidenceWithBody } from '@/api/types/ops';
import { useToasts } from '@/design/Toast';
import { useKeepAsEvidence, type KeepAsEvidence, type KeepRequest } from './KeepAsEvidence';
import { captureCall, describeRequest } from './keep';

const engine = vi.hoisted(() => ({
  methods: [] as string[],
  calls: [] as { method: string; params: unknown }[],
  answer: (_method: string, _params: unknown): unknown => undefined,
}));

vi.mock('@/api/hooks', () => ({
  useRpc: (method: string) => {
    if (method === 'app.methods') return { data: engine.methods, isError: false, error: null };
    if (method === 'incidents.list') return { data: { incidents: [], counts: {} }, isError: false, error: null };
    return { data: undefined, isError: false, error: null };
  },
}));

vi.mock('@/api/client', () => ({
  rpc: async (method: string, params: unknown) => {
    engine.calls.push({ method, params });
    const result = engine.answer(method, params);
    if (result instanceof Error) throw result;
    return result;
  },
}));

const SNAPSHOT = {
  text: '[line number 27]\nSW1> say line number 28\n[line number 28]\nSW1> show secret\nenable secret <secret:1>\nSW1>',
  lines: 6,
  redacted: true,
};

const FROM_SESSION: EvidenceWithBody = {
  id: '3oaad2qr5xtdrztm',
  kind: 'terminal',
  title: 'the last six',
  captured_at: 1795400989945,
  source: {
    from: 'terminal.snapshot',
    session_id: 'bm6f4g3axa7smevc',
    session_title: 'SW1-LAB',
    device_id: 'wlkdtfcp6sovgrze',
    device: 'SW1-LAB',
    peer: '127.0.0.1:42199',
    lab: false,
    params: { lines: 6 },
  },
  redacted: true,
  lines: 6,
  bytes: 103,
  sha256: 'd264b30f29589488adcb925bdc6fb6a7702a7b80cb6a30a33a556c74ef06d3eb',
  incident_ids: [],
  body: { type: 'text', text: SNAPSHOT.text },
};

const FROM_OPERATOR: EvidenceWithBody = {
  id: 'xenkcd27tblgf4ds',
  kind: 'text',
  title: 'Selection from SW1-LAB',
  captured_at: 1795400990010,
  source: {
    from: 'operator',
    session_id: 'bm6f4g3axa7smevc',
    session_title: 'SW1-LAB',
    device_id: 'wlkdtfcp6sovgrze',
    device: 'SW1-LAB',
    peer: '127.0.0.1:42199',
    lab: false,
    note: 'text as sent by the client, redacted on arrival; said to be from session `SW1-LAB`',
  },
  redacted: true,
  lines: 2,
  bytes: 41,
  sha256: 'ff9ac028b883b1ed3ee7664e27de7bf8f1ec12fcd914e0edfb0f83aa8e6edaab',
  incident_ids: [],
  body: { type: 'text', text: 'SW1> show secret\nenable secret <secret:1>' },
};

const SELECTION: KeepRequest = {
  kind: 'text',
  text: 'SW1> show secret\nenable secret Sup3rS3cret',
  title: 'Selection from SW1-LAB',
  session_id: 'bm6f4g3axa7smevc',
  from: 'the selection in SW1-LAB',
};

let root: Root | null = null;
let host: HTMLElement | null = null;
let keep: KeepAsEvidence | null = null;

function Screen() {
  keep = useKeepAsEvidence();
  return <>{keep.dialog}</>;
}

function mount(): KeepAsEvidence {
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  act(() => root?.render(<Screen />));
  if (!keep) throw new Error('the screen did not render');
  return keep;
}

const dialog = (): HTMLElement | null => document.querySelector('[role="dialog"]');

function button(name: string): HTMLButtonElement {
  const found = [...document.querySelectorAll('button')].find((candidate) => candidate.textContent?.trim() === name);
  if (!found) throw new Error(`no button called “${name}”`);
  return found;
}

function field(label: string): HTMLInputElement {
  const labels = [...document.querySelectorAll('label')].filter((candidate) => candidate.textContent?.startsWith(label));
  const input = labels.map((found) => document.getElementById(found.htmlFor)).find(Boolean);
  if (!(input instanceof HTMLInputElement)) throw new Error(`no field called “${label}”`);
  return input;
}

function type(input: HTMLInputElement, text: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
  act(() => {
    setter?.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

async function settled(): Promise<void> {
  await act(async () => {
    for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
  });
}

/** Past the pause the dialog waits for the number of lines to stand still. */
async function afterThePause(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(300);
  });
  await settled();
}

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers();
  engine.methods = ['evidence.capture_terminal', 'evidence.capture_log', 'evidence.add_text', 'evidence.delete'];
  engine.calls = [];
  engine.answer = (method) => {
    if (method === 'terminal.snapshot') return SNAPSHOT;
    if (method === 'terminal.logs.read') return { text: '2026-11-23T02:29:48.566+00:00 SW1> say hello\n', redacted: false };
    if (method === 'evidence.add_text') return FROM_OPERATOR;
    if (method === 'evidence.delete') return { deleted: true, tombstones: [] };
    return FROM_SESSION;
  };
  useToasts.setState({ toasts: [], history: [], unread: 0 });
  window.localStorage.clear();
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  keep = null;
  vi.useRealTimers();
});

describe('what is asked of the engine', () => {
  it('names a session and a number of lines, never their text', () => {
    expect(captureCall({ kind: 'terminal', session_id: 'bm6f4g3axa7smevc', lines: 6, session: 'SW1-LAB' }, ' the last six ')).toEqual({
      method: 'evidence.capture_terminal',
      params: { session_id: 'bm6f4g3axa7smevc', lines: 6, title: 'the last six' },
    });
  });

  it('names the bytes of a log page as the engine gave them', () => {
    const request: KeepRequest = { kind: 'log', name: '2026-11/SW1-LAB-20261123-022948.log', offset: 340, limit: 266, log: 'SW1-LAB' };
    expect(captureCall(request, '')).toEqual({
      method: 'evidence.capture_log',
      params: { name: '2026-11/SW1-LAB-20261123-022948.log', offset: 340, limit: 266 },
    });
    expect(describeRequest(request)).toContainEqual({ label: 'Read with', value: 'terminal.logs.read' });
  });

  it('sends text a person holds exactly as it is, with the session it is said to be from', () => {
    expect(captureCall(SELECTION, 'Selection from SW1-LAB')).toEqual({
      method: 'evidence.add_text',
      params: {
        text: 'SW1> show secret\nenable secret Sup3rS3cret',
        title: 'Selection from SW1-LAB',
        session_id: 'bm6f4g3axa7smevc',
      },
    });
    // What the dialog shows about it says whose it is.
    expect(describeRequest(SELECTION)).toContainEqual({
      label: 'Supplied by',
      value: 'you — the engine did not read this itself, and the item will say so',
    });
  });
});

describe('a build that cannot keep these', () => {
  it('offers none of the three and opens nothing', () => {
    engine.methods = ['evidence.capture_chart', 'terminal.snapshot'];
    const screen = mount();
    expect(screen.can('terminal')).toBe(false);
    expect(screen.can('log')).toBe(false);
    expect(screen.can('text')).toBe(false);
    act(() => screen.keep(SELECTION));
    expect(dialog()).toBeNull();
    expect(engine.calls).toEqual([]);
  });
});

describe('keeping the last lines of a session', () => {
  it('shows the engine’s redacted read before anything is stored', async () => {
    const screen = mount();
    act(() => screen.keep({ kind: 'terminal', session_id: 'bm6f4g3axa7smevc', lines: 6, session: 'SW1-LAB' }));
    await afterThePause();

    expect(engine.calls).toEqual([{ method: 'terminal.snapshot', params: { session_id: 'bm6f4g3axa7smevc', lines: 6 } }]);
    const preview = dialog()?.querySelector('[data-testid="keep-preview"]');
    expect(preview?.textContent).toContain('enable secret <secret:1>');
    expect(preview?.textContent).toContain('Secrets in it are already replaced.');
    expect(preview?.textContent).toContain('can hide more than this and never less');
  });

  it('keeps as many lines as the field says, and sends no text', async () => {
    const screen = mount();
    act(() => screen.keep({ kind: 'terminal', session_id: 'bm6f4g3axa7smevc', lines: 200, session: 'SW1-LAB' }));
    await afterThePause();
    type(field('Lines from the end'), '6');
    await afterThePause();
    // Read again for the new number, so what is on screen is what is asked for.
    expect(engine.calls.at(-1)).toEqual({ method: 'terminal.snapshot', params: { session_id: 'bm6f4g3axa7smevc', lines: 6 } });

    act(() => button('Keep').click());
    await settled();

    const kept = engine.calls.find((call) => call.method === 'evidence.capture_terminal');
    expect(kept?.params).toEqual({ session_id: 'bm6f4g3axa7smevc', lines: 6 });
    expect(JSON.stringify(kept)).not.toContain('line number');
    expect(dialog()).toBeNull();
    expect(useToasts.getState().toasts.map((toast) => toast.title)).toEqual(['Kept as evidence']);
  });

  it('does not ask for anything while the number of lines is not a number', async () => {
    const screen = mount();
    act(() => screen.keep({ kind: 'terminal', session_id: 'bm6f4g3axa7smevc', lines: 200 }));
    await afterThePause();
    engine.calls = [];
    type(field('Lines from the end'), 'all of it');
    await afterThePause();

    expect(engine.calls).toEqual([]);
    expect(button('Keep').disabled).toBe(true);
    expect(dialog()?.textContent).toContain('A whole number of lines.');
  });

  it('says so when the session cannot be read to show it, in the engine’s words', async () => {
    engine.answer = (method) =>
      method === 'terminal.snapshot' ? new RpcError('not_found', 'terminal session `nope` was not found') : FROM_SESSION;
    const screen = mount();
    act(() => screen.keep({ kind: 'terminal', session_id: 'nope', lines: 6 }));
    await afterThePause();

    expect(dialog()?.querySelector('[role="alert"]')?.textContent).toContain('terminal session `nope` was not found');
  });
});

describe('keeping a page of a session log', () => {
  it('shows what the engine reads at those bytes', async () => {
    const screen = mount();
    act(() => screen.keep({ kind: 'log', name: '2026-11/SW1-LAB-20261123-022948.log', offset: 340, limit: 266, log: 'SW1-LAB' }));
    await settled();

    expect(engine.calls).toEqual([
      { method: 'terminal.logs.read', params: { name: '2026-11/SW1-LAB-20261123-022948.log', offset: 340, limit: 266 } },
    ]);
    // Redacted: the dialog never asks for a log as it was written.
    expect(JSON.stringify(engine.calls)).not.toContain('redact');
    expect(dialog()?.querySelector('[data-testid="keep-preview"]')?.textContent).toContain('SW1> say hello');
  });
});

describe('keeping text a person holds', () => {
  it('says it is the operator’s before it is sent, and asks the engine to read nothing', async () => {
    const screen = mount();
    act(() => screen.keep(SELECTION));
    await settled();

    const preview = dialog()?.querySelector('[data-testid="keep-preview"]');
    expect(preview?.textContent).toContain('Supplied by the operator');
    expect(preview?.textContent).toContain('not yet redacted');
    expect(dialog()?.textContent).toContain('it keeps it as supplied by the operator');
    expect(engine.calls).toEqual([]);
  });

  it('needs a title, because the engine did not read it and cannot name it', async () => {
    const screen = mount();
    act(() => screen.keep(SELECTION));
    await settled();
    type(field('Title'), '   ');
    expect(button('Keep').disabled).toBe(true);
    type(field('Title'), 'What SW1 said');
    expect(button('Keep').disabled).toBe(false);
  });

  it('shows what was stored, as the engine redacted it, and can delete it there', async () => {
    const screen = mount();
    act(() => screen.keep(SELECTION));
    await settled();
    act(() => button('Keep').click());
    await settled();

    // Still open, on what the engine kept: the secret that was sent is not in it.
    const stored = dialog()?.querySelector('[data-testid="keep-stored"]');
    expect(stored?.textContent).toContain('enable secret <secret:1>');
    expect(stored?.textContent).not.toContain('Sup3rS3cret');
    expect(stored?.textContent).toContain('supplied by the operator, redacted on arrival');

    act(() => button('Delete it').click());
    await settled();
    expect(engine.calls.at(-1)).toEqual({ method: 'evidence.delete', params: { id: 'xenkcd27tblgf4ds' } });
    expect(dialog()).toBeNull();
  });

  it('leaves the dialog open with the engine’s reason when the text is refused', async () => {
    engine.answer = () =>
      new RpcError(
        'invalid',
        'This is 257 KiB of text and an evidence item holds at most 256 KiB: keep a smaller selection, or keep it as several items. Nothing was stored.',
        { limit: 'text_bytes', max: 262144, actual: 262150 },
      );
    const screen = mount();
    act(() => screen.keep(SELECTION));
    await settled();
    act(() => button('Keep').click());
    await settled();

    const refused = dialog()?.querySelector('[data-testid="keep-refused"]');
    expect(refused?.textContent).toContain('Too much text for one item');
    expect(refused?.textContent).toContain('Nothing was stored.');
    expect(dialog()?.querySelector('[data-testid="keep-stored"]')).toBeNull();
  });
});
