import { describe, expect, it } from 'vitest';
import { decodeBase64, SessionStream, type StreamSink } from './stream';

const text = (bytes: Uint8Array) => new TextDecoder().decode(bytes);
const bytes = (value: string) => new TextEncoder().encode(value);

function recorder() {
  let screen = '';
  const sink: StreamSink = {
    write: (chunk) => {
      screen += text(chunk);
    },
    reset: () => {
      screen = '';
    },
  };
  return {
    sink,
    get screen() {
      return screen;
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
