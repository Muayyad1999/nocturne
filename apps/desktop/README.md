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
(the default), `debug` or `trace`; `NOCTURNE_LOG` takes a full filter. An
argument it does not know, or an option without its value, is refused with
exit code 2 rather than ignored. `--version` says which edition it is:
`nocturne-isp 0.1.0 (ISP edition)`.

`cargo tauri dev` builds the interface first, waits for that, and then opens
the window. There is deliberately no Vite dev server behind the desktop
window: it always shows the interface that is built into it, so `cargo run`
is never a blank "connection refused" page. For interface work with hot
reload, run `nocturne-server` and open its URL in a browser — `ui/README.md`
explains the three ways to do that.

## What the shell does

* Starts the backend before the window exists, so the first call cannot race
  it, and shows the reason and exits if it cannot (a blank window would be
  worse). Without a display it says why in the terminal and in the log file
  and exits 1.
* `invoke('rpc', { method, params })` → the backend's result, or its error
  with the same `{code, message, details}` shape the server returns. A panic
  in a handler becomes an `internal` error rather than taking the window down.
* Every backend event reaches the windows as `nocturne://event`. If the
  interface falls behind, it is told how many it missed so it can refetch.
* A serious new alert raises a system notification while the window is not in
  front — at most one every ten seconds. Clicking it brings the window back.
* Closing the window quits, after giving running jobs up to three seconds to
  record how they ended. With **Settings → General → Keep running in the
  tray** on, closing only hides the window: a tray icon offers Show and Quit,
  and monitoring, receivers, schedules and notifications carry on. A second
  launch brings the first window forward.
* The WebView gets no filesystem, shell or HTTP plugin and cannot emit
  events. Everything goes through `rpc`, where the backend's own rules apply.
* Logs go to the terminal and to `logs/nocturne.log` in the data folder,
  moved along to `nocturne.1.log` … `nocturne.4.log` at 10 MiB each.

## Building installers

From this folder (`apps/desktop/src-tauri`), with the Tauri CLI
(`cargo install tauri-cli --version '^2'`, or `pnpm dlx @tauri-apps/cli@2`
in place of `cargo tauri`):

```bash
cargo tauri build
```

```bash
cargo tauri build --features isp --config tauri.isp.conf.json
```

The first is Nocturne, the second Nocturne ISP. A `--config` file is read
relative to the folder the command is run in; the interface build that runs
first is pinned to this folder (`beforeBuildCommand` has its own `cwd`), so
`cargo tauri build` and `cargo tauri dev` also work from the checkout's root.

Produces, for the platform you are on: `.deb`, `.rpm` and `.AppImage` on
Linux, `.msi` and an NSIS installer on Windows, `.dmg` and `.app` on macOS.
Cross-building is not configured; each platform builds its own. On Linux,
build against `libayatana-appindicator3-dev`, not `libappindicator3-dev`:
the CLI makes the packages depend on whichever of the two it finds (or on
Ayatana outright with `TAURI_LINUX_AYATANA_APPINDICATOR=true`, which the
release sets), and Debian 12 and 13 have only the Ayatana one.

Not set up, and deliberately so until the project decides: code signing with
a certificate (Windows Authenticode; an Apple Developer ID with
notarisation) and the Tauri updater. Both need secrets and a publishing
account. The macOS app carries an ad-hoc signature (`signingIdentity: "-"`),
which identifies nobody: it is what lets macOS 15 and later offer "Open
Anyway" for a downloaded copy instead of calling it damaged.
`docs/TRYING-IT.md` gives the steps a person takes on each platform.

## Making a release

1. Set the version in the root `Cargo.toml` (`[workspace.package]`). It is
   the only place: `tauri.conf.json` names none, so the installers' names,
   the packages' versions, `--version` and `app.info` all take this one.
2. Commit, tag `v<version>` and push the tag.

`.github/workflows/release.yml` then refuses a tag whose version the code
does not carry, makes a draft release (marked as a pre-release when the
version has a pre-release part, `0.2.0-alpha.3`), builds both editions on
the three platforms, and uploads only what it has checked:
`scripts/check-installers.sh` unpacks every installer and asks the program
inside for `--version`, so a file named for one edition that carries the
other — or the wrong version — fails the job instead. Each server binary is
started and asked for its methods, then uploaded as a `.tar.gz` (a `.zip` on
Windows) that keeps its execute bit. Publish the draft by hand once every
job has passed. A pre-release needs a number at the end of its pre-release
part (`-alpha.3`, `-rc.1`): Windows Installer takes numbers only, and is
given `0.2.0.3`.

## Icons

`icons/nocturne.svg` is the source — a crescent drawn as a constellation of
nodes and links. The rest of the directory is generated from it:

```bash
pnpm dlx @tauri-apps/cli@2 icon icons/nocturne.svg -o icons
```

## The relative paths in `tauri.conf.json` share one base

`build.frontendDist` is resolved relative to the folder holding
`tauri.conf.json` (`apps/desktop/src-tauri`), so it is `../../../ui/dist`.
The two hooks, `beforeBuildCommand` and `beforeDevCommand`, run in a folder
the CLI guesses unless they are given one: the folder it was started in if
that has a `package.json`, else the parent of the first `package.json`
within three levels below it, else `apps/desktop`. Started from the
checkout's root that guess is `ui/`, and `pnpm --dir ../../ui build` pointed
one level above the checkout — the same `ENOENT … lstat '…/ui'` the first
release attempt failed with when the paths were written for a different
base. So both hooks carry `"cwd": "."`, which the CLI takes relative to
`apps/desktop/src-tauri`, where it always is by the time a hook runs, and
their path is `../../../ui` like `frontendDist`'s.
