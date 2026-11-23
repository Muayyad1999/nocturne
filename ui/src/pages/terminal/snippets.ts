/**
 * The arithmetic of the snippets drawer: no React, no engine.
 *
 * What a form's fields become when they are sent as values; which sentence
 * of a refusal belongs beside which field; which rows a session is shown
 * first and which are "the others"; what the tags are; how a send and an
 * import are told to the person.
 *
 * One thing is deliberately *not* here: which variant of a snippet a session
 * gets. That is decided by the engine (`terminal.snippets.list {session_id}`
 * gives every row `chosen`), and this file only reads its answer.
 */

import { RpcError } from '@/api/errors';
import type {
  SessionInfo,
  Snippet,
  SnippetImportResult,
  SnippetNotSent,
  SnippetSendResult,
  SnippetValueProblem,
  SnippetVariable,
  TerminalLineClass,
} from '@/api/types/terminal';
import { plural } from '@/lib/format';

// ------------------------------------------------------------------ values

/** What the form holds: one text per variable, by name. */
export type Fields = Readonly<Record<string, string>>;

/** A variable without a default has to be given a value. */
export function isRequired(variable: SnippetVariable): boolean {
  return variable.default === undefined;
}

/**
 * The form as it opens: a variable with a default starts out holding it, so
 * what will be typed is on screen before anything is entered; one without
 * starts empty.
 */
export function initialFields(variables: readonly SnippetVariable[]): Record<string, string> {
  const fields: Record<string, string> = {};
  for (const variable of variables) fields[variable.name] = variable.default ?? '';
  return fields;
}

/**
 * What is sent as `values`. A field is sent exactly as it stands — nothing
 * is trimmed and nothing is checked here: whether a value may be typed is
 * the engine's to say, and it says it with a sentence.
 *
 * The one case that is not a value is a required variable left empty. It is
 * left out, so the engine answers `missing` for it instead of typing a gap
 * where a port name was meant to be.
 */
export function valuesFor(variables: readonly SnippetVariable[], fields: Fields): Record<string, string> {
  const values: Record<string, string> = {};
  for (const variable of variables) {
    const field = fields[variable.name] ?? '';
    if (field === '' && isRequired(variable)) continue;
    values[variable.name] = field;
  }
  return values;
}

/** The required variables that have nothing in them yet, in the snippet's order. */
export function unfilled(variables: readonly SnippetVariable[], fields: Fields): string[] {
  return variables
    .filter((variable) => isRequired(variable) && (fields[variable.name] ?? '') === '')
    .map((variable) => variable.name);
}

// --------------------------------------------------------------- refusals

export interface ValueRefusal {
  problem: SnippetValueProblem;
  /** The engine's own clause about this variable. */
  sentence: string;
}

export interface RenderRefusal {
  /** The engine's sentence for each variable it faulted, by the variable's name. */
  fields: Record<string, ValueRefusal>;
  /**
   * What is left to say that belongs to no field: a refusal that is not
   * about values at all, or one about a name the form does not have.
   */
  general: string | null;
}

function refusedVariables(details: unknown): { name: string; problem: SnippetValueProblem }[] | null {
  if (typeof details !== 'object' || details === null) return null;
  const variables = (details as { variables?: unknown }).variables;
  if (!Array.isArray(variables)) return null;
  const listed: { name: string; problem: SnippetValueProblem }[] = [];
  for (const entry of variables) {
    if (typeof entry !== 'object' || entry === null) return null;
    const { name, problem } = entry as { name?: unknown; problem?: unknown };
    if (typeof name !== 'string' || typeof problem !== 'string') return null;
    listed.push({ name, problem: problem as SnippetValueProblem });
  }
  return listed;
}

function sentence(text: string): string {
  const trimmed = text.trim();
  if (trimmed === '') return trimmed;
  const capital = trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return /[.!?]$/.test(capital) ? capital : `${capital}.`;
}

/**
 * A refused `terminal.snippets.render` (or `.send`), sorted to the fields
 * it is about.
 *
 * The engine names every faulty variable in `details.variables` and says why
 * in its message: `` `Name` cannot be filled in: `` followed by one clause
 * per variable, in the same order, joined with `; `. When the clauses can be
 * told apart that way each goes beside its field; when they cannot, the
 * whole sentence goes beside every field it names, which is longer but never
 * wrong. Nothing is reworded: what a value may hold is the engine's rule.
 */
