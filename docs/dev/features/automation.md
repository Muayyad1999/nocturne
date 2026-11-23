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

A run whose process ended before it did (a crash, a kill, a power cut) is
closed at the next start, before the scheduler and the triggers start:
`finished_at` is set, `cancelled` is `true`, and every device its job had
selected gets a `skipped` result whose `reason` is "Nocturne was closed while
this run was going, so what was sent to this device and what it answered were
not kept". The task it came from, if this was the task's last run and no
outcome was recorded, gets `last_ok: false`. (An Ansible run is stored only
when it ends, so a cut-short one leaves no run record; its job says what
happened.)

## Templates (contract)

MiniJinja text that renders to configuration lines (kind `config`) or to
read-only commands (kind `commands`).

```rust
struct Template { id, name, description, kind: config|commands, body, builtin: bool,
    profiles: Vec<String>,                       // device types it is written for; empty = any
    variables: Vec<Variable { name, label, kind: string|int|bool|ip|cidr|vlan|interface|list|choice, required, default, choices, help, multiline }>,
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
length is digits, not `+24` —, `interface` non-empty and on one line…)
before rendering, with the label in the message. Every value is **one line**,
because a line break in a value is a second configuration line and an escape
sequence moves somebody's cursor: a `string`, `choice` or `interface` value
with a control character in it is refused (`` `<label>` must be one line of
text: a line break in it would be a second line of configuration, and other
control characters have no place in one``; for an interface `` `<label>` must
be one interface name, on one line``), and so is a `list` with such an item,
however deep (`` `<label>` must be values of one line each: …``). The one
exception is a `string` variable declared `multiline: true` (the built-in
banner's text): it may hold line breaks (CR LF counts as one and is stored
as LF) and nothing else (`` `<label>` may span lines, but holds another control
character …``). `multiline` is `false` when left out. A value that is one line
can still be two things — an interface and a `shutdown`, an address and a
`disabled=yes` — so a template that puts a value into a command checks its
shape itself, with `fail`; the built-ins do (below).

`templates.apply` refuses devices the template is not written for, by the
device type the rendering sees: the device's own profile, or else the one
detection found (`render::profile_of`). A device with neither is refused too,
because "not known yet" is not "any": `` `<template>` is written for
<profiles>, and what these devices are is not known yet: <names>. Set each
one's device type, or let Nocturne reach it once so that it is detected``; a
device of another type: `` `<template>` is written for <profiles> and these
devices are not: <name> (<profile>), …``. A playbook's `template` step and
`ansible.export_playbook {template_id}` refuse the same devices with the same
words. `templates.render` previews whatever it is asked to.

**What a rendering may do.** One rendering for one device may write at most
**1 MiB** (1 048 576 bytes) and do at most **500 000 steps** of work, a step
being MiniJinja's fuel: one instruction, or 64 bytes of text the template makes
on the way, kept or not. One call — `templates.render`, `templates.apply`,
`ansible.export_playbook`, or one playbook run with all its devices and
template steps — renders at most **16 MiB** for all its devices together (the
most the server takes in one request). A rendering stops at the bound, keeps
nothing, and that device's `error` (in `templates.apply`, the call's
`invalid` error after the device's name; no change is created) is, word for
word:

* past 1 MiB: "the template renders more than 1 MiB for this one device, which
  no device's configuration comes near; nothing of it was kept. Look for a
  loop that runs too often, or a value that is far too long";
* past 500 000 steps: "the template did more than 500000 steps of work for this
  one device and was stopped; nothing of it was kept. Look for a loop that runs
  far too often";
* past 16 MiB for the call, for that device and every later one that does not
  fit: "not rendered: with the devices before it, this would come to more than
  16 MiB, the most one call renders for all its devices together. Render fewer
  devices at a time".

The built-ins are far inside: on a 48-port switch with 48 interfaces and 48
neighbours collected, the largest writes 3 447 bytes (the port descriptions
from neighbours) and the most work is 3 935 steps (the WAN policy with its
DSCP checks); a test holds every built-in to a hundredth of the bytes and a
twentieth of the steps. Rendering runs on the blocking pool, never on a thread
that answers requests.

Built-in templates (embedded data, each with a render test against the fake
device's facts): for IOS — NTP servers, syslog hosts, SNMPv3 user/group,
login banner, add VLAN, access port (VLAN, description, portfast, optional
voice VLAN), trunk port (allowed VLANs **with `add`**), port description
from LLDP/CDP neighbour (`neighbors` loop), local user, management ACL on
VTY, DNS servers, disable unused port. For RouterOS — NTP client, remote
syslog, add VLAN on bridge, address + DHCP server for a new LAN, user,
restrict services to a management subnet. Commands a real device rejects are
worse than no template: only ship what you are sure of, per platform.

Every value a built-in puts into a command is checked in its body, with
`fail`, so that a copy keeps the checks:

* **an IOS interface** goes through `ios_interface(value, label, example)`
  (the `IOS_INTERFACE` prelude, shared with the QoS templates): after `ifname`
  the name must start with an interface type written out in full —
  `GigabitEthernet`, `FastEthernet`, `TenGigabitEthernet`,
  `TwoGigabitEthernet`, `FiveGigabitEthernet`, `TwentyFiveGigE`,
  `FortyGigabitEthernet`, `HundredGigE`, `AppGigabitEthernet`, `Ethernet`,
  `Port-channel`, `Vlan`, `Loopback`, `Tunnel`, `Serial`, `Dialer`,
  `Multilink`, `Virtual-Template`, `Cellular`, `ATM`, `Dot11Radio`, `BDI`,
  `BVI` — in any case (it is written the router's way: `dialer1` renders
  `Dialer1`), followed by its number (digits, `/`, `.`, `:`), 64 characters in
  all. IOS takes any unambiguous start of a type and shows the full name
  afterwards, and `ifname` expands only its own short forms (`Gi`, `Te`, `Po`,
  …): `Gigabit0/2`, `TenGig1/1/1`, `Fast0/1` rendered as typed would be
  predicted as a section the device never has, and the rollback of that
  change resets the real port with `default interface`. So they are refused:
  `` <label> must be one interface, by the full name the router gives it in its
  configuration, such as GigabitEthernet0/1: `Gigabit0/2` is not one``. Used by
  `ios_ntp` (`source`), `ios_access_port`, `ios_trunk_port`,
  `ios_disable_port` and the three IOS QoS templates;
* **a value that is one word in its command** goes through `word(…)`, a list
  of them through `words(…)` (the `WORDS` prelude): IOS — a VLAN name (32
  characters of letters, digits and `_ . : / + @ # -`), an ACL name (a letter,
  then letters, digits, `_`, `-`; 64), a domain name, NTP servers and syslog
  hosts (an address or a host name each), name servers (an address each),
  trunk VLANs (an id or a range each), management prefixes (IPv4 CIDR each);
  RouterOS — interface and VLAN interface names (64 characters of letters,
  digits and `_ . @ : / + -`), the logging action name, the topics (names
  separated by commas, `!` allowed), the lease time, NTP servers, DNS servers
  and service prefixes. A space, `;`, `[`, `]`, `$`, `=` or a quote in any of
  them is refused, so a value cannot bring a parameter, a second command or a
  command in brackets with it;
