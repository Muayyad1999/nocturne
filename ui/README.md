# Nocturne UI

The web interface: React 19 + TypeScript, built with Vite. The same build runs
in the Tauri desktop shell and is served by `nocturne-server` in headless mode.

The design and the rules it follows are in [`docs/dev/spec-ui.md`](../docs/dev/spec-ui.md);
what each screen becomes is in [`docs/dev/spec-ui-pages.md`](../docs/dev/spec-ui-pages.md).
This file is how to work in here.

## Running it

```bash
pnpm install                 # pnpm 10, Node 22
pnpm dev                     # http://127.0.0.1:5273
```

The UI needs an engine. There are three ways to give it one:

| | How | When |
|---|---|---|
| Mock | open `http://127.0.0.1:5273/?mock` | design and component work, no backend needed |
| Server | `cargo run -p nocturne-server -- --data-dir /tmp/n --port 0`, then open the printed `…/#token=…` link | real data; the server serves the built UI |
| Server + dev UI | `NOCTURNE_API_URL=http://127.0.0.1:PORT pnpm dev`, then open `http://127.0.0.1:5273/#token=TOKEN` | real data with hot reload (`/api` is proxied) |

The mock (`src/dev/mock.ts`, handlers in `src/dev/mocks/`) exists in
development and test builds only and is stripped from production; `?mock` does
nothing in a production build, and `tests/bundle.spec.ts` fails if it ever
reaches one.

`?latency=0` makes the mock answer immediately (the screenshot runs use it).

## Scripts

| Script | What it does |
|---|---|
| `pnpm dev` | Vite dev server |
| `pnpm build` | typecheck, then build into `dist/` — must finish with no errors and no warnings |
| `pnpm typecheck` | `tsc --noEmit` over `src` and over the tests and configs |
| `pnpm test` | Vitest: the API client, `lib/`, and component logic |
| `pnpm e2e` | Playwright: the smoke spec and the production-bundle check |
| `pnpm shots` | Playwright: writes PNGs to `tests/shots/` for visual review |

`pnpm e2e` and `pnpm shots` start the dev server themselves and reuse one that
is already running.

`NOCTURNE_E2E_BACKEND=1 pnpm e2e` additionally runs `tests/backend.spec.ts`,
which builds the UI, starts a real `nocturne-server` on a temporary data
directory and drives the app against it. Copy it when a screen needs real data.

## What is where

```
src/
  main.tsx        mounts the app: fonts, tokens, theme, token capture, router
  app/            the shell — nav rail, header, status bar, assistant dock,
                  command palette, routes and their error boundaries
  api/            client.ts (transports), hooks.ts, errors.ts, types/<module>.ts
  design/         tokens.css, base.css, control.css, and one folder per component
  lib/            pure helpers with unit tests (format, time, keyboard, fuzzy, net …)
  pages/<area>/   one folder per screen; index.tsx is the route element
  dev/            the mock transport and the component showcase (dev/test only)
tests/            Playwright specs, the screenshot run, and support/
```

A page folder owns its own components and styles. Something moves to
`design/` (visual) or `lib/` (logic) only when a second page needs it.

### Conventions

* **Tokens, never literals.** Every colour, space, radius, duration and font
  size is a CSS custom property from `design/tokens.css`. If a value is not a
  token, it does not belong in a component.
* **CSS Modules**, one `.module.css` per component. No CSS-in-JS, no utility
  classes. Variants are data attributes (`data-size`, `data-tone`), not class
  soup. The shared text-control look is `design/control.css`, applied with
  `data-control` — see `Input` for the pattern.
* **Import components from their folder** (`@/design/Button`), never from a
  barrel: the chart and the virtualised table stay out of the main chunk.
* **Three states, always**: loading (Skeletons shaped like the content, never a
  lone spinner for a whole page), empty (`EmptyState` saying what the screen is
  for, with the one action that fills it), error (`ErrorState` with the
  `RpcError` message and Retry).
* **No invented data.** A number that could not be read is an em dash, a
  capability that failed says so. Nothing in a shipped screen is a placeholder.
