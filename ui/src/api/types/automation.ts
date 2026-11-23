/**
 * The automation feature (`nocturne-automation`): the command runner,
 * configuration templates, playbooks, saved tasks, schedules, event triggers
 * and Ansible interoperability.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), optional fields are `?`, and
 * every timestamp is milliseconds since the Unix epoch. Backend source:
 * `crates/nocturne-automation/src/{model,runner,compare,templates,playbooks,
 * tasks,schedules,triggers,ansible,settings}.rs`.
 *
 * Each RPC method is documented as `method` {@link Params} → {@link Result}.
 * A field written as absent (`?`) may arrive either genuinely absent or as an
 * explicit `null`, and the two mean the same thing: serde omits a `None` only
 * where the struct asks it to, and most of these do not. So test for a value,
 * never for `undefined` — `given()` in `pages/automation/given.ts` is the
 * guard. What the `?` does say is that the *absence itself* carries meaning
 * worth distinguishing: `parsed` with no value means "no parser covers this
 * command", which is a different thing from "the parser matched nothing".
 *
 * Two things to keep in mind when building screens on this:
 *
 * 1. **Nothing here configures a device.** A configuration template or a
 *    playbook's `template` step produces a *change* (`change_id`), which still
 *    has to be planned, approved and applied through the change screens. The
 *    only exception is `ansible.run`, which runs somebody else's playbook and
 *    says on every run that none of the change pipeline's guarantees apply —
 *    see {@link AnsibleStatus.caveats} and {@link Run.caveat}.
 * 2. **A run never shrinks.** `results.length` always equals the number of
 *    devices the selector picked; a device that could not be reached is a
 *    result with `state: 'unreached'`, not a missing row.
 */

// --------------------------------------------------------------- selection

/**
 * Device attributes to match. Every field given must match; `tags` means the
 * device must carry all of them.
 */
export interface Filter {
  /** Free text over name, address, site, role, tags, profile, model, serial. */
  query?: string;
  site?: string;
  role?: string;
  tag?: string;
  tags?: string[];
  profile?: string;
  /** `up`, `down` or `unknown` — as monitoring last saw the device. */
  status?: 'up' | 'down' | 'unknown';
  /** `true` only lab devices, `false` only real ones. */
  lab?: boolean;
  /** `true` only protected devices, `false` only unprotected ones. */
  protected?: boolean;
  /** At most this many devices, after sorting by name. */
  limit?: number;
}

/**
 * Which devices something runs on. A filter is resolved **when the run
 * starts**, so a task written for "every access switch in Oslo" keeps meaning
 * that. Ids and a filter combine as a union; `exclude_device_ids` is then
 * taken away. An id that names no device is an error — a typo must not quietly
 * shrink a run.
 *
 * Every method that takes a selection takes these three fields at the top
 * level of its params, not nested under a `select` key.
 */
export interface Selector {
  /** Device ids, names or addresses. */
  device_ids?: string[];
  filter?: Filter;
  exclude_device_ids?: string[];
}

// ------------------------------------------------------------ small shapes

/**
 * `{id}` — what every `*.get` and `*.delete` in this feature takes.
 *
 * (Added by the Automation screen: the method list below documented the shape
 * but nothing named it. `monitor.ts` has the same two types for the same
 * reason; a third screen wanting them is the point at which they should move
 * into `core.ts`.)
 */
export interface IdParams {
  id: string;
}

/** What a `*.delete` answers. */
export interface Deleted {
  deleted: boolean;
}

// -------------------------------------------------------------------- runs

export type RunKind = 'commands' | 'playbook' | 'ansible';

/**
 * How one device fared. `unreached` is not `failed`: the device was in the run
 * and Nocturne never got to speak to it. `skipped` means it was deliberately
 * not started — it was busy in another run, the run was stopped first, or
 * (for Ansible) the playbook's own `hosts:` pattern left it out.
 */
export type DeviceState = 'ok' | 'failed' | 'unreached' | 'skipped';

