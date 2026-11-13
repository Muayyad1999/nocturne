# Spec: the screens

The foundation (`spec-ui.md`) gives the shell, the design system and the API
client. This document says what each screen is for and how it behaves. The
backend contract for a screen is the feature spec named in its heading
(`docs/dev/features/<name>.md`) and the TypeScript types in
`ui/src/api/types/<area>.ts`, written by the backend's author.

## Rules for every screen

1. **Real data only.** A screen renders what RPC returns. No sample rows, no
   hard-coded numbers, no "coming soon" tabs: if a backend method is missing
   (`RpcError.code === 'not_found'` with "unknown method") the affected part
   shows a quiet "not available in this build" state, and nothing else breaks.
2. **Three states, always**: loading (skeletons shaped like the content),
   empty (what this is for + the one action that fills it), error (the
   message + Retry). Never a blank panel, never an infinite spinner.
3. **One primary action per screen**, top right of the toolbar. Destructive
   and device-touching actions say exactly what will happen, to which
   devices, before it happens.
4. **Live by events**, not polling: subscribe to the topics the feature spec
   lists (`useRpc(..., {invalidateOn})`), and show long work through the
   jobs it starts (`jobs.changed`, `jobs.log`) with a cancel button.
5. **Lab is always labelled.** Anything from a simulated device carries the
   `LAB` pill; lab data is never mixed into production totals without saying so.
6. **AI is always labelled and never final.** Anything the assistant
   proposed is tinted `--ai` until a person accepts it.
7. **Keyboard**: every screen registers its main actions in the command
   palette; tables support arrow keys + Enter; `/` focuses the screen's
   search; `Esc` closes the top layer.
8. **Density with calm**: tables are compact (28px rows) and monospace where
   a device said it; everything around them breathes. Truncate with tooltips,
   never wrap table cells. Numbers through `lib/format.ts`.
9. A screen works at 1280×720 and uses the space at 2560×1440. The assistant
   dock may be open (−380px): nothing may break at 900px content width.
10. Each screen ships with a Playwright spec that drives it against the real
    backend (`nocturne-server` on a temp data dir with the demo lab loaded —
    helper in `ui/tests/support/backend.ts`), asserts the primary flow end to
    end, checks there are no console errors, and writes screenshots (both
    themes, 1440×900, deviceScaleFactor 2) to `ui/tests/shots/<screen>/`.

## Global pieces (shell additions)

* **First run**: when the inventory is empty the Overview is a welcome with
  three doors, one sentence each — *Explore the demo network* (creates the
  demo campus lab: in ten seconds the user has devices, a topology, traffic,
  alerts and something safe to configure), *Add a device*, *Discover my
  network*. No tour, no carousel.
* **Jobs**: the status bar shows running jobs (count + the newest one's
  progress); clicking opens a drawer with running and recent jobs, each with
  progress, log tail, result summary, Cancel. A finished job the user
  started produces a toast with its outcome and a link.
* **Notifications bell**: open alerts and failed jobs, newest first; badge =
  unseen count.
* **Vault**: when `vault.state` is `locked`, a blocking unlock dialog (the
  app is useless without credentials); wrong password says so and stays.
* **Host key changed** (`connect.host_key_changed`): a dialog that cannot be
  missed — host, both fingerprints, when the old one was first seen, and two
  buttons: *Keep refusing* (default) and *Trust the new key* (`hostkeys.trust`).
  Explains in one sentence when each is right.
* **Device picker** (shared component): search + filters + multi-select with
  "select all matching", used by every screen that targets devices.
* **Ask the assistant** (shared): a small `--ai` button on device headers,
  alerts, insights, selected terminal/config text, failed changes — opens the
  dock with that context attached.

## Overview — `ops.md` (`overview.get`)

A page to open the morning with. Top row: five `Stat` tiles (devices up /
total with a down count in `--crit` when > 0, open alerts by worst severity,
changes awaiting approval, backup coverage, compliance score) — each a link
to its screen; a tile whose source is unavailable is hidden, not zeroed.
Left column (2/3): **Needs attention** — one merged, ranked list: devices
down, critical/high alerts, failed changes, failed backups, top insights;
each row says what, where, since when, with the one obvious action
(Open, Acknowledge, Review). When the list is empty: a calm "Nothing needs
you right now" with the time of the last check. Right column: **Activity**
(merged audit/alerts/changes feed, live), **Running** jobs. Bottom: a small
topology thumbnail of the default view (click → Topology) when one exists.

