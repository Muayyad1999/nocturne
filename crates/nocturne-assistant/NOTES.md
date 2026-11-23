# nocturne-assistant — running log

Spec: `docs/dev/features/assistant.md`. Conventions: `docs/dev/features/README.md`.

## Layout

```
src/lib.rs          init(): settings, state, RPC registration, repair of interrupted turns
src/settings.rs     the `assistant` section (Autonomy parses leniently -> draft), result caps
src/redact.rs       one redactor per chat: rules + profile rules + vault values + this chat's table
src/shape.rs        shaping for a model: tables with honest caps, head/tail text, times, data blocks
src/view.rs         ai.event payloads, UiMessage/UiBlock, the per-chat sequence numbers
src/providers.rs    provider entries, key in the vault, build/test/models/detect_local/status/tools
src/chats.rs        chat documents, UI messages, model context generations, chat RPC, carry-over
src/usage.rs        hourly token ledger, ai.usage
src/prompt.rs       the frozen system prompt, the per-turn briefing, Context, quick actions
src/turn.rs         ai.send / ai.cancel / ai.approve, the turn runner, the ToolHost, the collector
src/tools/          the boundary: boundary.rs (allow-list, never-reachable list with reasons), mod.rs
                    (gate + registry), 12 handler files (devices data watch net changes design;
                    signals ops work records ledger; traffic snippets)
tests/              common/ (harness, feature stubs, reach stand-ins, HTTP fixture) + 13 test files
../nocturne-suite/tests/assistant_tools.rs   the same tools against the real features
```

## Decisions (where the spec left room)

* **System prompt frozen per chat; volatile context travels in the user message.** The spec says
  "assembled per turn", but providers bind cached and signed context (Anthropic thinking
  signatures) to the prefix of system prompt + tools + earlier messages, and answer 400 when a
  replayed block follows an edited prefix. So the system prompt holds only stable text; the date,
  the autonomy mode, the environment snapshot, the team's instructions and the `Context` are a
  `<nocturne:briefing>` block at the top of each user message (append-only, and it keeps the
  prompt cache warm). When the stable prefix really does change (`Chat.prefix`: a new version of
  the prompt, a different tool set, a different model), the stored assistant turns are replayed
  without their provider-native blocks, which every API accepts.
* **Tool results are redacted at the gate**, on every string of every RPC result, before a handler
  sees them -- so capping can never cut a secret in half and no handler can forget. Text the crate
  composed itself gets the value passes only (`scrub_values`), because the configuration patterns
  give odd results on prose that already holds placeholders (and `nocturne_ai`'s `apply` can drop
  a line when a rule matches an already-masked value -- see "For the engine crates" below).
* **The chat's redactor is primed from the stored configurations.** A secret only has to look like
  a secret once: `snmp-server community X RO` in a backup is unmistakable, the same value in a log
  line or a `show` output no pattern covers is not. Priming (once per chat, then the table lives in
  the vault) is what makes "no secret reaches the model" hold whatever order the tools run in.
  Bounded to 250 devices / 8 MB; a bigger estate is primed partially, and the patterns plus the
  vault values still apply.
* **Blanket scrubbing is limited to credential-shaped values** (8+ characters, two character
  classes). The patterns are generous and collect the odd word out of a banner; replacing *those*
  everywhere would shred the text the model has to read. Values Nocturne knows are secrets (vault,
  restored) are hidden whatever they look like.
* **`max_steps` counts tool calls** (as the settings comment says). After the last allowed call the
  model is told to wrap up and the turn ends `max_steps`.
* **Cancel never aborts device I/O**: every RPC runs in its own task and the gate stops waiting;
  the read finishes and is audited. A turn timeout is a cancel, so history stays consistent.
* **`run_commands` classifies before connecting**, with the platform policy `devices.exec` uses, so
  a refused command opens no session at all; `;`, `&&` and control characters are refused outright.
* **Chat storage**: UI messages and the verbatim model context are separate per-chat collections;
  compaction starts a new context generation and the old one stays stored.
* **Carry-over on provider switch** is a deterministic digest of the old chat (no model call).
* **`device_ping` runs `ping <target>` through `devices.exec`** rather than adding
  `probe.device.ping` to the allow-list; the target is validated to address characters first.
