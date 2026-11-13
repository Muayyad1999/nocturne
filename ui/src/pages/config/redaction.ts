/**
 * Finding the places where a secret was taken out.
 *
 * Nocturne redacts a configuration before a person sees it, by default, and
 * puts a stable placeholder where the value was: `<secret:3>`, or
 * `<secret:3:weak>` when the value was a well-known default such as
 * `public`. The placeholder has to read as a placeholder. A screen that drew
 * it as ordinary configuration text would be showing something that looks
 * like a password and is not one, which is worse than showing nothing.
 *
 * The pattern is the backend's (`nocturne_ai::redact`), and redacted text has
 * exactly as many lines as the original, so every line number means the same
 * line either way.
 */

/** Matches one placeholder. Global: use `matchAll`, never `test`, on a shared regex. */
export const SECRET_PATTERN = /<secret:(\d+)(:weak)?>/g;

export interface Redaction {
  /** Character offset of the placeholder in the text. */
  from: number;
  to: number;
  /** The backend's number for this secret: the same value is always the same number. */
  id: number;
  /** The value was a well-known default (`public`, `cisco`, `admin`…). */
  weak: boolean;
}

/** Every placeholder in a piece of text, in the order they appear. */
export function findRedactions(text: string): Redaction[] {
  const found: Redaction[] = [];
  for (const match of text.matchAll(SECRET_PATTERN)) {
    const index = match.index ?? 0;
    found.push({
      from: index,
      to: index + match[0].length,
      id: Number(match[1] ?? 0),
      weak: match[2] !== undefined,
    });
  }
  return found;
}

export interface RedactionSummary {
  /** Placeholders in the text. */
  count: number;
  /** How many different secrets they stand for. */
  distinct: number;
  /** Of those, how many were well-known default values. */
  weak: number;
}

export function summarise(text: string): RedactionSummary {
  const found = findRedactions(text);
  const ids = new Set(found.map((entry) => entry.id));
  const weak = new Set(found.filter((entry) => entry.weak).map((entry) => entry.id));
  return { count: found.length, distinct: ids.size, weak: weak.size };
}

/** True when this one line has had something taken out of it. */
export function hasRedaction(line: string): boolean {
  return line.includes('<secret:');
}

/**
 * What the screen says about a redacted document. `redacted` is the
 * backend's own flag: a document can be redacted and contain no secrets,
 * which is worth saying — it means nothing was hidden from this reader.
 */
export function redactionSentence(redacted: boolean, summary: RedactionSummary): string {
  if (!redacted) {
    return 'Shown with its secrets in place. This view was recorded in the audit trail.';
  }
  if (summary.count === 0) {
    return 'Secrets are hidden by default. This configuration has none that Nocturne recognises, so nothing here is held back.';
  }
  const secrets = summary.distinct === 1 ? '1 secret' : `${summary.distinct} secrets`;
  const places = summary.count === 1 ? '1 place' : `${summary.count} places`;
  const weak =
    summary.weak > 0
      ? ` ${summary.weak === 1 ? 'One of them is' : `${summary.weak} of them are`} a well-known default value.`
      : '';
  return `${secrets} taken out in ${places}. Each marker stands for a value that is not shown — it is not the value.${weak}`;
}
