/**
 * Searching inside one configuration, in the browser.
 *
 * The network-wide search is the backend's (`config.search`) — it is the one
 * that has to look at every device. This is the other kind: the text is
 * already here, on screen, and the engineer wants the next occurrence of
 * `Vlan30` in it. Doing that locally is instant and needs no round trip.
 */

export interface Range {
  from: number;
  to: number;
}

export interface FindResult {
  ranges: Range[];
  /** The regular expression could not be read; the engine's words would be nicer but this is local. */
  error?: string;
}

export interface FindOptions {
  regex?: boolean;
  caseSensitive?: boolean;
}

const MAX_MATCHES = 5_000;

/**
 * Every place `query` occurs in `text`. An empty query matches nothing (not
 * everything), and a pattern that can match an empty string is advanced by
 * hand so it cannot loop for ever.
 */
export function findInText(text: string, query: string, options: FindOptions = {}): FindResult {
  if (query === '') return { ranges: [] };

  let pattern: RegExp;
  try {
    const source = options.regex ? query : query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    pattern = new RegExp(source, options.caseSensitive ? 'g' : 'gi');
  } catch (error) {
    return { ranges: [], error: error instanceof Error ? error.message : 'Not a valid regular expression' };
  }

  const ranges: Range[] = [];
  let at = 0;
  while (at <= text.length && ranges.length < MAX_MATCHES) {
    pattern.lastIndex = at;
    const match = pattern.exec(text);
    if (!match) break;
    const from = match.index;
    const to = from + match[0].length;
    if (to > from) ranges.push({ from, to });
    at = to > from ? to : from + 1;
  }
  return { ranges };
}

/** Which match is current, counted from 1, for "3 of 17". */
export function matchAt(ranges: readonly Range[], index: number): Range | undefined {
  if (ranges.length === 0) return undefined;
  const wrapped = ((index % ranges.length) + ranges.length) % ranges.length;
  return ranges[wrapped];
}

/**
 * Splits a line into the parts a query matched and the parts it did not, for
 * rendering a search result without `dangerouslySetInnerHTML`.
 */
export function splitMatches(
  line: string,
  query: string,
  options: FindOptions = {},
): { text: string; match: boolean }[] {
  const { ranges } = findInText(line, query, options);
  if (ranges.length === 0) return [{ text: line, match: false }];
  const parts: { text: string; match: boolean }[] = [];
  let cursor = 0;
  for (const range of ranges) {
    if (range.from > cursor) parts.push({ text: line.slice(cursor, range.from), match: false });
    parts.push({ text: line.slice(range.from, range.to), match: true });
    cursor = range.to;
  }
  if (cursor < line.length) parts.push({ text: line.slice(cursor), match: false });
  return parts;
}
