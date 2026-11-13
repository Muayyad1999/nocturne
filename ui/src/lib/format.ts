/**
 * Number formatting for the whole UI. Screens never format inline: a byte
 * count, a rate, a duration or a percentage always goes through here so the
 * same quantity reads the same everywhere.
 *
 * Missing values (null, undefined, NaN, ±Infinity) render as an em dash; a
 * number is never invented.
 */

/** Shown wherever a value is missing. */
export const EMPTY = '—';

/** Thin space: groups digits in threes without the ambiguity of a comma or a point. */
const GROUP = '\u2009';
/** No-break space between a value and its unit. */
const NBSP = '\u00a0';
const MINUS = '\u2212';

export type Numeric = number | null | undefined;

/** A formatted quantity split so the unit can be styled separately. */
export interface Quantity {
  value: string;
  unit: string;
}

function isMissing(value: Numeric): value is null | undefined {
  return value == null || !Number.isFinite(value);
}

function joinQuantity({ value, unit }: Quantity): string {
  return unit ? `${value}${NBSP}${unit}` : value;
}

/** Groups the integer part of a plain decimal string in threes. */
function group(digits: string): string {
  const [integer = '', fraction] = digits.split('.');
  const grouped = integer.replace(/\B(?=(\d{3})+(?!\d))/g, GROUP);
  return fraction === undefined ? grouped : `${grouped}.${fraction}`;
}

/** Prefixes a formatted magnitude with a minus sign, unless it rounded to zero. */
function signed(value: number, magnitude: string): string {
  return value < 0 && Number(magnitude.replaceAll(GROUP, '')) !== 0 ? MINUS + magnitude : magnitude;
}

function trimZeros(text: string): string {
  return text.includes('.') ? text.replace(/\.?0+$/, '') : text;
}

export interface NumberOptions {
  /** Fixed number of decimals. Default 0. */
  digits?: number;
}

/** A plain number with digit grouping: `12 345.6`. */
export function formatNumber(value: Numeric, { digits = 0 }: NumberOptions = {}): string {
  if (isMissing(value)) return EMPTY;
  return signed(value, group(Math.abs(value).toFixed(digits)));
}

/** A whole count with digit grouping: `1 204`. Fractions are rounded. */
/**
 * Counted things, written the way a person would: `1 device`, `3 devices`,
 * `1,200 interfaces`. An interface that writes "1 device(s)" has stopped
 * paying attention.
 */
export function plural(count: number, one: string, many = `${one}s`): string {
  return `${formatCount(count)} ${count === 1 ? one : many}`;
}

export function formatCount(value: Numeric): string {
  return formatNumber(value);
}

/**
 * A change with an explicit sign: `+4.2%`, `−12`, `0`. `format` is any
 * formatter from this module; it defaults to `formatNumber`.
 */
export function formatDelta(
  value: Numeric,
  format: (value: Numeric) => string = formatNumber,
): string {
  if (isMissing(value)) return EMPTY;
  const text = format(value);
  const roundedToZero = /^0(\.0+)?(\D.*)?$/.test(text);
  return value > 0 && !roundedToZero ? `+${text}` : text;
}

interface Scaled {
  /** Magnitude in the chosen unit: one decimal under 100, none above. */
  text: string;
  /** Index of the chosen unit. */
  exponent: number;
}

/**
 * Scales a magnitude into the largest unit that keeps it below `base`,
 * promoting when rounding reaches `base` (1023.97 KiB reads as 1 MiB, never
 * 1024 KiB). The first unit is never fractional.
 */
function scale(magnitude: number, base: number, maxExponent: number): Scaled {
  let exponent = 0;
  let scaled = magnitude;
  while (scaled >= base && exponent < maxExponent) {
    scaled /= base;
    exponent += 1;
  }
  const digits = exponent > 0 && scaled < 100 ? 1 : 0;
  let text = scaled.toFixed(digits);
  if (Number(text) >= base && exponent < maxExponent) {
    exponent += 1;
    text = (scaled / base).toFixed(1);
  }
  return { text: trimZeros(text), exponent };
}

