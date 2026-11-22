# Trying Nocturne

For someone who has been handed a build and wants to see what it does. You
need no account, no server and no network changes: it runs on your machine,
keeps its data in one folder beside itself, and does nothing to a device
you have not given it.

## Which file

There are two editions and three platforms. Take one row.

| You are on | Take the file |
|---|---|
| **Windows** | `Nocturne_0.1.0_x64-setup.exe` |
| **macOS** | `Nocturne_0.1.0_universal.dmg` |
| **Linux (Debian/Ubuntu)** | `Nocturne_0.1.0_amd64.deb` |
| **Linux (Fedora/RHEL)** | `Nocturne-0.1.0-1.x86_64.rpm` |
| **Linux (anything)** | `Nocturne_0.1.0_amd64.AppImage` |

The ISP edition's files sit beside those and carry **Nocturne ISP** in the
name instead of Nocturne — same platforms, same extensions. (The exact
spelling is whatever the packager produces from the product name; the Linux
packages lower-case it. Match on the name rather than on a string written
here.)

On Windows there is also a `.msi` beside the `.exe`. They install the same
application; take the `.exe` unless your organisation deploys MSIs.

**Which edition?** The **free** one is the whole network-operations tool:
terminal, discovery, monitoring, configuration backup and compliance, safe
configuration changes, automation, topology, IPAM, a virtual lab and an AI
assistant. The **ISP** edition is all of that plus a subscriber ledger —
customers, tariffs priced per megabit, invoices, payments, arrears. Take the
free one unless you sell bandwidth to other people.

The two are separate applications and can be installed side by side.

There is also a `nocturne-server-*` binary for each edition and platform. That
is the same application without a window, served to your browser instead —
useful on a machine you reach over SSH, unnecessary otherwise.

## Installing it

The installers are **not signed yet**, so your computer will warn you. This
is expected for a build that has not been through a signing authority, and
it is the honest state of it rather than something to work around quietly.

* **Windows** — SmartScreen says "Windows protected your PC". Click **More
  info**, then **Run anyway**.
* **macOS** — Gatekeeper refuses the first open. Right-click the app in
  Applications and choose **Open**, then confirm. (Double-clicking will not
  offer that choice; the right-click menu does.)
* **Linux** — the `.deb` and `.rpm` install normally. For the AppImage,
  `chmod +x` it and run it.

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

`docs/STATUS.md` says what is built and how each part was checked, including
what has only ever been tested against a simulator. It is written to be read
sceptically.
