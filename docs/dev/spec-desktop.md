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
* `window.__TAURI_INTERNALS__` is how the UI detects the desktop, and the
  only thing injected that the UI relies on. Two plugins the shell uses for
  itself inject scripts of their own: the dialog plugin replaces
  `window.alert` and `window.confirm`, the notification plugin
  `window.Notification`. None of their commands is granted to the page
  (below), so from the page they do nothing — which is right, since the UI
  uses neither and has dialogs of its own.
* Besides `rpc`, the interface calls exactly two plugin commands, neither of
  which is an application method (see "What the shell lends the interface"):
  `plugin:clipboard-manager|read_text` → the text on the clipboard (rejects
  with a sentence when there is none), and `plugin:opener|open_url {url}`
  for an `http` or `https` address. The UI side of both is
  `ui/src/api/transports/shell.ts`, and nothing else in the UI may call a
  plugin.

## Behaviour

* **Backend**: `nocturne_suite::start(AppConfig::at(data_dir))` on a
  multi-threaded tokio runtime that Tauri is told to use
  (`tauri::async_runtime::set`), started before the window is created so the
  first RPC cannot race it. `data_dir` = platform data directory +
  `Nocturne` (`dirs::data_dir()`), overridable with `NOCTURNE_DATA_DIR` and
  `--data-dir`. If the backend cannot start (no data directory can be
  chosen, database unreadable, data directory in use by `nocturne-server` or
  by the other edition — the core takes an exclusive lock and returns
  `conflict`) show a native error dialog titled `<product> cannot start`
  with the message, and exit with code 1 when it is dismissed; never a blank
  window, and never a silent exit. See "Starting, in order" below for why
  the order is what it is.
* **Single instance**: one copy of an edition runs per login session
  (`tauri-plugin-single-instance`, keyed by the identifier). A second launch
  hands its command line to the first and exits 0, having created nothing —
  no database, no vault key, no lock, no log file — and the first shows,
  restores and focuses its window. If the second launch named another data
  directory with `--data-dir`, the first also says so in a dialog
  (`<product> is already open`: which folder it is working on, which one was
  asked for, and that it has to be quit first), because a window that comes
  forward without a word would pass one inventory off as another. Two copies
  on two data directories are not supported: the plugin can only be keyed by
  data directory on Linux, and the three platforms should not differ in
  this. A copy that cannot start is not "the copy that is running": before
  it shows its `cannot start` dialog it gives the name up
  (`tauri_plugin_single_instance::destroy`), so that the next launch tries
  for itself — and either starts, if what was in the way has gone, or says
  why not in a dialog of its own — instead of handing over to a dialog that
  may be behind something and ending with nothing on the screen.
* **Window**: title `Nocturne`, 1440×900 default, minimum 1100×700,
  remembers size/position/maximised (`tauri-plugin-window-state`). Dark
  background colour set natively (`#090d18`) so there is no white flash
  before the UI paints. The default application menu stays (on macOS the
  Edit menu is what makes copy/paste work in the WebView).
* **Closing**: closing the window quits — `app.shutdown()`, then up to 3 s
  until every job that was running has recorded how it ended (`jobs.get`
  reads it back finished: `src/stopping.rs`), then the process ends. Before,
  the process ended a fraction of a second after the request, and the next
  start could only write "Nocturne was closed while this was running" over
  jobs that had been told to stop and never got to say so. Quit from the
  tray, ⌘Q and the last window closing all go the same way
  (`RunEvent::ExitRequested`).
  Unless the `general.keep_running_in_tray` setting is on: then closing
  hides the window (`prevent_close`), and monitoring, log receivers,
  schedules and notifications carry on. While the setting is on there is a
  tray icon (`src/tray.rs`) with **Show <product>** and **Quit <product>**;
  on Windows a click on the icon itself shows the window, on macOS a click
  opens the menu (and the Dock icon brings the window back,
  `RunEvent::Reopen`), on Linux the indicator only ever opens the menu.
  Turning the setting off takes the icon down, and shows the window if it
  was hidden. The icon is made once and from then on only shown and hidden:
  made a second time under its name, it never reached a Linux panel,
  because libayatana-appindicator leaves the first one's objects on the
  session bus ("an object is already exported") and the panel is left with
  an icon whose menu does not answer. Starting Nocturne again brings a
  hidden window back on every platform (single instance, below), and so
  does a click on a
  notification; that is the way back on a desktop that shows no tray icons
  (GNOME without the AppIndicator extension): the window still hides there,
  and the log says the icon could not be made only when it could not.
  The shell reads the setting the way any client does — `settings.get` for
  the `general` section once the engine is up, then every
  `settings.changed` for it — and the core declares the section. On Linux
  the icon is drawn by libayatana-appindicator3, loaded when the first icon
  is made; a missing library is a panic inside that call, which the shell
  catches, logs, and carries on without the icon. The Linux packages depend
  on `libayatana-appindicator3-1` (`.github/workflows/release.yml` builds
  against the Ayatana library: Debian 12 and 13 have no
  `libappindicator3-1`).
