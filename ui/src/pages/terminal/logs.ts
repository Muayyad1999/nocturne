/**
 * Reading a session log a page at a time: the arithmetic, with no React.
 *
 * A page is whole lines. `terminal.logs.read` starts it with the line the
 * asked-for offset falls in — which can be earlier than what was asked —
 * and says where that is in its answer's `offset`. So the reader never
 * works out where a page begins: it reads the answer. Going forward is
 * `next_offset`. Going back needs a question first, because only the engine
 * knows where lines start (`readBefore`).
 */

import type {
  SessionLogInfo,
  TerminalLogsReadParams,
  TerminalLogsReadResult,
} from '@/api/types/terminal';
import { formatBytes, formatCount } from '@/lib/format';
import { formatDateTime } from '@/lib/time';

/** How much of a log is on screen at once, in bytes of the file. About eight hundred lines. */
export const PAGE_BYTES = 65_536;

/** The most `evidence.capture_log` takes in one item. */
export const EVIDENCE_BYTES = 262_144;

export type ReadLog = (params: TerminalLogsReadParams) => Promise<TerminalLogsReadResult>;

/** What a read is asked with: `redact` only when it is to be switched off, which the engine records. */
function reading(name: string, offset: number, limit: number, raw: boolean): TerminalLogsReadParams {
  return raw ? { name, offset, limit, redact: false } : { name, offset, limit };
}

/** The page that starts at `offset` — or with the line `offset` falls in. */
export function readAt(read: ReadLog, name: string, offset: number, raw: boolean): Promise<TerminalLogsReadResult> {
  return read(reading(name, Math.max(0, offset), PAGE_BYTES, raw));
}

/**
 * The page that ends exactly where `before` begins: for "previous", and for
 * "the end" (`before` being the size of the file).
 *
 * Two reads. The first asks only where the line a page-length earlier
 * starts; it is always a redacted read of one line, so that looking for an
 * offset neither fetches secrets nor writes an entry to the audit trail.
 * The second reads from there to `before`, which is then whole lines and
 * exactly that many bytes — no line shown twice, none skipped.
 */
export async function readBefore(
  read: ReadLog,
  name: string,
  before: number,
  raw: boolean,
): Promise<TerminalLogsReadResult> {
  const wanted = Math.max(0, before - PAGE_BYTES);
  const start = wanted === 0 ? 0 : (await read({ name, offset: wanted, limit: 1 })).offset;
  // A line longer than a page: it is the page before, on its own.
  if (start >= before) return readAt(read, name, start, raw);
  return read(reading(name, start, before - start, raw));
}

/** Whether there is anything before this page. */
export function hasBefore(page: Pick<TerminalLogsReadResult, 'offset'>): boolean {
  return page.offset > 0;
}

/** Whether there is anything after this page, as far as the engine has said. */
export function hasAfter(page: Pick<TerminalLogsReadResult, 'eof'>): boolean {
  return !page.eof;
}

/** "Bytes 340 to 606 of 3,418", from what the answer says and not from what was asked. */
export function pageRange(page: Pick<TerminalLogsReadResult, 'offset' | 'next_offset' | 'size' | 'eof'>): string {
  if (page.next_offset <= page.offset) {
    return page.size === 0 ? 'The log is empty.' : `Nothing at byte ${formatCount(page.offset)}: the log ends at ${formatCount(page.size)}.`;
  }
  const where = `Bytes ${formatCount(page.offset)} to ${formatCount(page.next_offset)} of ${formatCount(page.size)}`;
  if (page.offset === 0 && page.eof) return `${where} — the whole log`;
  if (page.offset === 0) return `${where} — the start of the log`;
  if (page.eof) return `${where} — the end of the log`;
  return where;
}

/** The lines on a page: what the text holds, without the line break that ends it. */
export function pageLines(text: string): number {
  if (text === '') return 0;
  return text.replace(/\n$/, '').split('\n').length;
}

/**
 * What `evidence.capture_log` is asked for so that the engine reads the
 * page on screen again, and why not when it cannot be: the offset the page
 * really starts at and its length in bytes of the file.
 */
export function captureRange(
  page: Pick<TerminalLogsReadResult, 'offset' | 'next_offset'>,
): { offset: number; limit: number } | { why: string } {
  const limit = page.next_offset - page.offset;
  if (limit <= 0) return { why: 'There are no lines on this page to keep.' };
  if (limit > EVIDENCE_BYTES) {
    return { why: `This page is ${formatBytes(limit)} and an evidence item holds at most ${formatBytes(EVIDENCE_BYTES)}.` };
  }
  return { offset: page.offset, limit };
}

/** When a log was written, as its row says it. */
export function logPeriod(log: Pick<SessionLogInfo, 'started_at' | 'ended_at' | 'active'>): string {
  const started = formatDateTime(log.started_at);
  if (log.active) return `${started} — still being written`;
  // The end is not known for a session that Nocturne itself did not see end.
  if (log.ended_at === null || log.ended_at === undefined) return `${started} — end not recorded`;
  return `${started} to ${formatDateTime(log.ended_at)}`;
}

/** The device a log is of, by the inventory's name for it, or that it has none. */
export function logDevice(
  log: Pick<SessionLogInfo, 'device_id'>,
  devices: readonly { id: string; name: string }[] | undefined,
): string {
  if (!log.device_id) return 'not an inventory device';
  const device = devices?.find((entry) => entry.id === log.device_id);
  if (device) return device.name;
  // The list has not arrived, or the device was removed since: neither is "no device".
  return devices === undefined ? 'device not looked up yet' : 'a device no longer in the inventory';
}

/** What the list says about how much of the logs it shows. */
export function listCoverage(shown: number, limit: number, filtered: boolean): string {
  if (shown === 0) return '';
  const what = filtered ? 'logs that match' : 'logs';
  // The engine answers with a plain list; a full page is the only sign there may be more.
  if (shown >= limit) return `The ${formatCount(shown)} newest ${what}. There may be older ones.`;
  if (shown === 1) return filtered ? 'The one log that matches.' : 'The one log there is.';
  return filtered ? `All ${formatCount(shown)} logs that match.` : `All ${formatCount(shown)} logs there are.`;
}
