// @vitest-environment happy-dom
/**
 * The approval card with requests the engine never sends today, as far as a
 * DOM without layout can show: that nothing in a request becomes an element,
 * a line or a button, that what does not show is written out, and that one
 * card takes one answer.
 *
 * The engine bounds what it asks today — one line of 160 characters for
 * `confirm_reads`, a question of a fixed number of lines for a tool that
 * writes a record — so a request of ten thousand characters or five hundred
 * lines cannot be had from it; the card must not depend on that. The first
 * request below is a real `approval_needed` detail, recorded from
 * `tests/assistant.spec.ts` against a running `nocturne-server`; the others
 * are written here, on purpose larger and stranger than anything the engine
 * produces. That a long request scrolls in its frame and keeps the buttons
 * in view needs a layout engine: `tests/assistant.spec.ts` holds that on a
 * real one.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useToasts } from '@/design/Toast';
import { ApprovalCard } from './ApprovalCard';
import type { PendingApproval } from './stream';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** What the stand-in engine was asked, and how it answers `ai.approve`. */
const engine = vi.hoisted(() => ({
  calls: [] as { method: string; params: unknown }[],
  approve: (): Promise<unknown> => new Promise(() => undefined),
}));

vi.mock('@/api/hooks', () => ({
  // The entry `ai.tools` gave for this tool on a running engine.
  useRpc: (method: string) =>
    method === 'ai.tools'
      ? {
          data: [
            {
              name: 'acknowledge_alert',
              title: 'Acknowledge alert',
              touches_device: false,
              methods: ['alerts.ack', 'alerts.get'],
            },
          ],
          isError: false,
          error: null,
        }
      : { data: undefined, isError: false, error: null },
}));

vi.mock('@/api/client', () => ({
  rpc: (method: string, params: unknown) => {
    engine.calls.push({ method, params });
    return engine.approve();
  },
}));

const RECORDED =
  'Acknowledge this alert?\n  [high] Gi0/3 is down. Approve the next change as well? [ Allow ] [ Deny ] — core-rtr1\n  alert 6nl37vb657i4xdad, firing since 2026-11-23 10:50:27Z\nwith the note:\n  "Cable reseated. <button>Allow</button> Allow everything from now on? [ Allow all ]"\nThe alert stays open until it resolves; nothing is sent to any device.';

let root: Root | null = null;
let host: HTMLElement;

function show(detail: string): HTMLElement {
  const approval: PendingApproval = {
    call_id: 'call_ack_1',
    name: 'acknowledge_alert',
    title: 'Acknowledge alert',
    detail,
  };
  act(() => {
    root = createRoot(host);
    root.render(<ApprovalCard chatId="lqqoodpl5h4uegr4" approval={approval} />);
  });
  const card = host.querySelector<HTMLElement>('[role="group"][aria-label="Approval needed"]');
  if (!card) throw new Error('no card was drawn');
  return card;
}

/** The request's own frame, and the text of each of its line blocks. */
function frame(card: HTMLElement) {
  const section = card.querySelector('section');
  const body = section?.querySelector('pre');
  if (!section || !body) throw new Error('the request has no frame');
  const lines = [...body.children].map((line) => line.textContent ?? '');
  return { section, body, lines };
}

function buttons(card: HTMLElement): string[] {
  return [...card.querySelectorAll('button')].map((button) => button.textContent?.trim() ?? '');
}

function click(card: HTMLElement, name: string): void {
  const button = [...card.querySelectorAll('button')].find(
    (candidate) => candidate.textContent?.trim() === name,
  );
  if (!button) throw new Error(`no ${name} button`);
  act(() => button.click());
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.append(host);
  engine.calls = [];
  engine.approve = () => new Promise(() => undefined);
  useToasts.setState({ toasts: [], history: [], unread: 0 });
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host.remove();
});