/** The header of a run, as history lists it. */
export interface Run {
  id: string;
  kind: RunKind;
  name: string;
  /** The saved task this came from, when it came from one. */
  task_id?: string;
  /** `user`, `schedule`, `trigger` or `assistant`. */
  origin: string;
  job_id: string;
  /** The commands a `commands` run sent, in order. */
  commands: string[];
  playbook_id?: string;
  started_at: number;
  finished_at?: number;
  /** Devices the run selected — including the ones it could not reach. */
  devices: number;
  ok: number;
  failed: number;
  unreached: number;
  skipped: number;
  cancelled: boolean;
  /** Nothing was sent: this was a dry run. */
  dry_run: boolean;
  /**
   * Changes a playbook or template run handed to the change pipeline. Nothing
   * in here has been applied; each still needs its own approval.
   */
  change_ids: string[];
  /** Present on every run that is not covered by the change pipeline. */
  caveat?: string;
}

/** One command and what the device printed. */
export interface CommandResult {
  command: string;
  output: string;
  /** The device's own error line, when it rejected the command. */
  error?: string;
  elapsed_ms: number;
  /** The output was longer than the cap and was cut (the marker is in `output`). */
  truncated: boolean;
  /**
   * Rows from the matching ntc-template, when `parse` was asked for and one
   * exists. Absent is "no template", never "nothing matched".
   */
  parsed?: Record<string, string | string[]>[];
}

/** One playbook step on one device. */
export interface StepResult {
  name: string;
  state: 'ok' | 'failed' | 'skipped';
  detail: string;
  outputs: CommandResult[];
}

/** What one device did. */
export interface DeviceResult {
  device_id: string;
  name: string;
  state: DeviceState;
  /** `state === 'ok'`. */
  ok: boolean;
  error?: string;
  /** Why a device was skipped, or what a dry run would have done. */
  reason?: string;
  outputs: CommandResult[];
  /** Per step, for a playbook or Ansible run. */
  steps: StepResult[];
  change_id?: string;
  elapsed_ms: number;
}

// ------------------------------------------------------------------ runner

/** `runner.run` params. */
export interface RunnerRunParams extends Selector {
  /** Read-only commands; at most 50, one per entry. */
  commands: string[];
  /** Shown in history; defaults to the commands joined. */
  name?: string;
  /** Run each output through the device type's ntc-template, if there is one. */
  parse?: boolean;
  /** Say what would be sent and send nothing. */
  dry_run?: boolean;
  /** `user` (the default), `schedule`, `trigger`, `assistant`. */
  origin?: string;
  task_id?: string;
}

/** What a method that starts a run returns. */
export interface Started {
  job_id: string;
  run_id: string;
}

/** `runner.runs` params. */
export interface RunnerRunsParams {
  limit?: number;
  task_id?: string;
  kind?: RunKind;
}

/** `runner.get` params. */
export interface RunnerGetParams {
  run_id: string;
  /** Only this device's result. */
  device_id?: string;
}

/** `runner.get` / `playbooks.run_get` result. */
export interface RunDetail {
  run: Run;
  results: DeviceResult[];
}

/** `runner.compare` params. */
export interface CompareParams {
  run_id: string;
  command: string;
  /** Extra regular expressions whose lines are ignored when grouping. */
  ignore?: string[];
}

/** A device in a comparison group. */
export interface GroupMember {
  device_id: string;
  name: string;
}

/** One line of a diff: `=` same, `-` only in the majority, `+` only here. */
export interface DiffLine {
  op: '=' | '-' | '+';
  text: string;
}

export interface CompareGroup {
  devices: GroupMember[];
  /** The output every device in the group produced. */
  output: string;
  /** Against the largest group; empty for the largest group itself. */
  diff: DiffLine[];
}

/** A device in the run with no output for the command being compared. */
export interface CompareMissing {
  device_id: string;
  name: string;
  state: DeviceState;
  reason?: string;
}

