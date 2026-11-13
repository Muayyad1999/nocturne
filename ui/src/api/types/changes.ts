/**
 * The change pipeline (`nocturne-changes`): the one path by which Nocturne
 * configures a device — plan, approve, apply, verify, roll back.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and `?` marks a field the backend may send as `null` (or a
 * parameter it defaults). Backend source: `crates/nocturne-changes`.
 * Specification: `docs/dev/features/changes.md`.
 *
 * The shape of the flow:
 *
 * 1. `changes.create` — what should happen, on which devices.
 * 2. `changes.plan` — for each device: the exact commands, the predicted
 *    configuration, the rollback, the risk and the protected resources.
 * 3. `changes.approve` — a signature on *those* plans (`plan_hash`), with the
 *    device's name typed out for every device with a protected hit. Planning
 *    again or editing the change invalidates it.
 * 4. `changes.apply` — connect, drift check, snapshot, backup, arm the
 *    dead-man switch, send, settle, verify, roll back on any failure, disarm,
 *    save, final backup.
 */

// ------------------------------------------------------------------- model

/**
 * One command and the device's answer (`nocturne_drivers::session::CommandOutput`).
 */
export interface CommandOutput {
  command: string;
  output: string;
  /** The line that matched one of the platform's error patterns, if any. */
  error?: string;
  elapsed_ms: number;
}

/** Where a change came from. */
export type ChangeOrigin =
  | 'user'
  | 'assistant'
  | 'template'
  | 'compliance'
  | 'restore'
  | 'drift'
  | 'playbook';

/** Where a change is in its life. */
export type ChangeState =
  | 'draft'
  | 'planned'
  | 'approved'
  | 'applying'
  | 'applied'
  | 'failed'
  | 'rolled_back'
  | 'partially_applied'
  | 'cancelled';

/** How a change ended on one device. */
export type TargetState =
  | 'pending'
  | 'applying'
  | 'applied'
  | 'failed'
  | 'rolled_back'
  | 'rollback_failed'
  | 'skipped'
  /** Unreachable after the session was lost; its dead-man switch will fire. */
  | 'guard_fired';

export type RiskLevel = 'low' | 'medium' | 'high' | 'critical';

/** Configuration-mode lines exactly as they would be typed. */
export interface LinesIntent {
  type: 'lines';
  /** Section headers included; abbreviations are refused at planning. */
  lines: string[];
}

/** A desired configuration; the commands are computed by a hierarchical diff. */
export interface IntendedIntent {
  type: 'intended';
  config: string;
  /**
   * `true`: the device must equal `config` (what is not in it is removed).
   * `false` (default): `config` must be present — removals are dropped.
   */
  exact?: boolean;
}

/** What a change is, before it is turned into commands. */
export type Intent = LinesIntent | IntendedIntent;

/** One line of a unified diff (`nocturne_drivers::hier::DiffLine`). */
export interface DiffLine {
  /** `" "`, `"+"`, `"-"` or `"@"` for a gap between hunks. */
  tag: string;
  text: string;
  old_line?: number;
  new_line?: number;
}

export interface TextDiff {
  lines: DiffLine[];
  added: number;
  removed: number;
}

/** How dangerous a plan is, and why — in words to show the approver. */
export interface Risk {
  level: RiskLevel;
  reasons: string[];
}

/** A resource that may not be touched without a human typing the device name. */
export interface ProtectedHit {
  /**
   * Which rule matched: `protected_device`, `management_interface`,
   * `management_vlan`, `default_route`, `default_gateway`, `ip_routing`,
   * `access_vty`, `access_user`, `access_enable`, `access_aaa`, `access_ssh`,
   * `access_crypto`, `access_http`, `access_acl`, `access_service`,
   * `access_firewall`, `trunk_allowed_replace`, `trunk_allowed`, `uplink`,
   * `vlan_removal`, `spanning_tree`, `routing_removal`, `hostname`.
   */
  kind: string;
  /** What is protected: an interface, a VLAN, an address, the device. */
  subject: string;
  /** The command that would touch it. */
  command: string;
  /** What would happen, in words. */
  why: string;
}

/** The dead-man switch armed around the change. */
export interface GuardPlan {
  /** The platform's name for it, e.g. `reload timer`. */
  name: string;
  minutes: number;
  /** Exec commands that arm it. */
  arm: string[];
  /** Exec commands that cancel it once the change is verified. */
  disarm: string[];
  /** What happens if it fires. */
  effect: string;
}

