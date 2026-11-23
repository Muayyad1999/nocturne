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
 * * **Paging is by offset.** `monitor.interfaces`, `alerts.list` and
 *   `checks.list` return a plain array, so `cursor` is the number of rows
 *   already received, as a decimal string (`"200"`). A page shorter than
 *   `limit` is the last one.
 * * **A service check that could not run is not one that is down.**
 *   `CheckStatus` has five values for a reason; `cannot_run` means nothing
 *   is known about the service, and showing it as a failure invents an
 *   outage. See the type's own documentation.
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
  | 'if.oper'
  /**
   * Service checks. The instance is the **check's id**, not its name, so a
   * rename does not split the series; `checks.list` maps id to name.
   */
  /** 1 when the check found the service up or in warning, 0 when down. Written only for a run that reached a verdict. */
  | 'check.up'
  | 'check.rtt_ms'
  /** Days until the certificate expires; negative once expired. */
  | 'check.cert_days'
  | 'check.loss_pct';

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
  /**
   * Whether a maintenance window covered the alert the last time it was
   * seen, announced or resolved. A record of that moment, not a live answer.
   */
  muted_by_maintenance: boolean;
  /**
   * When the firing notification went out; `null` while nobody has been
   * told (a maintenance window, a snooze, a lab device while
   * `notify_for_lab` is off, or `notify: []`). An alert that was never
   * announced resolves without a notification too.
   */
  notified_at: number | null;
  /**
   * The channel ids `alerts.raise` named for this alert: `null` is the usual
   * routing (the rule's channels, or every enabled one), an empty list is
   * nobody.
   */
  notify: string[] | null;
  /**
   * About a simulated device. Its notifications go out only while the
   * `notify_for_lab` setting is on.
   */
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
  /**
   * A named service check on a device in scope is in `state`. This is what
   * lets a rule say "the web server on this box", which a metric threshold
   * cannot. `check` is a glob over check names; each matching check is its
   * own alert, keyed on its name, so two services failing on one device are
   * two alerts.
   *
   * `cannot_run` is a rule of its own because it is a different thing to be
   * told: not that a service is down, but that monitoring cannot tell.
   */
  | {
      type: 'check_state';
      /** A glob over check names; `null` or `*` is every check. */
      check: string | null;
      /** Only checks of these kinds; empty is every kind. */
      kinds: CheckKind[];
      state: 'down' | 'warning' | 'cannot_run';
    }
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


// ----------------------------------------------------------- service checks

/**
 * What a service check speaks. `http` and `https` are separate kinds: the
 * second inspects the certificate as well as the page.
 */
export type CheckKind =
  | 'icmp'
  | 'tcp'
  | 'dns'
  | 'http'
  | 'https'
  | 'tls'
  | 'ntp'
  | 'snmp';

/**
 * What a check is saying. Five values, not three, because three cannot tell
 * the truth:
 *
 * * `unknown` — it has never run. Nothing is known.
 * * `up` — it ran and the service answered properly.
 * * `warning` — it ran, the service answered, and something it measures is
 *   inside its warning margin (a certificate that expires in nine days).
 *   Available now; **counted as up** in every availability figure.
 * * `down` — it ran and the service did not answer, or answered wrongly.
 *   This is a measurement of the service.
 * * `cannot_run` — the check could not be attempted at all: no credential,
 *   nothing to aim at, a name that will not resolve, a machine that may not
 *   send ICMP, a simulated device. This says **nothing** about the service.
 *   It is not an outage, it writes no `check.up` sample, and it does not
 *   resolve an open outage alert. Show it as a blind spot, never as a
 *   failure.
 */
export type CheckStatus = 'unknown' | 'up' | 'warning' | 'down' | 'cannot_run';

/**
 * The stable reason codes a check's `code` can carry. Switch on these; show
 * `message`, which is the engine's own sentence.
 *
 * The ones that mean `cannot_run` come after the outage codes, and are never
 * an outage. The last two are not a run's verdict at all: they appear only on
 * a history row that says the check stopped knowing.
 */
