# juniper_junos: what is checked, what is not, and what did not fit

`juniper_junos.yaml` is `maturity: community`. This file says which parts rest
on recorded output and which on the platform's documentation.

## Checked against recorded device output

Everything under `captures/`. Each of the five cases is a verbatim copy of an
ntc-templates test capture, cited file by file in its own `SOURCE.md`, and
every value in its `expect.yml` was written by reading the `.txt`.

That covers all five collectors (`facts`, `interfaces`, `neighbors`, `vlans`,
`arp`) end to end, including the two things in the profile that are more than a
field rename:

* the inline TextFSM template that reads the speed, MAC address, duplex and
  rates out of `show interfaces` — none of which the bundled template captures
  although the command prints all four — and the merge of that second step into
  the first;
* reconstructing an interface address from the two values Junos prints
  separately, `Destination: 10.10.199.176/29` and `Local: 10.10.199.180`.

## Not checked against a device at all

* **Prompts** (`prompts.yml`). Written from the documented shapes. The thing to
  verify is the inversion: `cli.config_prompt` is `#` and `cli.user_prompt` is
  switched off, because on Junos `>` is operational mode and `#` is
  configuration mode — the opposite of IOS. If that is wrong the driver will
  think a perfectly ordinary operational session is sitting in configuration
  mode, or vice versa. The FreeBSD shell prompt (`root@host%`) is in
  `not_prompts` on purpose: a session that lands there is not a Junos CLI and
  must not be driven as one.
* **Error messages** (`errors.yml`, `rejected` section), typed from the
  platform's documented messages. Junos has no `%` marker, so these patterns
  are the only thing that notices a refused command. They are deliberately
  **case-sensitive**: Junos writes `error:` in lowercase, while
  `show interfaces` output is full of `BPDU Error: None` and `MAC-REWRITE
  Error: None`, which are counters. The first `clean` sample in `errors.yml` is
  three lines of the real capture for exactly that reason.
* **Detection** (`detect.yml`). sysDescr and `show version` samples from
  documented output; object ids from the vendor's MIBs. ScreenOS answers under
  the same Juniper enterprise object id and is in `probe.reject`.
* **The whole configuration path.** `config/edge-rtr.txt` is hand-written (see
  `config/SOURCE.md` for why `| display set` and not the native form), and so
  is `session/ssh-configure-commit.yml`.
* **The `snmp` block.** Written from Juniper's chassis MIB (`jnxOperatingTable`)
  and tested only against rows written by hand. CPU is `jnxOperatingCPU` of
  the running Routing Engine rows (`jnxOperatingDescr` and
  `jnxOperatingState` choose them), the busiest taken; temperature is
  `jnxOperatingTemp` of every component, its zeros ("unavailable or
  inapplicable") dropped. Memory is left out: `jnxOperatingBuffer` is
  buffer-pool utilisation, which Juniper's documentation of jnxBoxAnatomy
  says "is valid for the FPC and Control Board"; reading the Routing Engine's
  row as its memory is other tools' habit, not Juniper's word.

## Deliberately left out

* **`config.guard`.** Junos has the best dead-man switch of any platform in
  this pack: `commit confirmed <minutes>`, cancelled by a plain `commit`. It
  cannot be declared. `config.guard.arm` is a list of *operational mode*
  commands, run before configuration mode is entered, and `commit confirmed`
  is not a command there at all — it is a parameter of the commit, inside
  configuration mode. A guard declared here would send something Junos
  rejects and report that the change was protected. Supporting it needs a
  change to `ConfigSpec`: a `commit_confirmed` template alongside `commit`, or
  for `guard.arm` to run inside configuration mode. The same limitation
  applies to `cisco_xr`.
* **The native configuration format.** `config.running` asks for
  `show configuration | display set`, not `show configuration`. Nothing in
  `hier.rs` parses a brace-delimited tree — the four grammars are IOS-style
  indentation, VRP-style indentation, flat `set` lines and RouterOS lines — and
  fed to the IOS grammar the native form would parse by indentation, treat `}`
  as a configuration line and remediate by sending `no }`. `Syntax::Set` over
  `| display set` is exact. The cost is that a fetched backup is not in the
  form an engineer reads on the box.
* **`routes`.** There is a recorded `show bgp summary` and a recorded
  `show ospf neighbor`, and neither fits: the model has `Kind::Routes` for a
  routing table, and no kind for a protocol adjacency table. Both captures are
  therefore unused. `routes` needs `show route` output, of which there is none
  recorded.
* **`health`.** No capture. It comes from `show chassis routing-engine` (CPU,
  memory, temperature) and `show system uptime`.
* **`inventory`.** No capture. It comes from `show chassis hardware`.
* **`facts.uptime_s`.** Junos's `show version` carries no uptime — it is in
  `show system uptime` — so the field is left empty rather than filled from a
  command nobody has recorded output for.
* **Four fields the model cannot hold.** `show vlans` reports the routing
  instance a VLAN belongs to (`default-switch`); the ARP table reports `Flags`
  (`none` / `permanent`); `show interfaces` reports the SNMP ifIndex and the
  last-flapped time. `Vlan`, `ArpEntry` and `Interface` in `model.rs` have no
  field for any of them and no free-form `extra` map, so they are dropped.

## Two known loosenesses in the command policy

Both are in `policy.yml` with their cases; they are noted here because they
are judgement calls, not oversights.

1. `^(?:ping|traceroute)\s+\S` admits `traceroute monitor <host>`, which
   redraws continuously and does not return on its own. `monitor traffic` and
   `monitor interface traffic` are kept *off* the read-only list for exactly
   that reason, so this is inconsistent. Tightening it means spelling out
   which `traceroute` sub-forms are allowed, which trades one kind of
   wrongness for another; it is left as it is, with the reasoning visible.
2. `^show\s+\S` admits `show … | request message all message "…"`, which
   writes to other users' terminals. The policy engine's `WRITING_PIPES` list
   (`policy.rs`) catches `save`, `tee`, `redirect`, `append`, `file`, `copy`
   and `email`, but not Junos's `request` pipe. Adding `request` to that list
   is a change to shared code and is not made here.
