# Trying Nocturne

For someone who has been handed a build and wants to see what it does. You
need no account, no server and no network changes: it runs on your machine,
keeps its data in one folder of your user profile (below, "Where your data
is"), and does nothing to a device you have not given it.

## Which file

The files are on the release page. There are two editions and three
platforms; take one row. `<version>` is the release's version, the same in
every file name — for the release tagged `v0.1.0` it is `0.1.0`, and
`Nocturne_0.1.0_x64-setup.exe` is the Windows file.

| You are on | Nocturne (free) | Nocturne ISP |
|---|---|---|
| **Windows** | `Nocturne_<version>_x64-setup.exe` | `Nocturne.ISP_<version>_x64-setup.exe` |
| **macOS** (Intel or Apple silicon) | `Nocturne_<version>_universal.dmg` | `Nocturne.ISP_<version>_universal.dmg` |
| **Linux (Debian 12 or later, Ubuntu 22.04 or later)** | `Nocturne_<version>_amd64.deb` | `Nocturne.ISP_<version>_amd64.deb` |
| **Linux (Fedora/RHEL)** | `Nocturne-<version>-1.x86_64.rpm` | `Nocturne.ISP-<version>-1.x86_64.rpm` |
| **Linux (anything)** | `Nocturne_<version>_amd64.AppImage` | `Nocturne.ISP_<version>_amd64.AppImage` |

The ISP edition's product name is "Nocturne ISP"; GitHub turns the space in
a file name into a dot, which is why its files begin `Nocturne.ISP`. Once
installed, its program is `nocturne-isp` and its Linux package is
`nocturne-isp`; the free edition's are both `nocturne`.

On Windows there is also a `.msi` beside the `.exe`
(`Nocturne_<version>_x64_en-US.msi`). They install the same application;
take the `.exe` unless your organisation deploys MSIs. The
`….app.tar.gz` files are the macOS app without the disk image around it;
the `.dmg` is easier.

**Which edition?** The **free** one is the whole network-operations tool:
terminal, discovery, monitoring, configuration backup and compliance, safe
configuration changes, automation, topology, IPAM, a virtual lab and an AI
assistant. The **ISP** edition is all of that plus a subscriber ledger —
customers, tariffs priced per megabit, invoices, payments, arrears. Take the
free one unless you sell bandwidth to other people.

The two are separate applications and can be installed side by side, but
they open the **same** data — one inventory, whichever of them you start —
and only one of them can be open at a time. Start the second while the first
is open and it says it cannot start because Nocturne is already running with
that data folder; close the first. `nocturne-isp --version` (or
`nocturne --version`) says which edition and version you have.

### Without a window: the server

There is also a server for each edition and platform: the same application
without a window, used from a browser — useful on a machine you reach over
SSH, unnecessary otherwise. It is on the same release page:

| You are on | Free | ISP |
|---|---|---|
| **Linux** (x86-64) | `nocturne-server-free-linux-x64.tar.gz` | `nocturne-server-isp-linux-x64.tar.gz` |
| **macOS** (Intel or Apple silicon) | `nocturne-server-free-macos-universal.tar.gz` | `nocturne-server-isp-macos-universal.tar.gz` |
| **Windows** | `nocturne-server-free-windows-x64.zip` | `nocturne-server-isp-windows-x64.zip` |

Unpack it and start the program in the folder it makes:

```bash
tar -xzf nocturne-server-free-linux-x64.tar.gz
./nocturne-server-free-linux-x64/nocturne-server
```

On Windows, extract the zip and run `nocturne-server.exe` from a terminal
in that folder. It prints `Nocturne is listening on
http://127.0.0.1:7878/#token=…`; open that whole address, token included, in
a browser on the same machine. From another machine, forward the port first
(`ssh -L 7878:127.0.0.1:7878 you@that-machine`) and open the same address on
yours. It uses the same data folder as the desktop application, so the two
cannot run at the same time either. On macOS, if it is refused because it
was downloaded, `xattr -d com.apple.quarantine nocturne-server` lets it run.

## Installing it

Nothing here is signed by a certificate authority: there is no Windows
code-signing certificate and no Apple Developer ID, and the macOS app is not
notarised. Your computer will warn you. This is the honest state of a build
nobody has paid to have vouched for, not something to work around quietly.

* **Windows** — SmartScreen says "Windows protected your PC". Click **More
  info**, then **Run anyway**.
* **macOS** — open the `.dmg` and drag Nocturne into Applications. The app
  carries only an ad-hoc signature, which says nothing about who made it, so
  the first time you open it macOS refuses ("Apple could not verify
  'Nocturne' is free of malware…"). Click **Done** (**OK** on older
  versions), not Move to Trash.
  * **macOS 15 (Sequoia) and later**: open **System Settings → Privacy &
    Security**, scroll to the message about Nocturne near the bottom, click
    **Open Anyway** and confirm with your password, then open Nocturne
    again and choose **Open**. (Right-clicking the app and choosing Open no
    longer gets past the warning on these versions.)
  * **macOS 14 and earlier**: right-click (Control-click) the app in
    Applications, choose **Open**, then **Open** again.
  * If macOS says the app **is damaged and can't be opened**, neither of
    those helps; in Terminal run
    `xattr -dr com.apple.quarantine /Applications/Nocturne.app`
    (`"/Applications/Nocturne ISP.app"` for the ISP edition) and open it
    again.
* **Linux** — `sudo apt install ./Nocturne_<version>_amd64.deb` (it pulls in
  the WebView and `libayatana-appindicator3-1`, which draws the tray icon),
  or `sudo dnf install ./Nocturne-<version>-1.x86_64.rpm`. For the
  AppImage, `chmod +x` it and run it; if it says it cannot load
  `libfuse.so.2`, install `libfuse2t64` (Ubuntu 24.04) or `libfuse2`
  (Debian 12), or start it with `--appimage-extract-and-run`.

## The first ten minutes

Launch it. There is no sign-in and nothing to configure.

**Go to Lab and create one from an example.** The demo campus is the most
interesting; "basic switching" is the easiest to follow. This is the point
of the first ten minutes, and here is why: a simulated device in Nocturne is
an ordinary inventory device with `transport: sim`, and **every other part of
the application treats it exactly as hardware**. So the moment a lab is
running:

* **Devices** lists them with what they actually reported.
* **Terminal** gives you a console. Type `show ip interface brief`.
* **Monitoring** polls them and draws real interface rates.
* **Topology** finds the links and draws the map, with load on the lines.
* **Config** backs them up and shows you what changed between versions.
* **Changes** will plan an edit, show you the predicted configuration and the
  commands, and refuse to apply anything until you approve *that exact plan*.
* **Assistant** can use all of it, if you put an API key in Settings. It can
  draft and plan a change; whether one may be applied is decided by the
  change engine, never by the model.

You can break a lab freely. Nothing there touches anything real.

**Closing the window quits**, and monitoring stops with it. To keep
monitoring, log receivers and schedules running with the window closed, turn
on **Settings → General → Keep running in the tray**: closing then hides the
window, a Nocturne icon stays in the tray (the menu bar on macOS) with
**Show** and **Quit**, and serious new alerts still arrive as notifications —
clicking one brings the window back. Starting Nocturne again also brings the
window back. On GNOME the tray needs the AppIndicator extension (Ubuntu has
it; on Debian and Fedora install "AppIndicator and KStatusNotifierItem
Support"); without it there is no icon, the window still hides, and starting
Nocturne again is the way back.

## Pointing it at something real

**Devices → add a device.** It needs an address and a credential; the
credential is encrypted at rest and never appears in a log, an event, a job
output or anything a model is shown.

Two things worth knowing before you do:

* **It is read-only until you say otherwise.** Reads go through a policy that
  only sends a command the device's profile classifies as read-only —
  default deny. Configuration changes go through the change pipeline, which
  needs a human to approve the exact plan.
* **Interface counters need SNMP.** Without it, Nocturne reads interfaces over
  the command line every five minutes, and the numbers on the Devices screen
  are a snapshot from the last collection rather than a live figure — the
  screen says which, above the table. Tick **Interface counters and health
  over SNMP** on the device and give it a community or a v3 user, and it
  polls every sixty seconds.

## Where your data is, and removing it

Everything Nocturne knows — the inventory, the encrypted credentials, the key
that decrypts them (`vault.key`, kept beside the database unless you set a
master password), configuration backups and the log files — is in one
folder, shared by both editions and the server:

| | Folder |
|---|---|
| **Linux** | `~/.local/share/Nocturne` (or `$XDG_DATA_HOME/Nocturne`) |
| **macOS** | `~/Library/Application Support/Nocturne` |
| **Windows** | `%APPDATA%\Nocturne` |

`--data-dir <folder>` or the `NOCTURNE_DATA_DIR` environment variable
starts it on another folder instead. The window's size and position and the
WebView's own storage are kept apart, in folders named after the edition's
identifier (`app.nocturne.desktop`, `app.nocturne.desktop.isp`): under
`~/.local/share` and `~/.config` on Linux, under `~/Library` (Application
Support, Caches, WebKit) on macOS, and under `%APPDATA%` and
`%LOCALAPPDATA%` on Windows.

**Uninstalling leaves the `Nocturne` folder where it is**, on every
platform. The Windows uninstaller's "Delete the application data" box
removes only the identifier folders, because the `Nocturne` folder may still
be the other edition's or the server's; ticked, it tells you where the
folder is. If nothing else uses it, delete it yourself once Nocturne is
closed; that removes the inventory, the credentials and the key with them.

## What is worth reporting back

This has been run by very few people. The useful things to say are:

1. **Anything that shows an error card.** A screen saying "This screen ran
   into a problem" is a bug, not you. Say which screen and what the card
   said — the text in it is the diagnosis.
2. **Anything that claims to know something it cannot.** The one rule this
   project holds above the others is that a read that failed is reported as
   a failure and never filled in with a plausible number. If you see a zero
   where it should say "nobody looked", that is the most serious kind of bug
   here.
3. **Anywhere it is slow, confusing, or asks you something it should have
   worked out.**
4. **What you wanted it to do and could not find.**

With a report, say what `nocturne --version` (`nocturne-isp --version`)
printed, and send `logs/nocturne.log` from the data folder above if
something went wrong; nothing in it is a password or a configuration.

`docs/STATUS.md` says what is built and how each part was checked, including
what has only ever been tested against a simulator. It is written to be read
sceptically.
