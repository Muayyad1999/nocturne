# UI foundation: running log

Scope: `docs/dev/spec-ui.md`, section "What the foundation build delivers".

## Done

- Scaffold: package.json (exact pins), tsconfig (+ tsconfig.node), Vite 8,
  Vitest 5, Playwright 1.63 config, index.html, favicon.
- Tokens (`src/design/tokens.css`), base styles + tone variables (`base.css`).
- `src/lib`: cx, refs, storage, format, time, keyboard, fuzzy, diff, net
  (+ unit tests, all passing).
- `src/api`: errors, events (EventHub), transport selection, connection store,
  token capture, HTTP + Tauri transports, client.
- Overlay primitives (`src/design/overlay`): position, useFloating, layers,
  focus scope, Portal.
- Components so far: Icon, Spinner, Button, IconButton, Tooltip, Kbd,
  LogoMark, Badge, StatusDot, StatusPill (+ LabPill), EmptyState, ErrorState,
  Skeleton, Toast (+ notifications history), Popover, Menu/MenuPopup,
  DropdownMenu, ContextMenu, Tabs, TimeAgo, Highlight, Splitter/ResizeHandle,
  Field, Input, Textarea, Select, Combobox, Checkbox, Switch, RadioGroup,
  Slider, SearchInput, TagInput, Card, Panel, KeyValue, Toolbar, ProgressBar,
  Sparkline, Stat, CodeBlock, DiffView, Dialog, ConfirmDialog + confirm(),
  Drawer, Tree, Viewport (+ transform tests).
- Shell: AppShell grid, Nav, Header (+ notifications), StatusBar + status
  items store, AssistantDock frame (+ DockTranscript/DockComposer), command
  registry + CommandPalette, global shortcuts, layout/theme stores, routes
  with per-route error boundary, route stubs for every destination. Verified
  visually with a scratch screenshot (dark, 1440×900).
- API hooks (`useRpc`, `useRpcMutation`, `useEvent`), query client, mock
  transport (`src/dev/mock.ts` + `src/dev/mocks/dev.ts`).

## In progress

- Table, TimeSeriesChart (uPlot), then the showcase.

## Next

- API unit tests (transport selection, error mapping, reconnect, wildcard
  subscriptions, invalidation), tokens contrast test
- Showcase sections for every component
- Playwright smoke + bundle check, `pnpm shots`, README
- Visual review in both themes at 1440×900 and 1280×800

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
- **E2E** runs against the dev server with `?mock` (the mock is DEV-only by
  spec); the production bundle is built and checked by `tests/bundle.spec.ts`.
