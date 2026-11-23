/**
 * The seat belt in front of a paste.
 *
 * Two things are worth a second look: text with more than one line (every
 * newline in it is an Enter on the device) and a line the device's own policy
 * calls a change or dangerous. Everything else goes straight through — the
 * belt must not become a lock.
 *
 * This file is the arithmetic: what counts as a line, what is worth asking
 * about, and what bytes the device finally gets. `pasteFunnel.ts` is the one
 * road every paste takes through it.
 */

import type { CommandClass, TerminalLineClass } from '@/api/types/terminal';

/**
 * Every line ending as one `\n`: CRLF from a Windows clipboard, the bare
 * carriage return of text captured from a session, and LF. This is the form
 * the sheet shows and the policy is asked about — the backend splits on `\n`
 * only, so text left with bare carriage returns would be judged as one line.
 */
export function normaliseLines(text: string): string {
  return text.replace(/\r\n?/g, '\n');
}

/** Lines, keeping blank ones, in the order the backend classifies them. */
export function splitLines(text: string): string[] {
  return normaliseLines(text).split('\n');
}

/**
 * The text as the keyboard would have sent it: one carriage return per line
 * ending, because that is what the Enter key is. A line feed after it is a
 * second Enter to most devices, and answers whatever the first one asked —
 * `reload` followed by CRLF confirms its own `[confirm]`.
 */
export function asTyped(text: string): string {
  return normaliseLines(text).replace(/\n/g, '\r');
}

const PASTE_START = '\u001b[200~';
const PASTE_END = '\u001b[201~';

/**
 * The paste between the markers a program asks for with bracketed paste mode
 * (DECSET 2004): a shell then holds the lines instead of running each one,
 * and an editor does not re-indent them.
 *
 * The end marker is taken out of the text first, until none is left, so that
 * what is pasted cannot close the bracket itself and have its remainder run
 * as if it had been typed.
 */
export function bracketed(text: string): string {
  let inside = text;
  while (inside.includes(PASTE_END)) inside = inside.split(PASTE_END).join('');
  return PASTE_START + inside + PASTE_END;
}

export interface PasteRisk {
  lines: number;
  /** Lines that would change the configuration. */
  changes: number;
  /** Lines the policy calls disruptive. */
  dangerous: number;
  /** The first reason worth repeating in the summary. */
  headline: string | null;
}

const RANK: Record<CommandClass, number> = { read_only: 0, change: 1, dangerous: 2 };

export function pasteRisk(classes: readonly TerminalLineClass[]): PasteRisk {
  let changes = 0;
  let dangerous = 0;
  let worst: TerminalLineClass | null = null;
  for (const line of classes) {
    if (line.class === 'change') changes += 1;
    if (line.class === 'dangerous') dangerous += 1;
    if (!worst || RANK[line.class] > RANK[worst.class]) worst = line;
  }
  return {
    lines: classes.length,
    changes,
    dangerous,
    headline: worst && worst.class !== 'read_only' ? worst.reason : null,
  };
}

/**
 * Whether to put the sheet in front of this paste. `classes` is what
 * `terminal.classify` said; without it (the call failed) only the shape of
 * the text decides, because a missing answer must not wave anything through
 * *or* block everything.
 */
export function needsConfirmation(text: string, classes: readonly TerminalLineClass[] | null): boolean {
  if (hasSeveralLines(text)) return true;
  if (!classes) return false;
  return classes.some((line) => line.class !== 'read_only');
}

/** The lines a person would count: the line ending that closes the last one is not another. */
export function shownLines(text: string): string[] {
  return normaliseLines(text).replace(/\n$/, '').split('\n');
}

/**
 * More than one line of input. A single line with a trailing newline is one
 * command and an Enter: that is what typing it would do anyway.
 */
export function hasSeveralLines(text: string): boolean {
  return shownLines(text).length > 1;
}

/** Where pasted text came from, so the question can say it. */
export type PasteSource = 'paste' | 'snippet' | 'drop';

const VERB: Record<PasteSource, string> = {
  paste: 'Paste',
  snippet: 'Send this snippet',
  drop: 'Drop this text',
};

/** The question itself: what is being done, and to what. */
export function pasteTitle(question: { source: PasteSource; title: string; targets: number }): string {
  const where = question.targets > 1 ? `${question.targets} sessions` : question.title;
  return `${VERB[question.source]} into ${where}?`;
}

/** What the sheet says under its title, in one sentence. */
export function pasteSummary(text: string, risk: PasteRisk | null): string {
  const lines = shownLines(text).length;
  const count = lines === 1 ? 'one line' : `${lines} lines`;
  if (!risk || (risk.changes === 0 && risk.dangerous === 0)) {
    return `${count} will be sent to the device as if you had typed them.`;
  }
  const parts: string[] = [];
  if (risk.dangerous > 0) parts.push(`${risk.dangerous} disruptive`);
  if (risk.changes > 0) parts.push(`${risk.changes} that change the configuration`);
  return `${count}, ${parts.join(' and ')}.`;
}

/** The modifiers of a key press that matter for a paste chord. */
export interface PasteKey {
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}

/**
 * Who pastes when a paste chord (Ctrl+Shift+V, ⌘V, ⌘⇧V) is pressed in a
 * terminal.
 *
 * `browser`: the browser has its own binding for the chord and raises a
 * `paste` event that carries the text. That is the better road wherever it
 * exists, because it needs no permission to read the clipboard: Ctrl+Shift+V
 * off a Mac ("paste as plain text" in Chromium, Firefox, WebView2 and
 * WebKitGTK alike) and ⌘V on one.
 *
 * `clipboard`: nothing is bound to the chord, so the clipboard is read
 * instead. Ctrl+Shift+V on a Mac; and ⌘⇧V in the desktop shell there, whose
 * Edit menu has Paste but no "Paste and Match Style" as a browser's has.
 */
export function pasteKeyRoute(
  key: PasteKey,
  where: { mac: boolean; desktop: boolean },
): 'browser' | 'clipboard' {
  if (!where.mac) return 'browser';
  if (!key.metaKey) return 'clipboard';
  return key.shiftKey && where.desktop ? 'clipboard' : 'browser';
}

/**
 * What a dragged session tab carries besides its id. A tab's drag also holds
 * the id as plain text, and without this mark a tab let go over a terminal
 * would be pasted into it.
 */
export const TAB_DRAG_TYPE = 'application/x-nocturne-terminal-tab';

/** The part of a `DataTransfer` a drop is read through. */
export interface DroppedData {
  readonly types: readonly string[];
  getData(type: string): string;
}

/**
 * Whether a drag is text a terminal could take. Decided from the types alone:
 * while something is still being dragged the browser does not let a page read
 * what it is.
 */
export function carriesText(types: readonly string[]): boolean {
  return types.includes('text/plain') && !types.includes(TAB_DRAG_TYPE);
}

/** The text a drop would paste, or null when it is not text for a terminal. */
export function droppedText(transfer: DroppedData): string | null {
  if (!carriesText(transfer.types)) return null;
  const text = transfer.getData('text/plain');
  return text === '' ? null : text;
}
