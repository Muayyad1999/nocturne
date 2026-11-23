# Notes on the `cisco_nxos` profile

Written while building the profile from the recorded output in
`crates/nocturne-textfsm/tests/fixtures/cisco_nxos/` (the ntc-templates test
corpus, Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`).
**Nothing here has been pointed at a real Nexus**, which is why the profile
says `maturity: community` and must not be given a write path.

## Model fields that are left empty, and why

| field | why |
|---|---|
| `interfaces.vlan` | The access VLAN of a switchport is not in `show interface`; it needs `show interface switchport`, for which there is no capture. The template's `VLAN_ID` holds a *sub-interface's* encapsulation VLAN, which is a different thing, and the capture has no sub-interface, so nothing maps to `vlan` at all rather than something that is right only sometimes. |
| `interfaces.counters.in_octets` / `out_octets` on the older layout | NX-OS has two counter layouts. The current one puts the byte total on the input/output packet line, which the profile's inline template reads (`Ethernet1/1` in the capture). The older summary layout puts it on the *broadcast* line of the Rx and Tx blocks, where a single-state template cannot tell the Rx total from the Tx one; `mgmt0`'s octet counters are therefore empty, which `captures/n9k-interfaces/expect.yml` states outright. A template with Rx/Tx states would need a capture of a device that has both layouts to be worth the complexity. |
| `interfaces.ipv6` | No `show ipv6 interface brief` capture for NX-OS, although a bundled template exists. |
| `health.cpu_pct`, `health.memory_used`, `health.temperature_c` | No capture; see the missing commands below. `memory_total` is read out of `show version` with a small regex, because the bundled template does not capture it. |
| `facts.model` | Filled with what the device prints after "Nexus9000" (`C93180YC-FX3`), not the orderable part number (`N9K-C93180YC-FX3`), which only `show inventory` gives. |
| `arp` VRF | `show ip arp` was read across all contexts, and NX-OS does not print a VRF column in that layout, so there is nothing to drop here — but `ArpEntry` in `nocturne-drivers/src/model.rs` has no VRF field either, so `show ip arp vrf all` would lose it. |
| `mac_table` entries for `sup-eth1` and `Drop` | Dropped on purpose by `when`: the first carries the switch's own gateway MAC (flagged `G` in the capture's own legend) and the second is a blackhole entry. Neither is a host behind a port. |

## Commands I had no capture for (the next things to do)

1. **`show interface switchport`** — fills `interfaces.vlan` and would confirm
   `mode`. A bundled template exists (`cisco_nxos_show_interface_switchport`).
   This is the biggest functional gap: an access switch's VLAN assignment is
   invisible without it.
2. **`show processes cpu`** — `health.cpu_pct`. Bundled template exists.
3. **`show environment temperature`** — `health.temperature_c`. Bundled
   template exists.
4. **`show interface`** of a device using the older Rx/Tx counter layout
   *and* the current one — would justify a two-state inline template for the
   byte counters (above).
5. **`show vpc`** and **`show port-channel summary`** — not model kinds today,
   but a vPC peer's state is the first thing a Nexus operator looks at. A
   `show port-channel summary` capture exists in the corpus and is unused
   because `Interface` has no "member of" field.
6. **`show ipv6 interface brief`** — `interfaces.ipv6`.
7. **Memory over SNMP** — read `cpmCPUMemoryUsed` / `cpmCPUMemoryFree`
   (`…109.1.1.1.1.12` / `.13`) and `cseSysMemoryUtilization` off a Nexus next
   to `show system resources`, on one supervisor and on two, before the
   `snmp` block names either.

## Taken from vendor documentation rather than from a capture

Marked here because none of it is backed by recorded output:

* **`cli.enable: null` and `cli.user_prompt: null`.** NX-OS has one privilege
  level: a user lands at `switch#` and there is no `enable`. The scripted
  session `session/ssh-no-enable-configure-save.yml` is what holds the driver
  to it, and that transcript is scripted, not recorded.
* **`cli.errors`: `^ERROR:\s\S`.** NX-OS's features report a refusal with this
  prefix and no percent sign at all. The wordings in `errors.yml`
  (`ERROR: Invalid number of arguments`, `ERROR: Ethernet1/1: Config not
  allowed, as first port of the port-channel is a layer-2 port`) are the
  documented ones. **This pattern is broader than the base's**: any output
  line starting `ERROR: ` is read as a refusal, so a read whose *content*
  happens to begin that way (a `show tech-support` that quotes a log line)
  would be reported as failed rather than returned. That is the safe
  direction — a failed read is recorded as failed and nothing is invented —
  but it is a real false-positive risk and the first thing to re-examine
  against a device.
* **`cli.unsolicited`: the `%$ VDC-1 %$` pattern.** NX-OS puts the virtual
  device context between the timestamp and the facility tag, which the base's
  pattern (which allows no percent sign before the tag) cannot match. The same
  pattern is in `config.ignore`, so such a line does not make an untouched
  switch look changed.
* **`config.save`: `copy running-config startup-config`.** `write memory` is
  not an NX-OS command, so the base's value had to be replaced.
* **`config.ignore`: `^!Command: `, `^!Running configuration last done at: `,
  `^!Time: `** — the three header lines NX-OS generates, with no space after
  the `!`. The last two are timestamps, so without them an untouched switch
  looks changed on every fetch.
* **`detect.sysdescr` / `detect.sysobjectid`** — both NX-OS sysDescr wordings
  (`Cisco NX-OS(tm) …` and `Cisco Nexus Operating System …`) and `cevChassis`
  (`1.3.6.1.4.1.9.12.3.1.3.`), where Nexus chassis types live. The object ids
  in `detect.yml` are plausible values under those prefixes, not ones read off
  a device.
* **`detect.priority: 60`** — deliberately *equal* to `cisco_ios`, not below
  it. `cisco_ios` claims the whole `ciscoProducts` tree
  (`1.3.6.1.4.1.9.1.`) as an object-id prefix, and some Nexus platforms report
  their object id from there. A sysDescr match only outranks another profile's
  prefix when the two priorities are equal, so lowering this profile's
  priority would hand such a switch to `cisco_ios`. There is a case for it in
  `detect.yml` (`snmp.match[1]`), which fails if the priority is changed.
* **`snmp.cpu`: `1.3.6.1.4.1.9.9.109.1.1.1.1.7`** (`cpmCPUTotal1minRev`,
  CISCO-PROCESS-MIB), which NX-OS serves as IOS does: one row per processor
  the agent reports, the busiest taken (`combine: max`). No memory: NX-OS
  serves `cpmCPUMemoryUsed` / `cpmCPUMemoryFree`
  (`1.3.6.1.4.1.9.9.109.1.1.1.1.12` / `.13`, kilobytes) and the percentage
  `cseSysMemoryUtilization` (`1.3.6.1.4.1.9.9.305.1.1.2.0`), but nobody has
  read them off a Nexus, so which row comes first on two supervisors and
  whether "used" counts the page cache are not known. No temperature: the
  sensors are in CISCO-ENTITY-SENSOR-MIB, one table that also carries fan
  speeds, voltages and currents.
* **The hand-written configuration** under `config/`, as its own `SOURCE.md`
  says.

## Deliberately not claimed

* **No `detect.banner`.** NX-OS answers SSH as OpenSSH does and shows the same
  "User Access Verification" login screen as IOS, which `cisco_ios` already
  claims. A pattern here could only take detection away from it.
* **No `config.guard`.** NX-OS has no scheduled reload to arm from the exec
  prompt — there is no `reload in` — and its rollback is a checkpoint plus
  `rollback running-config`, which is a change pipeline of its own rather than
  a dead-man switch. Writing IOS's `reload in {minutes}` here would claim a
  command NX-OS does not have, and it would be sent and refused at the worst
  possible moment.
* **`cli.answers` left as the base's.** NX-OS's own confirmations ("This
  command will reboot the system. (y/n)?") all sit on destructive commands,
  and a dead-man answer to those belongs nowhere near an `answers` list. The
  inherited IOS entries are never asked by NX-OS, so they are harmless.
* **Nothing added to `policy.dangerous`.** Everything destructive I could find
  on NX-OS is already caught by the base: `reload`, `write erase`, `delete`,
  `format`, `install`, `guestshell`, `run bash`, `configure replace` and
  copying a file *onto* the device. `policy.yml` proves each one.

## Additions to `policy.read_only`

Two, both NX-OS-only, and the list had to be restated in full to make them
(see the awkwardness below):

**That `ping6` and `traceroute6` exist on NX-OS is from Cisco's documentation,
not from a capture** — the corpus has no recording of either — so it belongs
in the documentation list above as well. What the patterns *classify* is of
course proved either way; what rests on documentation is only that the
commands are worth allowing at all.

* **`^ping6\s+\S`** and **`^traceroute6\s+\S`**. IOS reaches IPv6 with
  `ping ipv6 …` and `traceroute ipv6 …`, which the base's abbreviation
  patterns already allow because they only demand white space after `ping`.
  NX-OS spells both as one word, and `^pi(?:n(?:g)?)?\s+\S` cannot match
  `ping6 2001:db8::1` — the `\s` has nowhere to go. Without these, an IPv6
  reachability check is classified as a change, so no collector and no
  assistant tool can run one. Both commands only send packets; they are the
  exact IPv6 counterparts of patterns the base already has. Each is proved in
  `policy.yml` under `read_only`, and a bare `ping6` with no argument is
  proved under `change`, so the pattern cannot be satisfied by the word alone.

## What the profile format made awkward

1. **A child replaces a list, it cannot add to one.** Adding `^ERROR:\s\S` to
   `cli.errors` meant restating the base's five patterns, two of which are
   long and gnarly; adding two IPv6 patterns to `policy.read_only` meant
   restating all eight of the base's. Every one of those restatements is a
   chance to weaken a policy by a typo, in a file that inherits from the one
   the `verified` `cisco_ios` profile uses. The tests catch a *behaviour*
   change, but only for the cases they list. An append-style merge for lists —
   or just `policy.read_only_extra` and `cli.errors_extra` — would remove the
   whole category of risk.
2. **`enable: null` works but is not documented.** Setting a map key to null
   to mean "this platform has none" is the only way to say that NX-OS has no
   privileged mode, and `docs/profiles.md` does not mention it. Worth a line
   in the `cli` table.
3. **A capture case cannot say which step produced which rows.** The
   `interfaces` kind here is built by two steps, one of which fills the octet
   counters for one interface and not another; `expect.yml` can state the
   result but not that the second step is the reason.
4. **Rows are found by their first field.** With four routes leaving by
   `Null0`, an expectation keyed on `interface` silently matched the wrong
   row — the harness compares against the first *unclaimed* row — and the
   failure read as a wrong prefix rather than a wrong key. Keying on a unique
   field (here the distance) is the workaround, but a case could do with an
   explicit "match this row by these fields".
5. **`show ip route vrf all` is one step, not two.** `cisco_ios` needs a
   required `show ip route` plus an optional `show ip route vrf *`; NX-OS
   accepts `vrf all` on every release and includes the default VRF, so one
   step does. That is a nicer shape, and the format allowed it — noted only
   because it is a difference a reader comparing the two profiles will notice.
