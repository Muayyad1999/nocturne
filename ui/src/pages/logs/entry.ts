/**
 * Reading one stored line.
 *
 * `LogEntry.body` is `unknown` on purpose: the shape depends on the stream and
 * on who wrote the row. The syslog receiver writes {@link SyslogBody}; a lab's
 * simulated devices write their lines straight into the same table with
 * `{line, lab_id, sim_time}` and no `raw`, `via` or mnemonic (see NOTES.md);
 * the `event` stream carries a signal's payload. So every field is read
 * defensively and a missing one is reported as missing, never guessed.
 *
 * The clock is the part worth being careful about. `ts` is **when Nocturne
 * received the line** — a device's own clock is its own business and is wrong
 * often enough that saying which one a time came from is not pedantry. What the
 * device wrote is kept verbatim as text in `body.timestamp`, and for a line
 * read out of a device's buffer (`logs.pull`) `ts` *is* the device's reading,
 * which `body.ts_source` says.
 */

import type { LogEntry } from '@/api/types/core';
import type { SyslogBody } from '@/api/types/logs';

type Body = Partial<SyslogBody> & {
  /** What the lab's simulated devices write instead of `raw`. */
  line?: string;
  lab_id?: string;
};

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

/** The body as far as it can be trusted. Never throws, never invents. */
export function bodyOf(entry: LogEntry): Body {
  if (!isObject(entry.body)) return {};
  const body = entry.body;
  return {
    ...(text(body.raw) === undefined ? {} : { raw: text(body.raw) }),
    ...(text(body.line) === undefined ? {} : { line: text(body.line) }),
    ...(text(body.via) === undefined ? {} : { via: text(body.via) as SyslogBody['via'] }),
    ...(text(body.mnemonic) === undefined ? {} : { mnemonic: text(body.mnemonic) }),
    ...(text(body.host) === undefined ? {} : { host: text(body.host) }),
    ...(text(body.app) === undefined ? {} : { app: text(body.app) }),
    ...(text(body.timestamp) === undefined ? {} : { timestamp: text(body.timestamp) }),
    ...(text(body.interface) === undefined ? {} : { interface: text(body.interface) }),
    ...(text(body.interface_text) === undefined
      ? {}
      : { interface_text: text(body.interface_text) }),
    ...(text(body.severity_name) === undefined ? {} : { severity_name: text(body.severity_name) }),
    ...(text(body.facility_name) === undefined ? {} : { facility_name: text(body.facility_name) }),
    ...(typeof body.facility === 'number' ? { facility: body.facility } : {}),
    ...(typeof body.sequence === 'number' ? { sequence: body.sequence } : {}),
    ...(text(body.ts_source) === undefined
      ? {}
      : { ts_source: text(body.ts_source) as SyslogBody['ts_source'] }),
    ...(text(body.lab_id) === undefined ? {} : { lab_id: text(body.lab_id) }),
  };
}

/**
 * The line as the device wrote it, for the drawer and for copying.
 *
 * The receiver keeps it in `body.raw`; a lab device's own line is in
 * `body.line`; with neither, the parsed message is all there is, and that is
 * what is shown rather than an empty box.
 */
export function rawOf(entry: LogEntry): { text: string; exact: boolean } {
  const body = bodyOf(entry);
  const raw = body.raw ?? body.line;
  return raw === undefined ? { text: entry.message, exact: false } : { text: raw, exact: true };
}

export type Clock = 'received' | 'device';

export interface ClockFacts {
  /** Which clock the time in the row's first column came from. */
  shown: Clock;
  /** `ts`: when this machine received or read the line. */
  receivedAt: number;
  /** What the device wrote, verbatim, when it wrote anything. */
  deviceText: string | null;
  /** One sentence naming the clock, for a tooltip or the drawer. */
  note: string;
}

/**
 * Which clock a row's time belongs to.
 *
 * A received line is stamped by this machine. A pulled buffer line keeps the
 * device's own date when it could be read, because stamping a week of history
 * at the moment of the pull would put it all in one second of the histogram.
 */
export function clockOf(entry: LogEntry): ClockFacts {
  const body = bodyOf(entry);
  const deviceText = body.timestamp ?? null;
  const fromDevice = body.ts_source === 'device';
  return {
    shown: fromDevice ? 'device' : 'received',
    receivedAt: entry.ts,
    deviceText,
    note: fromDevice
      ? 'This time is the device’s own, read out of its log buffer and interpreted in this machine’s time zone.'
      : deviceText === null
        ? 'This time is when Nocturne received the line. The line itself carried no timestamp.'
        : `This time is when Nocturne received the line. The device wrote “${deviceText}” by its own clock.`,
  };
}

/** How the line got here, in a word, or null when the row does not say. */
export function viaOf(entry: LogEntry): 'syslog' | 'pull' | 'lab' | null {
  const body = bodyOf(entry);
  if (body.via === 'pull') return 'pull';
  if (body.via === 'syslog') return 'syslog';
  if (body.lab_id !== undefined) return 'lab';
  return null;
}

/** The vendor mnemonic (`LINK-3-UPDOWN`), when the line had one. */
export function mnemonicOf(entry: LogEntry): string | null {
  return bodyOf(entry).mnemonic ?? null;
}

/** The interface this line is about, as the device spelled it. */
export function interfaceOf(entry: LogEntry): string | null {
  const body = bodyOf(entry);
  return body.interface_text ?? body.interface ?? null;
}

/**
 * What to put in the middle column: the mnemonic, or the application, or the
 * sending address — in that order, because that is the order a reader scans.
 * Never an empty cell with no explanation.
 */
export function tagOf(entry: LogEntry): string {
  return mnemonicOf(entry) ?? bodyOf(entry).app ?? entry.source;
}