export type CheckCode =
  /** All well. */
  | 'ok'
  /** Nothing came back within the timeout. */
  | 'timeout'
  /** The connection was refused: nothing is listening. */
  | 'refused'
  | 'unreachable'
  /** The peer answered, but not as the protocol requires. */
  | 'protocol'
  /** A connection the far end tore down. */
  | 'io'
  /** No echo request came back. */
  | 'no_reply'
  /** Loss at or above the check's own limit. */
  | 'loss'
  /** The HTTP status was not one the check accepts. */
  | 'http_status'
  /** The body did not contain the text the check insists on. */
  | 'http_expect_text'
  /** The server answered with a negative response code. */
  | 'dns_rcode'
  /** The server has no such record. */
  | 'dns_empty'
  /** No answer record contained the expected text. */
  | 'dns_expect'
  /** The certificate expired. */
  | 'tls_expired'
  /** The chain does not verify, and the check requires that it does. */
  | 'tls_untrusted'
  /** The certificate is not valid for the name asked for. */
  | 'tls_name_mismatch'
  /** Expiry is inside the check's warning margin: a `warning`, not a failure. */
  | 'tls_expiring'
  /** The server's own clock is not set (stratum 16). */
  | 'ntp_unsynchronised'
  /** The clock offset is beyond what the check allows. */
  | 'ntp_offset'
  | 'snmp_unreachable'
  | 'snmp_failed'
  // Below here: the check could not run, and nothing is known.
  /** No SNMP credential is set for the device. */
  | 'no_credential'
  /** The agent answered, but does not have the object asked for. */
  | 'snmp_no_such_object'
  /** Neither the check nor its device names anything to send to. */
  | 'no_address'
  /** The target name does not resolve, so there was nowhere to aim. */
  | 'dns_resolve'
  /** This machine is not allowed to send what the check needs to send. */
  | 'permission'
  | 'unsupported'
  /** The check's own parameters are wrong. */
  | 'invalid'
  /** A simulated device: a console, but no listening services. */
  | 'sim'
  /** The device is gone. */
  | 'no_device'
  /** The probe could not be attempted (the poller's own reachability layer). */
  | 'probe_unavailable'
  /** Something local failed — out of sockets, not the service's doing. */
  | 'local_error'
  // Below here: only on a history row whose `status` is `unknown`.
  /**
   * The check went unrun for longer than its cadence allows (the application
   * was closed, the check or its device's monitoring switched off). The row
   * is dated at the last run before the silence.
   */
  | 'not_measured'
  /** The check was pointed at something else, which drops what it had found. */
  | 'repointed';

/** An HTTP or HTTPS check. No request headers: one would carry a token. */
export interface HttpCheck {
  /** Path and query; `/` when empty. */
  path: string;
  /** 80 for `http`, 443 for `https`, when null. */
  port: number | null;
  /** `GET` when null. */
  method: string | null;
  /** Statuses that count as up. Empty means any 2xx or 3xx. */
  expect_status: number[];
  /** Unless this appears in the first 2 KiB of the body, the check is down. */
  expect_text: string | null;
  /** Redirects followed; 0 makes a redirect the answer. Default 3. */
  follow_redirects: number | null;
  /** HTTPS only: a certificate that does not verify makes the check down. */
  verify_tls: boolean | null;
  /** HTTPS only: days of certificate life left at which it goes to `warning`. */
  warn_days: number | null;
}

