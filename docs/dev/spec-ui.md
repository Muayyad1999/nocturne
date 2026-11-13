# Spec: the Nocturne UI foundation

Nocturne's interface is a web UI (React + TypeScript) hosted in the Tauri
desktop shell, and served as-is by the headless server mode. This document
defines the foundation every screen is built on: the design system, the app
shell, the API client, and the conventions page authors follow.

The product promise is **"so advanced yet so simple"**: a NOC engineer should
find every tool they use in a day here, and never feel they are operating a
cockpit. That is a design constraint, not a slogan:

* One primary action per screen, visually obvious. Everything else is quiet.
* Dense where engineers scan (tables, terminals, configs), calm everywhere
  else (generous empty states, short sentences, no decoration for its own sake).
* Never a dead end: every empty state says what the screen is for and offers
  the one action that fills it; every error says what happened and what to do.
* Keyboard first: everything reachable from the command palette (`Ctrl/⌘ K`).
* Real data only. No placeholder numbers, no lorem ipsum in shipped screens.

## Stack (fixed)

| Concern | Choice |
|---|---|
| Build | Vite + TypeScript (`strict`, `noUncheckedIndexedAccess`), pnpm |
| UI | React 19 (function components, hooks) |
| Routing | `react-router` with a **hash** router (works under `tauri://`, `file://` and http) |
| Server state | `@tanstack/react-query` v5 over the RPC client, invalidated by backend events |
| Client state | `zustand` (small stores, one concern each) |
| Styling | CSS custom properties (design tokens) + CSS Modules. No CSS-in-JS, no utility framework |
| Icons | `lucide-react` |
| Fonts | `@fontsource-variable/inter`, `@fontsource-variable/jetbrains-mono` (bundled — the app works offline) |
| Terminal | `@xterm/xterm` + `@xterm/addon-fit`, `-search`, `-web-links` |
| Editor / diff | CodeMirror 6 (`@codemirror/state`, `view`, `commands`, `language`, `search`, `merge`) |
| Charts | uPlot behind a React wrapper (time series); inline SVG for sparklines and gauges |
| Virtual lists | `@tanstack/react-virtual` |
| Tests | Vitest (logic), Playwright (screens; Chromium is pre-installed in the build environment) |

Keep dependencies to this list unless a screen truly needs more; every extra
dependency is bundle size and supply-chain surface in an app that holds
network credentials. Pin exact versions in `package.json`.

## Directory layout

```
ui/
  index.html
  package.json  pnpm-lock.yaml  tsconfig.json  vite.config.ts  playwright.config.ts
  src/
    main.tsx                 mounts <App/>, fonts, global styles
    app/                     shell: App.tsx, routes.tsx, Nav, Header, StatusBar, AssistantDock, CommandPalette
    api/                     client.ts (transport), hooks.ts (useRpc/useMutationRpc/useEvent), errors.ts, types/*.ts (one file per backend module)
    design/                  tokens.css, base.css, themes, and the component library (one folder per component)
    lib/                     pure helpers (format.ts, time.ts, net.ts, keyboard.ts …) with unit tests
    pages/<area>/            one folder per screen area; index.tsx is the route element
    dev/                     component showcase + mock transport (dev/test builds only; excluded from production)
  tests/                     Playwright specs and screenshot scripts
```

A page folder owns its components and styles. Shared things move to `design/`
(visual) or `lib/` (logic) only when a second page needs them.

## Brand and tokens

**Nocturne** — "the NOC that never sleeps". The identity is a clear night sky:
deep blue-black surfaces, moonlight for emphasis, and colour reserved for
*meaning* (status). The logo mark is a crescent drawn as a small constellation
of nodes and links (the topology of a network reads as a star map); draw it as
an inline SVG component, `LogoMark`, that inherits `currentColor`.

All colour, spacing, radius, shadow and type values are CSS custom properties
defined once in `design/tokens.css` on `:root`, with `[data-theme="light"]`
overrides. Components never hard-code a colour.

Dark theme ("Night", the default):