* **Links**: `http(s)` links clicked in the UI open in the system browser
  (`tauri-plugin-opener`), nothing navigates the WebView away from the app.
  No other scheme is honoured. Two kinds of link exist and both end at
  `plugin:opener|open_url`: an `<a target="_blank">`, which the plugin's own
  script intercepts, and a link in terminal output, which is not an element
  at all (xterm draws it) and is opened by `openExternal` in
  `ui/src/api/transports/shell.ts`. `window.open` goes nowhere in the
  WebView and must not be relied on. The permission carries a scope —
  `opener:allow-open-url` with `http://*` and `https://*` — because without
  one the plugin refuses every address; `opener:default` is not used, since
  it adds `mailto:`, `tel:` and revealing files.
* **Paste**: the WebView may not read the clipboard from a script (WebKitGTK
  refuses, WebView2 asks, WKWebView shows a bubble), and Nocturne does not
  switch that on. It does not need to for the keyboard: Ctrl+Shift+V,
  Shift+Insert, ⌘V, a middle click on Linux and Edit ▸ Paste are the
  WebView's own paste and raise a `paste` event that carries the text. Only
  a paste with no such event — the Terminal screen's toolbar button and
  context menu — reads the clipboard, through `tauri-plugin-clipboard-manager`
  with `clipboard-manager:allow-read-text` and nothing else of that plugin.
  Copying uses the WebView's own clipboard and needs no permission. What
  happens to pasted text afterwards is the interface's business
  (`docs/dev/features/terminal.md`, "one road for pasted text").
