/**
 * Monitoring (`nocturne-monitor`): polling, time series, alert rules, alerts,
 * maintenance windows and notification channels.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and a field the backend declares as optional is written
 * `T | null` because it is present in the JSON with a `null` value. Backend
 * source: `crates/nocturne-monitor`. Specification:
 * `docs/dev/features/monitor.md`.
 *
 * Two conventions run through the whole surface:
 *
 * * **A gap is not a zero.** Any rate or reading that could not be measured
 *   is `null` — a device that did not answer, a counter with nothing to
 *   subtract from, the poll after a reboot. `metrics.query` leaves those
 *   points out of its array entirely, so a graph drawn from them shows a
 *   hole rather than a drop to the floor.
 * * **Paging is by offset.** `monitor.interfaces` and `alerts.list` return a
 *   plain array, so `cursor` is the number of rows already received, as a
 *   decimal string (`"200"`). A page shorter than `limit` is the last one.
 */

// ---------------------------------------------------------------- vocabulary

/** How much attention something wants. Ordered: info < low < medium < high < critical. */
export type Severity = 'info' | 'low' | 'medium' | 'high' | 'critical';

/** `acknowledged` is still open; only `resolved` is closed. */
export type AlertState = 'firing' | 'acknowledged' | 'resolved';

/** Which interfaces an `interface_down` rule looks at. */
export type InterfaceSelector =
  /** Interfaces with a collected LLDP/CDP neighbour. */
  | 'uplinks'
  /** Interfaces with a description. */
  | 'described'
  | 'all';

/** Which way a threshold is crossed. */
export type ThresholdOp = 'gt' | 'lt';

/** Which resolution a metric query was answered from. */
export type Rollup = 'raw' | '5m' | '1h';

/**
 * The metric names monitoring writes. Device-level metrics have an empty
 * `instance`; `temp.c` is per sensor and every `if.*` is per interface, with
 * the canonical interface name as the instance.
 */
export type MetricName =
  | 'icmp.rtt_ms'
  | 'icmp.loss_pct'
  /** 1 when the poll answered, 0 when it did not. */
  | 'up'
  | 'uptime_s'
  | 'cpu.pct'
  | 'mem.pct'
  /** Instance = the sensor's table index. */
  | 'temp.c'
  | 'if.in_bps'
  | 'if.out_bps'
  | 'if.util_pct'
  | 'if.in_err_ps'
  | 'if.out_err_ps'
  | 'if.in_disc_ps'
  | 'if.out_disc_ps'
  /** 1 when the line protocol is up, 0 when it is not. */
  | 'if.oper';

/** Which devices a rule or a maintenance window covers. Empty lists mean "any". */
export interface Scope {
  sites: string[];
  roles: string[];
  tags: string[];
  device_ids: string[];
  /** Lab devices are left alone unless a rule opts in. */
  include_lab: boolean;
}

/** As sent to `alerts.rules.save` / `maintenance.save`: every facet optional. */
export interface ScopeInput {
  sites?: string[];
  roles?: string[];
  tags?: string[];
  device_ids?: string[];
  include_lab?: boolean;
}

// ------------------------------------------------------------------- alerts

export interface Alert {
  id: string;
  /** The rule that raised it, or `null` for `alerts.raise`. */
  rule_id: string | null;
  /** The rule's name, or the feature that raised it. */
  rule_name: string;
  device_id: string | null;
  /** The device's name when the alert was raised. */
  device: string | null;
  /** Interface name, sensor index, or whatever else the rule is keyed on. */
  instance: string | null;
  severity: Severity;
  state: AlertState;
  title: string;
  detail: string;
  /** The measurement that fired the rule, when there is one. */
  value: number | null;
  started_at: number;
  last_seen_at: number;
  resolved_at: number | null;
  acked_at: number | null;
  ack_note: string | null;
  snoozed_until: number | null;
  /** How often the condition has been seen since this alert was raised. */
  count: number;
  /** True while a maintenance window covers it: recorded, not notified. */
  muted_by_maintenance: boolean;
  lab: boolean;
  /**
   * What makes two sightings the same alert: `rule:<rule>:<device>:<instance>`
   * for a rule, whatever the caller passed for `alerts.raise`.
   */
  dedup_key: string;
}