| Token | Value | Use |
|---|---|---|
| `--bg-0` | `#090d18` | app background, terminal background |
| `--bg-1` | `#0e1422` | panels, nav, header |
| `--bg-2` | `#141b2d` | cards, inputs, table header |
| `--bg-3` | `#1b2439` | hover |
| `--bg-4` | `#243049` | active / selected |
| `--line` | `#222c44` | hairline borders (1px) |
| `--line-strong` | `#33405f` | input borders, dividers that must be seen |
| `--text-1` | `#e7ebf3` | primary text |
| `--text-2` | `#a8b3c7` | secondary text |
| `--text-3` | `#6c7890` | muted, placeholders, units |
| `--accent` | `#7c9cff` | primary actions, focus ring, links, selection ("moonlight") |
| `--accent-strong` | `#9db4ff` | hover of accent |
| `--accent-soft` | `rgba(124,156,255,.14)` | selected row / subtle accent fill |
| `--ai` | `#c2a5ff` | anything the assistant does or proposes |
| `--ai-soft` | `rgba(194,165,255,.14)` | |
| `--ok` | `#3dd68c` | up / healthy / success |
| `--warn` | `#f5b94a` | degraded / warning |
| `--crit` | `#ff6b6b` | down / critical / destructive |
| `--info` | `#5ab8ff` | informational |
| `--idle` | `#7a869a` | unknown / disabled / unmonitored |
| `--maint` | `#b08cff` | maintenance / lab |

Each status colour also has a `-soft` variant (same hue at ~14 % alpha) for
badge and row backgrounds. Status is never conveyed by colour alone: pair it
with an icon shape or a word.

Light theme ("Day"): same structure on `#f6f7fb` / `#ffffff` surfaces with
`#1a2233` text, accent `#3d5afe`, and status colours darkened to keep ≥ 4.5:1
contrast on white. Both themes must pass WCAG AA for text.

Type: Inter Variable for UI, JetBrains Mono Variable for anything a device
said or will be told (terminal, config, commands, addresses in tables).
Base size 13px / line-height 1.45; scale 11 · 12 · 13 · 14 · 16 · 20 · 28.
Use tabular numbers (`font-variant-numeric: tabular-nums`) in tables and
metrics. Weights: 400 body, 500 labels/buttons, 600 headings.

Space scale (px): 2 · 4 · 6 · 8 · 12 · 16 · 20 · 24 · 32 · 48. Radii: 4
(badges), 6 (controls), 10 (cards, panels), 14 (dialogs). Shadows only on
floating layers (menus, dialogs, toasts). Motion: 120 ms ease-out for hover
and focus, 180 ms for panels; respect `prefers-reduced-motion`.

Focus is always visible: a 2px `--accent` ring with 2px offset on keyboard
focus (`:focus-visible`).

## App shell

```
┌────┬───────────────────────────────────────────────────────┬─────────────┐
│    │ Header: page title · breadcrumbs        ⌘K search  🔔 │             │
│ N  ├───────────────────────────────────────────────────────┤  Assistant  │
│ a  │                                                       │  dock       │
│ v  │  Page (toolbar row, then content)                     │  (toggle,   │
│    │                                                       │  resizable) │
│    ├───────────────────────────────────────────────────────┴─────────────┤
│    │ Status bar: engine · monitoring · jobs · alerts · lab · version      │
└────┴──────────────────────────────────────────────────────────────────────┘
```

* **Nav rail** (left, 216px expanded / 56px collapsed, remembers its state):
  logo + wordmark, then grouped destinations with icon + label:

  | Group | Destinations (route) |
  |---|---|
  | Operate | Overview (`/`), Devices (`/devices`, `/devices/:id`), Terminal (`/terminal`), Topology (`/topology`), Monitoring (`/monitoring/:tab?`) |
  | Change | Config (`/config/:tab?`), Changes (`/changes`, `/changes/:id`), Automation (`/automation/:tab?`) |
  | Plan | Lab (`/lab`, `/lab/:id`), IPAM (`/ipam`), Tools (`/tools/:tool?`) |
  | Record | Operations (`/ops/:tab?`) — incidents, shift handover, reports, audit log |
  | (bottom) | Assistant (toggles the dock; `/assistant` is the full-page history), Settings (`/settings/:section?`) |

  Items can show a count badge (open alerts on Monitoring, pending approvals
  on Changes). The active item has an accent bar and `--bg-4` fill.

