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

### In progress
- `generic` profile + tests

### Next (order fixed by the brief)
1. `generic`, `mikrotik_routeros`, `linux` (+ their test data)
2. detect.rs unit tests (priority, probe order, rejected probes, rebinding)
3. `docs/profiles.md` + `tests/guide.rs` (loads the `# profile` YAML blocks)
4. clippy -D warnings, fmt, final report

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