/** `runner.compare` result: "47 switches say X, these 3 differ". */
export interface Comparison {
  command: string;
  /** Devices that have output for this command. */
  devices: number;
  missing: CompareMissing[];
  /** Largest first; the first group is the one the others are diffed against. */
  groups: CompareGroup[];
  /** The regular expressions whose lines were left out of the comparison. */
  ignored_patterns: string[];
}

/** `runner.search` params. */
export interface SearchParams {
  run_id: string;
  query: string;
  /** Treat `query` as a regular expression. */
  regex?: boolean;
}

/** One matching line. */
export interface SearchMatch {
  device_id: string;
  name: string;
  command: string;
  /** 1-based, within that command's output. */
  line_no: number;
  line: string;
}

/** `runner.export` params. */
export interface ExportParams {
  run_id: string;
  format?: 'csv' | 'json' | 'text';
}

/** `runner.export` result. */
export interface Exported {
  text: string;
}

/** `runner.delete` params. */
export interface DeleteRunsParams {
  run_ids: string[];
}

/** `runner.delete` result. */
export interface Removed {
  removed: number;
}

/**
 * - `runner.run` {@link RunnerRunParams} → {@link Started}. The run is refused
 *   up front, with nothing sent, if any command is not read-only on any
 *   target's device type.
 * - `runner.runs` {@link RunnerRunsParams} → {@link Run}[] (newest first)
 * - `runner.get` {@link RunnerGetParams} → {@link RunDetail}
 * - `runner.compare` {@link CompareParams} → {@link Comparison}
 * - `runner.search` {@link SearchParams} → {@link SearchMatch}[]
 * - `runner.export` {@link ExportParams} → {@link Exported}
 * - `runner.delete` {@link DeleteRunsParams} → {@link Removed}
 */
export type RunnerMethods = never;

// --------------------------------------------------------------- templates

export type TemplateKind = 'config' | 'commands';

export type VariableKind =
  | 'string'
  | 'int'
  | 'bool'
  | 'ip'
  | 'cidr'
  | 'vlan'
  | 'interface'
  | 'list'
  | 'choice';

export interface Variable {
  name: string;
  /** What a form and an error message call it; falls back to `name`. */
  label: string;
  kind: VariableKind;
  required: boolean;
  default?: unknown;
  /** Required when `kind` is `choice`. */
  choices: string[];
  help: string;
  /**
   * A `string` that may span lines (a banner). Every other value is one line:
   * the backend refuses a line break in it. Always in an answer; may be left
   * out of `templates.save`, where it means `false`.
   */
  multiline?: boolean;
}

/**
 * A MiniJinja body that renders either to configuration lines (`config`, which
 * become a change) or to read-only commands (`commands`, which become a run).
 *
 * The render context is `device` (`id`, `name`, `address`, `site`, `role`,
 * `tags`, `profile`, `notes`, `protected`, `is_lab`), `facts`, `interfaces`,
 * `neighbors`, `vlans` (the latest collected data), `vars`, and each variable
 * at the top level. Undefined is strict: a name the context does not have is
 * an error naming it, never an empty string.
 *
 * Built-in templates (`builtin: true`) are read-only; duplicate one to edit
 * it. No built-in takes a password, a community string or a key: a rendered
 * line ends up in a change document and in job output, and secrets stay in
 * the vault.
 */
export interface Template {
  id: string;
  name: string;
  description: string;
  kind: TemplateKind;
  body: string;
  builtin: boolean;
  /** Device types it is written for; empty = any. */
  profiles: string[];
  variables: Variable[];
  tags: string[];
  updated_at: number;
}

/** `templates.save` params (omit `id` to create). */
export interface TemplateSaveParams {
  id?: string;
  name: string;
  description?: string;
  kind?: TemplateKind;
  body: string;
  profiles?: string[];
  variables?: Variable[];
  tags?: string[];
}

/** `templates.duplicate` params. */
export interface DuplicateParams {
  id: string;
  name?: string;
}

/** `templates.validate` params. */
export interface ValidateTemplateParams {
  body: string;
  variables?: Variable[];
}

