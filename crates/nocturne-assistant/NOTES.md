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
src/tools/          the boundary: boundary.rs (allow-list), mod.rs (gate + registry), 5 handler files
tests/              common/ (harness, feature stubs, HTTP fixture) + 8 test files
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

## Status

Done: everything in the spec's "Tests" section, 75 tests, clippy clean with `-D warnings`,
`cargo fmt` applied, no `unsafe`. `ui/src/api/types/ai.ts` written.

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
