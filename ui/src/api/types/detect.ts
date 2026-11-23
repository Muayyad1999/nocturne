/**
 * Detections (`nocturne-detect`): a short, fixed catalogue of things worth
 * telling somebody about that no single log line and no single metric shows,
 * because each needs two sources put side by side.
 *
 * Field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch. Unlike most of the surface, a field with nothing to say
 * is **absent** here, not `null` (`device_id`, `alert_id`,
 * `unavailable_reason`, `last_hit_at`, `last_error`). Backend source:
 * `crates/nocturne-detect`. Specification: `docs/dev/features/detect.md`.
 * Read from a running `nocturne-server`: the catalogue, the switch and its
 * refusal, and hits with their evidence decided from a lab switch's own log
 * line, from two backups around an edit at its console, from a restart, from
 * a credential's secret replaced and a master password set and removed, from
 * wrong vault passwords and wrong server tokens, and from unredacted views.
 * The evidence of `host_key_changed` and `protected_change_outside_window`
 * is the specification's table alone, which that crate's tests assert field
 * by field.
 *
 * Three things a screen must not get wrong:
 *
 * * **A detection only tells.** It never changes a device and never drafts a
 *   change. A hit goes into the one alert list (`alert_id`); there is no
 *   second inbox.
 * * **`enabled` and `available` are independent.** `enabled` is the switch;
 *   `available` is whether this build can run it. A detection can be
 *   switched on and unavailable, and then it says why and stays silent.
 * * **A check that could not be made is not a check that found nothing.**
 *   `last_error` stays until the next check of that detection goes through.
 *
 * There are no users: Nocturne has one operator identity, so "who" in a hit
 * is whatever the device said (`by`, `from`), never an account.
 */

export type DetectionSeverity = 'info' | 'low' | 'medium' | 'high' | 'critical';

/** The ids of the catalogue, in catalogue order. */
export type DetectionId =
  | 'unplanned_config_change'
  | 'unplanned_reload'
  | 'host_key_changed'
  | 'unredacted_config_views'
  | 'secrets_changed'
  | 'failed_unlocks'
  | 'protected_change_outside_window';

/** A check of a detection that could not be made, and when. */
export interface DetectionCheckError {
  at: number;
  message: string;
}

/** One entry of the catalogue, as `detections.list` returns it. */
export interface Detection {
  /** A {@link DetectionId}; typed as text so a newer backend's entry still lists. */
  id: string;
  name: string;
  description: string;
  /** The usual severity; a hit carries its own. */
  severity: DetectionSeverity;
  /** What it is decided from, in a sentence that names its sources. */
  decided_from: string;
  /** Its stated blind spot. */
  cannot_see: string;
  /** The switch: `detect.enabled` and not in `detect.disabled`. */
  enabled: boolean;
  /** Whether this build can run it. */
  available: boolean;
  /** Why it cannot, when it cannot: the method that is missing. */
  unavailable_reason?: string;
  /** Survives the hit ageing out of the `event` stream. */
  last_hit_at?: number;
  /** Counted from the `event` stream, up to 100,000. */
  hits_24h: number;
  last_error?: DetectionCheckError;
}

export interface DetectionSetEnabledParams {
  id: string;
  enabled: boolean;
}

export interface DetectionRecentParams {
  /** Only one detection; `invalid` for an id that is not in the catalogue. */
  id?: string;
  device_id?: string;
  since?: number;
  /** Default 100, 1 to 1,000. */
  limit?: number;
}

/** The change that was looked at and did not explain what happened. */
export interface NearestChange {
  change_id: string;
  state: string;
  updated_at: number;
}

/** `unplanned_config_change`, decided from a log line. */
export interface ConfigChangeLogEvidence {
  decided_from: 'log';
  log_entry_id: number;
  stream?: string;
  /** `syslog`, `lab` or `pull`: how the line got here. */
  via?: string;
  /** Who, as the device said it, cut to 64 printable characters. */
  by?: string;
  /** From where, as the device said it. */
  from?: string;
  changes_checked: number;
  nearest_change?: NearestChange;
  grace_minutes: number;
  /** Present when the line came over unauthenticated syslog or a v1/v2c trap. */
  caution?: string;
}

/** `unplanned_config_change`, decided from two backups that differ. */
export interface ConfigChangeBackupEvidence {
  decided_from: 'backup';
  /** `running` or `startup`. */
  kind: string;
  version_id: number;
  previous_id: number;
  /** `from`: when the earlier version was last fetched unchanged; `to`: when the later one was first fetched. */
  between: { from: number; to: number };
  lines_added: number;
  lines_removed: number;
  changes_checked: number;
  nearest_change?: NearestChange;
  grace_minutes: number;
}

