/**
 * Logs (`nocturne-logs`): the syslog receiver, log search, the live tail,
 * pattern rules and a device's own log buffer.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and a field the backend declares as optional is written
 * `T | null` because it is present in the JSON with a `null` value. Backend
 * source: `crates/nocturne-logs`. Specification:
 * `docs/dev/features/logs.md`.
 *
 * Four conventions run through the whole surface:
 *
 * * **Severity counts down.** It is syslog's own scale: 0 is an emergency
 *   and 7 is debug, so `max_severity: 3` means "error and worse". The field
 *   is `null` when neither the message's priority nor the vendor's mnemonic
 *   said anything, which is not the same as `debug`.
 * * **`ts` is when Nocturne received the line**, not when the device says it
 *   wrote it: a device's clock is its own business and often wrong. What the
 *   device wrote is in `body.timestamp`, as text. The exception is a pulled
 *   buffer (`body.via === 'pull'`), where the device's date is used when it
 *   could be read — `body.ts_source` says which it is.
 * * **`device_id` is `null` when attribution refused.** The line is still
 *   stored; `logs.sources` says which address it came from and why no device
 *   could be named. A missing device is never guessed from a host name.
 * * **A number that was not measured is not reported as zero.** What the
 *   receiver dropped is counted and shown (`ReceiverStatus.dropped`); what
 *   the operating system dropped before the process saw it cannot be
 *   counted and is not pretended to be.
 */

import type { LogEntry } from './core';

// ---------------------------------------------------------------- searching

/**
 * What a search is looking for. Every field is optional, and the same shape
 * is used by `logs.query`, `logs.histogram`, `logs.top`, `logs.bursts` and
 * `logs.follow` — "follow what I am looking at" is the same filter.
 */
export interface LogFilter {
  /** `syslog` (the default), `audit`, `event`; `*` or `''` for every stream. */
  stream?: string;
  device_id?: string;
  /** Several devices at once (a site, a selection on screen). */
  device_ids?: string[];
  /** Keep entries at least this severe: numerically ≤, so 3 is "error and worse". */
  max_severity?: number;
  /** Syslog facility number, 0…23. */
  facility?: number;
  /** Vendor mnemonic, e.g. `LINK-3-UPDOWN`. A trailing `*` matches a prefix. */
  mnemonic?: string;
  /** An interface of the attributed device, in any spelling (`Gi1/0/3`). */
  interface?: string;
  /** The sending address, exactly. */
  source?: string;
  /**
   * Words that must appear in the message, the sending address or the
   * mnemonic. A word with a leading `-` must not appear; `"two words"` is
   * one phrase. At most sixteen terms.
   */
  text?: string;
  since?: number;
  until?: number;
  /** `true`: only lines no device could be attributed to. */
  unattributed?: boolean;
}

export interface LogQueryParams extends LogFilter {
  /** Page backwards: only entries with a smaller id. */
  before_id?: number;
  /** Only entries with a larger id (polling forwards). */
  after_id?: number;
  /** Default 200, at most 1000. */
  limit?: number;
}

export interface LogPage {
  /** Newest first. */
  entries: LogEntry[];
  has_more: boolean;
}

/**
 * The structured half of a syslog entry, as `LogEntry.body` carries it.
 * Every field is absent when the line did not say — including `raw` and
 * `via`: a lab's simulated devices write their lines into the same table
 * themselves (`crates/nocturne-lab/src/runtime.rs`, `{line, lab_id,
 * sim_time}`), without going through this feature's ingest, so a reader must
 * cope with a body that has neither (see `pages/logs/entry.ts`).
 */
