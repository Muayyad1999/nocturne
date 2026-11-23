# Assistant screen and dock — working notes

Owned paths: `ui/src/pages/assistant/`, `ui/src/app/AssistantDock/`,
`ui/tests/assistant.spec.ts`, `ui/tests/assistant.shots.ts`, and the nullable
corrections in `ui/src/api/types/ai.ts`.

## Shape

```
index.tsx        the screen at /assistant: tabs (Conversation · Models · Usage)
Dock.tsx         what the shell's dock frame renders — the same Conversation
Conversation.tsx the body both of them share: transcript, approvals, composer
chat.ts          useCurrentChat (one selected chat for both) + useLiveChat
stream.ts        the attach protocol: subscribe → read → drop by `seq`  (unit tested)
markup.ts        the little formatting a model's prose needs              (unit tested)
context.ts       what the screen offers the model, as removable chips     (unit tested)
useChats.ts      the chat list, and getting hold of a chat to talk in
Transcript.tsx   turns, thinking, notices, usage
ToolStep.tsx     one tool call, opened: what ran, what came back, what it may reach
links.ts         where a step's link goes, or that it goes nowhere       (unit tested)
ApprovalCard.tsx the inline Allow / Deny for `approval_needed`
request.ts       an approval's detail as the lines the engine wrote     (unit tested)
SettingsTab.tsx  the `isp_access` switch (ISP edition only)
ledger.ts        when it is offered, and what it means                   (unit tested)
Text.tsx         prose, lists and real code blocks
ChatList.tsx     the history rail: find, switch, rename, delete
Providers.tsx    the models this installation can use
ProviderDrawer.tsx  add/edit one, list its models, test it
SecretField.tsx  the write-only API key: Set / Not set, Replace, Clear
Usage.tsx        what the conversations cost, from ai.usage
```

Backend: all 19 `ai.*` methods, the `ai.event` stream, and the two extra
topics the crate added (`ai.chats.changed`, `ai.providers.changed`).

Tests: `ui/tests/assistant.spec.ts` (17, against a real `nocturne-server` —
the free edition, and the ISP edition for the ledger switch — and a real
OpenAI-compatible endpoint served by the test process) and
`ui/tests/assistant.shots.ts` (`pnpm shots`, 19 PNGs a theme into
`tests/shots/assistant/`). Both accept `NOCTURNE_UI_DIST` to skip building the
interface, the way `NOCTURNE_SERVER_BIN` skips cargo.

## Decisions

* **A conversation without a key.** The spec's tests drive the crate with the
  scripted provider; a screen cannot. So the Playwright suite starts a small
  HTTP server of its own that speaks the OpenAI streaming shape, and saves a
  provider of kind `openai` pointing at it. The engine then really streams,
  really calls its tools against the real inventory, really stores the history
  and really publishes `ai.event` — with no network and no key. The same
  server answers the `report_ok` probe that `ai.providers.test` sends, so the
  "can this model use tools" path is exercised both ways.
* **The backend's own sentence for "no model".** `ai.status` says
  `configured: false` but does not carry the sentence that names the three
  ways in. It is `ai.chats.create` that refuses, with `unavailable` and that
  sentence — and an empty installation needs a chat created anyway. So the
  bootstrap's one `ai.chats.create` is both: it opens the first conversation,
  or it hands back the words to show. No probe, no invented copy.
* **The composer explains itself.** With no model the field still takes a
  message; sending shows the backend's sentence and **keeps the text**
  (`DockComposer.onSubmit` returning `false` means "not sent, hold it").
* **A tool result is framed as data.** The step's result sits in its own
  region, `role="group"`, labelled *Data the assistant read — not its own
  words*, with the device marker when the tool opens a session. The backend
  wraps the same text in `<nocturne:data source="…">` before the model sees
  it; this is the screen's half of that distinction. The step also names the
  RPC methods that tool may reach at all (`ai.tools`), which is what makes an
  answer auditable rather than merely plausible.
* **Attach, then read, then drop.** `stream.ts` holds events that arrive
  before `ai.chats.get` answers, ignores anything the snapshot already covers,
  raises `gap` on a jump in `seq` rather than rendering a hole, and throws the
  live turn away the moment the stored message for it arrives — which the
  backend appends *before* it sends `done`, so the full answer lands exactly
  once. A turn that was already running when this window attached is marked
  and says so on screen; it is replaced in full when it ends.