* Extra, not in the contract: `ai.tools`, `ai.chats.changed` / `ai.providers.changed` events, the
  `preview_change` tool, settings `approval_timeout_s` and `max_concurrent_turns`,
  `providers::use_provider` (`#[doc(hidden)]`) as the one test seam.

## The second set of tools (logs, detections, checks, ops records, discovery, automation, three writers, the ISP ledger)

* **Two properties of a method live in `boundary.rs`, not on the tool**: `ASK_FIRST` (writes a
  record) and `LEDGER` (the ISP ledger). A tool has the property because it declares such a
  method (`Tool::asks_first`, `Tool::reads_ledger`), and the gate enforces each one call by call.
  So a new tool cannot acquire a write, or the ledger, without the boundary tests noticing. (A
  third, `ALREADY_REDACTED`, became a rule for every answer: see the third set below.)
* **A tool that writes asks from inside its handler** (`ToolCtx::confirm`), not before it runs:
  the question has to show the alert's title, the prefix's own name and every range, which are
  only known after the reads and the checks. The turn runner's `Approvals` is the one mechanism
  behind both this and `confirm_reads`. The gate refuses an `ASK_FIRST` method unless this call's
  `confirm` was answered yes, and spends the yes on the write.
* **A no is `ToolOk::declined`**: `ToolOutput::ok` for the model, `ToolStatus::Denied` for the
  card. `confirm_reads` refusals are unchanged (still an error result) because existing tests and
  the UI rely on them; only the three writers use the new form.
* **Ranges are parsed, not pattern-matched.** `discovery.scan` resolves anything it cannot read
  as a range (`targets::plan` falls back to DNS), so `ab1.de` — all address characters — would
  have been resolved after the person said yes. Each part must parse as an `IpAddr`.
* **The ISP name filter is re-applied here.** `isp.customers {query}` matches phone, e-mail and
  address (`db::customer_search`), so the tool keeps only rows whose *name* matches and does not
  repeat the ledger's `matching` count when a name was given.
* **`isp.subscriptions` and `isp.arrears` were added to the allow-list** beyond what the
  specification named: `isp.customers` returns neither a plan nor a balance. Per customer shown,
  so the work is bounded by the row cap whatever the size of the ledger.
* **Foreign placeholders become `<hidden>`** (`redact::without_foreign_placeholders`) before the
  chat's own redaction runs. `<hidden>` is on the engine's `already_masked` list, so a rule that
  captures it leaves it alone.
* **Short tool descriptions by measurement** (`tools::describe_briefly`): sixty tools in the long
  form are ~14.4k estimated tokens with the system prompt, more than the 11.2k at which the agent
  compacts a conversation for a 16k model — every step ended in a summary of the step before
  (the prompted-mode test caught it). The 43 tools before this set were already at 84% of that
  limit; in the short form the sixty are at 85%.
* **Fixed on the way**: `ipam_search` read `prefix` / `vlan` / `address` / `name`, the field names
  of a stub, where the address plan answers `cidr` / `vlan_id` / `ip` / `hostname`; the stub now
  has the real shape. `query_logs`, `list_alerts`, `get_metrics`, `interface_status` and
  `list_insights` look the device up first; `list_changes` and `search_notes` followed in review.

## What the review of the second set changed

Checked against the owning crates' handlers, then against a model, a device and a feature that
all misbehave (`tests/adversary.rs`; `bend` in `tests/common/reach.rs` bends every stand-in's
answer in one place).

* **Shapes that were wrong.** `service_checks` told the model that `status: problem` keeps
  everything that is not up and that `text` matches the device's name; `checks.list` leaves
  `unknown` out of `problem` and matches the check's own name only. `list_changes` passed the
  device *name* as `device_id` (the change list compares ids) and read `targets` /
  `device_count`, which a `ChangeSummary` does not have (`devices[].name`). `search_notes` did the
  same with the name and read a `body` that `notes.list` never returns (`NoteHead` plus an
  `excerpt` when searched): the model got titles over empty blocks, with the titles outside the
  block. The three stubs now answer in the real shapes; tests that want a whole change read
  `Harness::stored_changes()`.
