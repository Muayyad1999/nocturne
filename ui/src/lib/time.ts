/**
 * Time formatting. Timestamps from the backend are milliseconds since the
 * epoch. Absolute times are written ISO-style (`2026-11-13 02:47:05`) because
 * engineers compare them with device logs; relative times are short (`5m ago`).
 */

import { EMPTY, type Numeric } from './format';

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

const RELATIVE_UNITS: readonly (readonly [size: number, unit: string])[] = [
  [YEAR, 'y'],
  [MONTH, 'mo'],
  [DAY, 'd'],
  [HOUR, 'h'],
  [MINUTE, 'm'],
  [SECOND, 's'],
];

function isMissing(value: Numeric): value is null | undefined {
  return value == null || !Number.isFinite(value);
}

/** Relative time: `just now`, `42s ago`, `5m ago`, `3h ago`, `12d ago`, `in 5m`. */
export function formatRelative(at: Numeric, now: number = Date.now()): string {
  if (isMissing(at)) return EMPTY;
  const elapsed = now - at;
  const magnitude = Math.abs(elapsed);
  if (magnitude < 5 * SECOND) return 'just now';
  for (const [size, unit] of RELATIVE_UNITS) {
    if (magnitude >= size) {
      const count = Math.floor(magnitude / size);
      return elapsed > 0 ? `${count}${unit} ago` : `in ${count}${unit}`;
    }
  }
  return 'just now';
}

/**
 * How often a relative label for `at` needs re-rendering to stay correct:
 * every second while it counts seconds, then progressively less often.
 */
export function relativeRefreshInterval(at: number, now: number = Date.now()): number {
  const magnitude = Math.abs(now - at);
  if (magnitude < MINUTE) return SECOND;
  if (magnitude < HOUR) return 15 * SECOND;
  return MINUTE;
}

export interface DateTimeOptions {
  /** Format in UTC instead of the local time zone. */
  utc?: boolean;
}

const pad = (value: number, width = 2): string => String(value).padStart(width, '0');

function parts(at: number, utc: boolean) {
  const date = new Date(at);
  return utc
    ? {
        year: date.getUTCFullYear(),
        month: date.getUTCMonth() + 1,
        day: date.getUTCDate(),
        hours: date.getUTCHours(),
        minutes: date.getUTCMinutes(),
        seconds: date.getUTCSeconds(),
      }
    : {
        year: date.getFullYear(),
        month: date.getMonth() + 1,
        day: date.getDate(),
        hours: date.getHours(),
        minutes: date.getMinutes(),
        seconds: date.getSeconds(),
      };
}

/** A calendar date: `2026-11-13`. */
export function formatDate(at: Numeric, { utc = false }: DateTimeOptions = {}): string {
  if (isMissing(at)) return EMPTY;
  const { year, month, day } = parts(at, utc);
  return `${pad(year, 4)}-${pad(month)}-${pad(day)}`;
}

/** A time of day, 24-hour: `02:47:05`. */
export function formatTime(at: Numeric, { utc = false }: DateTimeOptions = {}): string {
  if (isMissing(at)) return EMPTY;
  const { hours, minutes, seconds } = parts(at, utc);
  return `${pad(hours)}:${pad(minutes)}:${pad(seconds)}`;
}

/** Date and time: `2026-11-13 02:47:05`. */
export function formatDateTime(at: Numeric, options: DateTimeOptions = {}): string {
  if (isMissing(at)) return EMPTY;
  return `${formatDate(at, options)} ${formatTime(at, options)}`;
}

/** The local offset from UTC at a given instant: `UTC+02:00`, `UTC-05:30`, `UTC`. */
export function formatUtcOffset(at: number): string {
  const minutesEast = -new Date(at).getTimezoneOffset();
  if (minutesEast === 0) return 'UTC';
  const magnitude = Math.abs(minutesEast);
  const sign = minutesEast > 0 ? '+' : '-';
  return `UTC${sign}${pad(Math.floor(magnitude / 60))}:${pad(magnitude % 60)}`;
}

/**
 * The unambiguous form of an instant for tooltips and audit trails: local
 * time with its offset, plus UTC when the two differ.
 * `2026-11-13 04:47:05 UTC+02:00 · 02:47:05 UTC`
 */
export function formatTimestamp(at: Numeric): string {
  if (isMissing(at)) return EMPTY;
  const offset = formatUtcOffset(at);
  const local = `${formatDateTime(at)} ${offset}`;
  if (offset === 'UTC') return local;
  const sameDay = formatDate(at) === formatDate(at, { utc: true });
  const utc = sameDay ? formatTime(at, { utc: true }) : formatDateTime(at, { utc: true });
  return `${local} · ${utc} UTC`;
}