* **The composer is never remounted when the chat changes.** It used to be
  keyed on the chat id; the list refreshing underneath a half-typed message
  threw the message away (and made the end-to-end suite flaky in a way that
  took a trace of every RPC to see). Per-chat state is cleared by hand instead.
* **Creating a chat is single-flight**, and `ensureChat` waits for a create in
  flight. Pressing Enter right after *New chat* must not drop the message in
  the old conversation, and the first message of a session must not open two.
* **One chat for the dock and the screen** (`useCurrentChat`), not persisted:
  coming back opens the most recent conversation, which is what the chat list
  already means.
* Context comes from the route and from `pages/terminal/assistant.ts`, which
  the Terminal screen already fills on *Explain this*. Reading another page's
  store is not where this belongs; when the shell grows the shared "Ask the
  assistant" button, that store moves to `app/` and only one import changes.

## What the backend made awkward

* **`ai.chats.get` can answer with a `seq` newer than its `messages`.** The
  handler reads the stored messages and then the sequence number, so a turn
  that finishes between the two is reported with a `seq` that covers events
  whose message is not in the answer. A client that trusts the contract
  ("anything above this is yet to come") then treats that whole turn as
  already seen and shows nothing, for ever. `useLiveChat` carries a watchdog
  for it: while the chat says a turn is running and nothing has arrived for
  two seconds, it reads again. Taking the sequence number first, or taking
  both under one lock, would remove the need.
* **A pending approval is only ever an event.** If the window reloads while a
  tool is waiting for `ai.approve`, nothing in `ai.chats.get` says so: the
  turn sits in the gate until `approval_timeout_s` and the user cannot answer
  it. `ai.chats.get` returning the pending `{call_id, name, title, detail}`
  would make approvals survive a reload — which, for the one feature that
  stops to ask a human, is worth more than it costs.
* **A tool result is a 120-character summary and nothing else.** The step can
  say *what* ran and *what came back in one line*, but an engineer auditing an
  answer wants the rows. The full result exists (it is what went to the
  model, redacted); a method that returned it for a `call_id` would make the
  transcript genuinely auditable. Related: there is no timing on a tool call,
  so the spec's "Ran `show ip route` on CORE-SW1 · 0.8 s" cannot be drawn —
  the step shows no duration rather than inventing one.
* **`ai.status` does not carry the "no model" sentence** (see above), and the
  sentence it is in says "Settings → AI" while this build puts the models on
  the Assistant screen's Models tab. It is shown verbatim all the same, with
  the three doors directly under it.
* `ProviderEntry.effort`, `.context_window`, `.max_output_tokens`,
  `Chat.pending_turn` and `ModelInfo.context_window` / `.max_output` are
  serialised as `null`, not omitted; `ui/src/api/types/ai.ts` said `?:`. The
  types now say `?: T | null`. That is the only change made to that file.

## The engine's newest tools (third batch)

Files: `SettingsTab.tsx` + `ledger.ts` (the `isp_access` switch), `request.ts`
(an approval's lines), `links.ts` (where a step's link goes), and changes to
`ApprovalCard.tsx`, `ToolStep.tsx`, `index.tsx`. Tests: `ledger.test.ts`,
`request.test.ts`, `links.test.ts`; five more end-to-end cases in
`tests/assistant.spec.ts` (17 in all) and two more shots tests (nine more
pictures a theme). Both e2e files take `NOCTURNE_ISP_SERVER_BIN` for the ISP edition the
way `NOCTURNE_SERVER_BIN` stands in for the free one, and
`NOCTURNE_WIRE_SHAPES` to append every `ai.event` and a few answers to a file.

* **The ledger switch is a tab of its own, in the ISP edition only.** Nothing
  in the interface showed the `assistant` settings section: the Settings
  screen lists an `assistant` section that needs `ai.providers` (a method
  that does not exist) and has no body. So the switch lives on this screen,
  under *Settings*, offered when `app.methods` has `isp.summary`,
  `settings.get` and `settings.set`. In the free edition the key exists,
  false and inert, and the tab is not there; `?tab=settings` opens the
  conversation.