/** A problem with a template body, located where it can be. */
export interface TemplateProblem {
  line?: number;
  message: string;
}

/** `templates.validate` result. */
export interface TemplateValidation {
  ok: boolean;
  problems: TemplateProblem[];
  /** The names the body reads, including `device`, `facts`, … */
  used_variables: string[];
  /** Names it reads that are neither declared nor part of the context. */
  undeclared: string[];
}

/** `templates.render` params: give a `template_id` or a `body`. */
export interface RenderParams extends Selector {
  template_id?: string;
  body?: string;
  /** Only with `body`: the variables to validate `vars` against. */
  variables?: Variable[];
  vars?: Record<string, unknown>;
}

/** One device's preview. Per device, so one failure does not hide forty. */
export interface Rendered {
  device_id: string;
  name: string;
  ok: boolean;
  lines?: string[];
  error?: string;
}

/** `templates.apply` params. */
export interface ApplyParams extends Selector {
  template_id: string;
  vars?: Record<string, unknown>;
  /** The change's title, or the run's name; defaults to the template's name. */
  title?: string;
  /** Render and report; create no change and send nothing. */
  dry_run?: boolean;
  origin?: string;
  task_id?: string;
}

/** What `templates.apply` returns for a `config` template. */
export interface Applied {
  change_id: string;
}

/** What `templates.apply` returns for a `config` template with `dry_run`. */
export interface AppliedDryRun {
  dry_run: true;
  would_create: string;
  title: string;
  rendered: Rendered[];
}

/**
 * - `templates.list` `{}` → {@link Template}[] (built-ins first)
 * - `templates.get` `{id}` → {@link Template}
 * - `templates.save` {@link TemplateSaveParams} → {@link Template}
 * - `templates.delete` `{id}` → `{deleted: true}`
 * - `templates.duplicate` {@link DuplicateParams} → {@link Template}
 * - `templates.validate` {@link ValidateTemplateParams} → {@link TemplateValidation}
 * - `templates.render` {@link RenderParams} → {@link Rendered}[] (pure preview)
 * - `templates.apply` {@link ApplyParams} → {@link Applied} for a `config`
 *   template (one change, one target per device, nothing applied),
 *   {@link AppliedDryRun} with `dry_run`, or {@link Started} for a `commands`
 *   template.
 */
export type TemplateMethods = never;

// --------------------------------------------------------------- playbooks

/**
 * A playbook is YAML: `name`, `description`, `vars` and `steps`. Each step
 * does exactly one thing — `collect`, `commands` (read-only), `assert`,
 * `backup`, `template`, `set` or `pause` — and may carry `name`, `when`,
 * `register` and `ignore_errors`.
 *
 * ```yaml
 * name: Pre-change health check
 * vars: { min_uplinks: 2 }
 * steps:
 *   - name: Collect state
 *     collect: [facts, interfaces, neighbors]
 *   - name: Save evidence
 *     commands: [show version]
 *     register: evidence
 *   - name: Uplinks are up
 *     assert:
 *       that: "interfaces | selectattr('oper_up') | list | length >= vars.min_uplinks"
 *       message: "{{ device.name }} has fewer than {{ vars.min_uplinks }} uplinks up"
 *   - name: Back up
 *     backup: true
 *   - name: NTP
 *     template: { id: ios_ntp, vars: { servers: [192.0.2.1] } }
 *     when: "device.profile == 'cisco_ios'"
 * ```
 *
 * `template` steps are accumulated per device and become **one change at the
 * end of the run**; nothing is sent. A failed step stops that device only.
 */
export interface Playbook {
  id: string;
  name: string;
  description: string;
  yaml: string;
  updated_at: number;
}

/** `playbooks.save` params (omit `id` to create). */
export interface PlaybookSaveParams {
  id?: string;
  name: string;
  description?: string;
  yaml: string;
}

/** A problem with a playbook; `step` is 1-based when the problem is in a step. */
export interface PlaybookProblem {
  step?: number;
  message: string;
}

