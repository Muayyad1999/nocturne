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

A template can refuse what it was given: `{{ fail("…") }}` stops the
rendering. The message, as written, is that device's `error` in
`templates.render`, and `templates.apply` fails with `invalid` and the same
message after the device's name; no change is created. "As written" has two
limits, because a template usually quotes the value it refuses and a value is
whatever the caller sent: control characters (a line break, an escape
sequence) become spaces, and a message longer than 400 characters is cut
there and ends in `…`. It is for what a
variable's kind cannot say: two values that contradict each other, shares
that add up to too much, a guarantee above its limit. Such a check lives in
the body, so a copy of the template keeps it. The functions of the language
(`fail`, `range`, `namespace`, …) are not variables: `templates.validate`
does not list them in `used_variables` or `undeclared`, and a body that
calls them can be saved.

| Method | Params → Result |
|---|---|
| `templates.list` / `.get` / `.save` / `.delete` / `.duplicate` | CRUD; built-ins read-only |
| `templates.validate` | `{body, variables}` → `{ok, problems: [{line, message}], used_variables: [..], undeclared: [..]}` |
| `templates.render` | `{template_id | body, device_ids, vars}` → `[{device_id, name, ok, lines?: [..], error?}]` — pure preview |
| `templates.apply` | `{template_id, device_ids, vars, title?}` → `{change_id}` (kind `config`: one change with a `Lines` intent per device via `changes.create`, origin `template`) or `{job_id, run_id}` (kind `commands`: a runner run) |

Variable validation by kind (`vlan` 1–4094, `ip`/`cidr` parse — a prefix
length is digits, not `+24` —, `interface` non-empty and on one line — a
line break in a value is a second configuration line…) before rendering,
with the label in the message. `string` is free text and may span lines (a
banner does), so a template that puts a string into a command checks it
itself, with `fail`.