* **the banner** is the one `multiline` value, and it may not hold `^`, which
  ends the banner: what followed would be read as configuration.

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
| `routeros_qos_simple_queue` | `mikrotik_routeros` | `queue_name` (string, required), `target` (cidr, required; IPv4), `upload_kbps`, `download_kbps` (int, required), `guaranteed_upload_kbps`, `guaranteed_download_kbps` (int, 0 = none), `place_before` (string, empty = at the end) | one `/queue simple add name= target= max-limit=<up>k/<down>k [limit-at=<up>k/<down>k] [place-before=<queue>]` |
| `routeros_qos_queue_tree` | `mikrotik_routeros` | `interface` (interface, required), `rate_kbps` (int, required), `name_prefix` (string, `wan`), `voice_dscp` (list, `46`), `voice_percent` (int, 10), `video_dscp` (list, `34 36 38`), `video_percent` (int, 20), `critical_dscp` (list, `18 20 22 24`), `critical_percent` (int, 25) | first `/queue tree add`: a parent on the interface with `max-limit=<rate>k`, and four leaves by `packet-mark` with `limit-at` = the class's share, `max-limit` = the rate and `priority` 1 (voice), 3, 5, 8 (best effort, which is guaranteed the remainder, so the guarantees add up to exactly the rate); then `/ip firewall mangle add chain=postrouting out-interface=…`, each rule ending in `comment=<prefix>-qos`: a rule that marks everything `<prefix>-best-effort` (`passthrough=yes`), then one `dscp=<n>` rule per code point that re-marks its class (`passthrough=no`) |

