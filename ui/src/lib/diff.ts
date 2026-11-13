/**
 * Line diff for configuration text: what DiffView renders.
 *
 * `diffLines` is Myers' O(ND) algorithm over lines, after trimming the common
 * prefix and suffix (config changes are usually a few lines in a large file).
 * `groupDiff` then folds long unchanged runs so a reviewer sees the changes
 * with a few lines of context around each.
 */

export type DiffLineKind = 'same' | 'add' | 'remove';

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
  /** 1-based line number in the old text; null for added lines. */
  oldNumber: number | null;
  /** 1-based line number in the new text; null for removed lines. */
  newNumber: number | null;
}

export interface DiffStats {
  added: number;
  removed: number;
}

/**
 * Beyond this edit distance the two texts are treated as unrelated and shown
 * as one replaced block, which keeps memory bounded on pathological input.
 */
const MAX_EDIT_DISTANCE = 2000;

/** Splits text into lines. A trailing newline does not produce an empty last line. */
export function splitLines(text: string): string[] {
  if (text === '') return [];
  const lines = text.split(/\r\n|\n|\r/);
  if (lines[lines.length - 1] === '') lines.pop();
  return lines;
}

type Edit = 'same' | 'add' | 'remove';

/** Shortest edit script between `a` and `b`, or null if it exceeds the distance limit. */
function myers(a: readonly string[], b: readonly string[]): Edit[] | null {
  const n = a.length;
  const m = b.length;
  const max = Math.min(n + m, MAX_EDIT_DISTANCE);
  const offset = max;
  let v = new Int32Array(2 * max + 2);
  const trace: Int32Array[] = [];

  let found = -1;
  search: for (let d = 0; d <= max; d++) {
    trace.push(v);
    const next = v.slice();
    for (let k = -d; k <= d; k += 2) {
      const down = k === -d || (k !== d && (v[offset + k - 1] ?? 0) < (v[offset + k + 1] ?? 0));
      let x = down ? (v[offset + k + 1] ?? 0) : (v[offset + k - 1] ?? 0) + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      next[offset + k] = x;
      if (x >= n && y >= m) {
        trace.push(next);
        found = d;
        break search;
      }
    }
    v = next;
  }
  if (found < 0) return null;

  // Walk the trace backwards from (n, m) to recover the script.
  const edits: Edit[] = [];
  let x = n;
  let y = m;
  for (let d = found; d > 0; d--) {
    const previous = trace[d];
    if (!previous) break;
    const k = x - y;
    const down =
      k === -d || (k !== d && (previous[offset + k - 1] ?? 0) < (previous[offset + k + 1] ?? 0));
    const previousK = down ? k + 1 : k - 1;
    const previousX = previous[offset + previousK] ?? 0;
    const previousY = previousX - previousK;
    while (x > previousX && y > previousY) {
      edits.push('same');
      x--;
      y--;
    }
    edits.push(down ? 'add' : 'remove');
    x = previousX;
    y = previousY;
  }
  while (x > 0 && y > 0) {
    edits.push('same');
    x--;
    y--;
  }
  return edits.reverse();
}

/** Line-by-line difference between two texts, in display order. */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = splitLines(before);
  const b = splitLines(after);

  let prefix = 0;
  while (prefix < a.length && prefix < b.length && a[prefix] === b[prefix]) prefix++;
  let suffix = 0;
  while (
    suffix < a.length - prefix &&
    suffix < b.length - prefix &&
    a[a.length - 1 - suffix] === b[b.length - 1 - suffix]
  ) {
    suffix++;
  }

  const middleA = a.slice(prefix, a.length - suffix);
  const middleB = b.slice(prefix, b.length - suffix);
  const edits: Edit[] = myers(middleA, middleB) ?? [
    ...middleA.map((): Edit => 'remove'),
    ...middleB.map((): Edit => 'add'),
  ];

  const lines: DiffLine[] = [];
  let oldIndex = 0;
  let newIndex = 0;
  const same = () => {
    lines.push({
      kind: 'same',
      text: a[oldIndex] ?? '',
      oldNumber: ++oldIndex,
      newNumber: ++newIndex,
    });
  };

  for (let i = 0; i < prefix; i++) same();
  for (const edit of edits) {
    if (edit === 'same') same();
    else if (edit === 'remove') {
      lines.push({ kind: 'remove', text: a[oldIndex] ?? '', oldNumber: ++oldIndex, newNumber: null });
    } else {
      lines.push({ kind: 'add', text: b[newIndex] ?? '', oldNumber: null, newNumber: ++newIndex });
    }
  }
  for (let i = 0; i < suffix; i++) same();
  return lines;
}

export function diffStats(lines: readonly DiffLine[]): DiffStats {
  let added = 0;
  let removed = 0;
  for (const line of lines) {
    if (line.kind === 'add') added++;
    else if (line.kind === 'remove') removed++;
  }
  return { added, removed };
}

export type DiffBlock =
  | { kind: 'lines'; lines: DiffLine[] }
  /** A run of unchanged lines hidden behind an expander. */
  | { kind: 'collapsed'; lines: DiffLine[] };

/**
 * Folds unchanged runs, keeping `context` lines around every change. A run is
 * only folded when that hides at least `minCollapsed` lines; hiding one or two
 * lines behind a button would cost more attention than showing them.
 */
export function groupDiff(lines: readonly DiffLine[], context = 3, minCollapsed = 4): DiffBlock[] {
  const blocks: DiffBlock[] = [];
  let visible: DiffLine[] = [];
  const flush = () => {
    if (visible.length > 0) blocks.push({ kind: 'lines', lines: visible });
    visible = [];
  };

  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (!line) break;
    if (line.kind !== 'same') {
      visible.push(line);
      index++;
      continue;
    }
    let end = index;
    while (lines[end]?.kind === 'same') end++;
    const run = lines.slice(index, end);
    const head = index === 0 ? 0 : context;
    const tail = end === lines.length ? 0 : context;
    if (run.length - head - tail >= minCollapsed) {
      visible.push(...run.slice(0, head));
      flush();
      blocks.push({ kind: 'collapsed', lines: run.slice(head, run.length - tail) });
      visible.push(...run.slice(run.length - tail));
    } else {
      visible.push(...run);
    }
    index = end;
  }
  flush();
  return blocks;
}
