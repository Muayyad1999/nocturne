import { describe, expect, it } from 'vitest';
import { diffLines, diffStats, groupDiff, splitLines, type DiffLine } from './diff';

const summary = (lines: DiffLine[]) =>
  lines.map((line) => `${{ same: ' ', add: '+', remove: '-' }[line.kind]}${line.text}`);

/** Applies a diff forwards and backwards: the strongest check that it is a real diff. */
function reconstruct(lines: DiffLine[]) {
  return {
    before: lines.filter((line) => line.kind !== 'add').map((line) => line.text),
    after: lines.filter((line) => line.kind !== 'remove').map((line) => line.text),
  };
}

describe('splitLines', () => {
  it('ignores one trailing newline and handles CRLF', () => {
    expect(splitLines('')).toEqual([]);
    expect(splitLines('a\nb\n')).toEqual(['a', 'b']);
    expect(splitLines('a\r\nb')).toEqual(['a', 'b']);
    expect(splitLines('a\n\n')).toEqual(['a', '']);
  });
});

describe('diffLines', () => {
  it('reports identical texts as unchanged', () => {
    const lines = diffLines('a\nb\nc', 'a\nb\nc');
    expect(summary(lines)).toEqual([' a', ' b', ' c']);
    expect(diffStats(lines)).toEqual({ added: 0, removed: 0 });
  });

  it('finds a changed line and numbers both sides', () => {
    const before = 'interface Gi0/1\n description uplink\n switchport mode access\n!';
    const after = 'interface Gi0/1\n description uplink to core\n switchport mode access\n!';
    const lines = diffLines(before, after);
    expect(summary(lines)).toEqual([
      ' interface Gi0/1',
      '- description uplink',
      '+ description uplink to core',
      '  switchport mode access',
      ' !',
    ]);
    expect(lines.map((line) => [line.oldNumber, line.newNumber])).toEqual([
      [1, 1],
      [2, null],
      [null, 2],
      [3, 3],
      [4, 4],
    ]);
    expect(diffStats(lines)).toEqual({ added: 1, removed: 1 });
  });

  it('handles insertions and deletions at the edges', () => {
    expect(summary(diffLines('b\nc', 'a\nb\nc'))).toEqual(['+a', ' b', ' c']);
    expect(summary(diffLines('a\nb\nc', 'a\nb'))).toEqual([' a', ' b', '-c']);
    expect(summary(diffLines('', 'a\nb'))).toEqual(['+a', '+b']);
    expect(summary(diffLines('a\nb', ''))).toEqual(['-a', '-b']);
    expect(diffLines('', '')).toEqual([]);
  });

  it('produces a minimal script', () => {
    // Classic Myers example: ABCABBA → CBABAC has an edit distance of 5.
    const lines = diffLines('A\nB\nC\nA\nB\nB\nA', 'C\nB\nA\nB\nA\nC');
    const { added, removed } = diffStats(lines);
    expect(added + removed).toBe(5);
    expect(reconstruct(lines)).toEqual({
      before: ['A', 'B', 'C', 'A', 'B', 'B', 'A'],
      after: ['C', 'B', 'A', 'B', 'A', 'C'],
    });
  });

  it('always reconstructs both texts', () => {
    // Deterministic pseudo-random edits over a config-like text.
    let seed = 7;
    const random = () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed / 0x7fffffff;
    };
    for (let round = 0; round < 40; round++) {
      const before = Array.from({ length: 60 }, (_, i) => `line ${i % 17}`);
      const after = before.flatMap((line) => {
        const roll = random();
        if (roll < 0.1) return [];
        if (roll < 0.2) return [line, `added ${Math.floor(random() * 5)}`];
        if (roll < 0.25) return [`changed ${line}`];
        return [line];
      });
      const lines = diffLines(before.join('\n'), after.join('\n'));
      expect(reconstruct(lines)).toEqual({ before, after });
    }
  });

  it('falls back to a replaced block for unrelated texts beyond the distance limit', () => {
    const before = Array.from({ length: 1500 }, (_, i) => `old ${i}`);
    const after = Array.from({ length: 1500 }, (_, i) => `new ${i}`);
    const lines = diffLines(before.join('\n'), after.join('\n'));
    expect(diffStats(lines)).toEqual({ added: 1500, removed: 1500 });
    expect(reconstruct(lines)).toEqual({ before, after });
  });
});

describe('groupDiff', () => {
  const numbered = (count: number) => Array.from({ length: count }, (_, i) => `l${i + 1}`);

  it('folds long unchanged runs and keeps context around changes', () => {
    const before = numbered(30);
    const after = [...before];
    after[14] = 'changed';
    const blocks = groupDiff(diffLines(before.join('\n'), after.join('\n')), 3);
    expect(blocks.map((block) => [block.kind, block.lines.length])).toEqual([
      ['collapsed', 11],
      ['lines', 8], // 3 before + removed + added + 3 after
      ['collapsed', 12],
    ]);
    const visible = blocks[1];
    expect(visible?.lines[0]?.text).toBe('l12');
    expect(visible?.lines.at(-1)?.text).toBe('l18');
  });

  it('does not fold a run that would hide only a few lines', () => {
    const before = numbered(12);
    const after = [...before];
    after[0] = 'first';
    after[11] = 'last';
    const blocks = groupDiff(diffLines(before.join('\n'), after.join('\n')), 3, 5);
    expect(blocks).toHaveLength(1);
    expect(blocks[0]?.kind).toBe('lines');
  });

  it('keeps every line exactly once', () => {
    const before = numbered(50);
    const after = before.filter((_, i) => i !== 5 && i !== 40);
    const lines = diffLines(before.join('\n'), after.join('\n'));
    const regrouped = groupDiff(lines).flatMap((block) => block.lines);
    expect(regrouped).toEqual(lines);
  });

  it('returns nothing for an empty diff', () => {
    expect(groupDiff([])).toEqual([]);
  });
});
