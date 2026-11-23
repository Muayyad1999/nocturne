import { describe, expect, test } from 'vitest';
import { spansLines } from './values';

describe('which values may span lines', () => {
  test('only a string that says so', () => {
    expect(spansLines({ kind: 'string', multiline: true })).toBe(true);
    expect(spansLines({ kind: 'string', multiline: false })).toBe(false);
    // Left out of a declaration, it means no.
    expect(spansLines({ kind: 'string' })).toBe(false);
  });

  test('a flag left on a variable that is no longer a string means nothing', () => {
    // The backend reads `multiline` only for a `string`; neither does the form.
    for (const kind of ['interface', 'list', 'choice', 'int'] as const) {
      expect(spansLines({ kind, multiline: true })).toBe(false);
    }
  });
});
