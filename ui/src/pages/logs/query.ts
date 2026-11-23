/**
 * The vocabulary of a log search: syslog's own severity scale, the severity
 * bands a histogram is stacked in, and the one-line query language the search
 * box accepts.
 *
 * Two rules decide everything here.
 *
 * **The backend owns the text search.** `logs.query` already understands
 * words, `-exclusions` and `"two word phrases"`, and it looks for each term in
 * the message, the sending address *and* the vendor mnemonic. Re-implementing
 * that in the browser would be a second, subtly different language. So
 * {@link parseQuery} only lifts out the `field:value` pairs that map onto the
 * filter's own columns and hands everything else to the backend **verbatim**,
 * quotes and minus signs included.
 *
 * **Severity counts down, and absent is not debug.** 0 is an emergency and 7
 * is debug, so "error and worse" is `max_severity: 3`. A line whose priority
 * and mnemonic both said nothing has `severity: null`, which is a third value:
 * SQL's `severity <= 3` excludes it, so *any* severity filter hides it. That is
 * what {@link SEVERITY_BANDS}' last band is for, and why the screen counts it
 * separately rather than letting it vanish.
 */

import type { LogFilter } from '@/api/types/logs';
import type { StatusTone } from '@/design/tone';
import { formatDate, formatTime } from '@/lib/time';

/** Syslog severity names, index = the number. The backend's own list. */
export const SEVERITY_NAMES = [
  'emergency',
  'alert',
  'critical',
  'error',
  'warning',
  'notice',
  'informational',
  'debug',
] as const;

/** Syslog facility names, index = the number. The backend's own list. */
export const FACILITY_NAMES = [
  'kern',
  'user',
  'mail',
  'daemon',
  'auth',
  'syslog',
  'lpr',
  'news',
  'uucp',
  'cron',
  'authpriv',
  'ftp',
  'ntp',
  'audit',
  'alert',
  'clock',
  'local0',
  'local1',
  'local2',
  'local3',
  'local4',
  'local5',
  'local6',
  'local7',
] as const;

/** `3` → `error`. Anything outside 0…7 is unknown, not a guess. */
export function severityName(severity: number | null | undefined): string {
  if (severity == null) return 'unknown';
  return SEVERITY_NAMES[severity] ?? 'unknown';
}

/** `20` → `local4`. */
export function facilityName(facility: number | null | undefined): string {
  if (facility == null) return 'unknown';
  return FACILITY_NAMES[facility] ?? 'unknown';
}

/**
 * The colour a severity carries. Status tones only, and never alone: every
 * place that uses one also writes the word.
 */
export function severityTone(severity: number | null | undefined): StatusTone {
  if (severity == null) return 'idle';
  if (severity <= 3) return 'crit';
  if (severity === 4) return 'warn';
  if (severity <= 6) return 'info';
  return 'idle';
}

/** What the severity filter offers, as `max_severity` values. */
export const SEVERITY_CHOICES: readonly { value: number; label: string }[] = [
  { value: 2, label: 'Critical and worse' },
  { value: 3, label: 'Error and worse' },
  { value: 4, label: 'Warning and worse' },
  { value: 5, label: 'Notice and worse' },
  { value: 6, label: 'Informational and worse' },
];

/** "error and worse", for a sentence about what is being shown. */
export function severityPhrase(max: number): string {
  return `${severityName(max)} and worse`;
}

export type BandId = 'error' | 'warning' | 'notice' | 'debug' | 'unsaid';

export interface SeverityBand {
  id: BandId;
  label: string;
  tone: StatusTone;
  /** The severities it covers, or `null` for the lines that gave none. */
  severities: readonly number[] | null;
}

/**
 * Eight severities are too many stacked segments to read, and a NOC reads four
 * groups anyway. The fifth band is not a severity: it is the lines that never
 * said, kept visible rather than counted as debug.
 */