/** Exactly what will be sent to one device, and what it is predicted to do. */
export interface Plan {
  /** Exactly what will be sent in configuration mode, in order. */
  commands: string[];
  /** What undoes it; empty means it could not be derived (see `warnings`). */
  rollback: string[];
  /** The predicted running configuration, before → after. */
  diff: TextDiff;
  risk: Risk;
  /** Why this needs a typed confirmation at approval. */
  protected_hits: ProtectedHit[];
  /** Everything a person should read before approving. */
  warnings: string[];
  /** The running-config backup the plan was computed against. */
  base_version_id: number;
  base_sha256: string;
  /** sha256 of the predicted configuration; empty when it could not be predicted. */
  predicted_sha256: string;
  /** The dead-man switch that will be armed, if any. */
  guard?: GuardPlan;
  /** sha256 over device, commands, rollback, base_sha256 and options. */
  plan_hash: string;
  planned_at: number;
  /** The device type the plan was computed for. */
  profile: string;
  /** `verified`, `community` or `experimental`. */
  maturity: string;
}

/** What must still be true after the change. */
export interface Verify {
  /** Re-read the running configuration and check it is what was predicted. Default true. */
  config?: boolean;
  /** No LLDP/CDP neighbour that existed before may be gone. Default true. */
  neighbors?: boolean;
  /** No interface that was up may be down, except the ones being shut down. Default true. */
  interfaces?: boolean;
  /** Default false. */
  routes?: boolean;
  /** Addresses the device itself must still be able to ping. */
  pings?: string[];
  /** Seconds to wait before verifying (0 on lab devices). Default 5. */
  settle_s?: number;
}

/** How a change is applied. */
export interface Options {
  /** Write to the startup configuration after a verified apply. Default true. */
  save?: boolean;
  /** Arm the dead-man switch for this many minutes; omitted = decide from the risk. `0` = never. */
  guard_minutes?: number;
  /** Devices at a time. Default 1 — one after another, in order. */
  parallel?: number;
  /** A failed device stops the ones after it. Default true. */
  stop_on_failure?: boolean;
  verify?: Verify;
  /** Reserved for the lab twin; ignored for now. */
  rehearse?: boolean;
}

/** A human's (or the autonomy policy's) signature on specific plans. */
export interface Approval {
  /** `user`, or `policy:draft` / `policy:lab` / `policy:low_risk`. */
  by: string;
  at: number;
  /** The `plan_hash` of every target, as it was approved. */
  plan_hashes: string[];
  note: string;
  /** Devices whose name was typed out to confirm a protected hit. */
  confirmed_devices: string[];
}

/** One step of the apply pipeline. */
export interface Step {
  /**
   * `connect`, `drift check`, `snapshot`, `backup`, `arm guard`, `apply`,
   * `verify`, `rollback`, `reconnect`, `guard`, `disarm guard`, `save`.
   */
  name: string;
  ok: boolean;
  detail: string;
  at: number;
}

/** One check run after the change. */
export interface Check {
  /** `configuration`, `neighbours`, `interfaces`, `routes`, `state`, `ping <address>`. */
  name: string;
  ok: boolean;
  detail: string;
}

/** What happened on one device. */
export interface TargetResult {
  state: TargetState;
  started_at: number;
  finished_at?: number;
  steps: Step[];
  /** Every configuration line with the device's answer. */
  sent: CommandOutput[];
  /** Index into `plan.commands` of the line the device rejected. */
  failed_line?: number;
  error?: string;
  before_version_id?: number;
  after_version_id?: number;
  verification: Check[];
  /** Redacted session transcript. */
  transcript: string;
}

/** One device and what it should get. */
export interface Target {
  device_id: string;
  intent: Intent;
  plan?: Plan;
  result?: TargetResult;
}

/** One intended configuration change, from draft to applied. */
export interface Change {
  id: string;
  title: string;
  description: string;
  origin: ChangeOrigin;
  state: ChangeState;
  targets: Target[];
  options: Options;
  approval?: Approval;
  created_at: number;
  updated_at: number;
  /** `user`, `assistant`, or whatever created it. */
  created_by: string;
  /** Free-text reference (a change-request number). */
  ticket?: string;
  /** The job applying (or last applying) this change. */
  job_id?: string;
}

/** One device inside a {@link ChangeSummary}. */
export interface DeviceSummary {
  device_id: string;
  name: string;
  planned: boolean;
  risk?: RiskLevel;
  protected_hits: number;
  commands: number;
  state?: TargetState;
}

/** A change as a list shows it: no plans, results or transcripts. */
export interface ChangeSummary {
  id: string;
  title: string;
  description: string;
  origin: ChangeOrigin;
  state: ChangeState;
  ticket?: string;
  created_at: number;
  updated_at: number;
  created_by: string;
  job_id?: string;
  /** The worst risk of any planned target, once every target is planned. */
  risk?: RiskLevel;
  /** Protected resources over every target. */
  protected_hits: number;
  approved_by?: string;
  devices: DeviceSummary[];
}

// --------------------------------------------------------------- methods