## Devices — core `inventory`, `collect`, `connect`

List: a table (status, name, address, type, site, role, model, OS version,
uptime, tags; LAB pill) with a search box that filters as you type, facet
chips (site, role, tag, status, type) from `devices.facets`, and a Lab
toggle (show / hide / only). Selecting rows reveals a bulk bar: Open
terminals, Collect, Back up, Run command…, Edit (site/role/tags/credential/
monitoring), Delete. Primary action: **Add device** — a drawer with name,
address, connection (SSH/Telnet/serial/monitor-only), port, device type
(searchable; "Detect automatically" default), credential (pick or create
inline), jump host, site, role, tags, monitoring options, and a **Test
connection** button that shows the stage reached, the detected type and
hostname, and warnings (legacy crypto) inline. Secondary: Import (paste or
drop CSV / SSH config / Ansible inventory → preview table with new / update
/ invalid rows → commit) and Export.

Detail (`/devices/:id`): header with status, name, address, type, site, and
actions (Terminal, Collect, Back up, Configure…, Test, Ask assistant, ⋯).
Tabs: **Summary** (facts as KeyValue, health gauges with sparklines when
monitored, open alerts and insights for this device, neighbours table);
**Interfaces** (table: status, name, description, mode/VLAN, speed, in/out
rate with sparkline, errors; click a row → drawer with counters, chart,
neighbour, MACs seen); **Configuration** (latest running with section
outline, history timeline, diff between any two; Back up now);
**Changes** (changes touching this device); **Logs** (this device's syslog/
traps); **Data** (each collected kind as a table with its age and a
Collect button; never-collected kinds say so); **Notes**. Tabs whose
feature is absent are not shown.

## Terminal — `terminal.md`

Left: a slim session/device list (search devices, recent, open sessions with
state dots; quick connect field `user@host[:port]`, serial ports, local
shell). Centre: tabs of xterm.js sessions, splittable (drag a tab to an
edge; up to 4 panes), each with a thin header (title, LAB pill, transport
info on hover incl. legacy-crypto warning, log indicator, reconnect).
Toolbar: broadcast-input toggle (types into all selected panes, with a very
visible banner), snippets drawer (click to paste, never auto-enter),
find-in-scrollback, copy/paste, clear, toggle logging, SFTP panel (for SSH
devices: remote tree, upload/download with progress). Pasting more than one
line or a line the policy flags (`terminal.classify`) shows a confirm sheet
with the lines classified — *Paste anyway* is one keystroke away; this is a
seat belt, not a lock. Selecting text offers *Explain* and *Copy*. Sessions
survive navigation and reload (`terminal.list` → `terminal.attach`).
Closing a tab asks only when the session is alive. Empty state: "Connect to
a device" with the device search focused.

## Topology — `topology.md`

Full-bleed canvas on `Viewport` (pan, zoom, fit). Nodes: an icon per kind,
name, status ring; unmanaged neighbours dashed with a `+`; lab nodes with
the LAB tint. Links: straight or gently curved, width by speed, colour by
utilisation when known (legend), dashed for `inferred`, with interface
labels appearing when zoomed in. Toolbar: view selector (save/rename),
layer (Physical / Layer 3), filters, Endpoints toggle, Auto-layout,
Refresh data (collects neighbours for the devices in view, as a job),
Export (SVG, draw.io, Mermaid, PNG of the canvas), and a search box that
accepts a device name, IP or MAC — an address runs `locate.host` and
highlights the switch, port and path. **Path mode**: pick two nodes → the
path is highlighted hop by hop with interfaces, and where data ran out the
path ends in a labelled gap. Click a node → side panel (summary, health,
neighbours, actions: Terminal, Open device, Ask assistant); click a link →
both ends, speed, utilisation chart, VLANs. Drag to arrange; positions save
per view; pinned nodes survive auto-layout. Stale data shows as a quiet
banner with the age and a Refresh. Empty: "No topology yet — collect
neighbour data from your devices" + the button.

## Monitoring — `monitor.md`, `logs.md`, `insights.md`

