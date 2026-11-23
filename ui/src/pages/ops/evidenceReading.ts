/**
 * Evidence, reasoned about with no React in it.
 *
 * An evidence item is something the engine read — what a device printed, the
 * map as it was drawn, the samples of a graph — kept with where it came from,
 * when, and a hash of what is stored. Three things about showing one can go
 * wrong in a way nobody would notice, so they live here as plain functions
 * with tests rather than inside a component:
 *
 * 1. **A drawing is hostile input, and there is exactly one way it reaches
 *    the screen.** A stored SVG is not sanitised, by design: it is the record
 *    of what `topology.export` returned. {@link svgImageSrc} turns the string
 *    into a `data:` address for an `<img>`, where a browser runs no script,
 *    follows no link and fetches nothing — and nothing else in this folder
 *    takes the string anywhere a document is parsed. `lib/capture` says why
 *    `data:` and not `blob:`.
 * 2. **Text is shown as it was printed.** A body can hold an escape sequence,
 *    a backspace or a direction override that would make a line *read* as
 *    something else. {@link visibleText} shows each as a picture of itself,
 *    by the engine's own rule, and the text goes into a `<pre>` as text —
 *    never into the terminal emulator, which would obey it.
 * 3. **A chart is re-drawn with its holes.** The stored series hold only the
 *    buckets that had a sample. {@link storedChart} puts them on one axis and
 *    marks where nothing was recorded, so the line lifts instead of joining
 *    two samples across an hour nobody measured; {@link coverageLines} then
 *    says, in words, how much of the period each series covers.
 *
 * And one about not knowing: an unread page of the list is not "no more
 * evidence". {@link listCoverage} says what is on screen and whether older
 * items exist, because the engine gives a cursor, not a total.
 */

import { RpcError } from '@/api/errors';
import type {
  Evidence,
  EvidenceBody,
  EvidenceChartSeries,
  EvidenceCoverage,
  EvidenceKind,
  EvidenceSeriesCoverage,
  EvidenceSource,
  IncidentHead,
  TimelineEntry,
} from '@/api/types/ops';
import { fileStem, svgDataUrl } from '@/lib/capture';
import { formatCount, formatDuration, plural } from '@/lib/format';
import { formatDateTime } from '@/lib/time';

// ------------------------------------------------------------------ vocabulary

export const KINDS: readonly EvidenceKind[] = ['terminal', 'log', 'text', 'topology', 'chart'];

export const KIND_LABEL: Record<EvidenceKind, string> = {
  terminal: 'Terminal',
  log: 'Session log',
  text: 'Text',
  topology: 'Drawing',
  chart: 'Chart',
};

/** What each kind is, in a sentence for a tooltip. */
export const KIND_MEANING: Record<EvidenceKind, string> = {
  terminal: 'The last lines of a terminal session, read by the engine.',
  log: 'A piece of a session log, read by the engine.',
  text: 'Text a person handed in. The engine did not read it from a device.',
  topology: 'The topology as the engine drew it.',
  chart: 'Stored samples of one or more series over a period.',
};

export function isEvidenceKind(value: string | null): value is EvidenceKind {
  return value !== null && (KINDS as readonly string[]).includes(value);
}

/** Text supplied by a person rather than read by the engine. It says so wherever it is shown. */
export function isOperatorSupplied(source: EvidenceSource): boolean {
  return source.from === 'operator';
}

/** Where an item came from, as one sentence. */
export function sourceSentence(source: EvidenceSource): string {
  if (isOperatorSupplied(source)) {
    return 'Supplied by a person: this is text the client sent, not something the engine read from a device.';
  }
  const from = (() => {
    switch (source.from) {
      case 'terminal.snapshot':
        return source.session_title
          ? `Read by the engine from the terminal session “${source.session_title}”`
          : 'Read by the engine from a terminal session';
      case 'terminal.logs.read':
        return source.session_title
          ? `Read by the engine from the log of the session “${source.session_title}”`
          : 'Read by the engine from a session log';
      case 'topology.export':
        return 'Drawn by the engine from the topology';
      case 'metrics.query_many':
        return 'Read by the engine from the stored samples';
      default:
        return `Read by the engine with ${source.from}`;
    }
  })();
  return source.peer ? `${from} (${source.peer}).` : `${from}.`;
}

// --------------------------------------------------------------------- text

/**
 * `text` with every control character except the line break and the tab
 * shown as a picture of itself (`␛` for escape, `␈` for backspace), and every
 * direction override as the replacement character (U+FFFD).
 *
 * The rule is the engine's own (`nocturne-ops`, `text::visible`), so a body
 * on this screen and the same body in an exported review read alike. A line
 * that ends in CR LF ends once.
 */
