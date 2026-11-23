# Device profiles: teaching Nocturne a new kind of device

Nocturne talks to a device through a **profile**: one YAML file that says what
the prompt looks like, how to turn paging off, which commands are safe, and
how to turn what the device prints into the data the rest of Nocturne works
with. Adding a device type means writing one of these files. No code, no
rebuild, no restart of anything but the session you are in.

This guide is for the engineer who knows the device, not for a Rust
developer. If you can read a `show` command's output and write a regular
expression, you can write a profile.

Contents:

1. [What you get out of a profile](#what-you-get-out-of-a-profile)
2. [Where profiles live](#where-profiles-live)
3. [A profile, built up in steps](#a-profile-built-up-in-steps)
4. [The file, field by field](#the-file-field-by-field)
5. [Parsers](#parsers)
6. [Expressions](#expressions)
7. [`extends`: sharing what is the same](#extends-sharing-what-is-the-same)
8. [Testing a profile](#testing-a-profile)
9. [Maturity: how far to trust a profile](#maturity-how-far-to-trust-a-profile)
10. [Checklist for a new profile](#checklist-for-a-new-profile)

---

## What you get out of a profile

| What the profile says | What Nocturne can then do |
|---|---|
| `detect` | Recognise the device from SNMP, its SSH banner or one probe command |
| `cli` | Log in, keep its place in the output, drive the pager, notice a refused command |
| `config` | Fetch the configuration, diff two fetches, apply lines, save, arm a dead-man switch |
| `policy` | Tell a read from a change before anything is sent — the AI and the automation never get past this |
| `collect` | Fill the common model: interfaces, neighbours, VLANs, MAC table, ARP, routes, health, inventory, per-class QoS statistics |
| `snmp` | Read CPU, memory and temperature without a login |

Everything is optional. A profile with nothing but a prompt and a read-only
list already gives you a usable terminal with a safety net; you can add the
collectors later.

Two rules run through the whole format:

* **Nothing is invented.** A command that fails is reported as failed. A field
  the device does not print is left empty, never filled with a guess.
* **Default deny.** A command is a change unless `policy.read_only` recognises
  it. Collectors may only run commands that come out read-only, and a profile
  that breaks that rule is rejected when it is loaded, not when it is run.

## Where profiles live

* The **built-in pack** ships inside Nocturne: `generic`, `cisco_ios`,
  `mikrotik_routeros`, `linux` and the abstract base `_cli_ios_like`.
* **Your profiles** are kept by Nocturne itself, not in a directory you have
  to find: paste the YAML into the profile editor (or hand it to the
  assistant and let it draft one). Nocturne checks it, tells you about every
  problem at once, and only then saves it.
* A profile of yours with the **same id as a built-in replaces it**. Delete
  yours and the built-in comes back. That is the supported way to fix a
  built-in for your network without waiting for a release.
* Ids use lower-case letters, digits and underscores (`arista_eos`). An id
  starting with `_` is an abstract base: it cannot be chosen for a device, it
  only exists to be extended.

## A profile, built up in steps

We will write a profile for **Arista EOS** from nothing. Each step below is a
complete, loadable profile — paste any of them into Nocturne and it will
work, as far as it goes.

### Step 1 — the smallest thing that works

A prompt, and the commands you promise are read-only.

```yaml
# profile
id: arista_eos
name: Arista EOS
vendor: Arista
os: EOS
description: Arista EOS switches.
cli:
  # `sw1>` and `sw1#`, and nothing else on the line.
  prompt: '^(?P<host>[A-Za-z0-9][\w.\-]{0,62})[>#]\s?$'
policy:
  read_only:
    - '^show\s+\S'
    - '^ping\s+\S'
```

That is enough to open a terminal, follow the prompt and stop the AI from
sending anything that is not a `show`. The `(?P<host>…)` group matters: it is
how the driver tells *your* device's prompt from a line of output that happens
to end in `#`.

### Step 2 — the modes the CLI has

EOS, like IOS, has an unprivileged mode, a privileged mode and a
configuration mode. Say how each prompt looks and how to get in.

```yaml
# profile
id: arista_eos
name: Arista EOS
vendor: Arista
os: EOS
description: Arista EOS switches.
cli:
  # The hostname, then an optional (config…) part: `sw1(config-if-Et1)#`.
  prompt: '^(?P<host>[A-Za-z0-9][\w.\-]{0,62})(?:\([\w.\-:+/]{1,40}\))?[>#]\s?$'
  user_prompt: '>\s?$'
  config_prompt: '\([^()\s]+\)#\s?$'
  enable:
    command: enable
    password_prompt: '(?i)password:\s*$'
  login:
    username_prompt: '(?i)(?:user ?name|login):\s*$'
    password_prompt: '(?i)password:\s*$'
    failure: '(?i)(?:login incorrect|authentication failed)'
config:
  syntax: ios
  enter: [configure terminal]
  exit: [end]
  save: [write memory]
  running: show running-config
policy:
  read_only:
    - '^show\s+\S'
    - '^ping\s+\S'
```

Try the prompt patterns against prompts you have really seen, including the
awkward ones: an FQDN hostname with dots, a hostname with a dash, a
`(config-if-range)#`, the standby supervisor's `-stby` suffix. Then try them
against ordinary output — a profile whose prompt pattern matches a line of a
`show running-config` will cut every command short.

### Step 3 — paging, and the pager when paging will not go away

```yaml
# profile
id: arista_eos
name: Arista EOS
vendor: Arista
os: EOS
description: Arista EOS switches.
cli:
  prompt: '^(?P<host>[A-Za-z0-9][\w.\-]{0,62})(?:\([\w.\-:+/]{1,40}\))?[>#]\s?$'
  # Sent once, right after login.
  init:
    - terminal length 0
    - terminal width 32767
  # Met only when `terminal length 0` was refused — by an authorisation
  # server, or on a console line. Space asks for the next page.
  pager:
    pattern: '^\s*--\s?More\s?--'
    key: ' '
policy:
  read_only:
    - '^show\s+\S'
    - '^terminal (length|width)\b'
```

`init` commands run once per session. Keep them to paging and terminal width:
they run before anything is classified, so they must be commands you would be
happy for the AI to run on every login.

### Step 4 — how the device says "no"

Nocturne has to know a refused command from a successful one, or it will
happily report an empty interface list as the truth.

```yaml
# profile
id: arista_eos
name: Arista EOS
vendor: Arista
os: EOS
description: Arista EOS switches.
cli:
  prompt: '^(?P<host>[A-Za-z0-9][\w.\-]{0,62})(?:\([\w.\-:+/]{1,40}\))?[>#]\s?$'
  errors:
    - '^% Invalid input'
    - '^% Incomplete command'
    - '^% Ambiguous command'
    - '^% Authorization denied'
    - '^% (?:Interface|VLAN) does not exist'
policy:
  read_only: ['^show\s+\S']
```

Test these from both sides: output that **must** be caught, and output that
must **not**. `% Access VLAN does not exist. Creating vlan 30` is not a
failure, it is the device telling you what it just did.

### Step 5 — facts

Now the data. Each kind lists steps; each step runs one command, parses it
and maps the parsed fields onto the model.

```yaml
# profile
id: arista_eos
name: Arista EOS
vendor: Arista
os: EOS
description: Arista EOS switches.
ntc_platform: arista_eos
cli:
  prompt: '^(?P<host>[A-Za-z0-9][\w.\-]{0,62})(?:\([\w.\-:+/]{1,40}\))?[>#]\s?$'
policy:
  read_only: ['^show\s+\S']
collect:
  facts:
    - command: show version
      # The bundled ntc-templates template for this platform and command.
      parser: {ntc: auto}
      map:
        model: MODEL
        serial: SERIAL_NUMBER
        os_version: IMAGE
        # "1 hour and 5 minutes" → 3900
        uptime_s: UPTIME | duration
        os: "'EOS'"
        vendor: "'Arista'"
        extra.system_mac: SYS_MAC | mac
    # The hostname is not in `show version` on this platform.
    - command: show hostname
      parser: {regex: '^Hostname:\s+(?P<hostname>\S+)'}
      map:
        hostname: hostname
```

`facts` is one row, so the steps are merged into it: the first step that
fills a field wins, and a step that could add nothing is skipped. A field
whose expression comes out empty is left out — not set to `""`.

### Step 6 — interfaces, and enriching them

```yaml
# profile
id: arista_eos
name: Arista EOS
vendor: Arista
os: EOS
description: Arista EOS switches.
ntc_platform: arista_eos
cli:
  prompt: '^(?P<host>[A-Za-z0-9][\w.\-]{0,62})(?:\([\w.\-:+/]{1,40}\))?[>#]\s?$'
policy:
  read_only: ['^show\s+\S']
collect:
  interfaces:
    - command: show interfaces
      parser: {ntc: auto}
      map:
        name: INTERFACE
        description: DESCRIPTION
        # "Ethernet3 is administratively down, line protocol is down"
        admin_up: "not LINK_STATUS is startingwith('administratively')"
        oper_up: "PROTOCOL_STATUS is startingwith('up')"
        mtu: MTU
        mac: MAC_ADDRESS | mac
        kind: HARDWARE_TYPE
        ipv4: "[IP_ADDRESS] if IP_ADDRESS else []"
  vlans:
    - command: show vlan
      parser: {ntc: auto}
      map:
        id: VLAN_ID
        name: VLAN_NAME
        status: STATUS
        interfaces: INTERFACES
```

A second step for a kind can either **add rows** or **enrich the rows the
first step made**. `merge_on: name` makes it an enrichment: each of its rows
is merged into the row whose `name` matches (interface names are compared
the way the device would, so `Et1` finds `Ethernet1`), fields that are
already filled are kept, and a row that matches nothing is dropped.
`optional: true` says the step may fail — an older release without the
command, a router without `show interfaces switchport` — without failing the
kind.

### Step 7 — the finished profile

The real thing inherits the IOS-style behaviour from `_cli_ios_like` instead
of repeating it, and adds detection so a new device identifies itself.

```yaml
# profile
id: arista_eos
name: Arista EOS
vendor: Arista
os: EOS
description: >-
  Arista EOS switches (7000 series, CCS, vEOS). Prompts, enable, configuration
  mode and the command policy come from the IOS-style base.
extends: _cli_ios_like
ntc_platform: arista_eos
maturity: experimental
detect:
  sysdescr: ['Arista Networks EOS']
  sysobjectid: ['1.3.6.1.4.1.30065.']
  probes:
    - command: show version
      matches: '(?m)^Arista\s+\S'
  priority: 55
cli:
  init:
    - terminal length 0
    - terminal width 32767
config:
  running: show running-config
  ignore:
    - '^! Command: show running-config'
    - '^! device: .* running '
    - '^! boot system flash:'
collect:
  facts:
    - command: show version
      parser: {ntc: auto}
      map:
        model: MODEL
        serial: SERIAL_NUMBER
        os_version: IMAGE
        uptime_s: UPTIME | duration
        os: "'EOS'"
        vendor: "'Arista'"
    - command: show hostname
      parser: {regex: '^Hostname:\s+(?P<hostname>\S+)'}
      map: {hostname: hostname}
  interfaces:
    - command: show interfaces
      parser: {ntc: auto}
      map:
        name: INTERFACE
        description: DESCRIPTION
        admin_up: "not LINK_STATUS is startingwith('administratively')"
        oper_up: "PROTOCOL_STATUS is startingwith('up')"
        mtu: MTU
        mac: MAC_ADDRESS | mac
        kind: HARDWARE_TYPE
        ipv4: "[IP_ADDRESS] if IP_ADDRESS else []"
  vlans:
    - command: show vlan
      parser: {ntc: auto}
      map:
        id: VLAN_ID
        name: VLAN_NAME
        status: STATUS
        interfaces: INTERFACES
snmp:
  cpu: [1.3.6.1.4.1.2021.11.11.0]
  memory_total: [1.3.6.1.4.1.2021.4.5.0]
  memory_free: [1.3.6.1.4.1.2021.4.11.0]
```

## The file, field by field

### Top level

| Key | Meaning |
|---|---|
| `id` | Unique key: lower-case letters, digits, `_`. A leading `_` means abstract. |
| `name`, `vendor`, `os`, `description` | What the UI shows. `os` is also the operating-system family in `facts`. |
| `extends` | Id of the profile to inherit from. |
| `ntc_platform` | Platform name in the ntc-templates index, for `parser: {ntc: auto}`. |
| `maturity` | `verified`, `community` or `experimental`. Never inherited. |
| `unverified` | Kinds this profile collects with a parser that has not yet met output recorded from a real device, e.g. `[qos]`. See [Maturity](#maturity-how-far-to-trust-a-profile). Each must be a kind the profile has `collect` steps for. |
| `interface_names` | `cisco` (default: `Gi1/0/1` is expanded to `GigabitEthernet1/0/1` everywhere, so data from different commands joins up) or `verbatim` (names are kept exactly as printed — right for RouterOS, Linux, Junos). |

### `detect`

| Key | Meaning |
|---|---|
| `banner` | Patterns for the SSH identification string, the pre-login banner and the login screen. |
| `sysdescr` | Patterns for SNMP `sysDescr`. |
| `sysobjectid` | `sysObjectID` prefixes, e.g. `1.3.6.1.4.1.30065.`. |
| `probes` | `command` + `matches`: a command to try on an unidentified device and the pattern its answer must match. Probe commands must be read-only under this profile's own policy. |
| `priority` | Higher wins when several profiles match. The built-ins use 60 for a vendor CLI, 20 for Linux, -100 for `generic`. |

Detection runs cheapest first: SNMP, then the banner, then — only if those
say nothing — a login with the `generic` profile and the probe commands, in
priority order, each distinct command once. A probe the device rejects is not
a failure: what a device says when it does not understand a command often
identifies it just as well. If nothing matches, the session stays on
`generic`, which is still a usable terminal.

### `cli`

| Key | Meaning |
|---|---|
| `prompt` | Matches a whole prompt line. A `(?P<host>…)` group pins it to this device's hostname. |
| `user_prompt`, `config_prompt` | Which prompts mean unprivileged and configuration mode. |
| `enable` | `command` and `password_prompt`, where the platform has a privileged mode. |
| `login` | `username_prompt`, `password_prompt`, `failure`, and `username_suffix` — appended to the user name when the driver authenticates (RouterOS's `+ct511w4098h`, which turns colour, terminal detection and paging off). |
| `init` | Commands sent once after login. |
| `pager` | `pattern` of the "more" prompt and the `key` that continues (` ` on most, `d` on RouterOS). |
| `newline` | What Enter is. `"\n"` by default, `"\r"` on RouterOS. |
| `exit` | How to log out politely. |
| `errors` | Output patterns that mean the command was refused. |
| `answers` | `prompt`/`answer` pairs for questions the device asks on its own (`[confirm]`, a licence question). |
| `unsolicited` | Lines the device prints by itself — console log messages. A prompt followed only by these still counts as the prompt. |
| `timeout_s` | Seconds of silence before a command is called hung. |

### `config`

| Key | Meaning |
|---|---|
| `syntax` | `ios`, `vrp`, `set`, `routeros` or `flat`. Decides how the configuration is parsed into blocks and how a difference becomes commands. |
| `enter`, `exit` | Commands that enter and leave configuration mode. Empty means the platform has none, and lines are applied at the prompt. |
| `commit`, `abort` | For platforms with a candidate configuration. |
| `save` | Commands that persist the running configuration. Empty means nothing needs saving — say so in a comment, so the next reader knows it is not an oversight. |
| `running`, `startup` | Commands that print each configuration. Leave `running` empty for a platform that has no such thing. |
| `ignore` | Patterns for lines to drop from a fetched configuration. **This is what makes two fetches of an unchanged device compare equal**: timestamps, byte counts, "Last configuration change at …", RouterOS's dated export header, log lines a console printed into the listing. |
| `errors` | Patterns for a rejected configuration line, when they differ from `cli.errors`. |
| `answers` | Questions asked while configuring. |
| `guard` | The platform's dead-man switch: `name`, `arm` (with `{minutes}`), `disarm`, `effect`. `reload in 5` on IOS, `commit confirmed` on Junos. If the platform has none that can be armed by sending a command, leave it out and say why — do not invent one. |
| `secrets` | Extra patterns with a `(?P<secret>…)` group, so passwords and keys are redacted before anything is shown, logged or sent to the AI. |

### `policy`

| Key | Meaning |
|---|---|
| `read_only` | Patterns for commands that only display things. Everything else is a change. |
| `dangerous` | Patterns for commands that can take the device, its configuration or its software away: `reload`, `erase`, `format`, `delete`, `write erase`, `/system reset-configuration`, a factory reset. |
| `shell` | `true` when the CLI is a POSIX shell: `|`, `&&`, `||` and `&` then chain separate commands, each classified on its own, and a line with a redirection or a command substitution is never read-only. |

Patterns are matched case-insensitively against the trimmed command. On a
network CLI, `|` is an output filter, so `show run | include vlan` stays
read-only — but `| redirect`, `| tee`, `| save` and the like write somewhere
and make it a change.

Write the read-only patterns tightly. `^/ip route print` is read-only;
`^/ip route print file=routes` writes a file and must not be. Because there
is no "not" in these patterns, the trick is to anchor the end and list the
arguments the read action really takes.

### `collect`

A map from kind to a list of steps. The kinds are `facts`, `interfaces`,
`neighbors`, `vlans`, `mac_table`, `arp`, `routes`, `health`, `inventory` and
`qos`; `facts` and `health` are one row, the rest are tables.

| Key of a step | Meaning |
|---|---|
| `command` | What to run. Must be read-only under this profile's `policy`. |
| `parser` | How to turn the output into rows — see below. |
| `map` | Model field → expression. An empty map passes the parsed row through; fields the model does not know are dropped. |
| `when` | Keep a row only when this expression is true. |
| `merge_on` | Merge this step's rows into earlier rows matched on this model field, instead of adding rows. |
| `optional` | A failure is recorded and skipped instead of failing the kind. |

A dotted key fills a nested field: `counters.in_errors`, `extra.image`.

What happens when things go wrong is fixed, and worth knowing:

* a kind with no steps is reported as **unsupported**, not as empty;
* a step that ran and found nothing gives an **empty table**, which is an
  answer: no neighbours, no policy attached. It is stored as such;
* if every step failed, the kind is **failed**, the errors are kept, and
  nothing is stored — what was read before stays, and is never replaced by
  an empty table;
* if one step failed but another produced rows, the kind is kept and a
  **partial** problem says what is missing;
* a row that does not fit the model is dropped and counted, not patched up;
* a number that does not fit in 64 bits — printed that large, or the sum of
  two that are — is left out and counted, never cut down to the largest
  number that does fit;
* the message of a problem quotes what the device printed, up to 2 000
  characters of it.

### The `qos` kind

One row per class, per direction, per interface — the per-class statistics of
whatever the platform calls a QoS policy.

| Field | Meaning |
|---|---|
| `interface` | The interface the policy is attached to. Leave it out when the platform attaches the policy to something else (a RouterOS queue on an address). |
| `direction` | `in` or `out`, seen from the device (`input`/`output` and `ingress`/`egress` are understood too). **Required**: a row that does not say which way is dropped and counted as a problem, not filed under one. |
| `policy` | The policy the class belongs to: an IOS policy-map; on RouterOS the menu the queue is in, `queue simple` or `queue tree`. |
| `class` | The class; on RouterOS the queue. `class-default` is a class like any other. |
| `parent` | The class, or queue, this one sits under, when policies nest. A parent's counters include its children's: add up one level, never the whole table. |
| `rate_bps` | What the class is held to or promised: a shaper's rate, a policer's CIR, a priority rate, a bandwidth guarantee, a queue's max-limit. |
| `packets`, `bytes` | Classified into the class. |
| `dropped_packets`, `dropped_bytes` | Every drop, whatever the reason. |
| `queue_dropped_packets` | Dropped from the queue — tail drop, WRED: **congestion**. |
| `police_dropped_packets` | Dropped by a policer: **the policy doing what it was told to**. |
| `queued_packets` | Queue depth at the moment of the read. |
| `offered_rate_bps`, `drop_rate_bps` | Rates as the device averages them itself, kept apart from anything Nocturne computes from the counters. |

Every number is optional. Map only what the platform prints: a counter that
is left out is "the device does not say", and a zero is a zero. That matters
most for the two drop fields — Nocturne alerts on congestion and deliberately
not on a policer, so fill them only where the output really tells the two
apart, and fill `dropped_packets` alone where it does not (RouterOS counts
one kind of drop).

A policy that nests is a tree, and most platforms show the tree by
indentation only. TextFSM cannot count columns, but it can match them:
`cisco_ios` reads the level of a class from the column its `Class-map:` line
starts in, keeps the name of the policy and of the class open at each level in
`Filldown` values, and stops with `-> Error "…"` at anything it cannot place.
Read its `collect.qos` step before writing one for another platform; an
error that fails the read is better than a counter under the wrong parent.

The same goes for an answer the template does not recognise at all. A
template that reads zero rows out of text it does not understand has said
"no policy attached", which nobody said. `cisco_ios` starts in a state that
knows only what may stand before the first interface (nothing, the two lines
of `exec prompt timestamp`, a log message) and stops at anything else, and it
insists on the order interface → attached policy → class, so that a class is
never filed under a name left over from the interface before it.

### `snmp`

Lists of OIDs for `cpu`, `memory_used`, `memory_free`, `memory_total` and
`temperature`, tried in order. Put the vendor's own OIDs first and the
standard ones after.

## Parsers

| Parser | Written as | Use it for |
|---|---|---|
| ntc-templates | `{ntc: auto}` or `{ntc: cisco_ios_show_vlan}` | Anything the bundled template library covers (~980 templates). `auto` resolves the template from `ntc_platform` plus the command. |
| TextFSM | `{textfsm: "…"}` | A table the library does not cover, or a field its template leaves out. |
| Regex | `{regex: '…'}` | One or two values out of free text. Every match is a row of its named groups. |
| Key/value | `kv` | RouterOS `print detail` / `print terse` listings. |
| Colon | `colon` | A `key: value` property sheet → one row. |
| JSON | `{json: 'path.to.*'}` | Platforms that can emit JSON. `*` means every element or every value. |
| Raw | `raw` | The whole output as one row with one field, `output`. |

**ntc** — value names are upper case (`HARDWARE`, `SERIAL`); you can use them
as written or lower-cased. A template that cannot be found is a profile error
at load time, not a surprise at 3 a.m.

```yaml
# profile
id: example_ntc
name: Example (ntc)
description: The bundled template library, resolved from the command.
ntc_platform: cisco_ios
policy: {read_only: ['^show\s+\S']}
collect:
  facts:
    - command: show version
      parser: {ntc: auto}
      map: {hostname: HOSTNAME, model: HARDWARE | first, uptime_s: UPTIME | duration}
```

**textfsm** — for the fields a bundled template does not capture. Write it
inline; `$$` is a literal `$`.

```yaml
# profile
id: example_textfsm
name: Example (TextFSM)
description: A template written in the profile for the counters IOS prints.
policy: {read_only: ['^show\s+\S']}
collect:
  interfaces:
    - command: show interfaces
      parser:
        textfsm: |
          Value Required INTERFACE (\S+)
          Value IN_OCTETS (\d+)

          Start
            ^\S+\s+is\s+.+?,\s+line\s+protocol.*$$ -> Continue.Record
            ^${INTERFACE}\s+is\s+.+?,\s+line\s+protocol
            ^\s+\d+\s+packets\s+input,\s+${IN_OCTETS}\s+bytes
      map:
        name: INTERFACE
        counters.in_octets: IN_OCTETS
```

**regex** — each match is a row; name every group you want.

```yaml
# profile
id: example_regex
name: Example (regex)
description: Two numbers out of a line of prose.
policy: {read_only: ['^show\s+\S']}
collect:
  health:
    - command: show processes cpu
      parser: {regex: 'CPU utilization for five seconds: (?P<five_seconds>\d+)%'}
      map: {cpu_pct: five_seconds}
```

**kv** — RouterOS listings. Rows get `index`, `flags` and `comment` on top of
the device's own properties, with `-` turned into `_`
(`mac-address` → `mac_address`). Wrapped lines, comments that run over
several lines, RouterOS 7's lower-case flags and items without an index are
all handled. A listing that has items but none in `key=value` form — a table,
which is what RouterOS prints where `detail` or `terse` is left out — fails
the step: it is not an empty listing, and is not read as one.

```yaml
# profile
id: example_kv
name: Example (key=value)
description: A RouterOS detail listing.
policy: {read_only: ['^/\S+ print( detail)?$']}
collect:
  interfaces:
    - command: /interface print detail
      parser: kv
      map:
        name: name
        description: comment
        admin_up: "'X' not in flags"
        oper_up: "'R' in flags"
        mtu: actual_mtu or mtu
```

**colon** — a property sheet becomes one row; keys become field names the
same way.

```yaml
# profile
id: example_colon
name: Example (property sheet)
description: RouterOS /system resource print.
policy: {read_only: ['^/system resource print$']}
collect:
  health:
    - command: /system resource print
      parser: colon
      map:
        cpu_pct: cpu_load | num
        memory_total: total_memory | bytes
        uptime_s: uptime | duration
```

**json** — the path selects the rows. `interfaces.*` means "every value of
the `interfaces` object"; an empty path means the document itself. A map key
is kept as `_key`.

```yaml
# profile
id: example_json
name: Example (JSON)
description: iproute2's JSON output.
policy: {shell: true, read_only: ['^ip -json (?:address|route) show$']}
collect:
  routes:
    - command: ip -json route show
      parser: {json: '*'}
      map:
        prefix: "'0.0.0.0/0' if dst == 'default' else dst"
        next_hop: gateway
        interface: dev
```

**raw** — the whole output in one field, for when a regex over everything is
simpler than a template.

```yaml
# profile
id: example_raw
name: Example (raw)
description: Reading one thing out of a whole page of output.
policy: {read_only: ['^show\s+\S']}
collect:
  facts:
    - command: show version
      parser: raw
      map:
        os: "'IOS-XE' if output is matching('IOS[ -]XE') else 'IOS'"
```

## Expressions

A `map` value and a `when` condition are
[MiniJinja](https://docs.rs/minijinja) expressions — the Jinja2 expression
syntax you already know from Ansible. The variables are the parsed row's
fields, plus `row` (the whole row, for a field whose name is not a valid
identifier) and `facts` (when facts were collected earlier in the same run).

A bare field name is the commonest expression of all:

```yaml
map: {name: INTERFACE}
```

Quote a literal, or it is read as a field name: `os: "'EOS'"`.

Filters Nocturne adds for network data:

| Filter | Example | Result |
|---|---|---|
| `duration` | `"2 weeks, 3 days" \| duration` | `1468800` (seconds) |
| `bps` | `"10 Gb/s" \| bps` | `10000000000` (bits per second) |
| `bytes` | `"64.0MiB" \| bytes` | `67108864` |
| `num` | `"17%" \| num` | `17` (nothing, when it is not a number). Whole numbers are exact up to 2⁶⁴−1, so 64-bit counters can be added up. |
| `mac` | `"aabb.ccdd.eeff" \| mac` | `"aa:bb:cc:dd:ee:ff"` |
| `ifname` | `"Gi1/0/1" \| ifname` | `"GigabitEthernet1/0/1"` |
| `cidr(mask)` | `"10.0.0.1" \| cidr("255.255.255.0")` | `"10.0.0.1/24"` |
| `cidr(a, b)` | `cidr(IP_ADDRESS, PREFIX_LENGTH)` | pairs two lists item by item |
| `format_each(t)` | `addr_info \| format_each("{local}/{prefixlen}")` | `["10.0.0.1/24", …]` |
| `regex_search(p)` | `version \| regex_search('^(\S+)')` | the first group, or nothing |
| `regex_replace(p, r)` | `name \| regex_replace(',$', '')` | the text with the matches replaced |

and the test `is matching('…')`. MiniJinja's own filters and tests work too;
the built-in profiles use `first`, `last`, `join`, `length`, `lower`,
`upper`, `capitalize`, `trim`, `default`, `list`, `reject`, `selectattr`, and
the tests `is startingwith(…)`, `is defined`, `equalto`.

Useful shapes:

```yaml
# A conditional, read as a sentence.
admin_up: "not LINK_STATUS is startingwith('administratively')"
# A fall-back chain: the first thing that is not empty.
device: NEIGHBOR_NAME or CHASSIS_ID
# A list, or an empty list — never a list with an empty string in it.
ipv4: "[IP_ADDRESS] if IP_ADDRESS else []"
# A lookup table with a fall-back.
protocol: "({'C': 'connected', 'S': 'static'}[PROTOCOL]) or PROTOCOL"
# Arithmetic.
memory_used: "(total_memory | bytes) - (free_memory | bytes)"
```

A field whose expression gives nothing — undefined, none, an empty string, an
empty list — is left out of the row. That is how "the device did not say" is
kept apart from "the device said nothing".

## `extends`: sharing what is the same

A child profile names its parent and states only what differs:

* maps are merged key by key,
* lists and plain values are **replaced** as a whole,
* `maturity` is never inherited: every profile states its own.

So a child that sets `cli.init` replaces the whole list, while leaving
`cli.errors` alone keeps the parent's. The built-in `_cli_ios_like` carries
the prompts, `enable`, `configure terminal`/`end`, the `%` error patterns and
the IOS-style command policy; `cisco_ios` adds detection, the collectors and
the reload-timer guard, and that is all it has to say.

Use a base when two platforms share a *CLI*, not when they share a vendor.

### Adding to an inherited list

Replacing is the right default — a child that lists its own `init` commands
means *those* commands — but it is wrong when you want the parent's list
**and** one more entry. Write `<name>_extra` and your entries are appended:

```yaml
extends: _cli_ios_like
policy:
  # NX-OS spells these as one word, so the inherited `ping`/`traceroute`
  # patterns cannot match them. The other eight stay exactly as the base
  # defines them.
  read_only_extra:
    - '^ping6\s+\S'
    - '^traceroute6\s+\S'
```

Use it, and do not copy a list in to add to it. Copying a list is how a
security-relevant one gets weakened: this repository has already had a secret
pattern that captured a key *number* instead of a key, written by an author
who needed one more pattern and restated nine. Restating
`policy.read_only` or `policy.dangerous` means retyping patterns that decide
whether a command may be sent to a device at all.

The extras go on the end, which is what order means for the lists where order
matters (`cli.init`) and does not matter for those where it does not (any
match in `policy.read_only` wins). It is refused, with the reason, if the
profile has no `extends`, if the value is not a list, or if the parent holds
that name as a single value rather than a list.

## Testing a profile

### While you write it

Paste the output of one command into the profile editor's preview and it
shows you the rows your `map` produces, including the fields that came out
empty. Offline, the same thing from a directory of files, one per command:

```
cargo run -p nocturne-drivers --example try_profile -- my_profile.yaml ./captures interfaces
```

The file names are the command with everything that is not a letter, a digit,
`.`, `+` or `-` turned into `_`: `show version` → `show_version.txt`,
`/ip route print detail` → `ip_route_print_detail.txt`,
`cat /proc/meminfo` → `cat_proc_meminfo.txt`. Run it without a directory and
it prints the names the profile expects.

### Before it ships

A profile in the built-in pack has its own directory of test data under
`crates/nocturne-drivers/tests/profiles/<id>/`. One test walks all of it, so
adding a profile's tests needs no Rust:

```
prompts.yml     exec / user / config prompts that must be recognised,
                `not_prompts` that must not be, `hosts` for the hostname read out of a prompt
errors.yml      exec.rejected / exec.clean (and config.*): output that must be caught as a
                refusal, and output that must not. A `rejected` entry may name the exact
                `error:` line that has to be reported.
policy.yml      read_only / change / dangerous: commands with the class they must get
detect.yml      banner / snmp / probe, each with `match` and `reject`, judged against the
                whole pack: evidence that must select this profile, and evidence that must not
captures/<case>/
                <command slug>.txt — the recorded output of each command, verbatim
                expect.yml       — per kind, the exact row `count` and the `rows` that must
                                   come out (each row is found by its first field; every
                                   field listed must match exactly). `count: 0` with no
                                   rows is a read that must work and find nothing.
                                   `origin:` says what the files are (below).
                SOURCE.md        — where every file came from
config/<case>.txt
                a real running configuration: it must survive cleaning unchanged, parse with
                the profile's syntax, and need no commands to be turned into itself.
                `<case>.again.txt`, a second fetch of the same unchanged device, must be
                identical after `config.ignore` has done its work.
session/<case>.yml
                a scripted login: every line the driver must send, what the device answers,
                and what the session must then do (`exec`, `configure`, `save`,
                `fetch_config`) with the result it must get
```

Run one profile's tests while you work on it:

```
NOCTURNE_PROFILE=arista_eos cargo test -p nocturne-drivers --test profiles -- --nocapture
```

Expected values are written by **reading the raw output**, not by running the
collector and pasting what it printed. Otherwise the test only proves the
code does what it does.

### What a capture is: `origin`

A capture proves as much as its source. `expect.yml` says which of three
things the files of a case are, and a case is only as good as its weakest
file:

| `origin` | The files are | Counts towards `verified` |
|---|---|---|
| `recorded` (the default) | Printed by a device of this platform and recorded: the ntc-templates corpus, a device you ran the command on. | yes |
| `documented` | A sample printed in the vendor's own documentation, copied as it stands there. `SOURCE.md` must give the page's address. | no |
| `synthetic` | Written by hand from a documented format, or printed by something that is not the platform — Nocturne's simulator included. (`synthetic: true` is the older spelling.) | no |

A documented sample is worth having — it is the vendor's own account of the
format — but it is not a recording: it has been through an editor and a
typesetter, nobody can say which release printed it, and samples that have
lost their indentation or gained a typing error are easy to find. Never call
anything `recorded` that was not.

A case may also expect the read to **fail** (`problems:` with `code: failed`
and the text the message must contain, and `qos: {}` under `kinds`): that is
how a profile's refusals are tested. Such a case proves the failure is
reported; it does not count as covering the kind.

## Maturity: how far to trust a profile

| `maturity` | What it claims |
|---|---|
| `verified` | Commands, prompts and parsers are checked against recorded output of the real platform by this repository's tests. Reserved for built-in profiles; a kind whose only captures are `synthetic: true` does not count. |
| `community` | Written from the vendor's documentation and other tools' definitions, and sound as far as that goes — but not yet checked against a device. |
| `experimental` | A draft. |

`community` and `experimental` profiles never get write access by default,
and the UI says which you are using. Be honest: a wrong `verified` is worse
than an honest `experimental`, because somebody will trust it at two in the
morning.

### One read that has not met a real device: `unverified`

Sometimes a profile has earned `verified` for everything it does and then
gains one more read for which no recording exists — the vendor documents the
command, nobody has a device to run it on. Lowering the whole profile to
`community` would take write access away from a platform that has earned it,
and leaving it `verified` would be a lie about that one read. So the profile
says exactly what is true:

```yaml
# profile
id: example_unverified
name: Example (one unverified read)
description: A profile that says which of its reads nobody has seen work on real hardware.
maturity: community
unverified: [qos]
policy: {read_only: ['^show\s+\S']}
collect:
  qos:
    - command: show policy-map interface
      parser: {regex: '(?m)^\s+Class-map: (?P<class>\S+)'}
      map: {class: class, direction: "'out'"}
```

What follows from it:

* Data collected for a listed kind is marked — `collect.get` and
  `collect.status` carry `unverified: true` on it, `profiles.list` lists the
  kinds — and every screen that shows it says the read has not been checked
  against a real device of that platform.
* The profile test accepts documented and synthetic captures for a listed
  kind, and insists on at least one that the read succeeds on.
* **The test fails as soon as a `recorded` capture covers a kind that is
  still listed.** The caveat comes off the day a recording arrives, in the
  same change — not whenever somebody remembers.

`cisco_ios` and `mikrotik_routeros` list `qos`. If you have such a device,
recording the command is read-only and takes a minute;
`crates/nocturne-drivers/tests/profiles/<id>/captures/SOURCE.md` says how to
anonymise and cite it.

## Checklist for a new profile

- [ ] `id`, `name`, `vendor`, `os`, `description` filled in; `maturity` honest.
- [ ] Prompt patterns tried against real prompts **and** against a real
      `show running-config`, counting false positives.
- [ ] `init` turns paging off; a `pager` is there for when it is refused.
- [ ] Error patterns catch what the device says when it refuses, and leave
      alone the output that merely sounds alarming.
- [ ] Every read-only command a NOC engineer uses daily classifies as
      read-only; nothing that changes state does. Destructive commands are in
      `dangerous`.
- [ ] `config.ignore` makes two fetches of an untouched device compare equal.
- [ ] `config.guard` is filled in, or its absence is explained in a comment.
- [ ] Each collector checked against real output of several devices — a
      stack, a chassis, a port that is down, a sub-interface, an interface
      with no address, a VRF.
- [ ] Units: speeds in bits per second, uptime in seconds, memory in bytes,
      MAC addresses normalised, masks as prefix lengths.
- [ ] Nothing is invented anywhere: a field the device does not print is left
      out.
- [ ] Test data under `tests/profiles/<id>/`, with `SOURCE.md` saying where
      every capture came from, and `origin:` saying what it is. Nothing that
      was not recorded from a device is called a recording.
- [ ] A read with no recording behind it is listed under `unverified`.