/** What a check does, and the parameters of that one kind. Tagged by `kind`. */
export type CheckSpec =
  | {
      kind: 'icmp';
      /** Echoes per run, 1–16; 3 when null. */
      count: number | null;
      /** Loss at or above this percentage is down; 100 by default. */
      loss_pct_down: number | null;
    }
  | { kind: 'tcp'; port: number }
  | {
      kind: 'dns';
      /** The name to ask about. */
      name: string;
      /** `A` when empty. */
      record: string;
      /** Unless an answer record contains this, the check is down. */
      expect: string | null;
      /** 53 when null. */
      port: number | null;
    }
  | ({ kind: 'http' } & HttpCheck)
  | ({ kind: 'https' } & HttpCheck)
  | {
      kind: 'tls';
      /** 443 when null. */
      port: number | null;
      /** The server name to send and check instead of the target's. */
      sni: string | null;
      /** A chain that does not verify is down. True by default. */
      require_trusted: boolean | null;
      /** A certificate not valid for the name asked for is down. True by default. */
      require_hostname: boolean | null;
      /** Days of life left at which it goes to `warning`; the setting when null. */
      warn_days: number | null;
    }
  | {
      kind: 'ntp';
      /** 123 when null. */
      port: number | null;
      /** A clock further out than this is down; null accepts any answer. */
      max_offset_ms: number | null;
    }
  | {
      kind: 'snmp';
      /** The object to read; `sysUpTime` when null. */
      oid: string | null;
    };

/** A named check attached to a device, with its own schedule. */
export interface ServiceCheck {
  id: string;
  device_id: string;
  /** What a person calls it: `web`, `mail`, `resolver`. Unique per device. */
  name: string;
  spec: CheckSpec;
  /** Empty means the device's own address, which is the usual case. */
  host: string;
  enabled: boolean;
  interval_s: number;
  timeout_ms: number;
  created_at: number;
  updated_at: number;
}

/**
 * What a run measured, beyond up or down. Every field is `null` until
 * something actually measured it: a check that timed out has no HTTP status,
 * and `0` would be a lie.
 */
export interface CheckFacts {
  /** Echoes lost, as a percentage of those sent. */
  loss_pct: number | null;
  http_status: number | null;
  /** The URL that answered, after redirects. */
  final_url: string | null;
  dns_rcode: string | null;
  /** `0` with `NOERROR` means the name exists and has no such record. */
  dns_records: number | null;
  /** How far this machine's clock is behind the server's. */
  ntp_offset_ms: number | null;
  ntp_stratum: number | null;
  /** Days until the certificate expires; negative once expired. */
  cert_expires_in_days: number | null;
  cert_subject: string | null;
  cert_issuer: string | null;
  /** End of validity, `YYYY-MM-DDTHH:MM:SSZ`. */
  cert_not_after: string | null;
  cert_trusted: boolean | null;
  tls_protocol: string | null;
  /** The SNMP object that was read, in presentation form. */
  snmp_value: string | null;
}

/** What a check last found. */
export interface CheckState {
  check_id: string;
  device_id: string;
  status: CheckStatus;
  code: CheckCode | string;
  /** The engine's own sentence. Empty while all is well. */
  message: string;
  rtt_ms: number | null;
  facts: CheckFacts;
  /** When the current status began: with `status`, "down since 09:14". */
  since: number | null;
  last_run_at: number | null;
  /** The last run that found it up (a warning counts as up). */
  last_ok_at: number | null;
  last_down_at: number | null;
  /** Consecutive runs that came out with this same status. */
  runs: number;
}

/** A check and what it last found, as the RPC surface returns it. */
export interface ServiceCheckView extends ServiceCheck {
  /** The kind, beside the spec that carries it, so a list can group on it. */
  kind: CheckKind;
  /** The device's name, for lists that span devices. */
  device: string;
  state: CheckState;
}

/** One change of status. The runs in between said the same thing. */
export interface CheckEvent {
  id: number;
  check_id: string;
  device_id: string;
  at: number;
  status: CheckStatus;
  previous: CheckStatus;
  /** How long `previous` had lasted; `null` when that is not known. */
  held_s: number | null;
  code: CheckCode | string;
  message: string;
}

/**
 * How a check spent a window. The five figures are kept apart on purpose:
 * time that could not be measured — before the first run, or while the check
 * could not run — is neither uptime nor downtime.
 */
