# IPAM screen — working notes

Specs: `docs/dev/spec-ui.md`, `docs/dev/spec-ui-pages.md` (the IPAM brief),
`docs/dev/features/ipam.md`. Backend contract: `crates/nocturne-ipam/src/api.rs`
and `ui/src/api/types/ipam.ts`; the backend author's own notes are in
`crates/nocturne-ipam/NOTES.md`.

```
index.tsx            the screen: the gate, the toolbar, four tabs, the dialogs
data.ts              every read and write, with `ipam.changed` as the one topic
tree.ts              namespaces, the containment tree, utilisation, provenance   (+ tree.test.ts)
addresses.ts         the four states of an address, the words, the basis         (+ addresses.test.ts)
reconcile.ts         coverage, "nobody looked" vs "not there", the sections      (+ reconcile.test.ts)
vlans.ts             record vs reported name, and the epoch-zero trap            (+ vlans.test.ts)
PrefixTree.tsx       the tree, with roots per namespace
PrefixPanel.tsx      one block: standing, utilisation, grid, free runs, interfaces
AddressGrid.tsx      the grid, its legend, and the table for a block too large
AddressDetail.tsx    one address: the record, the sightings, the disagreements
Vlans.tsx            the VLAN table with name mismatches
Conflicts.tsx        findings with their evidence, and no resolve button
Reconcile.tsx        coverage first, then the refusals, then what was found
PrefixDialog.tsx · AddressDialog.tsx · VlanDialog.tsx · SplitDialog.tsx
FreeSpaceDialog.tsx · TransferDialog.tsx · LearnDialog.tsx · SearchPanel.tsx
```

Gates, as run: `pnpm --dir ui test` (all suites), `pnpm --dir ui build`
(typecheck of the whole repo, then the bundle), `ui/tests/ipam.spec.ts`
(16 tests against a real `nocturne-server` with the demo campus collected
from, each asserting a clean console) and `ui/tests/ipam.shots.ts`
(20 screenshots per theme, 1440×900 and 1280×720, with and without the
assistant dock).

## What this screen is built to refuse to do

* **Never merge the plan with the network.** `source: discovered` is a word on
  the row, not a shade of a colour, and a discovered block shows the interface
  it was inferred from in its panel. The address grid has four states, not two.
  A VLAN row says whether it has a record at all.
* **Never call an address free.** The word "available" appears nowhere on the
  screen (the spec asserts that against the rendered page); the empty state in
  the grid reads "No record, nothing seen" and is the quietest fill there,
  beside `ipam.prefix.get`'s own `note` and the coverage of that block.
* **Never resolve a conflict the engine refuses to resolve.** The reconciliation
  tab prints `refused` verbatim, and the spec asserts that no button on the
  screen is called resolve, fix, apply or accept. The one write that turns
  evidence into records is `ipam.learn` — previewed, itemised, and offered as a
  decision a person makes, which is exactly the act the refusals describe
  ("which one is a decision, so it is only reported").
* **Never show a time that did not happen.** `last_seen`, `documented_at` and a
  VLAN's `updated_at` are 0 or null for things that never happened; those read
  as "never" / "no record", not as 1970.

## Deliberately not called

* **`ipam.address.get`** — the drawer is opened from a block whose
  `ipam.prefix.get` already carries the composed address with its sightings and
  disagreements, so a second read would say the same thing a moment later. It
  was called for a while to work around the search defect below; since that
  was fixed in the engine the workaround is gone, and so is the call. It is
  the call a future "jump straight to an address" route would want.
* **`ipam.prefix.get {addresses: false}`** — the panel always wants the
  addresses. The cheap form exists for callers that only want the row.
