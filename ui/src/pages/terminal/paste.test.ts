import { describe, expect, it } from 'vitest';
import type { TerminalLineClass } from '@/api/types/terminal';
import { hasSeveralLines, needsConfirmation, pasteRisk, pasteSummary, splitLines } from './paste';

const line = (text: string, cls: TerminalLineClass['class'], reason = ''): TerminalLineClass => ({
  line: text,
  class: cls,
  reason,
});

describe('splitLines', () => {
  it('keeps blank lines and takes any line ending', () => {
    expect(splitLines('a\r\nb\n\nc')).toEqual(['a', 'b', '', 'c']);
  });
});

describe('hasSeveralLines', () => {
  it('does not count the newline that ends a single command', () => {
    expect(hasSeveralLines('show version\n')).toBe(false);
    expect(hasSeveralLines('show version')).toBe(false);
    expect(hasSeveralLines('show version\nshow ip route')).toBe(true);
    expect(hasSeveralLines('show version\n\n')).toBe(true);
  });
});

describe('needsConfirmation', () => {
  it('asks about anything with more than one line', () => {
    expect(needsConfirmation('conf t\nint gi0/1\n', null)).toBe(true);
  });

  it('lets a read-only line through', () => {
    expect(needsConfirmation('show version', [line('show version', 'read_only')])).toBe(false);
  });

  it('asks about a single line the policy flags', () => {
    expect(needsConfirmation('reload', [line('reload', 'dangerous', 'disruptive')])).toBe(true);
    expect(needsConfirmation('conf t', [line('conf t', 'change', 'not read-only')])).toBe(true);
  });

  it('does not block a one-liner when the policy could not be asked', () => {
    expect(needsConfirmation('show version', null)).toBe(false);
  });
});

describe('pasteRisk', () => {
  it('counts the classes and keeps the worst reason', () => {
    expect(
      pasteRisk([
        line('show version', 'read_only', 'read-only command'),
        line('conf t', 'change', 'not on the read-only list'),
        line('reload', 'dangerous', '`reload` is disruptive'),
      ]),
    ).toEqual({ lines: 3, changes: 1, dangerous: 1, headline: '`reload` is disruptive' });
  });

  it('has no headline when everything is read-only', () => {
    expect(pasteRisk([line('show version', 'read_only', 'read-only command')]).headline).toBeNull();
  });
});

describe('pasteSummary', () => {
  it('says what will happen in one sentence', () => {
    expect(pasteSummary('show version', null)).toMatch(/^one line will be sent/);
    expect(pasteSummary('a\nb\nc', { lines: 3, changes: 2, dangerous: 1, headline: null })).toBe(
      '3 lines, 1 disruptive and 2 that change the configuration.',
    );
  });
});