* **Security**: a strict CSP in `tauri.conf.json` (`default-src 'self';
  script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:
  blob:; font-src 'self' data:; connect-src ipc: http://ipc.localhost`),
  no remote content, and a capability (`capabilities/default.json`) of
  exactly: `core:event:allow-listen` and `core:event:allow-unlisten` (the
  `nocturne://event` channel), `core:webview:allow-internal-toggle-devtools`
  (the inspector's shortcut; there is no inspector in a release build),
  `notification:allow-is-permission-granted` (asked by the notification
  plugin's own script on every page load; it answers yes or no and shows
  nothing — refused, it only puts an error in the console),
  `clipboard-manager:allow-read-text` and `opener:allow-open-url` for
  http/https. The `rpc` command needs no entry: an application's own
  commands are allowed. Nothing else: not `core:default` or
  `core:event:default`, which let the page *emit* on `nocturne://event` —
  and the shell, which listened there, turned a page's emit into an
  operating-system notification for an alert no device had raised — and
  not `dialog:default` or `notification:default`, which let a script open
  native file dialogs and show notifications of its own. The dialogs and
  notifications the shell shows are called from Rust and need no
  permission. No filesystem, shell or HTTP plugin is exposed to the
  WebView: everything the UI can *do* goes through `rpc`, where the
  backend's own rules apply. `src/capabilities.rs` pins the whole list, so
  any grant added or widened fails a test. DevTools only in debug builds.
* **Drag and drop**: belongs to the page. Tauri's own handler
  (which turns drops into `tauri://drag-drop` events carrying file paths) is
  switched off with `disable_drag_drop_handler()`: left on, it takes every
  drop away from the page on Linux and macOS and replaces WebView2's handler
  on Windows, so HTML5 drag and drop — a terminal tab onto another pane, a
  file onto the Import dialog, text onto a terminal — does not work, and
  nothing in the interface listens for Tauri's events. The other way round
  (keep the handler, read the dropped path in the shell) would need the
  shell to read files for the interface, which is a second door. A page
  would read a dropped file through `DataTransfer.files`, as it does in a
  browser — **except on Linux**, where the engine does not allow it:
  WebKitGTK gives a page no files from a drop (`DataTransfer::allowsFileAccess`
  is `false` outside Apple's ports, webkit.org/b/271957; with WebKitGTK
  2.52.6 `files` is empty and `types` has no `Files`, in a bare WebView
  with no Tauri in it). There a dropped file reaches a page only through an
  `<input type="file">` that the drop lands on, which then fires `change`
  with the file. So a screen that takes dropped files does it the way the
  Import dialog does (`ui/src/pages/devices/ImportDialog.tsx`): while
  something is dragged over the zone its file input lies over all of it,
  unseen, the drop lands on the input, and the input's `change` reads the
  file — in every engine, not as a special case for this one, so that the
  browser's end-to-end test (`ui/tests/devices.spec.ts`, "takes a dropped
  file through its file input") guards what the Linux application needs.
  Reading `DataTransfer.files` in a `drop` handler alone works in a browser
  and does nothing in the Linux application. One thing the handler did by
  accident is kept on purpose: a drop
  that misses every drop zone must not make the WebView *go to* the file.
  `on_navigation` allows the application's own pages (`tauri://…`,
  `http(s)://tauri.localhost` on Windows, and in a development build the
  one address `cargo tauri dev` serves the interface from — `build.devUrl`,
  which is where the window *starts* then, so without it the window stays
  blank), `about:` and `blob:` (a file the
  page offers to save), and refuses everything else
  (`src/navigation.rs`) — which is also what makes "nothing navigates the
  WebView away from the app" true of a link that is dragged in or followed
  without `target="_blank"`.
* **Logging**: `tracing` to stderr and to `<data_dir>/logs/nocturne.log`;
  never log RPC params or results. At 10 MiB the file is moved along to
  `nocturne.1.log` (the older ones to `.2` … `.4`, the oldest deleted) and a
  new one started, so five files and 50 MiB at most whatever the level.
  It used to be one file a day, `nocturne.<date>.log`, with no limit on
  its size — a day of `--log-level trace` was all of a disk — and no file
  of the name anybody would be asked to send. The
  file is opened only once this launch is known to be the one that stays;
  what was logged before that is held and written into it then. The log is
  closed (`Logs::close`) on `RunEvent::Exit` and before any other exit: the
  process ends inside the event loop with `std::process::exit`, which runs
  no destructor, and the lines still queued are the ones about why.
* **Notifications**: when an `alerts.changed` event carries
  `notification: "firing"` for an alert of severity high or critical and the
  window is not focused (its own focus events say so; a hidden window is
  not), show a system notification (`tauri-plugin-notification`), at most
  one per 10 s. The events are read from the engine's own stream
  (`App::subscribe`, `notify::watch`), not from the window channel, which
  a script in the page could write to. Clicking a notification brings the
  window forward — shown, restored, focused — on every platform: the shell
  registers the plugin's action handler (`on_action`, `tap`) before the
  first notification can be shown, since one shown while nothing listens
  reports no click (on Linux it then offers the server no default action,
  and KDE, XFCE, dunst and mako just dismiss it; GNOME and macOS bring the
  application forward by themselves, a hidden window included only through
  the handler). The shell does not decide what is new: the monitor says on
  which one event an alert's firing notice went to the channels
  (`docs/dev/features/monitor.md`), so maintenance windows, snoozes and a
  notice held back until a window closes are honoured here by reading that
  field and nothing else. `state`, `count`, `muted_by_maintenance` and
  `snoozed_until` are not looked at — guessing from them announced alerts
  the channels had kept quiet about, and announced an alert again whenever
  it was snoozed.

## Starting, in order