export function visibleText(text: string): string {
  let out = '';
  for (const ch of text.replaceAll('\r\n', '\n')) out += pictureOf(ch) ?? ch;
  return out;
}

/** The picture a character is shown as, or `null` for one that is shown as itself. */
function pictureOf(ch: string): string | null {
  const code = ch.codePointAt(0) ?? 0;
  if (ch === '\n' || ch === '\t') return null;
  if (code <= 0x1f) return String.fromCodePoint(0x2400 + code);
  if (code === 0x7f) return '\u2421';
  if (code >= 0x80 && code <= 0x9f) return '\ufffd';
  // Not control characters to Unicode, and they do the same job: an override
  // that makes a line read right to left shows a reader something other than
  // what was printed.
  if (
    (code >= 0x202a && code <= 0x202e) ||
    (code >= 0x2066 && code <= 0x2069) ||
    code === 0x200e ||
    code === 0x200f ||
    code === 0x061c
  ) {
    return '\ufffd';
  }
  return null;
}

/** How many characters of a text are shown as a picture of themselves. */
export function picturedCharacters(text: string): number {
  let count = 0;
  for (const ch of text.replaceAll('\r\n', '\n')) if (pictureOf(ch) !== null) count += 1;
  return count;
}

/**
 * How many lines of an item are on screen at once.
 *
 * An item holds up to 256 KiB, which can be more than a hundred thousand
 * short lines, and a numbered line is three elements: put on screen whole,
 * such an item froze the window for seconds (measured in Chromium: 131,000
 * lines took 7 s to appear, and every click after that 5 s). A page of this
 * size appears in a fraction of a second, and holds a whole capture of the
 * default 200 lines many times over.
 */
export const TEXT_PAGE_LINES = 5_000;

/** The lines of a text, as the viewer numbers them: a final line break does not start a line. */
export function textLines(text: string): string[] {
  if (text === '') return [];
  const lines = text.split('\n');
  if (lines.at(-1) === '') lines.pop();
  return lines;
}

/** The part of a long text that is on screen, and where it stands in the whole. */
export interface TextPage {
  /** The lines of this page, joined again. */
  text: string;
  /** The page this is, from 0: the one asked for, or the nearest there is. */
  page: number;
  pages: number;
  /** The number of its first and last line in the whole item, from 1. */
  firstLine: number;
  lastLine: number;
  totalLines: number;
}

/**
 * One page of `lines`. Nothing is left out without being counted: the page
 * says which lines it holds and how many there are, and the pages together
 * are every line exactly once.
 */
export function textPage(lines: readonly string[], page: number, size: number = TEXT_PAGE_LINES): TextPage {
  const pages = Math.max(1, Math.ceil(lines.length / size));
  const at = Math.min(Math.max(0, Math.floor(page) || 0), pages - 1);
  const start = at * size;
  const shown = lines.slice(start, start + size);
  return {
    // A page that ends in an empty line keeps it: a final line break alone
    // would be read as the end of the line before it.
    text: shown.join('\n') + (shown.at(-1) === '' ? '\n' : ''),
    page: at,
    pages,
    firstLine: lines.length === 0 ? 0 : start + 1,
    lastLine: start + shown.length,
    totalLines: lines.length,
  };
}

// ------------------------------------------------------------------ a drawing

/**
 * The address an `<img>` shows a stored drawing from. This is the whole
 * defence for a drawing that is hostile by assumption, and it is made in one
 * place for every screen: see {@link svgDataUrl} for why it holds.
 */
export function svgImageSrc(svg: string): string {
  return svgDataUrl(svg);
}

// -------------------------------------------------------------------- a chart

/** A stretch of the period with no sample in any of the series. */
export interface Gap {
  /** The last sample before the hole. */
  from: number;
  /** The first sample after it. */
  to: number;
}

export interface StoredChart {
  /** Sample times, ascending, with one extra time inside each hole so the line ends there. */
  timestamps: number[];
  series: { id: string; label: string; values: (number | null)[] }[];
  gaps: Gap[];
  /** Milliseconds inside the holes, in all. */
  missingMs: number;
  /** Series whose stored `points` are not `[time, value]` pairs: shown as empty, and said. */
  unreadable: number;
  /** Stored points that were not a time and a number, and were left out. */
  dropped: number;
}

/** How much further apart than usual two samples must be to count as a hole. */
const GAP_FACTOR = 1.5;