export function renderRefusal(error: unknown, variables: readonly SnippetVariable[]): RenderRefusal {
  const message = error instanceof Error && error.message !== '' ? error.message : 'The snippet could not be filled in.';
  const refused = error instanceof RpcError ? refusedVariables(error.details) : null;
  if (!refused || refused.length === 0) return { fields: {}, general: message };

  const after = message.indexOf(': ');
  const clauses = after < 0 ? [] : message.slice(after + 2).split('; ');
  const apart = clauses.length === refused.length;

  const known = new Set(variables.map((variable) => variable.name));
  const fields: Record<string, ValueRefusal> = {};
  const unplaced: string[] = [];
  refused.forEach((entry, index) => {
    const said = sentence(apart ? (clauses[index] ?? message) : message);
    if (known.has(entry.name)) fields[entry.name] = { problem: entry.problem, sentence: said };
    else unplaced.push(said);
  });
  return { fields, general: unplaced.length > 0 ? [...new Set(unplaced)].join(' ') : null };
}

/** The hash a refused send says the snippet renders to now, when it says one. */
export function changedHash(error: unknown): string | null {
  if (!(error instanceof RpcError) || error.code !== 'conflict') return null;
  const details = error.details;
  if (typeof details !== 'object' || details === null) return null;
  const sha256 = (details as { sha256?: unknown }).sha256;
  return typeof sha256 === 'string' ? sha256 : null;
}

/** The sessions a send was refused for before anything was typed, when the refusal lists them. */
export function refusedSessions(error: unknown): SnippetNotSent[] {
  if (!(error instanceof RpcError)) return [];
  const details = error.details;
  if (typeof details !== 'object' || details === null) return [];
  const refused = (details as { refused?: unknown }).refused;
  if (!Array.isArray(refused)) return [];
  return refused.filter(
    (entry): entry is SnippetNotSent =>
      typeof entry === 'object' &&
      entry !== null &&
      typeof (entry as SnippetNotSent).session_id === 'string' &&
      typeof (entry as SnippetNotSent).message === 'string',
  );
}

// ---------------------------------------------------------------- variants

export interface SnippetRows {
  /** What the session is offered: the engine's choice, or everything when no choice was asked for. */
  offered: Snippet[];
  /** Rows the engine did not choose for this session: the other variants. */
  others: Snippet[];
}

/**
 * The rows as the engine chose among them. `chosen: false` is the engine
 * saying "not this one, here"; a row without `chosen` comes from a list that
 * was asked for no session, and is simply a snippet.
 */
export function splitByChoice(rows: readonly Snippet[]): SnippetRows {
  const offered: Snippet[] = [];
  const others: Snippet[] = [];
  for (const row of rows) (row.chosen === false ? others : offered).push(row);
  return { offered, others };
}

/**
 * Why a row is among "the others", in a sentence. The engine gives the
 * verdict and, when another variant stands in, which one; this only puts it
 * into words.
 */
export function notChosenBecause(row: Snippet, all: readonly Snippet[]): string {
  if (row.chosen !== false) return '';
  const winner = row.chosen_instead ? all.find((other) => other.id === row.chosen_instead) : undefined;
  if (winner) {
    return winner.profile
      ? `Not used in this session: the ${winner.profile} variant is.`
      : 'Not used in this session: the variant for any device is.';
  }
  if (row.chosen_instead) return 'Not used in this session: another variant is.';
  return row.profile
    ? `For ${row.profile} devices. This session’s device type is not known, so it is not offered on a guess.`
    : 'Not offered for this session.';
}

/** Who a snippet is for, as the rows show it. */
export function audience(row: Pick<Snippet, 'profile'>): string {
  return row.profile ? row.profile : 'any device';
}

// -------------------------------------------------------------------- tags

export interface TagCount {
  tag: string;
  count: number;
}

/** Every tag on the rows with how many rows carry it, most used first, then by name. */
export function tagCounts(rows: readonly Snippet[]): TagCount[] {
  const counts = new Map<string, number>();
  for (const row of rows) for (const tag of new Set(row.tags)) counts.set(tag, (counts.get(tag) ?? 0) + 1);
  return [...counts.entries()]
    .map(([tag, count]) => ({ tag, count }))
    .sort((left, right) => right.count - left.count || left.tag.localeCompare(right.tag));
}

/** The rows that carry every one of the tags; all of them when no tag is picked. */
export function withTags(rows: readonly Snippet[], tags: readonly string[]): Snippet[] {
  if (tags.length === 0) return [...rows];
  return rows.filter((row) => tags.every((tag) => row.tags.includes(tag)));
}

/** Adds the tag to the filter, or takes it out. */
export function toggleTag(tags: readonly string[], tag: string): string[] {
  return tags.includes(tag) ? tags.filter((other) => other !== tag) : [...tags, tag];
}

// ----------------------------------------------------------------- sending

/** A session a snippet may be sent to. */
export interface SendTarget {
  sessionId: string;
  title: string;
  /** The inventory device whose policy judges the lines; the generic policy without one. */
  deviceId: string | null;
}

/** The sessions that can take a snippet right now: the open ones, in the engine's order. */
export function openTargets(sessions: readonly SessionInfo[]): SendTarget[] {
  return sessions
    .filter((session) => session.state === 'open')
    .map((session) => ({ sessionId: session.id, title: session.title, deviceId: session.device_id ?? null }));
}

