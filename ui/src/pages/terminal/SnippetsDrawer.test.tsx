// @vitest-environment happy-dom
/**
 * The snippets drawer, as far as a DOM without an engine can show.
 *
 * Held here: what the drawer offers depends on what the engine has, and is
 * never guessed; a snippet the engine cannot send is shown with the engine's
 * reason and cannot be sent; a refusal lands beside the field it is about;
 * and what is sent is held to the hash of what was on screen.
 * `tests/terminal.spec.ts` does the same against a real engine and a console
 * that records what it was sent.
 *
 * The rows, the renders and the refusals are answers recorded from a
 * running `nocturne-server` (ids shortened). The row with a `problem` is
 * from the engine's own test run: such a snippet can only exist in a store
 * written before today's rules.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { MemoryRouter } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RpcError } from '@/api/errors';
import type { SessionInfo, Snippet } from '@/api/types/terminal';
import { useToasts } from '@/design/Toast';
import { SnippetsDrawer, type SnippetsDrawerProps } from './SnippetsDrawer';

const engine = vi.hoisted(() => ({
  methods: undefined as string[] | undefined,
  snippets: [] as unknown[],
  listed: [] as unknown[],
  calls: [] as { method: string; params: unknown }[],
  answer: (_method: string, _params: unknown): unknown => undefined,
  questions: [] as unknown[],
  confirmed: true,
  /** While set, the question stays open: the answer comes when this resolves. */
  open: null as Promise<void> | null,
}));

vi.mock('@/api/hooks', () => ({
  useHasMethod: (method: string) => (engine.methods === undefined ? undefined : engine.methods.includes(method)),
  useRpc: (method: string, params: unknown) => {
    if (method === 'terminal.snippets.list') engine.listed.push(params);
    return { data: engine.snippets, isError: false, isPending: false, error: null, refetch: () => Promise.resolve() };
  },
  useRpcMutation: () => ({ mutate: () => undefined, reset: () => undefined, isPending: false, error: null }),
}));

vi.mock('@/api/client', () => ({
  rpc: async (method: string, params: unknown) => {
    engine.calls.push({ method, params });
    const result = engine.answer(method, params);
    if (result instanceof Error) throw result;
    return result;
  },
}));

// The registry owns xterm; all the drawer needs of it is the question.
vi.mock('./sessions', () => ({
  terminals: {
    confirm: async (question: unknown) => {
      engine.questions.push(question);
      if (engine.open) await engine.open;
      return engine.confirmed;
    },
  },
}));

const V2 = [
  'terminal.snippets.render',
  'terminal.snippets.send',
  'terminal.snippets.export',
  'terminal.snippets.import',
  'runner.run',
];

const VERSION: Snippet = { id: '3zukjtz7', name: 'Version', text: 'show version', profile: null, tags: ['daily'], created_at: 1, updated_at: 1, variables: [], chosen: true };
const SAY: Snippet = {
  id: '2p4lig3f',
  name: 'Say',
  text: 'say {{what}} on {{port=Gi0/1}}\nsay done\n',
  profile: null,
  tags: ['daily', 'ports'],
  created_at: 2,
  updated_at: 2,
  variables: [{ name: 'what' }, { name: 'port', default: 'Gi0/1' }],
  chosen: true,
};
/** One line that only reads: typed without a question, so nothing but the form stands between a key and the device. */
const PORT: Snippet = {
  id: 'port1',
  name: 'Port',
  text: 'show interface {{port}}',
  profile: null,
  tags: [],
  created_at: 4,
  updated_at: 4,
  variables: [{ name: 'port' }],
  chosen: true,
};
/** Stored before today's rules: its text holds an escape character. */
const OLD: Snippet = {
  id: 'old',
  name: 'Old',
  text: 'show interface {{port=Gi0/1\u001b[201~}}',
  profile: null,
  tags: [],
  created_at: 1,
  updated_at: 1,
  variables: [],
  problem: 'line 1, column 16: the default of `port` contains a control character (U+001B)',
};

const SESSION: SessionInfo = {
  id: 'bm6f4g3a',
  title: 'SW1-LAB',
  kind: 'telnet',
  device_id: 'wlkdtfcp',
  lab: false,
  opened_at: 1,
  state: 'open',
  cols: 120,
  rows: 40,
  info: { peer: '127.0.0.1:42199', legacy_crypto: false, via: [] },
  bytes_in: 0,
  bytes_out: 0,
};