export interface CheckSummary {
  up_s: number;
  warning_s: number;
  down_s: number;
  cannot_run_s: number;
  unknown_s: number;
  /**
   * `(up + warning) / (up + warning + down)` as a percentage. `null` when
   * nothing in the window was measured: no figure is better than 100 %.
   */
  availability_pct: number | null;
  /** How much of the window the check had a verdict for. */
  measured_s: number;
  down_count: number;
  last_down_at: number | null;
  /** How long that outage lasted, or has lasted so far. */
  last_down_for_s: number | null;
}

/** How many checks are in each state. Disabled checks are not in the rest. */
export interface CheckCounts {
  total: number;
  up: number;
  warning: number;
  down: number;
  unknown: number;
  /**
   * Checks that cannot run. Count and show these apart from `down`: they are
   * how much of the screen is guesswork, not how much is broken.
   */
  cannot_run: number;
  disabled: number;
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
   * `maintenance`, `interfaces`, `checks` (service checks that are not
   * answering) or `checks_blind` (service checks that cannot run — a blind
   * spot, not a failure).
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
  /** The service checks, counted by what they are saying. */
  checks: CheckCounts;
  problems: MonitorProblem[];
}

export interface MonitorDeviceView {
  state: DeviceState;
  health: DeviceHealth;
  interfaces: IfStatus[];
  /** The device's service checks. Empty unless somebody added one. */
  checks: ServiceCheckView[];
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
  /** Turns the service checks off without touching any check's own setting. */
  checks_enabled: boolean;
  /** How long a service check's changes of state are kept. */
  check_history_days: number;
  /**
   * Days of certificate life left at which a TLS or HTTPS check goes to
   * `warning`. A check may set its own.
   */
  cert_warn_days: number;
  /**
   * Whether an alert about a lab device is announced at all. Off by default:
   * such an alert is raised, listed and shown, and sent to no channel.
   */
  notify_for_lab: boolean;
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
  /**
   * Channel ids. Left out (or `null`): the usual routing. `[]`: nobody. A
   * list: those channels and no others; an id that is not a channel is
   * refused with `not_found` and nothing is raised.
   */
  notify?: string[] | null;
}

export interface AlertClearParams {
  /** The key the alert was raised under. */
  dedup_key: string;
  /** Becomes the resolved alert's `detail`. */
  note?: string;
}

export interface AlertClearResult {
  /** `false` when nothing was open under the key. */
  cleared: boolean;
}

/** One row of a device's SNMP interface table: its ifIndex and its name. */
export interface IfIndexEntry {
  index: number;
  /** Spelled as `monitor.interfaces` and the metrics spell it. */
  name: string;
}