/** What a rule watches for. Tagged by `type`. */
export type Condition =
  /** The damped reachability state says the device is down. */
  | { type: 'device_down' }
  /** Administratively up but the line protocol is down. */
  | { type: 'interface_down'; only: InterfaceSelector }
  /**
   * A metric crossed a threshold. `metric` may be a glob over metric names
   * (`if.*_err_ps`) and `instance` a glob over instances (`Gi0/*`).
   * `clear_value` gives hysteresis: between the two values the alert neither
   * fires nor resolves.
   */
  | {
      type: 'threshold';
      metric: string;
      instance: string | null;
      op: ThresholdOp;
      value: number;
      clear_value: number | null;
    }
  /** Nothing has been recorded for `metric` in the last `for_s` seconds. */
  | { type: 'no_data'; metric: string; for_s: number }
  /** The `config.changed` event fired for a device in scope. */
  | { type: 'config_changed' }
  /**
   * Round-trip time more than `multiple` times the median of the last
   * `window_s` seconds. Not one of the five conditions the specification
   * lists: the built-in "RTT above 5× the 24-hour median" rule cannot be
   * written with the others.
   */
  | { type: 'rtt_baseline'; multiple: number; window_s: number };

export interface AlertRule {
  id: string;
  name: string;
  enabled: boolean;
  /**
   * Shipped with Nocturne. A built-in rule can be edited and switched off
   * but not deleted (`alerts.rules.delete` refuses with `forbidden`).
   */
  builtin: boolean;
  severity: Severity;
  scope: Scope;
  condition: Condition;
  /** The condition must hold this long before the alert fires. */
  for_s: number;
  /** Notification channel ids; empty means every enabled channel. */
  notify: string[];
  created_at: number;
  updated_at: number;
}

// -------------------------------------------------------------- maintenance

export type Recurrence = 'weekly';

export interface MaintenanceWindow {
  id: string;
  name: string;
  scope: Scope;
  starts_at: number;
  ends_at: number;
  /** `weekly` repeats the same slot every seven days. */
  recurring: Recurrence | null;
  note: string;
  created_at: number;
  updated_at: number;
}

// ----------------------------------------------------------------- channels

export type ChannelKind =
  /** A JSON POST of the alert itself. */
  | 'webhook'
  | 'slack'
  | 'teams'
  | 'discord'
  /** SMTP, with STARTTLS where the server offers it. */
  | 'email';

export interface EmailSettings {
  host: string;
  port: number;
  /** `auto` (STARTTLS when offered), `required` or `never`. */
  starttls: string;
  username: string;
  from: string;
  to: string[];
}

/**
 * A notification channel. The webhook URL — which for Slack, Teams and
 * Discord *is* the secret — and the SMTP password are never part of this
 * record: they live in the vault, and `has_secret` says whether one is set.
 */
export interface Channel {
  id: string;
  name: string;
  kind: ChannelKind;
  enabled: boolean;
  /** Alerts below this severity are not sent to this channel. */
  min_severity: Severity;
  email: EmailSettings | null;
  has_secret: boolean;
  last_sent_at: number | null;
  /** Why the last attempt failed. Never contains the URL or the password. */
  last_error: string | null;
  created_at: number;
  updated_at: number;
}

// --------------------------------------------------------------- the device

/** One interface as monitoring last saw it. Every rate is `null` for a gap. */
export interface IfStatus {
  device_id: string;
  /** The device's name, for lists that span devices. */
  device: string;
  /** Canonical interface name; this is the series instance too. */
  name: string;
  description: string;
  admin_up: boolean | null;
  oper_up: boolean | null;
  speed_bps: number | null;
  in_bps: number | null;
  out_bps: number | null;
  util_pct: number | null;
  in_err_ps: number | null;
  out_err_ps: number | null;
  in_disc_ps: number | null;
  out_disc_ps: number | null;
  /** Whether a neighbour has been collected on this interface. */
  uplink: boolean;
  lab: boolean;
  at: number;
}

/** The device-level numbers of the last poll that produced them. */
export interface DeviceHealth {
  cpu_pct: number | null;
  mem_pct: number | null;
  /** The hottest sensor; the per-sensor values are in the `temp.c` series. */
  temp_c: number | null;
  uptime_s: number | null;
  at: number | null;
}