const RENDERED = {
  text: 'say hello on Gi0/1\nsay done',
  lines: 2,
  sha256: 'fffbddf1bd3ed2af219e53d16b6debc50ce27325084c010f19d971083c086c76',
  variables: SAY.variables,
};

let root: Root | null = null;
let host: HTMLElement | null = null;
const onPaste = vi.fn();
const onSent = vi.fn();

function draw(props: Partial<SnippetsDrawerProps> = {}): void {
  act(() =>
    root?.render(
      <MemoryRouter>
        <SnippetsDrawer
          open
          onClose={() => undefined}
          profile={null}
          sessions={[SESSION]}
          focusedId={SESSION.id}
          broadcast={[]}
          onPaste={onPaste}
          onSent={onSent}
          {...props}
        />
      </MemoryRouter>,
    ),
  );
}

function open(props: Partial<SnippetsDrawerProps> = {}): void {
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  draw(props);
}

const drawer = (): HTMLElement => {
  const found = document.querySelector<HTMLElement>('[role="dialog"]');
  if (!found) throw new Error('the drawer is not open');
  return found;
};

function rowButton(name: string): HTMLButtonElement {
  const found = [...drawer().querySelectorAll<HTMLButtonElement>('li > button')].find((button) =>
    button.textContent?.startsWith(name),
  );
  if (!found) throw new Error(`no row called “${name}”`);
  return found;
}

function button(text: RegExp): HTMLButtonElement | undefined {
  return [...drawer().querySelectorAll('button')].find(
    (candidate) => text.test(candidate.textContent?.trim() ?? '') || text.test(candidate.getAttribute('aria-label') ?? ''),
  );
}

/** A variable's field. It is a textarea: what is put into it stays as it arrived, line breaks and all. */
function field(label: string): HTMLTextAreaElement {
  const found = [...drawer().querySelectorAll('label')].find((candidate) => candidate.textContent?.startsWith(label));
  const input = found ? document.getElementById(found.htmlFor) : null;
  if (!(input instanceof HTMLTextAreaElement)) throw new Error(`no field called “${label}”`);
  return input;
}

function type(input: HTMLTextAreaElement, text: string): void {
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set;
  act(() => {
    setter?.call(input, text);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

/** The Enter key in a field, as a keyboard gives it. */
async function enter(input: HTMLTextAreaElement): Promise<boolean> {
  const event = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
  act(() => {
    input.dispatchEvent(event);
  });
  await settled();
  return event.defaultPrevented;
}

/** Past the pause the form waits after a key, and through the answers that follow. */
async function settled(): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(200);
    for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
  });
}

async function press(target: HTMLButtonElement | undefined): Promise<void> {
  if (!target) throw new Error('that button is not on screen');
  act(() => target.click());
  await settled();
}

const called = (method: string) => engine.calls.filter((call) => call.method === method);

beforeEach(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  vi.useFakeTimers();
  engine.methods = [...V2];
  engine.snippets = [SAY, VERSION];
  engine.listed = [];
  engine.calls = [];
  engine.questions = [];
  engine.confirmed = true;
  engine.open = null;
  engine.answer = (method, params) => {
    if (method === 'terminal.snippets.render') {
      const { id, values } = params as { id: string; values: Record<string, string> };
      if (id === VERSION.id) return { text: 'show version', lines: 1, sha256: 'aa11', variables: [] };
      if (id === PORT.id) {
        if (values.port === undefined) {
          return new RpcError('invalid', '`Port` cannot be filled in: `port` has no value and no default', {
            variables: [{ name: 'port', problem: 'missing' }],
          });
        }
        return { text: `show interface ${values.port}`, lines: 1, sha256: 'cc33', variables: PORT.variables };
      }
      if (values.what === undefined) {
        return new RpcError('invalid', '`Say` cannot be filled in: `what` has no value and no default', {
          variables: [{ name: 'what', problem: 'missing' }],
        });
      }
      if (values.what.includes('\t')) {
        return new RpcError('invalid', '`Say` cannot be filled in: the value of `what` contains a control character (U+0009)', {
          variables: [{ name: 'what', problem: 'control_character' }],
        });
      }
      if (/[\n\r]/.test(values.what)) {
        return new RpcError('invalid', '`Say` cannot be filled in: the value of `what` contains a line break', {
          variables: [{ name: 'what', problem: 'line_break' }],
        });
      }
      return RENDERED;
    }
    if (method === 'terminal.classify') {
      return (params as { text: string }).text.split('\n').map((line) => ({ line, class: 'read_only', reason: 'reads' }));
    }
    if (method === 'terminal.snippets.send') {
      const { session_ids } = params as { session_ids: string[] };
      return { sent: session_ids.map((session_id) => ({ session_id, lines: 1 })), failed: [] };
    }
    return undefined;
  };
  onPaste.mockReset();
  onSent.mockReset();
  useToasts.setState({ toasts: [], history: [], unread: 0 });
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
  vi.useRealTimers();
});