* **Header** (44px): page title and breadcrumbs on the left (set by the page
  through a `usePageHeader({title, breadcrumbs, actions})` hook), global
  search button that opens the command palette, notifications bell, theme
  toggle.

* **Assistant dock** (right, 380px default, resizable 320–640, toggled with
  `Ctrl/⌘ J`, state remembered): the foundation ships the dock frame with a
  header, a scrollable transcript area and a composer; the Assistant slice
  fills it in. Uses `--ai` for its accents.

* **Status bar** (24px): small, quiet, always true. Slots are filled from a
  `useStatusItems()` store that slices register into.

* **Command palette**: fuzzy list of navigation targets and registered
  commands (`registerCommand({id, title, group, keywords, shortcut, run})`),
  recent items first, full keyboard control, opens with `Ctrl/⌘ K`.

* **Toasts** (bottom-right, stack of 4, auto-dismiss except errors),
  **dialogs** (focus-trapped, `Esc` closes, destructive confirm requires
  typing or a clearly red button), **context menus**.

## Component library (`src/design/`)

Each component: its own folder (`Button/Button.tsx`, `Button.module.css`,
`index.ts`), typed props, forwards `ref` and `className`, keyboard accessible,
with a section in the showcase.

Buttons (`primary`, `secondary`, `ghost`, `danger`, `ai`; sizes `sm`/`md`;
loading state; icon-only variant with required `aria-label`) · Input,
Textarea, Select (native-backed), Combobox (filterable), Checkbox, Switch,
RadioGroup, Slider, Field (label + hint + error wrapper) · Tabs · Badge,
StatusDot, StatusPill (`ok|warn|crit|info|idle|maint` with icon) · Card, Panel
(titled, collapsible), Stat (label, big value, unit, delta, optional
sparkline) · Table (sticky header, sortable columns, row selection, column
alignment, compact 28px / comfortable 36px rows, virtualised above 200 rows,
empty and loading states, optional row actions menu) · KeyValue list ·
Tree · Tooltip, Popover, DropdownMenu, ContextMenu · Dialog, Drawer ·
Toast · EmptyState (icon, title, one sentence, primary action) · ErrorState
(what happened, retry) · Skeleton · Spinner, ProgressBar · Splitter
(resizable panes, horizontal/vertical, remembers size) · Toolbar · SearchInput
· Tag input · Kbd · CodeBlock (monospace, copy button, optional line numbers)
· DiffView (unified, +/- gutters, collapsed unchanged regions) · Sparkline ·
TimeAgo (live-updating relative time with absolute time in the tooltip) ·
`Viewport` (pan/zoom/fit container for SVG scenes: wheel-zoom to cursor,
drag-pan, pinch, fit-to-content, exposes screen↔world transforms — the
Topology and Lab canvases are built on it).

## API client (`src/api/`)

The backend exposes one RPC surface and one event stream; the same UI runs in
the desktop shell (Tauri IPC) and in a browser (HTTP + WebSocket).

```ts
// client.ts
export class RpcError extends Error { code: string; details?: unknown }
export function rpc<T = unknown>(method: string, params?: unknown): Promise<T>;
export interface AppEvent<P = unknown> { topic: string; payload: P; at: number }   // at = ms since epoch
/** `topic` may end in `*` ("devices.*"). Returns an unsubscribe function. */
export function subscribe<P = unknown>(topic: string, handler: (event: AppEvent<P>) => void): () => void;
export function connectionState(): 'connecting' | 'online' | 'offline';            // + a zustand store for it
```

* **Desktop**: detected by `window.__TAURI_INTERNALS__`; `rpc` calls
  `invoke('rpc', { method, params })`, events arrive via
  `listen('nocturne://event', …)` (use `@tauri-apps/api`).
