/**
 * The terminals themselves: one xterm.js instance per session, owned by this
 * module rather than by React.
 *
 * Why outside React: a session must survive leaving the screen, and output
 * must never cost a render. An instance is created when a session is first
 * opened in a tab and lives until the session is closed or forgotten; while
 * the screen is elsewhere the terminal stays parked off screen, still
 * receiving output, so coming back shows the device's last line instantly and
 * without a reattach.
 *
 * `terminal.output` events are decoded and written straight into xterm. React
 * is told only when something it draws changes — the attach state, the
 * selection, the find results — through a tiny store (`subscribe`/`view`).
 *
 * Pasted text is this module's business too, for the same reason the
 * terminals are: every screen that shows a terminal borrows it from here
 * (the Terminal screen, a lab's consoles, the session docked on the map), and
 * a paste must be asked about and sent in one way whichever of them it lands
 * in. See `paste` below and `pasteFunnel.ts`.
 */

// The stylesheet belongs with the module that creates the terminals, not
// with whichever screen remembers to import it: without it, xterm's hidden
// character-measuring element is painted over the output as a row of
// repeated glyphs. The Lab screen found that out by being the second screen
// to show a terminal. Anything that uses this registry now gets the styles
// with it.
import '@xterm/xterm/css/xterm.css';
import { FitAddon } from '@xterm/addon-fit';
import { SearchAddon } from '@xterm/addon-search';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { Terminal, type ITheme } from '@xterm/xterm';
import { rpc, subscribe, useConnection } from '@/api/client';
import { RpcError } from '@/api/errors';
import { openExternal } from '@/api/transports/shell';
import type {
  SessionInfo,
  TerminalAttached,
  TerminalClosedEvent,
  TerminalLineClass,
  TerminalOutputEvent,
  TerminalRemovedEvent,
} from '@/api/types/terminal';
import { confirm } from '@/design/Dialog';
import { toast } from '@/design/Toast';
import { pasteConfirmLabel, pasteRisk, pasteSummary, pasteTitle } from './paste';
import { PasteFunnel, type PasteQuestion, type PasteSource } from './pasteFunnel';
import { decodeBase64, SessionStream } from './stream';

/** Lines xterm keeps per session. The backend's ring buffer is the real bound. */
const SCROLLBACK_LINES = 10_000;
/** xterm's own limits for a resize, mirrored from the backend's `terminal.open`. */
const COLS = { min: 20, max: 500 };
const ROWS = { min: 5, max: 200 };
/** How long to wait before asking again for a scrollback that came back short. */
const RESYNC_MS = { first: 200, most: 5000 };

export type AttachState = 'attaching' | 'live' | 'failed';

/** What React draws about a session. Replaced, never mutated. */
export interface SessionView {
  attach: AttachState;
  /** Why the attach failed, in the backend's words. */
  error: string | null;
  /** The text selected in this terminal, or `''`. */
  selection: string;
  find: { index: number; count: number } | null;
  /** The reason the far end gave, once it has gone. */
  closed: string | null;
}

interface Entry {
  id: string;
  term: Terminal;
  fit: FitAddon;
  search: SearchAddon;
  stream: SessionStream;
  /** The element xterm draws in; moved between the pane and the parking lot. */
  host: HTMLDivElement;
  opened: boolean;
  view: SessionView;
  size: { cols: number; rows: number };
  resizeTimer: ReturnType<typeof setTimeout> | null;
  send: { inflight: boolean; pending: string };
  /** What the backend said the session is, once an attach has answered. */
  info: SessionInfo | null;
  /** Settles when the first attach has answered, one way or the other. */
  ready: Promise<void>;
  /** The attach in flight, and whether another was asked for meanwhile. */
  attaching: Promise<void> | null;
  attachAgain: boolean;
  /** Attach answers in a row that left a hole in the output, and the retry waiting on them. */
  shortAnswers: number;
  resyncTimer: ReturnType<typeof setTimeout> | null;
}

