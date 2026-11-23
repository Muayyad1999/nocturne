/**
 * Logs (`nocturne-logs`): the syslog receiver, the SNMP trap receiver and the
 * flow collector, log search, the live tail, pattern rules with thresholds
 * and clearing, forwarding to a log platform, and a device's own log buffer.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and a field the backend declares as optional is written
 * `T | null` because it is present in the JSON with a `null` value. Backend
 * source: `crates/nocturne-logs`. Specification:
 * `docs/dev/features/logs.md`. Every shape here was checked field by field
 * against a running `nocturne-server` (real datagrams into the real
 * receivers, a lab's lines through `logs.ingest`), not written from the
 * specification alone.
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
 * * **A secret is write-only.** A trap's community is never in anything
 *   returned (`<community>` stands where one was); a forwarder's URL and
 *   token are parameters of `logs.forward.save` and of nothing else.
 */

import type { LogEntry } from './core';

// ---------------------------------------------------------------- searching

/**
 * What a search is looking for. Every field is optional, and the same shape
 * is used by `logs.query`, `logs.histogram`, `logs.top`, `logs.bursts` and
 * `logs.follow` — "follow what I am looking at" is the same filter.
 */
export interface LogFilter {
  /** `syslog` (the default), `trap`, `audit`, `event`; `*` or `''` for every stream. */
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
 * Every field is absent when the line did not say. A simulated device's
 * line is handed over by the lab (`logs.ingest`) and takes the same path as
 * a datagram, so it has the same body with `via: 'lab'` — and an empty
 * `LogEntry.source`, because it was sent from no address. Only a build
 * without this feature's ingest writes the older `{line, lab_id, sim_time}`
 * body, which is why a reader still copes with a body that has no `raw`
 * (see `pages/logs/entry.ts`).
 */
export interface SyslogBody {
  /** The line as received, redacted. */
  raw?: string;
  /**
   * `syslog` for a line a device sent, `pull` for one read from its buffer,
   * `lab` for one a simulated device wrote and the lab handed over.
   */
  via?: 'syslog' | 'pull' | 'lab';
  /** A lab device's own line, in a build where the lab wrote it itself. */
  line?: string;
  /** The lab a simulated device's line came from, in that same case. */
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

/** One binding of a trap, as the receiver wrote it down. */
export interface TrapVarbind {
  /** The object's OID with its instance, dotted. */
  oid: string;
  /** `ifIndex.3`; `null` when the dictionary does not know the object. */
  name: string | null;
  /**
   * The value as text: `3`, `up(1)`, `<community>` where a community stood,
   * `<secret:n>` where the redactor found a secret.
   */
  value: string;
  /** The interface an `ifIndex` binding names, when the monitor's map has it. */
  interface?: string;
}

/**
 * The body of an entry of the `trap` stream. It is built field by field by
 * the receiver — never the decoded datagram, which carries the community —
 * and a field with nothing to say is absent. A trap has **no severity**:
 * `LogEntry.severity` is `null` and nothing here stands in for one.
 */
export interface TrapBody {
  version: 'v1' | 'v2c' | 'v3';
  /** The notification's name (`linkDown`); `null` when it has none here. */
  trap: string | null;
  /** The notification's OID, dotted; `null` when the datagram gave none. */
  oid: string | null;
  /** The sender's uptime in seconds, when it said. */
  uptime?: number;
  /** Present (and `true`) for an InformRequest. */
  inform?: true;
  /** SNMPv1 only. */
  enterprise?: string;
  generic?: number;
  specific?: number;
  /**
   * SNMPv1 only: the address the agent **claims**. It never decides the
   * device; `LogEntry.source` is where the datagram really came from.
   */
  agent_addr?: string;
  /** At most 64, in the order they were sent. */
  varbinds: TrapVarbind[];
  /** Bindings the datagram held that did not fit in `varbinds`. */
  varbinds_omitted?: number;
  /** The ifIndex the notification is about, when it names one. */
  if_index?: number;
  /** That interface's name, when the monitor's index map has it. */
  interface?: string;
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
  /**
   * What the lines waiting and the batch being written take up, in bytes.
   * Absent from a build whose queues are bounded in lines only.
   */
  queued_bytes?: number;
  /** The byte budget of the queue, as the receiver applies it (`queue_limit_bytes`). */
  max_queued_bytes?: number;
  rate_per_minute: number;
  last_at?: number | null;
  /**
   * Lines simulated devices wrote and the lab handed over. Counted apart
   * from `received`, which is what came off a socket.
   */
  from_lab: number;
}

/** Why the trap receiver refused what one address sent. */
export interface Rejection {
  address: string;
  /** The device the address belongs to, when one does. */
  device_id?: string | null;
  /** The engine's sentence: "the community is not the one stored for `SW1`". */
  reason: string;
  count: number;
  last_at: number;
}

/**
 * The SNMP trap receiver. Every datagram taken off the socket (`received`)
 * ends in exactly one of `stored`, `dropped`, `rejected`, `unauthenticated`,
 * `malformed` and `duplicates`.
 */
export interface TrapStatus {
  enabled: boolean;
  /** `false` only in a build that has no trap receiver. */
  implemented: boolean;
  /** `['udp 0.0.0.0:1162']` when bound. */
  listening: string[];
  /** Why it is not listening although it is enabled. */
  error?: string | null;
  received: number;
  stored: number;
  /** The queue was full (in notifications or in bytes), or the database would not take it. */
  dropped: number;
  queued: number;
  /** What the notifications waiting take up, in bytes; absent from an older build. */
  queued_bytes?: number;
  /** The byte budget of the queue; absent from an older build. */
  max_queued_bytes?: number;
  rate_per_minute: number;
  last_at?: number | null;
  /** Wrong community, no community stored, or an unknown source. */
  rejected: number;
  /** SNMPv3 notifications that need authentication: counted, never stored. */
  unauthenticated: number;
  /** Datagrams that are not a notification at all. */
  malformed: number;
  /** An inform that arrived again: answered again, stored once. */
  duplicates: number;
  /** Informs that were acknowledged (after they were stored). */
  acknowledged: number;
  require_known_community: boolean;
  /** The most recent 20 addresses that were refused, newest first. */
  rejections: Rejection[];
  /** A sentence: a simulated device sends no traps. */
  lab: string;
}

/** Flow records the collector decoded and did not count, by reason. */
export interface FlowDrops {
  /** The queue between the socket and the buckets was full. */
  queue_full: number;
  /** 256 exporters were already followed. */
  no_room: number;
  /** No device claims the exporter and `unknown_sources` is `drop`. */
  unclaimed: number;
  /** The inventory could not be read. */
  unreadable: number;
}

/** What the flow buckets hold in memory. Nothing of a flow is stored. */
export interface FlowMemory {
  /** One-minute buckets over all exporters. */
  minutes: number;
  keys: number;
  /** The ceiling on `keys`. */
  max_keys: number;
  /** Buckets let go early to stay under the ceiling. */
  evicted_minutes: number;
  /** The start of the oldest bucket; `null` when there is none. */
  oldest_at?: number | null;
}

/**
 * The flow collector. It counts in **two units**: `received`, `malformed`
 * and `rate_per_minute` are datagrams, everything else is flow records.
 * Every record decoded (`records`) ends in `counted` or `dropped` (the sum
 * of `drops`), or is still `queued`. It has no `stored`: no flow is.
 */
export interface FlowStatus {
  enabled: boolean;
  /** `false` only in a build that has no flow collector. */
  implemented: boolean;
  listening: string[];
  error?: string | null;
  /** Datagrams. */
  received: number;
  /** Datagrams that are not a flow export. */
  malformed: number;
  /** Flow records decoded. */
  records: number;
  /** Flow records that are in the buckets. */
  counted: number;
  /** Flow records not counted; `drops` says why. */
  dropped: number;
  drops: FlowDrops;
  queued: number;
  /** What the flow records waiting take up, in bytes; absent from an older build. */
  queued_bytes?: number;
  /** The byte budget of the queue; absent from an older build. */
  max_queued_bytes?: number;
  /** Datagrams from followed exporters in the last minute. */
  rate_per_minute: number;
  last_at?: number | null;
  /** Data sets that arrived before their template: what they held is in no number. */
  sets_without_template: number;
  exporters: number;
  unattributed_exporters: number;
  memory: FlowMemory;
  /** A sentence: a simulated device exports no flows. */
  lab: string;
}

/** What the pattern rules hold in memory. */
export interface RuleStatus {
  /** (device, instance) keys the rules are counting or throttling. */
  tracked: number;
  /** Keys let go because a rule reached `max_per_rule`. */
  forgotten: number;
  max_per_rule: number;
  /** Matches held back because one device spoke for too many instances at once. */
  held_back: number;
  /** Alerts a rule is waiting to clear, by its opposite or its quiet period. */
  open_alerts: number;
  /** Calls to the monitor that were dropped because its queue was full. */
  undelivered: number;
}

/** `disabled`; `idle`: on, nothing sent or tried yet; `ok`; `failing`. */
export type ForwarderState = 'disabled' | 'idle' | 'ok' | 'failing';

/** What one forwarder has done since the application started. */
export interface ForwarderStatus {
  id: string;
  name: string;
  state: ForwarderState;
  /**
   * Handed to the network. No syslog transport acknowledges anything, so
   * for those it does not mean "arrived"; for `https_json` it is what the
   * collector answered 2xx to.
   */
  sent: number;
  /** Never left: the queue was full, every attempt failed, or it was saved again. */
  dropped: number;
  /**
   * Entries waiting to be sent right now, not counting the batch being sent.
   * This and the two below are absent from an older build.
   */
  queued?: number;
  /** What the entries waiting and the batch being sent take up, in bytes. */
  queued_bytes?: number;
  /** What they may take up: the same for every forwarder. */
  max_queued_bytes?: number;
  /** How the last send failed, in the engine's words; never a URL or a token. */
  last_error?: string | null;
  /** When something was last sent. */
  last_at?: number | null;
}

export interface StoreStatus {
  /** What devices sent and is still stored: `syslog` and `trap` rows together. */
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
  traps: TrapStatus;
  flows: FlowStatus;
  rules: RuleStatus;
  /** Every forwarder, by name. */
  forwarders: ForwarderStatus[];
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

/**
 * The `logs` settings section, as `settings.get {section: 'logs'}` returns
 * it. `settings.set` merges, so one key can be sent on its own. Nothing in it
 * is a secret. Only `enabled`, a port and `bind` rebind a socket.
 */
export interface LogsSettings {
  syslog: { enabled: boolean; udp_port: number; tcp_port: number; bind: string };
  traps: { enabled: boolean; port: number; bind: string; require_known_community: boolean };
  flows: { enabled: boolean; port: number; bind: string };
  retention_days: number;
  max_rows: number;
  /** `store` or `drop`: what happens to what an address no device claims sends. */
  unknown_sources: string;
  /** What may wait to be written, per receiver. */
  queue_limit: number;
  /** And the bytes it may take up, per receiver (1 MiB to 1 GiB); absent from an older build. */
  queue_limit_bytes?: number;
}

/** `logs.status` {} → {@link LogsStatus} */
/** `logs.sources` {} → {@link LogSource}[] */
/**
 * `logs.restart` {} → {@link LogsStatus}: re-applies the receiver settings.
 * A receiver that could not bind is tried again; one that is running is left
 * alone.
 */

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

/** The streams a rule can read. */
export type RuleStream = 'syslog' | 'trap';

/**
 * "Eight failed logins from one address in a minute, not one." The lines are
 * counted per device and per value of the `group_by` captures; reaching
 * `count` inside `window_s` raises the signal and starts the count again.
 * The counts live in memory: a restart starts them again.
 */
export interface Threshold {
  /** 1 to 1,000. `1` raises on every line: "one instance per neighbour". */
  count: number;
  /** 1 to 86,400 seconds. */
  window_s: number;
  /** Named groups of the pattern. Their values together are the instance. */
  group_by: string[];
}

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
  /**
   * Ships with the application: never deleted, and `logs.rules.save` on it
   * changes its switch, its scope and its throttle and nothing else.
   */
  builtin: boolean;
  /**
   * The platform whose messages the pattern was written for, in words
   * (`Cisco IOS / IOS-XE`, `Any SNMP agent (SNMPv2-MIB, IF-MIB)`); empty
   * for a rule of one's own.
   */
  platform: string;
  /** Which stream the rule reads: `syslog` or `trap`. */
  stream: string;
  /**
   * A regular expression over the line as the device wrote it — for a trap,
   * over the message the receiver wrote for it (`linkDown ifIndex=3 …`).
   */
  pattern: string;
  /** Only lines at least this severe (numerically ≤). */
  min_severity?: number | null;
  scope: RuleScope;
  /** Raise only when this many matching lines arrive inside a window. */
  threshold?: Threshold | null;
  /**
   * Another rule's id: this one only counts a line while that rule's alert
   * is open for the same device and instance.
   */
  after?: string | null;
  /** The opposite event: a line matching it clears the alert. */
  clear_pattern?: string | null;
  /** Clear the alert after this long (1 s to 30 days) with no match. */
  clear_after_s?: number | null;
  emit: Signal;
  /** At most one signal per rule, device and instance per window; the rest are counted. */
  throttle_s: number;
  /** A line the rule is meant to match. */
  test_line: string;
  /**
   * Where that line came from: quoted from the vendor's documentation, or
   * `Synthetic: …` — written by hand to a documented format, or (a trap
   * rule) the message this receiver writes for a datagram net-snmp sent.
   * None is a line recorded from a device.
   */
  test_line_source: string;
  updated_at: number;
}

/**
 * What `logs.rules.save` takes: a {@link LogRule} in which everything has a
 * default. Without an `id` a rule is created. For a built-in rule only
 * `enabled`, `scope` and `throttle_s` are read. `invalid`, with the reason,
 * for a rule that cannot work.
 */
export type RuleSaveParams = Partial<Omit<LogRule, 'scope'>> & { scope?: Partial<RuleScope> };

export interface SignalPreview {
  kind: SignalKind;
  title: string;
  severity?: string | null;
  interface?: string | null;
}

export interface RuleTestParams {
  rule: RuleSaveParams;
  /** A pasted line — for a rule on the `trap` stream, a trap's message. */
  line: string;
}

export interface RuleTest {
  matched: boolean;
  /** Why it did not match, when it did not. */
  reason: string;
  /** Named groups the pattern captured. */
  groups: Record<string, string>;
  signal?: SignalPreview | null;
  /**
   * How the pasted line itself was understood. For a trap rule `mnemonic`
   * is the notification's name and there is no severity or facility.
   */
  parsed?: {
    severity?: number | null;
    facility?: number | null;
    mnemonic?: string | null;
    host?: string | null;
    message: string;
  } | null;
  /**
   * What the rule counts this line under: the interface, or the values of
   * the threshold's `group_by` captures. Empty = the whole device.
   */
  instance: string;
  /**
   * What else has to be true before the signal is raised, in a sentence (a
   * threshold wants more such lines, `after` wants another rule's alert
   * open). Empty when the line is enough.
   */
  note: string;
  /** The line is the rule's opposite event: it raises nothing and clears. */
  clears: boolean;
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
  /**
   * The lines that would point the device's SNMP notifications here. **Not
   * applied, and not complete**: where the community goes they say
   * `<snmp-community>`, because a community is a secret and is not returned.
   */
  trap_lines: string[];
  /** The port the trap receiver listens on (or would). */
  trap_port: number;
  /**
   * Says to put the device's own community there, whether the receiver
   * would keep what the device then sends (and why not, when not), and what
   * a platform that cannot be told a port does.
   */
  trap_note: string;
}

export interface IngestParams {
  /** A lab device's id, name or address. Hardware is refused (`forbidden`). */
  device_id: string;
  /** At most 10,000 a call. */
  lines: string[];
}

export interface Ingested {
  stored: number;
}

/** `logs.pull` `{device_id}` → {@link Pulled} */
/** `logs.ingest` {@link IngestParams} → {@link Ingested} — for the lab only */
/** `logs.device_setup` `{device_id}` → {@link DeviceSetup} */

// --------------------------------------------------------------- forwarding

export type ForwardKind = 'syslog_udp' | 'syslog_tcp' | 'syslog_tls' | 'https_json';

/** `https_json` is always `json`. */
export type ForwardFormat = 'rfc5424' | 'json';

/** What a forwarder can send. `audit` is opt-in, never a default. */
export type ForwardStream = 'syslog' | 'trap' | 'event' | 'audit';

/**
 * One destination everything that is stored is handed on to. **Never a URL
 * or a token**: for `https_json` those live in the vault, and `has_secret`
 * is all that is said about them.
 */
export interface Forwarder {
  id: string;
  name: string;
  enabled: boolean;
  kind: ForwardKind;
  /** Syslog kinds; `''` for `https_json`. */
  host: string;
  /** Syslog kinds; `0` for `https_json`. */
  port: number;
  format: ForwardFormat;
  streams: ForwardStream[];
  /**
   * Holds back what is less severe (0 … 7). An entry with no severity — every
   * trap — and the audit trail are never held back by it.
   */
  min_severity?: number | null;
  /** Also what a lab's simulated devices write; off unless asked for. */
  lab: boolean;
  /**
   * `syslog_tls`, `https_json`: the one authority the collector's
   * certificate must be signed by. Public text, stored and returned.
   */
  ca_pem?: string | null;
  /** `https_json`: whether a destination (URL, and perhaps a token) is stored. */
  has_secret: boolean;
  updated_at: number;
}

/**
 * `logs.forward.save`. **The document is replaced by what is sent**, and
 * what is left out takes its default — except `enabled` and the secret,
 * which are kept when left out.
 *
 * `url` and `token` are accepted here and returned nowhere. Left out, what
 * is stored stays; an empty `url` clears both. A new `url` does **not**
 * inherit the stored token: a token has to come with it, or there is none.
 */
export interface ForwarderSaveParams {
  /** Without one a forwarder is created. */
  id?: string;
  name: string;
  kind: ForwardKind;
  enabled?: boolean;
  /** Syslog kinds. */
  host?: string;
  /** Syslog kinds; 514 when left out, 6514 for `syslog_tls`. */
  port?: number;
  format?: ForwardFormat;
  /** `syslog`, `trap` and `event` when left out. */
  streams?: ForwardStream[];
  min_severity?: number | null;
  lab?: boolean;
  ca_pem?: string | null;
  /** `https_json`, write-only. */
  url?: string;
  /** `https_json`, write-only. */
  token?: string;
}

export interface ForwarderDeleted {
  deleted: string;
}

/** The only answer `logs.forward.test` gives; a failure is an error (`unavailable`) that says why. */
export interface ForwarderTested {
  sent: true;
}

/** `logs.forward.list` {} → {@link Forwarder}[], by name */
/** `logs.forward.save` {@link ForwarderSaveParams} → {@link Forwarder} */
/** `logs.forward.delete` `{id}` → {@link ForwarderDeleted} */
/** `logs.forward.test` `{id}` → {@link ForwarderTested} */

// -------------------------------------------------------------------- flows

/** The six ways a flow record is counted. */
export type FlowBy =
  | 'source'
  | 'destination'
  | 'conversation'
  | 'application'
  | 'interface'
  | 'dscp';

export interface FlowTopParams {
  /** Required. */
  by: FlowBy;
  /** Only the exporters of one device: its id, name or address. */
  device_id?: string;
  /** Only one exporter, by its address: how one no device claims is looked at. */
  exporter?: string;
  /** The last this many whole minutes and the running one; 1 to 1,440, default 15. */
  minutes?: number;
  /** Default 20, brought into 1 to 500. */
  limit?: number;
}

export interface FlowCounts {
  bytes: number;
  packets: number;
  flows: number;
}

/** Which interface an `interface` row is about. */
export interface FlowInterface {
  /** The exporter's address. */
  exporter: string;
  device_id?: string | null;
  /** `0` is "the exporter did not say". */
  if_index: number;
  /** From `monitor.ifindex`, when that knows the index. */
  name?: string | null;
  direction: 'in' | 'out';
}

export interface FlowRow extends FlowCounts {
  /** Stable text: an address, `a <-> b`, `tcp/443`, a DSCP number, `<exporter> <ifIndex> <in|out>`. */
  key: string;
  /** The same for a person, derived from numbers and carrying them: `tcp/443 (https)`, `EF (46)`. */
  label: string;
  /** Rows by `interface` only. */
  interface?: FlowInterface;
}

/** What an answer covers. Nothing here is multiplied or guessed at. */
export interface FlowCoverage {
  /** Whether the collector is listening now. When it is not, an empty answer is "not collecting". */
  collecting: boolean;
  window_start: number;
  until: number;
  /** The start of the first minute in the window that holds anything; `null` when none does. */
  since?: number | null;
  /** Flow records added up. */
  records: number;
  /** Data sets that arrived in the window before their template: in no row and not in `total`. */
  sets_without_template: number;
  /** The 1-in-N rates the exporters said, ascending, at most eight; empty when none said one. */
  sampling: number[];
  /** How many of `records` came with a sampling rate. */
  sampled_records: number;
  /** How many exporters have something in the window. */
  exporters: number;
}

/**
 * The biggest talkers by bytes. `bytes` and `packets` are **as exported** —
 * a sampling rate is reported beside them, never multiplied in — and a row
 * is a floor: never more than was seen. `total` is exact; `other` is the
 * part of it under no key. Rows by `interface` add up to twice `total`
 * (each flow is counted where it came in and where it left).
 */
export interface FlowTop {
  rows: FlowRow[];
  total: FlowCounts;
  other: FlowCounts;
  coverage: FlowCoverage;
}

/** One exporter the collector follows. */
export interface FlowExporter {
  address: string;
  device_id?: string | null;
  /** The device's name; `null` when no device claims the address. */
  device?: string | null;
  /** Why no device claims the address; empty when one does. */
  reason: string;
  /** What it last spoke: 5, 9, or 10 for IPFIX. */
  version: number;
  first_at: number;
  last_at: number;
  datagrams: number;
  /** Flow records counted from it. */
  records: number;
  /** Records not counted because it is unclaimed and `unknown_sources` is `drop`. */
  dropped: number;
  /**
   * Templates known for it now. Whether they outlive the collector being
   * rebound is the engine's to say (`docs/dev/features/logs.md`).
   */
  templates: number;
  sets_without_template: number;
  /** The sampling rates it has said. */
  sampling: number[];
}

/** `flows.top` {@link FlowTopParams} → {@link FlowTop} */
/** `flows.exporters` {} → {@link FlowExporter}[], most recently heard first */

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

/** `logs.signal`: a rule matched, or an alert a rule raised is over. */
export interface LogsSignal {
  rule_id: string;
  rule: string;
  /** `cleared` when the rule's alert ended; then `reason` says how. */
  kind: SignalKind | 'cleared';
  device_id?: string | null;
  device: string;
  severity?: string | null;
  title: string;
  interface?: string | null;
  /** The interface or the group key the signal is counted under; `''` = the device. */
  instance: string;
  groups: Record<string, string>;
  /** The line that said so; `null` when time cleared the alert. */
  entry_id: number | null;
  /** Matching lines it stands for, this one included; `0` for a clearing. */
  count: number;
  at: number;
  /** A clearing only: its opposite arrived, or the quiet period ended. */
  reason?: 'opposite' | 'quiet';
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
 *
 * A stored trap is on `logs.appended` like a line. The flow collector
 * publishes nothing of what it counts — only `logs.status_changed` when it
 * starts, stops or cannot bind — so a screen asks `flows.top` again.
 */