* **The question.** An alert's title, its device, an earlier note and a prefix's name went into
  the question as they came: a title with line breaks could forge "with the note:" and a second
  question. They are flattened and cut (`records::shown`). The alert id was not checked at all.
  U+2028, U+2029 and the direction marks are refused in anything that is recorded. A range that
  runs backwards is refused (the scan engine falls back to DNS for what it cannot expand). An
  address outside the prefix named is refused before asking. The scan's question said public
  space "is not included"; the engine refuses the whole scan, and the question now says that.
* **Stop.** A yes followed by a stop before the write used to race: the write could go ahead in
  its own task while the tool reported "cancelled" and wrote no audit entry. Now a stop before
  the write writes nothing, and a write that has begun is waited for.
* **The ledger's errors** were passed to the model in the ledger's own words. The gate now
  passes the code and a fixed sentence.
* **Bounds.** `get_evidence` (title, session, device, note, hash), `get_incident` (assignee and
  the other names), the detections catalogue and `log_context` (which lost its later headings to
  the cut in the middle) were bounded only by the agent's last-resort cut, which is blind to
  block markers. Every field now has a bound and every list its own share.
* **Redaction cost.** `scrub_json` worked out the list of values hidden everywhere once per
  string (a table export, a sort and a copy of the text per known value). It now runs the rules
  over the whole answer, takes the list once, and then hides the values — which also hides a
  secret in a row that comes before the row a rule recognised it in. The rules themselves
  (`nocturne_ai::Redactor::redact`) remain the cost: 5–8 s per 100,000 short rows in a debug
  build.
* A test worth knowing about: `what_the_new_tools_return_is_delimited_data_with_the_secrets_replaced`
  still passes with the gate's redaction removed, because the backups prime the redactor and the
  turn's last pass over a result hides known values. `a_secret_no_backup_ever_held_…` is the one
  that fails.

## The third set (QoS, flows, snippets), the boundary made whole, the small-model line

* **`qos_status`, `top_talkers`, `list_snippets`** (`tools/traffic.rs`, `tools/snippets.rs`).
  Each passes on the coverage its method gives — per device whether the QoS read worked, was
  never made, cannot be made or failed, whether it met a real device, how many classes are held
  and charted; whether the flow collector listens, where data starts, records, exporters, sets
  without a template, sampling (never multiplied); the engine's own choice of snippet variant.
  `list_snippets` asks `terminal.snippets.list {profile}` with the device's type the way a
  session gets one (set, else detected): the variant is the engine's decision, not ours, and
  `render`/`send` stay on the never-reachable list.
* **The never-reachable list is code**, `boundary::NEVER`, grouped by reason, and with
  `ALLOWED` it is every method the assembled application registers (482 in the ISP edition, 444
  in the free one; 71 allowed). The suite's `assistant_tools.rs` fails on a method on neither
  list, on both, or on a list and not registered — so a feature that adds a method breaks that
  test until somebody decides. The tests' tripwires and the never-reachable fuzz now use the whole
  list (411 names) instead of a hand-kept copy.
* **Every placeholder in every answer is foreign.** The first batch took the numbers off for five
  methods (`ALREADY_REDACTED`). Running the tools against the real features showed that most
  features redact what they answer, each in a table of its own: `config.latest` / `.search` /
  `.diff`, the change engine (`changes.create` / `.get` / `.preview`, which
  `docs/dev/features/changes.md` asks the gate to treat so), `terminal.snapshot`, the log store.
  The chat's `Redactor` leaves a `<secret:N>` it is handed alone (`already_masked`), so the model
  saw the config feature's `<secret:1>` and the chat would have restored it to *its* first
  secret — another device's — in a proposed line, under a plan a person reads with the value
  hidden. No feature holds the chat's table, so the gate now does it for every answer and every
  error. The model's reusable placeholders come from what it reads raw (`devices.exec`). With
  the rule taken out (deliberately, in the suite), the real `config.latest` answer for a device
  reached the model as `<secret:<secret:4>`, a profile rule having re-captured the tail of the
  config feature's `<secret:1>` as a "secret" `1>`: the artefact the first review saw in
  `log_context`, `search_notes` and evidence. Those answers now arrive as `<hidden>` before the
  chat's rules run, so the artefact is gone for every feature that redacts.