/** Typing that goes to several sessions at once, while a broadcast is on. */
export interface Broadcast {
  /** The sessions typing goes to. Input for any other session is its own. */
  targets: readonly string[];
  /** Sends to all of them in one call, so none can fall behind the others. */
  send: (data: string) => void;
}

/** Shows a paste question and resolves true to send, false to drop it. */
export type PasteAsker = (question: PasteQuestion) => Promise<boolean>;

/**
 * The question as a plain confirm dialog: for a screen that lends a terminal
 * without a paste sheet of its own (a lab console, the session docked on the
 * map). The Terminal screen replaces it with the sheet that shows every line.
 */
function askPlainly(question: PasteQuestion): Promise<boolean> {
  const risk = question.classes ? pasteRisk(question.classes) : null;
  const sentences = [pasteSummary(question.text, risk)];
  if (risk?.headline) sentences.push(`${risk.headline}.`);
  if (question.classes === null) sentences.push('The device policy could not be asked about these lines.');
  return confirm({
    title: pasteTitle(question),
    description: sentences.join(' '),
    confirmLabel: pasteConfirmLabel(question.source),
    tone: risk && risk.dangerous > 0 ? 'danger' : 'default',
  });
}

/** An error as a sentence: the shell rejects with a string, the engine with an `RpcError`. */
function reason(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === 'string' ? error : 'no reason was given';
}

const IDLE_VIEW: SessionView = { attach: 'attaching', error: null, selection: '', find: null, closed: null };

/** A terminal that is not on screen still needs somewhere to be, with a size. */
function parkingLot(): HTMLElement {
  const existing = document.getElementById('nocturne-terminal-parking');
  if (existing) return existing;
  const parking = document.createElement('div');
  parking.id = 'nocturne-terminal-parking';
  parking.setAttribute('aria-hidden', 'true');
  parking.style.cssText =
    'position:fixed;left:-20000px;top:0;width:960px;height:600px;overflow:hidden;visibility:hidden;';
  document.body.append(parking);
  return parking;
}

class Terminals {
  private entries = new Map<string, Entry>();
  private listeners = new Set<() => void>();
  private theme: ITheme = {};
  private fontFamily = 'monospace';
  private fontSize = 13;
  private lineHeight = 1.25;
  private started = false;
  /** Set while a broadcast is on: it decides where a keystroke goes. */
  private broadcast: Broadcast | null = null;
  /** How a paste question is shown; the plain dialog when no screen has said. */
  private asker: PasteAsker | null = null;

  /** The one road pasted text takes to a device: see `pasteFunnel.ts`. */
  private pastes = new PasteFunnel({
    target: async (id) => {
      const entry = this.entries.get(id);
      if (!entry) return null;
      // The title and the device arrive with the attach answer. A paste made
      // before it waits, rather than being judged by the generic policy
      // because nobody had yet said which device this is.
      await entry.ready;
      if (this.entries.get(id) !== entry) return null;
      return {
        title: entry.info?.title ?? 'this session',
        deviceId: entry.info?.device_id ?? null,
        targets: this.broadcast?.targets.includes(id) ? this.broadcast.targets.length : 1,
        bracketedPaste: entry.term.modes.bracketedPasteMode,
      };
    },
    classify: (deviceId, text) =>
      rpc<TerminalLineClass[]>('terminal.classify', { device_id: deviceId, text }),
    ask: (question) => (this.asker ?? askPlainly)(question),
    send: (id, data) => this.dispatch(id, data),
  });

  // ----------------------------------------------------------- the store

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** The view of a session; a stable object until something about it changes. */
  view(id: string): SessionView {
    return this.entries.get(id)?.view ?? IDLE_VIEW;
  }