function quantity(value: Numeric, base: number, units: readonly string[]): Quantity {
  if (isMissing(value)) return { value: EMPTY, unit: '' };
  const { text, exponent } = scale(Math.abs(value), base, units.length - 1);
  return { value: signed(value, text), unit: units[exponent] ?? '' };
}

const COUNT_UNITS = ['', 'k', 'M', 'B', 'T'];

/** A short count for badges and axis ticks: `950`, `1.2k`, `34k`, `5.6M`. */
export function formatCompact(value: Numeric): string {
  const { value: text, unit } = quantity(value, 1000, COUNT_UNITS);
  return text + unit;
}

const BYTE_UNITS = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];

/** A byte count in binary (IEC) units, split into value and unit. */
export function bytesQuantity(bytes: Numeric): Quantity {
  return quantity(bytes, 1024, BYTE_UNITS);
}

/** A byte count in binary (IEC) units: `512 B`, `1.2 GiB`, `153 MiB`. */
export function formatBytes(bytes: Numeric): string {
  return joinQuantity(bytesQuantity(bytes));
}

const RATE_UNITS = ['b/s', 'kb/s', 'Mb/s', 'Gb/s', 'Tb/s'];

/** A bit rate in decimal (SI) units, split into value and unit. */
export function rateQuantity(bitsPerSecond: Numeric): Quantity {
  return quantity(bitsPerSecond, 1000, RATE_UNITS);
}

/** A bit rate in decimal (SI) units: `940 Mb/s`, `1.2 Gb/s`, `56 kb/s`. */
export function formatRate(bitsPerSecond: Numeric): string {
  return joinQuantity(rateQuantity(bitsPerSecond));
}

export interface PercentOptions {
  /** Fixed decimals. Default: one decimal below 10 %, none from 10 % up. */
  digits?: number;
  /** The input is a ratio (0–1) rather than a percentage (0–100). */
  fromRatio?: boolean;
}

/** A percentage: `42%`, `0.3%`, `100%`. Takes 0–100 unless `fromRatio` is set. */
export function formatPercent(
  value: Numeric,
  { digits, fromRatio = false }: PercentOptions = {},
): string {
  if (isMissing(value)) return EMPTY;
  const percent = fromRatio ? value * 100 : value;
  const magnitude = Math.abs(percent);
  const text =
    digits === undefined
      ? trimZeros(magnitude.toFixed(magnitude < 10 ? 1 : 0))
      : magnitude.toFixed(digits);
  return `${signed(percent, text)}%`;
}

const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const YEAR = 365 * DAY;

const DURATION_UNITS: readonly (readonly [size: number, unit: string])[] = [
  [YEAR, 'y'],
  [DAY, 'd'],
  [HOUR, 'h'],
  [MINUTE, 'm'],
  [SECOND, 's'],
];

/**
 * A duration from milliseconds, as its two most significant units:
 * `3d 4h`, `4h 12m`, `12m 5s`, `45s`, `1.5s`, `320 ms`.
 */
export function formatDuration(milliseconds: Numeric): string {
  if (isMissing(milliseconds)) return EMPTY;
  const total = Math.abs(milliseconds);

  if (Math.round(total) < SECOND) {
    return `${signed(milliseconds, String(Math.round(total)))}${NBSP}ms`;
  }
  if (total < 10 * SECOND) {
    const seconds = trimZeros((total / SECOND).toFixed(1));
    if (Number(seconds) < 10) return `${signed(milliseconds, seconds)}s`;
  }

  // Round to whole seconds first so 59.6 s reads as 1m, not 60s.
  let rest = Math.round(total / SECOND) * SECOND;
  const parts: string[] = [];
  for (const [size, unit] of DURATION_UNITS) {
    const count = Math.floor(rest / size);
    rest -= count * size;
    if (count > 0) parts.push(`${formatCount(count)}${unit}`);
    else if (parts.length > 0) break; // "3d", not "3d 0h 12m"
    if (parts.length === 2) break;
  }
  return (milliseconds < 0 ? MINUS : '') + parts.join(' ');
}
