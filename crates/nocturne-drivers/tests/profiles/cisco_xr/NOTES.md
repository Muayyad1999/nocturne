# cisco_xr: what is checked, what is not, and what did not fit

`cisco_xr.yaml` is `maturity: community`. This file says exactly why, so that
whoever next has a real IOS XR router in front of them knows what to check
first.

## Checked against recorded device output

Everything under `captures/`. Each of the six cases is a verbatim copy of an
ntc-templates test capture, cited file by file in its own `SOURCE.md`, and
every value in its `expect.yml` was written by reading the `.txt` — the
captures were never fed to the parser to produce an expectation.

That covers all six collectors (`facts`, `interfaces`, `neighbors`, `arp`,
`routes`, `health`, `inventory`) end to end: the commands, the templates, the
field mappings and the expressions.

## Not checked against a device at all

* **Prompts** (`prompts.yml`). Written from the documented shapes
  (`RP/0/RSP0/CPU0:host#`, `sysadmin-vm:0_RP0#`). The node part before the
  colon is the thing to verify: it changes on a route-processor switchover, and
  if the real separator or character set differs the hostname is read wrong.
* **Error messages** (`errors.yml`, `rejected` section). IOS XR complains with
  `%` the way IOS does, so `_cli_ios_like`'s patterns are inherited unchanged
  and the samples were typed from the platform's documented messages. The
  `clean` samples are the opposite — most are lifted out of the real captures,
  because the patterns that matter are the ones that must leave ordinary
  output alone.
* **Detection** (`detect.yml`). The sysDescr and `show version` samples are
  written from documented output. The object ids under `snmp:` in the profile
  are from the vendor's MIBs.
* **The whole configuration path.** `config/edge-rtr.txt` is hand-written (see
  `config/SOURCE.md`), and so is `session/ssh-configure-commit.yml`. The
  two-stage commit, `abort` rather than `end`, and the `config.ignore` patterns
  are all documented behaviour, not observed behaviour.
* **The `snmp` block.** Written from Cisco's MIBs and tested only against rows
  written by hand. CPU is the busiest row of `cpmCPUTotal1minRev`, one row per
  node. Memory is left out: `cempMemPoolTable` has a row per node and pool,
  and which row is the route processor takes a look-up of `entPhysicalName`
  that a block cannot make; the lowest index, which a "first row" would read,
  is a line card or the admin virtual machine in every published recording
  looked at. Temperature is left out: the XR agents of those recordings serve
  no ENTITY-SENSOR-MIB rows, and CISCO-ENTITY-SENSOR-MIB mixes volts, amperes,
  watts, rpm, dBm and degrees in one table, with absent sensors at -32768.

## Deliberately left out

* **`config.guard`.** IOS XR has a dead-man switch — `commit confirmed
  <minutes>`, cancelled by a plain `commit` — and it is exactly what the change
  pipeline wants. It cannot be declared. `config.guard.arm` is a list of *exec*
  commands, run before configuration mode is entered; `commit confirmed` is
  only valid *inside* configuration mode, as a parameter of the commit. A
  guard declared here would send a command the router rejects and report that
  the change was protected. `guard: null` and a comment is the honest answer.
  Supporting it properly needs a change to `ConfigSpec`: either a
  `commit_confirmed` template alongside `commit`, or for `guard.arm` to run
  inside configuration mode.
* **`config.errors`.** XR prints a `!! SYNTAX/AUTHORIZATION ERRORS` banner in
  `show configuration failed`, which would be worth matching. It is not added,
  because `find_error` uses `config.errors` *instead of* `cli.errors` when they
  are non-empty rather than in addition to them — so adding that one pattern
  would mean retyping every inherited `%` pattern and keeping the copy in step.
  Commit failures are caught anyway: XR's commit failure message starts with
  `% Failed to commit …`.
* **`policy.read_only` and `policy.dangerous` are copied in full** from
  `_cli_ios_like` with the XR-only patterns appended, for the same reason:
  `extends` replaces a list rather than extending it. The two must be kept in
  step by hand. There is a comment saying so at the top of each list.
* **`show ipv4 interface brief`.** There is a recorded capture of it and it is
  not used. It would have to be either the step that creates the interface
  rows or a step that merges into them, and as a merge step it adds nothing a
  real device does not already print in `show interfaces` — the model's
  interface row has no field for the VRF, which is the one column `show ipv4
  interface brief` has and `show interfaces` does not.
* **CPU, memory and temperature in `health`.** `health` collects the uptime
  only. XR reports CPU with `show processes cpu` and memory and temperature
  from the admin plane (`admin show environment all`); there is no recorded
  output of either, and a step written from the command's documented shape
  would be a guess that reports numbers. A missing field is a missing field;
  an invented one is worse.

## The one template bug worth knowing about

`cisco_xr_show_interfaces` reads `<name> is <X>, line protocol is <Y>` into
`LINK_STATUS` and `ADMIN_STATE` **in that order**, so `ADMIN_STATE` holds the
line-protocol state and `LINK_STATUS` the administrative one. The names are the
wrong way round. The mapping in `cisco_xr.yaml` is correct and carries a
comment; anyone editing it who trusts the field names will invert `admin_up`
and `oper_up`. `captures/asr9k-interfaces/` exists partly to catch that.
