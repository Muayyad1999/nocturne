/**
 * The attach protocol, on its own so it can be reasoned about and tested.
 *
 * A session's byte stream reaches the UI as `terminal.output` events numbered
 * from 1 without gaps. `terminal.attach` hands over the scrollback the backend
 * kept and the number of the last event that scrollback includes; everything
 * above it is applied on top. The window may come and go, the event stream may
 * drop and come back, and the result on screen must be the same bytes the
 * device sent, once each, in order.
 *
 * The rules this implements:
 *
 *   * events that arrive before the attach answer are held, not dropped;
 *   * an event the scrollback already covers (`seq <= attached`) is ignored;
 *   * an event already applied is ignored (the same event twice, or an attach
 *     that lands behind what is already on screen);
 *   * if the backend has moved further than the last event we applied — the
 *     event stream was down — the screen is cleared and the scrollback is
 *     written again, because that is the only way to be sure of no gap;
 *   * an event that is not the next one is a hole: the server dropped events
 *     for a client that fell behind, on a socket that stayed open. Nothing
 *     after a hole is written. The stream stops, asks for the scrollback
 *     (`resync`) and holds what arrives until it has it.
 */

/** Decodes the base64 of an output event into the raw bytes it carries. */
export function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes;
}

export interface StreamSink {
  /** Write bytes to the screen, exactly as they came off the wire. */
  write(bytes: Uint8Array): void;
  /** Throw away what is on screen: a reattach is about to rewrite it. */
  reset(): void;
  /**
   * Events went missing: call `terminal.attach` again and give the answer to
   * `attach`. Asked once per hole; until the answer comes, events are held.
   */
  resync(): void;
}

/**
 * The most events held while an attach is unanswered. An event is at most
 * 32 KiB, and without a bound a device that keeps printing at a session whose
 * attach never answers would fill memory. Letting the oldest go is safe: an
 * attach that answers later covers them with its scrollback, and one that
 * answered earlier leaves a hole, which is noticed.
 */
const HELD_MAX = 1024;

/** What an attach did, for the caller's log and the tests. */
export interface AttachOutcome {
  /** The screen was cleared and the scrollback written again. */
  rewritten: boolean;
  /** Buffered events applied on top of the scrollback. */
  applied: number;
}

/**
 * Keeps one session's screen in step with the backend's numbering.
 *
 * Create it *before* calling `terminal.attach` and feed it every
 * `terminal.output` event for the session from that moment on.
 */
export class SessionStream {
  /** The last event number applied to the screen. */
  private applied = 0;
  /** Events that arrived before the first attach answer. */
  private held: { seq: number; bytes: Uint8Array }[] = [];
  /** False until an attach has established where we are. */
  private attached = false;

  constructor(private readonly sink: StreamSink) {}

  /** The number of the last event on screen. */
  get seq(): number {
    return this.applied;
  }

  get isAttached(): boolean {
    return this.attached;
  }

  /** How many events are waiting for an attach answer. */
  get heldEvents(): number {
    return this.held.length;
  }

  /** An output event for this session. */
  receive(seq: number, bytes: Uint8Array): void {
    if (!this.attached) {
      this.hold(seq, bytes);
      return;
    }
    if (seq <= this.applied) return;
    if (seq !== this.applied + 1) {
      // Writing this would put it straight after the last event applied, as
      // if the device had never printed what lies between.
      this.attached = false;
      this.hold(seq, bytes);
      this.sink.resync();
      return;
    }
    this.applied = seq;
    this.sink.write(bytes);
  }

  private hold(seq: number, bytes: Uint8Array): void {
    this.held.push({ seq, bytes });
    if (this.held.length > HELD_MAX) this.held.shift();
  }

  /**
   * The answer to `terminal.attach`. Safe to call again later — that is what
   * a reconnect does.
   */
  attach(scrollback: Uint8Array, seq: number): AttachOutcome {
    // The scrollback is only needed when it carries something the screen has
    // not already seen: the first attach, or one after missed events.
    const rewritten = !this.attached || seq > this.applied;
    if (rewritten) {
      this.sink.reset();
      if (scrollback.length > 0) this.sink.write(scrollback);
      this.applied = seq;
    }
    this.attached = true;
    const held = this.held;
    this.held = [];
    let applied = 0;
    // In the order they arrived; `receive` drops what the scrollback covers,
    // and stops again if this answer ends short of what is being held.
    for (const event of held.sort((a, b) => a.seq - b.seq)) {
      if (event.seq <= this.applied) continue;
      if (this.attached && event.seq === this.applied + 1) applied += 1;
      this.receive(event.seq, event.bytes);
    }
    return { rewritten, applied };
  }

  /** Back to "nothing on screen is known": the next attach writes the scrollback whatever its number. */
  detach(): void {
    this.attached = false;
    this.held = [];
  }
}