export const SEVERITY_BANDS: readonly SeverityBand[] = [
  { id: 'error', label: 'Error and worse', tone: 'crit', severities: [0, 1, 2, 3] },
  { id: 'warning', label: 'Warning', tone: 'warn', severities: [4] },
  { id: 'notice', label: 'Notice and info', tone: 'info', severities: [5, 6] },
  { id: 'debug', label: 'Debug', tone: 'idle', severities: [7] },
  { id: 'unsaid', label: 'No severity given', tone: 'idle', severities: null },
];

/** The band a line falls in, for colouring one row. */
export function bandOf(severity: number | null | undefined): BandId {
  if (severity == null) return 'unsaid';
  if (severity <= 3) return 'error';
  if (severity === 4) return 'warning';
  if (severity <= 6) return 'notice';
  return 'debug';
}

/** Whether a `max_severity` filter would keep a band's lines. */
export function bandKept(band: SeverityBand, max: number | undefined): boolean {
  if (max === undefined) return true;
  // `severity <= max` in SQL: a NULL severity satisfies nothing at all.
  if (band.severities === null) return false;
  return band.severities.some((severity) => severity <= max);
}

// ------------------------------------------------------------- the query bar

/** The streams the screen can search. Devices fill `syslog` and `trap`. */
export const STREAMS: readonly { value: string; label: string }[] = [
  { value: 'syslog', label: 'Device logs' },
  { value: 'trap', label: 'SNMP traps' },
  { value: 'event', label: 'Signals' },
  { value: 'audit', label: 'Audit trail' },
  { value: '*', label: 'Every stream' },
];

export interface ParsedQuery {
  /** Everything that was not a `field:value`, for the backend's text search. */
  text: string;
  mnemonic?: string;
  interface?: string;
  source?: string;
  facility?: number;
  unattributed?: boolean;
  /** `field:value` tokens whose field does not exist, so the bar can say so. */
  unknown: string[];
}

/** The fields the bar understands, and the aliases people type. */
const FIELDS: Record<string, 'mnemonic' | 'interface' | 'source' | 'facility' | 'is'> = {
  mnemonic: 'mnemonic',
  mn: 'mnemonic',
  interface: 'interface',
  int: 'interface',
  if: 'interface',
  source: 'source',
  from: 'source',
  facility: 'facility',
  fac: 'facility',
  is: 'is',
};

/** Splits on whitespace, keeping `"a phrase"` (and its quotes) in one piece. */
function tokenize(input: string): string[] {
  const tokens: string[] = [];
  let token = '';
  let quoted = false;
  for (const character of input) {
    if (character === '"') {
      quoted = !quoted;
      token += character;
    } else if (!quoted && /\s/.test(character)) {
      if (token !== '') tokens.push(token);
      token = '';
    } else {
      token += character;
    }
  }
  if (token !== '') tokens.push(token);
  return tokens;
}

/** `"local7"` or `"23"` → 23; anything else → undefined. */
function facilityOf(value: string): number | undefined {
  const named = FACILITY_NAMES.indexOf(value.toLowerCase() as (typeof FACILITY_NAMES)[number]);
  if (named >= 0) return named;
  const number = Number(value);
  return Number.isInteger(number) && number >= 0 && number <= 23 ? number : undefined;
}

function unquote(value: string): string {
  return value.startsWith('"') && value.endsWith('"') && value.length > 1
    ? value.slice(1, -1)
    : value;
}

/**
 * Reads the search box.
 *
 * `LINK-3-UPDOWN if:Gi1/0/3 "changed state" -up` becomes a mnemonic, an
 * interface, and `LINK-3-UPDOWN "changed state" -up` left for the backend —
 * which is where the phrase and the exclusion are understood.
 */