/** `unplanned_reload`. */
export interface ReloadEvidence {
  decided_from: 'log';
  log_entry_id: number;
  rule_id: string;
  stream?: string;
  via?: string;
  changes_checked: number;
  nearest_change?: NearestChange;
  jobs_checked: number;
  grace_minutes: number;
  caution?: string;
}

/** `host_key_changed`. */
export interface HostKeyEvidence {
  decided_from: 'connect.host_key_changed';
  host: string;
  port: number;
  known: { algorithm: string; fingerprint: string; first_seen: number };
  presented: { algorithm: string; fingerprint: string };
  /** The inventory devices with that address and SSH port: none, one or several. */
  device_ids: string[];
  host_key_policy: string;
  connection: 'refused' | 'went_ahead';
}

/** `unredacted_config_views`. */
export interface UnredactedViewsEvidence {
  decided_from: 'audit';
  action: string;
  count: number;
  window_min: number;
  /** At most 50. */
  audit_entry_ids: number[];
  through_audit_id: number;
  device_ids: string[];
  /** A sentence: there are no users, so this cannot say who. */
  who: string;
}

/** `secrets_changed`. */
export interface SecretsChangedEvidence {
  decided_from: 'audit';
  action: string;
  audit_entry_id: number;
  who: string;
}

/** `failed_unlocks`. */
export interface FailedUnlocksEvidence {
  decided_from: 'audit';
  what: 'vault' | 'server';
  action: string;
  count: number;
  window_min: number;
  audit_entry_ids: number[];
  through_audit_id: number;
  /** `server` only: the addresses the requests came from. */
  from?: string[];
}

/** `protected_change_outside_window`. */
export interface ProtectedChangeEvidence {
  decided_from: 'changes.changed';
  change_id: string;
  state: string;
  finished_at: number;
  devices: { device_id: string; protected_hits: number }[];
  devices_no_longer_in_inventory: string[];
  windows_checked: number;
  grace_minutes: number;
}

/**
 * The records a hit was decided from, by id: log entry, change, audit entry.
 * Ids and counts, never the text of a log line, a configuration or anything
 * that was typed — apart from the two words a device says about a
 * configuration change, `by` and `from`.
 *
 * The union documents what each detection writes. A screen should still read
 * it as an open record (see `pages/logs/detections.ts`): the backend types it
 * as free JSON, and a newer detection must not break an older screen.
 */
export type DetectionEvidence =
  | ConfigChangeLogEvidence
  | ConfigChangeBackupEvidence
  | ReloadEvidence
  | HostKeyEvidence
  | UnredactedViewsEvidence
  | SecretsChangedEvidence
  | FailedUnlocksEvidence
  | ProtectedChangeEvidence;

/** One finding. Also the body of its entry in the `event` stream. */
export interface DetectionHit {
  id: string;
  /** The detection's id. */
  detection: string;
  /** When what it was decided from happened, not when it was judged. */
  at: number;
  /** Absent when the hit is about no one device. */
  device_id?: string;
  /** The hit's own severity (`secrets_changed` is `high` for a removed master password). */
  severity: DetectionSeverity;
  title: string;
  evidence: DetectionEvidence | Record<string, unknown>;
  /** Absent when `alerts.raise` is not in the build, or refused. */
  alert_id?: string;
  /**
   * The hit is about a simulated device. Its alert is sent to no
   * notification channel unless the monitor's `notify_for_lab` is on — the
   * monitor's rule; `alerts.raise` is called without `notify` for it.
   */
  lab: boolean;
}

/** The `detect` settings section, as `settings.get {section: 'detect'}` returns it. */
export interface DetectSettings {
  /** The whole feature. While it is off no detection can be switched on. */
  enabled: boolean;
  grace_minutes: number;
  unredacted_views: { count: number; window_min: number };
  failed_unlocks: { count: number; window_min: number };
  /** The ids switched off one by one. */
  disabled: string[];
}

/** `detections.list` {} → {@link Detection}[], in catalogue order */
/**
 * `detections.set_enabled` {@link DetectionSetEnabledParams} → {@link Detection}.
 * `invalid` for an id that is not in the catalogue; `conflict` for switching
 * one on while detections are off as a whole.
 */
/** `detections.recent` {@link DetectionRecentParams} → {@link DetectionHit}[], newest first */

/** `detections.hit`: a detection found something. */
export interface DetectionsHit {
  hit: DetectionHit;
}

/**
 * `detections.changed`: a switch moved, or a `last_error` appeared, changed
 * or cleared. Without `id`, refetch the list.
 */
export interface DetectionsChanged {
  id?: string;
}

/**
 * Events
 *
 * | Topic | Payload |
 * |---|---|
 * | `detections.hit` | {@link DetectionsHit} |
 * | `detections.changed` | {@link DetectionsChanged} |
 */
