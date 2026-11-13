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
import type {
  TerminalAttached,
  TerminalClosedEvent,
  TerminalOutputEvent,
  TerminalRemovedEvent,
} from '@/api/types/terminal';
import { decodeBase64, SessionStream } from './stream';

/** Lines xterm keeps per session. The backend's ring buffer is the real bound. */
const SCROLLBACK_LINES = 10_000;
/** xterm's own limits for a resize, mirrored from the backend's `terminal.open`. */
const COLS = { min: 20, max: 500 };
const ROWS = { min: 5, max: 200 };

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
  private router: ((id: string, data: string) => boolean) | null = null;

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
    term.loadAddon(new WebLinksAddon());

    const host = document.createElement('div');
    host.className = 'nocturne-terminal-host';
    host.style.cssText = 'width:100%;height:100%;';
    parkingLot().append(host);

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
      }),
      view: { ...IDLE_VIEW },
      size: { cols: term.cols, rows: term.rows },
      resizeTimer: null,
      send: { inflight: false, pending: '' },
    };
    this.entries.set(id, entry);

    term.onData((data) => this.input(id, data));
    term.onSelectionChange(() => this.update(id, { selection: term.getSelection() }));
    search.onDidChangeResults((results) =>
      this.update(id, {
        find: results.resultCount > 0 ? { index: results.resultIndex + 1, count: results.resultCount } : null,
      }),
    );

    void this.attach(id);
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
   * Diverts keystrokes, for broadcast typing. The router returns true when it
   * has taken the input; anything it refuses goes to its own session as usual.
   */
  setRouter(router: ((id: string, data: string) => boolean) | null): void {
    this.router = router;
  }

  /** Disposes the terminal. The session itself is closed through `terminal.close`. */
  forget(id: string): void {
    const entry = this.entries.get(id);
    if (!entry) return;
    if (entry.resizeTimer) clearTimeout(entry.resizeTimer);
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
  private async attach(id: string): Promise<void> {
    const entry = this.entries.get(id);
    if (!entry) return;
    this.update(id, { attach: entry.view.attach === 'live' ? 'live' : 'attaching', error: null });
    try {
      const attached = await rpc<TerminalAttached>('terminal.attach', { session_id: id });
      if (!this.entries.has(id)) return;
      entry.stream.attach(decodeBase64(attached.scrollback), attached.seq);
      this.update(id, {
        attach: 'live',
        error: null,
        closed: attached.info.state === 'closed' ? (attached.info.close_reason ?? 'closed') : null,
      });
      if (attached.info.state === 'closed') {
        this.notice(id, `session closed — ${attached.info.close_reason ?? 'the far end hung up'}`);
      }
      entry.term.scrollToBottom();
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
   * Keystrokes go to the backend as they are typed. Sends are chained per
   * session so bytes cannot overtake each other, and anything typed while a
   * send is in flight rides along with the next one.
   */
  input(id: string, data: string): void {
    const entry = this.entries.get(id);
    if (!entry || data === '') return;
    if (this.router?.(id, data)) return;
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
