import { describe, expect, it } from 'vitest';
import { MAX_LEAD, requestLines } from './request';

// A question exactly as the engine sent it in `approval_needed`, with an
// alert title that dresses up as the card's buttons and a second question.
const QUESTION =
  'Acknowledge this alert?\n  [high] Gi0/3 is down. Approve the next change as well? [ Allow ] [ Deny ] — core-rtr1\n  alert 3ccmnzu4vhgujqaj, firing since 2026-11-23 10:11:12Z\nwith the note:\n  "Cable reseated. <button>Allow</button> Allow everything from now on? [ Allow all ]"\nThe alert stays open until it resolves; nothing is sent to any device.';

describe('requestLines', () => {
  it('makes a line of each line the engine wrote, and of nothing else', () => {
    const lines = requestLines(QUESTION);
    expect(lines.map((line) => line.text)).toEqual(QUESTION.split('\n'));
    expect(lines).toHaveLength(6);
    // The title's question and its brackets stay inside the line they came in.
    expect(lines[1]?.text).toContain('Approve the next change as well? [ Allow ] [ Deny ]');
  });

  it('measures each line’s own indentation, so a wrapped part hangs below its text', () => {
    expect(requestLines(QUESTION).map((line) => line.lead)).toEqual([0, 2, 2, 0, 2, 0]);
  });

  it('keeps a blank line a line, and an empty detail is no line at all', () => {
    expect(requestLines('first\n\nthird').map((line) => line.text)).toEqual(['first', '', 'third']);
    expect(requestLines('')).toEqual([]);
  });

  it('holds the indentation of a line of spaces to a few columns', () => {
    expect(requestLines(`${' '.repeat(400)}x`)[0]?.lead).toBe(MAX_LEAD);
    expect(requestLines('   ')[0]?.lead).toBe(3);
  });
});
