# UI foundation: running log

Scope: `docs/dev/spec-ui.md`, section "What the foundation build delivers".
How to work in here: `ui/README.md`.

## Done

- Scaffold: package.json (exact pins), tsconfig (+ tsconfig.node, which also
  typechecks `tests/`), Vite 8, Vitest 5, Playwright 1.63 config, index.html,
  favicon.
- Tokens (`src/design/tokens.css`), base styles + tone variables (`base.css`),
  shared text-control look (`control.css`).
- `src/lib`: cx, refs, storage, format, time, keyboard, fuzzy, diff, net
  (+ unit tests, all passing).
- `src/api`: errors, events (EventHub), transport selection, connection store,
  token capture, HTTP + Tauri transports, client, hooks, query client
  (+ unit tests: transport selection, error mapping, reconnect back-off,
  wildcard subscriptions, invalidation).
- Overlay primitives (`src/design/overlay`): position, useFloating, layers,
  focus scope, Portal.
- Components: Icon, Spinner, Button, IconButton, Tooltip, Kbd, LogoMark,
  Badge, StatusDot, StatusPill (+ LabPill), EmptyState, ErrorState, Skeleton,
  Toast (+ notifications history), Popover, Menu/MenuPopup, DropdownMenu,
  ContextMenu, Tabs, TimeAgo, Highlight, Splitter/ResizeHandle, Field, Input,
  Textarea, Select, Combobox, Checkbox, Switch, RadioGroup, Slider,
  SearchInput, TagInput, Card, Panel, KeyValue, Toolbar, ProgressBar,
  Sparkline, Stat, CodeBlock, DiffView, Dialog, ConfirmDialog + confirm(),
  Drawer, Tree, Viewport, **Table**, **TimeSeriesChart**.
- Shell: AppShell grid, Nav, Header (+ notifications), StatusBar + status
  items store, AssistantDock frame, command registry + CommandPalette, global
  shortcuts, layout/theme stores, routes with per-route error boundary, route
  stubs for every destination.
- Showcase at `/#/dev/components` (dev/test builds only), 11 groups, every
  component in every state.
- Playwright: `tests/smoke.spec.ts` (shell, navigation, palette, theme, dock,
  rail, 1280×720 with the dock open, showcase), `tests/bundle.spec.ts`
  (the production bundle contains neither the mock nor the showcase),
  `tests/shell.shots.ts` (`pnpm shots`), `tests/backend.spec.ts` (opt-in, a
  real `nocturne-server`), and `tests/support/` (`openApp`, `watchConsole`,
  `startBackend`).
- `ui/README.md`: running it, conventions, adding a page, registering
  commands / status items / nav badges.

## State

`pnpm build` (typecheck + build) is clean, with no warnings. `pnpm test`
262 tests green. `pnpm e2e` 9 specs green. `pnpm shots` writes 40 PNGs.

## Decisions

- **Chart palette**: the dataviz reference palette, validated with the
  skill's `validate_palette.js` on Nocturne's surfaces (dark bg-0/1/2: all
  checks pass; light: passes with the sub-3:1 contrast WARN on three hues, so
  charts always ship a legend and a table view). Slot 1 was not replaced by
  the brand blue: a periwinkle slot 1 collides with the violet slot 7.
- **AA text contrast**: the spec's `--text-3` (#6c7890) is 3.5–4.4:1 on the
  dark surfaces and `--idle` (#7a869a) is below 4.5:1 on its own soft fill.
  Both were lightened just enough to pass (#818da6, #93a0b4). The original
  `--text-3` value lives on as `--chart-muted` (a mark, not text).
  `src/design/tokens.test.ts` enforces the contrast contract in both themes.
- **Light theme**: accent `#3d5afe` is spec-fixed and cannot pass AA on its
  own soft fill, so accent text on `--accent-soft` uses `--accent-strong`
  (the `--tone-ink` variable does this for `data-tone="accent"`).
- **Events over WebSocket**: connected without a `?topics=` filter; routing is
  client-side so (un)subscribing never reconnects the socket. `events.lagged`
  and any reconnect invalidate all queries.
- **Desktop IPC**: `invoke('rpc', {method, params})` must resolve with the
  same `{ok, result | error}` envelope as HTTP (a rejection carrying
  `{code, message}` is also understood).
- **No barrel file** in `src/design`: import each component from its folder
  so heavy ones (uPlot chart, virtual table) stay out of the main chunk.
- **No cross-file `composes`.** The shared text-control look used to live in
  `design/control.module.css` and be composed by Input, Textarea, Select,
  Combobox and TagInput. The ICSS `:import` rules that generates have no
  source file, and Vite's URL-rewrite plugin warns about them on every build
  ("A PostCSS plugin did not pass the `from` option"). The spec asks for a
  build with no warnings, so the same rules are now a global stylesheet,
  `design/control.css`, addressed with `data-control`,
  `data-control-native` and `data-control-adornment`. It is imported from
  `main.tsx` right after `base.css`, so a component's own module rules — which
  are emitted later — still win at equal specificity.
- **E2E** runs against the dev server with `?mock` (the mock is DEV-only by
  spec); the production bundle is built and checked by `tests/bundle.spec.ts`.
  `openApp` goes through `about:blank` between loads: `…#/a` → `…#/b` is a
  fragment change, not a navigation, so without it the seeded theme and layout
  of a second `openApp` in the same test never took effect. It then seeds
  theme and layout to known values (defaults for anything not asked for), so
  what one `openApp` left in `localStorage` cannot leak into the next — but
  each call's seed is applied only once, so a `page.reload()` inside a test
  still shows what the test itself changed.
- **Full-page shots** of the showcase temporarily lift the scrolling off
  `main` onto the document (`unclip` in `shell.shots.ts`): the shell is a
  100 %-height grid, so a plain `fullPage: true` would capture one window and
  a page of empty paper.

## Known gaps / notes for the slice authors

- `src/api/types/` is empty: each backend module's author writes their own
  file there.
- The assistant dock ships as a frame; `pages/assistant/Dock.tsx` is a stub
  for the Assistant slice to fill.
- The status bar has only the engine and version items; monitoring, jobs,
  alerts and lab slots are registered by the slices that know them.
- xterm and CodeMirror are pinned in `package.json` but not yet used: the
  Terminal and Config screens bring them in.