* **Off is nothing, not "counts and money".** `docs/dev/features/isp.md` and
  this batch's brief say the assistant sees counts and money with the switch
  off. The engine does not: with it off, `ai.tools` in the ISP edition lists
  60 tools and none of the ledger's, and the gate refuses all five ledger
  reads, `isp.summary` among them (`tools/mod.rs`, `available` and `rpc`).
  The words say what the engine does; `assistant.md` agrees with them.
* **Turning it on asks once and names the default model**: names, plans and
  balances go to whichever model a conversation uses, which may be outside
  the building. Turning it off does not ask. The switch shows what the
  engine stored, never what was clicked, and the line under it is read from
  `ai.tools`, so it shows what the engine offers rather than what the switch
  was asked to do (a 16k model is offered none of the ledger's tools even
  with it on, and the line says so).
* **An approval's request is quoted, line by line.** The engine keeps an
  alert's title, a prefix's name and a note each on one line, so that none
  can add a line to the question. A narrow frame undid that by wrapping: in
  the dock, half of a hostile title began a line at the frame's edge and
  read like a line of the question. Each line the engine wrote is now its
  own block, numbered in a gutter (CSS counters, so the numbers are not in
  the text and a screen reader is not given them), and a wrapped part hangs
  two columns deeper than its line's own text with no number. The frame has
  its own ground and face; the card's words and buttons stay outside it.
* **A long request scrolls in its frame, and says so.** The frame is capped
  (`min(36vh, 320px)`, which keeps the whole card inside the dock at
  1280×720) and is a keyboard stop only when it scrolls. A cut-off list was
  silent, so a person could allow sixteen ranges having seen ten: the card
  now says "More of the request is below" until the last line has been in
  view, then "That is the whole request."
* **A step links only where there is somewhere to go.** A step without a
  `link` shows none. A `job` link opens `/ops/jobs?job=<id>` where the build
  has the operations screen and its Jobs tab; elsewhere the job's id is a
  fact of the step, no longer an "Open the job" that could not be followed.
  An alert link says *Open the alerts*: alerts have no page of their own.
  Ids are encoded into routes.
* The new tools (`qos_status`, `top_talkers`, `list_snippets`) needed nothing
  of their own: they are steps like every other. A test drives the end to end
  case with a provider that has a window: with none configured the engine
  assumes 16 000 tokens for an endpoint on this machine and offers only the
  ten core tools, and the newest tools are not among them.

What the backend (or the shell) made awkward this time:

* `DockTranscript` (`app/AssistantDock`, not this slice) follows the end only
  while the reader is within 32px of it, and decides that on the next scroll
  event. A card that grew a second time just after it appeared could end up
  with its buttons under the fold. The card now measures before its first
  paint (`useLayoutEffect`) so it appears at its final size; a test holds the
  whole card in view at 1280×720, but the race itself could not be
  reproduced on demand.
* An approval is still only an event (see above): a reload loses the card
  until `approval_timeout_s` answers it with "nobody answered".

## What was changed after looking at the screenshots

* Bullet and numbered lists had no markers at all (`base.css` resets
  `list-style`); a model's findings read as an unlabelled block.
* "0 in · 0 out" showed under a turn that had not reported any tokens yet.
* The "this answer was already being written" notice appeared on turns the
  window had in fact seen from the first event: `partial` now comes from
  `chat.pending_turn` at a read that could have missed something, not from
  "this window did not send it".
* Two *New chat* buttons, one in the rail and one in the toolbar; the rail's
  went, since the toolbar's is the screen's one primary action.
* The setup card's prose was 54ch wide while its three doors ran the full
  width of the screen; the whole card is now one 660px column.
* The period picker on Usage was 920px wide.
* The preset grid pushed the configured models off the top of the screen; it
  is collapsible and starts closed once there is a model. The *Add* buttons
  sat at different heights depending on whether a preset had a note.
* The dock's hint line wrapped to three lines at 380px; in the dock it is now
  the model and the provider, and nothing else.
* At 1280 with the dock open the chat rail and the transcript were fighting
  over 680px. A named container query (`assistantpage`) stands the rail down —
  and it had to be the last rule in the file to beat `.rail`'s own `display`.
* `ai.usage` was being refetched for ever: `since` was recomputed on every
  render and it is part of the query key.
