# The Nocturne desktop application

A Tauri 2 shell around the same backend the server runs. It starts
`nocturne-suite` in its own process, shows the interface in the system
WebView, and connects the two with one command and one event channel.

There is no product logic here. If something can be done in the desktop app
it can be done through `nocturne-server` as well, because both speak to the
same `App::rpc`.

## Running it

```bash
pnpm --dir ui build
```

```bash
cargo run -p nocturne-desktop
```

`--data-dir <folder>` or `NOCTURNE_DATA_DIR` chooses where the database,
vault key and logs live; the default is the platform's data folder plus
`Nocturne` — the same place `nocturne-server` uses, which is why only one of
them may run on it at a time. `--log-level` takes `error`, `warn`, `info`
(the default), `debug` or `trace`; `NOCTURNE_LOG` takes a full filter.

`cargo tauri dev` rebuilds the interface first and then opens the window.
There is deliberately no Vite dev server behind the desktop window: it always
shows the interface that is built into it, so `cargo run` is never a blank
"connection refused" page. For interface work with hot reload, run
`nocturne-server` and open its URL in a browser — `ui/README.md` explains the
three ways to do that.

## What the shell does

* Starts the backend before the window exists, so the first call cannot race
  it, and shows the reason and exits if it cannot (a blank window would be
  worse).
* `invoke('rpc', { method, params })` → the backend's result, or its error
  with the same `{code, message, details}` shape the server returns. A panic
  in a handler becomes an `internal` error rather than taking the window down.
* Every backend event reaches the windows as `nocturne://event`. If the
  interface falls behind, it is told how many it missed so it can refetch.
* A serious new alert raises a system notification while the window is not in
  front — at most one every ten seconds.
* Closing the window stops the backend; a second launch focuses the first.
* The WebView gets no filesystem, shell or HTTP plugin. Everything goes
  through `rpc`, where the backend's own rules apply.

## Building installers

```bash
cargo tauri build
```

Produces, for the platform you are on: `.deb`, `.rpm` and `.AppImage` on
Linux, `.msi` and an NSIS installer on Windows, `.dmg` and `.app` on macOS.
Cross-building is not configured; each platform builds its own.

Not set up yet, and deliberately so until the project decides: code signing
(Windows Authenticode, Apple Developer ID plus notarisation) and the Tauri
updater. Both need secrets and a publishing account.

## Icons

`icons/nocturne.svg` is the source — a crescent drawn as a constellation of
nodes and links. The rest of the directory is generated from it:

```bash
pnpm dlx @tauri-apps/cli@2 icon icons/nocturne.svg -o icons
```
