/**
 * Taking text out of a session: the small sums behind "keep as evidence"
 * and "save the last lines", with no React and no engine.
 */

/** Lines offered when the person has not said how many. The engine's own default. */
export const DEFAULT_LINES = 200;

/** The lines a snapshot is asked for when "the last screen" is meant: what the terminal shows at once. */
export function screenLines(rows: number | undefined): number {
  return rows !== undefined && Number.isInteger(rows) && rows > 0 ? rows : 24;
}

/**
 * What a lines field holds, as a number of lines — or null when it is not a
 * whole number. Whether the number is one the engine takes (1 to 10,000) is
 * the engine's to say where it says it: `evidence.capture_terminal` refuses
 * a number outside the range, in a sentence.
 */
export function wholeLines(text: string): number | null {
  const trimmed = text.trim();
  return /^\d{1,9}$/.test(trimmed) ? Number(trimmed) : null;
}

/** The most lines `terminal.snapshot` reads in one answer. */
export const SNAPSHOT_MOST = 10_000;

/**
 * Whether `terminal.snapshot` reads this many lines as asked. Unlike the
 * evidence capture it does not refuse a number outside 1 to 10,000: asked
 * for 0 it answers with one line, asked for 20,000 with 10,000, and says
 * neither. A screen that writes "the last N lines" into a file therefore
 * holds the range itself — otherwise it would say of a cut answer that it is
 * all the session holds.
 */
export function readsAsAsked(lines: number): boolean {
  return Number.isInteger(lines) && lines >= 1 && lines <= SNAPSHOT_MOST;
}

/** The lines in a selection, as a person would count them. */
export function selectionLines(text: string): number {
  if (text === '') return 0;
  return text.replace(/\r\n?/g, '\n').replace(/\n$/, '').split('\n').length;
}

/** What a selection kept as evidence is called until the person says otherwise. */
export function selectionTitle(session: string | undefined): string {
  return `Selection from ${session ?? 'a terminal session'}`;
}

function stamp(at: Date): string {
  const two = (value: number) => String(value).padStart(2, '0');
  return (
    `${at.getFullYear()}${two(at.getMonth() + 1)}${two(at.getDate())}` +
    `-${two(at.getHours())}${two(at.getMinutes())}${two(at.getSeconds())}`
  );
}

/**
 * `SW1-last-200-lines-20261123-031500.txt`. The session's title can come
 * from a device; `lib/download` cleans the name before it is offered.
 */
export function snapshotFilename(session: string, lines: number, at: Date = new Date()): string {
  return `${session}-last-${lines}-lines-${stamp(at)}.txt`;
}

/** The text of a file: what was read, ending in exactly one line break. */
export function asFile(text: string): string {
  return text.endsWith('\n') ? text : `${text}\n`;
}