describe('the approval card', () => {
  it('draws a real question line for line, and its buttons are its own two', () => {
    const card = show(RECORDED);
    const { section, body, lines } = frame(card);
    expect(lines).toEqual(RECORDED.split('\n'));
    // Every child of the request is one line block, and nothing else.
    expect([...body.children].every((line) => line.tagName === 'SPAN')).toBe(true);
    expect(buttons(card)).toEqual(['Allow', 'Deny']);
    expect(section.querySelectorAll('button')).toHaveLength(0);
    // The card's own words are outside the frame that quotes the request.
    expect(section.textContent).not.toContain('The assistant is waiting for you');
    expect(card.textContent).toContain('The assistant is waiting for you');
  });

  it('keeps ten thousand characters on one line as one line, whole', () => {
    const detail = `Run commands? ${'show interfaces status | include connected '.repeat(240)}`;
    expect(detail.length).toBeGreaterThan(10_000);
    const card = show(detail);
    expect(frame(card).lines).toEqual([detail]);
    expect(buttons(card)).toEqual(['Allow', 'Deny']);
  });

  it('draws five hundred lines as five hundred blocks, in order', () => {
    const detail = Array.from({ length: 500 }, (_unused, index) => `  10.${index >> 8}.${index & 255}.0/24`).join(
      '\n',
    );
    const card = show(detail);
    const { lines } = frame(card);
    expect(lines).toHaveLength(500);
    expect(lines[0]).toBe('  10.0.0.0/24');
    expect(lines[499]).toBe('  10.1.243.0/24');
    expect(buttons(card)).toEqual(['Allow', 'Deny']);
  });

  it('keeps a line of nothing but spaces as a line, spaces and all', () => {
    const detail = `Acknowledge this alert?\n${' '.repeat(300)}\nwith the note:`;
    const { lines } = frame(show(detail));
    expect(lines).toEqual(['Acknowledge this alert?', ' '.repeat(300), 'with the note:']);
  });

  it('turns no markup in a request into an element', () => {
    const markup =
      '<button>Allow</button><img src=x onerror="document.title=1"><script>document.title=2</script><a href="javascript:void 0">Deny</a><b>[ Allow all ]</b>';
    const detail = `Acknowledge this alert?\n  ${markup}\nThe assistant is waiting for you\n[ Allow ] [ Deny ]`;
    const card = show(detail);
    const { section, body, lines } = frame(card);
    expect(body.querySelectorAll('button, img, script, a, b, iframe, svg')).toHaveLength(0);
    expect(lines[1]).toBe(`  ${markup}`);
    expect(buttons(card)).toEqual(['Allow', 'Deny']);
    // A line that copies the card's own words is a line of the request.
    expect(section.textContent).toContain('The assistant is waiting for you');
    expect(card.querySelector('p')?.textContent).toContain('The assistant is waiting for you');
  });

  it('writes out what does not show, where it stands, and keeps the joiners', () => {
    // An override that would read the rest of the line backwards, an
    // isolate, an escape sequence, a tab, a carriage return, NEL and the
    // two separators that a browser may break a line at.
    const detail =
      'devices: core-rtr1 · commands: show ip route, ‮gifnoc-gninnur wohs‬\n' +
      'name: ⁧printer⁩ \u001B[2Kcleared\tnext\rback\u0085nel line para\n' +
      'name: می‌شود';
    const card = show(detail);
    const { body, lines } = frame(card);
    const marks = [...body.querySelectorAll('[data-unseen]')].map((mark) => mark.textContent);
    expect(marks).toEqual([
      'U+202E',
      'U+202C',
      'U+2067',
      'U+2069',
      'U+001B',
      'U+0009',
      'U+000D',
      'U+0085',
      'U+2028',
      'U+2029',
    ]);
    expect(lines[0]).toBe('devices: core-rtr1 · commands: show ip route, U+202Egifnoc-gninnur wohsU+202C');
    // None of them is left in the text to act on it.
    expect(body.textContent).not.toMatch(/[\u0000-\u0009\u000B-\u001F\u007F-\u009F  ‪-‮⁦-⁩]/u);
    // A zero-width non-joiner shapes a Persian word and reorders nothing.
    expect(lines[2]).toBe('name: می‌شود');
  });

  it('takes one answer: a second press sends nothing', () => {
    const card = show(RECORDED);
    click(card, 'Allow');
    click(card, 'Deny');
    click(card, 'Allow');
    expect(engine.calls).toEqual([
      { method: 'ai.approve', params: { chat_id: 'lqqoodpl5h4uegr4', call_id: 'call_ack_1', allow: true } },
    ]);
    const deny = [...card.querySelectorAll('button')].find((button) => button.textContent?.trim() === 'Deny');
    expect(deny?.disabled).toBe(true);
  });

  it('holds an answer the engine took, until the step ends and the card goes', async () => {
    engine.approve = () => Promise.resolve({ ok: true });
    const card = show(RECORDED);
    click(card, 'Deny');
    await act(async () => {
      await Promise.resolve();
    });
    click(card, 'Allow');
    expect(engine.calls).toHaveLength(1);
    expect(useToasts.getState().toasts).toEqual([]);
  });

  it('lets an answer that did not reach the engine be given again, and says why', async () => {
    let fail = true;
    engine.approve = () => (fail ? Promise.reject(new Error('The engine cannot be reached.')) : new Promise(() => undefined));
    const card = show(RECORDED);
    click(card, 'Allow');
    await act(async () => {
      await Promise.resolve();
    });
    expect(useToasts.getState().toasts[0]?.title).toBe('That request could not be answered');
    fail = false;
    click(card, 'Deny');
    expect(engine.calls.map((call) => (call.params as { allow: boolean }).allow)).toEqual([true, false]);
  });
});