function seriesName(coverage: EvidenceSeriesCoverage | undefined, stored: EvidenceChartSeries, several: boolean): string {
  const metric = coverage?.metric ?? (typeof stored.metric === 'string' ? stored.metric : 'series');
  const instance = coverage?.instance ?? (typeof stored.instance === 'string' ? stored.instance : '');
  const name = instance ? `${metric} · ${instance}` : metric;
  return several && coverage?.device ? `${coverage.device}: ${name}` : name;
}

/**
 * The stored series of a chart on one time axis, with the holes marked.
 *
 * The points are taken exactly as stored and nothing is filled in. A time at
 * which a series has no sample is `null` for it; where *no* series has a
 * sample for longer than usual, one extra time is inserted that nobody has a
 * value for, which is what makes the plot lift its pen. "Usual" is measured
 * from the data (the lower median distance between neighbouring samples) and
 * is never less than the bucket width the engine recorded.
 *
 * The body is whatever the monitor answered, redacted string by string, so
 * nothing about its shape is assumed: a point that is not a finite time and a
 * finite number is left out and counted.
 */
export function storedChart(body: Extract<EvidenceBody, { type: 'chart' }>): StoredChart {
  const stored = Array.isArray(body.series) ? body.series : [];
  const coverage = Array.isArray(body.coverage?.series) ? body.coverage.series : [];
  const several = new Set(coverage.map((entry) => entry.device_id)).size > 1;

  let unreadable = 0;
  let dropped = 0;
  const read = stored.map((entry) => {
    const points = new Map<number, number>();
    if (!Array.isArray(entry?.points)) {
      unreadable += 1;
      return points;
    }
    for (const point of entry.points as unknown[]) {
      const [at, value] = Array.isArray(point) ? (point as unknown[]) : [];
      if (typeof at === 'number' && Number.isFinite(at) && typeof value === 'number' && Number.isFinite(value)) {
        points.set(at, value);
      } else dropped += 1;
    }
    return points;
  });

  const axis = [...new Set(read.flatMap((points) => [...points.keys()]))].sort((a, b) => a - b);
  const steps = stored
    .map((entry) => entry?.step_s)
    .filter((step): step is number => typeof step === 'number' && Number.isFinite(step) && step > 0);
  const step = Math.max(1, ...steps) * 1000;
  const distances = axis.slice(1).map((at, index) => at - (axis[index] ?? at)).sort((a, b) => a - b);
  // The lower median: a handful of holes in a short series must not stretch
  // what counts as a normal distance and so hide themselves.
  const median = distances.length === 0 ? 0 : (distances[Math.floor((distances.length - 1) / 2)] ?? 0);
  const spacing = Math.max(step, median);

  const gaps: Gap[] = [];
  const timestamps: number[] = [];
  axis.forEach((at, index) => {
    const previous = index === 0 ? undefined : axis[index - 1];
    if (previous !== undefined && at - previous > spacing * GAP_FACTOR) {
      gaps.push({ from: previous, to: at });
      timestamps.push(previous + spacing);
    }
    timestamps.push(at);
  });

  return {
    timestamps,
    series: read.map((points, index) => ({
      id: `${index}`,
      label: seriesName(coverage[index], stored[index] ?? {}, several),
      values: timestamps.map((at) => points.get(at) ?? null),
    })),
    gaps,
    missingMs: gaps.reduce((total, gap) => total + (gap.to - gap.from), 0),
    unreadable,
    dropped,
  };
}

/** What the picture of a stored chart leaves out, in one line; `null` when it leaves nothing out. */
export function gapSentence(chart: Pick<StoredChart, 'gaps' | 'missingMs'>): string | null {
  if (chart.gaps.length === 0) return null;
  const first = chart.gaps[0];
  return chart.gaps.length === 1 && first
    ? `A gap of ${formatDuration(chart.missingMs)} from ${formatDateTime(first.from)}: nothing was recorded then.`
    : `${formatCount(chart.gaps.length)} gaps, ${formatDuration(chart.missingMs)} in all: nothing was recorded then.`;
}

export interface CoverageLine {
  /** `device: metric · instance`. */
  name: string;
  lab: boolean;
  /** No sample at all in the period. Empty, which is not "unknown". */
  empty: boolean;
  /** What is known about it, as a sentence. */
  text: string;
}

/**
 * The engine's own statement of coverage, one line per series, in the order
 * the series were asked for. A series without points says so in words: it was
 * read and there was nothing, which is a different fact from a series that
 * was never asked for.
 */
