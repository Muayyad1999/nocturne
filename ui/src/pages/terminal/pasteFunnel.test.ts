import { describe, expect, it } from 'vitest';
import type { TerminalLineClass } from '@/api/types/terminal';
import { PasteFunnel, type PasteQuestion, type PasteTarget } from './pasteFunnel';

const readOnly = (line: string): TerminalLineClass => ({ line, class: 'read_only', reason: 'read-only command' });

/** A promise the test settles by hand, to stand where an answer is awaited. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

/** Lets everything that is already runnable run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

interface World {
  funnel: PasteFunnel;
  /** Exactly what was handed to `terminal.input`, call by call. */
  sent: [string, string][];
  /** The questions put to the person, in order. */
  asked: PasteQuestion[];
  /** What the policy was asked about. */
  classified: [string | null, string][];
  /** Answers the question that is open. */
  answer: (confirmed: boolean) => void;
  target: PasteTarget;
  /** The session goes away: closed, or its tab forgotten. */
  close: () => void;
}

function world(options: {
  classify?: (deviceId: string | null, text: string) => Promise<TerminalLineClass[]>;
  target?: Partial<PasteTarget>;
} = {}): World {
  const sent: [string, string][] = [];
  const asked: PasteQuestion[] = [];
  const classified: [string | null, string][] = [];
  const answers: ((confirmed: boolean) => void)[] = [];
  const target: PasteTarget = {
    title: 'SW1',
    deviceId: 'dev-1',
    targets: 1,
    bracketedPaste: false,
    ...options.target,
  };
  let open = true;

  const funnel = new PasteFunnel({
    target: async () => (open ? target : null),
    classify: async (deviceId, text) => {
      classified.push([deviceId, text]);
      if (options.classify) return options.classify(deviceId, text);
      return text.split('\n').map(readOnly);
    },
    ask: (question) => {
      asked.push(question);
      return new Promise<boolean>((resolve) => answers.push(resolve));
    },
    send: (sessionId, data) => sent.push([sessionId, data]),
  });

  return {
    funnel,
    sent,
    asked,
    classified,
    target,
    answer: (confirmed) => {
      const next = answers.shift();
      if (!next) throw new Error('no question is open');
      next(confirmed);
    },
    close: () => {
      open = false;
    },
  };
}