/**
 * Which sessions are ticked when the send form opens: the one in front —
 * or, while typing is being broadcast and it is one of the targets, the
 * sessions the keyboard goes to anyway. Only sessions that are open.
 */
export function initialTargets(
  open: readonly SendTarget[],
  focusedId: string | null,
  broadcast: readonly string[],
): string[] {
  const available = new Set(open.map((target) => target.sessionId));
  if (focusedId && broadcast.length > 1 && broadcast.includes(focusedId)) {
    return broadcast.filter((id) => available.has(id));
  }
  return focusedId && available.has(focusedId) ? [focusedId] : [];
}

const RANK: Record<TerminalLineClass['class'], number> = { read_only: 0, change: 1, dangerous: 2 };

/**
 * What several devices' policies think of the same lines, as one verdict
 * per line: the worst of them, with its reason. A line that only reads on
 * one device and reloads another is a line that reloads.
 */
export function worstClasses(perDevice: readonly (readonly TerminalLineClass[])[]): TerminalLineClass[] {
  const [first, ...rest] = perDevice;
  if (!first) return [];
  return first.map((line, index) => {
    let worst = line;
    for (const other of rest) {
      const candidate = other[index];
      if (candidate && RANK[candidate.class] > RANK[worst.class]) worst = candidate;
    }
    return worst;
  });
}

export interface SendReport {
  /** Whether every session that was asked for took the text. */
  complete: boolean;
  /** One line for a toast. */
  headline: string;
  /** Session by session: what it took, or why it did not. */
  sessions: { sessionId: string; title: string; ok: boolean; says: string }[];
}

/**
 * A send as it is told to the person: who got the text and who did not.
 * What a session received cannot be taken back, so a partial send names
 * both.
 */
export function sendReport(result: SnippetSendResult, targets: readonly SendTarget[]): SendReport {
  const title = (id: string) => targets.find((target) => target.sessionId === id)?.title ?? id;
  const sessions = [
    ...result.sent.map((entry) => ({
      sessionId: entry.session_id,
      title: title(entry.session_id),
      ok: true,
      says: `Typed ${plural(entry.lines, 'line')}; the Enter is yours.`,
    })),
    ...result.failed.map((entry) => ({
      sessionId: entry.session_id,
      title: title(entry.session_id),
      ok: false,
      says: entry.message,
    })),
  ];
  const sent = result.sent.length;
  const failed = result.failed.length;
  const first = result.sent[0];
  let headline: string;
  if (failed === 0 && sent === 1 && first) headline = `Typed into ${title(first.session_id)}`;
  else if (failed === 0) headline = `Typed into ${plural(sent, 'session')}`;
  else if (sent === 0) headline = `Not typed into ${failed === 1 ? 'the session' : `any of ${failed} sessions`}`;
  else headline = `Typed into ${sent} of ${sent + failed} sessions`;
  return { complete: failed === 0 && sent > 0, headline, sessions };
}

// ---------------------------------------------------------- import, export

/** What an import would do, or did, in one sentence. */
export function importSummary(result: SnippetImportResult): string {
  if (result.problems.length > 0) {
    return `${plural(result.problems.length, 'entry', 'entries')} cannot be imported, so nothing ${result.dry_run ? 'would be' : 'was'}: an import is the whole document or none of it.`;
  }
  const would = result.dry_run;
  const parts: string[] = [];
  if (result.created > 0) {
    const renamed = result.renamed > 0 ? ` (${result.renamed} under a new name)` : '';
    parts.push(`${plural(result.created, 'snippet')} ${would ? 'would be' : 'were'} added${renamed}`);
  }
  if (result.replaced > 0) parts.push(`${result.replaced} ${would ? 'would be' : 'were'} replaced`);
  if (result.skipped > 0) parts.push(`${result.skipped} ${would ? 'would be' : 'were'} left as they are here`);
  if (parts.length === 0) return would ? 'Nothing would change.' : 'Nothing changed.';
  return `${parts.join(', ')}.`.replace(/^./, (first) => first.toUpperCase());
}

/** Whether an import may be applied after this dry run: it was one, and it found nothing wrong. */
export function mayApply(preview: SnippetImportResult | null): boolean {
  return preview !== null && preview.dry_run && preview.problems.length === 0;
}

/** `nocturne-snippets-20261123.json`, by the local date. */
export function exportFilename(now: Date = new Date()): string {
  const two = (value: number) => String(value).padStart(2, '0');
  return `nocturne-snippets-${now.getFullYear()}${two(now.getMonth() + 1)}${two(now.getDate())}.json`;
}

/** The lines a row's preview counts: what the text holds, without the line break that ends it. */
export function lineCount(text: string): number {
  return text.replace(/\r\n?/g, '\n').replace(/\n$/, '').split('\n').length;
}