export interface SyslogBody {
  /** The line as received, redacted. Absent on a line a lab device wrote. */
  raw?: string;
  /** `syslog` for a line a device sent, `pull` for one read from its buffer. */
  via?: 'syslog' | 'pull';
  /** A lab device's own line, when it wrote one instead of `raw`. */
  line?: string;
  /** The lab a simulated device's line came from. */
  lab_id?: string;
  facility?: number;
  /** `local7`, `daemon`, … */
  facility_name?: string;
  /** `error`, `notice`, … */
  severity_name?: string;
  /** The process or application that logged it. */
  app?: string;
  /** The host name the sender put in the message (not the inventory's). */
  host?: string;
  procid?: string;
  msgid?: string;
  /** The timestamp exactly as the device wrote it. */
  timestamp?: string;
  /** `LINK-3-UPDOWN`, `system-6-account`: facility, severity and name. */
  mnemonic?: string;
  /** The device's own counter; gaps mean lost messages. */
  sequence?: number;
  /** An interface of this device the line is about, canonically spelled. */
  interface?: string;
  /** How the line spelled it, when that differs. */
  interface_text?: string;
  /** Pulled lines only: whether `ts` came from the device or from the pull. */
  ts_source?: 'device' | 'received';
}

export interface HistogramParams extends LogFilter {
  /** Defaults to an hour ago. */
  since?: number;
  /** Defaults to now. */
  until?: number;
  /** Default 60, at most 1000. */
  buckets?: number;
}

export interface Bucket {
  /** Start of the bucket. */
  t: number;
  count: number;
  /** Counts by severity, index 0 (emergency) … 7 (debug). */
  by_severity: number[];
  /** Entries with no severity at all; included in `count`. */
  no_severity: number;
}

export type TopBy = 'device' | 'source' | 'severity' | 'mnemonic' | 'facility' | 'interface';

export interface TopParams extends LogFilter {
  /** Default `device`. */
  by?: TopBy;
  /** Default 20, at most 500. */
  limit?: number;
}

export interface TopRow {
  key: string;
  /** A readable label when the key is an id or a number; otherwise empty. */
  label: string;
  count: number;
}

export interface BurstParams extends LogFilter {
  /** Default 5. */
  min_count?: number;
  /** Default 50. */
  limit?: number;
}

/** A run of similar lines: what a screen shows instead of four hundred rows. */
export interface Burst {
  device_id?: string | null;
  /** The device's name, empty when unattributed. */
  device: string;
  mnemonic?: string | null;
  interface?: string | null;
  severity?: number | null;
  count: number;
  first_at: number;
  last_at: number;
  /** The first line of the burst, as an example. */
  example: string;
  first_id: number;
  last_id: number;
}

export interface ClearParams {
  /** Default `syslog`. One stream at a time; `*` is refused. */
  stream?: string;
  /** Only entries older than this; defaults to now (everything). */
  before?: number;
}

export interface Cleared {
  removed: number;
  stream: string;
}

/** `logs.query` {@link LogQueryParams} → {@link LogPage} */
/** `logs.histogram` {@link HistogramParams} → {@link Bucket}[] */
/** `logs.top` {@link TopParams} → {@link TopRow}[] */
/** `logs.bursts` {@link BurstParams} → {@link Burst}[] */
/** `logs.clear` {@link ClearParams} → {@link Cleared} */

// ---------------------------------------------------------------- receivers

export interface ReceiverStatus {
  enabled: boolean;
  /** What is actually bound, e.g. `['udp 0.0.0.0:5514', 'tcp 0.0.0.0:5514']`. */
  listening: string[];
  /** Why it is not listening although it is enabled (a port in use, …). */
  error?: string | null;
  /** Counted since the application started, not since forever. */
  received: number;
  stored: number;
  /** Deliberately not kept: the queue was full, or the source is unknown. */
  dropped: number;
  /** Lines waiting to be written right now. */
  queued: number;
  rate_per_minute: number;
  last_at?: number | null;
}

/**
 * A receiver the specification describes that this build does not have. It
 * is reported rather than left out, and never claims to be running.
 */
export interface NotBuilt {
  enabled: false;
  implemented: false;
  error: string;
}

export interface StoreStatus {
  rows: number;
  oldest_at?: number | null;
  retention_days: number;
  max_rows: number;
  last_prune_at?: number | null;
  pruned_by_age: number;
  pruned_by_rows: number;
}

