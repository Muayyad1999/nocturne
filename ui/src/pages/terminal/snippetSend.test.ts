/**
 * The order of events when a snippet is sent: what is asked, what is typed,
 * and — the part that matters — that nothing is typed before the answer.
 *
 * The engine is stood in for by four functions; the real one is driven by
 * `ui/tests/terminal.spec.ts`, where a console records what it was sent.
 */

import { describe, expect, it } from 'vitest';
import { RpcError } from '@/api/errors';
import type { SnippetSendParams, SnippetSendResult, TerminalLineClass } from '@/api/types/terminal';
import type { PasteQuestion } from './pasteFunnel';
import { sendSnippet, type SnippetSendRequest } from './snippetSend';
import type { SendTarget } from './snippets';

const reads = (line: string): TerminalLineClass => ({ line, class: 'read_only', reason: 'reads' });
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

const SW1: SendTarget = { sessionId: 's1', title: 'SW1', deviceId: 'dev-1' };
const SW2: SendTarget = { sessionId: 's2', title: 'SW2', deviceId: 'dev-2' };
const CONSOLE: SendTarget = { sessionId: 's3', title: 'COM3', deviceId: null };

function world(options: {
  classify?: (deviceId: string | null, text: string) => Promise<TerminalLineClass[]>;
  send?: (params: SnippetSendParams) => Promise<SnippetSendResult>;
} = {}) {
  const asked: PasteQuestion[] = [];
  const answers: ((confirmed: boolean) => void)[] = [];
  const sent: SnippetSendParams[] = [];
  const classified: (string | null)[] = [];
  return {
    asked,
    sent,
    classified,
    answer: (confirmed: boolean) => {
      const next = answers.shift();
      if (!next) throw new Error('no question is open');
      next(confirmed);
    },
    deps: {
      classify: async (deviceId: string | null, text: string) => {
        classified.push(deviceId);
        return options.classify ? options.classify(deviceId, text) : text.split('\n').map(reads);
      },
      confirm: (question: PasteQuestion) => {
        asked.push(question);
        return new Promise<boolean>((resolve) => answers.push(resolve));
      },
      send: async (params: SnippetSendParams) => {
        sent.push(params);
        if (options.send) return options.send(params);
        return { sent: params.session_ids.map((session_id) => ({ session_id, lines: 1 })), failed: [] };
      },
    },
  };
}

const request = (over: Partial<SnippetSendRequest> = {}): SnippetSendRequest => ({
  id: 'snip-1',
  values: { port: 'Gi0/2' },
  shown: { text: 'show interface Gi0/2', sha256: 'aa11' },
  targets: [SW1],
  ...over,
});

