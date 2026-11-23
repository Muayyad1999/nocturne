/**
 * The three receivers, as the cards show them.
 *
 * `logs.status` reports each receiver in its own units — lines for syslog,
 * notifications for traps, datagrams *and* flow records for the flow
 * collector — and each with the counters that mean something for it. This
 * module turns one status into three cards without adding anything to it:
 *
 * * **A counter the engine does not give is not shown as zero.** Every
 *   counter is read through {@link counter}, which yields a row only when the
 *   status holds a number under that name. A status from a build that knows
 *   nothing of `from_lab`, or a receiver reported as not built, has fewer
 *   rows — never a row of noughts that nobody measured.
 * * **Off, failed and absent are three states.** Switched off is the user's
 *   decision; enabled and not listening is a failure with the engine's own
 *   reason; `implemented: false` is a build that has no such receiver, and
 *   that card has no switch at all.
 * * **Everything is "since the application started"**, and the card says so:
 *   the counters are not persisted, so a small number after a restart is not
 *   a quiet network.
 *
 * The second half is the settings form: what a port field may hold before it
 * is sent, and the smallest patch that says what changed. A port that is not
 * a number is the one thing refused here rather than by the engine, because
 * `settings.set` stores what it is given and a section that no longer reads
 * falls back to its defaults — every receiver would silently switch off.
 */

import type { LogsSettings, LogsStatus } from '@/api/types/logs';
import type { StatusTone } from '@/design/tone';

export type ReceiverId = 'syslog' | 'traps' | 'flows';

export type ReceiverState =
  /** This build has no such receiver. */
  | 'absent'
  /** Switched off. Nothing is listening, by choice. */
  | 'off'
  /** Enabled, and the socket could not be opened. */
  | 'failed'
  /** Bound and receiving whatever is sent. */
  | 'listening';

export interface Counter {
  /** The engine's own field name, for a test id and a stable key. */
  id: string;
  label: string;
  value: number;
  /** What the number counts, when the card mixes units. */
  unit?: string;
  /** One sentence on what ends up under this counter. */
  hint?: string;
  /** Set when a non-zero value deserves the eye: something was lost or refused. */
  attention?: boolean;
  /** `bytes`: the value is a size, read as one (`12 KiB`). */
  format?: 'bytes';
  /** The ceiling the value is under, in the same unit, when the engine gives one. */
  of?: number;
}

export interface ReceiverCard {
  id: ReceiverId;
  title: string;
  /** What it listens for, in a clause. */
  what: string;
  state: ReceiverState;
  tone: StatusTone;
  /** `Listening`, `Switched off`, `Not listening`, `Not in this build`. */
  stateLabel: string;
  /** What is bound right now: `udp 0.0.0.0:5514`. */
  listening: string[];
  /** The engine's own reason for not listening (or for not being built). */
  error: string | null;
  enabled: boolean;
  counters: Counter[];
  /** Flow records not counted, by reason. Only the flow collector has these. */
  drops: Counter[];
  /** What the flow buckets hold. Only the flow collector has these. */
  memory: Counter[];
  /** `null`: nothing has arrived; `undefined`: the engine does not say. */
  lastAt: number | null | undefined;
  /** The start of the oldest flow bucket, when there is one. */
  oldestAt?: number | null;
  /** The engine's sentence about simulated devices, when it gives one. */
  lab: string | null;
  /** What anybody who can reach the port can do, where that matters. */
  caution: string | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * One counter, or nothing. `source` is whatever the status holds for a
 * receiver; a missing or non-numeric field yields `null`, so the caller's
 * list simply does not have that row.
 */
export function counter(
  source: unknown,
  id: string,
  label: string,
  extra: Omit<Counter, 'id' | 'label' | 'value'> = {},
): Counter | null {
  if (!isRecord(source)) return null;
  const value = source[id];
  if (typeof value !== 'number' || !Number.isFinite(value)) return null;
  return { id, label, value, ...extra };
}

/**
 * A counter's unit as it reads beside its number: `1 line`, `2 lines`,
 * `1 flow record`. Every unit here is a plural that ends in `s`.
 */
export function unitFor(entry: Pick<Counter, 'value' | 'unit'>): string | null {
  if (!entry.unit) return null;
  return entry.value === 1 ? entry.unit.replace(/s$/, '') : entry.unit;
}

/**
 * What a receiver's queue takes up in memory, against its budget — or
 * nothing from a build whose queues are bounded in entries only. The budget
 * is shown only beside a size, never as a counter of its own.
 */
export function queueBytes(source: unknown, what: string): Counter | null {
  const size = counter(source, 'queued_bytes', 'Waiting, in memory', {
    format: 'bytes',
    hint: `What the ${what} waiting to be written take up. What does not fit in the budget is dropped and counted.`,
  });
  if (size === null) return null;
  const ceiling = isRecord(source) ? source.max_queued_bytes : undefined;
  return typeof ceiling === 'number' && Number.isFinite(ceiling) ? { ...size, of: ceiling } : size;
}

function present(counters: (Counter | null)[]): Counter[] {
  return counters.filter((entry): entry is Counter => entry !== null);
}

function text(value: unknown): string | null {
  return typeof value === 'string' && value !== '' ? value : null;
}

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : [];
}