export interface SourceStatus {
  enabled: boolean;
  /** `null` until it has been tried. */
  ok: boolean | null;
  at: number | null;
  /** How reachability was established: `icmp`, `tcp/22`, `snmp`, `sim`. */
  via: string | null;
  error: string | null;
}

export interface Sources {
  icmp: SourceStatus;
  snmp: SourceStatus;
  cli: SourceStatus;
}

/** `nocturne_app::model::DeviceState`, as the inventory keeps it. */
export interface DeviceState {
  device_id: string;
  /** `null` until the first check. */
  reachable: boolean | null;
  rtt_ms: number | null;
  last_seen: number | null;
  last_checked: number | null;
  last_error: string | null;
  facts: unknown | null;
  facts_at: number | null;
  detected_profile: string | null;
}

// -------------------------------------------------------------------- status

/** Something a person should look at in the installation, not on a device. */
export interface MonitorProblem {
  /**
   * `unreachable`, `no_probe`, `snmp_credential`, `channel`, `ping_method`,
   * `maintenance` or `interfaces`.
   */
  kind: string;
  message: string;
  device_id?: string;
  count?: number;
}

export interface MonitorStatus {
  enabled: boolean;
  devices_monitored: number;
  /** Polls started in the last minute. */
  polls_per_min: number;
  /** How long the last full round of polls took. */
  last_cycle_ms: number;
  /** `icmp_datagram`, `icmp_raw` or `system_command`. */
  ping_method: string;
  problems: MonitorProblem[];
}

export interface MonitorDeviceView {
  state: DeviceState;
  health: DeviceHealth;
  interfaces: IfStatus[];
  sources: Sources;
  /** The damped verdict: true once `down_after` polls in a row have failed. */
  down: boolean;
  last_poll_at: number | null;
  lab: boolean;
}

// ------------------------------------------------------------------ metrics

/** `[timestamp, value]`. */
export type MetricPoint = [number, number];

export interface MetricSeries {
  device_id: string;
  metric: string;
  instance: string;
  /** Only the buckets that have data: a gap is a missing point. */
  points: MetricPoint[];
  /** Seconds each point covers. */
  step_s: number;
  rollup: Rollup;
}

export interface SeriesSummary {
  metric: string;
  instance: string;
  /**
   * The newest sample, as `[timestamp, value]`. Always present: a series
   * with no samples at all is not listed.
   */
  latest: MetricPoint;
}

export interface TopEntry {
  device_id: string;
  device: string;
  instance: string;
  value: number;
  at: number;
  lab: boolean;
}

// ------------------------------------------------------------------ settings

/** The `monitoring` settings section (`settings.get` / `settings.set`). */
export interface MonitorSettings {
  /** Turns the scheduler off without touching any device's own setting. */
  enabled: boolean;
  /** Probes in flight across the whole network. */
  concurrency: number;
  /** Consecutive failed polls before a device is called down. */
  down_after: number;
  /** How often devices without SNMP are read over the CLI. */
  cli_interval_s: number;
  retention_days_raw: number;
  retention_days_rollup: number;
  ping_timeout_ms: number;
}

// -------------------------------------------------------------- parameters

export interface PollNowParams {
  device_ids?: string[];
  /** Convenience for a single device. */
  device_id?: string;
}

export interface DeviceIdParams {
  device_id: string;
}

export interface InterfaceQueryParams {
  device_id?: string;
  /** Free text over the device name, the interface name and its description. */
  filter?: string;
  /**
   * Only interfaces that want looking at: administratively up while the line
   * protocol is down, taking errors, or at 90 % utilisation or more.
   */
  problem_only?: boolean;
  /** `name` (default), `device`, `util`, `in_bps`, `out_bps`, `errors`. */
  sort?: string;
  /** Default 200, at most 5000. */
  limit?: number;
  /** Rows already received, as a decimal string. */
  cursor?: string;
}

export interface MetricQueryParams {
  device_id: string;
  metric: string;
  /** Empty or absent for a device-level metric. */
  instance?: string;
  since: number;
  /** Defaults to now. */
  until?: number;
  /** Wanted seconds between points; the answer says what was used. */
  step_s?: number;
}