1. Arguments; `--help` and `--version` print and exit (`--version`:
   `<program> <version>`, plus ` (ISP edition)` when the assembled engine
   registers the subscriber ledger's methods — the engine's answer, read
   without starting it, which in a debug build takes seconds while the
   device profiles compile). An argument that is not an option, an
   option without its value (`--data-dir` last, or followed by another
   option) and a `--log-level` that is not one of the five are refused:
   each is said, with the near miss for a typo (`--data_dir`), and the
   program exits 2 having started nothing — it used to start on the default
   folder, or on a folder named `--log-level`, without a word. `-psn_…`,
   which older macOS adds to a program started from the Finder, is
   ignored. On Windows a release build has no console of its own, and what
   it prints to the one it was started from is dropped without an error, so
   when there is no handle to print to (`as_raw_handle` is null) these are
   shown in a message box instead; redirected or piped, they print. Logging
   starts, to the terminal only.
2. The data directory is *chosen* (not touched) and the runtime is built.
3. `tauri::Builder::build`. Plugins are set up here, in the order they were
   added, and `tauri-plugin-single-instance` is added first: this is where a
   second launch finds the first, hands over and ends. Nothing before this
   point may write into the data directory. A window system that cannot be
   reached is not an `Err` from `build`: GTK without a display panics in
   there (tao, "Failed to initialize gtk backend!"). The panic is caught
   (`catch_unwind`; the release profile unwinds) and ends the start the way
   an `Err` does: "Nocturne cannot start" in the terminal and in the log
   file, exit 1 — not a bare panic with 101 and nothing in the log, which is
   all a launcher start used to leave.
4. The `setup` closure, which Tauri runs when the event loop is ready
   (`src/lib.rs`, `start`): open the log file, start the engine
   (`src/startup.rs`), manage the state, forward events, open the window.
   A failure here is shown with the dialog plugin — the event loop is
   running, so a dialog can be — and the program exits 1 when the dialog is
   dismissed. No window is opened behind it, and from that moment the copy
   no longer answers to the application's name (see "Single instance").

The engine used to be started at step 2. A second launch then met the first
one's lock on the data directory before the plugin had its turn: it printed
the conflict to a terminal nobody has and exited 1, the first window stayed
where it was, and — with another `--data-dir` — it had made itself a
database and a vault key in that folder before the plugin ended it.

What is left of "the data directory is in use" after step 3 is the case the
plugin cannot see: `nocturne-server`, or the other edition. Both editions
keep their data in the same folder by design (one inventory, whichever
edition is opened), they have different identifiers, and so the second one
to start is told, in the dialog, that Nocturne is already running on that
folder.

Not covered, and said here rather than found out: a launch that chooses its
folder with `NOCTURNE_DATA_DIR` instead of `--data-dir` hands over like any
other, but the environment is not part of what is handed over, so the first
copy cannot tell that another folder was meant and shows no dialog. Where
there is no session bus (Linux, e.g. over plain SSH with X forwarding) a
second launch cannot find the first; it then meets the lock and gets the
`cannot start` dialog. A failure of the toolkit itself — no runtime, no
display, `build` failing — has no dialog to be shown in and goes to the
terminal and the log file, with exit code 1 (`tests/launch.rs`, run by every
`cargo test`, starts the program with no display and reads both).

## What the shell lends the interface

The rule is one door: everything Nocturne does is a method on
`App::rpc`, and the shell has one command for it. Reading the clipboard and
opening a link are deliberately **not** methods, and they are not a second
door either. The test is whose machine the thing belongs to:

* An application method acts on the engine's world — devices, sessions, the
  store, the vault. It means the same whether the engine is in this process
  or on a server three hops away, and the assistant can be given it by name.
* The clipboard and the web browser belong to the person at the window. In
  server mode the engine runs on another machine: a `clipboard.read` method
  would read the *server's* clipboard, and `links.open` would start a browser
  on a headless box. They are properties of where the window is, like having
  a window, a title bar or a notification — which is why the browser build
  gets them from the browser, with no backend involved at all, and the
  desktop build gets them from the shell.