export interface LogsStatus {
  syslog: ReceiverStatus;
  /** Not implemented in this build. */
  traps: NotBuilt;
  /** Not implemented in this build. */
  flows: NotBuilt;
  store: StoreStatus;
  /** Addresses that have sent and that no device in the inventory claims. */
  unattributed_sources: number;
}

/** One address that has sent lines. */
export interface LogSource {
  address: string;
  device_id?: string | null;
  /** Why the address is not attributed; empty when it is. */
  reason: string;
  first_at: number;
  last_at: number;
  count: number;
  /** Lines from this address received and deliberately not kept. */
  dropped: number;
  /** The device's name, when it is attributed. */
  device: string;
}

/** `logs.status` {} → {@link LogsStatus} */
/** `logs.sources` {} → {@link LogSource}[] */
/** `logs.restart` {} → {@link LogsStatus} (re-reads the settings and rebinds) */

// --------------------------------------------------------------- live tail

export interface FollowParams extends LogFilter {
  /** Renew an existing follow instead of opening another. */
  follow_id?: string;
}

export interface Follow {
  follow_id: string;
  /** Renew before this, or the follow is dropped. */
  expires_at: number;
  /** The event topic its batches arrive on: `logs.tail`. */
  event: string;
}

export interface Unfollowed {
  closed: boolean;
}

/** `logs.follow` {@link FollowParams} → {@link Follow} */
/** `logs.unfollow` `{id}` → {@link Unfollowed} */

// -------------------------------------------------------------- correlation

/** Whether a list borrowed from another feature could be filled. */
export type SourceState = 'ok' | 'unavailable' | 'failed';

/**
 * A list this feature asked another feature for. `state: 'unavailable'`
 * means that feature is not in this build — which is not the same as
 * "nothing happened", and is why the state is here at all.
 */
export interface Borrowed {
  state: SourceState;
  items: unknown[];
  note: string;
}

export interface AroundParams {
  /** Centre the window on this entry (and take its device from it). */
  entry_id?: number;
  /** Or on this moment. Defaults to now. */
  at?: number;
  device_id?: string;
  /** Half-width of the window in seconds; default 300. */
  window_s?: number;
  /** Entries returned; default 100. */
  limit?: number;
}

export interface Around {
  center?: LogEntry | null;
  since: number;
  until: number;
  device_id?: string | null;
  /** The interface the centre line was about, when it named one. */
  interface?: string | null;
  entries: LogEntry[];
  has_more: boolean;
  bursts: Burst[];
  /** Signals this feature's own rules raised in the window. */
  signals: LogEntry[];
  /** From `alerts.list`, when the monitoring feature is there. */
  alerts: Borrowed;
  /** From `changes.list`, when the change pipeline is there. */
  changes: Borrowed;
}

/** `logs.around` {@link AroundParams} → {@link Around} */

// -------------------------------------------------------------------- rules

export type SignalKind =
  | 'config_changed'
  | 'interface_down'
  | 'interface_up'
  | 'login_failure'
  | 'reload'
  | 'alert';

/** What a rule raises when it matches. Discriminated by `kind`. */
export type Signal =
  | { kind: 'config_changed' }
  | { kind: 'interface_down'; interface_group: string }
  | { kind: 'interface_up'; interface_group: string }
  | { kind: 'login_failure' }
  | { kind: 'reload' }
  | {
      kind: 'alert';
      /** `info` | `low` | `medium` | `high` | `critical`. */
      severity: string;
      /** May use `{group}` placeholders from the pattern. */
      title: string;
    };

/** Which devices a rule applies to. Every list empty = all of them. */
export interface RuleScope {
  device_ids: string[];
  sites: string[];
  roles: string[];
  tags: string[];
  /** Device profile ids (`cisco_ios`, `mikrotik_routeros`). */
  profiles: string[];
}

export interface LogRule {
  id: string;
  name: string;
  enabled: boolean;
  /** Ships with the application: can be switched off, never deleted, and
   * its pattern, name and signal cannot be edited. */
  builtin: boolean;
  /** Which stream the rule reads; only `syslog` is produced today. */
  stream: string;
  /** A regular expression over the line as the device wrote it. */
  pattern: string;
  /** Only lines at least this severe (numerically ≤). */
  min_severity?: number | null;
  scope: RuleScope;
  emit: Signal;
  /** At most one signal per device per rule per window; the rest are counted. */
  throttle_s: number;
  /** A line the rule is meant to match. */
  test_line: string;
  /** Where that line came from. */
  test_line_source: string;
  updated_at: number;
}