export function coverageLines(coverage: EvidenceCoverage): CoverageLine[] {
  return coverage.series.map((entry) => {
    const name = `${entry.device}: ${entry.instance ? `${entry.metric} · ${entry.instance}` : entry.metric}`;
    if (entry.points === 0) {
      return {
        name,
        lab: entry.lab,
        empty: true,
        text: 'no points — nothing was recorded for it in this period',
      };
    }
    const parts = [plural(entry.points, 'point')];
    if (entry.first_at !== undefined && entry.last_at !== undefined) {
      parts.push(
        entry.first_at === entry.last_at
          ? `at ${formatDateTime(entry.first_at)}`
          : `from ${formatDateTime(entry.first_at)} to ${formatDateTime(entry.last_at)}`,
      );
    }
    if (entry.longest_gap_ms !== undefined) {
      parts.push(`longest stretch between two of them ${formatDuration(entry.longest_gap_ms)}`);
    }
    if (entry.step_s !== undefined) {
      parts.push(`${entry.rollup ? `${entry.rollup} ` : ''}buckets of ${formatDuration(entry.step_s * 1000)}`);
    }
    return { name, lab: entry.lab, empty: false, text: parts.join(', ') };
  });
}

// ------------------------------------------------------------------- the list

/**
 * What a page of the list is a page *of*. The engine pages by cursor and
 * gives no total, so the honest statements are "this is all of it" and "this
 * is the newest N, and there is more".
 */
export function listCoverage(shown: number, more: boolean, filtered: boolean, firstPage: boolean): string {
  if (shown === 0) return filtered ? 'Nothing matches these filters.' : 'No evidence has been kept yet.';
  const items = plural(shown, 'item');
  if (!firstPage) {
    return more
      ? `${items} on this page, further back in time; older ones exist beyond it.`
      : `${items} on this page: the oldest ${filtered ? 'that match' : 'there are'}.`;
  }
  if (more) {
    return `The newest ${items}${filtered ? ' that match' : ''}; older ones exist and are not shown.`;
  }
  return filtered ? `All ${items} that match.` : `All ${items} that are kept.`;
}

// --------------------------------------------------------------- the timeline

/** An `evidence` entry whose item was deleted: no reference, and its text says when. */
export function isTombstone(entry: TimelineEntry): boolean {
  return entry.kind === 'evidence' && !entry.ref;
}

/** The evidence item a timeline entry points at, or `null` (not evidence, or a tombstone). */
export function attachedEvidenceId(entry: TimelineEntry): string | null {
  return entry.kind === 'evidence' && entry.ref?.type === 'evidence' ? entry.ref.id : null;
}

/**
 * What deleting an item does, said before it is done: the content is gone for
 * good, and every incident it is attached to keeps a line saying that
 * evidence was there. `incidents` are the ones whose titles are known; an id
 * with no title is still counted and named by its id.
 */
export function deleteWarning(item: Evidence, incidents: readonly Pick<IncidentHead, 'id' | 'title'>[]): string {
  const gone = `“${item.title}” and its content will be gone. This cannot be undone.`;
  if (item.incident_ids.length === 0) return `${gone} It is not attached to any incident.`;
  const names = item.incident_ids.map(
    (id) => incidents.find((incident) => incident.id === id)?.title ?? `incident ${id}`,
  );
  const list =
    names.length === 1
      ? `“${names[0]}”`
      : `${names
          .slice(0, -1)
          .map((name) => `“${name}”`)
          .join(', ')} and “${names.at(-1)}”`;
  return `${gone} ${list} will keep a line on ${names.length === 1 ? 'its' : 'their'} timeline saying that evidence was here and was deleted, with its hash — a tombstone, not a hole.`;
}

// ------------------------------------------------------------------- refusals

/** A failed read or write of evidence, as a headline and the engine's own sentence. */
export function failure(error: unknown): { headline: string; message: string } {
  const known = error instanceof RpcError ? error : null;
  const message = known?.message ?? (error instanceof Error ? error.message : 'Something went wrong.');
  switch (known?.code) {
    case 'unavailable':
      return { headline: 'The engine could not read it', message };
    case 'not_found':
      return { headline: 'It is not there', message };
    case 'invalid':
      return { headline: 'It was refused', message };
    case 'unknown_method':
      return { headline: 'Not in this build', message };
    default:
      return { headline: 'It did not work', message };
  }
}

// --------------------------------------------------------------------- files

/** What a file made from an item is called: its title, its id, an extension. */
export function evidenceFilename(item: Pick<Evidence, 'id' | 'title'>, extension: string): string {
  return `${fileStem(item.title, 'evidence')}-${item.id}.${extension}`;
}