/** `playbooks.validate` result. */
export interface PlaybookValidation {
  ok: boolean;
  problems: PlaybookProblem[];
  steps: number;
}

/** `playbooks.run` params: give a `playbook_id` or a `yaml`. */
export interface PlaybookRunParams extends Selector {
  playbook_id?: string;
  yaml?: string;
  /** Merged over the playbook's own `vars`. */
  vars?: Record<string, unknown>;
  /** Walk the steps against what was last collected; touch nothing. */
  dry_run?: boolean;
  origin?: string;
  task_id?: string;
}

/**
 * - `playbooks.list` `{}` → {@link Playbook}[]
 * - `playbooks.get` `{id}` → {@link Playbook}
 * - `playbooks.save` {@link PlaybookSaveParams} → {@link Playbook}
 * - `playbooks.delete` `{id}` → `{deleted: true}`
 * - `playbooks.validate` `{yaml}` → {@link PlaybookValidation}
 * - `playbooks.run` {@link PlaybookRunParams} → {@link Started}
 * - `playbooks.runs` `{limit?}` → {@link Run}[]
 * - `playbooks.run_get` `{run_id}` → {@link RunDetail}
 */
export type PlaybookMethods = never;

// ------------------------------------------------------------------- tasks

/** What a task does. Nothing here sends configuration itself. */
export type Action =
  | { type: 'commands'; commands: string[]; parse?: boolean }
  | {
      type: 'playbook';
      playbook_id?: string;
      yaml?: string;
      vars?: Record<string, unknown>;
    }
  | { type: 'template'; template_id: string; vars?: Record<string, unknown> }
  | {
      type: 'ansible';
      playbook_path: string;
      /** `--check`; `true` unless said otherwise. */
      check?: boolean;
      extra_args?: string[];
    };

/** A saved piece of repetitive work: what to run, and where. */
export interface Task {
  id: string;
  name: string;
  description: string;
  select: Selector;
  action: Action;
  tags: string[];
  created_at: number;
  updated_at: number;
  last_run_at?: number;
  last_run_id?: string;
  /** `true` when the last run reached every device without a failure. */
  last_ok?: boolean;
}

/** `tasks.save` params (omit `id` to create). */
export interface TaskSaveParams {
  id?: string;
  name: string;
  description?: string;
  select?: Selector;
  action: Action;
  tags?: string[];
}

/** `tasks.run` params. */
export interface TaskRunParams {
  id: string;
  /** Say what would happen; send nothing and create nothing. */
  dry_run?: boolean;
  /** Narrow this one run to these devices, without changing the task. */
  device_ids?: string[];
  origin?: string;
}

/** `tasks.devices` params: a saved task's selection, or one being written. */
export interface TaskDevicesParams extends Selector {
  id?: string;
}

/** One device a selection currently picks. */
export interface SelectedDevice {
  device_id: string;
  name: string;
  address: string;
  profile: string;
  site: string;
  role: string;
  protected: boolean;
}

/** `tasks.devices` result: what the filter means right now. */
export interface SelectedDevices {
  count: number;
  devices: SelectedDevice[];
}

/**
 * - `tasks.list` `{}` → {@link Task}[]
 * - `tasks.get` `{id}` → {@link Task}
 * - `tasks.save` {@link TaskSaveParams} → {@link Task}
 * - `tasks.delete` `{id}` → `{deleted: true}`
 * - `tasks.run` {@link TaskRunParams} → {@link Started}, or {@link Applied}
 *   for a `template` task
 * - `tasks.devices` {@link TaskDevicesParams} → {@link SelectedDevices}
 */
export type TaskMethods = never;

// --------------------------------------------------------------- schedules

/**
 * When a schedule fires. `daily` and `cron` are in the machine's local time
 * zone: "02:30" stays 02:30 across a clock change, the hour that does not
 * exist in spring is skipped, and the hour that happens twice in autumn fires
 * once.
 */
export type When =
  | { type: 'interval'; every_s: number }
  | { type: 'daily'; at: string; days?: ('mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun')[] }
  | { type: 'cron'; expr: string };