describe('with an engine that cannot render or send a snippet', () => {
  it('is the drawer it was: a click goes down the paste funnel and nothing newer is drawn', async () => {
    engine.methods = ['terminal.snippets.list', 'terminal.snippets.save'];
    // A row as an engine from before variables answers it: no `variables` at all.
    engine.snippets = [{ id: VERSION.id, name: 'Version', text: 'show version', profile: null, tags: ['daily'], created_at: 1, updated_at: 1 }];
    open({ profile: 'cisco_ios' });

    // Asked by device type, as before: such an engine knows no `session_id`.
    expect(engine.listed.at(-1)).toEqual({ profile: 'cisco_ios' });
    expect(button(/Import and export/)).toBeUndefined();
    expect(button(/^Choose where to send/)).toBeUndefined();
    expect(drawer().textContent).not.toContain('Automation');

    await press(rowButton('Version'));
    expect(onPaste).toHaveBeenCalledTimes(1);
    expect(onPaste.mock.calls[0]?.[0]).toMatchObject({ id: VERSION.id, text: 'show version' });
    expect(engine.calls).toEqual([]);
  });

  it('offers nothing newer while the method list has not arrived, rather than guessing', () => {
    engine.methods = undefined;
    open();
    expect(engine.listed.at(-1)).toEqual({ profile: null });
    expect(button(/Import and export/)).toBeUndefined();
    expect(button(/^Choose where to send/)).toBeUndefined();
  });
});