  private update(id: string, change: Partial<SessionView>): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    const next = { ...entry.view, ...change };
    const same = (Object.keys(change) as (keyof SessionView)[]).every((key) => {
      const before = entry.view[key];
      const after = next[key];
      if (key === 'find') {
        const a = before as SessionView['find'];
        const b = after as SessionView['find'];
        return a?.index === b?.index && a?.count === b?.count;
      }
      return before === after;
    });
    if (same) return;
    entry.view = next;
    for (const listener of this.listeners) listener();
  }

  // ------------------------------------------------------- the event feed

  /** Starts listening once, and keeps listening for as long as the app runs. */
  private start(): void {
    if (this.started) return;
    this.started = true;

    subscribe<TerminalOutputEvent>('terminal.output', ({ payload }) => {
      const entry = this.entries.get(payload.session_id);
      if (!entry) return;
      entry.stream.receive(payload.seq, decodeBase64(payload.data));
    });

    subscribe<TerminalClosedEvent>('terminal.closed', ({ payload }) => {
      if (!this.entries.has(payload.session_id)) return;
      this.update(payload.session_id, { closed: payload.reason });
      this.notice(payload.session_id, `session closed — ${payload.reason}`);
    });

    subscribe<TerminalRemovedEvent>('terminal.removed', ({ payload }) => {
      if (!this.entries.has(payload.session_id)) return;
      this.update(payload.session_id, { closed: this.view(payload.session_id).closed ?? 'the backend let it go' });
    });

    // The event stream can drop (a laptop lid, a restarted server). Whatever
    // the device printed in the meantime is in the backend's scrollback, so
    // every live terminal attaches again and the protocol sorts out the rest.
    useConnection.subscribe((connection, previous) => {
      if (connection.state !== 'online' || previous.state === 'online') return;
      for (const id of this.entries.keys()) void this.attach(id);
    });

    // The server drops events for a client that has fallen too far behind,
    // and says so on the same socket, which stays open: nothing reconnects.
    // A session that keeps printing shows the hole in its numbering and asks
    // for its scrollback itself (`SessionStream.receive`). One that went
    // quiet straight after the lost events never would, so every terminal
    // asks here; `attach` rewrites only those the backend has run ahead of.
    subscribe('events.lagged', () => {
      for (const id of this.entries.keys()) void this.attach(id);
    });
  }

  // --------------------------------------------------------- the sessions

  /**
   * The terminal for a session, created and attached the first time it is
   * asked for. Idempotent: both the screen and the pane call it.
   */
  adopt(id: string): void {
    this.start();
    if (this.entries.has(id)) return;

    const term = new Terminal({
      allowProposedApi: true,
      cursorBlink: true,
      cursorStyle: 'block',
      cursorInactiveStyle: 'outline',
      convertEol: false,
      drawBoldTextInBrightColors: false,
      fontFamily: this.fontFamily,
      fontSize: this.fontSize,
      lineHeight: this.lineHeight,
      letterSpacing: 0,
      // Enough contrast to read a device's dark blue on black, never so much
      // that the colours it chose stop meaning anything.
      minimumContrastRatio: 3,
      scrollback: SCROLLBACK_LINES,
      theme: this.theme,
      windowsPty: undefined,
      wordSeparator: ' ()[]{}\'"`,;:',
    });
    const fit = new FitAddon();
    const search = new SearchAddon();
    term.loadAddon(fit);
    term.loadAddon(search);
    term.loadAddon(
      // The add-on's own handler calls `window.open`, which goes nowhere in
      // the desktop shell's WebView: the link underlined and did nothing.
      new WebLinksAddon((_event, address) => {
        openExternal(address).catch((error: unknown) => {
          toast.error('The link could not be opened', { description: reason(error) });
        });
      }),
    );

    const host = document.createElement('div');
    host.className = 'nocturne-terminal-host';
    host.style.cssText = 'width:100%;height:100%;';
    parkingLot().append(host);

    // xterm listens for `paste` on its textarea and on its own element, and
    // sends the text to the device the instant the event arrives: before any
    // question, and a second time if a listener beside it then asks one and
    // is told yes. `preventDefault` does not stop a listener. Taking the
    // event here, in the capture phase on the element xterm lives in, means
    // it never reaches xterm at all. Every paste the browser raises comes
    // this way — Shift+Insert, Ctrl+Shift+V, ⌘V, a middle click on Linux,
    // the WebView's Edit menu — and carries its own text, so none of them
    // needs permission to read the clipboard.
    host.addEventListener(
      'paste',
      (event) => {
        event.preventDefault();
        event.stopPropagation();
        this.paste(id, event.clipboardData?.getData('text/plain') ?? '');
      },
      true,
    );

    const entry: Entry = {
      id,
      term,
      fit,
      search,
      host,
      opened: false,
      stream: new SessionStream({
        write: (bytes) => term.write(bytes),
        reset: () => term.reset(),
        resync: () => this.resync(id),
      }),
      view: { ...IDLE_VIEW },
      size: { cols: term.cols, rows: term.rows },
      resizeTimer: null,
      send: { inflight: false, pending: '' },
      info: null,
      ready: Promise.resolve(),
      attaching: null,
      attachAgain: false,
      shortAnswers: 0,
      resyncTimer: null,
    };
    this.entries.set(id, entry);

    term.onData((data) => this.input(id, data));
    term.onSelectionChange(() => this.update(id, { selection: term.getSelection() }));
    search.onDidChangeResults((results) =>
      this.update(id, {
        find: results.resultCount > 0 ? { index: results.resultIndex + 1, count: results.resultCount } : null,
      }),
    );

    entry.ready = this.attach(id);
  }

  /** Whether a session already has a terminal here. */
  has(id: string): boolean {
    return this.entries.has(id);
  }

  /** Every session with a terminal here. */
  ids(): string[] {
    return [...this.entries.keys()];
  }

  /**
   * Diverts keystrokes, for broadcast typing: what is typed (or pasted) into
   * one of the targets goes to all of them. Anything for another session goes
   * to that session as usual.
   */
  setBroadcast(broadcast: Broadcast | null): void {
    this.broadcast = broadcast;
  }

  /**
   * How a paste that is worth a question is shown. The Terminal screen gives
   * its sheet; `null` goes back to the plain dialog. Whoever sets one must
   * answer what it was asked — an unanswered question holds up every paste
   * behind it.
   */
  setPasteAsker(asker: PasteAsker | null): void {
    this.asker = asker;
  }

  /** Disposes the terminal. The session itself is closed through `terminal.close`. */
  forget(id: string): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    if (entry.resizeTimer) clearTimeout(entry.resizeTimer);
    if (entry.resyncTimer) clearTimeout(entry.resyncTimer);
    entry.term.dispose();
    entry.host.remove();
    this.entries.delete(id);
    for (const listener of this.listeners) listener();
  }

  // --------------------------------------------------------- the protocol

  /**
   * `terminal.attach`: the scrollback the backend kept, and the number of the
   * last event it covers. The stream was already listening, so nothing that
   * arrived while this was in flight is lost.
   */
  private attach(id: string): Promise<void> {
    const entry = this.entries.get(id);
    if (!entry) return Promise.resolve();
    if (entry.attaching) {
      // One at a time: an answer carries the whole scrollback, and a lagging
      // client that asked once per notice would bury itself. But the answer
      // on its way may have been taken before whatever prompted this request,
      // so it is asked once more when that one is in.
      entry.attachAgain = true;
      return entry.attaching;
    }
    entry.attaching = this.attachOnce(entry).finally(() => {
      entry.attaching = null;
      if (!entry.attachAgain) return;
      entry.attachAgain = false;
      if (this.entries.get(id) === entry) void this.attach(id);
    });
    return entry.attaching;
  }

  /**
   * The stream found a hole in the output and wants the scrollback. Asked at
   * once the first time. An answer can fall short of the hole once, because
   * it was already on its way; if answers keep falling short something is
   * wrong at the far end, and asking in a tight loop would only freeze the
   * window — so each further try waits longer than the last.
   */
  private resync(id: string): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    const tries = entry.shortAnswers;
    entry.shortAnswers += 1;
    if (tries === 0) {
      void this.attach(id);
      return;
    }
    if (entry.resyncTimer) return;
    const wait = Math.min(RESYNC_MS.most, RESYNC_MS.first * 2 ** (tries - 1));
    entry.resyncTimer = setTimeout(() => {
      entry.resyncTimer = null;
      if (this.entries.get(id) === entry) void this.attach(id);
    }, wait);
  }

  private async attachOnce(entry: Entry): Promise<void> {
    const id = entry.id;
    this.update(id, { attach: entry.view.attach === 'live' ? 'live' : 'attaching', error: null });
    try {
      const attached = await rpc<TerminalAttached>('terminal.attach', { session_id: id });
      if (!this.entries.has(id)) return;
      entry.info = attached.info;
      const { rewritten } = entry.stream.attach(decodeBase64(attached.scrollback), attached.seq);
      if (entry.stream.isAttached) {
        // The screen is whole again: the next hole is a new one.
        entry.shortAnswers = 0;
        if (entry.resyncTimer) clearTimeout(entry.resyncTimer);
        entry.resyncTimer = null;
      }
      // An attach that changed nothing on screen (it is asked for whenever
      // events may have been missed) must not say "closed" a second time, nor
      // pull someone reading the scrollback back down to the last line.
      const saidClosed = entry.view.closed !== null && !rewritten;
      this.update(id, {
        attach: 'live',
        error: null,
        closed: attached.info.state === 'closed' ? (attached.info.close_reason ?? 'closed') : null,
      });
      if (attached.info.state === 'closed' && !saidClosed) {
        this.notice(id, `session closed — ${attached.info.close_reason ?? 'the far end hung up'}`);
      }
      if (rewritten) entry.term.scrollToBottom();
    } catch (error) {
      if (!this.entries.has(id)) return;
      const message = error instanceof RpcError ? error.message : 'the session could not be attached';
      // A session the backend has forgotten is gone, not broken.
      const gone = error instanceof RpcError && error.code === 'not_found';
      this.update(id, gone ? { attach: 'live', closed: 'the backend no longer has this session' } : { attach: 'failed', error: message });
    }
  }

  /** A line of our own in the stream, clearly not the device talking. */
  private notice(id: string, text: string): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    entry.term.write(`\r\n\u001b[2m── ${text} ──\u001b[0m\r\n`);
  }

  // ------------------------------------------------------------- the keys

  /**
   * Text someone wants in a session, by whatever door it came: the browser's
   * own paste, the toolbar, the context menu, a snippet, a drop. It is judged
   * by the device's policy, asked about when that is worth a question, and
   * sent once — or not at all. Nothing reaches the device before the answer.
   */
  paste(id: string, text: string, source: PasteSource = 'paste'): void {
    if (text === '' || !this.entries.has(id)) return;
    this.pastes.offer(id, text, source).catch((error: unknown) => {
      toast.error('The paste was not sent', { description: reason(error) });
    });
  }

  /**
   * Puts a question about text that is typed by the engine, not from here —
   * a snippet sent with `terminal.snippets.send` — to the person, on the same
   * sheet and in the same queue as a paste's. True means yes; nothing is sent
   * by asking.
   */
  confirm(question: PasteQuestion): Promise<boolean> {
    return this.pastes.confirm(question);
  }

  /**
   * What the keyboard produced, or the terminal answering its device. It
   * goes straight out, unless a paste into the same session is still being
   * judged: then it waits behind the paste, as it was typed.
   */
  input(id: string, data: string): void {
    if (data === '' || !this.entries.has(id)) return;
    if (this.pastes.typed(id, data)) return;
    this.dispatch(id, data);
  }

  /**
   * Bytes go to the backend as they come. Sends are chained per session so
   * bytes cannot overtake each other, and anything typed while a send is in
   * flight rides along with the next one.
   */
  private dispatch(id: string, data: string): void {
    const entry = this.entries.get(id);
    if (!entry || data === '') return;
    if (this.broadcast?.targets.includes(id)) {
      this.broadcast.send(data);
      return;
    }
    if (entry.send.inflight) {
      entry.send.pending += data;
      return;
    }
    entry.send.inflight = true;
    void this.flush(id, data);
  }

  private async flush(id: string, data: string): Promise<void> {
    const entry = this.entries.get(id);
    if (!entry) return;
    try {
      await rpc('terminal.input', { session_id: id, data });
    } catch (error) {
      const message = error instanceof RpcError ? error.message : 'the session did not take that';
      this.update(id, { closed: this.view(id).closed ?? message });
      entry.send.pending = '';
    }
    const next = entry.send.pending;
    entry.send.pending = '';
    if (next === '') {
      entry.send.inflight = false;
      return;
    }
    await this.flush(id, next);
  }

  /** The size of the window the far end is told about. */
  resize(id: string, cols: number, rows: number): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    const wanted = {
      cols: Math.min(COLS.max, Math.max(COLS.min, cols)),
      rows: Math.min(ROWS.max, Math.max(ROWS.min, rows)),
    };
    if (wanted.cols === entry.size.cols && wanted.rows === entry.size.rows) return;
    entry.size = wanted;
    entry.term.resize(wanted.cols, wanted.rows);
    // A drag over a splitter is a hundred resizes; the device needs the last.
    if (entry.resizeTimer) clearTimeout(entry.resizeTimer);
    entry.resizeTimer = setTimeout(() => {
      void rpc('terminal.resize', { session_id: id, ...wanted }).catch(() => undefined);
    }, 120);
  }

  // ------------------------------------------------------------ the pane

  /**
   * Puts the terminal into a pane. The returned function parks it again, so a
   * pane can unmount without the session noticing.
   */
  mount(id: string, container: HTMLElement): () => void {
    const entry = this.entries.get(id);
    if (!entry) return () => undefined;
    container.append(entry.host);
    if (!entry.opened) {
      entry.term.open(entry.host);
      entry.opened = true;
    }
    this.refit(id);
    return () => {
      // A session forgotten while its pane was on screen has nothing to park.
      if (this.entries.get(id) === entry) parkingLot().append(entry.host);
    };
  }

  /** Fits the terminal to its pane and tells the far end the new size. */
  refit(id: string): void {
    const entry = this.entries.get(id);
    if (!entry?.opened) return;
    const size = entry.fit.proposeDimensions();
    if (!size || !Number.isFinite(size.cols) || !Number.isFinite(size.rows)) return;
    this.resize(id, size.cols, size.rows);
  }

  focus(id: string): void {
    this.entries.get(id)?.term.focus();
  }

  terminal(id: string): Terminal | undefined {
    return this.entries.get(id)?.term;
  }

  searchAddon(id: string): SearchAddon | undefined {
    return this.entries.get(id)?.search;
  }

  clear(id: string): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    // xterm's `clear` keeps the line the cursor is on, which is the prompt:
    // the screen looks wiped and the session is undisturbed.
    entry.term.clear();
  }

  // ------------------------------------------------------------ the looks

  /** The resolved design tokens, applied to every terminal at once. */
  setAppearance(appearance: { theme: ITheme; fontFamily: string; fontSize: number; lineHeight: number }): void {
    this.theme = appearance.theme;
    this.fontFamily = appearance.fontFamily;
    this.fontSize = appearance.fontSize;
    this.lineHeight = appearance.lineHeight;
    for (const entry of this.entries.values()) {
      entry.term.options.theme = appearance.theme;
      entry.term.options.fontFamily = appearance.fontFamily;
      entry.term.options.fontSize = appearance.fontSize;
      entry.term.options.lineHeight = appearance.lineHeight;
      if (entry.opened) this.refit(entry.id);
    }
  }
}

export const terminals = new Terminals();
