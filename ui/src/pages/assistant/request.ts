/**
 * An approval's `detail`, as the lines the engine wrote.
 *
 * The engine builds a question with a fixed number of lines, and flattens
 * whatever a device or a person wrote onto one of them, so that nothing in an
 * alert's title or a note can add a line. A narrow frame must not undo that by
 * wrapping: the card draws one block per line here, numbered, and a wrapped
 * continuation hangs deeper than the line's own text with no number of its
 * own. Only a line break the engine wrote makes a line.
 *
 * Word for word includes what does not show. A control character, a line or
 * paragraph separator, and a direction mark or override change how a line
 * reads without being seen: an override makes the rest of its line read
 * backwards, so "show running-config" can be what the eye takes from a
 * command that is something else. The engine keeps them out of what a device
 * or a person wrote into a question about a write, but a read's one line of
 * arguments (`confirm_reads`) and every step's summary of its arguments are
 * the model's text, and keep them. So each is written out where it stands,
 * as its code point, in a mark the eye can tell from typed text.
 */

/** A stretch of a line: text as it was written, or one character that does not show, by its code point. */
export type Run = { text: string } | { unseen: string };

export interface RequestLine {
  /** The line exactly as the engine wrote it. */
  text: string;
  /** The same line to draw, with every character that does not show written out. */
  runs: Run[];
  /** Leading spaces: the engine's indentation, which a continuation hangs below. */
  lead: number;
}

/** Deeper than any indentation the engine writes, and no deeper: a line of spaces cannot push its own text away. */
export const MAX_LEAD = 8;

/**
 * What the engine keeps out of a question's quoted text: the C0 and C1
 * controls (a line feed only ever separates lines here), the line and
 * paragraph separators, and the direction marks, embeddings, overrides and
 * isolates (`breaks_or_reorders_a_line` in the engine's `tools/records.rs`).
 * The joiners stay: they shape scripts that need them and reorder nothing.
 */
const UNSEEN = /[\u0000-\u0009\u000B-\u001F\u007F-\u009F؜‎‏  ‪-‮⁦-⁩]/gu;

/** `U+202E` for an override: four hex digits at least, upper case, as code charts write them. */
function codePoint(character: string): string {
  return `U+${(character.codePointAt(0) ?? 0).toString(16).toUpperCase().padStart(4, '0')}`;
}

/** `text` as stretches to draw: what shows as it is, and each character that does not, written out. */
export function runsOf(text: string): Run[] {
  const runs: Run[] = [];
  let from = 0;
  for (const match of text.matchAll(UNSEEN)) {
    const at = match.index ?? 0;
    if (at > from) runs.push({ text: text.slice(from, at) });
    runs.push({ unseen: codePoint(match[0]) });
    from = at + match[0].length;
  }
  if (from < text.length) runs.push({ text: text.slice(from) });
  return runs;
}

export function requestLines(detail: string): RequestLine[] {
  if (detail === '') return [];
  return detail.split('\n').map((text) => ({
    text,
    runs: runsOf(text),
    lead: Math.min(text.length - text.replace(/^ +/, '').length, MAX_LEAD),
  }));
}