export interface ChangesListParams {
  /** A {@link ChangeState}, or `open` for everything not finished. */
  state?: string;
  device_id?: string;
  origin?: ChangeOrigin;
  /** Default 50, at most 500. */
  limit?: number;
  /** The `id` of the last change of the previous page. */
  cursor?: string;
}

/** `changes.list` {@link ChangesListParams} → {@link ChangeSummary}[] (newest first) */

export interface ChangeIdParams {
  id: string;
}

/** `changes.get` {@link ChangeIdParams} → {@link Change} */

/** One device of a new change. */
export interface TargetInput {
  /** Device id, name or address. */
  device_id: string;
  intent: Intent;
}

export interface ChangesCreateParams {
  title: string;
  description?: string;
  origin?: ChangeOrigin;
  ticket?: string;
  targets: TargetInput[];
  options?: Options;
  /** Start planning straight away. */
  plan?: boolean;
  /** Who is creating it; defaults from `origin`. */
  created_by?: string;
}

/** The result of `changes.create` when `plan: true` was asked for. */
export interface ChangeCreatedWithJob {
  change: Change;
  job_id: string;
}

/** `changes.create` {@link ChangesCreateParams} → {@link Change}, or {@link ChangeCreatedWithJob} when `plan` is true */

export interface ChangesUpdateParams {
  id: string;
  title?: string;
  description?: string;
  ticket?: string;
  /** Replacing the targets discards every plan and the approval. */
  targets?: TargetInput[];
  /** Changing the options does the same. */
  options?: Options;
}

/** `changes.update` {@link ChangesUpdateParams} → {@link Change} (only while `draft` or `planned`) */

/** What a method that starts a job returns. */
export interface JobStarted {
  job_id: string;
}

/** `changes.plan` {@link ChangeIdParams} → {@link JobStarted}; fills `target.plan`, state → `planned` */

export interface ChangesApproveParams {
  id: string;
  /** The `plan_hash` of every target, exactly as they are now. */
  plan_hashes: string[];
  /** Device name → the same name typed out, for every device with a protected hit. */
  confirm?: Record<string, string>;
  note?: string;
  /** Who is approving; default `user`. */
  by?: string;
}

/** `changes.approve` {@link ChangesApproveParams} → {@link Change} (state `approved`) */

/** `changes.unapprove` {@link ChangeIdParams} → {@link Change} (back to `planned`) */

/** `changes.apply` {@link ChangeIdParams} → {@link JobStarted}; needs `approved` with the hashes still matching */

/** `changes.assistant_apply` {@link ChangeIdParams} → {@link JobStarted}, or `forbidden` with the reason a human is needed */

export interface ChangesRollbackParams {
  id: string;
  /** Devices to undo; empty = every device that was applied. */
  device_ids?: string[];
}

/** `changes.rollback` {@link ChangesRollbackParams} → {@link JobStarted} */

/** What `changes.cancel` answers. */
export interface ChangeCancelled {
  cancelled: boolean;
  /** `cancelled`, or `applying` when the running job was asked to stop. */
  state: ChangeState;
}

/** `changes.cancel` {@link ChangeIdParams} → {@link ChangeCancelled} */

/** What `changes.delete` answers. */
export interface ChangeDeleted {
  deleted: boolean;
}

/** `changes.delete` {@link ChangeIdParams} → {@link ChangeDeleted} (not while applying) */

export interface ChangesPreviewParams {
  /** Device id, name or address. */
  device_id: string;
  intent: Intent;
  options?: Options;
}

/** `changes.preview` {@link ChangesPreviewParams} → {@link Plan}, creating nothing */

// ---------------------------------------------------------------- events

/** `changes.changed` — any state change; the payload is the summary. */
export type ChangesChangedEvent = ChangeSummary;

/** `changes.progress` — one step of an apply, as it happens. */
export interface ChangesProgressEvent {
  change_id: string;
  device_id: string;
  /** See {@link Step.name}. */
  step: string;
  ok: boolean;
  detail: string;
}

// -------------------------------------------------------------- settings

/** The `changes` settings section (`settings.get` / `settings.set`). */
export interface ChangesSettings {
  /** Minutes the dead-man switch is armed for when the plan does not say. Default 10. */
  guard_minutes: number;
  /** The most devices a change may be applied to at once. Default 8. */
  parallel_limit: number;
  /** Attempts to reach a device again after a session is lost mid-apply. Default 3. */
  reconnect_attempts: number;
  /** Seconds between those attempts. Default 20. */
  reconnect_interval_s: number;
  /** Seconds to wait for a device another operation is holding. Default 45. */
  connect_wait_s: number;
  /** Keep the redacted session transcript in the change record. Default true. */
  keep_transcripts: boolean;
  /** Refuse to approve a change that names no ticket. Default false. */
  require_ticket: boolean;
}

/** How much the assistant may do by itself; part of the `assistant` section. */
export type Autonomy = 'draft' | 'lab' | 'low_risk';
