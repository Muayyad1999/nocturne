import { describe, expect, it } from 'vitest';
import { decodeBase64, SessionStream, type StreamSink } from './stream';

const text = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
const bytes = (value: string) => new TextEncoder().encode(value);

function recorder() {
  let screen = '';
  let resyncs = 0;
  const sink: StreamSink = {
    write: (chunk) => {
      screen += text(chunk);
    },
    reset: () => {
      screen = '';
    },
    resync: () => {
      resyncs += 1;
    },
  };
  return {
    sink,
    get screen() {
      return screen;
    },
    /** How many times the stream asked for the scrollback again. */
    get resyncs() {
      return resyncs;
    },
  };
}

describe('decodeBase64', () => {
  it('gives back the bytes, not a string', () => {
    // A character split across two events only survives as bytes.
    const snowman = bytes('line ☃');
    const half = decodeBase64(btoa(String.fromCharCode(...snowman.slice(0, 7))));
    const rest = decodeBase64(btoa(String.fromCharCode(...snowman.slice(7))));
    expect(text(new Uint8Array([...half, ...rest]))).toBe('line ☃');
  });
});

describe('SessionStream', () => {
  it('writes the scrollback and then everything above it', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);

    stream.attach(bytes('SW1> show version\r\n'), 7);
    stream.receive(8, bytes('FakeOS 1.0\r\n'));
    stream.receive(9, bytes('SW1> '));

    expect(screen.screen).toBe('SW1> show version\r\nFakeOS 1.0\r\nSW1> ');
    expect(stream.seq).toBe(9);
  });

  it('holds events that arrive before the attach answer, and drops the ones it covers', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);

    // The subscription is live first, so these arrive while attach is in flight.
    stream.receive(7, bytes('covered by the scrollback\r\n'));
    stream.receive(8, bytes('after\r\n'));
    stream.receive(9, bytes('and after\r\n'));
    expect(screen.screen).toBe('');

    const outcome = stream.attach(bytes('everything up to 7\r\n'), 7);

    expect(outcome).toEqual({ rewritten: true, applied: 2 });
    expect(screen.screen).toBe('everything up to 7\r\nafter\r\nand after\r\n');
    expect(stream.seq).toBe(9);
  });

  it('ignores an event it has already applied', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(new Uint8Array(), 0);

    stream.receive(1, bytes('once'));
    stream.receive(1, bytes('once'));
    stream.receive(0, bytes('older still'));

    expect(screen.screen).toBe('once');
  });

  it('rewrites the screen when the stream was down and events were missed', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(bytes('first\r\n'), 1);
    stream.receive(2, bytes('second\r\n'));

    // The socket dropped; the device printed while nobody was listening.
    stream.attach(bytes('first\r\nsecond\r\nthird\r\n'), 3);

    expect(screen.screen).toBe('first\r\nsecond\r\nthird\r\n');
    expect(stream.seq).toBe(3);
  });

  it('does not rewrite when nothing was missed, so nothing appears twice', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(bytes('first\r\n'), 1);
    stream.receive(2, bytes('second\r\n'));

    const outcome = stream.attach(bytes('first\r\nsecond\r\n'), 2);

    expect(outcome.rewritten).toBe(false);
    expect(screen.screen).toBe('first\r\nsecond\r\n');
  });

  it('stops at a hole in the numbering and asks for the scrollback, rather than printing around it', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(new Uint8Array(), 0);
    for (let seq = 1; seq <= 5; seq += 1) stream.receive(seq, bytes(`line ${seq}\r\n`));

    // The server dropped 6, 7 and 8 for a client that had fallen behind.
    stream.receive(9, bytes('line 9\r\n'));
    stream.receive(10, bytes('line 10\r\n'));

    // Nothing after the hole is on screen as if it followed line 5.
    expect(screen.screen).toBe('line 1\r\nline 2\r\nline 3\r\nline 4\r\nline 5\r\n');
    expect(screen.resyncs).toBe(1);
    expect(stream.isAttached).toBe(false);

    // The backend's scrollback has what was missed; the events that were held
    // are already in it and are not written twice.
    const all = Array.from({ length: 10 }, (_, index) => `line ${index + 1}\r\n`).join('');
    const outcome = stream.attach(bytes(all), 10);

    expect(outcome).toEqual({ rewritten: true, applied: 0 });
    expect(screen.screen).toBe(all);
    expect(stream.seq).toBe(10);

    stream.receive(11, bytes('line 11\r\n'));
    expect(screen.screen).toBe(`${all}line 11\r\n`);
    expect(screen.resyncs).toBe(1);
  });

  it('asks again when the scrollback it was given stops short of the events it is holding', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(bytes('one\r\n'), 1);

    stream.receive(5, bytes('five\r\n'));
    expect(screen.resyncs).toBe(1);

    // An answer that was already on its way when the hole appeared: it covers
    // less than what has been seen since, so the hole is still there.
    stream.attach(bytes('one\r\ntwo\r\n'), 2);

    expect(screen.screen).toBe('one\r\ntwo\r\n');
    expect(screen.resyncs).toBe(2);
    expect(stream.isAttached).toBe(false);

    stream.attach(bytes('one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n'), 5);
    expect(screen.screen).toBe('one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n');
    expect(stream.isAttached).toBe(true);
  });

  it('writes the scrollback once when two answers arrive for the same hole', () => {
    // The server's notice that it dropped events and the hole itself can each
    // ask for the scrollback; both answers come back.
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(bytes('one\r\n'), 1);
    stream.receive(4, bytes('four\r\n'));

    const first = stream.attach(bytes('one\r\ntwo\r\nthree\r\nfour\r\n'), 4);
    const second = stream.attach(bytes('one\r\ntwo\r\nthree\r\nfour\r\n'), 4);

    expect(first.rewritten).toBe(true);
    expect(second.rewritten).toBe(false);
    expect(screen.screen).toBe('one\r\ntwo\r\nthree\r\nfour\r\n');
    expect(screen.resyncs).toBe(1);
  });

  it('finds what a quiet session lost when it is asked to attach again', () => {
    // Nothing arrived after the dropped events, so no hole ever showed: only
    // the backend's count, which has run ahead of the screen, gives it away.
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(bytes('one\r\n'), 1);
    stream.receive(2, bytes('two\r\n'));

    const outcome = stream.attach(bytes('one\r\ntwo\r\nthree\r\nfour\r\n'), 4);

    expect(outcome.rewritten).toBe(true);
    expect(screen.screen).toBe('one\r\ntwo\r\nthree\r\nfour\r\n');
    expect(screen.resyncs).toBe(0);
  });

  it('does not keep every event for ever while an attach is not answered', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);

    // Far more than can be worth holding: the scrollback of the attach that
    // finally answers covers the old ones anyway.
    for (let seq = 1; seq <= 5000; seq += 1) stream.receive(seq, bytes('x'));
    expect(stream.heldEvents).toBeLessThanOrEqual(1024);

    // What was let go is noticed as a hole, not printed around.
    stream.attach(bytes('start\r\n'), 0);
    expect(screen.screen).toBe('start\r\n');
    expect(screen.resyncs).toBe(1);
  });

  it('starts over after detach', () => {
    const screen = recorder();
    const stream = new SessionStream(screen.sink);
    stream.attach(bytes('old\r\n'), 4);
    stream.detach();
    expect(stream.isAttached).toBe(false);

    stream.receive(5, bytes('while away\r\n'));
    stream.attach(bytes('old\r\nwhile away\r\n'), 5);

    expect(screen.screen).toBe('old\r\nwhile away\r\n');
  });
});