export interface SeriesRef {
  device_id: string;
  metric: string;
  instance?: string;
}

export interface MetricQueryManyParams {
  /** At most 200 series per call. */
  series: SeriesRef[];
  since: number;
  until?: number;
  step_s?: number;
}

export interface MetricTopParams {
  metric: string;
  /** Default 10. */
  limit?: number;
  /** Free text matched against the device's name, site, role and tags. */
  filter?: string;
  /** Ignore values older than this many seconds; default 900. */
  within_s?: number;
}

export interface AlertListParams {
  /** `firing`, `acknowledged`, `resolved`, or `open` for the first two. */
  state?: 'firing' | 'acknowledged' | 'resolved' | 'open';
  /** At least this severe. */
  severity?: Severity;
  device_id?: string;
  /** Only alerts seen since this time. */
  since?: number;
  /** Default 100, at most 2000. */
  limit?: number;
  /** Alerts already received, as a decimal string. */
  cursor?: string;
}

export interface AlertIdParams {
  id: string;
}

export interface AlertNoteParams {
  id: string;
  note?: string;
}

export interface AlertSnoozeParams {
  id: string;
  /** `0` cancels the snooze; at most 30 days. */
  minutes: number;
}

export interface AlertRaiseParams {
  /** The name of whatever is raising this; shown instead of a rule name. */
  rule?: string;
  device_id?: string;
  instance?: string;
  severity: Severity;
  title: string;
  detail?: string;
  /** Required: the same problem seen twice has to be one alert. */
  dedup_key: string;
}

export interface AlertSummary {
  open: number;
  /** Open alerts per severity name. */
  by_severity: Record<string, number>;
  newest: Alert | null;
}

/** Fields left out keep what is stored; `condition` is required for a new rule. */
export interface AlertRuleSaveParams {
  id?: string;
  name?: string;
  enabled?: boolean;
  severity?: Severity;
  scope?: ScopeInput;
  condition?: Condition;
  for_s?: number;
  notify?: string[];
}

export interface AlertRuleTestParams {
  /** An existing rule to try, or … */
  id?: string;
  /** … a rule that has not been saved yet. */
  rule?: AlertRule;
  /** Try it on these devices only, instead of everything in scope. */
  device_ids?: string[];
}

export interface AlertRuleTestMatch {
  device_id: string;
  device: string;
  instance: string | null;
  value: number | null;
  detail: string;
  /** How long the rule would wait before firing. */
  would_wait_s: number;
}

export interface AlertRuleTestResult {
  devices_in_scope: number;
  matches: AlertRuleTestMatch[];
  /** Empty unless the rule cannot be tried against stored data. */
  note: string;
}

export interface MaintenanceSaveParams {
  id?: string;
  name: string;
  scope?: ScopeInput;
  starts_at: number;
  ends_at: number;
  /** A weekly window has to be shorter than a week. */
  recurring?: Recurrence | null;
  note?: string;
}

export interface ChannelSaveParams {
  id?: string;
  name: string;
  kind: ChannelKind;
  enabled?: boolean;
  min_severity?: Severity;
  email?: EmailSettings;
  /**
   * The webhook URL, for every kind but `email`. Stored in the vault and
   * never returned. Absent leaves what is stored; an empty string clears it.
   */
  url?: string;
  /** The SMTP password, same rules. */
  password?: string;
}

export interface IdParams {
  id: string;
}

export interface Deleted {
  deleted: boolean;
}

export interface JobStarted {
  job_id: string;
}

export interface ChannelTestResult {
  sent: boolean;
  /** How many attempts it took (1–3). */
  attempts: number;
}

// ----------------------------------------------------------------- methods

/**
 * `monitor.status` — what the installation is watching and what is in the
 * way. No params → `MonitorStatus`.
 */
/**
 * `monitor.poll_now` — poll the named devices at once, as a job.
 * `PollNowParams` → `JobStarted`.
 */
/**
 * `monitor.device` — everything known about one device right now.
 * `DeviceIdParams` → `MonitorDeviceView`.
 */
/**
 * `monitor.interfaces` — the interface table, filtered, sorted and paged.
 * `InterfaceQueryParams` → `IfStatus[]`.
 */