/**
 * An RPC call a schedule or a trigger makes. The method must be one of
 * `config.backup`, `compliance.run`, `collect.run`, `discovery.scan`,
 * `runner.run`, `playbooks.run`, `reports.generate`, `insights.scan`,
 * `tasks.run` — and it must exist in this build. Nothing that changes a device
 * is on that list, now or later.
 */
export interface ActionCall {
  method: string;
  params?: Record<string, unknown>;
}

export interface Schedule {
  id: string;
  name: string;
  enabled: boolean;
  when: When;
  action: ActionCall;
  last_run_at?: number;
  last_job_id?: string;
  last_ok?: boolean;
  last_error?: string;
  /** `null` while the schedule is disabled. */
  next_run_at?: number;
  created_at: number;
}

/** `schedules.save` params (omit `id` to create). */
export interface ScheduleSaveParams {
  id?: string;
  name: string;
  /** Defaults to `true`. */
  enabled?: boolean;
  when: When;
  action: ActionCall;
}

/** `schedules.preview` params. */
export interface PreviewParams {
  when: When;
  /** Default 5. */
  count?: number;
}

/** `schedules.preview` result. */
export interface Preview {
  next: number[];
}

/**
 * - `schedules.list` `{}` → {@link Schedule}[]
 * - `schedules.save` {@link ScheduleSaveParams} → {@link Schedule} (validates
 *   `when` and `action`, works out `next_run_at`)
 * - `schedules.delete` `{id}` → `{deleted: true}`
 * - `schedules.run_now` `{id}` → the action's own result; `conflict` when the
 *   schedule's previous run has not finished
 * - `schedules.preview` {@link PreviewParams} → {@link Preview}
 */
export type ScheduleMethods = never;

// ---------------------------------------------------------------- triggers

/**
 * Run something when the rest of the application says something happened.
 *
 * Events arrive in bursts, so a trigger collects what arrives for `debounce_s`
 * and fires **once**; while its own previous run is still going it holds the
 * burst rather than starting a second one. A trigger may not watch the topics
 * automation publishes itself (`automation.`, `schedules.`, `triggers.`,
 * `tasks.`, `templates.`, `playbooks.`) — that would be a loop.
 */
export interface Trigger {
  id: string;
  name: string;
  enabled: boolean;
  /** An event topic exactly (`alerts.raised`) or as a prefix (`alerts.`). */
  topic: string;
  /**
   * Fire only when every JSON pointer into the payload has the given value,
   * e.g. `{"/severity": "critical"}`.
   */
  match_payload?: Record<string, unknown>;
  /** Seconds to collect events before firing once; `0` fires at the next sweep. */
  debounce_s: number;
  action: ActionCall;
  /** Pass the device the event was about to the action as `device_ids`. */
  pass_device: boolean;
  last_fired_at?: number;
  last_job_id?: string;
  fired_count: number;
  created_at: number;
}

/** `triggers.save` params (omit `id` to create). */
export interface TriggerSaveParams {
  id?: string;
  name: string;
  /** Defaults to `true`. */
  enabled?: boolean;
  topic: string;
  match_payload?: Record<string, unknown>;
  debounce_s?: number;
  action: ActionCall;
  pass_device?: boolean;
}

/**
 * - `triggers.list` `{}` → {@link Trigger}[]
 * - `triggers.get` `{id}` → {@link Trigger}
 * - `triggers.save` {@link TriggerSaveParams} → {@link Trigger}
 * - `triggers.delete` `{id}` → `{deleted: true}`
 * - `triggers.fire_now` `{id}` → the action's own result
 */
export type TriggerMethods = never;

// ----------------------------------------------------------------- ansible

/**
 * What this machine has, and what the Ansible path does not promise.
 *
 * `caveats` is not decoration: show it. A playbook run has none of the change
 * pipeline's guarantees — no backup, no drift check, no approved plan, no
 * verification and **no rollback** — and `caveats[0]` says exactly that.
 */
