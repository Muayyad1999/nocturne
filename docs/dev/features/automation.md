# Feature: automation (`nocturne-automation`)

The repetitive part of the job, made repeatable: run the same `show` command
on fifty switches and compare, fill in a configuration template per device,
chain steps into a playbook, put any of it on a schedule — and interoperate
with Ansible for teams that already have playbooks.

It never configures a device itself. Templates and playbooks that produce
configuration hand it to the change pipeline (`changes.create`) where it is
planned, reviewed and applied.

Engines: `minijinja` (as configured in the workspace), `nocturne-drivers`
(`expr::Expressions` for the filters network engineers need: `ifname`,
`cidr`, `mac`, …), core `connect::exec`, `collect`, `jobs`.

## Command runner (contract)

| Method | Params → Result |
|---|---|
| `runner.run` | `{device_ids?, filter?, commands: [..], name?, parse?: bool}` → `{job_id, run_id}` — read-only commands (enforced by `connect::exec`; the whole run is refused up front if any command is not read-only on any target's platform, saying which) |
| `runner.runs` | `{limit?}` → `[{id, name, commands, started_at, finished_at, devices, ok, failed}]` |
| `runner.get` | `{run_id, device_id?}` → `{run, results: [{device_id, name, ok, error?, outputs: [{command, output, error?, elapsed_ms, parsed?: [rows]}]}]}` |
| `runner.compare` | `{run_id, command}` → groups of devices by identical output (after trimming volatile lines such as timestamps/uptime — configurable regex list), largest group first, with a diff of each other group against it: "47 switches say X, these 3 differ" |
| `runner.search` | `{run_id, query, regex?}` → matching lines per device |
| `runner.export` | `{run_id, format: csv|json|text}` → `{text}` |
| `runner.delete` | `{run_ids}` |

`parse: true` runs each output through the device profile's ntc-templates
platform (`nocturne_textfsm::ntc::parse(platform, command, output)`) when a
template exists; no template → `parsed` absent (not an error).
Outputs are stored per run (documents, capped at 2 MiB per device/command
with a truncation marker; keep the last 50 runs by default).

## Templates (contract)

MiniJinja text that renders to configuration lines (kind `config`) or to
read-only commands (kind `commands`).

```rust
struct Template { id, name, description, kind: config|commands, body, builtin: bool,
    profiles: Vec<String>,                       // device types it is written for; empty = any
    variables: Vec<Variable { name, label, kind: string|int|bool|ip|cidr|vlan|interface|list|choice, required, default, choices, help }>,
    tags, updated_at }
```

Render context: `device` (`name`, `address`, `site`, `role`, `tags`,
`profile`), `facts` (latest collected facts or undefined), `interfaces`,
`neighbors`, `vlans` (latest collected rows, or undefined — a template that
needs them must fail with "collect interfaces for X first", not render
nonsense), and `vars` plus each variable at top level. Strict undefined:
a missing variable is an error naming it, never an empty string. Lines are
trimmed of trailing space; blank lines dropped.

| Method | Params → Result |
|---|---|
| `templates.list` / `.get` / `.save` / `.delete` / `.duplicate` | CRUD; built-ins read-only |
| `templates.validate` | `{body, variables}` → `{ok, problems: [{line, message}], used_variables: [..], undeclared: [..]}` |
| `templates.render` | `{template_id | body, device_ids, vars}` → `[{device_id, name, ok, lines?: [..], error?}]` — pure preview |
| `templates.apply` | `{template_id, device_ids, vars, title?}` → `{change_id}` (kind `config`: one change with a `Lines` intent per device via `changes.create`, origin `template`) or `{job_id, run_id}` (kind `commands`: a runner run) |

Variable validation by kind (`vlan` 1–4094, `ip`/`cidr` parse, `interface`
non-empty…) before rendering, with the label in the message.

Built-in templates (embedded data, each with a render test against the fake
device's facts): for IOS — NTP servers, syslog hosts, SNMPv3 user/group,
login banner, add VLAN, access port (VLAN, description, portfast, optional
voice VLAN), trunk port (allowed VLANs **with `add`**), port description
from LLDP/CDP neighbour (`neighbors` loop), local user, management ACL on
VTY, DNS servers, disable unused port. For RouterOS — NTP client, remote
syslog, add VLAN on bridge, address + DHCP server for a new LAN, user,
restrict services to a management subnet. Commands a real device rejects are
worse than no template: only ship what you are sure of, per platform.

## Playbooks (contract)

A playbook is a YAML list of steps run per device, in order — Nocturne's
own format, deliberately close to what an Ansible user expects to read:

```yaml
name: Pre-change health check
description: …
vars: { min_uplinks: 2 }
steps:
  - name: Collect state
    collect: [facts, interfaces, neighbors]
  - name: Save evidence
    commands: [show version, show ip route summary]
    register: evidence                       # outputs available to later expressions
  - name: Uplinks are up
    assert:
      that: "interfaces | selectattr('description', 'matching', 'uplink') | selectattr('oper_up') | list | length >= vars.min_uplinks"
      message: "{{ device.name }} has fewer than {{ vars.min_uplinks }} uplinks up"
  - name: Back up
    backup: true                             # calls config.backup for the device
  - name: NTP
    template: { id: ios_ntp, vars: { servers: [192.0.2.1] } }   # collected into ONE change at the end of the run
    when: "device.profile == 'cisco_ios'"
```

Step kinds: `collect`, `commands` (read-only), `assert`, `backup`,
`template` (config lines are accumulated per device and become one change,
created at the end — the playbook's result links to it; nothing is applied),
`set` (computed vars), `pause {seconds}`. `when` on any step. A failed
`assert` or step stops that device (other devices continue) unless
`ignore_errors: true`.

| Method | Params → Result |
|---|---|
| `playbooks.list` / `.get` / `.save` / `.delete` | `{id?, name, yaml, updated_at}` |
| `playbooks.validate` | `{yaml}` → `{ok, problems: [{step?, message}]}` (unknown keys, bad expressions, unknown templates, non-read-only commands) |
| `playbooks.run` | `{playbook_id | yaml, device_ids?, filter?, vars?}` → `{job_id, run_id}` |
| `playbooks.runs` / `playbooks.run_get` | history; per device per step `{name, state: ok|failed|skipped, detail, outputs?}` + `change_id?` |

## Schedules (contract)

```rust
struct Schedule { id, name, enabled,
    when: When,            // {type: "interval", every_s} | {type: "daily", at: "02:30", days?: [mon..sun]} | {type: "cron", expr} (5-field, local time)
    action: Action,        // {method, params} from an allow-list
    last_run_at, last_job_id, last_ok, next_run_at, created_at }
```

Allow-listed methods (anything that changes a device is not on it, and
never will be): `config.backup`, `compliance.run`, `collect.run`,
`discovery.scan`, `runner.run`, `playbooks.run`, `reports.generate`,
`insights.scan`. A method that is not registered in this build makes the
schedule invalid at save time with a clear message.

| Method | Params → Result |
|---|---|
| `schedules.list` / `.save` / `.delete` | CRUD (`save` validates `when` and `action`, computes `next_run_at`) |
| `schedules.run_now` | `{id}` → the action's result |
| `schedules.preview` | `{when}` → next 5 run times |

The scheduler is one background task: sleeps until the next due schedule,
runs actions by `app.rpc`, records the outcome, never runs the same schedule
twice concurrently, catches up at most once after downtime (a backup missed
while the laptop was closed runs once at start, not seven times), and uses
the local time zone for `daily`/`cron`. Time must be injectable for tests.

Event: `schedules.changed`, `schedules.fired {id, job_id?}`.

## Ansible interoperability (contract)

Nocturne does not need Ansible, and Ansible has no native Windows control
node — so this is interop, not a dependency.

| Method | Params → Result |
|---|---|
| `ansible.status` | → `{installed, version?, path?, collections?: [..]}` (looks for `ansible-playbook` on PATH; local access only) |
| `ansible.export_playbook` | `{change_id}` or `{template_id, device_ids, vars}` → `{playbook: yaml, inventory: yaml}` — an Ansible playbook that pushes the same lines with the platform's config module (`cisco.ios.ios_config` with `lines`/`parents`, `community.routeros.command`, `arista.eos.eos_config`, `junipernetworks.junos.junos_config`, … only for platforms you can map correctly; others return `unsupported` naming the platform) plus the matching inventory (the core's `devices.export` in `ansible` format). No passwords in either. |
| `ansible.run` | `{playbook_path, device_ids?, limit?, check?: true, extra_args?: [..]}` → `{job_id}` — runs `ansible-playbook` with a generated temporary inventory, streaming its output as job log lines; credentials are passed **only through the environment** (`ansible_password: "{{ lookup('env', 'NOCTURNE_PW_<n>') }}"`), temp files are `0600` and removed; `--check` by default. Local access only. |

## Tests

Runner: read-only gate (whole run refused, nothing sent), per-device
failures isolated, compare grouping with volatile-line trimming, parse with
and without a template, output cap, retention. Templates: strict undefined,
each variable kind's validation, render context from collected data and the
"collect first" error, every built-in renders to the expected lines,
`templates.apply` → the right `changes.create` call (stub it). Playbooks:
validation errors with step numbers, each step kind, `when`, `register`,
assert failure isolates the device, accumulated template lines become one
change per run. Schedules: every `When` form's next-run computation incl.
DST and month ends, allow-list, catch-up-once, no overlap, disabled, injected
clock. Ansible: exported playbook and inventory parse as YAML and contain no
secret; `ansible.run` against a fake `ansible-playbook` script on PATH (proves
env-only credentials, temp-file cleanup, output streaming, cancel kills the
process); the real binary if installed, else skipped.