describe('with an engine that does', () => {
  it('asks for the snippets of the session in front, so the choice among variants is the engine’s', () => {
    open();
    expect(engine.listed.at(-1)).toEqual({ session_id: SESSION.id });
    expect(button(/Import and export/)).toBeDefined();
    expect(drawer().querySelector('a[href="/automation/run"]')?.textContent).toBe('Automation');
  });

  it('shows a snippet the engine cannot send with the engine’s reason, as text, and does not send it', async () => {
    engine.snippets = [OLD, VERSION];
    open();

    const row = rowButton('Old');
    expect(row.disabled).toBe(true);
    expect(row.textContent).toContain(
      'Cannot be sent as it is — line 1, column 16: the default of `port` contains a control character (U+001B).',
    );
    // What it holds is shown as the text it is, escape character and all.
    expect(row.querySelector('code')?.textContent).toBe('show interface {{port=Gi0/1\u001b[201~}}');
    expect(button(/^Choose where to send Old/)).toBeUndefined();
    // It can still be corrected or thrown away.
    expect(button(/^Edit Old/)).toBeDefined();
    expect(button(/^Delete Old/)).toBeDefined();

    await press(row);
    expect(engine.calls).toEqual([]);
    expect(onPaste).not.toHaveBeenCalled();
  });

  it('sends a snippet without variables from its row: the engine’s text, held to its hash', async () => {
    open();
    await press(rowButton('Version'));

    expect(called('terminal.snippets.render').map((call) => call.params)).toEqual([{ id: VERSION.id, values: {} }]);
    // One line that only reads: no question, as when it is pasted.
    expect(engine.questions).toEqual([]);
    expect(called('terminal.snippets.send').map((call) => call.params)).toEqual([
      { id: VERSION.id, values: {}, session_ids: [SESSION.id], expect_sha256: 'aa11' },
    ]);
    expect(onSent).toHaveBeenCalledWith(SESSION.id);
    // Not typed from this window as well.
    expect(onPaste).not.toHaveBeenCalled();
  });

  it('says that a row goes to every session typing is broadcast to, and asks before it types into them', async () => {
    const second: SessionInfo = { ...SESSION, id: 'xro2pfhj', title: 'SW2-IOS', device_id: 'ios-device' };
    let answer: (yes: boolean) => void = () => undefined;
    engine.open = new Promise<void>((resolve) => {
      answer = (yes) => {
        engine.confirmed = yes;
        resolve();
      };
    });
    open({ sessions: [SESSION, second], broadcast: [SESSION.id, second.id] });

    // Not the session in front alone: the question will say two, so does the drawer.
    expect(drawer().textContent).toContain('Clicking one types it into 2 sessions.');
    expect(rowButton('Version').title).toBe('Type into 2 sessions');

    await press(rowButton('Version'));
    expect(engine.questions).toEqual([expect.objectContaining({ targets: 2, text: 'show version', source: 'snippet' })]);
    // Each device's own policy was asked; nothing is typed while the question is open.
    expect(called('terminal.classify').map((call) => (call.params as { device_id: string }).device_id)).toEqual([
      SESSION.device_id,
      second.device_id,
    ]);
    expect(called('terminal.snippets.send')).toEqual([]);

    await act(async () => {
      answer(true);
      for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
    });
    await settled();
    expect(called('terminal.snippets.send').map((call) => call.params)).toEqual([
      { id: VERSION.id, values: {}, session_ids: [SESSION.id, second.id], expect_sha256: 'aa11' },
    ]);
  });

  it('opens a form for a snippet with variables: defaults on screen, and nothing typed', async () => {
    open();
    await press(rowButton('Say'));

    expect(field('port').value).toBe('Gi0/1');
    expect(field('what').value).toBe('');
    expect(field('what').required).toBe(true);
    // The engine was asked with what the form holds; an empty required one is left out.
    expect(called('terminal.snippets.render').at(-1)?.params).toEqual({ id: SAY.id, values: { port: 'Gi0/1' } });
    expect(drawer().querySelector('[data-testid="snippet-nothing-yet"]')?.textContent).toContain('what needs a value');
    expect(button(/^Type into SW1-LAB/)?.disabled).toBe(true);
    expect(called('terminal.snippets.send')).toEqual([]);
  });

  it('puts the engine’s refusal beside the field it is about, and filters nothing itself', async () => {
    open();
    await press(rowButton('Say'));
    type(field('what'), 'up\tlink');
    await settled();

    // The value went to the engine as typed.
    expect(called('terminal.snippets.render').at(-1)?.params).toEqual({
      id: SAY.id,
      values: { what: 'up\tlink', port: 'Gi0/1' },
    });
    const what = field('what');
    expect(what.getAttribute('aria-invalid')).toBe('true');
    const described = document.getElementById(what.getAttribute('aria-describedby') ?? '');
    expect(described?.textContent).toBe('The value of `what` contains a control character (U+0009).');
    // The other field is not blamed.
    expect(field('port').getAttribute('aria-invalid')).not.toBe('true');
    expect(button(/^Type into SW1-LAB/)?.disabled).toBe(true);
  });

  it('keeps a line break that was pasted into a value, so the engine refuses it instead of the field joining it', async () => {
    open();
    await press(rowButton('Say'));
    type(field('what'), 'Gi0/1\nreload');
    await settled();

    // Not "Gi0/1 reload", which is what a single-line input would have made of it.
    expect(field('what').value).toBe('Gi0/1\nreload');
    expect(called('terminal.snippets.render').at(-1)?.params).toEqual({
      id: SAY.id,
      values: { what: 'Gi0/1\nreload', port: 'Gi0/1' },
    });
    const what = field('what');
    expect(what.getAttribute('aria-invalid')).toBe('true');
    expect(document.getElementById(what.getAttribute('aria-describedby') ?? '')?.textContent).toBe(
      'The value of `what` contains a line break.',
    );
    // Both lines of it are on screen in the field, and nothing can be sent.
    expect(what.getAttribute('rows')).toBe('2');
    expect(button(/^Type into SW1-LAB/)?.disabled).toBe(true);
    expect(await enter(what)).toBe(true);
    expect(engine.questions).toEqual([]);
    expect(called('terminal.snippets.send')).toEqual([]);
  });

  it('sends on Enter in a field only what is on screen, and never puts a line break into the value', async () => {
    open();
    await press(rowButton('Say'));
    type(field('what'), 'hello');
    // The engine has not answered for this value yet: there is nothing on screen to send.
    const early = new KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true });
    act(() => {
      field('what').dispatchEvent(early);
    });
    expect(early.defaultPrevented).toBe(true);
    expect(engine.questions).toEqual([]);
    expect(called('terminal.snippets.send')).toEqual([]);

    await settled();
    const what = field('what');
    expect(await enter(what)).toBe(true);
    // The key sent the form; it did not become part of the value.
    expect(what.value).toBe('hello');
    expect(engine.questions).toHaveLength(1);
    expect(called('terminal.snippets.send').map((call) => call.params)).toEqual([
      { id: SAY.id, values: { what: 'hello', port: 'Gi0/1' }, session_ids: [SESSION.id], expect_sha256: RENDERED.sha256 },
    ]);
  });

  it('types a snippet once however often Enter is pressed while it is on its way', async () => {
    engine.snippets = [PORT, SAY, VERSION];
    open();
    await press(rowButton('Port'));
    type(field('port'), 'Gi0/1');
    await settled();
    expect(drawer().querySelector('[aria-label="What will be typed"]')?.textContent).toContain('show interface Gi0/1');

    // A slow policy, as over a slow link: the first Enter is still on its
    // way when the second arrives. One read-only line into one session asks
    // nothing, so only the form can tell the second key from a second send.
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const answer = engine.answer;
    engine.answer = (method, params) =>
      method === 'terminal.classify' ? held.then(() => answer(method, params)) : answer(method, params);

    expect(await enter(field('port'))).toBe(true);
    expect(await enter(field('port'))).toBe(true);
    await act(async () => {
      release();
      for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
    });
    await settled();

    expect(called('terminal.snippets.send').map((call) => call.params)).toEqual([
      { id: PORT.id, values: { port: 'Gi0/1' }, session_ids: [SESSION.id], expect_sha256: 'cc33' },
    ]);
    expect(onSent).toHaveBeenCalledTimes(1);
  });

  it('asks once, not once per Enter, about a snippet that is worth a question', async () => {
    let answer: () => void = () => undefined;
    engine.open = new Promise<void>((resolve) => {
      answer = resolve;
    });
    open();
    await press(rowButton('Say'));
    type(field('what'), 'hello');
    await settled();

    expect(await enter(field('what'))).toBe(true);
    expect(engine.questions).toHaveLength(1);
    // The sheet is on screen; a second Enter in the form must not queue a second question behind it.
    expect(await enter(field('what'))).toBe(true);
    await act(async () => {
      answer();
      for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
    });
    await settled();

    expect(engine.questions).toHaveLength(1);
    expect(called('terminal.snippets.send')).toHaveLength(1);
  });

  it('shows the exact text, asks about it, and sends it held to the hash that was shown', async () => {
    open();
    await press(rowButton('Say'));
    type(field('what'), 'hello');
    await settled();

    const exact = drawer().querySelector('[aria-label="What will be typed"]');
    expect(exact?.textContent).toContain('say hello on Gi0/1');
    expect(exact?.textContent).toContain('say done');
    expect(called('terminal.snippets.send')).toEqual([]);

    await press(button(/^Type into SW1-LAB/));

    expect(engine.questions).toEqual([
      expect.objectContaining({ text: 'say hello on Gi0/1\nsay done', source: 'snippet', targets: 1, title: 'SW1-LAB' }),
    ]);
    expect(called('terminal.snippets.send').map((call) => call.params)).toEqual([
      { id: SAY.id, values: { what: 'hello', port: 'Gi0/1' }, session_ids: [SESSION.id], expect_sha256: RENDERED.sha256 },
    ]);
    expect(onSent).toHaveBeenCalledWith(SESSION.id);
  });

  it('sends nothing when the person says no on the sheet', async () => {
    engine.confirmed = false;
    open();
    await press(rowButton('Say'));
    type(field('what'), 'hello');
    await settled();
    await press(button(/^Type into SW1-LAB/));

    expect(engine.questions).toHaveLength(1);
    expect(called('terminal.snippets.send')).toEqual([]);
    expect(onSent).not.toHaveBeenCalled();
    // Still on the form, to change a value or leave.
    expect(drawer().querySelector('[data-testid="snippet-use"]')).not.toBeNull();
  });

  it('says so, and shows the text again, when the engine no longer renders what was shown', async () => {
    const render = engine.answer;
    engine.answer = (method, params) =>
      method === 'terminal.snippets.send'
        ? new RpcError(
            'conflict',
            '`Say` no longer renders to the text that was confirmed (the snippet or a value has changed); render it again and confirm what it shows now',
            { sha256: '0000' },
          )
        : render(method, params);
    open();
    await press(rowButton('Say'));
    type(field('what'), 'hello');
    await settled();
    const rendersBefore = called('terminal.snippets.render').length;
    await press(button(/^Type into SW1-LAB/));
    // The refusal arrives, and then the pause before the engine is asked again.
    await settled();

    const refused = drawer().querySelector('[data-testid="snippet-refused"]');
    expect(refused?.textContent).toContain('Nothing was sent.');
    expect(refused?.textContent).toContain('no longer renders to the text that was confirmed');
    // Asked again, so what is on screen is what it renders to now.
    expect(called('terminal.snippets.render').length).toBe(rendersBefore + 1);
    expect(onSent).not.toHaveBeenCalled();
  });

  it('still says the refusal when the snippet was edited while the question was open', async () => {
    const render = engine.answer;
    engine.answer = (method, params) =>
      method === 'terminal.snippets.send'
        ? new RpcError(
            'conflict',
            '`Say` no longer renders to the text that was confirmed (the snippet or a value has changed); render it again and confirm what it shows now',
            { sha256: '0000' },
          )
        : render(method, params);
    let answer: () => void = () => undefined;
    engine.open = new Promise<void>((resolve) => {
      answer = resolve;
    });
    open();
    await press(rowButton('Say'));
    type(field('what'), 'hello');
    await settled();
    await press(button(/^Type into SW1-LAB/));
    expect(engine.questions).toHaveLength(1);

    // Someone saves the snippet while the question is on screen: the list
    // comes back with the edit, and the form starts over for the new text.
    engine.snippets = [{ ...SAY, text: 'say {{what}} on {{port=Gi0/2}}\nsay done\n', updated_at: 3 }, VERSION];
    draw();
    await settled();
    expect(field('what').value).toBe('');

    // Yes to the text that was shown. The engine refuses it; the form that
    // asked is gone, and the refusal is said all the same.
    await act(async () => {
      answer();
      for (let turn = 0; turn < 10; turn += 1) await Promise.resolve();
    });
    await settled();
    expect(called('terminal.snippets.send')).toHaveLength(1);
    expect(onSent).not.toHaveBeenCalled();
    const said = useToasts.getState().toasts.map((toast) => `${toast.title} ${String(toast.description ?? '')}`);
    expect(said).toEqual([
      expect.stringMatching(/^“Say” was not sent .*no longer renders to the text that was confirmed/),
    ]);
  });

  it('names the session the engine refused, and that nothing went anywhere', async () => {
    const render = engine.answer;
    engine.answer = (method, params) =>
      method === 'terminal.snippets.send'
        ? new RpcError('closed', 'nothing was sent to any session: the session to SW1-LAB has ended (closed by the remote end); open a new one', {
            refused: [{ session_id: SESSION.id, code: 'closed', message: 'the session to SW1-LAB has ended (closed by the remote end); open a new one' }],
          })
        : render(method, params);
    open();
    await press(rowButton('Say'));
    type(field('what'), 'hello');
    await settled();
    await press(button(/^Type into SW1-LAB/));

    expect(drawer().querySelector('[data-testid="snippet-refused"]')?.textContent).toContain('Nothing was sent.');
    const target = drawer().querySelector('fieldset');
    expect(target?.textContent).toContain('the session to SW1-LAB has ended (closed by the remote end); open a new one');
  });
});