export function parseQuery(input: string): ParsedQuery {
  const parsed: ParsedQuery = { text: '', unknown: [] };
  const rest: string[] = [];
  for (const token of tokenize(input)) {
    const colon = token.indexOf(':');
    // A leading `-` is the backend's exclusion marker, and a bare `:` in the
    // middle of a word (an IPv6 address, a Cisco counter) is not a field.
    const key = colon > 0 && !token.startsWith('-') ? token.slice(0, colon).toLowerCase() : '';
    const field = key === '' ? undefined : FIELDS[key];
    if (field === undefined) {
      // `foo:bar` where `foo` is not a field: say so rather than silently
      // searching for the whole token, which would find nothing and look
      // like "no such lines".
      if (key !== '' && /^[a-z_]+$/.test(key)) parsed.unknown.push(key);
      else rest.push(token);
      continue;
    }
    const value = unquote(token.slice(colon + 1));
    if (value === '') continue;
    if (field === 'facility') {
      const facility = facilityOf(value);
      if (facility === undefined) parsed.unknown.push(`${key}:${value}`);
      else parsed.facility = facility;
    } else if (field === 'is') {
      if (value.toLowerCase() === 'unattributed') parsed.unattributed = true;
      else parsed.unknown.push(`${key}:${value}`);
    } else {
      parsed[field] = value;
    }
  }
  parsed.text = rest.join(' ');
  return parsed;
}

export interface FilterInput {
  query: string;
  stream: string;
  deviceId: string | null;
  maxSeverity: number | undefined;
  since: number;
  until?: number;
}

/** The parameters every call on this screen shares. */
export function filterFrom({
  query,
  stream,
  deviceId,
  maxSeverity,
  since,
  until,
}: FilterInput): LogFilter {
  const parsed = parseQuery(query);
  return {
    stream,
    ...(deviceId ? { device_id: deviceId } : {}),
    ...(maxSeverity === undefined ? {} : { max_severity: maxSeverity }),
    ...(parsed.text === '' ? {} : { text: parsed.text }),
    ...(parsed.mnemonic ? { mnemonic: parsed.mnemonic } : {}),
    ...(parsed.interface ? { interface: parsed.interface } : {}),
    ...(parsed.source ? { source: parsed.source } : {}),
    ...(parsed.facility === undefined ? {} : { facility: parsed.facility }),
    ...(parsed.unattributed ? { unattributed: true } : {}),
    since,
    ...(until === undefined ? {} : { until }),
  };
}

/** Whether anything but the time range is narrowing the search. */
export function isNarrowed(filter: LogFilter): boolean {
  return (
    filter.device_id !== undefined ||
    filter.max_severity !== undefined ||
    filter.text !== undefined ||
    filter.mnemonic !== undefined ||
    filter.interface !== undefined ||
    filter.source !== undefined ||
    filter.facility !== undefined ||
    filter.unattributed === true
  );
}

/** The same filter with the severity cut taken off, for the unfiltered counts. */
export function withoutSeverity(filter: LogFilter): LogFilter {
  const { max_severity: _dropped, ...rest } = filter;
  return rest;
}

// ------------------------------------------------------------- the time range

export interface Span {
  id: string;
  label: string;
  /** How far back it reaches, in milliseconds. */
  ms: number;
  /** Buckets to ask the histogram for. */
  buckets: number;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

const LAST_HOUR: Span = { id: '1h', label: 'Last hour', ms: HOUR, buckets: 60 };

export const SPANS: readonly Span[] = [
  { id: '15m', label: 'Last 15 minutes', ms: 15 * MINUTE, buckets: 45 },
  LAST_HOUR,
  { id: '6h', label: 'Last 6 hours', ms: 6 * HOUR, buckets: 72 },
  { id: '24h', label: 'Last 24 hours', ms: DAY, buckets: 96 },
  { id: '7d', label: 'Last 7 days', ms: 7 * DAY, buckets: 84 },
  { id: '30d', label: 'Last 30 days', ms: 30 * DAY, buckets: 90 },
];

export const DEFAULT_SPAN = LAST_HOUR.id;

export function span(id: string | null): Span {
  return SPANS.find((entry) => entry.id === id) ?? LAST_HOUR;
}

/**
 * An axis label for a time in a range of `spanMs`: the time of day alone when
 * the whole range is inside a day, the date as well when it is not. Seconds
 * are dropped — a histogram bucket is never a second wide, and `09:13` next to
 * `09:14` reads where `09:13:00` does not.
 */
export function timeLabel(at: number, spanMs: number): string {
  const time = formatTime(at).slice(0, 5);
  if (spanMs <= 36 * HOUR) return time;
  return `${formatDate(at).slice(5)} ${time}`;
}