So they are capabilities of the shell, provided by official Tauri plugins,
with the narrowest permission each plugin has, and with no application
state behind them: neither command can reach a device, the store or the
vault, and nothing the assistant does goes through them. Adding a third
needs the same argument, written here; "the UI needed it" is the argument
for an RPC method instead. (`notification:allow-is-permission-granted` is
not a third: the interface never calls it. The notification plugin's own
script asks it on every page load, and it answers yes or no.)

| Capability | Plugin and permission | Used for |
|---|---|---|
| Read text from the clipboard | `tauri-plugin-clipboard-manager`, `clipboard-manager:allow-read-text` | Paste from the Terminal screen's toolbar and context menu |
| Open a web address | `tauri-plugin-opener`, `opener:allow-open-url` scoped to `http://*`, `https://*` | Links in the UI and in terminal output |

## Project layout

```
apps/desktop/src-tauri/
  Cargo.toml            # package `nocturne-desktop`, bin `nocturne`; deps: tauri 2, plugins, nocturne-suite, tokio, tracing…
  build.rs              # tauri_build::build()
  tauri.conf.json       # identifier app.nocturne.desktop, productName Nocturne, no version (Cargo.toml's),
                        # frontendDist ../../../ui/dist, beforeDevCommand / beforeBuildCommand building ui/
                        # with "cwd": "." (this folder) so they work wherever the CLI is started
  capabilities/default.json
  windows/hooks.nsh     # the NSIS uninstaller's word on the shared data folder
  ../scripts/check-installers.sh  # run by the release on every installer before it is uploaded
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
updater and no certificate: nothing is signed by a certificate authority
(documented as a later step in `apps/desktop/README.md`, with the commands
to build each bundle). The macOS app carries an ad-hoc signature
(`bundle.macOS.signingIdentity: "-"`), which names nobody: a downloaded,
quarantined bundle with none is reported by macOS as damaged, with no
"Open Anyway" in Privacy & Security to get past it. The version is not in
`tauri.conf.json`: the bundler takes Cargo's, so the installers, the
packages, `--version` and `app.info` have one version between them, and the
release refuses a tag that is not `v` + that version.

No uninstaller deletes the data folder (`<platform data dir>/Nocturne`): it
is shared by both editions and `nocturne-server`, and an uninstaller cannot
tell that it is the last thing using it. The Windows (NSIS) uninstaller's
"Delete the application data" box removes the folders named after the
identifier (WebView storage, window state); with it ticked, a hook
(`windows/hooks.nsh`, `bundle.windows.nsis.installerHooks`) says that the
inventory, the credentials and their key are still in `%APPDATA%\Nocturne`
and why — it used to say nothing, and the box passed for having deleted
them. `src/editions.rs` holds both editions to the hook and the hook to
deleting nothing.

The release (`.github/workflows/release.yml`) builds the installers with
`tauri-action` without letting it upload, and runs
`apps/desktop/scripts/check-installers.sh` on what it produced: the
interface that was compiled in (`ui/dist`, which `build.rs` replaces with a
"not built" placeholder when it is missing — a page that compiles and
installs as well as the real one) must be the real one, and every file
must be named for its edition and version (`Nocturne_…`, `Nocturne ISP_…`;
GitHub shows the space as a dot), the Debian and RPM packages must have the
edition's package name and version and depend on the Ayatana indicator
library and not on `libappindicator3-1`, the macOS bundle must have the
edition's identifier and program, both architectures and a valid ad-hoc
signature, and the program inside every installer — `.deb`, `.rpm`,
AppImage, `.dmg`, `.app`, NSIS `.exe` and `.msi`, each unpacked rather than
installed — must answer `--version` with the edition's name, the version and
`(ISP edition)` exactly when it is the ISP build. Only then is anything
uploaded. What that cannot see is whether an installer *installs* and the
program then *runs* on a clean machine of each platform; a person trying
the release is the check for that.

The ISP edition is the same crate built with `--features isp` and packaged
with `--config tauri.isp.conf.json` laid over `tauri.conf.json`
(`docs/dev/spec-editions.md`). The two install side by side, which holds
only while nothing an installer writes has the same name in both. What the
packager names after what:

| On disk | Named after | Free | ISP |
|---|---|---|---|
| Debian/RPM package, `.desktop` file name, install folder, installer file names | `productName` | `Nocturne` → package `nocturne` | `Nocturne ISP` → package `nocturne-isp` |
| `/usr/bin/…`, `Exec=`, `Icon=`, `StartupWMClass=`, the icon files under `hicolor`, the `.exe`, the process the Windows installer looks for before it replaces files | the main binary: `mainBinaryName`, else Cargo's `[[bin]]` name | `nocturne` | `nocturne-isp` |
| bundle id, WebView data folder, single-instance name (D-Bus name, mutex, socket) | `identifier` | `app.nocturne.desktop` | `app.nocturne.desktop.isp` |

`mainBinaryName` is set in `tauri.isp.conf.json` only; Cargo still builds
`nocturne`, and `tauri build` renames it. `src/editions.rs` fails if any of
the three rows comes out the same for both. `--help` and `--version` print
the name the program was started under. Only a build made through the Tauri
CLI with the ISP configuration is a separate application: a plain
`cargo build -p nocturne-desktop --features isp` has the billing code and
the free edition's identifier and name.

## Validation you must do (this machine is headless Linux)

1. `cargo build -p nocturne-desktop` (the WebKitGTK development packages
   are installed) and `cargo clippy -p nocturne-desktop --all-targets -- -D warnings`.
2. Unit tests for everything that is not Tauri glue: error serialisation
   shape, the event forwarder incl. the lagged notice (drive it with a real
   `App` and a fake emitter behind a trait), data-dir resolution, the
   notification throttle, and the capability file (`src/capabilities.rs`:
   read-text only, http/https only, and the plugin behind each permission
   registered in `run` — a permission for a plugin that is not in the builder
   fails only in the installed program). The interface's half is checked in
   a browser made to behave like the WebView (`ui/tests/terminal.spec.ts`,
   "through the desktop shell"): no clipboard for a script, `window.open`
   going nowhere, and only the two plugin commands answered.
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
   With a session open to a console that records what it runs, check the
   two things only the real WebView can show. Paste: Shift+Insert,
   Ctrl+Shift+V and the toolbar button each open the paste sheet for a
   multi-line clipboard and the console has run nothing until it is
   confirmed, then each line once. Links: with stand-ins for `xdg-open` and
   friends first on `PATH`, clicking an `https://` address in the output
   calls one of them with that address.
