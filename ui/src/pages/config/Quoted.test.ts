import { describe, expect, it } from 'vitest';
import { splitQuoted } from './Quoted';

describe('splitQuoted', () => {
  it('picks the backticked spans out of a sentence', () => {
    expect(splitQuoted('Checked: the global line `ip ssh version 2`.')).toEqual([
      { text: 'Checked: the global line ', code: false },
      { text: 'ip ssh version 2', code: true },
      { text: '.', code: false },
    ]);
  });

  it('leaves an unclosed backtick alone rather than swallowing the rest', () => {
    expect(splitQuoted('a ` b')).toEqual([{ text: 'a ` b', code: false }]);
  });

  it('handles several spans and loses nothing', () => {
    const text = '`a` and `b` and more';
    expect(splitQuoted(text).map((part) => part.text).join('')).toBe('a and b and more');
    expect(splitQuoted(text).filter((part) => part.code)).toHaveLength(2);
  });

  it('is fine with text that has no quotes at all', () => {
    expect(splitQuoted('plain words')).toEqual([{ text: 'plain words', code: false }]);
  });
});