function stateOf(source: unknown): Pick<ReceiverCard, 'state' | 'tone' | 'stateLabel' | 'listening' | 'error' | 'enabled'> {
  const record = isRecord(source) ? source : {};
  const listening = strings(record.listening);
  const error = text(record.error);
  const enabled = record.enabled === true;
  // Only an explicit `false` is "not built": the syslog receiver has no such
  // field at all, and that is not an absence.
  if (record.implemented === false) {
    return { state: 'absent', tone: 'idle', stateLabel: 'Not in this build', listening: [], error, enabled: false };
  }
  if (!enabled) {
    return { state: 'off', tone: 'idle', stateLabel: 'Switched off', listening, error, enabled };
  }
  if (listening.length === 0) {
    return { state: 'failed', tone: 'crit', stateLabel: 'Not listening', listening, error, enabled };
  }
  return { state: 'listening', tone: 'ok', stateLabel: 'Listening', listening, error, enabled };
}

function lastAt(source: unknown): number | null | undefined {
  if (!isRecord(source) || !('last_at' in source)) return undefined;
  return typeof source.last_at === 'number' ? source.last_at : null;
}

const UNAUTHENTICATED_TRAPS =
  'SNMPv1 and v2c traps are unauthenticated UDP: whoever can reach this port can forge one, and the community travels in the clear. Nothing derived from a trap ever changes a device.';

/**
 * What the trap receiver has and has not met: the specification's own
 * statement (`docs/dev/features/logs.md`, "Not verified").
 */
export const TRAPS_NOT_VERIFIED =
  'Not verified against a device: this receiver has only met net-snmp’s snmptrap and snmpinform (5.9.4), never a switch or a router. What a device puts in its traps beyond the standard bindings, and how it repeats an inform, is untested.';

const UNAUTHENTICATED_FLOWS =
  'Flow export is unauthenticated UDP: whoever can reach this port can report traffic that never was, from any address. Nothing derived from a flow changes a device or raises an alert.';

