# nocturne-drivers — working notes (resume file)

Task: collectors, platform detection, first built-in profiles, harness, `docs/profiles.md`.
Spec: `docs/dev/spec-profiles.md`. Cargo: always
`CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/drivers cargo <cmd> -p nocturne-drivers`
(fmt too: `cargo fmt -p nocturne-drivers`). No git commands. Only touch this crate and `docs/profiles.md`.
Scratch: `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/drivers/`.
Corpus: `/home/user/.cache/vendor/ntc-templates` (tests/<platform>/<command>/*.raw + .yml).

## Status

Baseline: 61 unit tests green, clippy clean, fmt clean (before any change).

### Done
- Cargo.toml: `nocturne-textfsm.workspace = true`
- policy.rs: `Policy::shell(true)` (shell command lines) + test
- parse.rs: `parse_kv` rewritten for RouterOS v7 (index-less items, lower-case flags, `+`, log echo,
  multi-line comments); `ntc_templates`, `parse_ntc`, `parse_textfsm` + tests
- expr.rs: `bytes`, `format_each`, `cidr()` over lists, `Expressions::compile` + `Context`
- model.rs: `Kind::{is_single_row, empty_row, interface_fields}`, Display
- profile.rs: `maturity`, `interface_names`, `policy.shell`, validation (read-only collect/probe
  commands, ntc/textfsm templates, map keys are model fields, all-merge_on kinds), `Platform::
  {policy(), is_prompt, prompt_host}`, `Registry::platforms()`, compiled probe patterns
- collect.rs: Runner, Recorded, Snapshot, Collector (collect + map_output), 11 unit tests
- session.rs: `rebind`, `sync`, `login_screen`, `set_tap`, answers during login; 2 tests
- detect.rs: from_snmp, from_banner, from_probes, probe_session (unit tests still to write)

- tests/profiles.rs + tests/harness/{mod,cases,checks,captures,scripted}.rs: data-driven harness
  (prompts.yml, errors.yml, policy.yml, detect.yml, captures/<case>/, config/, session/);
  `NOCTURNE_PROFILE=<id>` runs one profile; mutation-checked by hand (it does fail when it should)
- examples/try_profile.rs: run a profile's collectors over a capture directory, print YAML
- profiles/_cli_ios_like.yaml (abstract base) + profiles/cisco_ios.yaml: DONE, harness green:
  49 prompts, 42 error samples, 54 commands, 23 detections, 20 captures (19 real from the
  ntc-templates corpus + 1 synthetic lab-device), 2 configs (+ .again), 3 scripted sessions
- session.rs: `cli.unsolicited` (prompt followed by console log lines still counts; the lab
  simulator logs %SYS-5-CONFIG_I after `end`), login settles 150 ms on the first prompt
- scratch helpers (NOT in repo): <scratch>/mkcases.py + cases_ios*.py copy corpus .raw files into
  capture dirs and write SOURCE.md

- tests/profiles.rs: `#[ignore]` removed (profiles exist now)
- `generic` profile + tests: DONE, harness green (43 prompts, 24 error samples, 27 commands,
  5 detections). generic.yaml policy gained RouterOS-style `<path> print|get|monitor` and
  bare `/export` read-only patterns, so an unidentified menu-driven CLI is still usable read-only.

- `mikrotik_routeros` profile + tests: DONE, harness green and `verified` (30 prompts,
  22 error samples, 83 commands, 16 detections, 10 captures / 62 rows, 1 config + .again).
  Collects facts, interfaces, neighbors, vlans, mac_table, arp, routes, health, inventory.
  8 of 10 capture cases are real corpus output; `ccr-core-v7` and `health-temperature` are
  marked synthetic (hand-written `/ip address print detail`, `/ipv6 address print detail`,
  `/system health print`), and every kind is also covered by a real case.

- `linux` profile + tests: DONE, harness green, `maturity: community` (36 prompts, 23 error
  samples, 87 commands, 19 detections, 3 captures / 20 rows; no config — a Linux host has no
  running configuration, `config.running` is empty on purpose). Collects facts, interfaces,
  mac_table, arp, routes, health via `ip -json`/`bridge -json`/`/proc`.
  Captures were RECORDED (not hand-written) from a real kernel: a network namespace built with
  veth+bridge on this machine, plus this machine's own uname/os-release/proc files. Only
  `hostnamectl` and the thermal-zone file are synthetic (no systemd, no sensors here).
  No IPv6/VLAN/bond in the netns capture (kernel has no modules) — noted in SOURCE.md.

- detect.rs unit tests: DONE (10 tests: snmp priority/prefix, banner, probe order + de-dup,
  rejected probes, rebinding through probe_session, generic fallback, banner fallback,
  missing generic, and the built-in pack's own detection). The test `generic` profile needed a
  `pager` or long probe output stalls — that is why the real generic.yaml has one.
- scripted sessions: mikrotik (ssh read + `d` pager + /export cleaning; telnet login with
  the `+ct511w4098h` suffix and the licence question), linux (shell prompt, command not found,
  permission denied), generic (an unidentified Huawei VRP driven through the pager).

- docs/profiles.md + tests/guide.rs: DONE. 14 loadable `# profile` examples; the worked
  example builds `arista_eos` up in 7 steps. tests/guide.rs loads every example on top of the
  built-in pack (as Origin::User, so none may claim `verified`) and checks the steps grow.
- expr.rs `Expressions::check` now also evaluates the expression once against an empty row and
  reports UnknownFilter/UnknownTest/UnknownFunction — a misspelled filter used to pass
  validation and only fail on a live device. (Additive; all existing profiles still load.)
- tests/harness/scripted.rs now requires a non-empty `description` in a session transcript
  (which also clears the pre-existing dead_code warning that failed clippy -D warnings).

### State at hand-off
- 100 lib tests + tests/profiles.rs (4 profiles) + tests/guide.rs (2) all green;
  clippy -D warnings clean; fmt clean. Registry::builtin() loads 5 profiles with zero errors.

### Next (nothing blocking; ideas for whoever carries on)
- RouterOS interface counters: no reliable command whose output is in the corpus
  (`/interface print stats` is a column table with space-separated thousands). Left out.
- RouterOS bridge-VLAN membership (`/interface bridge vlan print detail`) would complete `vlans`.
- Linux: no IPv6/VLAN/bond in the recorded netns (no kernel modules here); LLDP (`lldpctl -f
  keyvalue`) would give the `neighbors` kind.
- `Expressions::check` still cannot see a filter's argument types; only unknown names are caught.

## Decisions (keep; they go into the final report)

- Format additions (all optional, defaults keep old profiles valid):
  - `maturity: verified|community|experimental` (default experimental; NOT inherited through
    `extends`; a user profile may not claim `verified`).
  - `interface_names: cisco|verbatim` (default cisco = spec behaviour `ifname::long`; verbatim for
    platforms whose names must not be rewritten: RouterOS `vlan10`, Linux `eth0`/`lo`).
  - `policy.shell: true` — command line is a shell line: `|`, `&&`, `||`, `&` chain commands and each
    must be read-only; substitutions/redirections are never read-only (needed for `linux`).
- Map keys may be dotted (`counters.in_errors`, `extra.image`) to fill nested model fields.
- Merge: scalars keep the first non-empty value; list fields (`ipv4`, `ipv6`, `interfaces`) are
  appended to (needed for several addresses per interface).
- Kind result: every step failed → `Failed` (None); a required step failed but another succeeded →
  kept + `Partial`; optional failures only show in `steps`. A successful read with zero rows is an
  honest empty list, except single-row kinds (facts/health) where "no field" is `Failed`.
- After Timeout/Closed/Channel the remaining steps are not run (session lost).
- `kv` parser extended for RouterOS v7: items without an index number (dynamic routes), lower-case
  flags, `+` flag, log-echo lines, multi-line `;;;` comments.
- New expression helpers: `bytes` filter, `format_each` filter, `cidr()` over lists.
- `cli.unsolicited` patterns (new, optional): log lines a console prints by itself.
- Neighbor.remote_interface is NOT canonicalised (another device's port; compare with ifname::same).
- IOS speed_bps only from the negotiated speed (never the BW figure); `reload cancel` is a change.
- RouterOS: no `guard` (Safe Mode is interactive only), `newline: "\r"`, pager key `d` (dump),
  username suffix `+ct511w4098h` must be appended by the transport for SSH (app layer!).
- RouterOS listings are read with the `kv`/`colon` parsers, not ntc-templates: the bundled
  mikrotik templates match one release's `Flags:` legend and `^. -> Error` on anything else,
  while kv copes with v6 and v7 and keeps every property.
- RouterOS config is `/export terse` (needs 6.43+): the RouterOS diff/remediation in hier.rs
  only understands one self-contained command per line, which plain `/export` does not give.
- Linux: `ip -json`/`bridge -json` everywhere (no text parsing); no `config.running` (a Linux
  host has no single configuration to print); `policy.shell: true`; read as the login user,
  never through sudo. No cpu_pct: /proc has no instantaneous CPU figure, and calling the load
  average a percentage would be a fabrication.

## QoS read (`Kind::Qos`) — added 2026-11-22

Contract: `docs/dev/spec-qos.md` ("The read", "A read that has not met a real device"),
`docs/profiles.md` ("The `qos` kind", "What a capture is: `origin`", "`unverified`").

- model.rs: `Kind::Qos` (10 kinds now), `QosClass`, `Direction`. `direction` is the one field
  of the model without a default: a row that does not say which way is dropped and counted.
- collect.rs: `Snapshot.qos`. Zero rows = `Some(vec![])` (no policy); refused = `None` + `Failed`.
- profile.rs: `Profile.unverified: Vec<Kind>` + `is_unverified`, `ProfileSummary.unverified`.
  Load error when a listed kind has no steps or is listed twice. Inherited like any list.
- Harness: `expect.yml` `origin: recorded | documented | synthetic` (`synthetic: true` still
  read). Rules in `tests/harness/mod.rs::{unverified, maturity}`; each is broken on purpose by
  `tests/harness_rules.rs`. `count: 0` without rows is allowed. A case that expects the read to
  fail does not count as coverage.
- `cisco_ios` `collect.qos`: inline TextFSM for `show policy-map interface`. Levels are read
  from the column of `Class-map:` in bands (3–6, 7–10, 11–14); `Service-policy : X` at 5–8 /
  9–12 opens a nested policy. `-> Error` for: class outside the bands, 4th level, a target line
  with a colon (DLCI/VC/EFP), a policer whose `actions:` is not followed by an action.
  NOT read: the `queue stats for all priority classes` block, the old `Traffic Shaping` table
  (so no `rate_bps` for a pre-HQF shaper), WRED tables, `class … drop`.
- `mikrotik_routeros` `collect.qos`: kv over `/queue simple|tree print stats detail
  without-paging`. Simple queues are two rows (upload=in, download=out). No `rate_bps`
  (`max-limit` is not in the statistics listing; getting it needs a second listing merged on
  class AND direction, which `merge_on` cannot do — it takes one field). No interface for tree
  queues (a row's expression cannot see the row of its root).
- Both list `unverified: [qos]`. Corpus: Cisco command reference samples (`qos-doc-*`,
  `origin: documented`), the simulator (`qos-lab-*`, synthetic), hand-written
  (`qos-synthetic-*`). MikroTik: one documented sample (queue tree), the rest hand-written.
  NOTHING was recorded from a device. `tests/profiles/<id>/captures/SOURCE.md` says how to add
  a recording; the day one is added the profile test fails until the kind leaves `unverified`.
- Scratch (pages fetched, scripts): `<scratchpad>/work/build:qos-read/`.

### Review of the QoS read — 2026-11-23

What the second pair of eyes changed, and why (tests named in `src/collect.rs` unless said):

- `cisco_ios` template: three states before a class is read (`Start` → `Target` → `Policy`).
  Before, text that named no interface was read as "no policy attached", a policy shown
  without its interface line gave rows with an empty interface, and a class after an
  unrecognised policy line was filed under the policy (and direction) of the one before it.
  All three are `-> Error` now. Band 3 falls back like band 2 (a class further in than any
  policy announced for it is the nearest announced policy's), and `POLICY3` is emptied where
  `POLICY2` is, so no name survives from the interface before.
- Policer: `*_ACTION` values instead of `*_DROP` flags. Drops are computed only when both the
  conformed and the exceeded line were read with their action; an answer cut between
  `exceeded N packets; actions:` and `drop` used to read as "dropped 0".
- `bytes` only when every counter line of the class had bytes (`N packets` alone, followed by
  per-match `N packets, M bytes`, used to give the first match statement's bytes).
- `shape … cir N,` needs its comma; `police rate N bps, burst …` is read as the policer's rate;
  WRED rows at the left margin are skipped before the "below the interface" error; columns
  are counted in spaces only.
- `expr.rs` `num`: exact up to `u64::MAX` (was: a float above `i64::MAX`). `model.rs`
  `lenient::opt_u64`: beyond 64 bits is absent (was: `u64::MAX`).
- `collect.rs`: every problem message and step error is cut at 2 000 characters; a `kv`
  listing with items but no `key=value` (a table) fails the step instead of reading as empty.
- `mikrotik_routeros`: a simple queue's `a/b` is read only when it is exactly that.
- Harness: a recorded case without output for the kind does not force `unverified` off.
- Sources: every `qos-doc-*/SOURCE.md` named the wrong chapter of Cisco's reference (the page
  is "show mls qos through wrr-queue threshold"); all ten files were compared with the page
  byte for byte and are identical. Two documented IOS XE samples added
  (`qos-doc-iosxe-policing-…`, `qos-doc-catalyst9300-policer-…`).
- Known and not fixable in a parser: a number that ends its line (`b/w exceed drops: N`,
  `(total drops/bytes drops) N/M`) cannot be told from its own beginning if the answer is cut
  inside it, and a class cut off before its queue lines looks like a class without a queue.
  The session never hands over an answer that did not end in a prompt.
- Mutating a profile or a source file: restore by writing the file again (a fresh
  modification time), or cargo keeps the broken build.