* **`ipam.prefixes {parent_id, flat}`** — the whole tree is two queries in the
  backend and 43 ms for 201 rows (the crate's own measurement), so the screen
  reads it whole and does its own collapsing. `tree.ts` still copes with a row
  whose parent is missing, because a `flat` or site-filtered answer can contain
  one, and dropping it would hide a block that exists.
* **`ipam.sync {device_ids}`** — the button re-reads everything; per-device sync
  belongs on a device page, not here.
* **`ipam.export {format}` / `ipam.import {format}`** other than `csv` — the
  backend refuses anything else, so the format is not offered as a choice.
* **A nav-rail badge for conflicts.** `spec-ui.md` lists badges for Monitoring
  and Changes only; the count is in the status bar and on the Conflicts tab
  instead.

## What the backend or the design system made awkward

1. **`ipam.next_address`'s `basis` carries a raw epoch.**
   `crates/nocturne-ipam/src/addresses.rs:477` formats
   `"… last collected at {at} …"` with `at` in milliseconds since the epoch, so
   the sentence a screen is told to show reads "last collected at
   1794614077077". `addresses.ts::humaniseBasis` rewrites any 13-digit stamp
   into a readable time and keeps the claim word for word; the spec asserts no
   13-digit number reaches the page. The same wart is in the `conflict` message
   `addresses::reserve` returns ("collected at 1794614168771"), which this
   screen shows as the backend wrote it, because rewriting an error message
   felt worse than showing it. Both would be better fixed in the crate, where
   the time is still a number. (`sighting(s)` in the same sentence is the
   backend's own plural, and is left alone for the same reason.)
2. **A `device` disagreement carries ids on both sides.**
   `crates/nocturne-ipam/src/addresses.rs:104-107` sets
   `documented`/`observed` to device **ids** for `field: "device"`, while the
   row's own `device` field is the observed device's *name*. The same is true
   of `Mismatch` in the reconciliation report (`sync.rs`, same fields). So the
   screen reads `devices.list` only to name the side of a disagreement the
   backend already had a name for, and shows the id when that read is
   unavailable (`addresses.ts::disagreementText` and
   `reconcile.ts::groupMismatches`, both tested either way). A
   `documented_device` name beside the id would remove the extra read.
3. **~~Partial-IP search mistook an address for a MAC~~ — fixed in the
   engine (`db.rs`, commit c5fe67c).** It used to turn any query of ≥ 6 hex
   digits into its "bare MAC" form, so `10.10.10` matched nothing while
   `10.10.1` found seven addresses, and an address documented in a lab could
   not be found at all. `looks_like_a_mac` now decides by separators, and
   `escape_like` escapes `%`/`_` rather than deleting them. The screen's
   workaround (re-reading the typed address through `ipam.address.get`) is
   gone with it.

   What remains is not a defect but a shape worth knowing: `api.rs` still adds
   a **production-namespace row for a whole typed address**, known or not, on
   top of what the search found. So one typed address can arrive twice — once
   with its record and sightings in a lab, once empty in production.
   `addresses.ts::addressHits` keys hits by address *and* namespace, drops the
   empty row when something knows the address elsewhere, keeps it when nothing
   knows the address anywhere (that is the answer to the search), and lists
   two namespaces that both know it as two labelled hits. `addressScope` reads
   a row's namespace from its sighting, then from the block it falls in; both
   are unit-tested, and the spec pins all three cases.
4. **An address row's `source` is `manual` when there is no record at all.**
   `addresses.rs:88-96` falls back to `AddressSource::Manual` for a grid cell
   nothing is known about, so `source` cannot be used to tell a record from a
   sighting. The screen uses `documented` / `observed` / `observations` only,
   which is what the type's documentation says to do, but the field is a trap
   worth a comment in the crate.
5. **`usable` and `FreeRange.count` are JSON numbers.** For a /64 they arrive as
   `18446744073709551615`, which JavaScript has already rounded to
   `18446744073709552000`. Every figure on screen comes from `usable_text` and
   `count_text`; `tree.ts::utilisation` returns a different *shape* for a block
   with `util_known: false` so a caller cannot format the lossy number by
   accident (`tree.test.ts` asserts the shape has no `pct` at all).
6. **A VLAN that is only observed has `created_at`/`updated_at` of `0`.**
   Rendered through `TimeAgo` that is "56 years ago" — the exact failure the
   brief warns about. `vlans.ts::recordAge` returns `null` for it and the column
   reads "no record"; `vlans.test.ts` pins that, and the spec asserts the string
   "56 years ago" is nowhere in the VLAN table.
7. **Two syncs at once duplicate a discovered block.** `sync.rs:170-206`
   reads "which blocks cover these addresses" and then inserts the missing
   ones with a fresh id, with nothing between the read and the write. Calling
   `ipam.sync` while the debounced watcher is running the same sync (it fires
   1.5 s after `collect.updated`) makes both insert, and the estate then holds
   two `discovered` prefixes for one CIDR in one namespace — which the next
   `ipam.prefix.save` refuses as a duplicate of itself. Hit while seeding the
   spec; both seeds now wait for the automatic sync instead of racing it
   (`waitForSync` in `ui/tests/ipam.spec.ts`). A uniqueness check on
   `(scope, cidr)` at insert, or a lock around the discovery step, would fix
   it in the crate.
8. **`ipam.learn`'s preview reports `created_*: 0`.** Those fields count what
   was *written*, so a dry run says "0 blocks, 0 addresses, 0 VLANs" while its
   own rows say `create` 33 times. The dialog counts the rows itself in a
   preview and uses `created_*` only after the commit; a `would_create_*`
   trio, or filling `created_*` in a dry run, would be clearer.
9. **The `overlap` conflict kind looks unreachable.** `conflicts.rs:201-214`
   raises it only when two blocks overlap *without* either containing the other,
   which canonical CIDR blocks cannot do (and `prefix.save` canonicalises, so a
   stored block always is one). The screen handles the kind because the contract
   lists it; no seed could produce one to photograph.
10. **`ipam.learn` is scoped to what a sync can see, so its preview can be
   empty** even when reconciliation lists undocumented addresses. The dialog
   says so in a sentence rather than showing an empty table.
11. **The design system's `Tree` is one label and one trailing slot per row**
   (`design/Tree`), and this screen needs five columns and a utilisation bar.
   `PrefixTree.tsx` draws its own `role="tree"` and keeps `Tree`'s keyboard
   contract (up/down walk the visible rows, right/left open and close, Enter
   selects). A `Tree` that took a row renderer, or a `Table` that took a depth,
   would have saved it; that is a design-system change and outside this slice.
12. **`Panel` remembers its collapsed state per `storageId` in `localStorage`,**
    which is right for a person and surprising in a test: a section that found
    something starts open, so a test must not click its title to "open" it. The
    spec says so where it would otherwise look odd.
13. **A radio in the design system cannot be `check()`ed by Playwright.**
    `RadioGroup` draws its own dot over a transparent `<input>`, so the input
    never receives the click and `locator.check()` waits for ever — with no
    action timeout configured, that hangs a whole run rather than failing it.
    The screenshot script presses the label instead, as a person does, and
    says so where it does it. Worth either a `pointer-events: none` on the
    decorative dot or an `actionTimeout` in `playwright.config.ts`; both are
    outside this slice.
14. **No status-bar or nav badge API for a count that belongs to a screen that
    is not mounted.** The conflict count is only in the status bar while IPAM is
    open, like Monitoring's alert count. Noted in `pages/overview/NOTES.md` too.

## Things I wanted from outside my paths

* The crate defects above. `db.rs` (search) is **fixed** (c5fe67c). Still
  open, and still worked around here — re-checked against the rebuilt engine:
  `addresses.rs:477` still formats `basis` with a raw epoch, so
  `humaniseBasis` is still needed; `addresses.rs:104-107` still puts device
  **ids** on both sides of a `device` disagreement (and `Mismatch` with it),
  so the `devices.list` lookup is still needed. Also open: `sync.rs:170`
  (two syncs duplicate a discovered block) and `created_*` reading zero in a
  learn preview.
* `api/types/ipam.ts` needed no change: every shape matched what the running
  engine sent, including the fields the feature spec does not mention
  (`usable_text`, `util_known`, `child_coverage_pct`, `coverage`, `note`,
  `basis`, `refused`). The one thing it under-documents is that
  `AddressInput.ip` is optional in the Rust struct (`addresses.rs:486`) while
  the TypeScript marks it required — which is the safer direction, so it was
  left alone.
* A `useHasMethod` hook already exists in `api/hooks.ts`; three page folders
  still carry their own copy (`pages/devices/methods.ts`,
  `pages/changes/methods.ts`, `pages/config/methods.ts`). This screen uses the
  shared one.

## What looking at the screenshots changed

Twenty screens per theme, read one by one. What they showed, and what was done:

1. **Block names were clipped to one character** in the tree ("10.20.0.0/16
   H…"): four columns did not fit the 520px pane. The pane now starts at
   600px and the fixed columns are narrower; the name is the thing that is
   kept.
2. **"no interface seen" and "0 in use · no percentage" were both cut off.**
   The first is now "none seen"; the second is a figure and a dash in the
   percentage slot, because a block whose fullness cannot be known should not
   be given a sentence where every other row has a number.
3. **The same MAC disagreement was listed three times** — once per sighting —
   which read as three faults, in the address drawer and again in the
   reconciliation report, where the report also printed raw device ids
   ("written j6zhwiwgv62jejwc on the network sscp4qkq4xzlr45b"). Both lists
   now group by written-versus-seen pair with every sighting underneath, and
   both name the devices (`groupDisagreements`, `groupMismatches`).
4. **An address that is documented, seen *and* contradicted said "the network
   shows it where it should be."** That is the one case where the general
   sentence is false; `stateMeaning` replaces it.
5. **The documented DHCP range and gateway were invisible in the grid**, even
   though the block documents them: the grid showed only what was known about
   each address. They are now marked (a ring, a dotted underline) and named in
   the legend, as plan marks rather than states.
6. **At 1280px with the assistant dock open the panel was one word wide.** The
   tree pane is now clamped to 54% of the container when the window is narrow,
   and the tree's columns and the grid's width answer to their own pane rather
   than to the window.
7. **Searching a lab address said "No record, nothing seen" about an address
   with a record and seven sightings** — defect 3 showing through. The engine
   has since been fixed; the screen now keys address hits by namespace and
   drops the backend's empty production row for an address something knows,
   which is the same symptom handled where it belongs.
8. **The learn preview said "Would create: 0 blocks 0 addresses 0 VLANs"**
   above 33 rows that said `create` (defect 8), and listed a lab as
   `lab:aus52ssnoyqdpblk`. Both fixed; its three tables now have headings and
   a border so a cut-off row reads as a scroll rather than as a mistake.
9. **"Write 1 rows"** in the import dialog, and the VLAN column header
   truncated to "VL…". Both corrected.
10. Smaller things: the coverage line broke "last read" from its time; the
    known-address table overflowed its pane at the narrower width; a
    discovered block offered "Document this block" twice, in the header and in
    the provenance box, so the header is now plain "Edit".