/** The three cards, in the order the status gives them. */
export function receiverCards(status: LogsStatus): ReceiverCard[] {
  const { syslog, traps, flows } = status as { syslog: unknown; traps: unknown; flows: unknown };

  const syslogCard: ReceiverCard = {
    id: 'syslog',
    title: 'Syslog',
    what: 'log lines over UDP and TCP',
    ...stateOf(syslog),
    counters: present([
      counter(syslog, 'received', 'Received', { unit: 'lines', hint: 'Lines taken off a socket.' }),
      counter(syslog, 'stored', 'Stored', { unit: 'lines' }),
      counter(syslog, 'dropped', 'Dropped', {
        unit: 'lines',
        attention: true,
        hint: 'Deliberately not kept: the queue was full, or the source is unknown and unknown sources are dropped.',
      }),
      counter(syslog, 'queued', 'Waiting to be written', { unit: 'lines' }),
      queueBytes(syslog, 'lines'),
      counter(syslog, 'rate_per_minute', 'In the last minute', { unit: 'lines' }),
      counter(syslog, 'from_lab', 'Handed over by labs', {
        unit: 'lines',
        hint: 'Lines simulated devices wrote. They came off no socket, so they are counted apart from Received.',
      }),
    ]),
    drops: [],
    memory: [],
    lastAt: lastAt(syslog),
    lab: null,
    caution: null,
  };

  const trapState = stateOf(traps);
  const trapCard: ReceiverCard = {
    id: 'traps',
    title: 'SNMP traps',
    what: 'SNMP notifications over UDP',
    ...trapState,
    counters: present([
      counter(traps, 'received', 'Received', { unit: 'datagrams', hint: 'Every datagram taken off the socket ends in exactly one of the counters after this one.' }),
      counter(traps, 'stored', 'Stored'),
      counter(traps, 'dropped', 'Dropped', {
        attention: true,
        hint: 'The queue was full, or the database would not take it.',
      }),
      counter(traps, 'rejected', 'Rejected', {
        attention: true,
        hint: 'A wrong community, no community stored for the device, or a source no device claims. The addresses and reasons are listed below.',
      }),
      counter(traps, 'unauthenticated', 'Unauthenticated', {
        attention: true,
        hint: 'SNMPv3 notifications that need authentication or privacy. Verifying them is not built, so they are counted and not stored as if valid.',
      }),
      counter(traps, 'malformed', 'Malformed', {
        attention: true,
        hint: 'Datagrams that are not a notification at all.',
      }),
      counter(traps, 'duplicates', 'Repeated informs', {
        hint: 'The same inform arriving again: answered again, stored once.',
      }),
      counter(traps, 'acknowledged', 'Informs acknowledged', {
        hint: 'An inform is acknowledged after it is stored, never before.',
      }),
      counter(traps, 'queued', 'Waiting to be written'),
      queueBytes(traps, 'notifications'),
      counter(traps, 'rate_per_minute', 'In the last minute', { unit: 'datagrams' }),
    ]),
    drops: [],
    memory: [],
    lastAt: lastAt(traps),
    lab: isRecord(traps) ? text(traps.lab) : null,
    // Said where the receiver is switched on, which is when it applies.
    caution: trapState.enabled ? UNAUTHENTICATED_TRAPS : null,
  };

  const flowRecord = isRecord(flows) ? flows : {};
  const flowState = stateOf(flows);
  const flowCard: ReceiverCard = {
    id: 'flows',
    title: 'Flows',
    what: 'NetFlow v5, v9 and IPFIX over UDP (no sFlow)',
    ...flowState,
    counters: present([
      counter(flows, 'received', 'Received', { unit: 'datagrams' }),
      counter(flows, 'malformed', 'Malformed', {
        unit: 'datagrams',
        attention: true,
        hint: 'Datagrams that are not a flow export. They make nobody an exporter.',
      }),
      counter(flows, 'records', 'Decoded', {
        unit: 'flow records',
        hint: 'Every record decoded ends in Counted or Dropped, or is still waiting.',
      }),
      counter(flows, 'counted', 'Counted', { unit: 'flow records', hint: 'In the buckets the Flows pane reads.' }),
      counter(flows, 'dropped', 'Dropped', { unit: 'flow records', attention: true }),
      counter(flows, 'queued', 'Waiting to be counted', { unit: 'flow records' }),
      queueBytes(flows, 'flow records'),
      counter(flows, 'sets_without_template', 'Sets without a template', {
        unit: 'data sets',
        attention: true,
        hint: 'Data that arrived before its template. What it held is in no number: it could not be read, and nothing is guessed.',
      }),
      counter(flows, 'exporters', 'Exporters followed'),
      counter(flows, 'unattributed_exporters', 'Exporters no device claims', { attention: true }),
      counter(flows, 'rate_per_minute', 'In the last minute', { unit: 'datagrams' }),
    ]),
    drops: present([
      counter(flowRecord.drops, 'queue_full', 'The queue was full'),
      counter(flowRecord.drops, 'no_room', 'No room to follow one more exporter'),
      counter(flowRecord.drops, 'unclaimed', 'No device claims the exporter'),
      counter(flowRecord.drops, 'unreadable', 'The inventory could not be read'),
    ]),
    memory: present([
      counter(flowRecord.memory, 'minutes', 'One-minute buckets held'),
      counter(flowRecord.memory, 'keys', 'Keys held'),
      counter(flowRecord.memory, 'max_keys', 'Ceiling on keys'),
      counter(flowRecord.memory, 'evicted_minutes', 'Buckets let go early', {
        attention: true,
        hint: 'To stay under the ceiling the oldest minute went first, so the memory reaches back less than a day.',
      }),
    ]),
    lastAt: lastAt(flows),
    oldestAt: isRecord(flowRecord.memory) && typeof flowRecord.memory.oldest_at === 'number'
      ? flowRecord.memory.oldest_at
      : null,
    lab: text(flowRecord.lab),
    caution: flowState.enabled ? UNAUTHENTICATED_FLOWS : null,
  };

  return [syslogCard, trapCard, flowCard];
}

// ---------------------------------------------------------------- the settings

/** One editable text field of a receiver's settings. */
export interface SettingField {
  /** The key inside the receiver's settings block. */
  key: 'udp_port' | 'tcp_port' | 'port' | 'bind';
  label: string;
  hint: string;
  /** A port, or the address to bind. */
  kind: 'port' | 'address';
  /** Whether 0 means something (the syslog TCP listener: off). */
  zero?: string;
}

const BIND_HINT = '0.0.0.0 listens on every interface of this machine.';