export interface IfIndexMap {
  /**
   * When the map was last read off the device. Absent — not `null` — for a
   * device that was never read over SNMP, which is not the same as a map
   * with nothing in it.
   */
  at?: number;
  /** Sorted by index. Rows the agent gave no name are not in it. */
  interfaces: IfIndexEntry[];
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


// ------------------------------------------------------- service check calls

export interface CheckListParams {
  device_id?: string;
  /** `problem` is everything that is not up and not unknown. */
  status?: CheckStatus | 'problem';
  kind?: CheckKind;
  /** Free text over the check's name. */
  filter?: string;
  enabled?: boolean;
  /** Default 200, at most 5000. */
  limit?: number;
  /** Rows already received, as a decimal string. */
  cursor?: string;
}

/**
 * Fields left out keep what is stored. `device_id`, `name` and `spec` are
 * required for a new check.
 *
 * Changing the kind, the host or the port points the check at a different
 * service, so what it found at the old one is dropped rather than carried
 * over: the answer comes back with `state.status` of `unknown`.
 */
export interface CheckSaveParams {
  id?: string;
  device_id?: string;
  name?: string;
  spec?: CheckSpec;
  /** A host or address, never a URL; empty means the device's own address. */
  host?: string;
  enabled?: boolean;
  /** 5 s to 24 h; default 60. */
  interval_s?: number;
  /** Defaults per kind; may not exceed the interval. */
  timeout_ms?: number;
}

export interface CheckRunNowParams {
  id?: string;
  ids?: string[];
  /** Every enabled check of this device. At most 50 checks per call. */
  device_id?: string;
}

/** One check's result from `checks.run_now`, or why it could not be recorded. */
export interface CheckRunResult {
  check_id: string;
  name?: string;
  kind?: CheckKind;
  status?: CheckStatus;
  previous?: CheckStatus;
  changed?: boolean;
  code?: CheckCode | string;
  message?: string;
  rtt_ms?: number | null;
  facts?: CheckFacts;
  /**
   * Set when the check was not run because nothing is watching it: the
   * check is switched off (`disabled`), monitoring is off for its device or
   * for the installation (`monitoring_off`), or service checks are off
   * (`checks_off`). `error` then says which switch, and `ran` does not
   * count it.
   */
  skipped?: 'disabled' | 'monitoring_off' | 'checks_off';
  error?: string;
}

export interface CheckRunNowResult {
  ran: number;
  checks: CheckRunResult[];
}

export interface CheckHistoryParams {
  id: string;
  /** Default: seven days ago. */
  since?: number;
  /** Default: now. */
  until?: number;
  /** Default 500. */
  limit?: number;
}

export interface CheckHistory {
  check_id: string;
  device_id: string;
  name: string;
  kind: CheckKind;
  since: number;
  until: number;
  current: CheckState;
  /** The newest `limit` changes in the window, oldest first. */
  events: CheckEvent[];
  /** How many changes the window holds; `summary` is added up from all of them. */
  events_total: number;
  /** Whether `events` was cut to `limit`: page back with `until` before the oldest row. */
  truncated: boolean;
  summary: CheckSummary;
  /** Where the response times are: `metrics.query` with these. */
  series: {
    device_id: string;
    /** The series instance — the check's id. */
    instance: string;
    metrics: string[];
  };
}

export interface CheckKindInfo {
  kind: CheckKind;
  /** One line a form can show beside the name. */
  what: string;
  default_timeout_ms: number;
  sees_certificates: boolean;
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
 * `monitor.ifindex` — which SNMP ifIndex is which interface of a device, as
 * its agent last said. `DeviceIdParams` → `IfIndexMap`.
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
/**
 * `alerts.clear` — the condition another feature raised an alert for is
 * over: resolve the open alert under that key. `AlertClearParams` →
 * `AlertClearResult`. Refuses with `forbidden` when the alert belongs to an
 * alert rule.
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

/**
 * `alerts.changed` — an alert was raised, counted, announced late,
 * acknowledged, snoozed or resolved.
 */
export interface AlertsChangedEvent {
  alert: Alert;
  /**
   * `'firing'` on the one event at which the firing notification went to
   * the channels, `'resolved'` on the one at which the resolved one did,
   * `null` on every other change. Something that shows its own notification
   * goes by this field alone — not by `alert.count`, `muted_by_maintenance`
   * or `lab`: it already accounts for maintenance windows, snoozes, the lab
   * setting and `notify: []`.
   */
  notification: 'firing' | 'resolved' | null;
}

/** `checks.ran` — one service check has run. Published after every run. */
export interface ChecksRanEvent {
  check_id: string;
  device_id: string;
  name: string;
  kind: CheckKind;
  at: number;
  status: CheckStatus;
  previous: CheckStatus;
  /** Whether this run changed the status, and so added to the history. */
  changed: boolean;
  code: CheckCode | string;
  rtt_ms: number | null;
}

/**
 * `monitor.status_changed` — the setup moved: rules, maintenance windows, a
 * channel, or the service checks. `what` is `rules`, `maintenance`,
 * `channel` or `checks`.
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
  | { topic: 'checks.ran'; payload: ChecksRanEvent }
  | { topic: 'monitor.status_changed'; payload: MonitorStatusChangedEvent };
