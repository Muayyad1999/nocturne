import { describe, expect, it } from 'vitest';
import { fuzzyFilter, fuzzyMatch } from './fuzzy';

const titles = (query: string, items: string[]) =>
  fuzzyFilter(items, query, { text: (item) => item }).map((result) => result.item);

describe('fuzzyMatch', () => {
  it('matches characters in order, ignoring case', () => {
    expect(fuzzyMatch('dvc', 'Devices')?.indices).toEqual([0, 2, 4]);
    expect(fuzzyMatch('DEV', 'devices')?.indices).toEqual([0, 1, 2]);
  });

  it('returns null when a character is missing or out of order', () => {
    expect(fuzzyMatch('dx', 'Devices')).toBeNull();
    expect(fuzzyMatch('ved', 'Devices')).toBeNull();
    expect(fuzzyMatch('devicesx', 'Devices')).toBeNull();
  });

  it('matches everything for an empty query', () => {
    expect(fuzzyMatch('', 'Devices')?.indices).toEqual([]);
    expect(fuzzyMatch('   ', 'Devices')?.indices).toEqual([]);
  });

  it('prefers word starts over letters inside a word', () => {
    // "gd" should light up the G of Go and the D of Devices, not a later "d".
    expect(fuzzyMatch('gd', 'Go to Devices')?.indices).toEqual([0, 6]);
    expect(fuzzyMatch('tt', 'Toggle theme')?.indices).toEqual([0, 7]);
  });

  it('prefers a consecutive run to scattered letters', () => {
    expect(fuzzyMatch('con', 'Open config console')?.indices).toEqual([5, 6, 7]);
  });

  it('requires every word of a multi-word query', () => {
    expect(fuzzyMatch('open term', 'Open terminal')?.indices).toEqual([0, 1, 2, 3, 5, 6, 7, 8]);
    expect(fuzzyMatch('open lab', 'Open terminal')).toBeNull();
  });

  it('recognises camelCase humps as word starts', () => {
    expect(fuzzyMatch('sv', 'showVersion')?.indices).toEqual([0, 4]);
  });
});

describe('fuzzyFilter', () => {
  const items = ['Lab devices', 'Devices', 'Go to Devices', 'Overview', 'Dev components'];

  it('keeps the original order for an empty query', () => {
    expect(titles('', items)).toEqual(items);
  });

  it('ranks prefix matches first', () => {
    const ranked = titles('dev', items);
    expect(ranked[0]).toBe('Devices');
    expect(ranked).not.toContain('Overview');
    expect(ranked.indexOf('Dev components')).toBeLessThan(ranked.indexOf('Lab devices'));
  });

  it('finds items by keyword at a lower rank', () => {
    const commands = [
      { title: 'Toggle assistant', keywords: ['ai', 'chat', 'dock'] },
      { title: 'Chat history', keywords: [] },
    ];
    const results = fuzzyFilter(commands, 'chat', {
      text: (command) => command.title,
      keywords: (command) => command.keywords,
    });
    expect(results.map((result) => result.item.title)).toEqual(['Chat history', 'Toggle assistant']);
    expect(results[1]?.indices).toEqual([]);
  });

  it('is stable for equal scores', () => {
    expect(titles('a', ['alpha', 'alpha'])).toEqual(['alpha', 'alpha']);
  });
});