* **Small is 16,000 tokens or fewer** (the orchestrator's decision): a local model nobody
  configured a window for gets the core set. `ai.tools` / `ai.status` now ask the window a turn
  would use (`providers::context_window`: configured, else the provider's assumption — building
  the provider, which for tests is the override) instead of the configured window only, which
  would have listed sixty-three tools to a screen while the turn offered ten. The guard test holds
  the core set under a third of a 16k model's compaction limit (27% now) and every tool's short
  form under 90% of the smallest window that is offered all of them (87.1% at 16,001). To get
  there `DEVICE_ARG` was shortened (it is in ~32 schemas, and schemas are sent in both forms).
* **Fixed on the way, found by the suite test**: `collect_data` printed the collector's
  per-device problems (device text) and, when there was no table, a raw job result outside any
  data block, and did not hold its table to the character cap.
* **What the review of the third set changed.** Monitoring sets a class's `dropping` only from a
  drop rate it measured and clears it with the rates after a failed read, so `qos_status` shows
  `unknown` (never `no`) beside two unmeasured drop rates, and an empty `problem_only` answer is
  "none was measured dropping". `top_talkers` reads an answer without coverage as unknown, and a
  figure an answer does not carry as `-`, not 0. `list_snippets` writes a tab out like every
  other control character (on a command line it is a keystroke) and marks each line of a
  snippet's text `| `, because the text keeps its line breaks and a person's line
  `## Port bounce — for … — the one used on SW1` would otherwise pass for a heading of the list;
  an empty list for a device whose type is known says "no snippet fits", not "none is saved".
  `render_template` says, when the model gave a placeholder in `vars`, that it comes back
  `<hidden>` (the universal rule above cannot tell an echo from a feature's own) and that the
  model puts its placeholder back when proposing the lines. `unknown_device` no longer ends in
  a second "was not found". The guard holds the core set under a third, as the spec says (it said
  35%). The suite test now also runs `top_talkers` on what the real collector counted from
  hand-written NetFlow v5 datagrams sent over UDP (one sampled 1 in 100: nothing multiplied), and
  `qos_status` on the table real monitoring read off a fake device through the real `cisco_ios`
  read (the simulator's synthetic listing from the driver's captures).
* **A marker is what a model reads as one.** `shape::neutralize` defanged `</nocturne` and
  `<nocturne` in exactly that case, so `</NOCTURNE:DATA>`, `< /nocturne:data>` or a marker with a
  zero-width space in it sat in a block unchanged, for a model to take as the tool's own. Every
  `<` that, past spaces, invisible characters and a `/`, begins `nocturne` in any case is now
  defanged, for every tool; and the opening one by a visible `‹`, not by the zero-width space it
  used to get, which is itself invisible and so left a marker to a reader that does not see it.

## Status

Done: everything in the spec's "Tests" section, clippy clean with `-D warnings`, `cargo fmt`
applied, no `unsafe`. `ui/src/api/types/ai.ts` written (before the third set; the three new
tools need no new type: `ai.tools` and the events are unchanged in shape).

Wiring note for the lead: both this crate and `nocturne-changes` declare the `assistant` settings
section (the spec shares `autonomy` between them). This crate declares the whole section, the
change engine only `autonomy`, and `settings::declare` overwrites. Registering
`nocturne_assistant::init` *after* `nocturne_changes::init` keeps all the keys in `settings.all`;
the other order still works (reads fall back to the struct's defaults) but the UI sees fewer keys.

## For the engine crates (not changed here)

* `nocturne_ai::Redactor::redact` applied to text that already contains `<secret:N>` placeholders
  can (a) capture the tail of a placeholder as a new secret (`"1>"`, `"4096>"`) and (b) drop the
  rest of a line when a rule matches an already-masked value -- e.g.
  `redact("snmp-server community <secret:3> RO\n key <secret:4>\n")` loses the second line. This
  crate works around it by never running the patterns over its own composed text; the fix belongs
  in `redact.rs::apply` (advance `last` past a skipped match).
* `provider::scripted::Scripted` has no way to load more steps after construction, so the test
  harness replaces the whole provider to script a conversation (which forgets what was recorded).
  A `Scripted::load(steps)` would make multi-turn assertions simpler.
* `nocturne_app::store::secret_ids` is `pub(crate)`, so a feature cannot enumerate the vault; the
  redactor learns Nocturne's own secrets through `credentials::list` + `resolve` instead, which
  covers device credentials but not secrets another feature may have stored.
