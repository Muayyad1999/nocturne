# Spec: the desktop shell (`apps/desktop/src-tauri`)

Nocturne is a desktop application first. The shell is a Tauri 2 program that
starts the backend in-process, shows the web UI in the system WebView, and
connects the two. It contains no product logic: one command (`rpc`), one
event channel, window management, and packaging.

Read first: `docs/dev/spec-ui.md` (section "API client" — the UI side of this
contract; the code is `ui/src/api/transports/tauri.ts`, which you must match
exactly and may not change), `crates/nocturne-suite/src/lib.rs`,
`crates/nocturne-app/src/{app.rs,error.rs}`.

## Contract with the UI (fixed)

* `invoke('rpc', { method: string, params?: unknown })` → resolves with the
  RPC result, or rejects with `{ code, message, details? }` (the serialised
  `AppError`). Never panics across the boundary: a panic inside a handler
  rejects with `code: "internal"`.
* Every backend `Event` is emitted to all windows as `nocturne://event` with
  payload `{ topic, payload, at }`. If the forwarder falls behind the
  broadcast channel, emit `{topic: "events.lagged", payload: {missed}, at}`.
* `window.__TAURI_INTERNALS__` is how the UI detects the desktop; nothing
  else is injected.

## Behaviour

* **Backend**: `nocturne_suite::start(AppConfig::at(data_dir))` on a
  multi-threaded tokio runtime that Tauri is told to use
  (`tauri::async_runtime::set`), started before the window is created so the
  first RPC cannot race it. `data_dir` = platform data directory +
  `Nocturne` (`dirs::data_dir()`), overridable with `NOCTURNE_DATA_DIR` and
  `--data-dir`. If the backend cannot start (database unreadable, data
  directory in use by another instance or by `nocturne-server` — the core
  takes an exclusive lock and returns `conflict`) show a native error dialog
  with the message and exit non-zero; never a blank window.
* **Single instance**: a second launch focuses the existing window
  (`tauri-plugin-single-instance`).
* **Window**: title `Nocturne`, 1440×900 default, minimum 1100×700,
  remembers size/position/maximised (`tauri-plugin-window-state`). Dark
  background colour set natively (`#090d18`) so there is no white flash
  before the UI paints. The default application menu stays (on macOS the
  Edit menu is what makes copy/paste work in the WebView).
* **Closing**: closing the window quits — after `app.shutdown()` and up to
  3 s for tasks to stop — unless the `general.keep_running_in_tray` setting
  is on, in which case it hides to a tray icon (Open, Quit) and monitoring,
  log receivers and schedules keep running. Read the setting through
  `app.rpc("settings.get", …)`; the core declares the `general` section.
* **Links**: `http(s)` links clicked in the UI open in the system browser
  (`tauri-plugin-opener`), nothing navigates the WebView away from the app.
  No other scheme is honoured.
* **Security**: a strict CSP in `tauri.conf.json` (`default-src 'self';
  script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:
  blob:; font-src 'self' data:; connect-src ipc: http://ipc.localhost`),
  no remote content, capabilities limited to what the shell needs (the `rpc`
  command, core event listen, window-state, opener for http/https). No
  filesystem, shell or HTTP plugin is exposed to the WebView: everything the
  UI can do goes through `rpc`, where the backend's own rules apply.
  DevTools only in debug builds.
* **Logging**: `tracing` to stderr and to `<data_dir>/logs/nocturne.log`
  (size-capped rotation, 5 files); never log RPC params or results.
* **Notifications**: when an `alerts.changed` event announces a new firing
  alert of severity high or critical and the window is not focused, show a
  system notification (`tauri-plugin-notification`), at most one per 10 s.
  Clicking it focuses the window.

## Project layout

```
apps/desktop/src-tauri/
  Cargo.toml            # package `nocturne-desktop`, bin `nocturne`; deps: tauri 2, plugins, nocturne-suite, tokio, tracing…
  build.rs              # tauri_build::build()
  tauri.conf.json       # identifier app.nocturne.desktop, productName Nocturne, frontendDist ../../../ui/dist,
                        # devUrl http://localhost:5173, beforeDevCommand / beforeBuildCommand running pnpm in ui/
  capabilities/default.json
  icons/                # generated from the logo (see below)
  src/main.rs, src/lib.rs (run()), src/…
```

The lead adds `apps/desktop/src-tauri` to the workspace `members` (it is
excluded from `default-members` so `cargo test` at the root does not need
the WebView toolchain) — ask in your report; meanwhile build it with
`cargo build --manifest-path apps/desktop/src-tauri/Cargo.toml` and your own
`[workspace]`-less setup is NOT acceptable: tell cargo about the workspace by
temporarily running from the root with `-p nocturne-desktop` once the lead
has added the member (the lead adds it before you start).

Icons: render the `LogoMark` from `ui/src/design/` (the crescent
constellation) on the brand background to a 1024×1024 PNG (the pre-installed
Chromium via Playwright can rasterise an SVG; `ui/` has Playwright), then
`pnpm --dir ui dlx @tauri-apps/cli icon <png> -o apps/desktop/src-tauri/icons`
(or the cargo `tauri-cli`). Commit the generated icons and the source SVG.

Bundling (`tauri.conf.json` → `bundle`): targets deb, rpm, AppImage, msi,
nsis, dmg, app; category `DeveloperTool`; short and long description; no
updater, no signing configuration (documented as a later step in
`apps/desktop/README.md`, with the commands to build each bundle).

## Validation you must do (this machine is headless Linux)

1. `cargo build -p nocturne-desktop` (the WebKitGTK development packages
   are installed) and `cargo clippy -p nocturne-desktop --all-targets -- -D warnings`.
2. Unit tests for everything that is not Tauri glue: error serialisation
   shape, the event forwarder incl. the lagged notice (drive it with a real
   `App` and a fake emitter behind a trait), data-dir resolution, the
   notification throttle.
3. Run the real binary under a virtual display (`apt-get install -y xvfb`;
   `xvfb-run -a`) against a built `ui/dist` and a temp data dir: wait for the
   window (`xdotool search --name Nocturne`), take a screenshot (e.g.
   `import -window root` from imagemagick, or `xwd`), and look at it — the
   shell must be rendered (nav rail, header), not a blank or error page.
   Then drive it for real: with `tauri-driver` + `WebKitWebDriver`
   (`apt-get install -y webkit2gtk-driver`; `cargo install tauri-driver`)
   run a short WebDriver session that reads the page, confirms the
   connection indicator is online, and performs one RPC round trip through
   the UI (e.g. the device list renders its empty state from a real
   `devices.list`). If WebDriver cannot be made to work here, say exactly
   why and fall back to a debug-only `--self-test` flag that makes the shell
   evaluate a small script in the WebView calling `rpc('app.info')` and
   exits 0/1 with the result logged.
4. Second-instance behaviour and the data-directory-in-use dialog path
   (the latter by unit-testing the startup error mapping; a GUI dialog cannot
   be asserted headlessly — say so).
5. `cargo check -p nocturne-desktop --target x86_64-pc-windows-gnu` if the
   toolchain allows it; otherwise report what blocked it.