Tabs: **Alerts** (default: open alerts table with severity, device, title,
since, count; row actions Acknowledge / Snooze / Resolve; filters; details
drawer with the metric chart around the alert and related logs) ·
**Health** (device status table with RTT sparkline, CPU, memory, last seen;
problem-first sort) · **Interfaces** (network-wide interface table:
utilisation, errors, state; "Top talkers" and "Errors" quick filters) ·
**Charts** (metric explorer: pick devices/metrics/instances, time range,
overlay; uPlot with gaps shown as gaps) · **Logs** (query bar, severity and
stream filters, histogram, virtualised list, Live tail toggle that sticks
to the bottom until the user scrolls; a setup hint with the exact device
command when receivers are off or nothing has arrived) · **Insights**
(grouped by severity then category; each card: title, device, evidence
table, why it matters, suggested fix with *Create change* when there is
one, Dismiss; a coverage line so "no findings" is distinguishable from "not
checked") · **Rules** (alert rules, log rules, maintenance windows,
notification channels with Test).

## Configuration — `config.md`

Tabs: **Backups** (coverage table: device, last backup, age, versions,
last change, unsaved-changes flag; primary action *Back up now* for the
selection or all; rows open the device's history) · **History** (pick a
device → version timeline on the left, CodeMirror on the right; select two
versions → unified or side-by-side diff with section headers kept in view;
*Restore this version…* creates a change) · **Search** (one big search
field, regex toggle, scope filters; results grouped by device with the
section path and context lines; the query is in the URL so it can be
shared) · **Compliance** (score and trend, policies list with enable
toggles and an editor for custom rules with live test against a device's
config, findings table filterable by severity/device/rule with evidence
lines; select findings → *Fix…* creates a change) · **Intended** (per
device: intended config editor, drift preview, *Fix drift…*). Secrets are
redacted everywhere; revealing asks once per session and says it is audited.

## Changes — `changes.md`

The screen where trust is won. List: changes with state, title, devices,
risk, origin (an `--ai` chip for assistant-drafted), created, ticket;
filters by state. Primary action **New change**: step 1 pick devices; step 2
say what to do — *Lines* (a CodeMirror box; what you type is what is sent),
*Intended* (paste the configuration or a section as it should be; exact or
additive), or *From template*; step 3 **Plan** (runs as a job) and land on
the change page.

Change page: header (title, state, risk badge, ticket, origin); per device
an expandable card: **commands** exactly as they will be sent (numbered,
monospace), **predicted diff**, **risk** with its reasons in plain
sentences, **protected hits** as red callouts each explaining why it
matters, the **dead-man switch** that will be armed (or a warning that the
platform has none), **rollback** commands. Approve bar at the bottom: a
checkbox per device with protected hits requires typing the device name;
*Approve & apply* is disabled until every plan is expanded at least once or
the user ticks "I have reviewed the plans" — and it names the count of
devices. While applying: a live stepper per device (connect, drift check,
backup, arm, apply, verify, save) with the lines scrolling as the device
answers; on failure the card shows what was rejected, that it was rolled
back (or exactly what remains), and the transcript. Afterwards: verification
results, before/after diff, *Roll back…*, *Export as Ansible playbook*.
A stale plan (config drifted) says so and offers *Plan again*.

## Automation — `automation.md`

