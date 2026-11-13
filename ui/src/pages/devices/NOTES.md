# Devices + Settings screens — working log

Author: screens-devices slice.
Owned paths: `ui/src/pages/devices/`, `ui/src/pages/settings/`,
`ui/src/api/types/core.ts`, `ui/tests/devices.spec.ts`,
`ui/tests/settings.spec.ts`, `ui/tests/screens.shots.ts`.

## Log

- Read `ui/README.md`, `docs/dev/spec-ui-pages.md`, `docs/dev/spec-ui.md`,
  the showcase Patterns tab, and the Rust core
  (`inventory.rs`, `model.rs`, `connect.rs`, `collect.rs`, `credentials.rs`,
  `profiles.rs`, `settings.rs`, `system.rs`, `jobs.rs`) plus
  `nocturne-drivers` (`model.rs`, `profile.rs`, `collect.rs`) and
  `nocturne-transport` (`connection.rs`).
- This build answers `app.*`, `audit.*`, `changes.*`, `collect.*`,
  `compliance.*`, `config.*`, `credentials.*`, `devices.*`, `hostkeys.*`,
  `jobs.*`, `keys.*`, `profiles.*`, `settings.*`, `sftp.*`, `terminal.*`,
  `vault.*` — 116 methods. There is no `monitor.*`, `logs.*` or `ai.*`, so
  those Settings sections are hidden (`sections.ts` + `app.methods`).
- Nothing in this build writes `DeviceState`, so every device reads as
  "Unknown" and has no facts until it is collected from. The screens are
  built for that and say so; the populated paths are covered by
  `TestResult.test.tsx` and `device.test.ts`.
- Started `cargo build -p nocturne-server` to have a warm binary for the
  Playwright specs.

## Decisions

- **One read, local filtering.** `devices.list` is fetched once without a
  filter and the search box, facet chips and lab switch narrow it in the
  browser (`device.ts`). With 2 000 devices that keeps typing instant; the
  backend refreshes the list through `devices.changed` / `devices.state`.
- **Columns.** Model and OS version are collected *facts*, so they are
  `defaultHidden` and one click away in the column chooser: before a device
  has been collected from they are a wall of em dashes.
- **Test connection needs a saved device.** `devices.test` takes a device id,
  so the Add drawer's button is "Save and test connection" and says so; after
  the save the drawer becomes the Edit drawer for the new device.
- **"Cannot be read" vs "never collected" vs "device type not installed"** are
  three different states and the detail page says which. The third one exists
  because a device can name a profile this build does not ship.
- **The device-type editor is in-page, not a drawer**: a profile is 200 lines
  of YAML and a 640px drawer made it unreadable.
- **Settings sections are filtered by `app.methods`.** Monitoring, Logs and
  Assistant are declared in `sections.ts` and will appear by themselves when
  their crates are in the build.
- **Back up** is offered on the bulk bar and the device menu only when this
  build answers `config.backup` (`methods.ts` → `useHasMethod`). "Open
  terminals" and "Run command…" are left to the screens that own those flows.

## Things the foundation might want to look at

- `design/RadioGroup`: the real `<input>` is under `.dot`, which is
  `position: relative`, so Playwright cannot click a radio (it waits for
  pointer events forever). Checkbox and Switch do not have this problem
  because their drawn proxy is not positioned. Tests here click the label.
- `design/Field`: the "required" marker is text inside the `<label>`, so
  `getByLabel('Address')` sees "Addressrequired". The accessible name is
  right (the span is `aria-hidden`), so tests use
  `getByRole('textbox', { name: 'Address' })`.
- `design/Dialog` gives `confirm()` `role="dialog"`, not `alertdialog`.
- `plural()` is duplicated in both page folders; it belongs in
  `lib/format.ts` once a third screen wants it.

## State at hand-off

- `pnpm --dir ui build` clean, `pnpm --dir ui test` green (297 tests, 11 of
  them new here).
- `tests/devices.spec.ts` (8 tests) and `tests/settings.spec.ts` (7 tests)
  drive a real `nocturne-server` through `tests/support/backend.ts`, seeded
  over `/api/rpc` by `tests/support/seed.ts`. They run in the default
  `pnpm e2e`, which is what the CI `e2e` job runs after `cargo build`.
- `tests/screens.shots.ts` writes both themes at 1440×900 and 1280×720, with
  the dock open and closed, to `tests/shots/<night|day>/`.