/**
 * `metrics.query` — one series over a time range, gaps left as gaps.
 * `MetricQueryParams` → `MetricSeries`.
 */
/**
 * `metrics.query_many` — several series over one range, in the order asked.
 * `MetricQueryManyParams` → `MetricSeries[]`.
 */
/**
 * `metrics.series` — which series a device has, with their newest value.
 * `DeviceIdParams` → `SeriesSummary[]`.
 */
/**
 * `metrics.top` — the highest current values of a metric across devices and
 * interfaces ("top talkers", "busiest CPU"). `MetricTopParams` → `TopEntry[]`.
 */
/**
 * `alerts.list` — alerts, newest first. `AlertListParams` → `Alert[]`.
 */
/** `alerts.get` — one alert. `AlertIdParams` → `Alert`. */
/**
 * `alerts.ack` — acknowledge an alert; it stays open. `AlertNoteParams` →
 * `Alert`. Refuses with `conflict` once the alert has resolved.
 */
/**
 * `alerts.resolve` — close an alert by hand. `AlertNoteParams` → `Alert`.
 */
/**
 * `alerts.snooze` — mute an alert's notifications for a while.
 * `AlertSnoozeParams` → `Alert`.
 */
/**
 * `alerts.raise` — raise an alert from another feature (log rules, insights,
 * configuration), deduplicated on `dedup_key`. `AlertRaiseParams` → `Alert`.
 */
/** `alerts.summary` — open alerts by severity. No params → `AlertSummary`. */
/**
 * `alerts.rules.list` — every rule, most severe first. No params →
 * `AlertRule[]`.
 */
/**
 * `alerts.rules.save` — create or change a rule. `AlertRuleSaveParams` →
 * `AlertRule`.
 */
/**
 * `alerts.rules.delete` — delete a rule. `IdParams` → `Deleted`. Built-in
 * rules are refused with `forbidden`: switch them off instead.
 */
/**
 * `alerts.rules.test` — what a rule would fire on, against stored data,
 * without raising anything. `AlertRuleTestParams` → `AlertRuleTestResult`.
 */
/** `maintenance.list` — every window, earliest first. No params → `MaintenanceWindow[]`. */
/**
 * `maintenance.save` — create or change a window. `MaintenanceSaveParams` →
 * `MaintenanceWindow`.
 */
/** `maintenance.delete` — delete a window. `IdParams` → `Deleted`. */
/** `notify.channels.list` — every channel, by name. No params → `Channel[]`. */
/**
 * `notify.channels.save` — create or change a channel; the URL or password
 * goes to the vault. `ChannelSaveParams` → `Channel`.
 */
/**
 * `notify.channels.delete` — delete a channel and its secret. `IdParams` →
 * `Deleted`.
 */
/**
 * `notify.channels.test` — send a clearly marked test message.
 * `IdParams` → `ChannelTestResult`; `unavailable` when it could not be
 * delivered after three attempts.
 */

// ------------------------------------------------------------------- events

/** `monitor.polled` — one device has been polled. */
export interface MonitorPolledEvent {
  device_id: string;
  at: number;
  ok: boolean;
}

/** `monitor.interfaces` — a device's interface table has changed. */
export interface MonitorInterfacesEvent {
  device_id: string;
}

/** `alerts.changed` — an alert was raised, counted, acknowledged or resolved. */
export interface AlertsChangedEvent {
  alert: Alert;
}

/**
 * `monitor.status_changed` — the setup moved: rules, maintenance windows or a
 * channel. `what` is `rules`, `maintenance` or `channel`.
 */
export interface MonitorStatusChangedEvent {
  what: string;
  channel_id?: string;
  ok?: boolean;
}

/**
 * The events this feature publishes. `devices.state` comes from the core
 * (`nocturne_app::model::DeviceState`) and is republished on every poll.
 */
export type MonitorEvent =
  | { topic: 'devices.state'; payload: DeviceState }
  | { topic: 'monitor.polled'; payload: MonitorPolledEvent }
  | { topic: 'monitor.interfaces'; payload: MonitorInterfacesEvent }
  | { topic: 'alerts.changed'; payload: AlertsChangedEvent }
  | { topic: 'monitor.status_changed'; payload: MonitorStatusChangedEvent };