Tabs: **Run** (device picker + a command box, one per line; non-read-only
commands are refused with the reason before anything runs; results as a
device × command grid with a *Compare* view — "47 devices say this, 3
differ" with diffs — search, export, parsed-table view when available) ·
**Templates** (list with built-ins; editor: body on the left, variables
form in the middle, live render for a chosen device on the right with
errors inline; *Apply to devices…* → a change) · **Playbooks** (YAML editor
with validation markers, run against a selection, per-device step results) ·
**Schedules** (list with next run; editor with a human-readable preview of
the next five runs) · **Ansible** (detected or not; export inventory /
playbook; run a playbook in check mode with streamed output).

## Lab — `lab.md`, `spec-design.md`

Lab list: cards (name, node count, running state) + an examples gallery
("Start from: demo campus, two-router OSPF, VLANs and trunks, …") and
**New lab** / **Design a network…**. Lab editor (`/lab/:id`): a canvas on
`Viewport`. Left palette of models from `labs.catalog` (drag onto the
canvas); **cable tool**: click a node → choose a free port → click the other
node → choose a port (ports listed with their state; the common case — first
free suitable port — is one click each). Nodes show name, model icon and a
power state; links show port labels and **link lights** (green/amber/red
from `labs.tick`) and, when traffic flows, an animated utilisation tint.
Node context menu: Console (opens a Terminal tab), Configuration, Tables
(routes, ARP, MAC, VLANs, STP, neighbours as tabs in a drawer), Power off/on,
Rename, Delete. Toolbar: Run/Pause, speed, Reset, Connectivity check (matrix
dialog: who reaches whom, with the failing hop), Faults, Traffic flows,
Export/Import, *Open in Topology*. **Simulation mode** (toggle): capture on;
"Send test packet" from A to B (ICMP/TCP/UDP); the event list on the right
(time, at, protocol, summary) with step / play / speed; selecting an event
highlights the hop on the canvas and shows the OSI-layer breakdown; a dropped
packet shows where and why. **Design wizard**: a short requirements form
(sites with users, voice, guest Wi-Fi, servers; redundancy; WAN; base
block; routing) with plain-language help → the design: decisions list,
VLAN/addressing tables, device list, warnings → *Create lab* → the lab
opens, runs its checks and shows the result; *Export plan* (Markdown) and
configs.

## IPAM — `ipam.md`

Left: prefix tree (indentation = containment) with utilisation bars, site
filter, search (an address jumps to its prefix). Right: the selected prefix
— header (CIDR, name, VLAN, gateway, utilisation), the **address grid** for
/24 and smaller (16 columns; free / active / reserved / DHCP / conflict
colours with a legend; click → details and edit; drag-select to reserve a
range), a table for larger ones, free ranges, devices with interfaces in it.
Actions: Add prefix, Split / VLSM…, Next free address (copies it), Reserve,
Import, Export. Tabs for **VLANs** (id, name, site, where it exists, name
mismatches flagged) and **Conflicts**. Lab prefixes under a separate, labelled root.

## Tools — `toolbox.md`

Left: tool list from `tools.list`, grouped (Addressing, Probes, Decoders,
Converters, Reference, Servers) with a filter box. Right: the tool. Every
calculator is a small form that answers as you type (debounced RPC), results
in a KeyValue block with copy buttons and the binary/hex views network
people expect. Probes show live progress: ping as a rolling RTT chart with
loss marked and the summary line; traceroute as a hop table filling in;
port scan as a grid; each with Stop and a "from this computer / from a
device" switch where the backend supports it. Results are plain text
copyable. The last inputs per tool are remembered locally.

## Operations — `ops.md`

Tabs: **Incidents** (list; an incident page with the timeline as its
centrepiece — notes, linked alerts and changes at their real times — an
"add note" box always at hand, suggested links, state buttons, *Export
review*) · **Handover** (pick the shift start → the generated summary by
section with every item linked; a notes box; Save; Export Markdown; *Ask
the assistant to tidy my notes*) · **Reports** (kinds available, period,
filter, format; preview; Save / Download / Print) · **Notes** (Markdown
runbooks with device links) · **Audit** (who/what/when, filterable, links
to the objects) · **Jobs** (full history).

## Assistant — `assistant.md`

The dock (and the same component full-page at `/assistant` with the chat
list). Transcript: user messages plain; assistant text streamed; **tool
steps** as compact collapsible chips ("Ran `show ip route` on CORE-SW1 ·
0.8 s") that expand to input and result summaries and link to the object;
thinking summaries collapsed by default; **change cards** when the
assistant proposed a change — devices, risk, a peek at the diff, and
*Review & approve* (opens the change page) — the assistant's side of the
card says what it did and what it is waiting for. Composer: multi-line, Enter
sends, context chips above it (current device/page/selection; removable),
quick actions from `ai.presets` relevant to the current context, a Stop
button while a turn runs, model/provider picker. No provider configured →
a setup card: *Use a local model* (detected Ollama/LM Studio models, one
click) or *Add an API key* (Claude, OpenAI-compatible). Approval prompts
(`approval_needed`) appear inline with Allow / Deny. Errors appear as a
message with what to do, never as a dead spinner.

## Settings — core + each feature's settings section

Sections: **General** (theme, density, time format) · **Security** (vault
state and master password; known SSH host keys with forget; host-key
policy) · **Credentials** (list; editor that never shows a stored secret —
only "set / not set" with replace and clear) · **Device types** (the profile
list with maturity badges; a YAML editor with validation as you type, the
resolved view, and *Test against pasted output* per collector; duplicate a
built-in to customise) · **Connections** · **Monitoring** · **Logs &
traps** (receivers with status and the device-side lines to copy) ·
**Assistant** (providers with Test, autonomy with a plain description of
each level, extra instructions) · **Notifications** · **About** (version,
data directory, licences). Every setting has one sentence of help; changes
save on blur with a quiet confirmation; dangerous ones (host-key policy
`accept`, autonomy above `draft`) ask once and say what they mean.
