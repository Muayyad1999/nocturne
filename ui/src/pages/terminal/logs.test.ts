/**
 * Paging through a session log.
 *
 * The engine is stood in for by `fileReader`, which answers as
 * `terminal.logs.read` does — a page is whole lines, it starts with the line
 * the asked-for offset falls in, and it never holds less than one. That is
 * the behaviour recorded from a running server: asked for byte 347 with a
 * limit of 300 it answered from byte 340, `{"offset":340,"next_offset":606}`.
 * The end-to-end test reads a real log through the real method.
 */

import { describe, expect, it } from 'vitest';
import type { TerminalLogsReadParams, TerminalLogsReadResult } from '@/api/types/terminal';
import { formatCount } from '@/lib/format';
import {
  captureRange,
  EVIDENCE_BYTES,
  hasAfter,
  hasBefore,
  listCoverage,
  logDevice,
  logPeriod,
  PAGE_BYTES,
  pageLines,
  pageRange,
  readAt,
  readBefore,
} from './logs';

/** A log of `count` lines, each `width` bytes with its line break. */
function logOf(count: number, width: number): string {
  let text = '';
  for (let index = 0; index < count; index += 1) {
    text += `${String(index).padStart(6, '0')} ${'x'.repeat(width - 8)}\n`;
  }
  return text;
}

/** What was asked of the engine, and its answers. ASCII only, so a character is a byte. */
function fileReader(file: string) {
  const asked: TerminalLogsReadParams[] = [];
  const read = async (params: TerminalLogsReadParams): Promise<TerminalLogsReadResult> => {
    asked.push(params);
    const size = file.length;
    const wanted = Math.min(params.offset ?? 0, size);
    const limit = params.limit ?? 262_144;
    // The line the offset falls in starts after the line break before it.
    const start = wanted >= size ? size : file.lastIndexOf('\n', wanted - 1) + 1;
    let end = start;
    while (end < size) {
      const lineEnd = file.indexOf('\n', end) + 1 || size;
      if (end > start && lineEnd - start > limit) break;
      end = lineEnd;
      if (end - start >= limit) break;
    }
    return {
      text: file.slice(start, end),
      offset: start,
      size,
      eof: end >= size,
      next_offset: end,
      redacted: params.redact !== false,
    };
  };
  return { read, asked };
}

describe('reading forward', () => {
  it('starts where the answer says the page starts, not where it was asked to', async () => {
    const file = logOf(10, 100);
    const { read } = fileReader(file);

    // Byte 250 is in the middle of the third line, which starts at 200.
    const page = await readAt(read, 'a.log', 250, false);
    expect(page.offset).toBe(200);
    expect(page.text.startsWith('000002 ')).toBe(true);
  });

  it('covers a log end to end with no line twice and none missing', async () => {
    const file = logOf(3000, 100);
    const { read } = fileReader(file);

    let page = await readAt(read, 'a.log', 0, false);
    let seen = page.text;
    while (hasAfter(page)) {
      const next = await readAt(read, 'a.log', page.next_offset, false);
      expect(next.offset).toBe(page.next_offset);
      seen += next.text;
      page = next;
    }
    expect(seen).toBe(file);
    expect(page.next_offset).toBe(file.length);
  });

  it('asks for a redacted page unless the person asked otherwise', async () => {
    const { read, asked } = fileReader(logOf(4, 100));
    await readAt(read, 'a.log', 0, false);
    await readAt(read, 'a.log', 0, true);
    expect(asked).toEqual([
      { name: 'a.log', offset: 0, limit: PAGE_BYTES },
      { name: 'a.log', offset: 0, limit: PAGE_BYTES, redact: false },
    ]);
  });
});

describe('reading backward', () => {
  it('gives the page that ends exactly where the one on screen begins', async () => {
    // 97-byte lines: a page length is not a whole number of them, so the
    // page before cannot be found by subtracting.
    const file = logOf(3000, 97);
    const { read } = fileReader(file);
    const end = await readBefore(read, 'a.log', file.length, false);
    expect(end.eof).toBe(true);
    expect(end.next_offset).toBe(file.length);

    let page = end;
    let seen = page.text;
    while (hasBefore(page)) {
      const before = await readBefore(read, 'a.log', page.offset, false);
      // Adjacent: it ends where the next begins, on a line start.
      expect(before.next_offset).toBe(page.offset);
      expect(before.offset % 97).toBe(0);
      seen = before.text + seen;
      page = before;
    }
    expect(page.offset).toBe(0);
    expect(seen).toBe(file);
  });

  it('looks for where a line starts with a redacted read of one line, even in a raw reading', async () => {
    const file = logOf(3000, 97);
    const { read, asked } = fileReader(file);

    const page = await readBefore(read, 'a.log', file.length, true);

    // The first read only finds an offset: it must not fetch secrets or
    // leave an entry in the audit trail, so it never says `redact: false`.
    expect(asked[0]).toEqual({ name: 'a.log', offset: file.length - PAGE_BYTES, limit: 1 });
    expect(asked[1]).toEqual({ name: 'a.log', offset: page.offset, limit: file.length - page.offset, redact: false });
    expect(asked).toHaveLength(2);
    expect(page.redacted).toBe(false);
  });

  it('reads a short log, or the first page, in one go', async () => {
    const file = logOf(5, 100);
    const { read, asked } = fileReader(file);
    const page = await readBefore(read, 'a.log', file.length, false);
    expect(page.text).toBe(file);
    expect(asked).toEqual([{ name: 'a.log', offset: 0, limit: file.length }]);
  });
});

