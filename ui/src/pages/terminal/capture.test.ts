import { describe, expect, it } from 'vitest';
import {
  asFile,
  DEFAULT_LINES,
  readsAsAsked,
  screenLines,
  selectionLines,
  selectionTitle,
  snapshotFilename,
  wholeLines,
} from './capture';

describe('how many lines', () => {
  it('reads a whole number and nothing else', () => {
    expect(wholeLines('200')).toBe(200);
    expect(wholeLines(' 50 ')).toBe(50);
    expect(wholeLines('')).toBeNull();
    expect(wholeLines('2e3')).toBeNull();
    expect(wholeLines('-5')).toBeNull();
    expect(wholeLines('12.5')).toBeNull();
    expect(wholeLines('all')).toBeNull();
  });

  it('does not decide the range of a capture: 0 and 20000 go to the engine, which says no in a sentence', () => {
    expect(wholeLines('0')).toBe(0);
    expect(wholeLines('20000')).toBe(20000);
  });

  it('holds the range of a snapshot itself, because the engine cuts one to fit without saying so', () => {
    // Recorded from a running engine: `terminal.snapshot {lines: 0}` answered
    // one line and `{lines: 20000}` every line it had, neither with a refusal.
    expect(readsAsAsked(0)).toBe(false);
    expect(readsAsAsked(1)).toBe(true);
    expect(readsAsAsked(200)).toBe(true);
    expect(readsAsAsked(10_000)).toBe(true);
    expect(readsAsAsked(10_001)).toBe(false);
    expect(readsAsAsked(20_000)).toBe(false);
  });

  it('takes the last screen to be what the terminal shows at once', () => {
    expect(screenLines(40)).toBe(40);
    expect(screenLines(undefined)).toBe(24);
    expect(screenLines(0)).toBe(24);
    expect(DEFAULT_LINES).toBe(200);
  });
});

describe('a selection', () => {
  it('counts lines as a person would', () => {
    expect(selectionLines('')).toBe(0);
    expect(selectionLines('SW1> show clock')).toBe(1);
    expect(selectionLines('a\nb\n')).toBe(2);
    expect(selectionLines('a\r\nb')).toBe(2);
  });

  it('is named after the session it was made in', () => {
    expect(selectionTitle('SW1-LAB')).toBe('Selection from SW1-LAB');
    expect(selectionTitle(undefined)).toBe('Selection from a terminal session');
  });
});

describe('the last lines as a file', () => {
  it('is named after the session, the lines and the moment', () => {
    expect(snapshotFilename('SW1-LAB', 200, new Date(2026, 10, 23, 3, 5, 9))).toBe(
      'SW1-LAB-last-200-lines-20261123-030509.txt',
    );
  });

  it('ends in one line break, added only when it is missing', () => {
    expect(asFile('SW1>')).toBe('SW1>\n');
    expect(asFile('SW1>\n')).toBe('SW1>\n');
  });
});