describe('PasteFunnel', () => {
  it('sends nothing while the question is unanswered, and once when it is confirmed', async () => {
    const { funnel, sent, asked, answer } = world();

    const outcome = funnel.offer('s1', 'conf t\nint gi0/1\n');
    await settle();

    expect(asked).toHaveLength(1);
    expect(asked[0]).toMatchObject({ sessionId: 's1', title: 'SW1', text: 'conf t\nint gi0/1\n', targets: 1 });
    expect(sent).toEqual([]);

    answer(true);
    await expect(outcome).resolves.toBe('sent');
    expect(sent).toEqual([['s1', 'conf t\rint gi0/1\r']]);
  });

  it('sends nothing at all when the person backs out', async () => {
    const { funnel, sent, answer } = world();

    const outcome = funnel.offer('s1', 'conf t\nint gi0/1\n');
    await settle();
    answer(false);

    await expect(outcome).resolves.toBe('cancelled');
    expect(sent).toEqual([]);
  });

  it('lets one read-only line straight through, without a question', async () => {
    const { funnel, sent, asked } = world();

    await expect(funnel.offer('s1', 'show version')).resolves.toBe('sent');

    expect(asked).toEqual([]);
    expect(sent).toEqual([['s1', 'show version']]);
  });

  it('hands the device carriage returns whatever the clipboard used for line endings', async () => {
    const { funnel, sent, classified, answer } = world();

    const windows = funnel.offer('s1', 'say A\r\nsay B\r\n');
    await settle();
    answer(true);
    await windows;

    // One Enter per line: the line feed of a CRLF would be a second one.
    expect(sent).toEqual([['s1', 'say A\rsay B\r']]);
    // And the policy saw the same lines the person is shown.
    expect(classified).toEqual([['dev-1', 'say A\nsay B\n']]);
  });

  it('asks about every line of text whose lines end in a bare carriage return', async () => {
    // The backend splits on line feeds only: without this the two commands
    // would be judged as one line, and that line is not `reload`.
    const { funnel, classified, asked, answer } = world();

    const outcome = funnel.offer('s1', 'show clock\rreload');
    await settle();

    expect(classified).toEqual([['dev-1', 'show clock\nreload']]);
    expect(asked).toHaveLength(1);
    answer(false);
    await outcome;
  });

  it('still asks about several lines when the policy cannot be asked, and says so', async () => {
    const { funnel, asked, sent, answer } = world({
      classify: async () => {
        throw new Error('no profile is loaded');
      },
    });

    const outcome = funnel.offer('s1', 'conf t\nreload\n');
    await settle();
    expect(asked[0]?.classes).toBeNull();
    expect(sent).toEqual([]);
    answer(true);
    await outcome;
    expect(sent).toEqual([['s1', 'conf t\rreload\r']]);
  });

  it('always asks before text goes to several sessions at once', async () => {
    const { funnel, asked, sent, answer } = world({ target: { targets: 3 } });

    const outcome = funnel.offer('s1', 'show version');
    await settle();

    expect(asked[0]).toMatchObject({ targets: 3 });
    expect(sent).toEqual([]);
    answer(true);
    await outcome;
    expect(sent).toEqual([['s1', 'show version']]);
  });

  it('never presses Enter for a snippet', async () => {
    const { funnel, sent } = world();

    await funnel.offer('s1', 'show version\r\n', 'snippet');

    expect(sent).toEqual([['s1', 'show version']]);
  });

  it('brackets the paste when the program at the far end asked for that', async () => {
    const { funnel, sent, answer } = world({ target: { bracketedPaste: true } });

    const outcome = funnel.offer('s1', 'ls\npwd\n');
    await settle();
    answer(true);
    await outcome;

    expect(sent).toEqual([['s1', '\u001b[200~ls\rpwd\r\u001b[201~']]);
  });

  it('does not bracket what goes to several sessions, which have not all asked', async () => {
    // To a device that never asked, the markers are six characters typed at
    // its prompt.
    const { funnel, sent, answer } = world({ target: { bracketedPaste: true, targets: 2 } });

    const outcome = funnel.offer('s1', 'show version');
    await settle();
    answer(true);
    await outcome;

    expect(sent).toEqual([['s1', 'show version']]);
  });

  it('asks about one paste at a time, in the order they were made', async () => {
    const { funnel, asked, sent, answer } = world();

    const first = funnel.offer('s1', 'a\nb');
    const second = funnel.offer('s1', 'c\nd');
    await settle();

    // The second waits behind the first rather than replacing its question.
    expect(asked.map((question) => question.text)).toEqual(['a\nb']);
    answer(true);
    await first;
    await settle();
    expect(asked.map((question) => question.text)).toEqual(['a\nb', 'c\nd']);
    answer(true);
    await second;

    expect(sent).toEqual([
      ['s1', 'a\rb'],
      ['s1', 'c\rd'],
    ]);
  });

  it('keeps what is typed on the heels of a paste behind it', async () => {
    const answerPolicy = deferred<TerminalLineClass[]>();
    const { funnel, sent } = world({ classify: () => answerPolicy.promise });

    // Paste, then Enter, faster than the policy answers.
    const outcome = funnel.offer('s1', 'show version');
    await settle();
    expect(funnel.typed('s1', '\r')).toBe(true);
    expect(sent).toEqual([]);

    answerPolicy.resolve([readOnly('show version')]);
    await outcome;

    expect(sent).toEqual([
      ['s1', 'show version'],
      ['s1', '\r'],
    ]);
    // Afterwards typing is nobody's business but the session's.
    expect(funnel.typed('s1', 'x')).toBe(false);
  });

  it('throws away with a refused paste what was typed blind behind it', async () => {
    const answerPolicy = deferred<TerminalLineClass[]>();
    const { funnel, sent, answer } = world({ classify: () => answerPolicy.promise });

    const outcome = funnel.offer('s1', 'reload\n');
    expect(funnel.typed('s1', '\r')).toBe(true);
    answerPolicy.resolve([
      { line: 'reload', class: 'dangerous', reason: '`reload` is disruptive' },
      readOnly(''),
    ]);
    await settle();
    answer(false);

    await expect(outcome).resolves.toBe('cancelled');
    // Not even the Enter: it was meant to follow a paste that never happened.
    expect(sent).toEqual([]);
  });

  it('holds only the session being pasted into', async () => {
    const answerPolicy = deferred<TerminalLineClass[]>();
    const { funnel } = world({ classify: () => answerPolicy.promise });

    const outcome = funnel.offer('s1', 'show version');
    expect(funnel.typed('s2', 'x')).toBe(false);
    answerPolicy.resolve([readOnly('show version')]);
    await outcome;
  });

  it('lets the keyboard go once the question is on screen', async () => {
    // From then on the keyboard is the question's. Anything that still
    // arrives for the session is the terminal answering the device, and must
    // not wait for a person.
    const { funnel, answer } = world();

    const outcome = funnel.offer('s1', 'conf t\nreload\n');
    await settle();
    expect(funnel.typed('s1', '\u001b[?1;2c')).toBe(false);
    answer(false);
    await outcome;
  });

  it('does not hold the terminal\'s replies behind a paste that is waiting its turn', async () => {
    const { funnel, answer } = world();

    const first = funnel.offer('s1', 'conf t\nreload\n');
    const second = funnel.offer('s1', 'show version');
    await settle();

    // The first question is open; the second paste has not been looked at.
    expect(funnel.typed('s1', '\u001b[?1;2c')).toBe(false);
    answer(false);
    await first;
    await second;
  });

  it('sends nothing to a session that went away while the question was open', async () => {
    const { funnel, sent, answer, close } = world();

    const outcome = funnel.offer('s1', 'conf t\nreload\n');
    await settle();
    close();
    answer(true);

    await expect(outcome).resolves.toBe('gone');
    expect(sent).toEqual([]);
  });

  it('has nothing to send for text that is empty', async () => {
    const { funnel, sent, asked, classified } = world();

    await expect(funnel.offer('s1', '')).resolves.toBe('empty');
    await expect(funnel.offer('s1', '\n', 'snippet')).resolves.toBe('empty');

    expect(classified).toEqual([]);
    expect(asked).toEqual([]);
    expect(sent).toEqual([]);
  });

  it('carries on with the next paste after one that failed', async () => {
    const { funnel, sent } = world();
    const broken = new PasteFunnel({
      target: async () => {
        throw new Error('boom');
      },
      classify: async () => [],
      ask: async () => true,
      send: () => undefined,
    });

    await expect(broken.offer('s1', 'show version')).rejects.toThrow('boom');
    // The same funnel is still usable: one failure must not jam every later paste.
    await expect(broken.offer('s1', 'show version')).rejects.toThrow('boom');
    expect(broken.typed('s1', 'x')).toBe(false);

    await funnel.offer('s1', 'show version');
    expect(sent).toHaveLength(1);
  });
});