**Running a RouterOS template twice.** A queue's name is unique on the
router (MikroTik, "Queues": "Unique queue identifier"), and a mangle rule has
no name: it is added again every time. So the queue tree adds its queues
first, and a second run is refused at its first line, before a rule is added
twice; the comment is how the rules one run added can be found and removed by
hand (`/ip firewall mangle remove [find comment=wan-qos]`). The simple queue
only adds: run again for a subscriber who already has a queue, the router
refuses it, and changing an existing queue's limits is not built. Simple
queues are tried in order and a packet stops at the first that matches it
("Queues": "Simple queues have a strict order"), and a new queue goes to the
end of the list: a queue above it whose target covers the subscriber (a
subnet, a catch-all) takes the traffic, and the new limits never apply. Its
description says so, and `place_before` names that queue (one word, 64
characters, not the queue being added); it is a name, not a number, because a
number exists only after a `print` in the same session (MikroTik, "Command
Line Interface": `place-before` "places a new item before an existing item",
and items with names may be named instead of numbered). Comparing the
router's configuration as a multiset after a rollback, so that a duplicate
rule counts, is the configuration engine's (`nocturne_drivers::hier`), not a
template's.

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
* an interface that is not one interface: for IOS, after `ifname`, a type
  written out in full and its number (`ios_interface`, above:
  `GigabitEthernet0/1 shutdown`, `range Gi0/1 - 24`, anything with a separator
  in it, and an abbreviation `ifname` does not expand such as `Gigabit0/2` are
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

Not built: bursts on the simple queue and IPv6 subscribers, changing an
existing subscriber's queue, a parent for the subscriber queue, the download
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
`insights.scan`, `tasks.run`. A method that is not registered in this build
makes the schedule invalid at save time with a clear message.

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

**A run in flight.** Before the action is called the stored schedule is
moved on to its next slot and records the run as started (`last_run_at` set,
`last_ok`, `last_error` and `last_job_id` empty: no outcome yet). A process
that dies during the action (out of memory, a power cut) therefore comes back
to a schedule that is not due, and does not run the same thing again on every
start. A firing by the scheduler first checks, on the stored record, that the
schedule is still on and still due (one switched off since the tick read it
does not run); `schedules.run_now` runs it whatever its clock says. When the
run ends its outcome is written onto the schedule **as it is stored then**,
and `next_run_at` worked out from that record's `when` and `enabled`: a schedule deleted while it ran stays deleted, and one edited or
switched off keeps the edit (off: `next_run_at` is `null`). Saving and
deleting take the same lock as that read and write, so neither can land
between them. At start, a schedule whose run was started and never got an
outcome (the process ended) gets `last_ok: false` and `last_error` "Nocturne
was closed while this was running".

Event: `schedules.changed` (when a run starts and when it ends, too),
`schedules.fired {id, job_id?}`.

## Triggers (contract)

```rust
struct Trigger { id, name, enabled,
    topic,                 // an event topic exactly (`alerts.raised`), or a prefix (`alerts.`, `alerts.*`)
    match_payload,         // {"/pointer": value, …}: every one must hold
    debounce_s,            // collect for this long, then fire once (0 = the next sweep; at most 3600)
    action: ActionCall,    // the schedules' allow-list
    pass_device,           // hand the device the events were about to the action as `device_ids`
    last_fired_at, last_job_id, fired_count, created_at }
```

| Method | Params → Result |
|---|---|
| `triggers.list` / `.get` / `.save` / `.delete` | CRUD (`save` validates the topic, the filter and the action) |
| `triggers.fire_now` | `{id}` → the action's result, with no device from an event |

A trigger may not watch what its own run publishes: the topics of this crate
(`automation.`, `schedules.`, `triggers.`, `tasks.`, `templates.`,
`playbooks.`) and **job events** (`jobs.`), because every action a trigger may
call starts a job and the core announces each job on `jobs.changed` as it
starts, moves and ends. A pattern that would cover one of those — `*`, `t*`,
`job*`, `jobs.` — is refused as surely as the topic itself, with `invalid` and
"… a trigger on it would run in a loop". A trigger stored on one of them
anyway (by an older build, or by hand) is never set off by them: those events
are not looked at. Not caught by this: a trigger whose action publishes the
topic it watches through some other route (`devices.state` running
`collect.run`, which updates the device's state) is set off again by each of
its own runs, and runs back to back until it is switched off. The bus carries
no cause, so nothing here can tell those events from anyone else's.

`pass_device` takes the device from `/device_id` or `/device/id` in the
payload, and from `/id` only for a `devices.*` topic: an alert or a change has
an id of its own, which names no device.

A burst is collected for `debounce_s` and fires once; if the trigger's
previous run is still going it is held for another window (at least 5 s)
rather than lost. The open window is kept on disk (`automation.trigger_pending`)
as it changes, so a restart inside it still fires it: when the window closes,
or at the first sweep after the start if it closed while nothing was running
— unless it closed longer ago than `catch_up_within_s`, when it is let go and
the log says so. A window is let go too when triggers are switched off
(`triggers_enabled: false`). When it fires it is removed from disk and the
firing counted (`fired_count`, `last_fired_at`) on the stored trigger before
the action is called, so a process that dies in the action does not fire the
same burst again; `last_job_id` is written onto the trigger as stored when the
action returns — deleted meanwhile, it stays deleted; edited, it keeps the
edit.

Events: `triggers.changed`, `triggers.fired {id, topic, events, job_id?,
device_ids}`.

## Tasks (contract)

A task is a saved selector and an action (`commands`, `playbook`, `template`,
`ansible`): `tasks.list` / `.get` / `.save` / `.delete`, `tasks.run {id,
dry_run?, device_ids?, origin?}` → the action's result, `tasks.devices`
→ what the selector means now. `tasks.run` records the run (`last_run_at`,
`last_run_id`, `last_ok: null`) on the task **as it is stored when the action
returns** — a task deleted while a template or an Ansible run took its time
stays deleted, one edited keeps the edit — and `last_ok` when its job ends,
only while that run is still the task's last one.

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
`templates.render` still previews; the queue tree adds its queues before any
rule and labels every rule; an abbreviation IOS would expand is refused in
every IOS template that takes an interface; the subscriber queue goes before a
named queue. Bounds (`tests/templates.rs`, `tests/render_bounds.rs`): 5 MB for
one device is refused at 1 MiB and creates no change, twenty devices of
900 kB stop at the nineteenth, a loop that writes nothing is stopped by its
steps, and every built-in on a 48-port switch takes less than a hundredth of
the bytes and a twentieth of the steps. Values: a line break in a one-line
value (a string, a list item) is refused, the banner keeps its lines but not
an escape sequence or a `^`, and a value with a second word, a `;` or a
bracket in it is turned away by every older built-in. Device types: a device
with no type yet is refused, a detected one is followed, a playbook's
template step is refused the same way. Playbooks:
validation errors with step numbers, each step kind, `when`, `register`,
assert failure isolates the device, accumulated template lines become one
change per run. Schedules: every `When` form's next-run computation incl.
DST and month ends, allow-list, catch-up-once, no overlap, disabled, injected
clock; the slot is spent while the action is still going; a copy of a live
data directory taken while an action is held (what a dying process leaves)
does not run it again when opened, and says why; deleted or switched off
during a run, it stays so. Triggers: job topics and patterns that cover
automation's own refused, a stored one never set off; an alert's own id is
not a device; deleted or edited during the action, it stays so and the
firing counts; a burst open when the process ended fires after the restart;
switched off, an open burst is let go. Tasks: deleted or edited during a run,
it stays so; a run open when the process ended is closed at the next start
with every device skipped and why, and its task's `last_ok` is `false`.
Ansible: exported playbook and inventory parse as YAML and contain no
secret; `ansible.run` against a fake `ansible-playbook` script on PATH (proves
env-only credentials, temp-file cleanup, output streaming, cancel kills the
process); the real binary if installed, else skipped.