describe('what a page says about itself', () => {
  const page = (offset: number, next_offset: number, size: number) => ({ offset, next_offset, size, eof: next_offset >= size });

  it('says which bytes it is, from the answer', () => {
    // Numbers are grouped as the rest of the interface groups them.
    const size = formatCount(3418);
    expect(pageRange(page(340, 606, 3418))).toBe(`Bytes 340 to 606 of ${size}`);
    expect(pageRange(page(0, 340, 3418))).toBe(`Bytes 0 to 340 of ${size} — the start of the log`);
    expect(pageRange(page(3000, 3418, 3418))).toBe(`Bytes ${formatCount(3000)} to ${size} of ${size} — the end of the log`);
    expect(pageRange(page(0, 301, 301))).toBe('Bytes 0 to 301 of 301 — the whole log');
  });

  it('tells an empty log from a place past the end of one', () => {
    expect(pageRange(page(0, 0, 0))).toBe('The log is empty.');
    const end = formatCount(3418);
    expect(pageRange(page(3418, 3418, 3418))).toBe(`Nothing at byte ${end}: the log ends at ${end}.`);
  });

  it('counts its lines', () => {
    expect(pageLines('')).toBe(0);
    expect(pageLines('a\nb\n')).toBe(2);
    expect(pageLines('a\nb')).toBe(2);
  });

  it('knows whether there is more either side', () => {
    expect(hasBefore({ offset: 0 })).toBe(false);
    expect(hasBefore({ offset: 340 })).toBe(true);
    expect(hasAfter({ eof: true })).toBe(false);
    expect(hasAfter({ eof: false })).toBe(true);
  });
});

describe('keeping a page as evidence', () => {
  it('asks the engine for the bytes the page really is', async () => {
    const file = logOf(3000, 97);
    const { read } = fileReader(file);
    // Asked for the middle of a line: the page starts before that.
    const shown = await readAt(read, 'a.log', 1000, false);
    const range = captureRange(shown);
    expect(range).toEqual({ offset: shown.offset, limit: shown.next_offset - shown.offset });
    expect(shown.offset).toBeLessThan(1000);

    // Read again with exactly that, as the engine will: the same lines.
    if ('why' in range) throw new Error(range.why);
    const again = await read({ name: 'a.log', offset: range.offset, limit: range.limit });
    expect(again.text).toBe(shown.text);
  });

  it('says why a page cannot be kept rather than keeping part of it', () => {
    expect(captureRange({ offset: 3418, next_offset: 3418 })).toEqual({ why: 'There are no lines on this page to keep.' });
    const tooMuch = captureRange({ offset: 0, next_offset: EVIDENCE_BYTES + 1 });
    expect('why' in tooMuch && tooMuch.why).toMatch(/an evidence item holds at most/);
    expect(captureRange({ offset: 0, next_offset: EVIDENCE_BYTES })).toEqual({ offset: 0, limit: EVIDENCE_BYTES });
  });
});

describe('a log in the list', () => {
  it('says a log is still being written, or that its end was not recorded', () => {
    expect(logPeriod({ started_at: 1795400988014, ended_at: null, active: true })).toMatch(/ — still being written$/);
    expect(logPeriod({ started_at: 1795400988014, ended_at: null, active: false })).toMatch(/ — end not recorded$/);
    expect(logPeriod({ started_at: 1795400988014, ended_at: 1795400999014, active: false })).toMatch(/ to /);
  });

  it('names the device, and tells "no device" from "not known"', () => {
    const devices = [{ id: 'wlkdtfcp6sovgrze', name: 'SW1-LAB' }];
    expect(logDevice({ device_id: 'wlkdtfcp6sovgrze' }, devices)).toBe('SW1-LAB');
    expect(logDevice({ device_id: null }, devices)).toBe('not an inventory device');
    expect(logDevice({ device_id: 'gone' }, devices)).toBe('a device no longer in the inventory');
    expect(logDevice({ device_id: 'wlkdtfcp6sovgrze' }, undefined)).toBe('device not looked up yet');
  });

  it('says whether the list is all of them or only the newest', () => {
    expect(listCoverage(3, 100, false)).toBe('All 3 logs there are.');
    expect(listCoverage(1, 100, false)).toBe('The one log there is.');
    expect(listCoverage(2, 100, true)).toBe('All 2 logs that match.');
    expect(listCoverage(100, 100, false)).toBe('The 100 newest logs. There may be older ones.');
    expect(listCoverage(0, 100, false)).toBe('');
  });
});