export interface AnsibleStatus {
  installed: boolean;
  version?: string;
  path?: string;
  /** `null` when `ansible-galaxy` could not be asked — not "none installed". */
  collections?: string[];
  /** Why it is not usable here, when it is not. */
  unsupported?: string;
  caveats: string[];
}

/** `ansible.export_playbook` params: a change, or a template with devices. */
export interface ExportPlaybookParams extends Selector {
  change_id?: string;
  template_id?: string;
  vars?: Record<string, unknown>;
}

/**
 * `ansible.export_playbook` result. Neither document contains a secret: the
 * inventory refers to credentials as `{{ lookup('env', 'NOCTURNE_PW_1') }}`.
 *
 * A device type with no module Nocturne is certain of fails with the error
 * code `unsupported`, naming the platform, rather than guessing.
 */
export interface ExportedPlaybook {
  playbook: string;
  inventory: string;
  caveat: string;
}

/** `ansible.run` params. Desktop only: it starts a local process. */
export interface AnsibleRunParams extends Selector {
  playbook_path: string;
  /** Passed to `--limit`; defaults to the generated hosts. */
  limit?: string;
  /** `--check`; `true` unless said otherwise. */
  check?: boolean;
  /**
   * Extra arguments. `-i`/`--inventory`, `-l`/`--limit` and the
   * password-prompt flags are refused with the reason.
   */
  extra_args?: string[];
  origin?: string;
  task_id?: string;
  /** Report the command line, the inventory and the devices; start nothing. */
  dry_run?: boolean;
}

/** `ansible.run` result: follow the job for the streamed output. */
export interface AnsibleStarted {
  job_id: string;
}

/**
 * - `ansible.status` `{}` → {@link AnsibleStatus}
 * - `ansible.export_playbook` {@link ExportPlaybookParams} → {@link ExportedPlaybook}
 * - `ansible.run` {@link AnsibleRunParams} → {@link AnsibleStarted}. The job's
 *   result carries `{run_id, ok, failed, unreached, skipped,
 *   outside_change_pipeline: true, caveat}`; the per-device outcome is read
 *   with `runner.get`.
 */
export type AnsibleMethods = never;

// ---------------------------------------------------------------- settings

/** The `automation` settings section (`settings.get`/`settings.set`). */
export interface AutomationSettings {
  /** Devices worked on at once in one run (1–64). */
  concurrency: number;
  /** Runs kept in history; the oldest are deleted with their outputs. */
  keep_runs: number;
  /** Cap on one command's output on one device, in bytes. */
  output_cap_bytes: number;
  /** Regular expressions whose lines `runner.compare` ignores. */
  volatile_patterns: string[];
  schedules_enabled: boolean;
  triggers_enabled: boolean;
  /** A schedule missed by more than this moves on instead of catching up. */
  catch_up_within_s: number;
  /** `ansible-playbook` to use; empty = look it up on `PATH`. */
  ansible_path: string;
  ansible_timeout_s: number;
}

// ------------------------------------------------------------------ events

/**
 * Events this feature publishes.
 *
 * - `automation.runs.changed` → {@link Run} (when a run starts and when it ends)
 * - `tasks.changed` → {@link Task}, or `{id, deleted: true}`
 * - `templates.changed` → `{id}`
 * - `playbooks.changed` → `{id}`
 * - `schedules.changed` → {@link Schedule}, or `{id, deleted: true}`
 * - `schedules.fired` → {@link ScheduleFired}
 * - `triggers.changed` → {@link Trigger}, or `{id, deleted: true}`
 * - `triggers.fired` → {@link TriggerFired}
 *
 * Progress and the streamed output of a run arrive as the core's own
 * `jobs.changed` (`Job` in `core.ts`) and `jobs.log` events.
 */
export interface ScheduleFired {
  id: string;
  job_id?: string;
  at: number;
}

export interface TriggerFired {
  id: string;
  /** The topic of the last event in the burst. */
  topic: string;
  /** How many events the debounce window collected. */
  events: number;
  job_id?: string;
  device_ids: string[];
}