describe('sendSnippet', () => {
  it('types one read-only line into one session without a question, held to the hash that was shown', async () => {
    const { deps, asked, sent, classified } = world();

    const outcome = await sendSnippet(deps, request());

    expect(asked).toEqual([]);
    expect(classified).toEqual(['dev-1']);
    expect(sent).toEqual([
      { id: 'snip-1', values: { port: 'Gi0/2' }, session_ids: ['s1'], expect_sha256: 'aa11' },
    ]);
    expect(outcome).toEqual({ kind: 'sent', result: { sent: [{ session_id: 's1', lines: 1 }], failed: [] } });
  });

  it('sends nothing while the question about several lines is unanswered, and once after a yes', async () => {
    const { deps, asked, sent, answer } = world();

    const sending = sendSnippet(deps, request({ shown: { text: 'interface Gi0/2\n shutdown', sha256: 'bb22' } }));
    await settle();

    expect(asked).toEqual([
      {
        sessionId: 's1',
        title: 'SW1',
        text: 'interface Gi0/2\n shutdown',
        classes: [reads('interface Gi0/2'), reads(' shutdown')],
        source: 'snippet',
        targets: 1,
      },
    ]);
    expect(sent).toEqual([]);

    answer(true);
    await expect(sending).resolves.toMatchObject({ kind: 'sent' });
    expect(sent).toHaveLength(1);
    // What is sent is what was asked about: the same render, by its hash.
    expect(sent[0]?.expect_sha256).toBe('bb22');
  });

  it('sends nothing at all when the person says no', async () => {
    const { deps, sent, answer } = world();

    const sending = sendSnippet(deps, request({ shown: { text: 'a\nb', sha256: 'cc33' } }));
    await settle();
    answer(false);

    await expect(sending).resolves.toEqual({ kind: 'cancelled' });
    expect(sent).toEqual([]);
  });

  it('asks about a single line that any of the devices would not just read', async () => {
    const { deps, asked, answer } = world({
      classify: async (deviceId, text) =>
        deviceId === 'dev-2'
          ? [{ line: text, class: 'dangerous', reason: 'restarts the device' }]
          : [reads(text)],
    });

    const sending = sendSnippet(deps, request({ shown: { text: 'reload', sha256: 'dd44' }, targets: [SW1, SW2] }));
    await settle();

    expect(asked).toHaveLength(1);
    expect(asked[0]?.classes).toEqual([{ line: 'reload', class: 'dangerous', reason: 'restarts the device' }]);
    answer(false);
    await sending;
  });

  it('always asks when the text goes to more than one session, and says how many', async () => {
    const { deps, asked, sent, classified, answer } = world();

    const sending = sendSnippet(deps, request({ targets: [SW1, CONSOLE, { ...SW1, sessionId: 's4', title: 'SW1 (2)' }] }));
    await settle();

    expect(asked).toHaveLength(1);
    expect(asked[0]).toMatchObject({ targets: 3, sessionId: 's1', title: 'SW1' });
    // One question to the policy per device, not per session.
    expect(classified).toEqual(['dev-1', null]);
    expect(sent).toEqual([]);

    answer(true);
    await sending;
    // In the order they were ticked: the engine types into them in this order.
    expect(sent[0]?.session_ids).toEqual(['s1', 's3', 's4']);
  });

  it('does not let a policy that cannot be asked wave several lines through unasked', async () => {
    const { deps, asked, sent, answer } = world({
      classify: async () => {
        throw new Error('the device is not in the inventory any more');
      },
    });

    const sending = sendSnippet(deps, request({ shown: { text: 'a\nb', sha256: 'ee55' } }));
    await settle();

    // Asked, with no verdict to show: the sheet says the policy could not be asked.
    expect(asked[0]?.classes).toBeNull();
    expect(sent).toEqual([]);
    answer(true);
    await expect(sending).resolves.toMatchObject({ kind: 'sent' });
  });

  it('has nothing to send for an empty text or no session', async () => {
    const { deps, asked, sent, classified } = world();

    await expect(sendSnippet(deps, request({ shown: { text: '', sha256: 'e3b0' } }))).resolves.toEqual({ kind: 'nothing' });
    await expect(sendSnippet(deps, request({ targets: [] }))).resolves.toEqual({ kind: 'nothing' });

    expect(classified).toEqual([]);
    expect(asked).toEqual([]);
    expect(sent).toEqual([]);
  });

  it('passes the engine’s refusal on as it is: nothing was typed, and nothing is tried again', async () => {
    const changed = new RpcError(
      'conflict',
      '`Say` no longer renders to the text that was confirmed (the snippet or a value has changed); render it again and confirm what it shows now',
      { sha256: 'ff66' },
    );
    // An engine that would take the hash it names: sending again with it
    // would type a text the person never saw.
    const { deps, sent } = world({
      send: async (params) => {
        if (params.expect_sha256 === 'ff66') return { sent: [{ session_id: 's1', lines: 1 }], failed: [] };
        throw changed;
      },
    });

    await expect(sendSnippet(deps, request())).rejects.toBe(changed);
    expect(sent.map((params) => params.expect_sha256)).toEqual(['aa11']);
  });

  it('hands back who took it and who did not', async () => {
    const result: SnippetSendResult = {
      sent: [{ session_id: 's1', lines: 1 }],
      failed: [{ session_id: 's2', code: 'closed', message: 'the session to DEAD has ended (connection closed); open a new one' }],
    };
    const { deps, answer } = world({ send: async () => result });

    const sending = sendSnippet(deps, request({ targets: [SW1, SW2] }));
    await settle();
    answer(true);

    await expect(sending).resolves.toEqual({ kind: 'sent', result });
  });
});