4. Second-instance behaviour and the data-directory-in-use dialog, against
   the built program: `tests/launch.rs` starts it more than once on a
   display and looks at its windows, exit codes, log file and what was left
   in the data directory. Four of them are `#[ignore]`d because they need
   a display, a session bus and `xdotool`:
   `xvfb-run -a dbus-run-session -- cargo test -p nocturne-desktop --test launch -- --ignored`;
   the fifth starts it with no display at all, needs nothing, and runs with
   every `cargo test`.
   The tray was checked by hand the same way, under `xvfb-run` and
   `dbus-run-session` with a stand-in `org.kde.StatusNotifierWatcher` and
   `org.freedesktop.Notifications` on the session bus and the page driven
   through WebKit's inspector: the setting saved through `rpc` puts the
   icon up with Show and Quit, `WM_DELETE_WINDOW` hides the window and the
   process stays, an alert raised then becomes a notification carrying the
   `default` action and its `ActionInvoked` brings the window back, the
   menu's Show and Quit do what they say, turning the setting off takes the
   icon down, and with it off closing quits.
   The parts that need no window are ordinary unit tests: the startup error
   and its log line (`src/startup.rs`), the late log file (`src/logging.rs`),
   which folder a second launch asked for and which arguments are refused
   (`src/data_dir.rs`), where the window may navigate (`src/navigation.rs`),
   and — with the real engine behind `App::rpc` — which alert events become
   a notification (`src/notify.rs`), what the tray setting makes closing do
   (`src/tray.rs`), and that quitting waits for a job to record that it was
   stopped (`src/stopping.rs`).
5. `cargo check -p nocturne-desktop --target x86_64-pc-windows-gnu` if the
   toolchain allows it; otherwise report what blocked it.