Built-in templates (embedded data, each with a render test against the fake
device's facts): for IOS — NTP servers, syslog hosts, SNMPv3 user/group,
login banner, add VLAN, access port (VLAN, description, portfast, optional
voice VLAN), trunk port (allowed VLANs **with `add`**), port description
from LLDP/CDP neighbour (`neighbors` loop), local user, management ACL on
VTY, DNS servers, disable unused port. For RouterOS — NTP client, remote
syslog, add VLAN on bridge, address + DHCP server for a new LAN, user,
restrict services to a management subnet. Commands a real device rejects are
worse than no template: only ship what you are sure of, per platform.

### Built-in QoS templates

Five built-ins carry the tag `qos` (contract: `docs/dev/spec-qos.md`,
"Changing QoS"). Like every `config` template they render lines and send
nothing; `templates.apply` hands the lines to `changes.create`, where an
uplink or the management interface is a protected resource. Every line
follows a page of the vendor's documentation, and each template's
`description` names the pages (read on 2026-11-22, and read again by the
reviewer the same day). None has met a real device: the three IOS templates
are typed into the simulator in the tests (below), the RouterOS ones rest on
the documentation alone.

**A line is rendered the way the device shows it afterwards.** The change
pipeline compares the device with what it predicted from the lines, and a
difference rolls the change back. So an interface is written with its long
name, and a DSCP by the name IOS has for it, a class's markings in ascending
order: `voice_dscp: "EF, 44"` renders `match dscp 44 ef`, `dscp: 46` renders
`set dscp ef` in a policy named `MARK-DSCP-EF`. That spelling is what the
simulator writes back (a test compares the two); it has not been compared
with a real IOS device, whose `show running-config` may differ in spacing.

| Id | For | Variables (kind, default) | Renders |
|---|---|---|---|
| `ios_qos_wan_edge` | `cisco_ios` | `interface` (interface, required), `policy_name` (string, `WAN-EDGE`), `voice_dscp` (list, `ef`), `voice_percent` (int, 10), `video_dscp` (list, `af41 af42 af43`), `video_percent` (int, 20), `critical_dscp` (list, `af21 af22 af23 cs3`), `critical_percent` (int, 25), `shape_kbps` (int, 0 = no shaper) | three `class-map match-any <policy>-VOICE/-VIDEO/-CRITICAL` with `match dscp` (names where IOS has them, ascending); `policy-map <policy>` with `priority percent` for voice and `bandwidth percent` for video and critical data (best effort is `class-default`, implicit); `service-policy output` on the interface. With `shape_kbps`: a parent `policy-map <policy>-SHAPE` whose `class-default` has `shape average <bps>` and `service-policy <policy>`, and the parent is what is attached |
| `ios_qos_shaped_uplink` | `cisco_ios` | `interface` (interface, required), `rate_kbps` (int, required), `policy_name` (string, empty = `SHAPE-<rate>K`) | `policy-map` with `shape average <bps>` in `class-default`; `service-policy output` |
| `ios_qos_access_port_dscp` | `cisco_ios`: Catalyst 2960, 2960-S and 3750 (the `mls qos` command set, IOS 12.2(55)SE) | `interface` (interface, required), `mode` (choice `trust`\|`set`, `trust`), `dscp` (string, mode `set`), `policy_name` (string, empty = `MARK-DSCP-<DSCP>`, the DSCP by its name: `MARK-DSCP-EF` for 46), `enable_qos` (bool, false) | `trust`: `mls qos trust dscp` on the port. `set`: `policy-map` whose `class-default` has `set dscp <name or number>`, and `service-policy input` on the port (not the 2960-S: no `class-default` there). `mls qos` globally **only** with `enable_qos`: it makes every untrusted port rewrite DSCP to 0 |
| `routeros_qos_simple_queue` | `mikrotik_routeros` | `queue_name` (string, required), `target` (cidr, required; IPv4), `upload_kbps`, `download_kbps` (int, required), `guaranteed_upload_kbps`, `guaranteed_download_kbps` (int, 0 = none) | one `/queue simple add name= target= max-limit=<up>k/<down>k [limit-at=<up>k/<down>k]` |
| `routeros_qos_queue_tree` | `mikrotik_routeros` | `interface` (interface, required), `rate_kbps` (int, required), `name_prefix` (string, `wan`), `voice_dscp` (list, `46`), `voice_percent` (int, 10), `video_dscp` (list, `34 36 38`), `video_percent` (int, 20), `critical_dscp` (list, `18 20 22 24`), `critical_percent` (int, 25) | `/ip firewall mangle add chain=postrouting out-interface=…`: first a rule that marks everything `<prefix>-best-effort` (`passthrough=yes`), then one `dscp=<n>` rule per code point that re-marks its class (`passthrough=no`); `/queue tree add`: a parent on the interface with `max-limit=<rate>k`, and four leaves by `packet-mark` with `limit-at` = the class's share, `max-limit` = the rate and `priority` 1 (voice), 3, 5, 8 (best effort, which is guaranteed the remainder, so the guarantees add up to exactly the rate) |

What they refuse, with `fail` (the messages name the variable by its label):

* a share outside 1–100, or three shares above 99 % (Cisco, "Low Latency
  Queueing with Priority Percentage Support": the percentages of `priority
  percent` and `bandwidth percent` "cannot exceed 99 percent of the total
  bandwidth available on the interface"; MikroTik: the children's `limit-at`
  may not exceed the parent's `max-limit`, and best effort needs a share).
  Not refused, but said in the description and the variable's help: IOS
  before 12.4(20)T reserves 75 % of an interface at most unless
  `max-reserved-bandwidth` raises it (command reference), and there the
  device itself refuses to attach a policy that asks for more;
* a DSCP that is not one — IOS: 0–63, `ef`, `af11`…`af43`, `cs1`…`cs7`,
  `default`, eight per class at most (one `match dscp` line); RouterOS:
  numbers only, because its `dscp` matcher takes no names; a class with no
  DSCP at all (only a copy without the defaults can get there);
* a DSCP listed twice, in one class or across them (only the first would
  ever match) — by code point, not by spelling: `46` and `ef`, `0` and
  `default`, `34` and `AF41` are one marking;
* a name that is not one word of letters, digits and a few signs (anything
  else would end the name and start another parameter, another line or, on
  RouterOS, a command in brackets), or too long: 40 characters for an IOS
  name, counting the suffix; 64 for a queue; 32 for the queue tree's prefix;
* a queue-tree prefix equal to the interface's name or to `global`: the
  parent queue is named after the prefix and the classes attach to it by
  that name;
* an interface that is not one interface: for IOS, after `ifname`, one word
  that starts with a letter and ends in its number (`GigabitEthernet0/1
  shutdown`, `range Gi0/1 - 24` and anything with a separator in it are
  refused); for RouterOS one word of 64 characters at most;
* a rate out of range: IOS shaping below 8 kbit/s, RouterOS limits below
  1 kbit/s, anything above 128 Gbit/s (a sanity range: the device has the
  last word on what its hardware shapes, and nothing a keyboard slip or an
  overflow produces becomes a line). Cisco's command reference gives `shape
  average` 1 000 to 1 000 000 000 bit/s on IOS 15.2(1)T, and 8 000 at least
  for `shape` on 15.1(1)T; hardware with faster ports takes more, so the
  documented 1 Gbit/s is said in the variable's help and not enforced;
* an IPv6 subscriber address for the simple queue: the pages read show IPv4
  targets only;
* a share of a shaped circuit that comes to less than a router takes for a
  class: 1 kbit/s for `priority`, 8 kbit/s for `bandwidth` (the command
  reference: 8 is the least on IOS 15.1(1)T, later releases take 1; a
  `bandwidth percent` below the range makes the router refuse to attach the
  policy). Without a shaper the interface's bandwidth is not known to the
  template and the router has the last word;
* a guaranteed rate above its limit, a negative one, or only one of the two;
* an uplink rate too small to give every class 1 kbit/s;
* `dscp` or `policy_name` given to the access-port template in mode `trust`
  (it would be silently unused).

Choices that are deliberate: voice is `priority percent`, never a bare
`priority` (Cisco allows no `bandwidth` class beside a strict-priority one);
each class is left with `exit` before the next, as Cisco's example types it,
because the change pipeline's predictor (`nocturne_drivers::hier`) reads a
`class` typed inside a `class` as nested in it; rates are whole kbit/s so
that one number means the same in every template.

Not built: bursts on the simple queue and IPv6 subscribers, the download
direction of the queue
tree (it needs a second tree on the inside interface), WRED, `fair-queue`,
policing, NBAR, and any Catalyst family other than the three named.

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
`templates.apply` → the right `changes.create` call (stub it), `fail` (the
message as written, no change created) and the language's functions in
`templates.validate`; a refusal is one line of bounded length. QoS templates
(`tests/qos_templates.rs`): each renders with its defaults to the documented
lines; each refusal above; a table of hostile values (a name with a line
break, a space, a separator or RouterOS brackets; an interface followed by a
command; a negative, overflowing or fractional rate; shares that add up to
too much; a megabyte of text) each of which must be refused, with a short
message free of control characters; the same marking written two ways; the
three IOS templates are typed into a simulated router (`nocturne-netsim`, a
dev-dependency) which must take every line and show the policy attached in
`show policy-map interface` — except `mls qos` and `mls qos trust dscp`,
which the simulator does not model and refuses; **the configuration the
simulated router ends up with is exactly what `nocturne_drivers::hier`
predicts from the lines**, compared as the change pipeline compares them
(every line with the section it is in), for defaults and for values typed as
numbers, in any order and any case; the WAN policy is predicted as a nested
policy; `templates.apply` makes one change and sends nothing, and in a build
without the change pipeline it fails with `unavailable` while
`templates.render` still previews. Playbooks:
validation errors with step numbers, each step kind, `when`, `register`,
assert failure isolates the device, accumulated template lines become one
change per run. Schedules: every `When` form's next-run computation incl.
DST and month ends, allow-list, catch-up-once, no overlap, disabled, injected
clock. Ansible: exported playbook and inventory parse as YAML and contain no
secret; `ansible.run` against a fake `ansible-playbook` script on PATH (proves
env-only credentials, temp-file cleanup, output streaming, cancel kills the
process); the real binary if installed, else skipped.