* **Formatting lives in `lib/format.ts`** (bytes, rates, durations, percentages,
  counts) and `lib/time.ts`. Never format inline.
* **Monospace for anything a device said** — addresses, interface names,
  commands, config — through `mono` props or the `.mono` class.
* **Status is never colour alone**: `StatusPill` and `StatusDot` carry a shape
  and a word. Lab and simulated things wear the `LAB` pill everywhere.
* **Anything the assistant proposes is tinted `--ai`** until a human accepts it.
* Everything is reachable by keyboard, has an accessible name, and shows a
  focus ring.

## Adding a page

1. Create `src/pages/<area>/index.tsx` with a default export.

   ```tsx
   import { usePageHeader } from '@/app/Header';
   import { Page } from '@/app/Page';
   import { Toolbar } from '@/design/Toolbar';
   import { useRpc } from '@/api/hooks';

   export default function DevicesPage() {
     usePageHeader({ title: 'Devices' });            // title, breadcrumbs, header actions
     const devices = useRpc<Device[]>('devices.list', undefined, {
       invalidateOn: ['devices.*'],                  // refetch when the backend says so
     });
     return (
       <Page>
         <Toolbar>{/* filters left, the one primary action right */}</Toolbar>
         {/* loading / empty / error, then the content */}
       </Page>
     );
   }
   ```

2. Add one line to `src/app/routes.tsx`:

   ```tsx
   screen('devices/:id?', () => import('@/pages/devices')),
   ```

   The route is lazy and gets its own error boundary, so a broken screen never
   takes the shell down.

3. If it belongs in the nav rail, add it to `DESTINATIONS` in
   `src/app/destinations.ts` (id, path, label, icon, group, keywords). That
   also gives it a "Go to …" command in the palette for free.

4. Write the screen's Playwright spec in `tests/`, using `support/app.ts` and,
   when it needs real data, `support/backend.ts`.

### Types from the backend

`src/api/types/<module>.ts` mirrors one backend module and is written by that
module's author. Field names stay the backend's (`snake_case`) and timestamps
are milliseconds since the epoch (`number`). Do not edit another module's file.

## Registering commands, status items and badges

All three are "while my component is mounted" registrations: call the hook, or
return the unregister function from a `useEffect`.

```tsx
// A command: appears in the palette, and runs app-wide if it has a shortcut.
useEffect(
  () =>
    registerCommand({
      id: 'devices.add',               // unique, namespaced by slice
      title: 'Add device',             // an imperative
      group: 'Devices',                // the heading it is listed under
      keywords: ['new', 'create'],
      shortcut: 'mod+shift+d',         // lib/keyboard notation; optional
      icon: Plus,
      run: () => setAdding(true),
    }),
  [],
);

// A status bar slot. It must always be true: when you do not know, pass null.
useStatusItem(
  poll.data
    ? { id: 'monitoring', tone: 'ok', label: `${poll.data.up} up`, detail: '…', onSelect: open }
    : null,
);

// A count badge on a nav destination.
useNavBadge('changes', pending > 0 ? { count: pending, label: 'pending approvals' } : null);
```

`registerCommand` is in `@/app/CommandPalette/commands`, `useStatusItem` in
`@/app/StatusBar`, `useNavBadge` in `@/app/navBadges`.

## The component showcase

`http://127.0.0.1:5273/#/dev/components` (development and test builds only)
renders every component in every state, grouped into tabs. When you add a
component or a variant, add it there — it is where the design system is
reviewed, and `pnpm shots` photographs it in both themes.

## Screenshots

`pnpm shots` writes `tests/shots/<night|day>/*.png` at 1440×900,
deviceScaleFactor 2: the shell on several screens, with the dock open, with the
rail collapsed, at 1280×720, the command palette, every showcase group, and the
floating layers (dialog, confirm, drawer, popover, menu, toasts). They are not
compared against anything — they exist to be looked at before a change ships,
and they are not kept in the repository (a run is a few megabytes and takes
about forty seconds).