export interface SignalPreview {
  kind: SignalKind;
  title: string;
  severity?: string | null;
  interface?: string | null;
}

export interface RuleTestParams {
  rule: LogRule;
  line: string;
}

export interface RuleTest {
  matched: boolean;
  /** Why it did not match, when it did not. */
  reason: string;
  /** Named groups the pattern captured. */
  groups: Record<string, string>;
  signal?: SignalPreview | null;
  /** How the pasted line itself was understood. */
  parsed?: {
    severity?: number | null;
    facility?: number | null;
    mnemonic?: string | null;
    host?: string | null;
    message: string;
  } | null;
}

export interface RuleDeleted {
  deleted: string;
}

/** `logs.rules.list` {} → {@link LogRule}[] */
/** `logs.rules.save` {@link LogRule} → {@link LogRule} */
/** `logs.rules.delete` `{id}` → {@link RuleDeleted} */
/** `logs.rules.test` {@link RuleTestParams} → {@link RuleTest} */

// ------------------------------------------------------------------ devices

export interface Pulled {
  device_id: string;
  /** The command that was sent (`show logging`, `/log print …`). */
  command: string;
  /** Lines of the buffer that look like log lines. */
  read: number;
  stored: number;
  /** Lines already stored by an earlier pull. */
  duplicates: number;
  /** Lines whose time came from the device rather than from the pull. */
  dated_by_device: number;
  oldest_at?: number | null;
  newest_at?: number | null;
  /** Set when there was nothing to read, and why. */
  note: string;
}

export interface DeviceSetup {
  device_id: string;
  profile: string;
  /** The configuration lines, in order. **Not applied** — hand them to
   * `changes.create`. */
  lines: string[];
  /** The address the device is told to send to. */
  address: string;
  /** How that address was worked out. */
  address_note: string;
  udp_port: number;
  note: string;
}

/** `logs.pull` `{device_id}` → {@link Pulled} */
/** `logs.device_setup` `{device_id}` → {@link DeviceSetup} */

// ------------------------------------------------------------------- events

/** `logs.appended`: the live tail, at most four a second. */
export interface LogsAppended {
  /** At most 200, and at most 64 KiB of log text — a single line can be
   * 64 KiB on its own, so the newest that fit are sent and the rest counted. */
  entries: LogEntry[];
  /** Lines left out of this event (sampled away or over the buffer). */
  dropped: number;
  /** `true` when `entries` is an even sample of the window, not all of it. */
  sampled: boolean;
  /** Lines the window actually held. */
  received: number;
}

/** `logs.tail`: the same, for one `logs.follow` and its filter. */
export interface LogsTail extends LogsAppended {
  follow_id: string;
}

/** `logs.signal`: a rule matched. */
export interface LogsSignal {
  rule_id: string;
  rule: string;
  kind: SignalKind;
  device_id?: string | null;
  device: string;
  severity?: string | null;
  title: string;
  interface?: string | null;
  groups: Record<string, string>;
  entry_id: number;
  /** Matches in this throttle window, this one included. */
  count: number;
  at: number;
}

/** `logs.config_changed`: a device's configuration was changed. */
export interface LogsConfigChanged {
  device_id?: string | null;
  /** Who, when the line says. */
  by?: string | null;
  /** From where (`console`, `vty0`, an address), when the line says. */
  from?: string | null;
  entry_id: number;
  at: number;
}

/**
 * Events
 *
 * | Topic | Payload |
 * |---|---|
 * | `logs.appended` | {@link LogsAppended} |
 * | `logs.tail` | {@link LogsTail} |
 * | `logs.status_changed` | {@link LogsStatus} |
 * | `logs.signal` | {@link LogsSignal} |
 * | `logs.config_changed` | {@link LogsConfigChanged} |
 */
