/**
 * Fuzzy matching for the command palette and filterable lists.
 *
 * A query matches when its characters appear in order in the text
 * (case-insensitive). Matches score higher when they are consecutive, start a
 * word, or start the text, so `dev` ranks "Devices" above "Lab devices" and
 * `gd` still finds "Go to Devices". Queries with several words must match
 * every word.
 */

export interface FuzzyMatch {
  /** Higher is better. Only comparable between matches of the same query. */
  score: number;
  /** Indices of the matched characters in the text, ascending: for highlighting. */
  indices: number[];
}

const SCORE_MATCH = 16;
const BONUS_TEXT_START = 12;
const BONUS_WORD_START = 10;
const BONUS_CAMEL = 6;
const BONUS_CONSECUTIVE = 14;
const PENALTY_GAP = 1;
const PENALTY_LEADING = 0.5;
const MAX_LEADING_PENALTY = 6;

function isWordSeparator(char: string): boolean {
  return /[\s\-_./:\\()[\]]/.test(char);
}

function boundaryBonus(text: string, index: number): number {
  if (index === 0) return BONUS_TEXT_START;
  const previous = text[index - 1] ?? '';
  const current = text[index] ?? '';
  if (isWordSeparator(previous)) return BONUS_WORD_START;
  if (previous === previous.toLowerCase() && current !== current.toLowerCase()) return BONUS_CAMEL;
  return 0;
}

/** Best in-order match of a single word in `text`, or null. */
function matchWord(word: string, text: string): FuzzyMatch | null {
  const n = word.length;
  const m = text.length;
  if (n === 0) return { score: 0, indices: [] };
  if (n > m) return null;

  const lowerText = text.toLowerCase();
  // best[i][j]: best score with word[i] matched exactly at text[j].
  const best: Float64Array[] = [];
  // from[i][j]: text index where word[i - 1] was matched on that best path.
  const from: Int32Array[] = [];

  for (let i = 0; i < n; i++) {
    const row = new Float64Array(m).fill(Number.NEGATIVE_INFINITY);
    const back = new Int32Array(m).fill(-1);
    const char = word[i];
    const previousRow = best[i - 1];

    // Running best of previousRow[k] + gap penalty for k < j - 1.
    let carry = Number.NEGATIVE_INFINITY;
    let carryIndex = -1;

    for (let j = i; j < m; j++) {
      if (previousRow && j >= 2) {
        const candidate = previousRow[j - 2] ?? Number.NEGATIVE_INFINITY;
        if (candidate > carry) {
          carry = candidate;
          carryIndex = j - 2;
        }
      }
      if (carry > Number.NEGATIVE_INFINITY) carry -= PENALTY_GAP;

      if (lowerText[j] !== char) continue;
      const own = SCORE_MATCH + boundaryBonus(text, j);

      if (!previousRow) {
        row[j] = own - Math.min(j * PENALTY_LEADING, MAX_LEADING_PENALTY);
        continue;
      }
      const adjacent = (previousRow[j - 1] ?? Number.NEGATIVE_INFINITY) + BONUS_CONSECUTIVE;
      if (adjacent >= carry) {
        row[j] = adjacent + own;
        back[j] = j - 1;
      } else {
        row[j] = carry + own;
        back[j] = carryIndex;
      }
    }
    best.push(row);
    from.push(back);
  }

  const lastRow = best[n - 1];
  if (!lastRow) return null;
  let end = -1;
  let score = Number.NEGATIVE_INFINITY;
  for (let j = n - 1; j < m; j++) {
    const value = lastRow[j] ?? Number.NEGATIVE_INFINITY;
    if (value > score) {
      score = value;
      end = j;
    }
  }
  if (end < 0) return null;

  const indices = new Array<number>(n);
  let j = end;
  for (let i = n - 1; i >= 0; i--) {
    indices[i] = j;
    j = from[i]?.[j] ?? -1;
  }
  return { score, indices };
}

/**
 * Matches `query` against `text`. Returns null when any word of the query is
 * not found. An empty query matches everything with score 0.
 */
export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  let score = 0;
  const indices = new Set<number>();
  for (const word of words) {
    const match = matchWord(word, text);
    if (!match) return null;
    score += match.score;
    for (const index of match.indices) indices.add(index);
  }
  // Prefer shorter texts among equally good matches ("Lab" over "Lab devices").
  score -= text.length * 0.05;
  return { score, indices: [...indices].sort((a, b) => a - b) };
}

export interface FuzzyResult<T> {
  item: T;
  score: number;
  /** Matched character indices in the item's primary text. Empty if only keywords matched. */
  indices: number[];
}

export interface FuzzyOptions<T> {
  /** The text that is shown and highlighted. */
  text: (item: T) => string;
  /** Extra words that also find the item, at a lower rank. */
  keywords?: (item: T) => readonly string[] | undefined;
}

const KEYWORD_WEIGHT = 0.6;

/**
 * Filters and ranks `items` by `query`. With an empty query every item is
 * returned in its original order.
 */
export function fuzzyFilter<T>(
  items: readonly T[],
  query: string,
  { text, keywords }: FuzzyOptions<T>,
): FuzzyResult<T>[] {
  if (!query.trim()) return items.map((item) => ({ item, score: 0, indices: [] }));

  const results: (FuzzyResult<T> & { order: number })[] = [];
  items.forEach((item, order) => {
    const primary = fuzzyMatch(query, text(item));
    let score = primary?.score ?? Number.NEGATIVE_INFINITY;
    for (const keyword of keywords?.(item) ?? []) {
      const match = fuzzyMatch(query, keyword);
      if (match) score = Math.max(score, match.score * KEYWORD_WEIGHT);
    }
    if (score > Number.NEGATIVE_INFINITY) {
      results.push({ item, score, indices: primary?.indices ?? [], order });
    }
  });
  results.sort((a, b) => b.score - a.score || a.order - b.order);
  return results.map(({ item, score, indices }) => ({ item, score, indices }));
}