export const SETTING_FIELDS: Record<ReceiverId, readonly SettingField[]> = {
  syslog: [
    { key: 'udp_port', label: 'UDP port', kind: 'port', hint: 'The standard port, 514, needs privileges on Unix.' },
    { key: 'tcp_port', label: 'TCP port', kind: 'port', hint: '0 turns the TCP listener off.', zero: 'TCP off' },
    { key: 'bind', label: 'Listen on', kind: 'address', hint: BIND_HINT },
  ],
  traps: [
    { key: 'port', label: 'UDP port', kind: 'port', hint: 'The standard port, 162, needs privileges on Unix.' },
    { key: 'bind', label: 'Listen on', kind: 'address', hint: BIND_HINT },
  ],
  flows: [
    { key: 'port', label: 'UDP port', kind: 'port', hint: 'Exporters send to 2055 by custom.' },
    { key: 'bind', label: 'Listen on', kind: 'address', hint: BIND_HINT },
  ],
};

/** What the form holds while it is being typed in: text, one entry per field. */
export type SettingDraft = Partial<Record<SettingField['key'], string>>;

/** The stored settings of one receiver as the form's text. */
export function draftOf(id: ReceiverId, settings: LogsSettings): SettingDraft {
  const block = settings[id] as unknown as Record<string, unknown>;
  const draft: SettingDraft = {};
  for (const field of SETTING_FIELDS[id]) {
    const value = block[field.key];
    draft[field.key] = typeof value === 'number' || typeof value === 'string' ? String(value) : '';
  }
  return draft;
}

export type Parsed<T> = { ok: true; value: T } | { ok: false; message: string };

/**
 * A port as typed. Whole numbers only, and inside what a port can be: the
 * engine's field is sixteen bits wide and a value that does not fit would
 * make the whole `logs` section unreadable.
 */
export function parsePort(typed: string, { zero = false }: { zero?: boolean } = {}): Parsed<number> {
  const trimmed = typed.trim();
  if (!/^\d+$/.test(trimmed)) return { ok: false, message: 'A port is a whole number.' };
  const port = Number(trimmed);
  if (port > 65_535) return { ok: false, message: 'A port is at most 65535.' };
  if (port === 0 && !zero) return { ok: false, message: 'A port is between 1 and 65535.' };
  return { ok: true, value: port };
}

export interface SettingChange {
  /** What to hand to `settings.set` as `value`: only the keys that changed. */
  patch: Record<string, Record<string, number | string | boolean>> | null;
  /** What is wrong with each field, by key. Empty when the draft can be sent. */
  errors: Partial<Record<SettingField['key'], string>>;
}

/**
 * The difference between what is typed and what is stored, as a patch.
 * `settings.set` merges, so a patch of one key leaves every other alone —
 * and a receiver is only rebound when its port or its address changed.
 */
export function settingChange(id: ReceiverId, draft: SettingDraft, settings: LogsSettings): SettingChange {
  const block = settings[id] as unknown as Record<string, unknown>;
  const changed: Record<string, number | string> = {};
  const errors: SettingChange['errors'] = {};
  for (const field of SETTING_FIELDS[id]) {
    const typed = draft[field.key] ?? '';
    // What is stored is not this form's to object to: only what was typed
    // over it is checked. (A stored port of 0 is "any free port", which the
    // engine takes and reports; the form does not offer it, and does not
    // complain about finding it either.)
    if (typed.trim() === String(block[field.key] ?? '')) continue;
    if (field.kind === 'port') {
      const parsed = parsePort(typed, { zero: field.zero !== undefined });
      if (!parsed.ok) errors[field.key] = parsed.message;
      else if (parsed.value !== block[field.key]) changed[field.key] = parsed.value;
    } else {
      const address = typed.trim();
      // Whether it is an address this machine has is the engine's to say,
      // and it does, in the card's error. An empty one says nothing at all.
      if (address === '') errors[field.key] = 'Give an address to listen on.';
      else if (address !== block[field.key]) changed[field.key] = address;
    }
  }
  const sendable = Object.keys(errors).length === 0 && Object.keys(changed).length > 0;
  return { patch: sendable ? { [id]: changed } : null, errors };
}

/** The patch that switches one receiver on or off and touches nothing else. */
export function enablePatch(id: ReceiverId, enabled: boolean): Record<string, { enabled: boolean }> {
  return { [id]: { enabled } };
}

/**
 * The port a card names while its receiver is off: what the settings would
 * bind. `null` when the settings have not been read (or cannot be).
 */
export function wouldBind(id: ReceiverId, settings: LogsSettings | undefined): string | null {
  if (!settings) return null;
  if (id === 'syslog') {
    const { udp_port: udp, tcp_port: tcp, bind } = settings.syslog;
    return `UDP ${udp}${tcp === 0 ? '' : ` and TCP ${tcp}`} on ${bind}`;
  }
  const { port, bind } = settings[id];
  return `UDP ${port} on ${bind}`;
}