* **Browser / server mode**: `POST /api/rpc` with JSON `{method, params}` and
  `Authorization: Bearer <token>`; events over `WebSocket /api/events?token=…`
  with automatic reconnect (back-off 0.5 s → 10 s) and re-subscription. The
  token comes from the `#token=…` URL fragment on first load and is kept in
  `sessionStorage`.
* Responses are `{ok: true, result}` or `{ok: false, error: {code, message, details?}}`.
  `RpcError.code` is a stable machine string (`not_found`, `invalid`,
  `conflict`, `unauthorized`, `unavailable`, `timeout`, `internal`, …).
* **Dev mock transport** (`src/dev/mock.ts`): selected with `?mock` in dev and
  test builds only, and stripped from production (`import.meta.env.DEV`
  guard + dynamic import). It exists so the shell and showcase can be
  developed and screenshot-tested without the backend. It must not leak into
  the production bundle — the Playwright suite asserts that the built
  bundle does not contain the mock.

```ts
// hooks.ts
export function useRpc<T>(method: string, params?: unknown, options?: { enabled?: boolean; refetchInterval?: number; invalidateOn?: string[] }): UseQueryResult<T, RpcError>;
export function useRpcMutation<T, P = unknown>(method: string, options?: { invalidates?: string[]; successToast?: string }): UseMutationResult<T, RpcError, P>;
export function useEvent<P>(topic: string, handler: (payload: P, event: AppEvent<P>) => void): void;
```

`invalidateOn` lists event topics that refetch the query (`['devices.*']`);
`invalidates` lists method prefixes whose cached queries are dropped after a
mutation. Query keys are `[method, params]`.

Types that mirror backend structs live in `api/types/<module>.ts`, written by
the author of that backend module; field names are the backend's
(`snake_case`), timestamps are milliseconds since the epoch (`number`).

## Page conventions

* A page calls `usePageHeader` and renders a `Toolbar` (filters left, primary
  action right) followed by content. Three states are always handled: loading
  (Skeletons shaped like the content, never a lone spinner for a whole page),
  empty (EmptyState), error (ErrorState with the `RpcError.message` and Retry).
* Destructive or device-touching actions state exactly what will happen and on
  which devices before they happen.
* Lab/simulated devices carry the `--maint` "LAB" pill everywhere they appear
  so they can never be mistaken for production.
* Anything proposed by the AI is tinted `--ai` until a human accepts it.
* Tables remember sort and column choices per table id (`localStorage`).
* Numbers: `lib/format.ts` owns bytes (`1.2 GiB`), rates (`940 Mb/s`),
  durations (`3d 4h`), percentages, counts with thin-space grouping; never
  format inline.
* Every interactive element is reachable and operable by keyboard, has an
  accessible name, and a visible focus state.

## What the foundation build delivers

1. The `ui/` project (scripts: `dev`, `build`, `typecheck`, `test`, `e2e`,
   `shots`), building with zero TypeScript errors and no warnings.
2. Tokens, base styles, both themes, fonts bundled.
3. The full component library above with a showcase at `/#/dev/components`
   (dev/test builds only) that renders every component in every state.
4. The app shell: nav rail with every destination, header, status bar,
   assistant dock frame, command palette, toasts, dialogs, theme toggle,
   persisted layout state, an error boundary per route.
5. Route stubs for every destination: a real `EmptyState` that explains what
   the screen will do (one honest sentence each — no fake data), ready for the
   slice authors to replace.
6. The API client and hooks, with unit tests (transport selection, error
   mapping, reconnect, wildcard subscriptions, invalidation).
7. `lib/format.ts`, `lib/time.ts`, `lib/keyboard.ts` with unit tests.
8. Playwright: a smoke spec (shell renders, navigation works, palette opens
   and navigates, theme toggles, dock toggles, no console errors), the
   production-bundle check, and `pnpm shots`, which writes PNGs of the shell
   and the showcase in both themes at 1440×900 (deviceScaleFactor 2) to
   `ui/tests/shots/` for visual review.
9. `ui/README.md`: how to run, the conventions above in brief, how to add a
   page, how to register commands and status items.
