# nocturne-assistant — running log

Spec: `docs/dev/features/assistant.md`. Conventions: `docs/dev/features/README.md`.

## Layout

```
src/lib.rs          init(): settings, state, RPC registration, crash repair
src/settings.rs     `assistant` settings section (Autonomy is parsed leniently → draft)
src/redact.rs       one redactor per chat: nocturne_ai rules + profile rules + known vault values; table in the vault
src/view.rs         ai.event payloads, UiMessage/UiBlock, the live turn view
src/providers.rs    provider entries, key in the vault, factory + cache, test / models / detect_local / status
src/chats.rs        chat documents, UI messages, model context store, chat RPC, carry-over
src/usage.rs        hourly usage ledger, ai.usage
src/prompt.rs       frozen system prompt, per-turn briefing, Context, quick actions
src/turn.rs         ai.send / ai.cancel / ai.approve, the turn runner, recorder (provider decorator), tool host
src/tools/          the boundary: allow-list, gate, registry, shaping, handlers per area
tests/it/           integration tests through app.rpc (scripted provider, stub features, fake HTTP model servers)
```

## Decisions (where the spec left room)

* **System prompt is frozen per chat; volatile context travels in the user message.** The spec says
  "assembled per turn", but Claude's current models bind thinking blocks to the conversation prefix
  (`system` + `tools` + earlier messages) and answer 400 when a replayed block follows an edited
  prefix. So: the system prompt holds only stable text (identity, method, safety rules); date,
  autonomy mode, environment snapshot, the team's standing instructions and the `Context` are a
  `<nocturne:briefing>` block at the start of each user message (append-only, also keeps the prompt
  cache warm). When the stable prefix does change (app upgrade, tool set changed), earlier assistant
  turns are replayed without their provider-native blocks — which the API accepts.
* **Tool results are redacted at the gate**, on every string of every RPC result, before a handler
  sees them — so capping can never cut a secret in half and no handler can forget.
* **Known vault values are scrubbed too** (device passwords, enable secrets, SNMP secrets, provider
  API keys, 8+ characters) wherever they appear, not only where a config rule matches.
* **`max_steps` counts tool calls** (as the settings comment says). After the last allowed call the
  model is told to wrap up; further calls are refused and the turn ends `max_steps`.
* **Cancel never aborts device I/O**: every RPC runs in its own task; the tool stops waiting, the
  read finishes and is audited.
* **`run_commands` classifies before connecting**, with the same platform policy `devices.exec`
  uses, so a refused command opens no session at all.
* **Chat storage**: model context (verbatim `nocturne_ai::Message`) and UI messages are separate
  per-chat collections; context is synced at every model call (crash-safe), compaction starts a new
  context generation and old generations stay stored.
* **Carry-over on provider switch** is a deterministic digest of the old chat (no model call).
* Extra, not in the contract: `ai.tools` (what the assistant can reach in this build),
  `ai.chats.changed` / `ai.providers.changed` events, `preview_change` tool (`changes.preview`),
  settings `approval_timeout_s` and `max_concurrent_turns`.

## Status

See the bottom of this file (updated as work proceeds).

- [ ] settings, redact, view
- [ ] tools: shape, gate, registry, handlers
- [ ] prompt, providers, chats, usage, turn, lib
- [ ] ui/src/api/types/ai.ts
- [ ] tests
- [ ] clippy / fmt
