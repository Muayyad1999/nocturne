# Radio links: measuring, recommending and changing the channel of a point-to-point link

What Nocturne knows and can do about a point-to-point radio link. The work
crosses eleven crates and the interface, so the contract is here and each
crate's own specification points at it.

**What the first build does, in plain words.**

* **It reads both radios** of a point-to-point link on MikroTik RouterOS,
  pairs them into a link, charts signal, noise, modulation, rates and drops,
  and alerts when the link loses its peer or its quality falls.
* **It says which channel would serve the link better, and why.** The
  candidates are the channels the radios themselves say they may use. The
  evidence is a scan of each end that Nocturne reaches without crossing that
  link (you approve each scan, because it takes the link down while it runs)
  and what the radios report while they carry traffic. Every recommendation
  says what it was computed from, what the scan could not see, and which end
  it could not look at.
* **It changes the channel when you approve that exact change.** Nocturne
  sets the master's frequency, and the slave finds the master by itself, as it
  does when you change the master by hand, because the new frequency is in
  its scan list. Before it sends the new frequency, Nocturne arms a revert
  **on the master itself**: unless Nocturne confirms that the link came back
  and holds, the master puts the old frequency back on its own, also if
  Nocturne's server dies in the middle.
  * *You control the master only* (the slave is not in Nocturne, or Nocturne
    cannot log in to it): Nocturne moves the master, checks the link from the
    master's side and never touches the slave. By approving, you state that
    the slave scans the new frequency; the plan says what happens if it does
    not.
  * *You control both*: first, as an approved change of its own, Nocturne
    adds the new frequency to the slave's scan list, with a revert armed on
    the slave because it is usually reached across the link. Then it moves the
    master and checks the link from both ends.
  * *You control the slave only*: Nocturne can add frequencies to its scan
    list and recommend a channel to whoever controls the master. It never
    moves a link from the slave's side.
* **It moves a master only when it can show that it reaches that master
  without crossing a radio link**, the link's own or another on the way. It
  works that out from the routes, ARP and bridge tables of the master, of the
  routers between it and Nocturne, and of the other radios. Where it cannot
  tell, it names the fact it is missing (usually a router between Nocturne
  and the radios that is not in the inventory) and does not move that
  master. Every move also tests this again while it runs, and a master that
  did not answer while its link was down is put back.
* **It tests the revert on each radio before it relies on it**, and refuses a
  change on a radio whose revert has not been seen to work on that software
  and under that login.

**What it does not do.**

* **Nothing by itself.** Every scan, every revert test and every channel
  change is a plan that a person approves. When a move turns out worse over
  the following days, Nocturne raises an alert with the move back already
  planned, and a person approves it. Automatic changes are not in this build;
  "Automatic changes: not in the first build" says why and what brings them
  back.
* **No radar (DFS) channel is ever a target**, and a channel whose radar
  status the radio did not state is treated as radar. It never changes the
  width, the transmit power, the country or any other regulatory setting, and
  never takes an entry out of a slave's scan list.
* **It never moves a master that it reaches through a radio link, and never
  scans an end that it reaches through the link**: if such a radio did not
  come back, nothing could reach it to bring it back.
* **Out of the box it changes nothing on a real radio.** No RouterOS radio
  command has been recorded on a real device yet. Until the first two
  recordings (read-only, minutes at a terminal) the radio's own channel list is
  not trusted, so no channel can be a target; until the bench recordings,
  every radio change needs the installation-wide setting "allow unverified
  writes" and says so on the plan. "Not verified" lists the recordings and
  what each one opens.
* **RouterOS `wifi` and 60 GHz radios are read and charted only**; radios that
  choose their own channel are watched and never fought; other vendors come
  later, as profiles.

**Nothing in this file has met a real radio.** No radio was available to the
research or to the reviews this file was written from. Every statement about
a device is marked by where it comes from: *documented* (the vendor's manual
prints it), *names only* (the manual lists a name and prints no output),
*remembered* (somebody's memory, checked nowhere), or *recorded* (none yet).
"Not verified" at the end lists what each label waits for.

**What this is not.** It is not Wi-Fi planning: no access points with many
clients, no sectors, no roaming, no WMM. It does not touch a licensed link, a
link whose two directions use different frequencies, or a radio that is
choosing its own channel. It does not move a link because the link is failing
right now. It does not take a radio off the air without a person's approval
of that outage. It draws no spectrum waterfall and has no table of what any
country allows. "Not building, and why" at the end has the whole list.

## Words

| Word | Meaning here |
|---|---|
| master | The end that owns the frequency: the access-point or bridge end. |
| slave | The end that looks for the master: station, subscriber, client. It has no frequency of its own, only a list of places to look. |
| near, far | The **near** end is reached without crossing the link. The **far** end is reached through it. Which one is master is a separate question. |
| reach | Which radio links the way between Nocturne and a device crosses: none (`direct`), the device's own link (`through_link`), another radio link Nocturne knows (`behind_link`), or `unknown`, with the fact that is missing. Everything that decides treats the last three alike. |
| channel | A control frequency, a width and where the control channel sits in it. Written `5795/40`: centre and width. The **control frequency** is the 20 MHz primary channel a platform configures; the channel is the whole span. |
| survey | A measurement of channels other than the one in use. On the first-build platform it takes the radio off the air. |
| follow | What the slave does after the master moves: it walks its list until it finds the master again. |
| dead-man, guard | A revert the **device performs by itself** when nobody confirms in time. |
| radar channel | A channel on which the device must listen for radar before it transmits and must leave when it hears one. Its status at a radio is `radar`, `not_radar` or `unknown`, and only the device says which. |
| blind end | An end with no survey. It is never assumed clean. |
| measured, heard, nothing decoded | The three things a survey can say of a slice of spectrum: how busy it was (measured); that a transmitter was decoded there, with no word on how often it transmits (heard); that the scan decoded nothing, which says nothing about what it cannot decode (nothing decoded). |
| trial, probation | A move made on incomplete evidence is a trial; the hours after any move are probation, in which the link's own record on the new channel is compared with its record on the old one. |
| proof | The record that a step was seen to work on this device, on the software it runs now. |
| ledger | The change engine's own list of every revert it armed and has not yet read back as gone. |
| forward, home | A move **forward** goes to a channel the link was not on. A move **home** is a return to the channel the last forward move left. |
| world | The devices that can share a radio link: the real network, or one lab. Nothing of one world is ever consulted for another. |

## Five facts the design hangs from

1. **Only the master moves a link; the slave only decides where it looks.** A
   master moved to a frequency the slave does not scan is not a worse channel.
   It is an outage that lasts until somebody visits.
2. **The link is usually the only management path to one of its two devices**,
   and sometimes to everything behind it. A change that could cut a device off
   must be one that device undoes by itself, and in the first build the device
   that owns the frequency is only changed when Nocturne reaches it another
   way.
3. **Nothing is done to a radio that a person did not approve, and each step
   needs what it rests on to have been seen to work on that link.** The steps
   are a ladder (below): each rung needs the rungs its row names, and there
   is no way round those. In this build nothing runs unattended at all.
4. **Unknown is not clean, and an assumption is not a measurement.** A channel
   never surveyed is unknown. A scan that decodes nothing is blind to what it
   cannot decode and says so. Every figure says which of the two it is, and
   a recommendation that rests on an assumption is called a trial.
5. **Unconfirmed means undone.** A change removes its armed revert in three
   situations only: nothing was sent after it was armed (a cancel before the
   change), the link is verified on the new channel, or it is verified back
   on the old one. In every other situation the revert is left to fire. A
   revert that was left behind and did not fire is removed by one thing only,
   a change a person approves for that purpose and nothing else.

Everything below follows from those and from the project's rules: a channel
change is a change and goes through the pipeline; a failed read is a recorded
failure; a device type is data; the model proposes and the pipeline decides.

## The first build, by platform

Decided from what the research could establish. The owner has been asked
which radios they run and has not answered yet; the answer changes rows of
this table and profile data, not the design.

| Family | First build | Why |
|---|---|---|
| MikroTik RouterOS, `/interface wireless` (802.11, Nv2, Nstreme; v6 and v7) | **Read, pair, chart, alert, survey, recommend, change.** | The usual outdoor point-to-point stack. The manual documents the properties, the role of the scan list, the one bounded survey (`scan ... rounds=1`) and every part of a scheduled revert. A verified profile exists and already classes the read commands read-only. Every wireless read and every write is still unverified. |
| RouterOS `/interface wifi` (v7.13+) | **Read, pair, chart, alert, recommend from passive reads. No survey, no change.** | A value can live at four levels of inherited profiles, how to unset one is not established, whether a manual change is announced or restarts the radio is not documented, and the stack reselects channels by itself. A survey there would be a planned outage on a link whose channel Nocturne then cannot move, so the survey is out with the change. |
| RouterOS `/interface w60g` (60 GHz) | **Read, pair, chart, alert. Nothing else.** | Four to six fixed channels, no width, no noise or airtime figure, every scan drops the link. Little to optimise and a link to lose. |
| Radios that choose their own channel (a RouterOS master on `frequency=auto` or an `XX` width, the `wifi` stack's reselection, Ubiquiti AF-5XHD and LTU, Cambium DSO and DCS, Mimosa) | **Observed, never fought.** Their moves are recorded and Nocturne's ranking is shown beside the device's choice. | Two controllers on one frequency undo each other. |
| Ubiquiti airMAX AC (airOS 8) | Later, first in line. | The only family in the research that can measure both ends without an outage, and only on units with the second (airView) radio, for targets that are not radar channels, with firmware 8.6.0 or later on the access point. No profile, no recorded command-line output, a change is a file edit and a restart, and the vendor's banner says command-line changes void the warranty: the owner's call. |
| Ubiquiti airMAX M, Cambium ePMP | Later. | No profile, no captures. |
| Licensed links, split-frequency links (Nstreme Dual, AF-5, AF-24), Nv2 synchronised groups | **Never changed.** Read where a profile exists. | One legal frequency, or a frequency that belongs to a site plan. |

A device type is data, so "later" means a profile with a `radio:` section and
recordings, not new Rust.

**What that amounts to on a real RouterOS link, in order.** The feature is in
the free set of features (the ISP edition contains the free set).

1. *With nothing recorded:* reads, pairing, charts, alerts and the findings
   that need no survey, every one under the banner "this read has not been
   checked against a real RouterOS device".
2. *After two read-only recordings from the terminal (R1, R2 in "Not
   verified"):* the device's own channel list becomes the source of
   candidates and of radar marks. Until then Nocturne does not know which
   channels are radar channels on a RouterOS radio, so it treats every channel
   as one: it retunes nothing and surveys no master.
3. *On a bench pair, with `changes.allow_unverified_writes` on:* the revert
   test, a survey, a scan-list preparation and a retune, each approved by a
   person. Their transcripts are the recordings that take the labels off.
   That setting is one switch for the whole installation; what every radio
   plan says while it is on is under "Staged trust".
4. *On a link, while the labels are still on:* the same, with the setting on
   for as long as the work takes, each plan saying so: surveys of ends reached
   directly, a ranking that says what was heard and what could not be seen,
   and moves by a person's approval, with a device-side revert that has been
   tested on that device.
5. *Once the shipped profile carries the recordings:* the same with the
   setting off, because the plans are `verified`.

Nothing on that list happens by itself. A person approves every scan, every
revert test and every move, including the move back after a move that
probation judged worse ("Automatic changes: not in the first build").

## Staged trust

Per link, enforced by the change engine from its own proof records, and not
by the feature that proposes:

| Rung | What happens | What it needs |
|---|---|---|
| 0. Observe | Reads, pairing, monitoring, findings, recommendations. | Nothing. |
| 1. Survey | A person approves a survey of an end reached directly. | The maturity gate (below). |
| 2. Test the revert | The dead-man drill passes on the master, and on the slave if its scan list is to be changed. | The gate. |
| 3. Supervised moves | A retune a person approved ends `applied`; every later one is approved the same way. | Rung 2 on the master; the slave already looks on the target (or, master only, the person states that it does). |

There is no fourth rung in this build: nothing is applied without a person
("Automatic changes: not in the first build").

A person may approve a retune to a channel of their own choosing without a
survey (rung 3 needs rung 2, not rung 1): that is the change they make by
hand today, made safer. A proof dies when the device's software version
changes, when the profile lines it proved change, or when the account
Nocturne logs in with changes.

**The maturity gate is the one the pipeline has.** `require_verified_or_allowed`
in `nocturne-changes/src/rpc.rs` refuses, at the start of every apply, a plan
whose `maturity` is not `verified`, unless `changes.allow_unverified_writes`
is on. It is used as it is used today; only its sentence gains a clause
that names what is unverified. What changes is what the planner writes into
`Plan.maturity` for a typed radio plan. It starts from the profile's own
maturity (a user's profile is never `verified`: the loader reserves that
word for built-in profiles; on a lab device, from the lab rule under "The
lab"), and it is `unverified` whenever anything the plan stands on is still
listed, for that stack **and that role**, in the profile: an operation it
uses (the change lines, the guard, the survey, the configuration check, the
channel list, the single-peer rule), **the reads of the written device's
role, or the reads of the witness's role**. A label can only lower the
maturity a plan starts from, never raise it. The planner's refusals and the
whole verification are those reads, so a plan is not `verified` while a
parser it leans on has met no device.

What that gate is, said plainly: `changes.allow_unverified_writes` is one
switch for the whole installation and for every device type, and nothing in
it limits it to a bench. It must be on for the bench recordings (R4 to R6),
and while it is on the gate is open for a tower too. The gate is not
changed and no second setting is added (that was decided). So the plan
carries the warning. Every radio plan that needs the setting says, on a
line of its own, "this plan can be applied only because the
installation-wide setting 'allow unverified writes' is on; that setting
also opens the unverified writes of every other device type", and carries
the protected hit `radio_unverified_write` (critical), whose subject lists
the operations and reads in the plan that have met no real device. The
typed confirmation a critical hit forces is then a confirmation of exactly
that. On top of the gate stand the proofs of this section, which no setting
opens: no retune without a drill that passed on that device, on that
software, under that account.

The gate applies to going back as well. The way back from a retune is a new
planned retune (`rollback.rs`), so undoing a move that was made while the
setting was on needs the setting on again. That is stricter than the
pipeline's own on-demand rollback today, which runs outside the gate; work in
flight on the change engine makes that one a planned change too.

A recording lifts a label in the **shipped** profile, and the harness
enforces that for shipped profiles only. A person who saves their own
override of a device type with the labels taken out has taken that on
themselves, as with any profile today; such a profile is `community` at
best, so its plans still need the setting.

## The reads: `radio` and `radio_peers` (`nocturne-drivers`)

Two new kinds. `Kind::Radio` is one row per radio interface; `Kind::RadioPeers`
is one row per registered peer per interface. Two kinds and not one nested
row, because a profile's `map` allows one level of nesting, a master can hold
several peers, and the two reads fail independently.

```rust
struct Radio {
    interface: String,
    default_name: Option<String>,          // the factory name the configuration export addresses the radio by
    stack: String,                         // which of the platform's radio stacks produced the row: "wireless", "wifi", "w60g"
    role: RadioRole,                       // master | slave | other, from the profile's role table. A mode that is neither
                                           // (sniffer, alignment, a WDS slave) is `other`, never guessed into one of the two.
    mode: Option<String>,                  // the platform's own word for it
    single_peer: Option<bool>,             // the master can serve one station only (its mode, or a cap of one read from the
                                           // device). None: cannot be told, or the profile's rule for it is still listed
                                           // unverified. Both are treated as "many".
    virtual_of: Option<String>,            // a virtual interface on this physical radio: read, never changed. "none" for a physical one.
    enabled: Option<bool>,
    mac: Option<String>,
    bssid: Option<String>,                 // a slave: the master it says it is connected to
    ssid: Option<String>,
    name: Option<String>,                  // what the peer sees this radio as
    protocol: Option<String>,              // "802.11", "nv2", "nstreme"
    band: Option<String>,
    state: Option<String>,                 // the platform's live status word
    radar_wait: Option<bool>,              // `state` is one of the profile's radar-wait words
    frequency_mhz: Option<u32>,            // control frequency in use NOW
    width_mhz: Option<u32>,                // width of the whole channel. A master: from its configured width. A slave: only
    offset_mhz: Option<i32>,               // where a recorded live read gives it. Centre of the channel minus the control frequency.
    configured_frequency: Option<String>,  // as written: a number, "auto", a channel name. A revert restores it as written.
    configured_width: Option<String>,
    scan_list: Option<String>,             // a slave's list of places to look, as written ("default,5560")
    scan_numbers: Option<String>,          // the plain frequencies in it, MHz, comma separated, as the profile parsed them
    scan_opaque: Option<String>,           // the entries that are not plain frequencies ("default", a list name, a range)
    scan_effective: Option<String>,        // the list the device says it will actually walk, where it prints one
    country: Option<String>,
    regulatory_mode: Option<String>,
    installation: Option<String>,
    tx_power_dbm: Option<i32>,             // shown where the platform prints it; nothing is computed from it
    noise_floor_dbm: Option<i32>,          // on the working channel
    peers: Option<u32>,
    auto_channel: Option<bool>,            // a MASTER is choosing its own channel. Always false for a slave.
    managed_by: Option<String>,            // something else owns this radio's settings: "capsman", "nv2-sync"; "none" when nothing does
}

struct RadioPeer {
    interface: String,
    peer_mac: String,                      // a row without it is dropped and counted as a `partial` problem
    peer_name: Option<String>,
    signal_dbm: Option<i32>,               // what THIS end receives from the peer
    signal_chains: Option<String>,         // per chain, "-61,-63"
    remote_signal_dbm: Option<i32>,        // what the peer says it receives from this end, where the platform passes it on
    noise_floor_dbm: Option<i32>,
    snr_db: Option<i32>,
    tx_rate_bps: Option<u64>,
    rx_rate_bps: Option<u64>,
    tx_mcs: Option<u32>,
    rx_mcs: Option<u32>,
    tx_quality_pct: Option<f32>,           // the platform's own link-quality figure; `quality_kind` says which
    rx_quality_pct: Option<f32>,
    quality_kind: Option<String>,          // "ccq" | "per"
    tx_frames: Option<u64>,
    tx_retries: Option<u64>,               // retransmitted frames, as the PROFILE derives them from the platform's counters;
                                           // absent where the protocol gives those counters no such meaning
    uptime_s: Option<u64>,                 // restarts when the link drops
    distance_m: Option<u32>,
}
```

Every number is optional and uses the lenient deserialisers, as `QosClass`
does: a field the platform does not print is absent, never zero.

*Width and where the control channel sits come from the configuration, not
from the live string.* The manual lists the values of the configured width
(`20/40mhz-Ce`, `20/40mhz-eC`, ...) and prints one live channel string, for a
20 MHz channel. So **for a master** `width_mhz` and `offset_mhz` are looked up
from `configured_width` in a table the profile holds, and a configured width
that lets the device pick (`XX`) has no offset: `auto_channel` is true. The
table's signs (`Ce` is +10) rest on the one worked example the manual
prints, which is the `wifi` stack's `current-channels` and not a page about
the legacy stack; R1 at 40 MHz with the extension on each side settles it.
**A slave has no channel of its own.** Its configured width is what it will
accept, not the layout it is associated on, so for a slave both fields are
absent until a recording (R1 on a slave) shows a live figure the profile can
map; `auto_channel` is false for a slave whatever its width says, and no
rule about a radio choosing its channel applies to one. If a master's
live string is present and does not parse to the same width, both stay as
configured and the row carries a `partial` problem that says so; planning
refuses such a radio (a wrong side moves the link onto a span that was never
scored). `5180` with width 40 and offset +10 is the span 5170 to 5210, centre
5190.

*The scan list reaches everything else already taken apart.* The profile's
expression splits the configured list into plain frequencies (`scan_numbers`)
and everything else (`scan_opaque`). An opaque entry covers nothing as far as
Nocturne can tell: what `default` or a named list expands to is only known
where the device prints its effective list (`scan_effective`) and that read
has been checked.

*Three things stay different, everywhere.* **Unknown**: the read failed or was
never made; nothing is stored and the failure is recorded. **Empty**: the read
succeeded and the device has no radio, or a radio with no peer; zero rows are
stored. **Not available**: the stack or the protocol has no such figure. The
profile says which figures each stack and protocol provide (`provides`), and
every screen and every score names what cannot be given. Under Nv2 and
Nstreme the frames comparison is "not comparable" by the manual's own words
and what CCQ means is not documented, so `retry_pct` and the quality rule are
computed for 802.11 only and say "not available under this protocol"
otherwise, until a recording says more.

*A field a decision tests is never left absent by a successful read.* The
planner refuses on `managed_by`, `auto_channel`, `virtual_of`, `country`,
`regulatory_mode`, `mode`, `default_name` and the configured frequency and
width. For each the profile maps the device's answer to a value also when
the answer is "no" (`managed_by: none`, `virtual_of: none`, `auto_channel:
false`). Absent therefore means one thing, "the read did not give it". A
stack lists the fields a write needs (`requires:` in the `radio:` section),
and a radio with one of them absent is refused with "could not read
<field>", never planned as if the answer were no.

*What RouterOS fills.* "documented": the manual prints an example of that
command's output. "names only": the manual lists field names and prints no
output. Nothing is recorded.

| Field | `wireless` (legacy) | `wifi` | `w60g` |
|---|---|---|---|
| `role` | master: `ap-bridge`, `bridge`; slave: the `station*` modes; other: the rest | master: `ap`; slave: the `station*` modes | master: `bridge`, `ap-bridge`; slave: `station-bridge` |
| `single_peer` | remembered, not documented: `bridge` takes one station; `max-station-count` caps the rest. It is the operation `single_peer`, listed unverified, and read as "many" while it is (R4 tests it) | cannot be told | cannot be told |
| `default_name` | `default-name` in `print detail` (remembered: it is the name the export addresses a radio by; R1 and R3 record it) | the same | the same |
| `frequency_mhz` | `monitor`: `channel` (documented once, on a master, 20 MHz) | `monitor`: `channel` (names only) | `monitor`: `frequency` (the manual's example has two pairs on one line, an artefact of its web page; the capture written from it is synthetic) |
| `width_mhz`, `offset_mhz` | from `channel-width` through the profile's table (values documented) | from `channel.width` | none |
| `configured_frequency` | `frequency` | `channel.frequency` | `frequency` |
| `scan_list` | `scan-list` | `channel.frequency` on a slave | `scan-list` |
| `scan_effective` | `info scan-list` (names only; operation `effective_scan_list`) | never | never |
| `bssid` | `monitor`: `bssid` (names only) | names only | names only |
| `country`, `regulatory_mode`, `installation` | `country`, `frequency-mode`, `installation` | `configuration.country`, none, `configuration.installation` | `region`, none, none |
| `noise_floor_dbm` | `monitor`: `noise-floor` (documented) | never | never |
| `state` | `monitor`: `status` (documented for a master) | `monitor`: `state` (names only) | `monitor`: `connected` |
| `auto_channel` | a master with `frequency=auto`, or with a width ending in `XX` | more than one frequency configured or none, or a reselect setting | `frequency=auto` |
| `managed_by` | `nv2-mode` of `sync-master` / `sync-slave`; CAP mode | `configuration.manager` other than `local` | never |
| `signal_dbm` | registration table `signal-strength` (names only; printed as a composite whose shape is unseen) | registration table `signal` (the manual's example is the column form of a different command; the capture is synthetic) | per station, `station monitor` (not read in the first build) |
| `remote_signal_dbm` | `tx-signal-strength` (no description in the manual) | never | never |
| `snr_db`, quality, `uptime_s`, `distance_m` | `signal-to-noise`, `tx-ccq`/`rx-ccq`, `uptime`, `distance` | never, never, `uptime`, never | never, `per` (an error rate, where 0 is good: stored with `quality_kind: per`, charted as an error rate, read by no quality rule), never, `distance` |
| `tx_frames`, `tx_retries` | `frames`, and `hw-frames` less `frames` by the profile's expression, for 802.11 only (names only) | never | never |
| `tx_power_dbm` | `monitor`: `current-tx-powers` (names only; a list per rate whose shape is unseen). Shown where it parses | never | never |

Whether a station's registration table lists its access point at all is not
settled: the manual says the table is "used only for Access Points", the
researcher remembers otherwise. So pairing accepts the slave's `bssid` as the
slave's statement (below), and R1 records which of the two a station prints.

A profile reads them with ordinary `collect` steps, in YAML:

| Stack | Commands | Parser |
|---|---|---|
| `wireless` | `/interface wireless print detail without-paging proplist=...`; `/interface wireless monitor {interface} once` per radio; `/interface wireless registration-table print detail without-paging` | `kv`, `colon`, `kv` |
| `wifi` | `/interface wifi print detail without-paging proplist=...`; `/interface wifi monitor {interface} once` per radio; `/interface wifi registration-table print detail without-paging` | `kv`, `colon`, `kv` |
| `w60g` | `/interface w60g print detail without-paging proplist=...`; `/interface w60g monitor {interface} once` per radio; `/interface w60g station print detail without-paging` | `kv`, `colon`, `kv` |

Four things the collector cannot do today and must learn:

* **A stack that is not there is empty, not failed, and nothing else is.**
  Today a kind whose every step failed is `Failed`, optional or not, so every
  RouterOS router without a wireless package would show two failed kinds for
  ever. A step may declare the answers that mean the feature is absent
  (`absent: ['bad command name']`; that text is remembered, and R1 records
  it); such an answer is a successful read of zero rows from that step.
  `absent` is accepted on a kind's listing step only, never on a `per: row`
  step, and each pattern must hold the stack's own menu word (`bad command
  name wireless`): RouterOS gives that answer to any wrong word, and a
  mistyped sub-command must read as a failure, not as "no radio". Any other error (a timeout, "not enough
  permissions") is a failure, also on an optional step: a 60 GHz read that
  timed out is never stored as "no 60 GHz radio".
* **A command per row, for the rows it belongs to.** A step gains `per: row`
  with a selector (`for: "stack == 'wireless'"`): it runs once for each
  matching row the kind already has, with `{interface}` substituted, and the
  row it produces is given that `interface` so `merge_on` has something to
  match. A step also carries `stack:`, which is what the per-stack label
  below hangs on.
* **A substituted value is checked, and the command is classified after the
  substitution.** Device names are free text on RouterOS. The profile declares
  the pattern each substituted value must match (`values:` in the `radio:`
  section); a value that does not match is not sent. A radio whose name is
  not a plain word is still listed (from `print`), its per-row fields are
  absent, and the kind carries a `partial` problem naming it. The rendered
  command, not the template with a sample, is what must classify read-only.
  There is no escaping rule in the first build, because none has been seen
  to work; R5 records one. A value whose pattern admits no character that
  means anything inside double quotes (a country name) may be placed between
  them.
* **Reads that print secrets.** MikroTik's own examples of `print detail` show
  a Wi-Fi passphrase and a 60 GHz password in clear. The steps ask for the
  properties they map and nothing else (`proplist=`), so the secret is not
  sent at all. A read that uses `proplist=` and is refused **fails**; it never
  falls back to the unrestricted listing. Raw output kept anywhere goes
  through the profile's `config.secrets` patterns first, and the harness runs
  the same patterns over every capture and refuses one in which they find a
  value not marked as a documentation sample. The patterns gain the WEP names
  (`static-key-0` to `static-key-3`), which they miss today.

### Reads that have not met a real device: labelled per stack and per role

No recording of any wireless command exists in this repository. `spec-qos.md`
labels an unverified read per kind (`unverified: [qos]`), and the harness
takes the label off the day one recorded capture covers the kind. Per kind is
too coarse here: the easiest radio for anyone to record is a home router on
the `wifi` stack, and that one recording would take the banner off the legacy
stack, whose parsers have still met nothing, and off station reads when only
an access point was recorded. So for these two kinds the label lives in the
stack:

```yaml
radio:
  stacks:
    wireless:
      unverified:
        reads: [master, slave]          # roles whose reads have met no recording
```

* A capture's `expect.yml` names the stack and the role it was taken from. The
  harness accepts documented and synthetic captures for a role still listed,
  requires at least one the read succeeds on, and **fails if a recorded
  capture exists for a stack and role still listed**. A role comes off the
  list when it has a recording; a stack's reads are verified when both roles
  have.
* The two kinds are not listed in the profile's top-level `unverified`. The
  harness's rule that a verified profile needs a recorded capture per kind is
  met, for these two kinds, by the per-stack list.
* Every row view carries its own flag: `collect.get` and `collect.status`
  carry `unverified: true` on a document any of whose rows comes from a listed
  stack and role; `monitor.radio`, `radio.links` and every evaluation carry it
  per radio; every screen that shows the data says "this read has not been
  checked against a real RouterOS device" above it. In Rust:
  `Profile::radio_unverified(stack, role)`.
* Only `wifi print detail` may carry a `documented` capture: it is the one
  read whose exact command the manual prints with its output. The rest are
  synthetic and say so.
* `maturity` stays `verified`: lowering it would take write access away from
  everything else the RouterOS profile has earned.

Recording the reads from a real device is read-only and possible today from
the terminal.

## What is vendor-specific lives in the profile: the `radio:` section (`nocturne-drivers`)

`Profile` refuses unknown keys, so this is a schema change in `profile.rs`
with its documentation in `docs/profiles.md`. It holds everything the engine,
the pipeline and the feature need to know about a platform's radios, per
stack. Nothing vendor-specific is written in Rust: no scan-list grammar, no
layout words, no export-line shape, no escaping rule.

```yaml
radio:
  stacks:
    wireless:
      since: "6.35"                # the lines below exist from this software version; the planner refuses below it
      menu: '^/interface wireless\b'          # which configuration lines belong to this stack
      line: '^/interface wireless set \[ find default-name={default_name} \]'   # the export line that is this radio;
                                   # {default_name} goes in as a literal, every pattern character in it escaped
      line_parser: kv              # which of the drivers' parsers takes that line apart into keys and values
      roles:
        master: [ap-bridge, bridge]
        slave:  [station, station-wds, station-bridge, station-pseudobridge, station-pseudobridge-clone]
      single_peer: { modes: [bridge], cap: { property: max-station-count, equals: 1 } }
      owner: master                # who holds the frequency: master | both (both = a mirrored pair: read-only)
      follow: scan                 # how the slave arrives: scan (it rescans its list) | protocol (the master announces)
      provides:                    # per protocol; a figure not listed is "not available", never missing
        "802.11": [signal, remote_signal, noise_floor, snr, quality, retries, rates, uptime, distance]
        nv2:      [signal, remote_signal, noise_floor, snr, rates, uptime, distance]
      rates: { "5ghz-a/n": ht, "5ghz-a/n/ac": vht, "5ghz-onlyac": vht }     # the rate table by the radio's band
      layout:                      # configured width -> width and offset of the centre from the control frequency
        "20mhz":            { width: 20, offset: 0 }
        "20/40mhz-Ce":      { width: 40, offset: 10 }
        "20/40mhz-eC":      { width: 40, offset: -10 }
        "20/40/80mhz-Ceee": { width: 80, offset: 30 }
        # ... one row per fixed form; the XX forms are absent: the device chooses
      scan_list: { split: ',', number: '^[0-9]{4,5}$' }     # anything else in the list is opaque
      values:                      # every value that may be put into a command, and the pattern it must match
        interface:    '^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$'
        default_name: '^[A-Za-z0-9][A-Za-z0-9_.-]{0,31}$'
        control_mhz:  '^[0-9]{4,5}$'
        scan_list:    '^[A-Za-z0-9_.,:-]{1,200}$'
        country:      '^[A-Za-z0-9 ._-]{1,40}$'     # may hold a space: only ever placed between double quotes
      refuse:                      # radios nothing is planned for, each with the sentence a person reads
        - { when: "managed_by",                       why: "{managed_by} owns this radio's settings" }
        - { when: "auto_channel",                     why: "this radio is choosing its own channel" }
        - { when: "virtual_of",                       why: "this is a virtual interface of {virtual_of}" }
        - { when: "not country",                      why: "this radio reports no country" }
        - { when: "country == 'no_country_set'",      why: "this radio reports no country" }   # RouterOS's word for none (remembered; R1)
        - { when: "regulatory_mode == 'superchannel'", why: "this radio is in a conformance-testing mode: its own list of allowed channels is everything the card can tune" }
      requires: [mode, country, regulatory_mode, configured_frequency, configured_width,
                 managed_by, auto_channel, virtual_of, default_name]     # absent = "could not read": refused
      config_fields: [configured_frequency, configured_width, scan_list, country, regulatory_mode,
                      installation, mode, ssid, band, protocol, enabled]  # the fields of the row that are configuration
      unbounded_power: ["manual-txpower"]      # modes in which the device does not lower power by itself
      radar_wait_states: [radar-detecting]
      allowed:                     # what the DEVICE says it may use. The only source of candidates and of radar marks.
        steps:                     # an operation read: steps shaped like collect steps, to rows of radio::AllowedRow
          - command: '/interface wireless info allowed-channels {interface}'
            parser: <a parser the drivers have>          # kv | colon | regex | textfsm
            map: { control_mhz: <expr>, widths: <expr>, radar: <expr: true, false or none>,
                   max_power_dbm: <expr>, restrictions: <expr> }
          - command: '/interface wireless info country-info "{country}"'
            parser: <a parser>
            map: { control_mhz: <expr>, max_power_dbm: <expr> }
            merge_on: control_mhz  # merged into the rows above as collect steps merge
        indoor_only: [indoor]      # the device's own restriction words that mean "not outdoors"
      effective_scan_list:         # to rows of radio::ScanEntry
        steps: [{ command: "/interface wireless info scan-list {interface}", parser: <a parser>, map: { control_mhz: <expr> } }]
      survey:
        - id: scan
          command: "/interface wireless scan {interface} rounds=1"
          since: "6.35"
          link_effect: down        # down | none | unknown. `unknown` is treated as `down`.
          measures: networks       # the only kind the drivers have rows for in the first build
          max_s: 120               # an estimate until measured
          parser: <a parser>       # to rows of radio::Heard
          map: { address: <expr>, ssid: <expr>, name: <expr>, control_mhz: <expr>, width_mhz: <expr>,
                 offset_mhz: <expr>, signal_dbm: <expr>, noise_floor_dbm: <expr> }
      change:
        set_channel: { lines: ["/interface wireless set {interface} frequency={control_mhz}"], keys: [frequency] }
        set_scan:    { lines: ["/interface wireless set {interface} scan-list={scan_list}"],   keys: [scan-list] }
                                   # keys: the properties of the radio's export line the operation may alter, and no other
      never: ['\bcountry=', '\bfrequency-mode=', '\binstallation=', '\bantenna-gain=', '\btx-power',
              '\bmode=', '\bssid=', '\bsecurity', '\bskip-dfs-channels=', '\bwireless-protocol=',
              '\bchannel-width=', '\bband=']
      timing: { follow_estimate_s: 180, management_return_s: 120, measured: false }
      unverified:
        reads: [master, slave]
        operations:                # each with the roles it has not been checked on
          allowed: [master, slave]
          effective_scan_list: [master, slave]
          scan: [master, slave]
          set_channel: [master]
          set_scan: [slave]
          config_check: [master, slave]
          single_peer: [master]
  reach:
    wired: [ether, sfp]            # interface kinds that are positively a cable
    bridges: [bridge]              # looked through by the MAC table
    vlans: [vlan]                  # judged as the interface they ride on (`Interface.parent`); a parent not read is unknown
  guard: ...                       # the device-side revert; see "The dead-man switch"
```

Rules the loader enforces: every `allowed`, `effective_scan_list` and
`collect` command, and the guard's four reads (`clock`, `check`,
`mark_read`, `leftover`), classifies read-only, and all of them are sent
through `Cli::read`; every `survey` command classifies `disruptive` (next
section) and nothing else may; every `change` line classifies as a change
and matches none of the stack's own `never` patterns; a stack with `owner:
both` has no `change`; every `{name}` in a template is declared under
`values` or is one the engine fills from its own numbers; every `refuse`
rule tests a field listed under `requires`; `unverified.operations` names
operations the stack has, with roles the stack has; every operation read maps
to the fields of its row type and nothing else; a survey's `measures` is one
the drivers have rows for (`networks`, in the first build).

Rules enforced on every rendered line, because the template is not what is
sent: each substituted value matched its pattern; the rendered line
classifies as the template did; the rendered line matches none of the
`never` patterns. A radio whose name contains a space and `country=...` is
refused, not sent.

**One renderer, and it is in the drivers.** Turning a template and values
into the line that is sent (`{name}`, `{delay}`, the joined and escaped body
of a revert), and those three checks on the result, are functions of
`nocturne-drivers` (`radio::render`). The change engine calls them and has
no rendering of its own. The profile harness calls the same functions
through four session actions (`radio_change`, `guard_arm`, `guard_check`,
`guard_disarm`): a scripted session holds the values and the line it
expects, and the harness renders the line, so "Nocturne sends what the
profile describes" is tested by rendering and not by a line somebody typed.

`never` is what makes regulatory settings a refusal and not a risk score:
several of them are ordinary writable properties, so "the device accepted it"
protects nothing.

There is no radar wait in a profile. A figure from a regulation, sitting in a
vendor profile and used for every country, is a one-row country table. Where
the device does not print a wait (RouterOS prints none that the manual
shows), the wait is unknown, and unknown is the longest:
`changes.radio.unknown_wait_s` (600; a setting, named for what it is: how
long Nocturne waits before it may say a link did not come back), until this
link has measured one.

**Operations and their reads.** The reads an operation makes (`allowed`,
`effective_scan_list`, a survey, and the guard's `clock`, `check`, `mark_read`
and `leftover`) are steps of the same shape as collect steps: a command, a
parser the drivers already have, a `map` and, where two commands make one set
of rows, `merge_on`. They do not produce collected kinds. Each produces rows
of a type the drivers define in `radio.rs`, through one function the change
engine calls (`radio::read_operation(profile, stack, operation, role, text)`),
so the engine parses no vendor text of its own:

```rust
struct AllowedRow { control_mhz: u32, widths: Vec<u32>, radar: Option<bool>,   // None: the device printed no mark
                    max_power_dbm: Option<i32>, restrictions: Vec<String> }
struct ScanEntry { control_mhz: u32 }
struct Heard { address: String, ssid: Option<String>, name: Option<String>, control_mhz: u32, width_mhz: Option<u32>,
               offset_mhz: Option<i32>, signal_dbm: Option<i32>, noise_floor_dbm: Option<i32> }
struct GuardEntry { name: String, run_count: Option<u32>, next_run: Option<DeviceInstant> }
struct DeviceInstant { text: String, seconds: i64 }   // seconds on the device's own clock, from the declared formats
```

The manual prints no output for the `info` commands or for `scan`, so E
writes the RouterOS `parser` and `map` against synthetic text, and they are
corrected by the recordings that lift their labels (R2, R5, R6).

**Operations and their labels.** `allowed` (the channel list with radar marks,
power and restrictions), `effective_scan_list`, each survey by its id,
`set_channel`, `set_scan`, `config_check` (the shape of the radio's line in
the export), `single_peer` (the rule by which a master is said to take one
station only) and, in the guard block, `guard`. Each is listed with the
roles it has not been checked on, and a plan counts an operation for the
role of the device it uses it on. While `single_peer` is listed,
`Radio.single_peer` is "cannot be told". While `allowed` is listed, a
successful read of the list is treated exactly as a failed one: a parser
written against invented text may have matched a subset or a superset, and
every check that reads the list would agree with it. And once it is checked:
a parse that yields no rows, or leaves a line of the answer unparsed, is "the
list could not be read", never a list.

What lifts each label is a recorded artefact of one named form, for the role
it was taken on, with `origin: recorded`; a synthetic one lifts nothing, and
the harness fails if a recorded artefact exists for an operation and role
still listed, as it does for reads:

| Operation | Lifted by | Kept as |
|---|---|---|
| `allowed`, `effective_scan_list` | a capture of each command's output with the rows it must parse to (R2) | `captures/<case>/<command slug>.txt`, with `operation:`, stack, role and the expected rows in `expect.yml` |
| a survey (`scan`) | a capture of the survey's whole output with its rows (R6) | the same |
| `config_check` | two captures of the export, before and after a recorded change, and the keys of the radio's line that differ (R3) | the same, `operation: config_check` |
| `set_channel`, `set_scan` | a recorded session of the change line and the reads after it (R4) | `session/<case>.yml`, rendered through the harness's session actions |
| `guard` | a recorded session of arming, the read-back, the mark and disarming (R5) | the same |
| `single_peer` | a recorded session in which a second station is refused by a master in the single-peer mode or cap (R4) | the same |

A session's file says `origin: recorded | synthetic` as a capture does, and a
file with no `origin` is synthetic, so the existing session files need no
edit. The harness reads an operation capture under the slug of an operation
command of the `radio:` section, as it reads a collect capture under the slug
of a collect command.

## A command that interrupts service: the `disruptive` class (`nocturne-drivers`, `nocturne-app`)

A survey is not a read: it takes a radio off the air. It is not configuration
either: it changes nothing a diff could show. Today the policy has no place
for it, so it would be either wrongly listed read-only, which hands it to
collectors, schedules, playbooks, the runner and the assistant, all
unattended; or sendable only as a configuration line somebody typed into a
change, where it would never end.

* `CommandClass` gains `Disruptive`: "changes no configuration; interrupts
  service while it runs". The enum's order becomes `ReadOnly < Change <
  Disruptive < Dangerous`, because `classify` keeps the worst part of a line.
  A profile lists such commands under `policy.disruptive`, and a command is
  tested against that list **before** the read-only list, as it is against
  the dangerous one: `/interface wireless align monitor wlan1` also matches
  today's broad read-only pattern for `monitor`.
* `Cli::read`, `connect::exec`, `devices.exec` and the collectors refuse it
  without being touched: every one of them tests "is not read-only". Its
  classification carries the sentence a person should read ("this command
  takes the radio off the air; it can only be run as a survey, from Monitoring
  → Radio links"), and `connect.rs` prints that instead of "go through a
  change".
* The planner refuses a `disruptive` command typed into a `Lines` change, as
  it refuses a `dangerous` one. Today it tests `== Dangerous`, which a
  disruptive line would pass; the test becomes "anything above `Change`".
* The only caller that may send one is the change engine, for a typed survey
  target, over `Cli::session()`, with the per-command timeout the session
  already has (`exec_with_timeout`).

For RouterOS the list is: `/interface wireless scan`, `frequency-monitor`,
`spectral-scan`, `spectral-history`, `snooper`, `sniffer` (remembered: it
takes the card into sniffing), `align`; `/interface wifi scan`,
`frequency-scan`, `flat-snoop`, `spectral-scan`; `/interface w60g scan`,
`align`. The manual says the link drops for some of these and says nothing
for the others; where it is silent the class is still `disruptive`. R1 lists
each menu's whole command set, so that a command that takes the card off the
air and is missing here is added before anything reads that menu.
The `info` commands and `/interface wifi radio reg-info` become read-only,
each by a pattern that names that one command exactly; the manual describes
them as lookups and prints output for `hw-info` and `reg-info` only. Until
that lands they can be recorded from the terminal and not through
`devices.exec`.

## Links (`nocturne-radio`, a new feature crate)

The crate depends on the core, `nocturne-drivers` (the row types) and
`nocturne-rf` (the engine, below). It links to no feature, and it never opens
a device session that is not a policy-checked read and never sends a line to
a device.

Every call it makes to another feature or to the core's methods goes through
one module (`calls.rs`), which holds the list of method names it may use, and
says so when one is absent. Its background task acts with nobody watching,
and nothing at the RPC layer tells a feature from a person, so its reach is a
closed list with a test, as the assistant's is. The list is exactly this, and
the test fails if `calls.rs` holds any other name or lacks one of these:

| Method | Why |
|---|---|
| `changes.create` | create a radio change with `plan: true`: a survey, a revert test, a preparation, a retune (the move back that probation asks for among them) or the removal of a leftover. Every one of them waits for a person. |
| `changes.get`, `changes.list` | a link's changes, for its history and its ladder |
| `changes.reach` | how each end is reached, for display |
| `changes.guards`, `changes.guards.check` | the ledger, and a closing read of it when a device it names answers again |
| `changes.proofs` | the ladder |
| `monitor.radio` | the two radios' present figures |
| `metrics.query` | the series the evaluation and probation read |
| `alerts.raise`, `alerts.resolve` | the alerts the feature derives (under "Monitoring") |
| `collect.get`, `collect.run` | the two kinds and the tables pairing reads; a refresh |
| `devices.list`, `devices.get` | names, sites and the lab marker |

Nothing on that list can approve, apply, roll back, cancel, grant, change a
setting, open a maintenance window, edit a device or save a profile.

### The model

```rust
struct RadioLink {
    id, name,
    ends: [LinkEnd; 2],                 // ends[0] is the master, ends[1] the slave
    pairing: Pairing,
    control: Control,                   // both | master_only | slave_only | none: derived, never set by hand
    sync_group: Option<String>,         // a person's statement: these radios transmit in the same synchronised phase
    policy: LinkPolicy,                 // need, exclusions: tuning that can only narrow, never permission
    state: LinkState,                   // observing | proposal | executing | probation | settled | held
    held_by: Vec<String>,               // why nothing is happening, in sentences
    created_at, updated_at,
}
struct LinkEnd {
    role: RadioRole,                    // master | slave
    device_id: Option<String>,          // None: this end is not in the inventory
    interface: Option<String>,
    mac: Option<String>,
    seen_as: Option<String>,            // the name the other end reports for it
    manageable: Manageable,             // yes | read_only | unreachable | not_in_inventory
    site: Option<String>,               // the device's `site`; None when it is empty
    stack: Option<String>,
    software: Option<String>,           // from `facts`
}
struct Pairing { source: auto | manual, confidence: confirmed | one_sided | manual,
                 evidence: Vec<String>, seen_many_at: Option<i64>, at }
```

Reach is not in the link: it belongs to the change engine (below) and the
feature asks for it. Links, evaluations and surveys are documents; there is
no new table.

### Pairing

*Automatically*, from the two collected kinds, after every `collect.updated`
that carries them:

* **confirmed**: device A has a peer row whose `peer_mac` is the `mac` of a
  radio on device B, and B says the same of A, either by a peer row or, for a
  slave, by its `bssid`. Both ends say so.
* **one-sided**: A names B's radio, and B's rows are missing (B unreachable,
  B's read failed, B's device type has no radio read).
* **far end not in the inventory**: A's peer matches no radio Nocturne knows.
  The link has one end with a `device_id` and one with a MAC and the name A
  reports for it. This is a link, and it is the "one of them" case.

A link is made only from a master that has had **exactly one** registered peer
on three reads running, with roles master and slave. A master with several
peers is a sector, which this feature does not manage; it is listed under
"not point-to-point" with the count, so nothing is silently missing. A link
whose master is later seen with more than one peer records when
(`seen_many_at`) and is held. Three reads are fifteen minutes, and at night a
small sector with one client online looks exactly like a link, so pairing is
not what decides whether a master may be moved: the planner is (the protected
hit `radio_master_serves_many`, below).

*By hand*: `radio.link.save` names the two ends, each as an inventory device
and interface or as a bare MAC and name. A manual link is checked against the
reads whenever they exist and says when they disagree; it is never silently
rewritten.

*One world.* Both ends of a link are in the same world: two lab devices of
the same lab, or two real devices. Automatic pairing only ever compares the
radios of one world, `radio.link.save` refuses two ends of different worlds
with the reason, and a lab link is one whose two ends are lab devices of one
lab. A link with one end that is only a MAC is in the world of its other
end. The planner refuses a radio plan whose witness is in another world than
the written device.

A pairing that stops being true (a different MAC registers, a role flips, a
device leaves the inventory) does not vanish: the link is `held` with the
reason. The engine checks the same facts by itself when it plans (below);
neither relies on the other.

### Which end is reached through the link (`nocturne-changes`)

This is the fact the ordering and the refusals depend on, and nothing in
Nocturne knows it today. It is worked out **by the change engine**, because
the engine must not take a safety fact on a proposing feature's word.

There is one question: **which radio links does the way between Nocturne and
the device cross?** For a device `D` the answer is `direct` (none),
`through_link` (`D`'s own link), `behind_link` (another link, named) or
`unknown` (with the fact that is missing). Only `direct` lets a master be
moved or an end be surveyed; everything that decides treats the other three
alike.

**What it is computed from.** The `routes`, `arp`, `mac_table`, `interfaces`
and `radio` rows of the devices in `D`'s **world**: a lab device's world is
its own lab, and a real device's world is every device that is not a lab
device. Nothing of another world is ever read for it, so no lab changes the
reach of a real device and no real device the reach of a lab's. The
**radios** of a world are the radio interfaces in its devices' `radio` rows,
each with its registered peer, which names its link. That is the engine's
own list, from collected rows: it does not ask the radio feature for its
links, and it covers every link the feature can pair, because pairing reads
the same rows.

**When it is read.** A plan reads every table it uses live, after Nocturne's
session to `D` has carried traffic: `D` first, then, in parallel, every other
device of the world that has a radio, then the routers the walk reaches. A
bridge learns the addresses of the traffic that crosses it and forgets them
in minutes (remembered: RouterOS's default is five), so a table read just
after Nocturne has talked to `D` holds the addresses of that very traffic on
every bridge it crossed, and an address that aged out of a table cannot
decide anything (step 4). The same reads are made again just before anything
is sent; a stale answer never decides. `changes.reach`, for display, answers
from the stored reads with their age ("as of <time>; a plan reads again") and
opens no session.

**The walk.** Nocturne starts at `D` and walks toward itself one hop at a
time, using at each hop only what that device reports about itself.

1. **Where `D` sees the session come from** (`src`). The profile's own read of
   it, where the profile has one that has been checked (none for RouterOS
   yet: the listing of active users is remembered, and is recording R9).
   Otherwise the local address of Nocturne's socket, and only when there is
   no jump host and the socket's peer address is one of the device's own
   collected addresses (no translation toward it). For a lab device, the
   lab's management station. Otherwise `src` is not known and the answer is
   `unknown`. A translation of Nocturne's own address on the way cannot be
   seen from Nocturne's side, so the evidence says "taken from Nocturne's
   socket, not from the device"; a translating router of the inventory is on
   the walk like any other, and one outside it stops the walk (step 5).
2. **The way back from the current device `X`** (`D` first). In `X`'s
   `routes`, among those that cover `src` and are in use (`Route.active`,
   where the profile fills it; where it does not, every route listed), the
   one with the longest prefix. Two or more at that length is `unknown`
   ("two ways toward Nocturne at `X`": the reply and the request may travel
   differently). One: its next hop `Y`, or `src` itself on a connected route;
   then, in `X`'s `arp`, `Y`'s MAC and the interface it was learned on. No
   such entry is `unknown`.
3. **The interface `X` sends it by**, judged by its collected `kind` and the
   profile's `reach` table. A radio, or an interface that belongs to one
   (virtual, per-station): the way crosses that radio's link. A bridge: look
   `Y`'s MAC up in `X`'s `mac_table` and judge the port it was learned on in
   the same way. A VLAN: judge the interface it rides on (`Interface.parent`,
   a new optional field a profile fills where the platform prints it; for
   RouterOS the `interface=` of `/interface vlan`, remembered, recording R9).
   A management VLAN is the usual arrangement on a tower and must not be
   `unknown` merely for being one. Positively a cable (`reach.wired`): there
   is no radio at `X` on this way. Anything else (a VLAN whose parent was not
   read, a tunnel, a mobile modem, a per-station interface that cannot be
   tied to a radio, a kind the profile does not list) is `unknown`, naming
   `X` and the interface. "Learned on some other port" is never enough: in
   the usual transparent arrangement a peer is remembered to be learned on a
   dynamic per-station interface, which is not the radio and is not a cable
   either.
4. **The bridges between `X` and `Y`.** A radio that bridges appears in
   nobody's routes, so each one is asked, and it is asked about the two MACs
   that the frames of this stretch carry. **`X`'s MAC is the MAC of the
   interface `X` sends by toward `Y`**: the interface that `X`'s `arp` entry
   for `Y` is on (step 2), which is the bridge or the VLAN itself where `X`
   sends through one, not a port under it. For `D` that is the interface its
   answers to Nocturne leave by. At a router of the walk it is the router's
   interface toward Nocturne, never the one the walk arrived by: a router
   sends from each interface with that interface's own MAC, and a bridge on
   the stretch has learned only that one. An interface whose MAC was not
   collected is `unknown`, naming `X` and the interface. `Y`'s MAC is the one
   in `X`'s `arp`. Then, for every radio of the world that is not on `X`
   itself, in its device's `mac_table`: both MACs there **on different
   sides** of that radio (one learned on the radio or an interface that
   belongs to it, the other on any other port step 3 can judge): the way from
   `X` to `Y` crosses that radio's link. Both there on the same side, or on
   one port: it does not. One of them or neither there: that radio says
   nothing about this stretch, because the traffic between `X` and `Y`, which
   has just flowed, did not pass it with those addresses. One of them learned
   on a port that step 3 would call `unknown` (a per-station interface that
   cannot be tied to a radio, a kind the profile does not list) and the other
   elsewhere: that radio's answer is `unknown`, naming the radio and the
   port. A link is crossed when any of its radios shows a crossing; a stretch
   that no radio shows crossed, and that one radio left `unknown`, is
   `unknown`.
5. **The next hop.** `Y` is `src`: the walk ends. `Y` is a device of the world
   (`Y`'s address is one of its collected addresses): before going on, `Y`'s
   own route toward `D`'s address, chosen as in step 2 (two or more at the
   longest prefix is `unknown`, "two ways toward `D` at `Y`"), must leave by
   the interface on which `Y` reaches `X`. The way there must be the way
   back, or the walk would have followed the answers and not the requests
   (`unknown`: "at `Y` the way to `D` and the way back differ"). Then `X`
   becomes `Y`, and the walk goes on from step 2. `Y` is neither: `unknown`,
   "the router at <address> (<MAC>) on the way from `D` to Nocturne is not in
   the inventory". A walk that comes back to a device it has visited is
   `unknown` ("the way loops at <device>").

**The answer.** `through_link` when the way crossed `D`'s own link, at any
step of any stretch; otherwise `behind_link`, naming the first link
crossed, when it crossed another; `direct` when the walk reached `src` and
crossed none; `unknown`, with the step, the device and the missing fact,
when a step could not be completed or a stretch was left `unknown`. A link
that says nothing never makes an answer `unknown`, so one link Nocturne
cannot read never stops every device, and a link that is not on the way is
never asked a question it cannot answer.

**The common cases, decided.** The rule is written so that these come out
right, and the reach tests of the change engine are these cases, over
collected rows, each in a flat and in a routed form:

| The network | Device | Answer, and why |
|---|---|---|
| Both radios of a link have a cable to the management network | either end | `direct`: each way back leaves by a cable, and the other radio has learned the two addresses on one side or not at all |
| The usual link: master at the NOC on a cable, slave across the link | the master | `direct`: its way back leaves by a cable, and the slave, where it can be read, has learned the master's address and its next hop's on its radio side, or not at all |
| | the slave | `through_link`: its way back leaves by its radio, and the master has learned the two addresses on different sides |
| A relay: link A from the NOC to mast T2; on T2 the master of link B | B's master, one flat management network | `behind_link` via A: A's radios have learned B's master and its gateway on different sides |
| | B's master, routed, the router on T2 in the inventory | `behind_link` via A: the walk goes from B's master to the T2 router, and from it to the NOC router across A, whose radios have learned the T2 router's interface toward the NOC and the NOC router's interface toward T2 on different sides |
| | B's master, routed, the router on T2 not in the inventory | `unknown`: "the router at <address> on the way from B's master to Nocturne is not in the inventory" |
| A master whose answers leave by a second uplink while Nocturne's requests arrive across its own link | that master | `unknown` at the first router of the walk that sends toward the master by another way than the answers came; where no router of the walk does, the retune's reach test (below) catches it |
| Nocturne on a subnet of its own behind a router that is not in the inventory | every device | `unknown`, naming that router: put it in the inventory |

**What Nocturne cannot see, it says.** A radio link it does not read
(another vendor's pair on the way to a tower), a radio whose device cannot be
read at that moment, a radio in a mode that hides the addresses behind its
station from its access point (the pseudo-bridge station modes) seen only
from the access point, and Nocturne's own host, whose routing it cannot read
(where the host's network has more than one router toward the radios, the
walk assumes the host sends through the one it ended at): none of these can
show a crossing, so none can stop a `direct`. Every `direct` answer lists
what was not seen ("not seen: <links>; if one of these is on the way,
Nocturne cannot tell"). What stands behind that case is the same for all of
them: the drill-proven revert on every written device, the retune's own reach
test, and probation, which counts a device that answers less often after the
move than before it (under "After a move").

**Nobody can state a reach.** There is no method by which a person declares
an `unknown` end `direct`: on a far master that would be the very move this
build refuses, entered by hand, and a survey made on such a statement would
have no revert behind it at all. The cure for an `unknown` is the missing
fact, which the answer names.

**A retune tests the reach it rests on** (the executor, below): a master
reached without its link answers while its link is down, and a master reached
through it cannot. The engine keeps the outcome of the latest test for each
master. A master whose latest test did not pass is not surveyed until a
retune passes it, and every plan to retune it shows that outcome to the
person who approves. The outcome decides nothing else: it does not change the
computed answer, and it is overwritten by the next test.

*What depends on a link.* The far end's device, always. Beyond it, from the
reads the plan made: every inventory device of the world whose MAC the
link's near end (the end whose own way back does not cross it) has learned
on its radio, or whose address it routes out of its radio, and every device
whose walk the plan computed crosses the link. For devices that are not in
the inventory Nocturne does not know, and says "and possibly others Nocturne
cannot see" instead of "none".

### Control: what can be done with each kind of link

**The first build moves a link only when its master is reached directly**,
as computed and never on anybody's say-so. A master reached through the
link, behind another link, or of unknown reach is read, and its link can be
surveyed from an end that is reached directly and recommended for, and is
never retuned by Nocturne. The reason is in "Not building": once such
a move is confirmed, nothing brings that master home if the new channel turns
bad that evening. Every row below is a person's: nothing in this build acts
on a link by itself.

| `control` | When | Can | Cannot |
|---|---|---|---|
| `both` | both ends manageable | read both; survey each end that is reached directly; prepare the slave's list; move a directly reached master; verify at both ends | survey an end reached through the link; move a master not reached directly |
| `master_only` | the slave is not in the inventory, unreachable, or read-only | read the master, and survey it when it is reached directly; read what the master reports of the slave; move a directly reached master to a frequency the person approving states the slave will find | read the slave's list, country, regulatory mode or power mode; survey the slave's end; write the slave |
| `slave_only` | the master is not manageable | read the slave; survey its end when reached directly; recommend, to whoever controls the master; add frequencies to the slave's list | **move the link.** A slave pointed where the master is not is a self-inflicted outage. |
| `none` | neither is writable | read, chart, alert | anything else |

## Monitoring (`nocturne-monitor`)

Link quality is read over the CLI on its own cadence, `radio_interval_s`
(default 300, minimum 60; 30 for lab devices, counted on the lab's clock), for devices whose profile
defines the kinds, **also for devices otherwise polled over SNMP**. A failed
read is a gap in the series, not zeros. A device the last read found to have
no radio stack is not asked again until its software version or its interface
list changes. While a radio change holds a device (below) the read is not
attempted and the gap says "a radio change is being applied".

*A lab device is read on its lab's clock.* The monitor has one clock for
its cadence and its stamps and knows nothing of a lab's time. For a device
with the lab marker it waits for the next radio read on
`connect::device_clock` instead, so a lab at eight times speed is read
eight times as often in real time and exactly as often in its own. Each
such read also writes `radio.clock_ms`, the lab's time at that sample,
beside the sample's ordinary stamp. Charts use the ordinary stamp.
Everything the radio feature measures on a lab link (durations, hours of
the day, days) it measures on `radio.clock_ms`. For a real device the two
are one clock and the series is not written.

It lives in the monitor because threshold rules judge the poller's own
measures: a series written by another feature would be charted and never
alerted on. `spec-qos.md` specifies the same shape for QoS, which is being
added to the monitor as this is written (`monitor.qos`); this piece starts
when that is on main, and follows it.

| Series | Instance | Meaning |
|---|---|---|
| `radio.freq_mhz` | `<interface>` | control frequency in use; a step in it is a move |
| `radio.width_mhz` | `<interface>` | channel width in use |
| `radio.noise_dbm` | `<interface>` | noise floor on the working channel, where the stack has one |
| `radio.peers` | `<interface>` | registered peers |
| `radio.signal_dbm` | `<interface>\|<peer mac>` | received from the peer |
| `radio.remote_signal_dbm` | same | what the peer reports receiving |
| `radio.snr_db` | same | where the stack has it |
| `radio.quality_tx_pct`, `radio.quality_rx_pct` | same | the platform's own quality figure where 100 is good (`quality_kind: ccq`), where the protocol gives it a meaning |
| `radio.per_pct` | same | a packet error rate, where 0 is good (`quality_kind: per`) |
| `radio.tx_rate_bps`, `radio.rx_rate_bps` | same | modulation rate in use, each direction |
| `radio.retry_pct` | same | `Δtx_retries / (Δtx_frames + Δtx_retries)` between two reads, only where the profile fills `tx_retries`; an interval in which the peer re-registered is thrown away |
| `radio.peer_uptime_s` | same | restarts at every drop: the cheapest drop detector there is |
| `radio.clock_ms` | `<interface>` | lab devices only: the lab's time at this sample |

| Method | Params → Result |
|---|---|
| `monitor.radio` | `{device_id?, interface?, problem_only?, limit?: 200, cursor?}` → `{rows: [RadioStatus], next_cursor?, coverage: {devices: [{device_id, state: ok\|unsupported\|no_radio\|failed\|held\|never, at?, unverified, error?}]}}` |

```rust
struct RadioStatus { device_id, interface, stack, role, state?, frequency_mhz?, width_mhz?, noise_dbm?,
    peers: [{ peer_mac, peer_name?, signal_dbm?, remote_signal_dbm?, snr_db?, quality_tx_pct?, quality_rx_pct?,
              tx_rate_bps?, rx_rate_bps?, retry_pct?, uptime_s? }],
    not_available: [String],      // figures this stack or protocol does not have
    at, unverified: bool }
```

Event: `monitor.radio.updated {device_id}`.

Built-in, editable rules:

* **"Radio link lost its peer"**: `radio.peers` below 1 for 120 s on an
  interface that has had a peer; severity `high`.
* **"Radio link quality is low"**: `radio.quality_tx_pct` below 60 for 900 s;
  `medium`. Only where the figure is available. An error rate is a different
  series and this rule never reads it: a healthy 60 GHz link has an error
  rate near zero.
* **"Radio link signal-to-noise is low"**: `radio.snr_db` below 15 for 900 s;
  `medium`.

There is deliberately no built-in rule on the noise floor alone: it is an
uncalibrated number that differs by chipset, and it misses the bursts that
actually hurt.

Alerts that need the link, not one radio, have one owner each, the one that
holds the facts, and both raise them through `alerts.raise` by name, keyed by
device and by guard name or link, and resolve them with `alerts.resolve` when
the condition ends:

* **The change engine**, about its own changes and its ledger: "a channel
  change was reverted" (`medium`), "the link did not come back"
  (`critical`), "an armed revert was left behind on <device>" and "a revert
  may still be armed on <device>, which cannot be read" (`high`; resolved by
  the read that closes the ledger entry). Only the engine knows which
  properties were set, when a guard is due and what the ledger holds.
* **The radio feature**, about what it derives from reads and history: "the
  link moved by itself", "this link sits on a radar channel and its slave
  does not look everywhere its master may go" (`medium`), "the link does
  worse on <channel> than it did on <channel>; the move back is planned"
  (`high`, from probation, linking the planned change), "the link is down on
  its new channel; the move back is planned" (`critical`), and "the model
  missed twice on this link" (`medium`).

A planned outage should not page anybody and should not block other links'
health checks. Before a radio change sends anything, the change engine opens
a maintenance window by name (`maintenance.save`) for the link's devices and
every device known to be behind the link, for the stated worst-case outage,
named after the change, and removes it at the end (`maintenance.delete`). If
the method is absent the plan says "alerts will fire during this outage".

Later: the same figures over SNMP, as a table mapping in the profile's
`snmp:` section. Not before the CLI path is proven, as for QoS.

## Surveys

A survey measures the channels a link is **not** on. It is its own kind of
operation because on the first-build platform it is an outage: the RouterOS
manual says of `scan` that "the interface operation is disabled (wireless
link is disconnected during the scan operation)", on a master and on a slave
alike.

**Rules of the first build.**

* A survey is a change (`Intent::RadioSurvey`): planned, its outage stated, a
  person approves that exact plan, its raw output kept in the change's
  record. It is never a collector step, a schedule, a playbook step or
  something the assistant can start, and it is never unattended.
* **Only an end reached directly is surveyed.** For an end reached through the
  link the manual does describe a form the device finishes by itself
  (`scan ... rounds=1 save-file=`: "disconnect the wireless link, does the
  scan ... saves the results to file, exits the scan and connects the wireless
  link back"), and Nocturne would learn from the near end's registration
  table that the far radio did not return. But the format of that file is not
  documented, so nothing could be read back; and if the radio did not return,
  nothing would bring it back, because a survey changes no configuration and
  so there is nothing for a revert to restore. That is an outage with a visit
  at the end of it and no measurement to show for it. Such an end is a
  **blind end**, said so on every screen, and every figure computed without
  it is labelled as seen from one end only. A master whose latest retune did
  not pass its reach test is not surveyed either, until a retune passes it:
  that test is the one piece of evidence that can contradict the computed
  reach (under "Which end is reached through the link").
* **A master whose channel is a radar channel, or whose radar status is
  unknown, is not surveyed.** After a scan such a master has to listen again
  before it transmits (remembered; the vendor documents nothing), for a time
  nobody has measured, and a detection during that wait sends it to a
  channel of its own choosing that the slave may not scan. Recording R6
  brings this back.
* A survey that failed is a recorded failure with the reason.
* **A channel that was never surveyed is unknown, not clean.** A channel
  surveyed at 03:00 is known at 03:00 and unknown at 21:00, and the record
  says which hours it covers.
* A survey says what it measured: `networks` (transmitters the radio could
  decode, with their strength: blind to everything else, and no airtime),
  `usage` (per channel, the share of time busy and a noise floor), `spectrum`
  (power per frequency).
* **What a `networks` survey covered is what can be shown that it walked.**
  The scan takes no frequency argument: it walks the interface's scan list.
  Coverage is the device's effective list where that read has been checked,
  and otherwise only the plain frequencies in the configured list. A list of
  `default`, or a named list, is "coverage not known": expanding it from a
  table would be the country table this feature does not have.
* So on a slave whose list was narrowed by hand to the one frequency in use,
  a survey covers one channel and every candidate is "never surveyed at the
  slave". The order is: **prepare** (add the frequencies to the slave's
  list), **survey**, **evaluate**; and the screen says that every entry added
  makes every later reconnection a little slower, for good.
* **A survey that could rank nothing is not planned.** The planner works
  out, by the rule above, the coverage the survey will be able to claim, and
  puts it in the plan. Where that is "not known", or only the frequency in
  use, the plan is refused with the reason: on a slave, "prepare the list
  first"; on a master, "this radio's list is a name, and what it walks
  cannot be shown on this software". An outage that can rank no channel is
  why the `wifi` survey and the through-link survey are out, and it is the
  same outage here.
* The plan of a survey says where else the other end could go meanwhile: a
  radio of the operator's that carries the same network name is listed
  ("the slave may join TOWER-C/wlan1 during the scan and stay there"), and
  for a slave Nocturne cannot read the plan says it does not know.

What the first build's survey measures:

| Stack | Command | Measures | Notes |
|---|---|---|---|
| `wireless` | `scan <if> rounds=1` | `networks` | Documented to disconnect, and to stop by itself with `rounds=`. Sees only what it can decode; an Nv2 network is seen only by RouterOS. How long a round takes is not documented. The airtime tool of this stack (`frequency-monitor`) is documented as three field names and nothing else; it becomes a second survey of this stack the day R6 shows how it behaves. The one scan documented to leave a link up (`background=yes`, 802.11 only) is not used: what it does when its conditions are not met is not documented. |
| `wifi`, `w60g` | none | | |

```rust
struct Survey {                          // one survey of one end; kept by the change engine in the target's result,
                                         // mirrored as a document by the radio feature
    id, link_id?, device_id, interface, end: master | slave,
    change_id,                           // the change that ran it: raw output, approval and transcript are there
    survey: String,                      // the profile's survey id
    measures: networks | usage | spectrum,
    started_at, finished_at,
    outage_s: Option<u32>,               // measured from the other end's peer uptime. None: could not be measured.
    covered: Coverage,                   // { known: bool, control_mhz: Vec<u32> }
    rows: Vec<SurveyRow>,
    networks: Vec<SurveyNetwork>,
    removed: Vec<String>,                // addresses left out because they are this link's own peer
    ok: bool, error: Option<String>,
    unverified: bool,                    // the parser has not met output recorded from a real device
}
struct SurveyRow { control_mhz, width_mhz?, busy_pct?, noise_floor_dbm?, networks?, max_signal_dbm?, avg_dbm?, peak_dbm? }
struct SurveyNetwork { address, ssid?, name?, control_mhz, width_mhz?, offset_mhz?, signal_dbm?, noise_floor_dbm?,
                       own: bool }       // one of the operator's own radios, matched by MAC against collected `radio` rows
```

`Survey`, `SurveyRow`, `SurveyNetwork` and `Channel` are types of
`nocturne-rf`. The drivers parse a survey's output to generic rows, as for any
command; the change engine turns them into these.

Network names and radio names in a survey were chosen by strangers. They are
data: shown to a person as text, handed to a model only inside a block marked
as untrusted device output, and never used to decide anything.

## The score (`nocturne-rf`, a new engine crate)

Pure arithmetic with no application knowledge: it depends on `serde` only,
reads no clock and no device. The radio feature uses it to rank channels.
The change engine uses the same code for two things: the function that says
which rows of a device's list a channel spans, which its radar and power
refusals read, and the recomputation of a proposal's figures from the
proposal's own input, which a plan shows beside the figures the proposer
claimed. That second user is why it is an engine and not a module of the
feature.

"Best" means: **the channel on which the link's worse direction carries what
the link needs, in its busiest hours, without hurting the operator's other
links, and which both radios are allowed to use and able to find.** Which
channel is best can be recomputed freely; whether to move is slow and
stubborn, and is a separate rule.

The function was worked out in the research for this feature and computed
by a reference calculator kept with that research (a short Python script;
neither is in this repository, and this file is the contract). It was
**changed where reviews showed it wrong**: what a scan of networks can say,
when to act, unknown read as "no", who chooses the evidence, what the
engine can recompute, where the lab flattered, and which statistic stands
for what a link achieves. Every such change is marked ▲. The numbers marked
*default* are policy, shown to the operator in every proposal and held as
settings or profile data, never as constants in code.

**The same answer on every machine.** ▲ The engine never calls the platform's
`log10` or `powf`, whose last digit is not the same everywhere. `lin` and `dB`
are its own two functions, built from addition, multiplication and division
(which IEEE 754 defines exactly), and every comparison with a threshold is
made on integer hundredths of a decibel: `round(100 * x) >= 100 * t`. A test
pins both functions bit for bit on a table of inputs.

### Inputs

Frequencies in MHz, powers in dBm. A candidate is `c = (f, W)`: centre
frequency and width. `lin(x) = 10^(x/10)`, `dB(y) = 10*log10(y)`.

**Per end `e`** (master `M`, slave `S`):

* `signal[e]`: the signal received from the peer, total over chains: ▲ the
  **median over the evidence window**, not one reading. Both the proposer and
  the change engine use that same statistic.
* `observed[e]`: whether this end has survey data.
* `survey[e]`: for each 5 MHz bin `k` (the bin is `[k, k+5)`): `q[k]` the
  quiet level, `p[k]` the busy level, `b[k]` the busy fraction in 0..1 **in
  the link's busy hours**, and ▲ a **state**: `measured`, `heard`,
  `nothing_decoded`, or (not in the survey's coverage) unknown.
* `own[e]`: the operator's other radios at the same site that are **not** in
  the same synchronised transmit phase: each a centre, a width, and the level
  it produces in this receiver when on the same channel (`coupled`).
* `now[e]`, for an unobserved end: quiet SNR, busy SNR and busy fraction on
  the current channel, and the width they were measured at.

From what a radio reports to the bins:

| The survey gives | q | p | b | state |
|---|---|---|---|---|
| `usage`: busy % and noise floor per channel, and the strongest level measured there | the noise floor, spread per bin | the strongest level the survey measured, spread per bin | the busy % | `measured`. A covered bin with a busy % of 0 is quiet: `q = p`, `b = 0`. ▲ A bin with a busy % above 0 and **no level** has a measured `b` and an invented `p` (noise floor + 10 dB, *default*): its state is `level_assumed`, it is treated as `heard`, and the evaluation lists "busy level at <end> on <channel>" under `assumed`. |
| `spectrum`: average and peak per bin | the lowest average seen on the band | the peak | `(lin(avg) - lin(q)) / (lin(peak) - lin(q))`, clipped to 0..1 | `measured` |
| `networks`: transmitters decoded, with strength, **no airtime** ▲ | the device's own noise floor on its working channel, spread per bin; `quiet_bin` where it reports none, and then the figure says "noise floor assumed" | where a network was decoded: its signal, spread per bin. Elsewhere `q`. | where a network was decoded: `assumed_busy` (*default 1.0*). Elsewhere 0. | `heard` where a network was decoded; `nothing_decoded` on every other covered bin |

"Spread per bin" is minus `10*log10(number of bins)`. Levels are only ever
compared within one radio.

▲ **The link's own peer is not an interferer.** Before a survey is binned,
every network whose address is the peer's (`peer_mac`, and every address of
the peer device's radios that the inventory knows, which covers virtual
access points on the master's radio) is taken out, and the survey records
which (`removed`). Left in, a slave's scan of its own channel hears its
master at the link's signal, scores a signal-to-noise of 0 dB, and makes
every other channel look like a gain (example N1). The operator's radios
heard from **another** site stay in.

▲ **What a `networks` survey may and may not say.** It can rule a channel
out: a transmitter was decoded there, this strong. It can never rule one in:
where it decoded nothing, other vendors' equipment, 5 and 10 MHz channels,
radar and anything that is not 802.11 may still be there. So the two states
are carried to the result, never merged into "quiet", and the **evidence
class** of a candidate at an end is the weakest state among the bins that
count for it (the bins inside the channel, and the outside bins that lend it
their busy fraction): `measured` over `heard` over `nothing_decoded`. What
the classes are allowed to do is under "When to move".

▲ **Several surveys of one end become one set of bins by the worst.** The
window holds surveys taken at different hours. Per bin, among the surveys
that cover it: where any is `measured`, the largest `b`, the largest `p` and
the largest `q` among the measured ones; otherwise the strongest network
heard; otherwise `nothing_decoded`. Never the newest and never the mean: a
figure from 03:00 must not stand for 21:00. And a candidate's class at an
end is `measured` only if at least one measured survey of its bins at that
end was **taken inside the link's busy hours** (the survey's own start time
against the busy hours stored with the evaluation). Otherwise it is
`measured_off_peak`: scored and shown, labelled "measured outside the busy
hours", and proposed to a person as a trial (row 2).

**Per link**: streams `nss`; the rate table of each radio's band; the need in
each direction; the candidates; for each candidate its radar status at each
end and the radar detections recorded on it at either site in the last 30
days.

### Three kinds of candidate ▲

Radar channels and other widths are "scored and shown with what they would
cost", so being scored and being a possible target are different things.
There are three kinds, and every screen shows all three.

**Cannot be scored**, with the reason: the evidence does not cover it at an
observed end ("never surveyed at <end>"); the radio's band has no rate table
for its width; an unobserved end has no present figures.

**Scored, and not a possible target**, each with its reasons, shown beside
its figures:

1. Its radar status at either end is `radar` or `unknown`, on **any** row of
   that end's list that its span covers (T12). (Decision: never the target
   of a change through this feature.)
2. It is not in the master's own allowed list, or that list could not be
   read, or its read has not been checked. Where the slave can be asked:
   the same for the slave's list.
3. The ends report different countries, or either is in a state the profile
   refuses.
4. Where the slave can be asked: it does not look there. The control
   frequency is not a plain number in its list (or in its effective list
   where that read is checked). The proposal says "prepare the slave first".
   ▲ Where the slave **cannot** be asked, nothing is known of its list:
   rules 2 and 4 judge the master alone, the candidate stays a possible
   target, carries `radio_slave_unseen`, and is always a trial (T9b, T11c).
   Rule 8 does not judge the master alone (below).
5. Its width or its layout differs from the link's present ones. (Decision:
   no width change.)
6. Excluded by the operator; in quarantine (a channel this link was sent
   home from in the last 30 days, *default*, by a revert or a probation that
   blamed the channel: `rollback.rs` says which do); licensed spectrum.
7. ▲ The link might not stay up there: predicted **quiet or busy** SNR at
   either end is under the SNR of the lowest modulation plus 10 dB
   (*default*). A channel that is fine when the neighbour sleeps and under
   the floor when he transmits is not a candidate.
8. ▲ Its permitted power is not known: a row of an end's list that it spans
   has no figure. Or the device does not bound its own power
   (`unbounded_power`) and the candidate's row figure is under the row figure
   of the channel in use. Both are rows of the same checked list. ▲ Where the
   slave cannot be asked, its power mode is not known and is taken as one
   that does not bound its power, because unknown is not "no": a candidate
   whose row figure **on the master's own list** is under the present
   channel's is not a target ("the slave's power mode and its list cannot be
   read"). A slave in a fixed-power mode that followed onto a lower limit
   would transmit above it. The
   radio's present transmit power is used nowhere: no read of it has been
   seen, and a term that is silently zero when a read is missing makes a
   move from a 30 dBm part of the band to a 23 dBm part look 7 dB better
   than it is.
9. The device marks it indoor-only and the radios do not both say they are
   installed indoors.

**Possible targets**: everything else. Only these are ranked for a move.

The current channel is always scored: it is the baseline.

### Need

```
need[d] = max(committed[d], p95_7days(traffic[d]) * headroom) / (airtime_share[d] * mac_efficiency)
```

for each direction `d` (down: master to slave; up: slave to master), as a
modulation rate. *Defaults*: `headroom = 1.5`, `mac_efficiency = 0.7`.
▲ `airtime_share[d]` is the device's own split where it reports one;
otherwise the direction's share of the traffic, `p95[d] / (p95[down] +
p95[up])`, held between 0.1 and 0.9; 0.5 only when there is no traffic
history. (A backhaul with 100 Mbit/s down and 10 up needed 428.6 down under a
flat half and was recorded as short while carrying everything asked of it;
by traffic it needs 238.1 down and 214.3 up: example N5.) Traffic is the
radio interface's own `if.*` rate series. With no traffic history and no
committed figure the need is unknown and `need[d] = ref`, the top rate of the
radio's table at the present width: the function then prefers capacity, and
every proposal for such a link says that its need is not known.

Capping at the need is what stops a link that has enough from being moved.

### Noise, signal-to-noise, rate

Constants, all profile or policy data; these are the *defaults*:

```
bin        = 5 MHz
quiet_bin  = -174 + 10*log10(5e6) + 6 = -101 dBm
snr_req    = [4, 7, 9, 12, 16, 20, 21, 22, 27, 29] dB       for modulation index 0..9
bits       = [0.5, 1, 1.5, 2, 3, 4, 4.5, 5, 6, 20/3]        data bits per subcarrier per stream
nsd        = {20: 52, 40: 108, 80: 234}                     data subcarriers
tsym       = 4 microseconds
rate(m, W) = nss * nsd[W] * bits[m] / tsym   Mbit/s         (0 when no modulation is met)
leak(gap)  = 20 dB if gap < 10 MHz; 28 dB if gap < 20; 40 dB if gap < 40; ignored from 40 MHz
```

`snr_req` is derived from the 802.11ac minimum sensitivities; a device's data
sheet is the better source and goes in the profile. `leak` is an assumption
shaped like the 802.11 transmit mask. A band whose table is `ht` stops at
index 7. `rate` reproduces the 802.11ac tables (two streams: 173.3, 360 and
780 Mbit/s at the top for 20, 40 and 80 MHz).

For end `e` and candidate `c` spanning `[lo, hi) = [f - W/2, f + W/2)`:

```
own(e,c)  = sum over own radios j at e's site, with gap_j = |f - f_j| - (W + W_j)/2, of
              lin(coupled_j) * min(1, overlap_MHz / W_j)     if gap_j < 0   (the channels overlap)
              lin(coupled_j - leak(gap_j))                   if 0 <= gap_j < 40
              0                                              otherwise

Nq(e,c)   = dB( sum over bins k inside c of lin(q[k])  +  own(e,c) )

Nb(e,c)   = dB( sum over bins k inside c of lin(max(p[k], q[k]))
              + sum over bins k outside c with gap_k < 40 of lin(p[k] - leak(gap_k))
              + own(e,c) )
            where gap_k = lo - (k + 5) for a bin below the channel, k - hi for a bin above it

busy(e,c) = the largest b[k] over bins inside c, and over outside bins whose leaked level
            p[k] - leak(gap_k) is at least quiet_bin

S(e,c)    = signal[e] - max(0, row(o, cur) - row(o, c))       o = the other end, which transmits
            row(o, x) = the lowest power figure among the rows of o's own list that x spans

SNRq = S - Nq          SNRb = S - Nb
mcs(x)    = the highest m with round(100 x) >= 100 snr_req[m]

R(e,c)    = (1 - busy) * rate(mcs(SNRq), W) + busy * rate(mcs(SNRb), W)
```

`R(e,c)` is the expected rate **of the direction received at `e`**. ▲ `S`
only ever allows for **less** power. A device lowers its power where its
list says it must; a higher limit elsewhere in the band is a limit, not a
setting, and nothing says the radio would use it, so it earns a candidate
nothing. The term is the difference of two rows of the list the device
printed (example N7); where a row has no figure the candidate is not a
target at all, and where the device does not bound its own power the term
is 0 and rule 8 decides. ▲ Where the transmitter's list cannot be read at
all (the slave, when only the master can be asked: the uplink's term), the
term is 0 and the evaluation lists "the slave's permitted power on
<channel>" under `assumed`, so the figure is never presented as a
measurement (T9b).

Three behaviours fall out of the arithmetic: a wider channel gains subcarriers
and collects more noise and every interferer in the added half, so wide and
dirty can lose to narrow and clean; energy just outside the channel counts,
attenuated; a channel that is very quiet most of the time and unusable for a
third of it scores as two thirds of a good channel, not as "average noise is
fine".

### Two ends, and an end that cannot be seen

```
down(c) = R(S, c)        the slave receives the downlink
up(c)   = R(M, c)        the master receives the uplink

Q(c)    = min( min(down(c), need[down]) / need[down],
               min(up(c),   need[up])   / need[up]   )           0 <= Q <= 1
```

`Q` is the share of its need that the worse direction would get. Where the
evidence class is `measured` at every observed end it is a prediction. Where
it is `heard` it is "what this channel would give if every network the scan
decoded were transmitting all the time and nothing else were there". Where it
is `nothing_decoded` it is "not ruled out": the most the channel can be from
what is known (own radios, neighbours' leakage), and silent about anything
the scan cannot decode. Every screen prints the figure with those words, not
bare.

> An end that was not observed is assumed to be **no better on any candidate
> than it is now.**

`R(blind end, c)` uses that end's present quiet SNR, busy SNR and busy
fraction for every candidate, less `10*log10(W / W_now)` when the candidate
is wider. If the bottleneck is the direction received at the blind end, no
candidate beats the current channel, the answer is "stay", and the report
says the problem is at the end that cannot be seen. If the bottleneck is at
the visible end a candidate can win, the required gain doubles, and the move
is a trial that only a person can approve: "no better than now" is an
assumption.

How the three present figures of a blind end are taken from passive reads is
this file's definition and needs checking against real links: **quiet SNR**
is the 90th percentile of that end's SNR over the evidence window; **busy
fraction** is the share of busy-hour samples in which the direction's
modulation rate is more than one step below what the quiet SNR supports;
**busy SNR** is `snr_req` of the median modulation over those samples. Where
the stack reports no SNR and no noise floor at that end, the end has no
present figures and nothing can be scored.

### The operator's other links: `harm`

```
harm(c) = sum over the operator's other links j that share a site with this link of
            max(0, Q_j(now) - Q_j(with this link on c))

A(c)    = Q(c) * k_dfs(c) - harm(c)
k_dfs   = 1 - 0.05 * [c is a radar channel] - 0.05 * min(radar detections on c in 30 days, 5)
```

`Q_j(with this link on c)` is link j's own `Q` recomputed with this link's
radio entered in its `own` list at the candidate. `k_dfs` is what a radar
channel "would cost"; it prices a candidate that is shown and never chosen.

Rank the possible targets by `A(c)`, highest first. Among candidates within
0.001 of each other prefer, in order: the current channel; the lower
`harm`; ▲ a channel on which this link's own history shows it achieved its
need; the wider gap in MHz to the nearest network any survey heard and to
the nearest of the operator's own radios; and only then the lower
frequency. Where the tie is among candidates on which nothing was decoded,
the screen says "equal as far as the scan can tell" instead of a rank: with
a `networks` survey every such channel scores the same, and a rule that
sent each tie to the lowest frequency would propose one channel to every
link in a town.

### When to move

Let `cur` be the current channel, `best` the highest-ranked other possible
target, and `Qach(cur)` what the link **achieves** on its current channel in
its busy hours: ▲ per hour, the **mean** over that hour's samples of the
worse direction's modulation rate against its need, each sample capped at
1; then the median over the busy hours of the window. The mean, because it
is the statistic `R` predicts (a share of the time at one rate, the rest at
another). A median calls a link that is degraded for 40% of its busy hours
perfect, and one degraded for 60% as bad as if it never recovered, so row 4
below could not hold on the very links this feature is for (example N6).
It is unknown where the stack reports no rates.

```
base     = max( A(cur), k_dfs(cur) * Qach(cur) )      A(cur) alone when Qach is unknown        ▲
gain     = A(best) - base
required = 0.10 * (2 if an end is blind)
           (shown as 0.20 for a radar channel: what it would have had to clear)

qualifies today  =  gain >= required  AND  A(best) >= 1.20 * base
```

▲ The baseline is the better of what the surveys predict for the current
channel and what the link really does there. With occupancy assumed, a faint
network that transmits one percent of the time makes the current channel
score low while the link runs at its top rate; measured from the prediction
alone, every such link "gains" by moving, once a week (example N3).

In this build nothing acts on that by itself: the outcome is what a person
reads. When the gain qualifies, the evaluation is a **proposal**, and the
rows below say how far its evidence goes. Each row that does not hold is
printed with the proposal (`held_by`); rows 2 and 3 make it a **trial**. A
proposal with nothing held is one whose evidence meets every rule this
build has, and it is still a person's to approve.

| # | Condition | Rule (*defaults*) |
|---|---|---|
| 1 | ▲ Sufficient | If `Qach(cur) >= 0.95`, the link has what it needs: **stay**, and no proposal either. |
| 2 | ▲ Measured | The evidence class is `measured` at both ends, for the current channel and for `best`. A `heard` or `nothing_decoded` candidate is proposed as a **trial**. |
| 3 | ▲ Both ends seen | Neither end is blind. A one-ended result is a trial. |
| 4 | ▲ Explained, both ways | `A(cur)` and `Qach(cur)` are within 0.15 of each other. If the surveys predict the current channel much better than it does, the cause is something they did not see, or is not the channel. If they predict it much worse, they are counting something that does not hurt. Either way the model is wrong about this link, and the proposal says so with the finding. `Qach` unknown: not explained. |
| 5 | ▲ Persistence | Surveys of `best` at each end on at least 3 different days within the last 5 (`persistence.days`, `persistence.window_days`), at least one of them at each end inside the link's busy hours, and `qualifies today` with the same `best` in the evaluation made when each was stored. **Days that have a fresh survey are counted, not evaluations**: four evaluations of one survey agree with each other and prove nothing. |
| 6 | Coverage | The window (7 days) holds passive observations of the current channel in the link's busy hours on at least 3 different days at each end. |
| 7 | ▲ Fresh | The newest survey of `best` at each end is at most 72 hours old, and no survey the gain rests on is outside the 5-day window. A neighbour who also optimises at night has moved by next week. |

The link's busy hours are the hours of the week in which its own traffic is
in its top quarter; with no traffic history, 19:00 to 23:00 on the machine's
local clock.

"Stay" is the common, healthy answer.

**What this means on a real RouterOS link.** The first-build survey decodes
networks and measures no airtime, so row 2 never holds there and every
proposal is a trial. And only an end reached directly is surveyed, so on the
usual link, whose slave is across it, row 3 does not hold either. Every
proposal says both. Neither stops a person from approving it: a trial is the
move a person makes by hand today, with the evidence and its gaps on one
page.

**After a move.** *Verification* takes minutes and belongs to the pipeline.
*Probation* takes 48 hours and at least one full busy period, and compares
the link's achieved `Q` on the new channel with its own record on the old
channel, hour of the week for hour of the week.

* ▲ Hours in which the signal at either end was more than 5 dB under its
  median are left out: rain and ducting are not the channel. If fewer than 3
  comparable busy hours remain, probation runs on, to 7 days at most, and
  then ends "undecided": the link stays, and a person is told.
* ▲ Probation may end **early**, with "worse", as soon as one full busy
  period holds 3 comparable busy hours and the new channel is worse in them
  by more than the margin. A channel plainly broken on its first evening
  does not get a second.
* If the new channel is worse by more than 0.05, the link should go home.
  The feature plans the move back at once (a retune to the channel the move
  left; the plan marks it `home`) and raises an alert that says what
  probation saw and links that planned change; a person approves it, and the
  master is reached directly, so it can be applied at once. The candidate is
  quarantined for 30 days. A return starts no probation of its own: a link
  that went home and still does badly is a link for a person.
* ▲ A link that keeps dropping, or is down, on its new channel raises its
  alert at once (`critical` when it is down) with the move back already
  planned. A link that is down is planned with no peer registered (`plan.rs`,
  step 4), and the typed confirmation says what that means. Nothing moves it
  by itself: that would be the reactive mode under another name.
* ▲ Probation watches the neighbours too: every link that shares a site with
  either end, and every link with a device that the move's plan listed as
  behind this link (`radio_only_path`). A drop of more than 0.05 in any of
  them against its own record counts as worse for the moved link. The victim
  of a move is often another link.
* ▲ **A device that answers less often counts.** For the master, the slave,
  and every device the move's plan listed as behind the link: a busy hour in
  which it lost more of what the monitor sends it than in the same hour of the
  week before the move (its `icmp.loss_pct` where the monitor pings it; for
  the two radios, the share of their radio reads that failed), *default* any
  loss where there was none, counts for the moved link as worse than the old
  channel. A victim Nocturne does not read (another vendor's pair on the way
  to the mast) shows itself this way and no other, and it need not fall
  silent for a whole hour to be counted: a link that collapses in bursts
  loses reads in bursts.
* ▲ A link that moved **off a radar channel** cannot go home: home is a radar
  channel, and those are never a target. Its plan says so before it is
  approved, and a probation that goes badly ends in a proposal for another
  channel or in a finding.
* During probation the feature proposes nothing else at either site or on a
  link on the path.
* ▲ A return is a **miss**. So is a move whose realised gain (`Qach` after,
  less `base` before) is under half the gain predicted. After two misses on a
  link the feature raises the finding `model_missed`, and every later
  proposal for that link is a trial for that reason. (Comparing absolute `Q`
  after the move with the prediction would pass moves that gained nothing
  whenever the predictions are pessimistic.)
* ▲ **A timer nobody measured is not the channel's fault.** A move that was
  reverted because the slave did not follow inside a deadline that was an
  estimate (`measured: false`) quarantines nothing. The record says "the
  follow time of this link has never been measured and the estimate may be
  short"; if the link came home, the way home measured how long the slave
  took to find its master, and the next plan uses that (`plan.rs`, Timers).
  A return that fails never quarantines home.

**A degraded link is not an invitation.** When a link is failing right now
the control path through it is at its least reliable. A degraded link
produces an alert and a ready proposal for a person. There is no "reactive"
mode.

### The worked examples are the test list

Computed by the method research's reference calculator (a short Python
script kept with the research, not in this repository and not project code),
with the default constants, two streams, quiet bins at -101 dBm. The changed
and new cases were computed with the same calculator plus the changes marked
▲. Every number a test needs is in the tables below. `interferer(f, W, P, b)` is a transmitter centred on `f`, `W`
wide, total received power `P` dBm, busy fraction `b`, spread evenly over its
bins; `network(f, W, P)` is the same thing as a `networks` survey reports it:
no `b`. Rates in Mbit/s. The engine's tests assert `Q` and `A` to three
decimals and rates to one.

A note in italics that begins *Was:* says what the first version of this
method did with the same figures, for a reader who has an old proposal in
hand.

**As the reference calculator computes them, unchanged:**

| # | Setup | Expected |
|---|---|---|
| T1 | Signal -55 at both ends; current 5755/40; `usage` surveys; master hears `interferer(5745, 20, -70, 0.60)`; slave clean; candidate 5795/40. | 5755/40: down 360.0, up 208.8, Q 0.580. 5795/40: 360.0 / 360.0, Q 1.000. With `Qach` unknown: gain 0.420, **qualifies today**. |
| T5 | As T4 (below) with a need of 150 each way. | Q 1.000 and 1.000, gain 0. **Stay.** |
| T9a | Master only. Master clean. The downlink, received at the unseen slave: quiet SNR 37, busy SNR 11, busy fraction 0.5. Current 5755/40, candidate 5795/40. | Both: down 220.5, up 360.0, Q 0.613. Gain 0. **Stay**, finding `blind_bottleneck`. |
| T13 | The device is choosing its own channel. | Nothing is moved; each move the device makes is recorded; the ranking is shown beside the device's choice. |
| T3 | Signal -55 both ends; chains differ by 9 dB at the slave; surveys clean everywhere; downlink achieves 45% of what its signal supports in every hour of the week. | Predicted Q 1.000 everywhere, gain 0. **Stay**, finding `not_the_channel`. Variant: 45% only between 19:00 and 23:00, surveys taken at night: `evidence_insufficient`, never "clean". |

**Same figures, different outcome or reason** (the arithmetic of `Q` and `A`
is the calculator's; what is done with it changed):

| # | Setup | Expected, and what changed |
|---|---|---|
| T1a | T1, and the link achieves 0.60 in its busy hours. | base 0.600, gain 0.400: qualifies today. *New input.* |
| T1b | T1, achieving 0.97. | **Stay**: sufficient (row 1). *New.* |
| T1c | T1, achieving 0.80. | Gain 0.200 qualifies, and prediction 0.580 is 0.220 from achieved: **proposal**, finding `surveys_overstate`: not explained (row 4). *New.* |
| T2 | As T1 but the master hears `interferer(5745, 20, -74, 0.20)` and the slave hears `interferer(5795, 40, -61, 0.80)`; candidates 5795/40 and 5510/40 (radar). | 5755/40: 360.0 / 320.4, Q 0.890. 5795/40: down 93.6, Q 0.260, **not a possible target**: busy SNR 6.0 dB at the slave. 5510/40: Q 1.000, A 0.950, shown, **never a target**. No possible target: **stay**. From the master's survey alone 5795/40 scores 1.000 and is a possible target: the test asserts that too. *Was: scored 0.260 and lost on gain.* |
| T4 | Signal -58; current 5775/80; master clean; slave hears `interferer(5755, 40, -68, 0.90)`; candidate 5795/40; `ref` 780. | 5775/80: down 235.9, up 780.0, Q 0.302. 5795/40: 360.0 / 360.0, Q 0.462; would gain 0.159 at a ratio of 1.53. **Stay**, finding `narrower_would_serve`: a different width is shown and is not a target. *Was: move.* |
| T6 | Two of the operator's links have their masters on one mast. L1 on 5745/20 is evaluated; L2 on 5785/20 couples -48 dBm into L1's receiver; not synchronised. L1's master survey: `interferer(5745, 20, -73, 0.70)`, `interferer(5805, 20, -80, 0.30)`, `interferer(5825, 20, -80, 0.10)`. Candidates 5765, 5805, 5825 at 20 MHz. | Without L2 in the model: 5745 Q 0.615; 5765 Q 1.000; 5805 Q 0.925; 5825 Q 0.975: it picks 5765, beside L2. With L2: 5745 A 0.615; 5765 and 5805 Q 0.300, harm 0.700, A -0.400, **and not possible targets** (quiet SNR 13.0 dB at the master); 5825 Q 0.975, harm 0, A 0.975. Best 5825/20, gain 0.360. Variant: L1 and L2 declared synchronised: the term drops and 5765/20 is right. *Added: the margin rule also excludes them.* |
| T7a | Signal -55; slave clean; current 5755/40; master hears `interferer(5755, 40, -70, 0.40)` and `interferer(5795, 40, -66, 0.30)`; candidates 5795/40 and 5510/40 (radar, one detection in 30 days). | A: 0.720, 0.690, 0.900. 5795/40 is not a possible target (busy SNR 11.0 dB at the master). 5510/40 is shown: it would gain 0.180 against the 0.20 a radar channel would have had to clear. **Stay.** |
| T7b | As T7a with busy fractions 0.80 and 0.70 and no radar history. | A: 0.440, 0.380, 0.950. 5510/40 would gain 0.510. **Stay**, finding `best_is_radar`: the best channel is a radar channel; it is shown with what it would cost and is not offered. *Was: move to 5510/40.* |
| T9b | Master only. Master hears `interferer(5755, 40, -65, 0.70)`; the slave's present figures are clean; the master's own list gives 5755 and 5795 the same power figure. | 5755/40: up 164.7, Q 0.458. 5795/40: Q 1.000, with "the slave's permitted power on 5795/40" under `assumed` (the uplink's power term is 0: the slave's list cannot be read). Gain 0.542, required 0.20. **Proposal, as a trial**, to a frequency a person states the slave will find. Variant: the master's list gives 5795 a lower figure than 5755: 5795/40 is not a possible target ("the slave's power mode and its list cannot be read"). *Was: move as a trial.* |

**Changed in substance:**

| # | Setup | Expected |
|---|---|---|
| T8 | Evaluations of T1's link. (a) Four evaluations on four days, all resting on one survey of 5795/40: gains 0.42 each. (b) Surveys of 5795/40 at both ends on three different days inside five, one at each end in the busy hours: gains 0.42, 0.40, 0.38. (c) As (b), and the busy-hour survey shows the candidate occupied: by the worst-per-bin rule the gain is 0.03. (d) As (b) with all three surveys taken at night. | (a) **Proposal**, held by row 5: one survey counted four times. (b) A proposal with nothing held when the third is stored. (c) Back to observing. (d) Proposal, a trial: "measured outside the busy hours". *Was: three of four evaluations, whatever they rested on.* |
| T11 | (a) Both ends; slave's list is `5745,5755,5765`; candidate control 5785. (b) Slave's list is `default`. (c) Master only, nothing stated about the slave. (d) The ends report different countries. (e) Candidate on the master's allowed list and not the slave's. | (a), (b) Not a possible target: "the slave does not look there; prepare it first". After a `RadioPrepare` has been applied the list reads `5745,5755,5765,5785` in (a) and `default,5745,5785` in (b): possible. (c) Proposal only, with `radio_slave_unseen`. (d) Nothing is a possible target; the mismatch is the report. (e) Not a possible target. |
| T12 | The device's own list. (a) A 40 MHz candidate whose two 20 MHz rows are both marked not radar. (b) One of its rows is marked radar. (c) One of its rows carries no mark. (d) A row with no power figure. (e) Spans are half-open: a channel that ends at 5650 does not include the row at 5660. (f) The current channel is radar and no wait has been measured on this link. | (a) Possible. (b), (c) Never a target: "radar channel" and "radar status unknown". (d) Not a possible target: permitted power not known. (f) A move off it is possible; its plan gives the way home 600 s plus twice the follow time before "did not come back" may be said, and says the link cannot be sent home after it is confirmed. *Was: a table of waits for radar targets.* |
| T14 | See "`rollback.rs`": the table there is the test. | |

**New: a `networks`-only survey, which is all the first-build platform has:**

| # | Setup | Expected |
|---|---|---|
| N1 | T1's link, seen by `scan`. The master's survey decodes `network(5745, 20, -70)`. The slave's survey decodes its own master, `network(5755, 40, -55)`, and nothing else. Candidate 5795/40: nothing decoded at either end. The link achieves 0.580 in its busy hours. | With the master left in the slave's survey: slave busy SNR 0.0 dB, down 0.0, Q(cur) 0.000: **the test asserts this is what must not happen.** With it removed: 5755/40 up 108.0, Q 0.300, class `heard` at the master. 5795/40: 1.000, class `nothing_decoded`: "not ruled out". base = max(0.300, 0.580) = 0.580; gain 0.420. **Proposal, as a trial** (row 2). |
| N2 | T2's link, seen by `scan`: master decodes `network(5745, 20, -74)`; slave decodes `network(5795, 40, -61)`. | 5755/40: up 162.0, Q 0.450 ("if the network heard were always transmitting"). 5795/40: down 27.0, Q 0.075, **ruled out** and not a possible target (busy SNR 6.0 dB at the slave). From the master's survey alone: "not ruled out". |
| N3 | A link that has what it needs. 20 MHz, signal -55 at both ends, need 140 each way. The master decodes an idle neighbour, `network(5745, 20, -75.5)`, on the current channel 5745/20, and `network(5825, 20, -83)` on the candidate. The link achieves 1.000 in its busy hours. | 5745/20: busy SNR 20.5, up 104.0, Q 0.743. 5825/20: busy SNR 28.0, up 156.0, Q 1.000. From the surveys alone: gain 0.257 at a ratio of 1.35, which a rule that reads only the surveys calls a move. base = 1.000, gain 0.000, and row 1: **stay**, no proposal. |
| N4 | The slave's list is `5755`. Its survey covers 5755. | Every other candidate: cannot be scored, "never surveyed at the slave". Finding `survey_covered_one_channel`, with the order: prepare, survey, evaluate. With the list `default`: coverage not known, and the same finding. |
| N5 | Need. Traffic 100 Mbit/s down and 10 up at the 95th percentile, nothing committed, no split reported by the device. | Shares 0.9 and 0.1 (held at the bounds): need 238.1 down, 214.3 up. With 40 and 40: 171.4 each way. |
| N6 | `Qach`. T1's link sampled through one busy hour: the uplink at 360 in 40% of the samples and at 108 in 60%; need 360. | The mean of the capped ratios is 0.580, which is what `R` predicts for T1: row 4 holds. The median modulation would give 0.300, and row 4 would fail on perfect evidence. With the shares the other way round (108 for 40%): 0.720 by the mean, where the median would say 1.000, "sufficient", of a link degraded for 40% of its busy hours. |
| N7 | Power. The transmitter's own list gives 30 dBm on the row of the channel in use and 23 on the candidate's; signal -55. | `S` on the candidate is -62. With 23 and 30 the other way round: -55, a higher limit earns nothing. With no figure on the candidate's row: not a possible target. |

The header of the engine's test module says what these tests prove: that the
Rust equals the calculator written from the same formulas. They do not prove
that the model fits a link; only moves that were predicted and then measured
do that.

T1, T1a to T1c, T2, T4 to T7, T9, T12 and N1 to N7 are tests of `nocturne-rf`
alone. T3, T8, T11 and T13 are tests of the radio feature with a paused
clock. T14 is a test of the change engine. (T10, the engine's dwell and caps
for an automatic move, left with automatic changes.)

## The recommendation (`nocturne-radio`)

Once a day per link (`evaluate_at`), whenever a survey of the link is stored,
and on request, the feature gathers the reads, the surveys and the history,
runs the engine, and stores an evaluation. An evaluation changes nothing on any device. One that failed is
stored as a failure and is "the latest": an old success is never shown as
current.

```rust
struct Evaluation {
    id, link_id, at,
    ok: bool, error: Option<String>,
    input: EngineInput,                      // everything the engine was given: an evaluation can be recomputed from it
    input_hash: String,
    basis: Basis,
    need: { down_bps?, up_bps?, source: committed | traffic | unknown },
    achieved: Option<f64>,                   // Qach(cur); None where the stack reports no rates
    current: Scored,
    candidates: Vec<Scored>,                 // every scored candidate, possible targets first, ranked
    not_scored: Vec<{ channel, reason }>,
    decision: stay | proposal | held,
    best: Option<Channel>, gain: Option<f64>, required: Option<f64>,
    trial: bool,
    held_by: Vec<String>,                    // each row of "when to move" that did not hold
    findings: Vec<Finding>,
}
struct Basis {
    ends: { master: observed | blind | unreachable, slave: observed | blind | unreachable | not_in_inventory },
    one_sided: bool,                         // true whenever either end is not `observed`
    surveys: Vec<{ end, survey_id, at, hours: busy | quiet | other, measures, coverage_known: bool, unverified }>,
    passive: { window_days, busy_hour_days, samples, from: monitor | none },
    assumed: Vec<String>,                    // every measurement an assumption stood in for: "occupancy at the master",
                                             // "noise floor at the slave", "coupling of TOWER-A/wlan2", "the slave end is no better than now"
    not_available: Vec<String>,              // figures the stacks and protocols involved cannot give
    constants: Map,                          // every default that went into the numbers
    unverified: Vec<String>,                 // reads and operations in the chain that have not met a real device
}
struct Scored { channel, radar: radar | not_radar | unknown, down_bps, up_bps, q, harm, a,
                evidence: { master: measured | measured_off_peak | heard | nothing_decoded | no_better_than_now, slave: (the same) },
                not_a_target: Vec<String> }  // empty: a possible target
struct Finding { kind, text }
```

What an evaluation must say, because nothing is invented:

* **What it was computed from and when**: each survey with the time it was
  taken (the age is computed when it is shown, never stored) and the hours it
  covers; the passive window; which ends were seen.
* **One end only**, in words, whenever `one_sided`: "computed from the master
  end only; the slave end is assumed no better on any channel than it is
  now".
* **Unknown, not clean**: a candidate with no survey at an observed end is in
  `not_scored`, never in `candidates`.
* **Blind to what it cannot decode**, wherever a `networks` survey is in the
  basis, and per candidate which of "heard" or "nothing decoded" applies.
* **Every assumption by name** (`assumed`). A proposal from an evaluation
  with a non-empty `assumed` is a trial.
* **A trial is called a trial.**

Everywhere a recommendation travels without its evaluation (the link list,
the `radio.evaluated` event, what the assistant is told), `best` and `gain`
travel with `{one_sided, assumed: bool, trial, unverified, at}`. A proposal
made from an evaluation more than 24 hours old is recomputed first.

Findings are the cases where the right answer is a sentence, not a channel:

| `kind` | When | Says |
|---|---|---|
| `not_the_channel` | a direction achieves much less than its signal supports at all hours, on a channel the surveys call clean; or chains differ by more than 5 dB | "alignment, polarisation or hardware; a channel change will not cure this" (T3) |
| `surveys_overstate` | the surveys predict the current channel more than 0.15 worse than the link does on it | "the surveys count something that is not hurting this link" (T1c, N3) |
| `evidence_insufficient` | the link does badly in busy hours and the only surveys are from quiet hours | what a busy-hour survey would cost, and that it needs a person's approval |
| `blind_bottleneck` | the worse direction is received at the end that cannot be surveyed | which end, and why it cannot be seen (T9a) |
| `slave_cannot_follow` | the best channel is not in the slave's list | "prepare the slave first", or, master only, what a person must state |
| `survey_covered_one_channel` | a survey's coverage is one frequency, or not known | the order: prepare, survey, evaluate (N4) |
| `best_is_radar`, `narrower_would_serve` | the best-scoring channel is one Nocturne will not move to | the figures, and why it is shown and not offered (T7b, T4) |
| `radar_exit_uncovered` | the link sits on a radar channel | whether the slave's list covers every frequency the master may choose after a detection: covered, not covered (with the missing frequencies), or "cannot be told: the effective lists have no checked read". That link is one radar pulse from an outage today, with no change of Nocturne's involved. |
| `country_mismatch`, `no_country`, `test_mode`, `outside_own_list` | the ends disagree on the country; a device reports none; a device is in a conformance-testing mode or does not bound its power; a device is on a frequency it does not list | the facts. Nothing is offered and nothing is "fixed". |
| `device_chooses` | `auto_channel` or `managed_by` at either end | Nocturne will watch and not move this link (T13) |
| `moved_by_itself` | the frequency changed and no change of Nocturne's explains it | from, to, when; radar if the device said so. The link is **not** pushed back. |
| `master_not_direct` | the master's reach is not `direct`: through its link, behind another link, or unknown | which of them, with the step, the link and the missing fact; Nocturne recommends and does not move this link in this build |
| `reach_test_failed` | the latest retune of the master did not pass its reach test | what was seen, and that the master is not surveyed until a retune passes the test |
| `model_missed` | two moves of this link missed (a return, or under half the predicted gain) | the two moves; every later proposal for this link is a trial |
| `colocated_unknown` | either end's `site` is empty | the operator's other radios at that site are not in the model, and the harm term is computed without them |
| `licensed`, `not_point_to_point`, `split_frequency` | | why this link is out of scope |

The link's `state`: observing → proposal → executing → probation →
settled, with a return to observing after a move home, and `held` from
anywhere with its reason. Every transition is an event.

The feature's background task (evaluate, watch probation, plan the move back
when probation says worse, raise the alerts the feature owns) is visible:
`radio.status` says what it has planned, for which link and why. It runs
while `radio.enabled` is on and stops at shutdown. It applies nothing: no
method it can call applies, approves or rolls back a change (`calls.rs`).

## Changing a channel (`nocturne-changes`)

There is no write path outside the pipeline. A retune is a change: plan, risk
and protected resources, approval of that exact plan, backup, dead-man, send,
verify, roll back. What is new is that the pipeline has to understand what a
radio change is, because the generic path cannot carry one safely:

* a RouterOS change cannot be predicted by the planner today, so it comes out
  `critical`, fails its configuration check and is rolled back (read in the
  code; no test runs it);
* a session that is lost and comes back is always rolled back;
* verification is one look after a fixed sleep, on one device;
* RouterOS has no dead-man in the profile, and the existing guard shape
  cannot hold one.

None of that is worked around. Each is changed inside `nocturne-changes`, for
typed radio targets only. A `Lines` or `Intended` change behaves exactly as
it does today, and that is made true by construction: the RouterOS profile's
`config.guard` stays `null` (the radio revert lives in `radio.guard`, which
`guard_plan` never reads), and the existing phases are split out of
`apply_device` in a commit that changes no behaviour and no existing test
file. The generic path reads the `radio:` section for four things, each of
which can only refuse or leave out: the two protected kinds for typed lines
that touch a radio; the refusal to plan on a device that holds a revert of
Nocturne's; the filter that keeps a derived rollback from re-creating one
(these under "The dead-man switch"); and the rule that keeps every derived
rollback off a radio's own line and off a revert's entry (under
"`rollback.rs`").

### Typed targets

```rust
enum Intent {
    // ... Lines, Intended as today
    /// Slave: make sure it will look on these control frequencies. Adds; never removes.
    RadioPrepare { interface, peer_mac: String, add: Vec<u32> },
    /// Master: move to this channel.
    RadioRetune { interface, peer_mac: String, to: Channel },
    /// Either end: run the profile's survey `survey` on this radio.
    RadioSurvey { interface, survey: String },
    /// Either end: arm the dead-man with a revert that changes nothing, let it fire, and check what it did and when.
    RadioGuardDrill { interface },
    /// Either end: remove a revert of Nocturne's that was left behind and did not fire.
    RadioGuardRemove { name: String },
}
struct Channel { control_mhz: u32, width_mhz: u32, offset_mhz: i32 }   // centre = control + offset
```

There is no intent that takes an entry out of a slave's list ("Not
building").

Rules that make whole classes of mistake impossible:

* **One written device per change.** A change that contains a radio intent
  contains exactly one target. Preparing the slave and moving the master are
  two changes, so a retune never holds a line for the slave, a revert armed
  on the slave is gone (and read back as gone) before a retune can even be
  planned, and nothing a retune does can take a frequency out of a slave's
  list. The other end of the link takes part in a retune as a **witness**: it
  is read, over policy-checked reads, and never written.
* **No caller options.** A change with a radio intent has the engine's fixed
  options (no parallel lanes, stop on failure, verification on, the guard
  always). `changes.create` and `changes.update` refuse any `options` that
  differ, and the planner refuses them again (an update puts a change back
  to draft, and the plan is where a draft is next judged). The fixed set is
  in the plan hash, and nothing a caller sends can switch the dead-man off.
* **The commands are never supplied by the caller.** The planner builds them
  from the profile's templates and the values it reads from the device, so a
  radio change can contain a frequency line, a scan-list line, the dead-man's
  own lines, or a survey command, and nothing else.
* **One world per change.** The written device and the witness are in the
  same world: both lab devices of one lab, or both real devices. A plan whose
  witness is in another world is refused.
* **One radio change at a time in a world.** The engine takes the world's
  lock before a radio change sends anything and holds it to the end; a
  second one is told what is in flight. A night holds a dozen moves at most,
  and the rule removes every dependence on two site names being spelled
  alike, on two jobs starting in the same second, and on a survey upstream
  of a move. **Each lab is a world of its own with its own lock**: a change
  on a lab somebody paused must not hold the lock that a real link's way home
  is waiting for.
* **The ledger.** Before the engine sends the line that arms a revert it
  writes a ledger entry: the device, **the MAC of its radio**, the guard's
  name, the change, what the revert would restore (property and value), when
  it was armed and when it is due, all from the plan, never from device text.
  One thing closes an entry: a read of that device that shows no entry of that
  name, after a disarm or after it fired. While an entry is open, every radio
  plan that names that device as the written device **or as the witness**, or
  whose `peer_mac`, registered peer or witness has that radio's MAC, is
  refused, whether or not the device can be asked at that moment, and whether
  or not it is still in the inventory under that id or any other: a slave
  deleted and added again, or a link paired again to a bare MAC, is still the
  radio the entry names. The only exception is the change that removes that
  revert. So a revert that may still be armed is known from the engine's own
  record, and not only from a read that a device without power cannot answer.
  The engine makes the closing read whenever a radio plan is attempted, and on
  `changes.guards.check`, which the radio feature calls after every
  successful monitor read of a device the ledger names; so an entry whose
  device was dark is closed by the first read after its revert has run.
* **Both devices are held.** For the length of a radio change the engine
  holds the device it writes and the witness (`connect::hold`): any other
  automated session to them (the monitor's read, a backup, a collector) is
  refused at once with "a radio change is being applied", instead of queueing
  for the device's one session slot and taking it at the moment the link
  returns. No backup can be taken while a radio change holds the device. (A
  revert left on a device after its change ended, which a nightly backup can
  store, is covered by the three rules under "The dead-man switch": a stored
  version that holds one cannot arm it again.)
* **The link's clock.** Every wait, deadline and count of a radio change is
  measured on the clock of the devices
  (`connect::device_clock`): Nocturne's own for real devices, the lab's
  simulated time for lab devices. The time left on an armed revert is never
  assumed from a stopwatch: it is read from the device. A wait on a lab's
  clock that sees that clock stand still for `lab_stall_s` (120) real
  seconds ends the change as `failed`, "the lab stopped": a lab at speed 0
  holds nothing for ever.

`Change` gains `radio: Option<RadioContext>`: the link's two ends as the
proposer believes them, the evaluation's `input` and `input_hash`, and the
figures claimed. Nothing in it is trusted: the planner reads both devices
itself, and the claimed figures are only shown beside the engine's own
recomputation from that input. `Origin` gains `radio`.

A planned radio change lapses: it cannot be approved or applied more than
`plan_valid_h` (12) hours after it was planned, and says "plan again".
Just before anything is sent, the whole radio plan is rebuilt from live reads
and must equal the approved one (reach, the lists, radar marks, the peer,
timers), not only the configuration's hash: a RouterOS export rarely changes,
and a ring can fail over between approval and apply.

### `plan.rs`

For a radio target, over one session to the written device and, where the
witness is manageable, one read-only session to it, reading both **now**:

1. Fetch the running configuration (the base) and collect `radio`,
   `radio_peers`, `routes`, `arp`, `mac_table`, `interfaces` and `facts`; then,
   for reach, the same tables of every other device of the world that has a
   radio and of the routers the walk reaches (under "Which end is reached
   through the link").
2. Find the radio and its stack. Refuse: a role the target does not fit
   (`RadioRetune` on a slave: "a slave cannot move a link"); a `refuse` rule
   of the stack (the device chooses its own channel; something else manages
   it; no country; a testing mode; a virtual interface); a stack with no
   `change`; software older than the stack's `since`; a name or value that
   does not match its declared pattern; a width or layout the profile cannot
   look up, or a live channel string that disagrees with the configuration; a
   field the stack `requires` that the read did not give ("could not read
   <field>"); a witness in another world; an open ledger entry for either
   device or either radio's MAC, or a `leftover` read that finds an entry on
   either (the change that removes it excepted); a peer registered now that
   is not `peer_mac`; a slave associated with a master other than this one.
3. Work out `reach` for the written device. A device that will have a guard
   armed on it needs a valid drill proof, **whatever its reach**: the guard
   is the only thing that ends an outage Nocturne cannot see coming (a path
   that crosses a radio it does not know, its own host dying), so it is
   never armed unproven. The one exception is the drill itself, which is how
   a proof is made: its second stage is armed only after its first has
   passed in the same change (under "The dead-man switch"). The device's
   clock is read with its uptime, as the reference the executor compares later
   reads with; its time zone, and whether it is right, do not matter.
4. `RadioRetune`. Refuse unless all of this holds:
   * the master's reach is `direct`, as computed;
   * the master's operating frequency is its configured one ("this radio has
     moved itself; its configuration and its channel differ");
   * the target keeps the present width and offset;
   * **every row of the master's own allowed list that the target's span
     covers** is marked `not_radar` and has a power figure, by the function
     T12 tests in `nocturne-rf`. The control row alone is not enough: a
     40 MHz channel moved to the top of a block puts its other half on rows
     nobody read. Where the slave can be asked, the same over its list;
   * in a mode where a device does not bound its own power, the target's row
     figure is not under the present channel's, at each end that can be
     asked; **where the slave cannot be asked, its mode is not known and is
     taken as one that does not bound its power, so the target's row figure
     on the master's list must not be under the present channel's**; and a
     channel either device marks indoor-only is refused unless both radios say
     they are installed indoors. A move **home** is exempt from these two
     rules, because it returns the link to the limits it lived under before
     the move. This list is the whole list: a channel a person chooses by hand
     meets no other gate;
   * where the slave can be asked, its list holds the target's control
     frequency **and the present one** as plain numbers;
   * **`peer_mac` is registered now.** A forward move is never started into
     a link that is already down: the change would be blamed for an outage
     that began before it, and nothing it verified would mean anything.

   If the allowed list cannot be read, or its read is still listed
   unverified, there is no plan: a target that cannot be confirmed as
   allowed and as not a radar channel is not sent. The plan notes, as a
   survey's does, any other radio of the operator's that carries the same
   network name (a slave that finds that one first has not followed), and
   the master's latest reach test where it did not pass ("the retune of
   <date> did not show this master answering while its link was down: <what
   was seen>; if it is reached through its link, this retune will be put back
   the same way").

   *The one exception to the last point is the return with the link down.* A
   retune whose target is the channel that **the last forward move** of this
   link left, by the engine's own record of that move, may be planned with no
   peer registered. Its plan says `link_down: true`, carries
   `radio_link_down_return`, and shows the slave's list **as the engine last
   read it, with the date** (it cannot be read now). Nothing in its planning
   or its order needs a link that is up; what the person's typed
   confirmation means is under "The order".
5. `RadioPrepare`. Refuse an addition that the slave's own allowed list does
   not hold: a slave may have a narrower band or installation than its
   master. Nothing is asked of the entry's radar mark. A list entry is a
   place to look, not a target, and the cure for `radar_exit_uncovered` is
   to add exactly the radar frequencies the master may go to. The new list
   is the old list as written, plus the **present operating control
   frequency** if it is not already there as a plain number (whatever its
   mark), plus the additions: `default` becomes `default,5745,5795`. "The
   old frequency never left the slave's list" is made true here, not
   assumed. Additions the list already holds as plain numbers are dropped; if
   nothing is left the target plans to no commands and says so. The plan
   says that Nocturne adds and does not remove: once applied, these entries
   are taken out by hand or not at all.
6. `RadioGuardRemove`. The device holds an entry of that name, or the ledger
   has it open and the device can be read. The plan shows what the ledger
   says the revert would restore, which change armed it and when, the
   device's own figure for its next run, and whether its mark is in the
   log. It is one command, the guard's `disarm` line, sent **at once,
   whatever the entry's next run**: racing a stale revert costs nothing,
   because the read afterwards says which happened, while waiting for it
   would make sure that a value from another night is applied, which on a
   slave whose master has moved since is how a slave is stranded. No guard
   is armed for it and it has no rollback: putting back a revert nobody
   planned is what this change exists to prevent, and the plan says "cannot
   be undone". While the guard is listed unverified the plan needs
   `changes.allow_unverified_writes`, as the arming did, and says so; the
   alert that reported the leftover says so too.
7. `RadioSurvey`. Refuse unless this end's reach is `direct`, as computed;
   refuse a master whose channel is `radar` or `unknown`, or whose latest
   reach test did not pass; refuse without a registered peer; refuse when the
   coverage the survey could claim is "not known" or only the frequency in
   use (under "Surveys").
8. Build the commands from the templates and the rollback from the values
   read (`configured_frequency` as written; the scan list as written). Render
   with the drivers' renderer, which checks every rendered line as the
   profile section describes.
9. Compute the timers and the worst-case outage (below).

   **"The slave looks on a frequency"** means the same thing wherever this
   file says it, or says the slave's list "holds" one: where the slave's
   effective list has a checked read, the frequency is in it, and that
   decides (a named list edited last month is caught here). Otherwise the
   frequency is a plain number in the configured list **and** is in the
   slave's own allowed list.
10. The prediction of a typed radio target is not text. It is "the radio's
    planned properties take the planned values, and nothing else in the
    configuration changes". `Plan.diff` is empty and `Plan.radio.changes` is
    a property list (`frequency: 5745 → 5795`).
11. `plan_hash` covers commands, rollback, base hash, the fixed options and
    the whole radio plan.

```rust
struct RadioPlan {                       // Plan.radio
    step: prepare | retune | survey | drill | guard_remove,
    interface, stack, role,
    reach: { value, via: Option<String>, evidence: Vec<String>, not_seen: Vec<String>,
             last_test: Option<{ change_id, at, passed: bool, seen: String }> },
    witness: Option<{ device_id, interface, mac, manageable: bool }>,
    changes: Vec<{ property, from, to }>,
    from: Option<Channel>, to: Option<Channel>,
    home: bool,                          // a return to the channel the last forward move of this link left
    link_down: bool,                     // planned with no peer registered: a return only
    operate: Vec<String>,                // the survey command; empty otherwise
    coverage: Option<Coverage>,          // a survey: what it will be able to claim
    notes: Vec<String>,                  // sentences a person must read: where else the slave could go, the slave's
                                         // list as last read, the last reach test that did not pass
    setting_opened: bool,                // can be applied only because allow_unverified_writes is on
    link_effect: none | down | unknown,
    guard: Option<{ name, delay_s, arm: Vec<String>, check: String, disarm: Vec<String> }>,
    follow_deadline_s: u32,              // from the `set` to "the peer is registered"
    management_return_s: u32,            // from "registered" to "the far end answers"
    return_deadline_s: u32,              // on the way home, before "did not come back" may be said
    outage_expected_s: u32,              // shown at approval, in words
    outage_worst_s: u32,                 // forward, failed, reverted, home: shown beside it
    measured: bool,                      // false: the timers are estimates, never measured on this link
    engine: Option<{ input_hash, claimed, recomputed, agrees: bool }>,   // the engine's own recomputation, shown beside the proposer's figures
    unverified: Vec<String>,             // operations AND reads this plan stands on that have not met a real device
}
```

**Timers.** Two deadlines, not one, because the radio coming back and the
far end answering are different events: routing and bridging above the radio
take their own time (OSPF's wait, spanning tree: remembered, tens of
seconds).

```
follow            = max(60, 3 * the largest follow time this link showed in its last three retunes)
                    where the link has none, or the slave's list has changed length since they were
                    measured: the profile's follow_estimate_s
follow_deadline_s = follow
management_return_s = max(the profile's figure (120), 3 * the largest this link has shown)
guard.delay_s     = follow_deadline_s + management_return_s + stable_s + verify_budget_s + confirm_margin_s
return_deadline_s = 2 * follow + (wait of the OLD channel: 0 when it is not_radar; the largest this
                    link has measured on it; unknown_wait_s (600) when it is radar or unknown and
                    nothing was measured)
outage_worst_s    = guard.delay_s + return_deadline_s
```

A **follow time** is what Nocturne measured from setting a frequency on the
master to the peer registering on it, on the way out or on the way home, and
only when the peer registered inside the deadline that was running. So a
move that was put back at an estimated deadline still gives the link a
measured figure when it comes home: the time the slave took to find its
master on the old channel. A change in which the slave restarted (its uptime,
where it can be read) gives no figure. There is no other rule for an
estimate that was too short: the next plan uses what the way home measured.

A learned figure only ever lengthens a deadline: a link whose far end once
answered in 3 s does not get 9 s the next time. None of the four deadlines
may exceed `max_deadline_s` (1800); a plan that would is refused with the
figure that made it so ("this link's slave took <n> s to find its master on
<date>; Nocturne will not wait <m> s"). Nothing shortens it again, and the
refusal says so: with the defaults, a link whose slave once needed more than
300 s to follow (200 s where the channel being left is a radar channel, whose
wait the way home must also allow) is not moved by this build. `outage_worst_s` is a promise as well as a figure:
"the link did not come back" is never said before it has passed, because the
way home is counted from the last moment the old frequency was applied, and
that may be the guard's own run (`rollback.rs`). Both figures are shown at
approval, with the sentence "sessions and routing adjacencies across this
link will be torn down and rebuilt". A stated outage built on an estimate
says "estimated, never measured on this link".

### `risk.rs` and `protect.rs`

Today a RouterOS radio line produces no protected hit at all. New kinds:

| Kind | When | Level |
|---|---|---|
| `radio_management_path` | a radio change on a device reached through its own link (a far slave being prepared, or its revert being tested) | critical |
| `radio_unverified_write` | the plan can be applied only because `changes.allow_unverified_writes` is on. The subject lists the operations and reads in it that have met no real device, and says that the setting is installation-wide | critical |
| `radio_link_down_return` | a return planned with no peer registered: "the link is down now, and Nocturne cannot test the path, the signal or the slave's list first; the slave's list is shown as it was last read on <date>". Typing the device names confirms exactly that ("The order", the return with the link down) | high |
| `radio_only_path` | other inventory devices are behind this link; they are **listed in the hit's subject**, with "and possibly others Nocturne cannot see" | high |
| `radio_slave_unseen` | master only: Nocturne cannot read the slave's list, country, regulatory mode or power mode, and the subject names each. "If the slave does not look on <f>, the link is down until the revert at <time>." Approving is the person's statement that it will. | high |
| `radio_master_serves_many` | the master's mode can take more than one station and no cap of one was read. "Stations that are off right now are not prepared and may not find this master again." | high |
| `radio_return_through_radar` | the present channel is `radar` or `unknown`: the way home passes through a radar wait of up to <n> s, during which the master may move itself; and the link cannot be sent home after the move is confirmed | critical |
| `radio_survey_outage` | every survey: the outage, and "no revert protects a survey: it changes no configuration" | high |
| `radio_drill_may_blink` | every drill: "setting a radio to the value it already has may restart it; whether it does on this device is what this test records" | medium |
| `radio_guard_remove` | every removal of a leftover revert: what it would have restored, and "cannot be undone" | high |
| `radio_config` | a `Lines` or `Intended` change touches a line the stack's `menu` pattern matches. The hit says to use a retune instead. | high |
| `radio_regulatory` | a `Lines` or `Intended` change touches a property on the stack's `never` list | critical |

`protect::is_management_or_access` learns `radio_management_path` by name, so
it makes a plan `critical` and brings the planner's existing management-path
wording; a kind that merely starts with `radio_` would be `high`. The other
levels in the table are set by the radio planner. Every hit forces the typed
confirmation that any protected hit forces. The level of a typed radio plan
is the worst of its hits, with `medium` as the floor; the generic "cannot be
predicted" raise does not apply (its prediction is exact), and "cannot be
undone" does not apply to a retune or a prepare (each has a rollback) and
does apply to a survey and to the removal of a leftover.

`changes.assistant_apply` refuses any change that contains a radio intent,
**as the first statement of its handler**, before the shortcut that lets the
assistant start a change a person has already approved (see "Found on the
way"; work in flight on the change engine removes that shortcut, and the
radio refusal stays first either way). A person who approved tonight's
retune at 16:00 presses Apply themselves.

### The order, exactly

One executor for radio changes (`radio/execute.rs`) runs the phases below. It
uses the pipeline's own steps (connect, drift check, backup, send, final
backup), adds the guard and the waiting verification, and bypasses none.

**Every radio change, before anything is sent.** Take the radio lock of the
world the devices are in. Hold the written device and the witness. Check the
ledger, and read `leftover` on the written device and on a manageable
witness. Rebuild the plan from live reads and compare. Drift check. Backup
of the written device (before arming, so no stored version contains the
dead-man). Open the maintenance window. Read the baseline at both ends (the
signal each end reports, where it reports one, and the master's frequency),
the written device's uptime and its clock. Any failure here ends the change
with nothing sent. Nothing here needs the link to be up: a link that is down
has no baseline signal, and that is recorded, not refused.

**Preparing a slave** (`RadioPrepare`):

| # | Step | Why it is safe |
|---|---|---|
| 1 | If the slave is not reached directly: arm its guard with "put the scan list back", delay `follow_deadline_s + management_return_s + margins`. Read back that it is armed (below). | The master is not touched by this change at all. |
| 2 | Set the new list. | The master has not moved, so the slave finds it where it was. If changing the list restarts the radio (not known), the link drops and re-forms on the old frequency. |
| 3 | Wait for the link, then for the slave to answer; read it back: associated with this master, and the list reads exactly as planned. A session lost in this step is expected and is judged by this read, not rolled back on sight. | Proves the list took effect. A list that does not read back as planned is put back (sent, or by the guard) and the change ends `rolled_back`. |
| 4 | Disarm, and read back that **no** guard of Nocturne's is on the slave; that read closes the ledger entry. If the read cannot be made, the change waits past the guard's due time and reads again. It then has four endings, and the fourth has a bound. The entry is gone and the list is as planned: `applied`. The entry is gone and the list is as it was: `rolled_back`, the guard having done its work (with or without its mark: under "The dead-man switch"). An entry is still there: `failed`, "an armed revert was left behind", at once and not at the next planning. **The slave cannot be read by the guard's due time plus `return_deadline_s` plus `management_return_s`**: `failed`, "the slave is unreachable; a revert armed at <time> may still be on it, and if the device restarted it runs one interval after it next starts". The lock and the holds are released, the engine raises its alert, and the ledger entry stays open. | A retune is refused while the ledger names either device or either radio, so a slave's revert can never fire after a master has moved, also when the slave was dark when the retune was planned. The entry re-arms from every boot, which is what lets it survive one; without the bound a slave that lost power would hold the lock for as long as it was dark, and no other link could go home that night. The bound allows what the slave needs after its revert runs: a list put back may restart its search. |

**Moving a master** (`RadioRetune`; the master is reached directly):

| # | At | Step | Why it is safe |
|---|---|---|---|
| 1 | master | Arm the guard: back to the old frequency after `guard.delay_s` unless confirmed. Read back that it is armed. | Always armed, also when the master is near: Nocturne's host may die, and the path to the master may cross a radio Nocturne does not know. |
| 2 | master | Set the new frequency. The clock for every deadline starts here; the time left on the guard is read from the device. The moment the `set` returned is recorded. | From here the link is down until the slave finds the master. |
| 3 | master | **The reach test**, then the follow. Read the master's registration table back to back, each read sent as soon as the previous one has been answered, from the moment the `set` returned until the test has passed; after that every `follow_poll_s` (2), until `peer_mac` is registered, the master reports the target channel and is not in a radar-wait state; all of it within `follow_deadline_s`. The test **passes** when two reads in a row are answered showing the peer not registered. The time each read was sent and answered, and what it showed, are recorded. | The second of those reads was sent after the first answer had arrived, so after the master had dropped its peer. From that moment until the peer registers again, a request that has to cross the link cannot reach the master, and the answer it gets afterwards shows the peer. So two such answers in a row come only from a master Nocturne reaches without its link, whichever way the answers travel back. One answer is not enough, nor is the `set`'s own return: either can have crossed, or left, before the radio stopped. A test that did not pass means this change is never confirmed (`rollback.rs`). A slave that is back before the second answer arrives also leaves it not passed: a revert, never a stranded master, and neither the channel nor the master is blamed. |
| 4 | slave | Where the slave is manageable: poll until it answers and reports itself associated **with this master's address** on the target channel; at most `management_return_s`. A slave that associated with somebody else has not followed. | |
| 5 | both | Verify (below). | |
| 6 | master | Confirm. Read the guard: present, its mark not in the log, at least `confirm_margin_s` left **by the device's own figure**. Read the device's uptime: **it has not restarted since the guard was armed**. Read its clock: **it has moved on by the link clock's time since the arming, within the guard's `tolerance_s`**. Read `leftover` on a manageable witness: nothing. Then remove the guard; read back that it is gone; **read the master's frequency once more.** | A revert that fired a moment before the removal is seen here, not recorded as success. With less than the margin left, the confirmation is not attempted: the device reverts and the change is recorded as reverted. A master that restarted in the middle is not confirmed either: its entry was re-timed from the boot, and "present, never run" no longer means what it meant. A clock stepped in the middle (a time server correcting it; summer time, where the device keeps local time) makes the device's own figure for the time left mean nothing: not confirmed. |
| 7 | both | Final backup; release; record the follow time and the management-return time actually seen, and the reach test's outcome as the master's latest. | They feed the next plan's timers, and the next plan's note. |

**Master only.** The same, without step 4. The slave is a blind end: the
plan carries `radio_slave_unseen`, and verification uses what the master
reports of the slave.

**Slave only.** There is no retune. What exists: reads; a survey of the
slave's end when it is reached directly; a recommendation that says it is one
end's view and is for whoever controls the master; and `RadioPrepare`, so
that when the master is moved by a person, by its own selection or by radar,
the slave finds it.

**A return with the link down** (`link_down: true`; a person's change, with
the typed confirmation that `radio_link_down_return` forces). The same steps,
with no witness and nothing that needs the link: there is no baseline signal
to read, and the reach test is made as for any retune (with the link already
down, a master reached without it answers at once that the peer is not
there). If the peer registers on the old channel inside `follow_deadline_s`,
the move is verified from the master's reports (registration, the channel,
`stable_s`; the signal floor is `not_run`, "there was no signal to compare
with") and confirmed. If it does not, the master is set back to the
frequency it was found on, the guard is left to fire and is read back as gone
after its due time, and the change ends `rolled_back` with "the link was down
before this change and is down after it; neither channel carries it now, so
the cause is not the channel". No "did not come back" alert is raised for an
outage the change did not start. **What the person confirms by typing the
device names**: that the link is down now; that Nocturne could not test the
path, the signal or the slave's list before sending (the list shown is the
one last read, with its date); and that if no peer registers on the old
channel, the master is left exactly as it was found. Nocturne does not leave
a master on a channel where nothing was verified, the old one included: a
person who wants that sets it by hand, as today, and Nocturne records an
outside move.

**Cancel.** Before step 2 a cancel ends the change with the guard removed and
read back. After step 2 it means: do not confirm; send the rollback; wait for
home as below. It never means "stop where you are".

**Shutdown.** A radio change in flight is cancelled at shutdown like any job
(every job's token is a child of the shutdown token). Before step 2 the guard
is removed and read back if that can be done at once, and is otherwise left to
fire, which sets what the device already has. After step 2 the old frequency
is sent to the master if its session answers, and the executor stops without
waiting for the way home: the guard and the restart rule below finish it. The
change stays `applying`.

**A restart of Nocturne during a radio change.** At the next start the change
is found `applying`. `db::recover_interrupted`, which today fails every such
change at start-up, leaves radio changes to this rule. For a radio change the
engine first takes the radio lock of its world again, then reads both devices
before it says anything: whether a guard is still there and when it is due,
which frequency the master is on and what the slave's list holds, whether the
peer is registered. It records that, waits out a guard that is still armed
(holding the lock), and marks the change by the endings its kind has, from
what it then reads. A retune is `applied` only when its confirmation had been
completed before the restart (no entry, the master on the target with the
peer registered), `rolled_back` when the link is verified on the old channel,
and `failed` otherwise; a prepare or a drill takes its four endings; a survey
is `failed` unless the peer is registered again on the same channel. Nothing
is confirmed that was not seen confirmed.

### The dead-man switch, per platform

The existing guard (`config.guard`) means "the device reloads into its
**saved** configuration unless told not to". RouterOS has no
saved-versus-running distinction, so its dead-man must carry the undo
itself. That is a different thing, so it is a different block: `radio.guard`,
used only by typed radio targets, whose rollback the planner built from
templates. `config.guard` for RouterOS stays `null`. Text that came from a
`Lines` change, typed by a person or drafted by a model, is therefore never
placed inside a device-side script, and an ordinary RouterOS change is armed
with nothing, as today.

```yaml
radio:
  guard:
    name: scheduled revert
    body:                         # what the device runs, in this order
      - '/system scheduler remove [find name={name}]'     # first: it can fire once, whatever the revert does
      - '{rollback}'                                      # the plan's rollback lines
      - ':log info "{name} ran"'                          # the mark: proof that the lines before it ran
    join: '; '
    escape: [['\', '\\'], ['"', '\"'], ['$', '\$']]       # replacements applied to the joined body: data, not Rust
    arm: ['/system scheduler add name={name} start-time=startup interval={delay} on-event="{body}"']
    delay: '{seconds}s'
    clock:                                                # the device's own time; read-only
      command: '/system clock print'
      fields: { time: 'time: (\S+)', date: 'date: (\S+)' }
      instant: '{date} {time}'
      formats: ['%b/%d/%Y %H:%M:%S', '%Y-%m-%d %H:%M:%S']   # version 6, version 7: remembered, R5 records both
    check: '/system scheduler print detail without-paging where name="{name}"'      # read-only
    armed:
      run_count: 'run-count=(\d+)'
      next_run: 'next-run=(\S+( \S+)?)'
      formats: ['%b/%d/%Y %H:%M:%S', '%Y-%m-%d %H:%M:%S']
    tolerance_s: 10
    mark_read:                                            # read-only
      command: '/log print without-paging where message~"{name} ran"'
      present: '{name} ran'      # the mark is there when a line of the answer holds this text; an error is "could not be read"
    disarm: ['/system scheduler remove [find name={name}]']
    leftover:                                             # read-only
      command: '/system scheduler print detail without-paging where name~"^nocturne-guard-"'
      name: 'name="?(nocturne-guard-[a-z2-7]{16}(-[12])?)'   # how each entry's name is picked out of the listing
    line: '^/system scheduler add .*\bname=nocturne-guard-[a-z2-7]{16}(-[12])?\b'    # its line in the export
    effect: the device sets the old values back by itself
    unverified: [master, slave]   # the roles the revert has not been checked on
```

`{name}` is `nocturne-guard-` and the change's id (16 characters of `a-z2-7`),
with `-1` or `-2` for the two stages of a drill.

How a device's text becomes an instant is data as well. `fields` pick the
parts out of the answer, `instant` joins them, and `formats` is a list tried
in order, written in a small fixed set of tokens (`%Y %m %d %e %b %a %H %M
%S` and literal characters; month and day names in English, in any case).
The engine has one parser for that token set and knows no vendor's dates.
A text that fits no format is "the device's time could not be read", and
then nothing is sent. The device's uptime comes from the `facts` kind.

**The device's instants are only ever compared with each other.** The next
run is compared with the device's own clock, and the device's clock with what
it said at arming; it is never compared with Nocturne's clock. So its time
zone, and whether it is right at all, do not matter, and nothing needs a zone
token or the device's offset. What matters is that the clock is not
**stepped** while a revert is armed, because then the device's figure for the
time left means nothing, and that is read: with every read of the guard the
executor reads the clock too, and a device whose time has moved on by more
or less than the link's clock did since the arming, beyond `tolerance_s`, has
had its clock stepped. The change is then not confirmed (`rollback.rs`).

Why this form:

* **`start-time=startup interval={delay}`** is the form the vendor's reference
  page for the scheduler (version 7.26) describes: "With a non-zero interval
  the first run comes one interval after startup (or one interval after the
  entry is added while the router runs)". The form with `interval=` alone
  leaves the start at "the time when the entry is added", which is the case
  the manual's own two examples disagree about. And one entry covers a reboot
  with the guard armed: the device reverts one interval after it comes back
  up. A change during which the device restarted is never confirmed: the
  executor reads the uptime at arming and again before it confirms.
* **The commands sit in `on-event` directly**, which the same page allows:
  no script object, no mismatch between a script's policy and the entry's,
  one thing to add, one to remove, one to look for.
* **It removes itself first.** A script that runs its revert first and stops
  at an error never reaches its own removal, and then repeats every interval
  for ever: an unattended configuration writer outside the pipeline. Removed
  first, it fires at most once. Whether the lines after the removal still run
  is exactly what the drill proves, by the mark.

How the pipeline uses it:

* **Armed means read back, with the time.** After arming, `check` and `clock`
  are read together. The entry must be present, its run count zero, and its
  next run within `tolerance_s` of now plus the delay **on the device's own
  clock**, both read through the declared formats. An entry that is
  missing, has run, or is due early or not at all stops the change with
  nothing sent. That clock reading, with the link's clock beside it, is the
  reference for every later one.
* **The time left is the device's figure.** Whenever the executor needs to
  know how long it has, it reads `check` and `clock` again; a clock that was
  stepped ends the confidence in that figure (above).
* **Whether it has run is the mark, and where the mark is gone, the radio's
  values.** The vendor's page says the run count is "the number of runs since
  the last startup" and that a reboot resets it to 0, so the count is used
  once, in the read-back right after arming. Everywhere else "has not run"
  means the entry is present **and its mark is not in the log**. The mark
  lives in the device's log, which on RouterOS is by default a memory log of
  a fixed number of lines that a restart clears (remembered; R5 records
  whether a mark survives one), so a mark can be lost, and nothing is ever
  decided by its absence alone. Where the entry is gone, the radio's values
  decide: the old values mean the change is home, recorded as `guard_fired`
  when the mark is there and as "the revert ran or was removed; its mark
  could not be found" when it is not; any other values mean `failed`, and a
  person is told. What is reported of a leftover is whether its mark is
  there, never a count.
* **Disarming is a read, a removal and a read.** The entry must exist, with
  no mark in the log, before it is removed (a removal that matches nothing is
  remembered to answer nothing, so "no error" proves nothing), and `check`
  must show it absent afterwards; that read is what closes the ledger entry.
  Until it does, no later phase starts. On the way home there is one more
  ending, and it is not a failed disarm: the entry is already gone and the
  radio is on the old values. The device did what the guard is for, and the
  change is `rolled_back`, as `guard_fired` or with the sentence above.
* **The configuration comparison** ignores, while a guard is armed, exactly
  the export line that matches `line` with **this change's** name, and
  requires it gone afterwards. Nothing is added to `config.ignore`, which
  applies to every backup.
* **A leftover is found**, by the ledger and by a read: planning any radio
  target runs `leftover` on both devices; an entry found refuses the plan,
  makes the engine raise its alert, and reports whether its mark is in the
  log. The alert says that removing it is a person's change, and that it needs
  `changes.allow_unverified_writes` for as long as the guard is listed
  unverified, as the arming did.
* **A leftover is removed by its own change**, `RadioGuardRemove`, which is
  the one radio plan allowed while that refusal stands. It cannot be done
  as a `Lines` change. On RouterOS a `Lines` change cannot be predicted,
  fails its configuration check and is rolled back, and that rollback is
  not planned: `apply.rs::roll_back` (and `rollback::run`) derive their
  lines on the spot with `hier::remediate`, and `remediate_routeros` begins
  by adding back, word for word, every line of the stored configuration
  that the device no longer has (read in the code). The stored
  configuration holds the scheduler entry. A person would remove the
  leftover and the pipeline would put it back, armed from that moment, with
  the revert of some earlier night.
* **A leftover is never re-armed**, by three rules, none of which trusts the
  planner alone. (1) No change of any kind is planned, and none is sent, on
  a device whose configuration holds a line that matches a profile's
  `radio.guard.line`: the generic planner sees it in the base configuration
  it already fetches and the drift check sees it again before sending. The
  only exceptions are `RadioGuardRemove` and the radio change whose own
  entry it is. (2) The planner refuses any change, restores included, one of
  whose commands **or rollback lines** matches `line`. (3) Every rollback
  derived at apply time or on demand leaves out each derived line that
  matches a profile's `radio.guard.line`, or that removes an entry whose
  name matches the guard's name pattern, and says in its step that it did:
  a leftover is removed by its own change and by no other (`rollback.rs`).
  A stored version that holds an armed revert (a nightly backup taken while
  a leftover sat on the device) cannot arm it again by any road.
* A dead-man that fired is reported as today's `guard_fired`, with what the
  device did and when.

Everything RouterOS-specific in that block is assembled from documented
parts, and **none of it has been run.** Not known: whether `[find ...]`
inside a quoted `on-event` is evaluated when the entry is added or when it
runs; whether the lines after the self-removal run; the policies the entry
needs; what a stepped clock does to the next run; how each version prints
a date; all of version 6. That is why `RadioGuardDrill` exists, and why its
result is a proof and not a hope.

**The drill**, two stages in one change. It needs no proof to be planned: it
is how a proof is made.

1. The guard with a body of the removal and the mark only: nothing touches
   the radio. The executor polls `check` and records when the entry
   disappeared. It passes only if that is within the tolerance of the delay
   (`drill_delay_s`, 60), the device's clock was not stepped meanwhile, and
   `mark_read` shows the mark. This proves the timer, the quoting of the
   frame, the self-removal, and that lines after it run, under the account
   and the policies the real change will use. A guard that fires at once, or
   late, or never, fails here, with no radio involved.
2. Armed only after stage 1 has passed in the same change: the guard with the
   real rollback line setting the values the radio already has. Same checks;
   and the peer's uptime is read before and after, which answers a question
   nobody could: does setting a radio to its present value restart it? The
   plan says `link_effect: unknown` and carries `radio_drill_may_blink`, and
   on a device reached through its own link (a far slave) also
   `radio_management_path`. Its endings are a prepare's, with the same bound:
   the entry read gone and the peer up: passed. The entry gone and the
   radio's values as they were, but the peer not back by the entry's due time
   plus `return_deadline_s`: `failed`, "the radio did not come back after it
   was set to its own values", and the engine's critical alert names the
   property and value that would set it again. The device not readable by
   the due time plus `return_deadline_s` plus `management_return_s`: `failed`,
   the ledger entry left open, the lock and the holds released. An entry
   still there: `failed`, "an armed revert was left behind".

   **Stage 2 is never planned on a master whose channel is `radar` or
   `unknown`** (until R2, every master). A restart there would put the master
   through a radar wait nobody has measured, and a detection during it sends
   it to a channel of its own choosing that its slave may not scan. On such a
   master the drill is stage 1 only, and what stage 2 proves (that the real
   line runs inside the revert under this account, and whether a same-value
   set restarts the radio) is taken from a stage 2 that passed on another
   radio **of the same role, software, account and line**, which the proof
   names. Where there is none, that master has no drill proof yet, and is not
   retuned until one exists.

Rejected: RouterOS Safe Mode. It is entered with a key press, which the
driver cannot send; where it can be scripted (7.18 and later) the command
names are in no manual page; it reverts only when the session is declared
dead, after a TCP timeout of 9 minutes; it does nothing when the session
survives and the link is bad; and it undoes other people's changes made in
the meantime. Rejected too: a script object named by the scheduler entry
(two things to clean up, and a policy mismatch the manual warns of).

`cisco_ios` keeps its reload timer. The lab's radio has a revert of this same
kind (below), so arming at the exec prompt, the escaping, the read-back with
the time and the leftover check all run in a lab, on a body that holds a
quoted string and on times that need the declared formats.

### `verify.rs`

`Verify` gains `radio: Option<RadioVerify>`, filled by the planner:

```rust
struct RadioVerify {
    on: Option<Channel>,           // the master must report this control frequency (its width and offset are its own
                                   // configuration, which the change does not touch). The slave must report this control
                                   // frequency and this master's address; its width and offset are checked only where a
                                   // recorded read gives them, and are `not_run` otherwise.
    peer_mac: String,              // and each other
    stable_s: u32,                 // default 60: the peer's uptime must keep growing for this long
    signal_floor_dbm: [Option<i32>; 2],   // per end: tonight's baseline on the channel being left (read just before the
                                   // `set`), less max(0, row(old) - row(target)) from the transmitter's own list, less
                                   // max_signal_drop_db (6). None at an end that reported no signal before the `set`
                                   // (a link that was down, an end that does not pass the figure on): that check is `not_run`.
}
```

* **It waits.** Each check polls a read until the condition holds or its
  deadline passes. It never concludes from one look after a sleep.
* **It reads both devices.** The witness's own report is part of the master's
  verification.
* **The signal floor is tonight's, for every move.** The baseline is read
  just before the `set`, on the channel being left, and the floor allows for
  the difference of the two rows of the transmitter's own list. So a move
  from a 30 dBm part of the band to a 23 dBm part is not failed for the 7 dB
  the score already allowed for; a night of rain lowers the baseline as well
  and fails neither the move nor the way home, so no move needs a weather
  test before it is sent; nothing is checked against a figure from another
  week or from the proposer; and a move to a channel a person chose without
  a survey is verified exactly like any other. There is no rate floor: a
  modulation read at 02:00 on an idle link says little about the channel,
  and the busy hours are probation's.
* **In a radio change a lost session is judged, never rolled back on
  sight.** The generic path reconnects and rolls back at once. A radio
  change cannot: some of its sessions are expected to drop. The written
  device's session, on a slave reached through its link, drops while its
  list changes and is judged by the read-back. On a master it is the reach
  test's evidence, and the change is not confirmed (`rollback.rs`). The
  witness's session through the link drops in every retune and is reopened
  inside `management_return_s`. For every other kind of change a lost
  session behaves as it does today.
* **The configuration check** of a radio target has three parts. (1) Every
  line of the export other than this radio's own (the stack's `line` pattern
  with `default_name` inserted as a literal) and the armed guard's is
  unchanged, by hash. (2) This radio's own line, before and after, is taken
  apart by the parser the stack names (`line_parser`, one the drivers
  already have: `kv` for RouterOS and for the lab), and the keys whose value
  differs, appears or disappears must all be among the `keys` of the
  operation that was planned. Keys are compared by name and values by hash,
  so nothing is quoted, and a property the read does not map cannot change
  unseen. (3) The
  radio's **configuration fields**, read back through the collected kind,
  equal the plan: the planned ones have the planned values and the others
  are as they were. Which fields those are is the stack's `config_fields`;
  the live figures of the row (noise floor, state, peers, the frequency in
  use) change with every retune and are not among them. A property at its
  default is absent from a RouterOS export line and appears when set
  (remembered, from the one recorded export of another menu), which is why
  (2) lets a planned key appear and (3) compares what the device reports. A
  radio whose `default_name` was not read has no line to find, and is
  refused at planning. While `config_check` is listed unverified the plan
  says "the shape of this radio's line in the configuration has not been
  checked against a real device".
* The generic `neighbors` and `interfaces` checks leave this radio's own
  interface out during the follow wait.
* A survey verifies that the link is registered again on the **same** channel
  with the same peer, and that the configuration's hash is the base's.
* `Check` gains `not_run: bool`. Where a figure is not available on the
  stack, its check is `not_run` with "this stack has no <figure>", shown
  differently from passed and from failed. Today a check that could not run
  is `ok: true` with a sentence in its detail; that stays as it is for the
  existing checks.

**No secret leaves by this path.** On RouterOS a radio is one export line
holding every property, on version 6 (remembered) with the link's key in it,
and the security profiles with their keys sit beside it. So: a radio plan's
`diff` is empty and its changes are a property list; no check, step, event,
audit entry, job-log line or error of the radio path quotes a configuration
line; a mismatch is reported by property name, and by value only for the
properties the plan set. Anything else that is device text (a survey's
output, a refused line's answer, a guard's listing) leaves through
`redact::Hider`, built for the device's profile: that is the one door the
change engine has for text said about a change, and the radio path adds no
second mechanism beside it and no way round it. The redacted views of a
change and of a plan (`redact::change`, `redact::plan`) are extended to
`Plan.radio`, `Change.radio` and the stored survey rows, so the new fields
are answered through that door too.

### `rollback.rs`, and when the link does not come back

Before confirmation the guard stays armed through everything in this table.
"Revert" means: send the old frequency, as it was written, to the master
(which is reached directly); then wait for the peer on the old channel.
**That wait is `return_deadline_s`, counted from the last moment the old
frequency was applied.** That is the revert, unless the guard may apply the
frequency again: where the device's drill proof says a same-value set
restarts the radio (`blinked: true`), or does not say, the guard's own run
starts the wait afresh. So "did not come back" is never said before the
guard's due time plus `return_deadline_s`, which is the `outage_worst_s`
the plan promised, and a link two minutes from returning through a radar
wait is not declared lost.

The last column says whether the row is evidence about the target channel.
Only such a row quarantines the target, after a forward move that ended home;
every other row records why nothing was quarantined.

| Seen | Meaning | What happens | Blames the channel |
|---|---|---|---|
| The peer is not registered at the follow deadline | it did not follow, or cannot register there | Revert. | Yes where the deadline was measured; no where it was an estimate, and the record says "the follow time of this link has never been measured and the estimate may be short" |
| The master reports a radar-wait state on the target, or any frequency other than the target | the target was a radar channel after all, or the device moved itself | **Not confirmed.** Revert. The guard is never removed with the link down. | Yes |
| Link up, and the slave is associated with another master, or reports another channel | it found another master of the same name, half applied, or a stale read | Read once more; if still so, revert. | No: the plan names the other radio where it knows one |
| Link up, far end not answering at the management deadline | the radio layer is back and management is not | Revert. | No |
| Link up, signal below the floor, or it dropped again within `stable_s` | it followed, and the channel is bad | Revert. | Yes, unless the uptime of either device, where it can be read, shows it restarted inside the change; master only, the slave's cannot be read, and the record says so |
| **The reach test did not pass** (no two reads in a row answered showing the peer gone, between the `set` and the peer's return) | the master is reached through its link whatever was computed; or Nocturne's own path failed in those seconds; or the slave came back before a second read | **Never confirmed in this change, whatever is read afterwards.** The revert is sent as soon as the master answers; if it does not answer, the guard fires. The outcome, with every read's times, is kept as the master's latest reach test. | No |
| The session to the master is lost and stays lost | something on the way to it depended on this channel, or Nocturne's own path failed | Not confirmed. Reconnect until the guard's due time plus `return_deadline_s` plus `management_return_s`: the guard brings the master home, the slave then needs up to `return_deadline_s` to find it and management its own allowance. When the master answers it is read, and the ending is one of the two "after a revert" rows. Only when it has not answered by then: `failed`, with the ledger entry left open, as in the slave's fourth ending. | No |
| The master's uptime shows it restarted while the guard was armed | power, or a watchdog | Not confirmed. Revert; the entry, re-timed from the boot, is left to fire. | No |
| The master's clock moved by more or less than the link's clock since the arming | a time server stepped it, or summer time where it keeps local time | Not confirmed. Revert. | No |
| Cancel, or shutdown, after the `set` | a person, or Nocturne stopping | Not confirmed. Revert (at shutdown: the old frequency sent if the master answers, the rest left to the guard and the restart rule). | No |
| After a revert: the peer registers on the old channel in time | home | Remove the guard and read it back as gone; or, where the entry is already gone and the master is on the old values, `guard_fired` (with its mark, or "its mark could not be found"). `rolled_back` either way. The time the slave took to find the master on the old channel is recorded as a follow time. After a **forward** move whose row blames the channel, the candidate is quarantined. After a **return** nothing is quarantined: the link stays on the channel it tried to leave and a person is told what was tried and what was seen. No retry tonight. | |
| After a revert: it does not | **the link did not come back** | `failed`. The engine raises the `critical` alert. It names both devices and the properties that were set (by name, never a line), when the guard was or is due, and the facts as last seen: when each end last answered, the channel and the master the slave last reported, the slave's scan list as last read, and the master's frequency now. The guard is left to fire, the device is read again after its due time, and nothing further is attempted. | |
| After confirmation: the master is on a frequency Nocturne did not put it on | the device, or a person, moved it | **Not fought.** Recorded as `moved_by_itself`, and by the engine as an outside move; the next evaluation starts from fresh reads. | |

**Where the engine records an outside move.** It reads no device outside a
change, so it records one when it sees one: at every radio plan it builds
for that radio, by comparing the master's frequency with where its own last
change of that radio left it. The date is the date it was seen, never earlier
than the move. A retune a person approved through Nocturne is not an outside
move: the engine made it.

A failed `RadioPrepare` puts the list back as it was written (sent, or by
the slave's guard) and is `rolled_back` when the list reads as before; its
four endings and the bound on the last are under "The order". A survey has no
configuration to undo: if its command has not returned by `max_s` the session
that runs it is closed (there is no line that stops a scan), the radio is
read on a new session, and if the peer is not registered at
`return_deadline_s` the change is `failed` with the `critical` alert; the
session to that device still works, because only an end computed `direct` is
surveyed. The alert names any radio of the operator's that carries the same
network name: the peer may have joined it.

**`changes.rollback` on a radio change never sends anything.** Today that
method starts a rollback at once, with no plan to read and no approval (see
"Found on the way"). For a retune it creates a **new planned change** and
returns its id: a `RadioRetune` to the previous channel, refused when that
channel is a radar channel or its status is unknown. It is approved like any
radio change, and it needs `changes.allow_unverified_writes` again if the
plan is still `unverified` ("Staged trust"). For every other radio change it
refuses and says why: a prepare, "Nocturne does not take entries out of a
slave's list; this change added <frequencies>"; a survey, a drill or the
removal of a leftover, "there is nothing to undo".

**A derived rollback never touches a radio.** `changes.rollback` on an
ordinary change, and the rollback after a failed check
(`apply.rs::roll_back`), derive their lines with `hier::remediate`, and for
RouterOS `remediate_routeros` re-sends, word for word, every line of the
stored version that the device no longer has, then removes every `add` line
the stored version lacks (read in the code). A radio is one export line
holding every property, so the rollback of a three-week-old change of the SNMP
contact would put the radio back on that week's frequency and, on a slave,
that week's scan list, which may no longer hold its master's channel; and it
would remove a leftover revert with a `remove [find ...]` line. So in every
rollback the engine derives rather than plans from a typed intent (the
on-demand one today, the planned undo that work in flight makes of it, and
the one after a failed check), **a derived line that matches a stack's
`menu` pattern or the guard's `line`, or that removes an entry whose name
matches the guard's name pattern, is never sent**. It is left out; the
rollback (or the undo's plan, before anyone approves it) says so on its own
line, "<radio> is left as it is: a radio is changed only by a radio change";
and the target ends `rollback_failed`, naming the properties, because the
device is not all the way back. A `Lines` change that a person typed and that
touches a radio's menu is sent as typed, with `radio_config` or
`radio_regulatory` and their typed confirmation; it is only its derived
rollback that leaves the radio alone.

After any revert the next evaluation starts from fresh reads.

### What a radio plan can never contain

The country, region or regulatory mode; any test or compliance mode; the
antenna gain; any transmit power; the link's name, security or identifiers;
the role; the band or the width; a radar setting; a radar channel as a
target; the removal of a scan-list entry; anything on a device that chooses
its own channel. The typed
planner cannot produce them; a person who types them into a `Lines` change
gets `radio_regulatory` or `radio_config` and a typed confirmation, and no
automation of any kind will apply it.

## Proofs (`nocturne-changes`)

A proof is the engine's own record that something was seen to work here. It
is written by the executor at the end of a change, never by a caller.

```rust
struct Proof {
    id, kind: guard | prepare | retune | survey,
    device_id, interface, role, software,    // of the written device, at the time
    peer: Option<{ device_id?, mac, software? }>,
    reach: direct | through_link | behind_link | unknown,     // of the written device, at the time
    account: String,                         // the login Nocturne used (never its secret)
    lines_hash: String,                      // sha256 of the profile lines it exercised, as rendered with placeholders
    stages: { frame: bool, real_line: bool },   // a drill: which of its two stages passed on this device
    real_line_from: Option<String>,          // a drill whose second stage was not run here: the proof it was taken from
    measured: { fired_after_s?, blinked?: bool, follow_s?, management_return_s?, outage_s? },
    change_id, at,
    valid: bool, invalid_because: Option<String>,
}
```

A proof is valid while the device's software, the account and the lines'
hash are what they were. A proof is of a class, and it only justifies the
same class: a guard proof made while the device was reached directly does not
cover a device now reached through its link; a retune proof names the slave
that followed. A guard proof is complete when both stages passed on the
device, or when its frame passed on the device and its real line is taken,
for a master on a radar or unknown channel, from a stage 2 that passed on
another radio of the same role, software, account and line
(`real_line_from`); it dies when either proof does. `changes.delete` refuses
to delete a change that a valid proof or an open ledger entry refers to. Each
of those is the engine's own record; it is not asked to know the feature's
documents.

## Automatic changes: not in the first build

In this build nothing changes a radio without a person approving that exact
change. That is a decision about this build and not about the feature: the
owner asked for links that pick their own best channel, and what this build
keeps (below) is there so that bringing it back is a specification and not a
redesign.

What is out, and why:

* **A move forward by itself.** It would need the channel's occupancy
  measured at both ends of the link in its busy hours, on several days. On
  RouterOS the first-build survey hears networks and measures no airtime
  (`frequency-monitor` waits for recording R6), and only an end that Nocturne
  reaches without crossing the link is surveyed, which on the usual link
  leaves the far end unseen. So on a real link the evidence such a move would
  rest on cannot exist in this build, and a machine that could only ever run
  in the lab, and could not be tested anywhere else, is not shipped.
* **A move home by itself**, after probation judged a move worse. It is the
  one automatic change this build could make safely, and only once a RouterOS
  radio plan can be `verified`, because an unattended apply requires that
  whatever the bench setting says; no shipped profile has those recordings
  yet. Without it, the cost is a person's click: when probation says worse,
  the move back is already planned and linked from the alert, the master is
  reached directly, and it can be applied at once.

What is kept: one written device per change and only a directly reached
master; the revert armed on the device and proven by a drill; the evidence
rules of "When to move" and their words on every proposal; the engine's
recomputation of a proposal from the proposal's own input; probation and its
move back; the ledger and the proofs.

What brings it back: the recordings that let a RouterOS radio plan be
`verified` (R1 to R5); for a move forward, a survey that measures occupancy
(R6) and a link both of whose radios are reached directly, or a recording that
shows a far end can be measured and comes back by itself; a record, on this
installation, of moves that were predicted and then measured; and a
specification of its own for a standing approval: a person's recorded
approval, for one link, of the exact lines that may be sent, bounded by
rules that person set, and checked by the change engine against its own
records before every use.

## Links on the same mast are planned together

The vendor that sells the best-known per-link optimiser says not to use it
where the interference is the operator's own; a per-link answer for T6 is
wrong for both links.

* **Who is at a site.** Every `radio` row of every device whose `site` equals
  either end's `site`, whether or not it belongs to a link, whether or not it
  can be changed. `site` is free text: two spellings of one mast are two
  sites, which is why nothing that protects a link depends on it (one radio
  change at a time in a world). A link whose devices have no `site` is
  evaluated alone and says "co-located radios not known". In a lab, a node's
  site is its mast (under "The lab").
* **How loud.** `coupled` is measured when a survey has ever heard that radio
  (a `SurveyNetwork` with `own: true`); otherwise the default for an
  unshielded neighbour on the same mast (`own_coupled_dbm`, -45), and then
  the evaluation lists it under `assumed`.
* **Synchronised radios** are left out of `own` only when a person has said so
  (`sync_group`). A radio in an Nv2 synchronisation group is not retuned at
  all.
* **After a move** every link at the site is evaluated again, and probation
  watches them (above).
* **No move that lowers the sum of `Q` over the site's links**, whatever it
  does for the link moved.
* **Two links that each want the other's channel**: the link with the lower
  id keeps its channel.

`radio.site {site}` returns the joint view.

## Radar channels and regulatory rules

The software does not know the law of the place, the licence the operator
holds, the antenna on the mast or whether a radio is indoors. The device's
country setting is the only regulatory fact it has, and that is the
installer's statement.

1. **Candidates come from the devices, never from a table in the
   application.** What both radios themselves list as allowed under their
   present country setting, less what the operator excluded. Nocturne ships
   no country table and no table of radar waits.
2. **Never written**: the list under "what a radio plan can never contain".
3. **A regulatory state that makes the device's own list meaningless is a
   refusal in the engine**, for every change, not only a finding of the
   feature: no country, a conformance-testing mode. In a mode where the
   device does not lower its own power, and for a slave whose mode cannot be
   read, a move to a channel whose listed limit is under the listed limit of
   the channel in use is refused (a move home excepted: it restores the limits
   the link lived under).
4. **A mismatch is reported, not repaired.**
5. **Power follows the device.** The prediction allows for the device's
   listed limit at the candidate where it is lower; the plan leaves the power
   setting alone.
6. **A radar channel is never the target of a change made through this
   feature**, whoever asks. Moving to one is minutes of
   silence by law, of a length this platform does not print; and a master
   that hears radar during or after that wait picks its next channel from
   **its own** list, which no plan of Nocturne's controls and which the slave
   may not scan. A plan that disarms and walks away from that is an outage
   until somebody visits. So such channels are scored and shown with what
   they would cost, and are not offered.
7. **Unknown is radar.** A channel whose radar mark the device did not print,
   or printed through a read that has not been checked, is treated as a radar
   channel.
8. **A link that already sits on a radar channel may be moved off it**, by a
   person's approval, with `radio_return_through_radar`: the way home passes
   through a wait, and after confirmation there is no way home at all.
9. **Nocturne expects to find such a link on a frequency it did not choose**,
   records that as the device's move, and does not push it back. And it tells
   the operator, before any pulse arrives, when the slave's list does not
   cover where the master may go (`radar_exit_uncovered`).
10. No command that disables radar detection, shortens a wait, clears a
    barred channel, or re-selects one while it is barred.
11. **Licensed and coordinated spectrum is not a candidate.**
12. **Indoor-only channels** (the device's own mark) are not a target for a
    link whose radios do not both say they are installed indoors.
13. **Say what was relied on.** Every proposal and every change records the
    country and regulatory mode each device reported, the rows of its list
    that were used, and the exclusions in force.
14. **Claim nothing about legality.** "Within what both devices report as
    permitted under country X" is the strongest sentence any screen may
    print.

## RPC, events and settings (contract)

### `radio.*` (`nocturne-radio`)

| Method | Params → Result |
|---|---|
| `radio.status` | → `{links, proposals, held, in_probation, planned: [{link_id, change_id, what, why}], unverified: [String]}`: `planned` is what the background task planned and is waiting for a person (a move back). Also what the interface gates the tab on. |
| `radio.links` | `{site?, device_id?, control?, state?, problem_only?, limit?: 100, cursor?}` → `{rows: [LinkSummary], next_cursor?, not_point_to_point: [{device_id, interface, peers}], coverage: {devices: [{device_id, state, at?, unverified, error?}]}}` |
| `radio.link.get` | `{id}` → `{link: RadioLink, ends: [EndStatus; 2], reach: [Reach; 2], evaluation?: Evaluation, proofs: [Proof], ladder: [{rung, done: bool, missing?: String}]}` |
| `radio.link.save` | `{id?, name?, ends?: [{device_id?, interface?, mac?, seen_as?}; 2], sync_group?, policy?}` → `RadioLink`. Refused when the two ends are in different worlds. Audited. |
| `radio.link.delete` | `{id}`. Refused while a change of this link is applying. Audited. |
| `radio.links.refresh` | `{device_ids?}` → `{job_id}`: collects the two kinds and pairs again |
| `radio.evaluate` | `{link_id}` → `Evaluation`: computed now, and stored |
| `radio.evaluations` | `{link_id, limit?: 30, cursor?}` → `{rows: [Evaluation], next_cursor?}` newest first |
| `radio.surveys` | `{link_id?, device_id?, id?, limit?: 50, cursor?}` → `{rows: [Survey], next_cursor?}`; rows and networks only when `id` is given |
| `radio.survey.request` | `{link_id, end: master\|slave, survey?}` → `{change_id, job_id}`: creates and plans a survey change. Nothing runs until a person approves it. |
| `radio.retune.propose` | `{link_id, to?: Channel}` → `{change_id, job_id}`: creates and plans a retune (default: the latest evaluation's best, recomputed if older than a day). Nothing runs until a person approves it. |
| `radio.prepare.propose` | `{link_id, add: [u32]}` → `{change_id, job_id}`: the slave's list |
| `radio.drill.propose` | `{link_id, end}` → `{change_id, job_id}` |
| `radio.guard.remove.propose` | `{device_id, name}` → `{change_id, job_id}`: plans the removal of a revert that was left behind. Nothing runs until a person approves it. |
| `radio.site` | `{site}` → `{radios: [..], links: [{link_id, current, ranking: [Scored]}], sum_q_now, sum_q_best, co_located_known: bool}` |
| `radio.history` | `{link_id, limit?: 50, cursor?}` → `{rows: [Move], next_cursor?}`: every channel the link has been on; `by: person \| device \| outside \| unknown`; predicted gain and realised gain |

```rust
struct LinkSummary { id, name, control, state, held_by, master: EndBrief, slave: EndBrief,
    channel?: Channel, one_sided: bool, in_probation: bool,
    last_evaluation?: { at, ok, decision, best?, gain?, one_sided, assumed: bool, trial, unverified },
    unverified: bool, lab: bool }
struct EndBrief { device_id?, device_name?, interface?, mac?, manageable, reach, signal_dbm?, snr_db?, quality_pct? }
struct EndStatus { end: LinkEnd, radio?: Radio, peer?: RadioPeer, read_at?, read_error?, not_available: [String], unverified: bool }
struct LinkPolicy { committed_down_bps?, committed_up_bps?, exclude_mhz: [(u32, u32)], evaluate: bool }
struct Move { at, from?: Channel, to: Channel, by, change_id?, predicted_gain?, realised_gain?, outcome? }
```

Events: `radio.link.changed {id}`; `radio.state {link_id, state, held_by}`;
`radio.evaluated {link_id, ok, decision, best?, gain?, one_sided, assumed,
trial, unverified, at}`; `radio.survey.stored {link_id, survey_id, ok}`;
`radio.moved {link_id, from, to, by}`.

Settings section `radio`:

```
{ enabled: true, evaluate_at: "12:00",                          // the machine's local time, as schedules are
  evidence_days: 7, busy_hours: [19, 23],                       // used only where a link has no traffic history
  headroom: 1.5, mac_efficiency: 0.7, fade_margin_db: 10,
  own_coupled_dbm: -45, assumed_busy: 1.0,
  min_gain: 0.10, min_ratio: 1.20, sufficient: 0.95, explained_within: 0.15,
  persistence: { days: 3, window_days: 5 }, survey_max_age_h: 72,
  probation_h: 48, probation_worse_by: 0.05, probation_fade_db: 5, quarantine_days: 30,
  pair_after_reads: 3 }
```

Read key by key, each brought into range.

### Added to `changes.*` (`nocturne-changes`)

| Method | Params → Result |
|---|---|
| `changes.create`, `changes.update` | accept the five radio intents and `radio?: RadioContext`; refuse a second target beside a radio intent, and any `options` that differ from the engine's fixed ones (the planner refuses the same again) |
| `changes.reach` | `{device_id, interface}` → `{reach: direct\|through_link\|behind_link\|unknown, via?, evidence: [String], not_seen: [String], last_test?: {change_id, at, passed, seen}, as_of}`: from the stored reads, with their age; it opens no session. There is no method that states a reach. |
| `changes.guards` | `{device_id?}` → `[{device_id, mac, name, change_id, restores: [{property, value}], armed_at, due_at, open}]`: the ledger |
| `changes.guards.check` | `{device_id}` → `{job_id}`: reads `leftover` on that device, a policy-checked read, and closes the ledger entries the read shows gone |
| `changes.proofs` | `{device_id?, interface?}` → `[Proof]` |
| `changes.rollback` | for a retune: `{id}` → `{change_id}` of a **new planned change**; for any other radio change: refused, with the reason; sends nothing in either case. For an ordinary change, the derived lines leave every radio alone (`rollback.rs`). |
| `changes.assistant_apply` | refuses a change with a radio intent, first thing, in every state and every autonomy mode |
| `changes.delete` | refuses a change that a valid proof or an open ledger entry refers to |

`changes.progress` gains the step names `lock`, `hold`, `ledger`, `reach`,
`arm`, `armed`, `set`, `follow`, `reach_test`, `management`, `verify`,
`confirm`, `disarmed`, `revert`, `home`, `survey`, `drill`, `guard_remove`; a
step's `detail` for a radio change names properties and never quotes
configuration.

Settings, in the `changes` section: `radio: {stable_s: 60, verify_budget_s:
120, confirm_margin_s: 30, follow_poll_s: 2, max_signal_drop_db: 6,
max_deadline_s: 1800, unknown_wait_s: 600, drill_delay_s: 60, plan_valid_h:
12, lab_stall_s: 120}`.

### Added to the core (`nocturne-app`, `nocturne-transport`)

* `connect::hold(app, device_ids, reason) -> Hold`: while it lives, every
  other automated session to those devices fails at once with the reason; the
  holder's own sessions pass.
* `connect::device_clock(app, device_id) -> DeviceClock {now_ms, sleep_until}`:
  Nocturne's clock for a real device; for a lab device the lab's simulated
  time, which the lab provides through `SimConnector`: a new method **with a
  default body** that answers "no lab clock", so the trait's existing
  implementations keep compiling until their own pieces land.
* `connect::in_band(connection, path_up, clock, timeout_s)`: the one
  definition of a session that depends on a simulated path. It stalls while
  the path is down and is closed when the path has been down for the
  timeout. The lab's connector and the test adapter both use it, so the
  change engine is tested against the behaviour it will meet in a lab.
* Behind the `testing` feature only: a connector hook for devices that are
  **not** lab-marked. Today the only way a test can serve a fake device is
  the simulator hook, which needs the lab marker, so every rule this file
  states for a real device would otherwise be untestable. That no shipped
  binary carries it is a CI step, not a test (under "Tests").
* `connect::seen_from(app, device_id)`: the address the device sees the
  session come from, or "not known" (step 1 of reach). The transport's
  `ConnectionInfo` gains the socket's local address.
* `collect`: the stored `unverified` mark for the two radio kinds is per row.
* `nocturne-drivers`: `Interface.parent` and `Route.active`, both optional,
  for the reach walk (steps 2 and 3).

### Added to `monitor.*` (`nocturne-monitor`)

`monitor.radio` and `monitor.radio.updated`, above; setting
`radio_interval_s`; the lab's clock for lab devices and `radio.clock_ms`.

### Method names the boundary tests must find absent

From the assistant's allow-list and from the list schedules and triggers
call: `changes.guards.check`, `radio.link.save`, `radio.link.delete`,
`radio.drill.propose`, `radio.prepare.propose`, `radio.guard.remove.propose`.
The second list also lacks `radio.survey.request` and `radio.retune.propose`.

From the radio feature's own list of calls (`calls.rs`): its test asserts
that the list is exactly the fifteen names under "Links", so any other name,
and among them every method that approves, applies, rolls back, cancels or
changes a setting, fails it.

No method named `changes.reach.state`, `changes.reach.forget`,
`changes.standing.*` or `changes.standing_apply` is registered: a reach
cannot be stated or withdrawn, and there is no standing approval in this
build.

## The lab (`nocturne-netsim`, `nocturne-lab`, `nocturne-app` testing)

"Lab devices are devices" is not yet true of a radio pair: the simulator has
no radio medium, its one access point has no command line, and a lab device
is reached through its console, out of band, so cutting a simulated link can
never cut Nocturne's session to the device behind it.

**The lab radio is built to be the same kind of thing as the first-build
platform**, so that passing in the lab says something about RouterOS. It is
not RouterOS (the simulator speaks one dialect, prints no trademark, and that
stays), but it shares the shapes that decide whether the code is right:

| RouterOS legacy | The lab radio | What that exercises |
|---|---|---|
| settings are exec-level lines that take effect and persist at once; no configuration mode, no save | the same | the pipeline's path with no config mode and no save |
| one export line per radio holding every property, the link's key among them | the same | the configuration check by line pattern and read-back; secrets staying out of change records |
| `scan ... rounds=1`: off the air, transmitters decoded, no airtime, no word on what it covered | the same (`scan rounds 1`) | the `networks` class, the own peer in the slave's scan, coverage from the list |
| a revert armed at the exec prompt as a quoted command string on a timer, **with a quoted string inside it**; first run one interval after it is added or after a restart; a run count that a restart resets; configuration, so it shows in the export; times printed in the platform's own forms, not ISO | the same (`schedule add`) | arming; the escape table on text that needs escaping; the read-back with the time, through the declared formats; self-removal; the mark; leftovers; a reboot with the guard armed |
| the device prints which channels are allowed, their power and marks; **no radar wait** | the same | radar status from the device, the wait learned by measuring |
| the slave walks a list; `default` is a word, not frequencies | the same | preparing, opaque entries, follow time growing with the list |
| follow time unknown | a property of the lab that the profile's estimate does not know | timers that start as estimates and are replaced by measurements |

The lab radio is no more than the first-build platform: its one survey is
`scan`, which hears networks and measures no airtime. (A survey that measured
occupancy would only have served an automatic move forward, which is not in
this build; the score's measured path is tested on its own, in `nocturne-rf`.)
Where a lab differs from a real link it is in the table under "What a lab
link relaxes", and nowhere else.

### One written description of the lab radio

This section is that description. The simulator implements it, the profile
`nocturne_sim_radio` parses it, and the test fake is the simulator behind an
adapter, so there is one implementation and nothing to drift. A catalogue
model `radio-bridge`: `GigabitEthernet0`, `Dot11Radio0`, a management address
on the bridge of the two; the IOS-style prompt and the existing `show
version` (the software a proof is tied to, and the uptime a restart test
reads), `show ip route`, `show arp`, `show mac address-table` and `show
interfaces`.

**Settings.** One line each, at the privileged prompt, effective and stored
when accepted; a success prints nothing:

```
radio <if> set role {root|non-root|off}
radio <if> set ssid <word>
radio <if> set key <word>
radio <if> set channel {<MHz>|auto}          # the control frequency; stored and ignored on a non-root
radio <if> set width {20|40|80}
radio <if> set offset <MHz>                  # centre minus control: 0, 10, -10, 30, ...
radio <if> set scan-list {default|<MHz>[,<MHz>...]}
radio <if> set country <word>
radio <if> set regulatory {normal|test}
radio <if> set max-stations <n>
radio <if> set tx-power <dBm>
```

Refusals are the dialect's: `% channel 5123 is not allowed in country zz`
(nothing changes; a node option `accepts-disallowed` makes the radio accept
it and go `down` instead, which is the worst outcome the RouterOS manual
describes), `% unknown radio`, `% invalid value`.

**In `show running-config`**, one line per radio, every property, this
order, as `name=value` pairs after the interface (so the stack's
`line_parser` is `kv`, as it is for RouterOS):

```
radio Dot11Radio0 role=root ssid=TOWERS key=7Q2xLab channel=5745 width=40 offset=10 scan-list=5745 country=zz regulatory=normal max-stations=1 tx-power=23
```

and one line per schedule entry: `schedule <name> startup interval <s> run "<text>"`.

**Reads.**

```
NAME#show radio brief
Interface    Role      State      Channel  Width
Dot11Radio0  root      up         5745     40

NAME#show radio Dot11Radio0
interface: Dot11Radio0
role: root
state: up
ssid: TOWERS
address: 02:00:5e:10:00:01
channel: 5745
width: 40
offset: 10
configured-channel: 5745
scan-list: 5745
country: zz
regulatory: normal
tx-power: 23
noise-floor: -95
stations: 1
max-stations: 1
root-address:

NAME#show radio peers
Interface    Address            Name     Signal  Remote  Noise  SNR  TxRate  RxRate  Uptime
Dot11Radio0  02:00:5e:10:00:02  TOWER-B  -55     -56     -95    40   360.0   360.0   3600

NAME#show radio channels Dot11Radio0
Channel  Widths    MaxPower  Flags
5180     20,40,80  20        indoor
5500     20,40,80  27        radar
5745     20,40,80  30        -

NAME#show radio scan-list Dot11Radio0
5745,5765,5785,5805,5825

NAME#show clock
02:33:00 UTC Mon Jan 1 2024
```

`state` is one of `up`, `radar-wait`, `scanning`, `searching`, `down`.
`root-address` is filled on a non-root that is associated. `key` is never
printed by `show radio`. `show radio peers` with no peer prints the header
only; `show radio brief` on a node with no radio prints `% no radio`, which
the profile declares as the stack being absent. Rates are in Mbit/s, uptime
in seconds. The clock is the lab's simulated time counted from Monday
1 January 2024, 00:00:00, moved by any `ClockStep` fault (below). It is
printed in the dialect's own form and `next-run` (below) in a second one, so
that neither can be read without the formats the profile declares.

**The survey.** It takes the radio off the air for as long as it runs and
prints when it ends:

```
NAME#radio Dot11Radio0 scan rounds 1
Address            SSID      Channel  Width  Signal  Noise  Name
02:00:5e:20:00:07  NEIGHBOR  5745     20     -70     -95    nb-7
```

`scan` walks the effective scan list, one simulated second a channel, and
lists every decodable source it heard above -90 dBm, its own peer included;
it prints no line saying what it walked. Without `rounds` it runs until the
console closes.

**The revert.**

```
schedule add <name> startup interval <seconds> run "<text>"
schedule remove <name>
show schedule [<name>]
log mark "<text>"
show log marks
```

`<text>` is exec lines separated by `;`; inside the quotes `\"` is a quote
and `\\` a backslash. The first run is `<seconds>` after the entry is added,
or after the node restarts with the entry present; it repeats at that
interval until removed; it runs with no console attached; a line that is
refused ends the run. `run-count` is the number of runs since the node last
started. `schedule remove` of a name that does not exist prints nothing.
`show schedule` prints one line per entry and nothing when there is none.
Armed at 02:33:00 with the lab's timers (30 + 20 + 10 + 30 + 10 = 100 s):

```
name=nocturne-guard-k3xmtq7mw2abzcde interval=100 next-run=2024/01/01-02:34:40 run-count=0 run="schedule remove nocturne-guard-k3xmtq7mw2abzcde; radio Dot11Radio0 set channel 5745; log mark \"nocturne-guard-k3xmtq7mw2abzcde ran\""
```

`show log marks` prints one line per mark: `Jan  1 02:34:40: <text>`.

**How the radios behave.** Every time below is an exact function of the
event that causes it, the lab's seed and what it describes, never something
discovered by a tick: one `advance` and a thousand that add up to it give the
same instants, and a test says so.

* A link is up when both radios are enabled, one is root and one is not, the
  names and keys match, the root's wait is over, and the non-root has found
  it: `2 s × (the position of the root's control channel in the non-root's
  effective list, from 1) + 3 s` after the later of "the root began serving"
  and "the non-root began searching". A non-root whose list does not hold the
  channel never finds it. A non-root prefers no other root: with two roots of
  the same name in range it takes the first in its list order.
* A root set to a channel its country marks `radar` stays in `radar-wait` for
  a wait the device does not print: 60 s, or 600 s for the channels the lab's
  country table marks long. `Fault::Radar {node, control_mhz}` makes the root
  leave exactly 10 s later for the next channel of **its own** effective
  list, and bars the old one for 1800 s.
* Node options, each off by default, for the things nobody knows about the
  real platform: `list-change-restarts` (setting the scan list restarts the
  search), `same-value-restarts` (setting a property to its present value
  restarts the radio), `accepts-disallowed`. And three that named tests
  need: `software <text>` (the version the node reports; a proof must die
  when it changes), `unmarked-radar <MHz>` (a channel the list prints with
  `-` and the radio treats as a radar channel: the target that turns out to
  be one after all), and `site <name>` (the mast the node stands on; a node
  without one is a site of its own, so radios share a site in a lab only
  where the lab says so).
* Faults for the tests: `GuardRunsAtOnce`, `GuardRunsLate {by_s}`,
  `GuardKeepsEntry` (its self-removal silently fails), `GuardStopsAfterRemove`
  (the lines after it do not run), `ScanNeverEnds`, `Radar`, `Interferer`
  (adds or removes an RF source), `ClockStep {node, by_s}` (the node's clock
  jumps, as a time server would make it), and the existing power-off for a
  reboot.

**A radio medium.** `CableKind::Radio` joins two radio ports with a
`path_loss_db`. Signal at an end is the other end's power less the loss.

**An RF environment that is a pure function of the seed.** `LabSpec.rf` lists
sources: at which node each is heard, centre, width, level, whether a scan
can decode it, how busy it is (constant, or by hour of the simulated week),
and an optional `penalty` in dB. Whether a source is transmitting in a given
second is the stateless mixer of the seed, the source and that second,
against its busy share.

**The simulator's air is its own, and simpler than the engine's.** It does
not link `nocturne-rf` and does not copy it; its dependency rule stands. In
integer hundredths of a decibel: the noise at a node on a channel is the
largest of the thermal floor for the width, the strongest source
transmitting inside the channel (less a table value for the share of it that
overlaps), and sources just outside it less the leak table. The modulation
is the highest whose threshold the signal-to-noise meets, from the same
default table the engine uses; a link whose signal-to-noise stays under the
lowest threshold for 10 s drops. So a lab's achieved rates are produced by
something that is not the scoring engine, and the two agree only where the
model is right about that lab: a test in the lab crate pins the two default
tables equal, and pins that on the built-in lab they pick the same
modulations. **A lab proves the control loop: pairing, reach, the survey as
an outage, the order, the follow wait, the dead-man, every way home,
probation and its move back. It does not prove that the model fits real
air**, and the lab's screen says so. A source's `penalty` makes the achieved
rate fall short of any prediction, so that a miss can be shown.

**In-band management** (`nocturne-lab`). A lab node may be `management:
in_band`. Its session is opened only when the lab's management station can
reach the node's address over the simulated network, stalls while the path
is down, and is closed when the path has been down for `in_band_timeout_s`
(20 simulated seconds). That behaviour is defined once, by
`connect::in_band` in the core, and the lab has no version of its own. The
address the session comes from is the management station's. Radio bridges are in-band by default in a lab that has a
management station; every other node stays on its console unless asked. A
node option `station-ports` makes a root learn its stations on a per-station
port (`Dot11Radio0.1`), which is how reach meets the arrangement it must not
mistake for a cable.

**The profile** `nocturne_sim_radio` is kept beside the grammar it describes
(`nocturne-netsim` exports its text; the simulator does not read it). The lab
crate registers it, as it registers `nocturne_sim_host`. Its captures are
the simulator's own output and say `synthetic`.

**The maturity gate, for a lab device.** A profile a feature ships is
`community`. The gate asks whether a device type was checked against the
hardware it runs on; for a lab device that hardware is the simulator, and
the lab crate's tests check this profile against the simulator's real
output. So a plan on a lab device **starts** from `verified` when, and only
when: the device carries the lab marker (which forces the simulated
transport, so no real radio can wear it), the profile is
`nocturne_sim_radio`, and the profile in effect is the shipped one, not a
user's override of the same id. From there the labels work as anywhere
else: an operation or a read listed in the profile lowers the plan to
`unverified`, and nothing raises it. The shipped lab profile lists none.
Simulated hosts stay as they are.

**A built-in lab**, "Two towers": a management station and a switch at the
NOC; the **tower link**, a root bridge at the NOC on that switch and a
non-root bridge on mast T2, reached only through the radio, with a switch
behind it; on T2, on that switch, the **relay link**, a root bridge whose
non-root is on mast T3; and RF sources laid out as in N1 and T6. Each node
has the site of its mast. The tower link rehearses what a real RouterOS link
can do: prepare the slave through the link, survey the near end, test the
revert, an approved retune, the way home. The relay link shows the reach
rule deciding the other common case: its root is behind the tower link, so it
is read and recommended for and never moved, and its non-root is reached
through its own link. Nothing loops: every radio bridge has one cable.

**Fakes** (`nocturne_app::testing`). `testing::radio_world(spec)` is a
simulator world behind an adapter, built from a `LabSpec`: console in, console
out, in-band sessions through `connect::in_band`, simulated time advanced in
step with tokio's paused clock at a stated `speed` (1 by default; the tests
of the clock run it at 2 and at 0.5). `testing::radio_pair` is one preset of
it, the usual link: a master on a cable and a slave across the link. Its
knobs: the simulator's options and faults; how each end is managed
(`wired`, `in_band`, `none`); the node sites; a third radio of the same
network name, for the slave that joins somebody else; **a master whose
session runs over its own link while its tables are those of a wired master**
(the case the reach test exists for, which no consistent topology produces);
and `lab: bool`. With `lab: false` the devices carry no lab marker and are
served through the testing-only connector hook, and they speak
`test_sim_radio`: the lab radio's profile text registered under that id as a
**built-in, `verified`** profile, with the `radio:` labels a test asks for
(reads and operations by role; none by default). So a test of the real
column meets real limits and real intervals under the paused clock, a plan
can be `verified`, and a test of maturity lists exactly the one label whose
effect it tests. The core's `testing` feature therefore depends on the engine
`nocturne-netsim`. Nothing else in the core does.

### What a lab link relaxes, all of it

Every duration of a lab link is simulated time. Beyond that, exactly this
differs, and nothing else may:

| Rule | Real link | Lab link |
|---|---|---|
| follow and management estimates | the profile's (180 s, 120 s) | the lab profile's (30 s, 20 s): data, like any profile's |
| `stable_s`, `verify_budget_s`, `confirm_margin_s`, `drill_delay_s` | 60, 120, 30, 60 | 10, 30, 10, 20 |
| the radio lock | one for all real devices | one per lab |
| the monitor's radio cadence and stamps | Nocturne's clock, every 300 s | the lab's clock, every 30 s, with `radio.clock_ms` |
| evaluation | daily, and when a survey is stored | every 60 s, and when a survey is stored |
| busy hours | the top quarter of the link's own traffic hours | every hour |
| persistence (row 5) | surveys on 3 days of 5, one at each end in busy hours | one survey at each end |
| coverage (row 6) | busy hours on 3 days | one observation |
| probation | 48 h and a busy period, hour of the week against hour of the week | the 300 s after the move against the 300 s before it, as two means, with at least 5 samples left on each side after the fade rule |
| maturity | the profile and its labels | starts at `verified` for the shipped lab profile on a lab device; its labels can only lower it |

Engine tests run against `testing::radio_world` (mostly its `radio_pair`
preset) under the paused clock. Those that test a rule of the real column say
`lab: false`, and the test list marks them. Only the suite's end-to-end run
uses a lab.

## The interface

No new entry in the navigation rail. **Monitoring → Radio links** is a tab
offered when `radio.status` answers. The plan and its approval stay where
every plan is approved, in **Changes**.

*The list.* One row per link: name; both ends with their roles and how each
is reached; what Nocturne controls; channel; signal at each end;
signal-to-noise and quality where available; state; last move. Below it,
"not point-to-point": masters with several peers.

*A link.* The two ends side by side, every figure with its age. Charts of
signal, signal-to-noise, quality, rates and frequency, moves marked. The
ladder: which rungs are done and what the next one needs ("the revert has
not been tested on TOWER-B since its upgrade to 7.16"). The evidence: a strip
of the week showing which hours have observations at each end; surveys per
end with their time, hours, kind and coverage. The candidates in their three
kinds: possible targets ranked with both directions, `Q`, `harm` and `A`;
scored and not a target, each with its reasons; not scored, each with its
reason. The recommendation in a sentence, with what is holding it. The
history of moves, predicted gain against realised.

*What a person can do*, each a button that creates a change and opens it in
Changes: **Survey this end…** (states the outage; disabled, with the reason,
for an end that is not reached directly); **Test the revert…**; **Add to the
slave's list…**; **Move to 5795/40…**; and, on a device the ledger names,
**Remove the leftover revert…**.

*A radio change in Changes* has its own plan view: the steps in order, the
device each runs on, how that device is reached and on what evidence (the
walk, hop by hop, and what it could not see), the latest reach test where it
did not pass, the exact lines, the dead-man and when it fires, the expected
and the worst-case outage in seconds with "estimated" where they are, what
verification waits for, the engine's recomputed figures beside the
proposer's, and the operations in it that have not met a real device. A
check that was not run reads differently from one that passed. A return with
the link down says above its confirmation box what typing the names
confirms. The ledger of a device is shown on its plan, with **Remove the
leftover revert…** where an entry is open.

*Nothing is automatic, and the screens say so.* The link page says, under
the recommendation, "Nocturne does not change this link by itself: every
change is a plan you approve in Changes"; a move back that probation asked
for appears as a planned change linked from its alert.

States that must read differently everywhere, in words and not only in
colour: not collected for this device type; collected and no radio; the read
failed; held by a radio change; this figure does not exist on this stack or
protocol; seen from one end only; never surveyed, surveyed and measured
quiet, a network heard, nothing decoded; an assumption, by name; a trial;
radar, not radar, radar status unknown; reach computed, a reach test that did
not pass, and what the walk could not see; and
"this read has not been checked against a real RouterOS device" above every
table and chart built on an unverified read and on every plan built on an
unverified operation.

Devices → a device → Data gains the two tables (the two kinds join `Kind`,
`KINDS` and the table labels). The lab editor gains the radio cable, the
radio node with its site, RF sources and the faults. TypeScript types in
`ui/src/api/types/radio.ts` (new), with additions in `core.ts`, `changes.ts`,
`monitor.ts`, `lab.ts` and, for the fourth command class, `terminal.ts`.
No route is added: the radio tab lives under `monitoring/:tab?` and the radio
plan view inside the existing change page.

## The assistant

It may read link state and propose. It can never apply a radio change, and a
proposal of its making waits for a person like any other.

| Tool | Reads or writes | Through |
|---|---|---|
| `radio_links` | reads | `radio.links`, `radio.link.get` |
| `radio_evaluation` | reads | `radio.evaluations`, `radio.site`, `radio.history`, `radio.surveys` |
| `get_collected` | reads | the two new kinds |
| `propose_radio_retune` | creates a planned change that waits for a person, like `propose_change` | `radio.retune.propose` |
| `request_radio_survey` | creates a planned survey change that waits for a person; the tool's answer states the outage | `radio.survey.request` |

`changes.assistant_apply` refuses radio changes before it looks at anything
else. What the assistant is told about a link carries the same labels a
person sees: one end only, never surveyed, heard, nothing decoded, assumed,
trial, unverified. Radio reads pass through redaction like any device
output, and **every string a radio reports** reaches a model only inside a
block marked as untrusted device output: network and radio names in a
survey, and equally a radio's own name and network name and the names it
reports for its peers (`peer_name`, `seen_as`), through `get_collected` and
`radio_links` alike. Strangers choose some of those names, and an
operator's own radio can carry a name a stranger set.

## Open questions for the owner

1. **Which radios do you run?** RouterOS legacy wireless, the newer `wifi`
   package, 60 GHz, Ubiquiti (which families), others. The first build
   assumes RouterOS legacy.
2. **Are your links on radar (DFS) channels?** In much of the world every
   outdoor 5 GHz channel is one. This build never moves a link to a radar
   channel, so where every channel your radios list is one, it reads, alerts
   and recommends, and moves nothing.
3. **Is the master of your links at the near end, and how is management
   carried?** This build moves a link only when its master is reached
   without crossing a radio, and it has to be able to show that. It walks the
   way back from the master to Nocturne through the routers in the inventory:
   **every router between Nocturne and your radios must be in the
   inventory**, unless Nocturne sits on the radios' own management subnet. A
   management VLAN is followed to the port it rides on. Nobody can overrule
   an "unknown"; the answer names the fact that would settle it.
4. **Can you record from a pair?** The list under "Not verified" is ordered:
   the first two items are read-only and take minutes from the terminal.
5. **Do your radios keep time (NTP)?** Nocturne does not need their clocks to
   be right, only not to be stepped while a change runs: a clock that moves
   in the middle of a change makes the change go back. A time server that
   steps a clock nightly, or summer time on a device that keeps local time,
   would send some changes back for that reason alone.
6. **A dedicated RouterOS account** with the `test`, `write` and `policy`
   rights and without `sensitive`: acceptable? The revert test is how you
   find out whether it is enough.
7. **Is `site` filled in on your devices?** Without it, the operator's other
   radios on the same mast are left out of the score, and the recommendation
   says so.
8. **Does any link have a second way to its far end?** Such a link can have
   both of its ends surveyed, which is what a move forward by itself would
   one day need (under "Automatic changes: not in the first build").
9. **Ubiquiti by SSH?** The vendor's banner says command-line changes void
   the warranty.

## Not building, and why

Each row that is out because of something nobody has seen says what would
bring it back.

| Not built | Why | What would bring it back |
|---|---|---|
| **Moving a link to a radar channel** | Minutes of silence of a length the platform does not print; and a master that hears radar during or after it moves to a channel from its own list, which the slave may not scan. With a revert disarmed that is an outage until a visit. | The effective scan lists of both roles recorded (R2), a move to a radar channel timed on a bench and what a master does after a detection recorded (R4), and then a planning rule that the slave's effective list holds every frequency the master's can select. |
| **Moving a master that is reached through a link** (or behind another, or of unknown reach), **and letting a person declare such a master directly reached** | The change itself could be made safe. The hours after it cannot: once confirmed, a channel that was only ever seen at 03:00 can go bad at 19:00, and a master nobody can reach has no way home. | A revert that stays armed as a lease through the first busy period and is renewed only while the link is healthy, which needs the dead-man recorded across a reboot and a clock step (R5) and a record of moves predicted and then measured on this installation. |
| **Surveying an end reached through the link** | Nothing could be read back (the result file's format is not documented), and nothing brings back a radio that stays searching: a survey has no configuration for a revert to restore. | R6: what the saved file holds, what happens to the session, and a restart-the-radio revert that has passed a drill. |
| **Surveying a master that sits on a radar channel** | It has to listen again afterwards, for an unmeasured time, and may be sent elsewhere while it does. | R6, on such a master. |
| **The drill's second stage on a master that sits on a radar channel** | A same-value set may restart the radio, which then waits for radar for a time nobody has measured, and a detection meanwhile sends it to a channel of its own choosing. Such a master borrows its second stage from another radio of the same role, software, account and line; where no such radio exists, it cannot yet be moved off its radar channel. | R4 with a radar channel timed on a bench, R6 on such a master, and then a second stage that waits that wait out. |
| **Unattended surveys** | An outage nobody approved. | A survey shown to leave the link up (the 802.11 background scan, a second-radio analyser) becomes an ordinary read-only kind the day a recording shows it does. |
| **Any change without a person**: a move forward by itself, a move home by itself, a standing approval | A move forward would rest on evidence this build cannot gather on a real link (occupancy at both ends); a move home by itself could only run once RouterOS radio plans are `verified`, which no shipped profile allows yet, and a person's click on the ready move back does the same. Under "Automatic changes: not in the first build". | R1 to R5 lifting the labels; for a move forward also R6 and a link with both ends reached directly; a record of predicted-then-measured moves; and a specification of its own for a standing approval. |
| Leaving a master on the old channel with the link down | No change leaves a link where nothing was verified. A person who wants it sets it by hand, and Nocturne records an outside move. | Not planned. |
| **Declaring a reach**, or withdrawing what a reach test saw | The cure for an `unknown` is the missing fact (usually a router to put in the inventory); a statement would be the refused move entered by hand. A reach test that did not pass decides only its own change and a survey of that master, and the next retune that passes overwrites it. | Not planned. |
| A quoting rule for device names that are not plain words | None has been seen to work. Such a radio is read as far as its listing and never written. | R5 includes a radio whose name has a space. |
| An automatic restart of a radio that did not return from a survey | Whether a restart cures it is somebody's memory. The session to that device still works and a person is there. | R6. |
| Changing channel **width** | A second change, and no manual read says how a connected slave follows one. The score says when a narrower channel would serve better. | A platform shown to coordinate it. |
| A "reactive" mode that moves a link because it is failing now | That is when the control path through the link is least reliable. | Not planned. |
| Surveys and changes on RouterOS `wifi`; anything but reads on `w60g` | See "The first build, by platform". | The owner's answer, and recordings R7 and R8. |
| Ubiquiti, Cambium, Mimosa, InfiNet, Siklu | No profile and no recorded output. | A profile with a `radio:` section and recordings. |
| Moving a radio that chooses its own channel; licensed, split-frequency and synchronised links | Two controllers undo each other; one legal frequency. | Not planned. |
| A country table, a table of radar waits, anything that adds a channel the device does not offer | The software does not know the law. | Not planned. |
| Raising transmit power; antenna alignment; antenna gain | A channel change cures none of what those are for. | Not planned. |
| Spectral scan, a spectrum display | Not on the `ac` chips under the legacy package; effect on a live link not documented. | |
| RouterOS Safe Mode as the dead-man; a script object for the revert | See "The dead-man switch". | |
| A revert the device decides for itself ("go back only if no peer registered") | "Unconfirmed means undone" is the one contract simple enough to trust on a device nobody can reach. | |
| **Taking an entry out of a slave's list**, the undo of a prepare included | An entry the master may one day use, removed with nobody watching, is the classic way to strand a link; and an undo defined as "the list as it was before" also drops what was added since. A prepare that fails is undone inside its own change; one that was applied is not undone by Nocturne. | A specification of its own: the order, a guard through the link that puts the longer list back, verification, refusal while a planned retune names a frequency it would drop, removal of exactly what the named change added, and both tests. |
| Stripping the dead-man's lines out of stored backups | A stored version should be what the device had. No backup is taken while a radio change holds the device, and a stored version that does hold a revert cannot arm it again: no plan, no rollback line and no derived rollback may re-create one. | |
| Vendor controllers (UISP, cnMaestro) as the route to a radio | Another system with its own approval model. | The owner's decision. |
| A second way to run a command that is not read-only | The survey goes through the change pipeline. There is no "gated exec". | |
| The assistant applying, or schedules and triggers applying | The project's rules. | |
| Radio figures over SNMP; radio links drawn in topology; the ISP edition reading link capacity | Later; each would read by name. | |
| A fix for RouterOS changes in general | A `Lines` change to a RouterOS device still cannot be predicted and still rolls itself back, now with a derived rollback that leaves every radio alone. Typed radio targets have their own exact prediction; the general problem is left where it is, named. | |
| A skew check of the device's clock against Nocturne's | It needs the device's time zone, which no read has shown, and it refuses devices that are not at risk: what is dangerous is a clock that moves during a change, and that is read. | |

## Not verified

**Nothing in this specification has been run.** No radio was available to any
of the research, no Rust was compiled for it, and no line in it was sent to a
device. It was written from four research files, a draft, three reviews of
that draft, and the parts of the code those cite, read again where a point
had to be settled: the change model, the maturity gate, the assistant's apply
handler, the on-demand rollback, how a step is published, the guard planner,
the command classes, the profile's guard and step shapes, the per-device
session gate, how shipped and user profiles are told apart, and the vendor's
reference page for the scheduler as the research had fetched it.

*Not known about RouterOS, each answered by a recording below:* what any
legacy wireless read prints on a slave; how a 40 or 80 MHz channel is
written in the live channel string; whether `proplist=` works with `detail`;
what the `info` commands print and how they take the interface; what `scan
rounds=1` prints on a session with terminal detection off, how long it
takes, and what a session through the link does meanwhile; whether
`frequency-monitor` can be bounded and whether it drops the link; what the
console answers to a frequency the country does not allow, and what the
radio does then; whether changing a scan list restarts a connected slave;
whether setting a frequency to its present value restarts a master; **how
long a slave takes to find a master that moved** (no manual of any vendor
read for this work states it); everything about the scheduled revert on
version 6, and on version 7 everything beyond the reference page's words;
the radar wait on any channel; what a master does after a radar detection;
which rights each step needs; whether a station lists its access point in
its registration table; what `bridge` mode and `max-station-count` do (the
single-peer rule is remembered); how each version prints a date and its
clock, and what a stepped clock does to an armed entry; whether the log keeps
a mark across a restart; whether `default-name` is in the detailed listing;
what a VLAN's listing calls the interface it rides on; whether a route
listing marks the routes in use; whether `no_country_set` is the word for no
country.

*This file's own definitions, to be tested against real links before they
are trusted:* how a blind end's present figures are taken from passive
reads; `Qach`; the "explained" bound; the fade rule of probation and its
count of lost reads; the reach walk (it has never met address translation, a
VRF, a routed radio, a pseudo-bridge station or a host with two gateways, and
no bridge table has been read just after the traffic it judges); the
executor's reach test (it has never met a slow or lossy management path, or
a slave that comes back within two reads); every timer's default.

*Assumed by the method and carried here as defaults:* the SNR table, the leak
table, the two-state mix, the 0.10 and 1.20 thresholds, persistence,
probation, the coupling of a neighbour on the same mast.

*Remembered, in the lab's design only:* nothing of the lab radio is modelled
on a real product's commands; the shapes it shares with RouterOS are listed
above and are the documented ones.

### What is labelled unverified, and where the label is read

| Thing | Label | Lifted by |
|---|---|---|
| Legacy reads, master role | `unverified.reads` on stack `wireless`; the banner on every table, chart, alert and evaluation built on them | R1 on a master |
| Legacy reads, slave role | the same, role slave | R1 on a slave |
| Width and offset of the channel in use | "taken from the configured width; the live value could not be checked" | R1 |
| Retries, CCQ and rates under Nv2 and Nstreme | "not available under this protocol" | R1 on such a link |
| `wifi` reads; `w60g` reads | per stack and role; only `wifi print detail` may be `documented` | R7; R8 |
| The allowed list, radar marks, permitted power, restrictions | operation `allowed`: no candidate from the device, every channel treated as a radar channel, no retune, no survey of a master, no second stage of a drill on a master | R2 on both roles |
| The effective scan list | operation `effective_scan_list`: coverage is the plain numbers of the configured list; `radar_exit_uncovered` says "cannot be told" | R2 |
| The shape of the radio's line in the export | operation `config_check`: said in every plan | R3 |
| The single-peer rule | operation `single_peer`: every master is read as able to serve many, and `radio_master_serves_many` is on every retune | R4 |
| The change lines | `set_channel`, `set_scan`: the plan's maturity is `unverified` and the gate refuses it unless `changes.allow_unverified_writes` is on, in which case the plan says so and carries `radio_unverified_write` | R4 |
| Follow time, management return, outage | `timing.measured: false`; "estimated, never measured on this link" beside every stated outage until the link has its own figure | R4, and then each link's own moves |
| The dead-man: when it first runs, quoting, self-removal, rights, a reboot, a clock step, the two date formats, a mark across a restart | `guard.unverified: [master, slave]`: the gate, as for the change lines; and the drill per device, software version and account, always | R5 |
| The survey parser, the survey's effect on the link | operation `scan`: the gate; every stored survey carries `unverified`; the class is "heard / nothing decoded", never "measured" | R6 |
| Reach in bridged and per-station arrangements | `reach.evidence` names each hop of the walk and the rule that decided it; anything not positively a cable is `unknown`; what could not be seen is listed | R9 |
| The model's fit to real air | on the lab's page and in `docs/STATUS.md`: "the lab proves the control loop; its air is the simulator's own" | only by moves that were predicted and then measured |

`docs/STATUS.md` gets a "Radio links" entry that says the same, by platform.

### The recordings

How to record, so the files are usable: through **Nocturne's own session**
(the terminal page; `devices.exec` once a command is read-only), because the
profile logs in with the suffix that turns terminal detection off and the
output differs from an interactive console. One device per RouterOS major
version in use (6.49.x and 7.x), legacy wireless package. Note board,
version, protocol (`nv2` and `802.11` both, if both are in use) and width.
Save each output as `tests/profiles/mikrotik_routeros/captures/<case>/<command
slug>.txt` with `origin: recorded`, the stack and role (and, for the command
of an operation, the operation and its rows) in `expect.yml`, and a
`SOURCE.md`; a change or a revert is saved as a session. Secrets appear in full listings: redact before a file leaves the
machine. `M` is the master, `S` the slave, `wlan1` the radio. R1 to R3 and R9
are read-only and safe on a production link. **R4 to R6 are for a pair on a
desk at low power and must not be first tried on a tower.**

The command lines are assembled from the manual pages the research fetched;
the `where` expressions, the `:log` line and the two listings named in R9 are
remembered. None was run. If the console refuses one as written, that
refusal is itself a recording worth keeping.

| # | Lifts | Where | Commands |
|---|---|---|---|
| R1 | Legacy reads | M and S, link up; at 20 MHz, at 40 MHz with the extension on each side, at 80 MHz if used | `/interface wireless print detail without-paging`, and again with the `proplist=` the profile sends; `/interface wireless monitor wlan1 once`; `/interface wireless registration-table print detail without-paging`; `/interface wireless registration-table print stats without-paging`. Once more on S while it is searching (M disabled). And the first command on a router that has no wireless package, for the refusal's text; and the console's list of the commands `/interface wireless` offers, so that none that takes the card off the air is missing from the disruptive list. |
| R2 | Allowed list, radar marks, power, the effective scan list | M and S | `/interface wireless info allowed-channels wlan1`; `/interface wireless info scan-list wlan1`; `/interface wireless info default-scan-list wlan1`; `/interface wireless info country-info <the configured country>`; `/interface wireless info hw-info wlan1`. If the positional name is refused, the same with `interface=wlan1`, and keep the refusal. |
| R3 | The export line and the configuration check | M and S | `/export terse` before and after each change of R4. |
| R4 | The change lines, follow time, refusals | bench pair, S reached through the link | On S: `/interface wireless set wlan1 scan-list=<present value>,<new>`; then on both ends `monitor wlan1 once` and M's registration table: did the peer's uptime restart? On M: `/interface wireless set wlan1 frequency=<new>`; then M's registration table read back to back, with the time each answer arrived, until the peer is back: when the table emptied (the reach test leans on it emptying at once) and the follow time. Once more with M managed only across the link (Nocturne reaching it through S): the same reads back to back, to record that the second one stalls until the peer is back, which is the reach test's whole claim. Repeat with S's list at two entries and at `default`. On M: `/interface wireless set wlan1 frequency=<present value>`: does it drop the link? On M: `/interface wireless set wlan1 frequency=<a value the country does not allow>`: the console's answer, then `monitor wlan1 once`; set it back. With a radar channel as the target: `monitor wlan1 once` every 5 s until the peer is back (the wait). For each: did the session to S survive, and for how long did it stall. With M in `bridge` mode and S registered: a second station tries to register; and the same on `ap-bridge` with `max-station-count=1`: is it refused, and what does M's registration table show. |
| R5 | The dead-man | M and S; under the account Nocturne will use, with exactly its rights | `/system clock print`; `/system scheduler add name=nocturne-guard-test start-time=startup interval=1m on-event="/system scheduler remove [find name=nocturne-guard-test]; :log info \"nocturne-guard-test ran\""`; at once `/system scheduler print detail without-paging where name="nocturne-guard-test"`; the same listing every 10 s until the entry is gone, with the time; `/log print without-paging where message~"nocturne-guard-test"`. The same with the real line `/interface wireless set wlan1 frequency=<present value>` between the removal and the mark. `/system scheduler remove [find name=does-not-exist]`: the answer. On a radio whose name contains a space: what quoting the console accepts. Once: arm with 5 minutes, reboot the device, and record when it fires after boot. Once: arm with 5 minutes, set the clock an hour forward, and record when it fires. Once, the field case, which is both at once: a board with no battery clock and NTP configured, armed with 5 minutes and power-cycled; record the clock at boot, when it was corrected, and when the entry ran. Once: let the test entry run, reboot, and read the log again: is its mark still there? On each major version, `/system clock print` and the entry's `next-run` as printed: the date formats. |
| R6 | The survey | M (not on a radar channel), then S through the link | `/interface wireless scan wlan1 rounds=1`: the whole output and the times it started and ended; from the other end the peer's uptime before and after; whether the session to S came back by itself. `/interface wireless scan wlan1 rounds=1 save-file=nocturne-scan`, then `/file print detail where name~"nocturne-scan"` and the file's contents. On an 802.11 link: `/interface wireless scan wlan1 background=yes`, and whether the peer's uptime restarted. `/interface wireless frequency-monitor wlan1`: what it prints, whether it ends by itself, and the peer's uptime. On a master on a radar channel: `scan wlan1 rounds=1`, then `monitor wlan1 once` every 5 s until the peer is back. |
| R7 | `wifi` reads (only if such radios are in use) | access point and station | `/interface wifi print detail without-paging`; `/interface wifi monitor wifi1 once`; `/interface wifi registration-table print detail without-paging`; `/interface wifi radio print detail without-paging`; `/interface wifi radio reg-info country=<country> 0`. |
| R8 | `w60g` reads (only if in use) | bridge and station | `/interface w60g print detail without-paging`; `/interface w60g monitor wlan60-1 once`; `/interface w60g station print detail without-paging`. |
| R9 | Reach | S, reached through the link, in the bridging arrangement actually used | `/interface print detail without-paging`; `/interface bridge port print detail without-paging`; `/interface bridge host print detail without-paging`; `/ip arp print detail without-paging`; `/ip route print detail without-paging`; the listing of dynamic per-station interfaces if the link uses them (`/interface wireless wds print detail without-paging`); the listing of active users, which says what address the device sees the session come from (`/user active print detail without-paging`); and where management is on a VLAN, `/interface vlan print detail without-paging`, for the interface it rides on. |

What each changes: R1 takes the banner off legacy reads for the role
recorded. R2 makes the device's list the source of candidates and of radar
marks, and makes a retune possible at all. R3 makes the
configuration check of a radio change a real check. R4 and R5 need
`changes.allow_unverified_writes` on, for as short a time as the bench work
takes: after them, on that RouterOS major version, a person-approved retune
on a real link is something this software has done before. R6 lets a survey state its outage, and decides whether RouterOS gets
a survey that measures occupancy. Nothing in this list makes the scoring
model true.

## Found on the way, to be fixed whether or not this feature ships

Each of these is in today's change engine and has nothing to do with radios.
The radio work does not depend on any of them being fixed first and does not
make any of them worse; how is said in each.

1. **Configuration lines in change documents, events and job logs.** When
   this file was first written only the session transcript was redacted:
   plan diffs, the details of checks and steps, `changes.progress` events
   and job-log lines carried configuration lines whole, against the rule
   that a secret appears in no document, event, log line or job output. That
   is being fixed by other work, and the tree as read for this revision
   holds it in the working copy: `crates/nocturne-changes/src/redact.rs`
   says "nothing a change is made of leaves this crate except through this
   module", the lines are answered through redacted views (`redact::change`,
   `redact::plan`), everything said about them goes through a `Hider`, and
   `tests/secrets.rs` searches for leaks. Whoever builds the radio pieces
   reads that module as it is on main the day they start; this item is
   closed or open by what they find, not by this paragraph. *Radios:* the
   design does not depend on it: a radio plan has no text diff, and its
   checks, steps and errors name properties and never quote a line. Where
   the module exists, radio text leaves through its `Hider` and its views
   and through nothing else. It matches on `Intent` with no wildcard arm, as
   `rpc.rs::refuse_placeholders` does, so both gain the radio intents in the
   commit that adds them. The schedule does depend on it: the build plan
   gives `redact.rs` to the one owner of the change engine, who starts from
   main once that work has landed rather than from a copy of it.
2. **The assistant can start any change a person has approved.**
   `changes.assistant_apply` begins `if change.state ==
   ChangeState::Approved { return start_apply(&app, change, "assistant") }`,
   before `autonomy::decide` is called. A person who approves at 16:00
   meaning to apply at 02:00 has handed the moment to the model, and a model
   reads text that strangers wrote. The rule is that the model proposes and
   the pipeline decides. Work in flight on the change engine (finding r9)
   makes the handler judge an approved change by the autonomy decision too.
   *Radios:* the refusal of radio changes is the first statement of that
   handler, ahead of the shortcut and of the decision, so it holds before and
   after that work lands.
3. **`changes.rollback` sends lines nobody approved.** It starts
   `rollback::run` at once: the lines are derived at that moment from the
   device's present configuration back to the stored one, with no plan to
   read, no approval and no typed confirmation, and deliberately outside the
   maturity gate. The rule is that a human approves that exact plan. The
   rollback after a failed check (`apply.rs::roll_back`) sends lines derived
   the same way, by `hier::remediate`, which nobody has read either; for
   RouterOS they begin with every stored line the device no longer has.
   Work in flight (finding r9) makes the on-demand rollback a planned undo
   that a person approves. *Radios:* for a radio change the method creates a
   new planned change or refuses, and sends nothing. And every rollback the
   engine derives (today's on-demand one, the planned undo, and the one after
   a failed check) leaves a radio's own line and a revert's entry alone
   (`rollback.rs`): without that rule the radio work would make this item
   worse, because the rollback of any change older than a move would put the
   radio back where it was then.
4. **Who approved is whatever the caller says.** `changes.approve` takes `by`
   from its parameters, so `policy:low_risk` can be written by hand.
   *Radios:* nothing in this build depends on it; there is no standing
   approval to impersonate.
5. **A script or a scheduler entry in a `Lines` change is opaque to every
   protection rule.** `/system script add source="... country=..."` meets no
   rule; the profile's `dangerous` pattern for running a script is anchored
   at the start of a line. *Radios:* `radio_config` and `radio_regulatory`
   cannot see inside one either, and say so; typed radio changes never carry
   caller text.
6. **A check that could not run is recorded as passed** (`ok: true` with a
   sentence in its detail). *Radios:* radio checks use the new `not_run`.
7. **A RouterOS `Lines` change is applied and then rolled back** with the
   default options, because its result cannot be predicted; no test in the
   change engine uses a RouterOS device. And `verify.pings` cannot pass on
   RouterOS (no count, IOS wording).
8. The simulator has no `reload in`, so the IOS dead-man is not exercised in
   a lab.
9. **`changes.allow_unverified_writes` is one switch for every device type.**
   Turning it on for a bench opens every unverified write in the
   installation, and nothing on a plan says the gate was opened by it.
   *Radios:* every radio plan that needs the setting says so on its own line
   and carries a critical hit; the setting is left as it is.

## Tests

Each is named for the behaviour and fails if the behaviour is removed. A test
that would prove less than its name says is replaced or called what it is.
A test marked † runs against devices that are **not** lab devices
(`testing::radio_world` with `lab: false`, speaking the built-in `verified`
profile `test_sim_radio` with exactly the labels the test lists), because the
rule it tests is one a lab link relaxes or one that turns on a label. Each
crate's list says which delivery of the build plan makes each test pass;
nothing is assigned to a delivery that cannot yet run it.

**`nocturne-rf`** (no devices, no clock). The module's header says what
these prove: that the Rust equals the calculator, not that the model fits a
link.

* *A1*: `lin_and_db_give_the_same_bits_on_every_platform` (a table of inputs
  with the expected bit patterns); `a_threshold_met_exactly_is_met`;
  `a_span_that_ends_on_a_row_does_not_include_it`;
  `a_radar_row_anywhere_under_the_span_refuses_the_channel`;
  `several_surveys_of_one_end_combine_by_the_worst_bin`;
  `a_busy_bin_with_no_level_is_an_assumption`;
  `the_links_own_peer_is_not_an_interferer` (N1, both ways);
  `the_way_home_through_a_radar_channel_waits_for_it`;
  `a_longer_scan_list_forgets_the_measured_follow_time`;
  `a_learned_time_never_shortens_a_deadline`;
  `a_follow_time_measured_on_the_way_home_sets_the_next_deadline`;
  `a_deadline_over_the_limit_is_refused_with_the_figure_that_made_it`. The
  table of `lin` and `dB` cannot fail on the machine it was written on, and
  does not fail if somebody puts `powf` back, so the crate also carries
  `clippy.toml` with `disallowed-methods` for `f64::log10`, `ln`, `exp`,
  `powf` and `powi`: CI runs clippy with `-D warnings`, and there is no
  `clippy.toml` at the root to shadow it.
* *A2*: the worked examples T1, T1a to T1c, T2, T4 to T7, T9a, T9b, T12, N1
  to N7, `Q` and `A` to three decimals;
  `a_scan_that_decoded_nothing_does_not_rule_a_channel_in`;
  `a_channel_never_surveyed_is_unknown_and_is_never_scored_as_clean`;
  `a_link_that_has_what_it_needs_stays` (N3);
  `a_prediction_far_from_what_the_link_achieves_moves_nothing_either_way`;
  `a_channel_the_link_would_drop_on_when_the_neighbour_transmits_is_not_a_target`;
  `a_radar_channel_is_priced_and_never_chosen`;
  `a_channel_with_no_radar_mark_is_a_radar_channel`;
  `another_width_is_shown_and_never_chosen`;
  `a_blind_end_is_never_better_on_a_candidate_than_it_is_now`;
  `a_wider_candidate_costs_a_blind_end_its_thermal_noise_and_a_narrower_one_earns_it_nothing`;
  `a_higher_power_limit_elsewhere_earns_nothing`;
  `a_channel_with_no_power_figure_is_not_a_target`;
  `an_unseen_slaves_power_is_an_assumption_and_a_lower_limit_on_the_masters_list_is_not_a_target`
  (T9b and its variant);
  `ties_keep_the_current_channel`;
  `four_evaluations_of_one_survey_are_not_persistence`;
  `the_airtime_is_split_by_the_traffic`;
  `a_candidate_measured_only_outside_the_busy_hours_is_a_trial`;
  `the_power_term_is_two_rows_of_the_devices_own_list` (N7);
  `what_a_link_achieves_is_a_mean_within_the_hour` (N6);
  `ties_among_undecoded_channels_do_not_all_go_to_the_lowest_frequency`;
  `a_master_only_candidate_is_a_trial_and_not_a_non_target`. Property: the
  ranking does not depend on the order candidates are given in.

**`nocturne-drivers`**:
* *B1a*: `disruptive_sorts_between_change_and_dangerous`;
  `a_command_on_both_lists_is_disruptive_not_read_only`;
  `a_collector_refuses_a_disruptive_command`;
  `an_error_rate_is_not_a_quality_figure`.
* *B1b*: `an_absent_answer_must_name_the_stacks_menu_word_and_sit_on_the_listing_step`;
  `a_router_with_no_radio_package_reads_as_empty_not_failed`;
  `a_timeout_on_an_optional_stack_is_a_failure_not_an_empty_table`;
  `a_step_per_row_runs_once_for_each_radio_of_its_stack_and_no_other`;
  `a_radio_whose_name_is_not_a_plain_word_is_listed_and_not_monitored`;
  `a_rendered_command_is_classified_not_its_template`;
  `a_refused_proplist_fails_the_read_and_does_not_fall_back`.
* *B2a*: `a_field_a_refusal_tests_that_was_not_read_refuses`;
  `a_slave_has_no_layout_and_is_never_choosing_its_channel`;
  `the_guards_four_reads_are_read_only`;
  `width_and_offset_come_from_the_configured_width`;
  `a_live_channel_string_that_disagrees_is_a_partial_problem`;
  `an_opaque_scan_list_entry_covers_nothing`;
  `a_template_value_with_no_declared_pattern_is_refused_at_load`;
  `an_operation_read_that_maps_a_field_its_row_lacks_is_refused_at_load`;
  `a_survey_that_measures_what_the_drivers_have_no_rows_for_is_refused_at_load`.
* *B2b*: `a_date_in_either_declared_format_is_the_same_instant`;
  `a_date_in_no_declared_format_is_not_an_instant`;
  `a_default_name_with_a_dot_in_it_finds_its_own_line_and_no_other`;
  `an_operation_read_parses_to_its_typed_rows` (each of `allowed`,
  `effective_scan_list`, a survey, `check`, `leftover`, `clock`, `mark_read`,
  on synthetic text); `a_mark_read_that_answers_an_error_is_not_a_mark`;
  `a_leftover_listing_names_every_entry_it_holds`.
* *B3*: `a_session_with_no_origin_is_synthetic`;
  `a_capture_that_holds_a_secret_is_refused`; `the_wep_key_names_are_redacted`;
  `a_recording_of_one_stack_does_not_lift_the_label_of_another`;
  `a_recording_of_a_master_does_not_lift_the_slaves_label`;
  `a_recorded_capture_for_a_role_still_listed_fails_the_harness`;
  `an_operation_capture_lifts_its_label_and_a_synthetic_one_does_not`;
  `a_recorded_operation_capture_for_a_role_still_listed_fails_the_harness`;
  `a_synthetic_session_does_not_lift_an_operations_label`.
* *E1, E2* (the RouterOS profile's own tests): the captures, each labelled
  documented or synthetic with its stack and role; the operation captures,
  synthetic, with their rows; `a_radio_that_reports_no_country_set_is_refused`;
  `every_survey_command_classes_disruptive_and_nothing_read_only_does`. The
  scripted sessions for a change line and for arming, checking and disarming
  the revert use the four session actions, which render through
  `radio::render`: a session file holds the values and the line it expects,
  and the harness renders. They are `origin: synthetic`.

**`nocturne-app`** (*D2*): `a_held_device_refuses_other_sessions_at_once`;
`the_address_a_device_sees_is_unknown_behind_a_jump_host`;
`the_unverified_mark_of_a_radio_row_follows_its_stack_and_role`;
`a_lab_device_is_timed_by_its_labs_clock`;
`an_in_band_session_stalls_with_its_path_and_is_closed_at_the_timeout`.
(*D3* is tested by the engine's tests, which are its only users.) That the
testing-only connector hook is in no shipped binary is not a test, because a
test runs in a build that has it: it is a CI step of W1b, which runs `cargo
tree -e features -i nocturne-app` over the binaries' normal edges and fails if
the `testing` feature appears.

**`nocturne-changes`** (against `testing::radio_world`, paused clock; every
RPC method through `App::rpc`).

* *F0*: none of its own: the review rule below.
* *F1a*: `a_radio_change_takes_no_caller_options`;
  `caller_options_are_refused_on_update_as_on_create`;
  `a_radio_change_writes_one_device`;
  `a_read_that_has_met_no_device_makes_the_plan_unverified` †;
  `an_unverified_operation_makes_the_plan_unverified_and_the_gate_refuses_it` †;
  `a_plan_the_setting_opened_says_so_and_carries_the_hit` †;
  `the_shipped_lab_profile_on_a_lab_device_is_verified_and_a_users_override_is_not`;
  `a_label_lowers_a_lab_plan_and_nothing_raises_it`.
* *F1b*: `a_slave_cannot_move_a_link`;
  `a_radio_that_chooses_its_own_channel_is_not_planned_for`;
  `a_peer_that_is_not_the_one_named_refuses_the_plan`;
  `a_witness_in_another_world_is_refused`;
  `a_radar_row_under_the_other_half_of_the_channel_refuses_the_target`;
  `a_lower_power_or_indoor_only_target_is_refused_by_the_planner_itself`;
  `an_unseen_slave_is_taken_as_not_bounding_its_power` †;
  `a_move_home_is_exempt_from_the_power_and_indoor_rules`;
  `a_forward_move_is_not_planned_into_a_link_that_is_down`;
  `only_a_return_may_be_planned_with_the_link_down`;
  `a_radar_frequency_may_be_added_to_a_slaves_list`;
  `a_survey_that_could_rank_nothing_is_not_planned`;
  `a_master_on_a_radar_channel_is_not_surveyed`;
  `a_radar_target_is_refused_for_a_person_too`;
  `a_target_with_no_radar_mark_is_refused`;
  `an_unverified_channel_list_is_treated_as_unreadable_and_plans_nothing` †;
  `a_master_that_moved_itself_is_not_retuned`;
  `a_retune_waits_until_the_slaves_list_holds_old_and_new_as_numbers`;
  `preparing_a_default_list_writes_the_present_frequency_beside_the_new_one`;
  `an_addition_the_slaves_own_list_does_not_allow_is_refused`;
  `a_name_with_a_space_and_a_property_in_it_is_refused_not_sent`;
  `a_test_mode_is_a_refusal_not_a_finding`;
  `a_planned_radio_change_lapses`.
* *F2*, reach as a pure function over collected rows, one test per row of
  "The common cases, decided", each flat and routed. The routed fixtures give
  every interface of a router its own MAC, as a RouterOS router has, so that
  a walk that took a router's MAC from the interface it was reached by, and
  not from the one it sends toward Nocturne by, fails the routed form of
  `a_relay_master_is_behind_the_first_link_flat_and_routed`:
  `both_ends_on_a_cable_are_both_direct`;
  `the_usual_link_has_a_direct_master_and_a_slave_through_its_link`;
  `a_relay_master_is_behind_the_first_link_flat_and_routed`;
  `a_router_is_looked_for_by_the_mac_of_the_interface_it_sends_toward_nocturne_by`;
  `a_router_not_in_the_inventory_names_itself`;
  `a_way_there_that_is_not_the_way_back_is_unknown`;
  `a_link_that_says_nothing_never_makes_a_device_unknown`;
  `an_address_aged_out_of_a_bridge_decides_nothing`;
  `the_longest_route_decides_and_two_of_that_length_are_unknown` (at `D`
  and at a router of the walk);
  `an_inactive_route_is_not_a_way_back`;
  `a_management_vlan_is_judged_by_the_port_it_rides_on`;
  `a_mac_learned_on_a_per_station_port_is_not_a_cable`;
  `a_mac_on_a_port_the_walk_cannot_place_leaves_the_stretch_unknown` (and a
  crossing that another radio shows still decides);
  `a_devices_own_link_crossed_at_a_later_stretch_is_through_its_link`;
  `a_walk_that_loops_is_unknown`;
  `a_lab_does_not_change_the_reach_of_a_real_device` and
  `a_real_device_does_not_change_the_reach_of_a_lab`;
  `what_was_not_seen_is_listed_with_a_direct_answer`. Through `App::rpc`:
  `a_master_reached_through_the_link_is_not_retuned`;
  `a_master_behind_another_link_is_not_retuned_or_surveyed`;
  `a_survey_is_refused_through_the_link`;
  `a_master_whose_last_reach_test_failed_is_not_surveyed`;
  `the_plan_reads_the_bridges_after_it_has_talked_to_the_device`;
  `changes_reach_answers_from_stored_reads_and_opens_no_session`.
* *F3*: one test per protected kind;
  `lines_that_touch_a_radio_menu_or_a_never_property_are_protected_hits`;
  `a_sector_with_one_client_online_carries_the_serves_many_hit`;
  `a_survey_command_typed_into_a_lines_change_is_refused`;
  `no_change_of_any_kind_is_planned_on_a_device_that_holds_a_revert`;
  `no_plan_may_recreate_a_dead_man_of_nocturnes`;
  `a_rollback_line_that_is_a_dead_man_refuses_the_plan`;
  `a_derived_rollback_never_recreates_a_dead_man`;
  `rolling_back_an_older_change_never_touches_a_radio_or_a_dead_man` (a
  device whose stored version holds an older radio line and a leftover's
  removal: the derived lines leave both out, say so, and end
  `rollback_failed`, and nothing radio is sent);
  `a_lines_change_on_routeros_is_never_armed_with_the_revert_guard`.
* *F4a*, the guard's pieces against a fake device, the ledger and the lock:
  `a_dead_man_that_ran_at_once_stops_the_change_before_anything_is_sent`;
  `a_dead_man_due_early_or_not_at_all_is_not_armed`;
  `a_disarm_that_removed_nothing_is_not_a_disarm`;
  `whether_a_guard_ran_is_its_mark_and_not_its_counter`;
  `a_lost_mark_leaves_the_radios_values_to_decide`;
  `a_stepped_device_clock_is_seen_at_the_next_read_of_the_guard`;
  `a_leftover_dead_man_on_either_device_refuses_the_plan`;
  `an_open_ledger_entry_refuses_every_plan_that_names_the_device_even_when_it_is_dark`;
  `a_deleted_slave_with_an_open_ledger_entry_still_blocks_its_masters_retune`;
  `a_ledger_device_read_again_gets_its_closing_read`;
  `a_paused_lab_does_not_hold_the_lock_of_real_links`.
* *F4b*, the first radio changes that run: the prelude, the drill, proofs,
  the removal of a leftover, and the three refusals that come with them:
  `two_radio_changes_do_not_run_at_once`;
  `nothing_else_gets_a_session_while_a_radio_change_holds_the_devices`;
  `a_plan_whose_live_facts_changed_since_approval_is_not_applied`;
  `a_lab_clock_that_stands_still_ends_the_change`;
  `the_first_drill_on_a_device_can_be_planned`;
  `the_drills_second_stage_is_armed_only_after_its_first_passed`;
  `the_drill_fails_a_guard_that_fires_late`;
  `the_drill_fails_a_guard_whose_lines_after_the_removal_do_not_run`;
  `the_drill_fails_a_guard_that_keeps_its_entry`;
  `the_drill_records_whether_a_same_value_set_restarts_the_radio`;
  `a_drill_whose_radio_does_not_come_back_ends_within_its_bound`;
  `a_master_on_a_radar_channel_gets_only_the_first_stage_and_borrows_the_second`;
  `a_borrowed_stage_must_be_of_the_same_role_software_account_and_line`;
  `a_reboot_with_the_guard_armed_reverts_after_boot`;
  `a_proof_dies_with_the_software_the_lines_or_the_account`;
  `a_proof_made_directly_does_not_cover_a_device_now_behind_its_link`;
  `a_leftover_revert_is_removed_by_its_own_change_and_by_no_other`;
  `a_removal_races_the_revert_and_reports_which_happened`;
  `a_removal_that_leaves_the_entry_is_failed_and_says_so`;
  `the_assistant_cannot_start_a_radio_change_that_a_person_approved`;
  `rolling_back_any_other_radio_change_is_refused_with_its_reason`;
  `a_change_a_proof_or_a_ledger_entry_refers_to_cannot_be_deleted`.
* *F5*, preparing a slave and the waiting verification:
  `a_slaves_session_lost_while_its_list_changes_is_judged_by_the_read_back`;
  `a_prepare_whose_list_does_not_read_back_as_planned_is_put_back`;
  `a_slave_that_goes_dark_with_its_guard_armed_ends_the_change_and_stays_on_the_ledger`;
  `a_failed_radio_verification_names_properties_and_never_quotes_the_export`
  (the lab radio's `key` is a marker string searched for in every document,
  event, job log and audit entry of the change);
  `a_check_that_could_not_run_is_not_shown_as_passed`;
  `a_clock_stepped_during_a_prepare_puts_the_list_back`.
* *F6a*, moving a master and the table of `rollback.rs` (T14 is this list):
  `the_master_is_never_touched_while_a_slaves_revert_may_still_fire`;
  `a_slave_that_never_follows_brings_the_master_home` (both endings);
  `a_master_found_in_a_radar_wait_is_reverted_and_never_disarmed_with_the_link_down`;
  `a_slave_that_joined_another_master_has_not_followed`;
  `a_far_end_that_answers_inside_its_own_deadline_is_not_reverted`;
  `a_master_that_did_not_answer_while_its_link_was_down_is_never_confirmed`
  (the knob that runs the master's session over its own link);
  `a_stalled_not_registered_answer_does_not_prove_a_direct_path`;
  `a_slave_back_within_two_reads_is_put_back_and_blames_nothing`;
  `a_master_that_restarted_with_the_guard_armed_is_not_confirmed`;
  `a_clock_stepped_during_a_retune_is_not_confirmed`;
  `a_revert_that_fired_a_moment_before_the_confirmation_is_not_recorded_as_applied`;
  `the_witness_is_read_for_a_leftover_before_the_master_is_confirmed`;
  `a_guard_that_fires_on_the_way_home_restarts_the_wait_and_is_not_a_failed_disarm`;
  `the_way_home_through_a_radar_channel_is_not_called_lost_before_its_wait`;
  `did_not_come_back_is_never_said_before_the_worst_case_has_passed`;
  `a_lost_session_is_waited_for_until_the_way_home_could_have_ended`;
  `the_alarm_says_what_each_end_last_reported`;
  `only_a_row_about_the_channel_quarantines_it`;
  `a_failed_return_quarantines_nothing`;
  `a_follow_missed_on_an_estimate_quarantines_nothing_and_the_way_home_measures_one`;
  `a_rainy_night_fails_neither_the_move_nor_the_way_home`;
  `no_lost_session_is_rolled_back_on_sight_in_a_radio_change`;
  `every_wait_runs_on_the_devices_clock` (the same retune with the world at
  twice and at half the speed of Nocturne's clock ends the same way).
* *F6b*, the rest of a retune's life:
  `a_return_with_the_link_down_is_not_stopped_by_anything_that_needs_the_link`;
  `a_return_with_the_link_down_that_finds_no_peer_puts_the_master_back`;
  `cancel_after_the_set_means_go_home`;
  `shutdown_mid_change_sends_the_old_frequency_and_leaves_the_rest_to_the_guard`;
  `a_restart_mid_change_takes_the_lock_and_reads_the_devices_before_it_says_anything`;
  `recovery_at_start_up_leaves_radio_changes_to_their_own_rule`;
  `an_outside_move_is_recorded_when_it_is_seen`.
* *F7*, surveys and the methods:
  `a_survey_takes_the_link_down_and_verifies_the_same_channel_after`;
  `a_scan_that_never_ends_is_closed_and_reported`;
  `a_survey_leaves_out_the_links_own_peer_and_says_so`;
  `rolling_back_a_retune_plans_a_change_and_sends_nothing`;
  `the_radio_fields_leave_through_the_redacted_views`.

Not a test, a review rule: the commit that splits `apply_device` into phases
changes no behaviour, and its diff touches **no test file that exists at
that commit** (today `applying`, `approval`, `orchestration`, `planning`,
`property` and `secrets` under `crates/nocturne-changes/tests/`, and
`tests/common/**`). A test cannot assert that other tests pass.

**`nocturne-monitor`**:
* *G1*: `two_reads_make_a_rate_and_a_re_registration_throws_the_interval_away`;
  `a_failed_radio_read_is_a_gap`;
  `retries_are_computed_only_where_the_profile_gives_them`;
  `a_lab_devices_radio_is_read_on_its_labs_clock` (at twice the speed, twice
  the reads in real time and the same stamps in the lab's).
* *G2*: `a_device_held_by_a_radio_change_is_a_gap_that_says_so`;
  `a_router_with_no_radio_is_not_asked_again_until_it_changes`;
  `the_quality_rule_never_reads_an_error_rate`; each of the three rules; the
  method list and the count of built-in rules in `src/tests/rpc.rs`, updated
  with them.

**`nocturne-radio`** (fakes standing in for `changes.*`, `monitor.radio`
and `metrics.query`; paused clock):
* *I1*: the three pairings and the sector that is not paired;
  `a_slave_that_names_its_master_confirms_the_pair`;
  `a_master_seen_with_two_peers_is_held`;
  `a_link_with_ends_in_two_worlds_is_refused`;
  `the_feature_calls_exactly_its_listed_methods` (the list in `calls.rs`,
  compared with the fifteen names under "Links");
  `a_method_another_feature_lacks_is_reported_not_assumed`; every method of
  I1 through `App::rpc`.
* *I2a*: T3 and its variant; T8; T11; T13; N4;
  `an_evaluation_says_what_it_was_computed_from`;
  `a_recommendation_from_one_end_says_so`;
  `every_assumption_is_named_and_makes_a_trial`;
  `a_failed_evaluation_is_the_latest`;
  `a_stale_evaluation_is_recomputed_before_it_is_proposed`;
  `an_evaluation_is_made_when_a_survey_is_stored`;
  `a_link_on_a_radar_channel_whose_slave_does_not_cover_its_master_is_reported`.
* *I2b*: `probation_leaves_out_a_faded_hour`;
  `probation_ends_early_on_a_plainly_worse_first_evening`;
  `a_neighbour_that_got_worse_counts_against_the_move`;
  `a_device_that_loses_reads_in_bursts_counts_against_the_move`;
  `a_worse_move_gets_its_move_back_planned_and_nothing_applied`;
  `a_link_down_on_its_new_channel_gets_a_critical_alert_and_a_planned_return`;
  `a_link_that_left_a_radar_channel_is_not_sent_home`;
  `two_misses_make_every_later_proposal_a_trial`;
  `a_lab_links_probation_ends_the_same_at_any_speed`;
  `the_task_applies_nothing_and_says_what_it_planned`;
  `a_ledger_device_answering_again_is_checked`; every method of I2 through
  `App::rpc`.

**`nocturne-netsim`**: the radio fuzz test never panics;
`one_advance_and_a_thousand_give_the_same_instants`;
`the_same_seed_replays_the_same_scan`;
`a_link_forms_only_where_the_non_root_looks`;
`a_longer_list_takes_longer_to_follow`; `a_scan_lists_its_own_peer`;
`a_scan_does_not_see_what_it_cannot_decode`;
`a_schedule_fires_with_no_console_and_again_after_a_restart`;
`radar_sends_the_root_to_its_own_list`;
`the_key_is_in_the_running_config_and_not_in_show_radio`;
`a_schedule_body_with_a_quoted_string_runs_it`;
`the_clock_and_the_next_run_are_printed_in_two_forms_neither_of_them_iso`;
`a_clock_step_moves_the_printed_time_and_not_the_simulated_one`;
`a_restart_resets_the_run_count_and_retimes_the_entry`;
`a_channel_the_list_does_not_mark_can_still_make_the_root_wait`;
`the_shipped_lab_profile_loads_and_reads_the_simulators_own_output`.

**`nocturne-lab`**: `an_in_band_session_drops_with_the_link_and_returns_with_it`;
`the_lab_radio_profile_reads_every_kind_from_the_simulators_own_output`;
`the_simulators_tables_are_the_engines_defaults`;
`a_built_in_lab_and_the_engine_pick_the_same_modulations`;
`t1s_source_makes_the_lab_link_achieve_what_the_engine_predicts` (0.58,
within 0.03); `a_lab_nodes_site_is_its_mast`.

**`nocturne-assistant`**: the boundary test lists the methods that must be
absent; `a_proposed_retune_waits_for_a_person`;
`survey_text_reaches_the_model_marked_as_untrusted`;
`names_a_radio_reports_reach_the_model_marked_as_untrusted`.

**The suite**: the schedules' allow-list (`nocturne_automation::schedules::ALLOWED`,
a public constant) holds none of the method names listed under "Method names
the boundary tests must find absent"; none of the method names that list says
are not registered is registered.

**End to end, in the suite and in the interface's run**, on "Two towers".
*On the tower link:* pair and read; test the revert on the master and on the
far slave; prepare the slave through the link (its session lost and judged
by the read-back); survey the near end with `scan` (the outage stated, the
ranking saying "heard" and "nothing decoded"); a retune by approval, the far
slave's session lost and regained; verify. Then: the slave whose list lacks
the channel (no plan); the slave that never follows (home); a
person-approved move to a channel with a `penalty`, which probation finds
worse: the move back is planned and linked from the alert, the run approves
it, and the link is home. *On the relay link:* its root reads `behind_link`
via the tower link, and a retune or a survey of it is refused with that
reason; its non-root reads `through_link`. The run is repeated with the lab
at eight times speed and must end the same, which it can because every clock
a lab link is measured on is the lab's.

## Build plan

Every file has exactly one owner; nobody else edits it, except for the few
lines listed under "What is still one piece's line in another's file" below,
each of which names who adds it and why. A **delivery** is
sized for one agent to build in one sitting and another to review, by this
repository's own measure: the monitor's QoS slice (`qos.rs`, its tests and the
files they touched) is about the most one delivery holds. Where a crate's
pieces cannot compile apart (adding an enum variant breaks every exhaustive
match in the crate), the crate has **one owner who makes several deliveries**,
in order, each reviewed by itself. The orchestrator owns the workspace
`Cargo.toml`, `Cargo.lock`, `crates/nocturne-suite/**`, `docs/STATUS.md`,
`README.md` and `.github/**`. `Cargo.lock` is never merged by hand: a delivery
that adds a dependency says so when it is handed over, and the lock is
regenerated when it is integrated. No delivery adds a dependency outside the
workspace's existing set except where its row says so.

**When a delivery starts.** When every delivery in its "Needs" column has
landed on main, and, where its "Starts when" column names work by others on
the same files, when that work is on main too: an owner starts from main,
never from a copy of somebody's working tree. As this is written, that work
is: the SNMP health fix (the drivers' `snmp` blocks and `profile.rs`, every
device profile, the monitor's SNMP reading); the monitor's QoS additions; the
change engine's redaction review and finding r9 (the assistant past the
autonomy gate; `changes.rollback` as a planned undo, with its interface); the
transport's bulk-traffic fix; the assistant's tool work; the interface's QoS
views (`monitor.ts`, `core.ts`).

**When a delivery is done.** When its row, the tests listed for it under
"Tests", and every line of "Known and accepted for the first build" (at the
end of this file) that names it are done, and its review has checked each of
them. Where such a line changes what a section above says, the line is what
is built.

| # | Owner | Owns | What | Needs | Starts when |
|---|---|---|---|---|---|
| W0 | orchestrator | workspace `Cargo.toml`, `Cargo.lock` | The two `[workspace.dependencies]` lines, and the two new crates created **empty and compiling** (`Cargo.toml`, an empty `src/lib.rs`), so that no manifest ever names a crate that is not there. The two directories pass to A and to I at that commit. | | now |
| A1 | engine | `crates/nocturne-rf/**` | `Channel`, spans and "which rows a span covers" (the planner's function); the survey types, bins with their states, the own peer taken out, worst per bin; the timers' arithmetic and the deadline limit; `lin` and `dB`; `clippy.toml`; their tests | W0 | now |
| A2 | engine | the same | Need, noise, SNR, rate, `Q`, `harm`, `A`; the three kinds of candidate; rows 1 to 7 and the ties; the worked examples | A1 | |
| B1a | drivers | `crates/nocturne-drivers/src/{lib,model,collect,policy,profile,parse,expr}.rs`, `crates/nocturne-drivers/src/radio.rs` (new), `crates/nocturne-drivers/tests/harness/**`, `crates/nocturne-drivers/tests/harness_rules.rs`, `docs/profiles.md`, `docs/dev/spec-profiles.md`, `docs/dev/features/README.md` | The two kinds and their row types; `Interface.parent`, `Route.active`, `tx_retries`; `CommandClass::Disruptive` with its order, its precedence and its sentence; the kinds line of `docs/dev/features/README.md` | | the SNMP health fix is on main |
| B1b | drivers | the same | The collector's four additions: `absent` on a listing step, `per: row` with `stack:`, values checked and the rendered command classified, a refused `proplist=` a failure | B1a | |
| B2a | drivers | the same | The `radio:` section's schema (stacks, roles, layout, scan list, values, `refuse`, `requires`, `config_fields`, operations and their reads with `map` and row types, labels by role, `reach`, the guard block) and the loader's rules; `Profile::radio_unverified`; the documentation | B1b | |
| B2b | drivers | the same | `radio::render` and the checks on a rendered line; the one date parser; `radio::read_operation` to `AllowedRow`, `ScanEntry`, `Heard`, `GuardEntry`, `DeviceInstant` and the mark | B2a | |
| B3 | drivers | the same | The harness: labels per stack and role; operation captures (`operation:` in `expect.yml`) and what lifts each label; `origin` on sessions (absent means synthetic, so the existing session files are not touched); the four session actions; the harness rules | B2b | |
| C1 | simulator | `crates/nocturne-netsim/**`, `docs/dev/spec-netsim.md` | The radio bridge's settings, export line and reads, as a catalogue model of the existing `NodeKind::AccessPoint` with the existing `CliKind::Ios` (a new variant of either would break `nocturne-lab`, which matches both without a wildcard, until H); the radio medium; association and follow | | now |
| C2 | simulator | the same | `scan`; the RF environment; the air; radar | C1 | |
| C3 | simulator | the same | The schedule with a quoted body, marks, the two time forms, a restart; the node options (`site` among them) and faults (`ClockStep` among them); the exact-instant tests | C2 | |
| C4 | simulator | the same | The profile text `nocturne_sim_radio` and the built-in lab "Two towers", with the test that loads the profile through the drivers (a dev-dependency of the simulator on the drivers; no cycle) against the simulator's own output | C3, B2b | |
| D1 | transport | `crates/nocturne-transport/src/**`, `docs/dev/spec-transport.md` | The socket's local address in `ConnectionInfo`, where the socket is opened | | the transport's bulk-traffic fix is on main |
| D2 | core | `crates/nocturne-app/src/{connect,collect,profiles}.rs` | `hold`, `device_clock`, `seen_from`, `in_band`; the new `SimConnector` method with its default body; the testing-only connector hook; the disruptive refusal's sentence in `connect.rs`; the per-row mark; "is the `radio:` section in effect the shipped one" | B2a, D1 | |
| D3 | core fakes | `crates/nocturne-app/src/testing.rs`, `crates/nocturne-app/Cargo.toml` | `testing::radio_world` over the simulator, its `radio_pair` preset and its knobs; `test_sim_radio` registered as a built-in, `verified`, with labels as knobs | C4, D2 | |
| E1 | RouterOS data | `crates/nocturne-drivers/profiles/mikrotik_routeros.yaml`, `crates/nocturne-drivers/tests/profiles/mikrotik_routeros/**` | Collect steps for the three stacks, `Route.active`; the policy lists (the disruptive commands, the `info` reads); the WEP patterns; the read captures with origin, stack and role | B3 | the SNMP health fix is on main |
| E2 | RouterOS data | the same | The `radio:` section and the guard; the operation captures (synthetic, with their rows); the sessions that render | E1 | |
| F0 | change engine | `crates/nocturne-changes/**` (every file: `src/{lib,model,plan,risk,protect,settings,apply,verify,rollback,rpc,db,redact,compare,autonomy,abbrev}.rs`, the new `src/radio/**`, `Cargo.toml`, `tests/**` with `tests/common/**` and `tests/secrets.rs`), `docs/dev/features/changes.md` | `apply_device` split into phases; no behaviour change; the review rule under "Tests" | | the redaction review and r9 are on main |
| F1a | change engine | the same | The five intents and everything that stops compiling when they are added, in the same commit (the matches in `redact.rs` and in `rpc.rs::refuse_placeholders`, `Check.not_run` with every existing `Check` literal in `verify.rs` and `tests/secrets.rs`); `RadioPlan`, `Change.radio`, `Origin::Radio`; one target and the fixed options; the plan of each intent without its refusals; the maturity rule, the gate's sentence, `radio_unverified_write`. An apply of a radio change is refused, "radio changes are applied by the radio executor", until F4b gives it one. | F0, A1, B2a, D3 | |
| F1b | change engine | the same | The planner's refusals that need only the two devices' reads and the profile (steps 2 and 4 to 11 without reach, ledger or proofs); the lapse; the world rule | F1a | |
| F2 | change engine | the same | Reach: the walk over collected rows, the live reads of a plan, `changes.reach` from stored reads, the record of the latest reach test; the reach refusals of steps 4 and 7 | F1b | |
| F3 | change engine | the same | The protected kinds and the level of a radio plan; `radio_config` and `radio_regulatory` on `Lines`; the three rules that keep a dead-man from being re-created and the rule that keeps every derived rollback off a radio (in `apply.rs` and `rollback.rs`) | F2 | |
| F4a | change engine | the same | The guard's pieces: arm and read back with the time, the clock reference and the step check, `check`, disarm, `leftover`, the mark; the ledger with the radio's MAC, `changes.guards` and `.check`, and the ledger and leftover refusals; the lock per world; raising and resolving the engine's alerts by name | F3 | |
| F4b | change engine | the same | The executor's common prelude; the drill, both stages, its endings and the borrowed second stage; proofs and `changes.proofs`; `RadioGuardRemove`; the apply of a radio change routed to the executor, and with it the three refusals that must stand before any radio change can run: `changes.assistant_apply` refuses a radio intent first thing, `changes.rollback` refuses every radio change (F7 makes a retune's a planned way back), `changes.delete` refuses a change a proof or an open ledger entry refers to | F4a | |
| F5 | change engine | the same | Preparing a slave with its four endings; the waiting verification on two devices with its three-part configuration check; a lost session judged | F4b | |
| F6a | change engine | the same | Moving a master: arm, set, the reach test, the follow, the slave, verify, confirm with uptime and clock; the table of `rollback.rs` with its blame column; follow times | F5 | |
| F6b | change engine | the same | The return with the link down; cancel; shutdown; the restart rule and `recover_interrupted` leaving radio changes to it; the outside-move record | F6a | |
| F7 | change engine | the same | Surveys, planned and run; the methods that remain (`changes.rollback` on a retune as a planned way back, create, update); the redacted views extended to the radio fields | F6b | |
| G1 | monitoring | `crates/nocturne-monitor/**`, `docs/dev/features/monitor.md` | The radio cadence, on the lab's clock for a lab device; the series, `radio.clock_ms` among them; `monitor.radio` and its event; the method list in `src/tests/rpc.rs` | B1a, D2, D3 | the monitor's QoS additions and the SNMP health fix are on main |
| G2 | monitoring | the same | The three rules (and the count of built-in rules in `src/tests/rpc.rs`); the "held" and "no radio" states | G1 | |
| H | lab | `crates/nocturne-lab/**`, `docs/dev/features/lab.md` | In-band management through `connect::in_band`; the lab's clock for the core; registering the radio profile; radio nodes as devices, each with its mast as its site; the tests that pin the profile and the tables to the simulator | A2, B2b, C4, D2 | |
| I1 | feature | `crates/nocturne-radio/**`, `docs/dev/features/radio.md` (new), `docs/ARCHITECTURE.md` | Links and pairing within one world; the list, get, save, delete, refresh and history methods; `calls.rs` with its exact list and its test; the specification that points here | W0, A1, B1a | |
| I2a | feature | the same | Evidence, the evaluation and its labels, findings, the site view | I1, A2 | |
| I2b | feature | the same | The link's states; probation (lost reads, neighbours, faded hours); misses; the propose methods; the move back planned; the alerts the feature owns; the background task and `radio.status` | I2a; the contracts of F and G, against fakes | |
| J | assistant | `crates/nocturne-assistant/src/tools/{radio.rs (new),boundary.rs,mod.rs,data.rs}`, `crates/nocturne-assistant/tests/{boundary.rs,tools.rs,radio.rs (new)}`, `docs/dev/features/assistant.md` | The five tools, each answering through `tests/tools.rs`'s test of what a tool hands a model; the two kinds in `data.rs`'s list; every string a radio reports wrapped by the existing untrusted block of `shape.rs` (called, not edited); the boundary list, edited deliberately, with its test: the names its tools reach move from `NEVER` to `ALLOWED` | this contract; against fakes; it lands after W1a, because the suite offers a tool only when its feature is in the build | the assistant's tool work is on main |
| K0 | interface: radio links | `ui/src/api/types/terminal.ts`, `ui/src/pages/terminal/{paste,snippets}.ts`, `ui/src/pages/terminal/PasteSheet.tsx` | The fourth command class: `disruptive` in `CommandClass` and in the terminal's maps over it (the two `RANK`s, `CLASS_LABEL`), with the sentence a person reads, so that the paste funnel ranks what the backend sends from B1a on | B1a | |
| K1a | interface: radio links | `ui/src/api/types/radio.ts` (new), `ui/src/api/types/{core,monitor}.ts`, `ui/src/pages/monitoring/radio/**` (new), `ui/src/pages/monitoring/{index.tsx,vocab.ts}`, `ui/src/pages/devices/{collected.tsx,device.ts}`, `ui/src/pages/settings/DeviceTypes.tsx`, `ui/tests/radio.spec.ts` (new) | The tab, the list, the link page with its charts (the radio series' words in `vocab.ts`) and the ladder | the contracts of I and G; a mocked backend first | the interface's QoS views are on main |
| K1b | interface: radio links | the same | The three kinds of candidate and every label; the two Data tables (the kinds in `core.ts`, and in the maps over `Kind` in `device.ts` and `DeviceTypes.tsx`, which are exhaustive); the end-to-end test | K1a; W1a, for the end-to-end test, which needs a server with the radio feature | |
| K2 | interface: changes | `ui/src/pages/changes/radio/**` (new), `ui/src/pages/changes/{ChangePage,index,ApplyStepper}.tsx`, `ui/src/api/types/changes.ts` | The radio plan view (the walk, what was not seen, the last reach test, the confirmation text of a dead-link return); the not-run check, drawn by `ApplyStepper.tsx`, which today reads only `check.ok`; the ledger and the removal of a leftover | the contract of F | r9's interface change is on main |
| K3 | interface: lab | `ui/src/pages/lab/{CableDialog.tsx,Palette.tsx,Inspector.tsx,Editor.tsx,model.ts,cabling.ts}`, `ui/src/api/types/lab.ts` | The radio cable and node with its site, RF sources, the options and faults, "what this lab proves" on the lab's page (`Editor.tsx`) | the contracts of C and H | |
| W1a | orchestrator | `crates/nocturne-suite/**`, `docs/STATUS.md`, `README.md`, `.github/**` | Register the two crates, with every method of the radio feature added in the same commit to `NEVER` in `crates/nocturne-assistant/src/tools/boundary.rs`, with its reason; the suite's method test and its boundary test over the schedules' public allow-list (no file of `nocturne-automation` is edited) | F7, G2, H, I2b | |
| W1b | orchestrator | the same | The end-to-end run on the lab; the CI step that no shipped binary carries the testing hook; the status entry that says what rests on documentation | everything else | |

**The order that follows.** W0, A1, C1 and the simulator's chain start now;
F0 as soon as the change engine's work is on main. The drivers' chain B1a to
B3 starts with the SNMP health fix, and D1 with the transport's fix; D2 after
B2a and D1; D3 after C4 and D2. The change engine's chain F1a to F7 then runs
in order, each delivery tested by the tests listed for it, none needing one
after it. G, H and I2a run beside it once their needs have landed; I1 can
start as soon as A1 and B1a have. K0 lands right after B1a. J and K1 to K3
run beside all of it against this contract and a mocked backend, and are
finished against the real crates. **W1a** registers the radio feature once
F7, G2, H and I2b have landed; J and K1b's end-to-end test then finish
against it; **W1b** last. Nothing needs a radio. The first thing that does
is the list of
recordings, which only the owner can supply; R1 and R2 are read-only and can
be taken the day E1's reads exist, or today from the terminal.

**What is still one piece's line in another's file.** With one owner per
crate there is little left:

* A trait method added by D2 has a default body, so every implementation of
  `SimConnector` (in `nocturne-lab`, H's; in `nocturne-terminal`'s tests,
  nobody's in this plan; in the core's `connect.rs` tests and `testing.rs`,
  D2's and D3's; in `nocturne-changes/tests/secrets.rs`, F's) compiles
  untouched.
* **The assistant's line, `NEVER` in
  `crates/nocturne-assistant/src/tools/boundary.rs` (J's file).** The suite's
  `every_method_the_application_registers_is_on_one_side_of_the_assistants_line`
  fails for a registered method that is on neither of its two lists, and
  for a listed name that is not registered. So a delivery that registers a
  method adds that name to `NEVER`, with its reason, in the same commit:
  F2 `changes.reach`; F4a `changes.guards`, `changes.guards.check`; F4b
  `changes.proofs`; G1 `monitor.radio`; W1a every method of the radio
  feature. That is one line per method, and the test's own failure message
  asks for exactly it. J, which owns the file, then moves to `ALLOWED` the
  names its tools reach and no other.
* A scripted session with no `origin` is synthetic, so the session files of
  the eight profiles that E does not own are not edited.
* The assistant's untrusted block is called by J's tools and not changed.
* Outside this plan, for the owners of those files, and not blocking: the
  sentence printed when a `disruptive` command is refused in
  `nocturne-automation` (runner, playbooks), `nocturne-assistant`
  (`tools/devices.rs`) and `nocturne-config` (`backup.rs`). They refuse
  correctly as they are and say "go through a change", which is the wrong
  advice for a command that cannot be typed into one.

## Known and accepted for the first build

The third round of review graded every point still open. One was a blocker,
and it is closed above: at a router of the reach walk, step 4 looked for the
router by the MAC of the interface the walk had arrived by, which no bridge
between that router and the next has learned, so a master behind a routed
radio hop came out `direct` (tower N25). Step 4 now takes the MAC of the
interface a device sends by toward the next hop, and F2's routed fixtures
give every interface of a router its own MAC, so the relay test fails on the
old reading.

Every other point the three reviews raised in that round is below, one line
each. The reviews graded none of them as stranding a device or a site,
giving a figure that anything acts on, or breaking a rule of the project as
the text would be built. Each line says what the point is and who resolves
it:

* **a piece of the build plan**: the line is part of that piece and is
  built, tested and reviewed with it ("When a delivery is done", under
  "Build plan"); where the line changes what a section above says, the line
  is what is built;
* **above**: closed in the text in this revision, and built by the piece
  named;
* **the owner**: settled by a recording;
* ***later***: not in the first build; the line says what the first build
  does instead and what brings it back.

The points carry the numbers the reviews gave them. The buildability
review's numbers start again at N1, so under "By piece" they are written
"build N1", beside "tower N26" and "rules R3-1".

### The tower review

| Point | What it is | Who resolves it, and how |
|---|---|---|
| N26 | A person's return on a link that flaps races the rebuild before sending: planned while the link was down and applied while it is up, or the reverse, the rebuilt plan differs and nothing is sent. | **F6b.** A move home is planned in one form whatever the peer is doing: no witness; always `radio_link_down_return`, worded "the link may be down when this is sent", with the meaning its typed confirmation has under "A return with the link down"; and the rebuild before sending does not compare whether the peer is registered. Whether the peer is registered in the baseline read just before the `set` decides the endings: with a peer, those of a master-only retune, with a signal floor at the master's end; without one, those of the return with the link down. The ledger still refuses it by the slave's MAC. Test: `a_return_on_a_link_that_flaps_is_sent_whether_or_not_the_peer_is_there`. |
| N27 | If the master's drill proof died after the forward move (its software upgraded during probation, or a Nocturne upgrade that changes the shipped guard lines), the return with the link down cannot be planned, and stage 2 of the drill that would renew the proof cannot pass with no peer to come back: decision D at its edge. | **F4b.** A drill on a device with no registered peer runs stage 2 with the peer check `not_run` and `blinked` recorded as unknown, and passes on the entry read gone, its mark and the radio's values. With `blinked` unknown the way home already waits from the guard's run. A person proves the revert again and then sends the master home, and neither needs a live link. Test: `a_drill_with_the_link_down_proves_the_revert_and_leaves_blinked_unknown`. |
| N28a | An inventory radio whose device cannot be read when a plan is made is silent, so a master behind it can come out `direct` with only a "not seen" line on the plan. | **F2.** Such a radio is asked through its latest stored `mac_table`: where that table held the stretch's two MACs on different sides, its answer is `unknown`, naming it and the table's age, so a stale table can only refuse. A stored table that holds one of them or neither leaves the radio silent and listed under `not_seen`, as now. Test: `an_unreadable_radio_whose_last_table_showed_the_crossing_leaves_the_stretch_unknown`. |
| N28b | Step 4 counted a per-station port that the profile cannot tie to its radio as "any other port", so an access point never showed the crossing, while step 3 calls the same port `unknown`. | **Above** (step 4: such a port leaves that radio's answer `unknown`, and a crossing another radio shows still decides). F2, with `a_mac_on_a_port_the_walk_cannot_place_leaves_the_stretch_unknown`. |
| N28c | The tables of every radio are read after `D`'s session carried traffic, but a bridge forgets in minutes and a large world takes minutes to read. | **F2.** Until the last table of a plan is in, `D`'s session sends one policy-checked read every `follow_poll_s`, so every stretch that session crosses keeps its two MACs fresh; a table that does not come in is that radio's "cannot be read" (N28a). Test: `the_plan_reads_the_bridges_after_it_has_talked_to_the_device`, also with a world slower to read than the bridges forget. |
| N29a | Row 1 of the `rollback.rs` table ("not registered at the follow deadline") blames the channel where the deadline was measured, with no exception for a restart, so a slave that lost power at the `set` and came home gets the target quarantined although its uptime shows the restart. | **F6a.** Row 1 takes row 5's exception: no blame where the uptime of either device, where it can be read, shows it restarted inside the change. Test: `a_slave_that_restarted_during_the_follow_wait_blames_nothing`. |
| N29b | In master-only control a slave's restart cannot be read, so a follow time measured on the way home from a slave that was off becomes the link's figure, and one such figure over 300 s refuses that link for good. | **F6a and A1.** In master-only control a follow time measured on the way home is recorded "may include a restart of the slave" (F6a); such a figure lengthens the deadlines only as far as keeps every one of them within `max_deadline_s`, never makes a plan refused, and makes the deadline it sets count as an estimate in the blame column (A1's arithmetic). Test (A1): `a_way_home_figure_that_may_include_a_restart_never_refuses_the_link`. |
| N29c | A follow time runs from the `set` to the peer registering, so on the way home to a radar channel the master's own radar wait is counted as the slave's follow, and one ten-minute wait refuses the link for good. | **F6a.** A follow time is counted from the end of the master's radar-wait state, where it reports one; the wait is recorded on its own, as the wait `return_deadline_s` already uses. Test: `a_follow_time_on_the_way_home_through_a_radar_channel_leaves_the_wait_out`. |
| N30 | `radio_slave_unseen` says the revert ends the outage; in master-only control that holds only if the slave still looks on the present frequency, which nothing can read. | **F3.** The hit's sentence reads "If the slave does not look on <f>, the link is down until the revert at <time>, provided it still looks on <present>; Nocturne cannot read its list." |
| N31 | A link that moved off a radar channel and is down on its new one can have nothing planned (home is a radar channel; a forward move needs a registered peer), yet the feature's alert says "the move back is planned". | **I2b.** For such a link the critical alert says "Nocturne cannot plan a return to a radar channel: set <master>'s frequency back to <old> by hand", which is what the plan said before approval (`radio_return_through_radar`), and nothing is planned. Asserted in `a_link_that_left_a_radar_channel_is_not_sent_home`. |
| N32 | Probation's lost-read rule compares one busy hour with the same hour a week before and by default counts any loss where there was none, so one lost ping in each of three busy hours quarantines a good channel. | **I2b.** A busy hour counts as worse for a device when it lost at least `probation_loss_margin` (3, *default*, a setting of the `radio` section) more samples than in its median busy hour of the evidence window before the move: pings not answered and, for the two radios, radio reads that failed. Bursts still count; one lost ping does not. Test: `one_lost_ping_is_not_a_worse_channel`, beside `a_device_that_loses_reads_in_bursts_counts_against_the_move`. |
| N33a | "The answer" said where `through_link` is seen ("at `D` in step 3, or at its peer in step 4"), which missed a device's own radio seeing its own link crossed at a later stretch. | **Above** (at any step of any stretch). F2, with `a_devices_own_link_crossed_at_a_later_stretch_is_through_its_link`. |
| N33b | Step 5 did not say what two equal routes at `Y` toward `D` mean. | **Above** (chosen as in step 2: two at the longest prefix is `unknown`). F2, in `the_longest_route_decides_and_two_of_that_length_are_unknown`. |
| N33c | The restart rule takes the world's lock again but not `connect::hold`, so the monitor or a backup could take the devices' sessions while a guard is waited out. | **F6b.** At restart the engine takes the lock, then holds both devices again before it reads them, and keeps both until the change is marked. In `a_restart_mid_change_takes_the_lock_and_reads_the_devices_before_it_says_anything`. |
| N33d | The way home is disarmed on the peer registering on the old channel, while fact 5 says "verified back on the old one". | **F6a.** They are one thing: the way home is verified by the peer registering again on the old channel inside `return_deadline_s` and by nothing more, because it restores what the link lived on, and that is what fact 5's words mean. No test changes. |
| N33e | It was not said whether the rebuild before sending compares `reach.not_seen`. | **F4b.** It does not: it compares the reach and its `via`. A radio that could be read at approval and cannot be read at apply decides through its stored table (N28a), which can only refuse; comparing the list as well would refuse every return planned with the link up and sent with it down, because the far radio cannot be read then (N26). In `a_plan_whose_live_facts_changed_since_approval_is_not_applied`. |
| N33f | It was not said whether "home" survives an outside move. | **F6b.** It does not: a move home exists only while no outside move of that radio has been recorded since the last forward move, and after one the link has no home until its next forward move. In `an_outside_move_is_recorded_when_it_is_seen`. |
| N33g | R4 times the reach test on a directly reached master only. | **Above**, for the owner: R4 now has one run with M managed only across the link, recording that the second read stalls. Nothing in the build waits for it. |
| N33h | In master-only control the signal floor at the master's end needs the power-row term of the slave's list, which cannot be read. | **F5.** The term is 0 there, and the verification lists "the slave's permitted power on <target>" as assumed, as the score's T9b does (rules R3-17 is the same point). Asserted in build N3's test. |
| N33i | An end that is `behind_link` but whose management does not cross its own link (S2 in the tower review's timeline B-1) is not surveyed, though a survey could not cut it off. | ***Later.*** The first build surveys only an end computed `direct`. What brings it back: a rule that the end's own link is on none of its stretches, with its tests. |

### The rules review

| Point | What it is | Who resolves it, and how |
|---|---|---|
| S13, R3-8 | Fact 3 said each step needs the one below it and there is no way round a rung, while "Staged trust" lets a person's retune need the drill (rung 2) and no survey (rung 1). | **Above** (fact 3: each rung needs the rungs its row names). The engine's proofs (F4b) and the ladder that `radio.link.get` returns (I1) follow the table. The orchestrator is asked to confirm that a person's retune needs the drill and not a survey. |
| R3-1 | r9's undo is an exact intended configuration, so the rule that leaves a radio's line out of it makes it fail its check on RouterOS whenever the radio moved since the change being undone. | **F3.** The undo's intended configuration is the stored `before` with its lines that match a stack's `menu` replaced by those the device holds now, and its lines that match `radio.guard.line` left out; its plan says so on its own line ("<radio> is left as it is: a radio is changed only by a radio change"), its exact check can pass, and it ends `applied` with that note. A device that holds a leftover is refused by rule (1) before any of this. Test: `an_undo_on_a_device_whose_radio_moved_since_leaves_the_radio_and_still_verifies`, and build N13. |
| R3-2 | r9 exempts every undo from the maturity gate, while "Staged trust" says the way back from a retune needs `changes.allow_unverified_writes` again. | **F7.** The way back from a retune is a `RadioRetune` that carries `undoes`, so the Changes screen links the two and, once it is applied, the original reads as rolled back as r9 records any undo; and it is not exempt: `require_verified_or_allowed` exempts an undo whose targets are `Intended` (lines the engine works out from a stored configuration), not a radio change. Test: `the_way_back_from_a_retune_is_linked_to_it_and_needs_the_setting_while_unverified`. |
| R3-3 | On RouterOS every `Lines` change fails its check and is rolled back by derivation, so the rule that keeps derived rollbacks off a radio's line would leave a radio line a person typed where it was typed, with the rest undone. | **F3.** That rule is for the rollback of an older change (r9's undo, R3-1). The rollback after a failed check of the same apply keeps rule (3), which keeps any revert's entry out of it, and otherwise stays as it is today: its base is the backup that apply took just before sending, after its drift check, so the only radio line it can put back is one this change's own commands changed, and the person's typed change is undone like the rest of it. Test: `a_typed_radio_line_whose_change_fails_its_check_is_put_back_with_the_rest`. |
| R3-4 | `follow = max(60, 3 × measured)` with the estimate only where nothing was measured: a quick way home gives the next forward move a deadline shorter than the estimate that already failed, marked measured, and its revert then blames the channel. | **A1 and F6a.** `follow = max(follow_estimate_s, 3 × the largest follow time this link showed in its last three retunes)`, the estimate alone where the slave's list changed length since: the estimate is the floor, a learned figure only lengthens, and a deadline equal to the estimate counts as an estimate in the blame column. Test (A1): `a_quick_way_home_does_not_shorten_the_next_forward_deadline`; `a_learned_time_never_shortens_a_deadline` then holds against the estimate. |
| R3-5 | A master on a radar or unknown channel borrows stage 2 from another radio of the same role, software, account and line, and with it that board's `blinked`, though whether a same-value set restarts the radio belongs to the board. | **F4b.** A borrowed stage 2 never says `blinked: false`: on such a master the guard's run always restarts the wait for the way home, as where the proof does not say. Asserted in `a_borrowed_stage_must_be_of_the_same_role_software_account_and_line`. |
| R3-6 | A ledger entry refuses plans by the radio's MAC but is listed, closed and removed by device id, so a slave deleted and added again under a new id keeps an entry that nothing closes. | **F4a, F4b and I2b.** An entry belongs to the inventory device whose radio has its MAC as much as to the id it was written for: `changes.guards {device_id}` lists it and `changes.guards.check` closes it under either (F4a), `RadioGuardRemove` removes it under either (F4b), and the feature calls `changes.guards.check` after a successful monitor read of every device whose radio has an open entry's MAC (I2b). Test (F4a): `a_deleted_slave_with_an_open_ledger_entry_still_blocks_its_masters_retune` goes on to show the first read under the new id closing it. |
| R3-7 | `the_feature_calls_exactly_its_listed_methods` proves the list in `calls.rs`, not that nothing else in the crate calls a method. | **I1.** A second test reads the crate's own sources and fails if any file but `calls.rs` calls `rpc(`: `no_file_but_calls_reaches_another_method`. |
| R3-9 | Probation does not leave the move itself out: a move approved inside a busy hour loses pings for its own outage, and radio reads fail while the link re-forms, both counted against the new channel. | **I2b.** Probation starts at the first whole hour after the change ended; samples inside the change's maintenance window are left out of both sides of the comparison; a read refused because a radio change held the device is a gap, never a failed read. Test: `a_move_in_a_busy_hour_is_not_judged_by_its_own_outage`. |
| R3-10 | A survey has its success test and a test of a scan that never ends, and none of a scan that returns after which the peer does not come back. | **F7.** `a_survey_after_which_the_link_does_not_come_back_is_failed_with_the_alert`: `failed`, with the critical alert naming any radio of the same network name. |
| R3-11 | The assistant's refusal of radio intents and the radio branches of `changes.rollback` and `changes.delete` land in F7, after F4b already routes a radio apply to the executor. | **Above**, in F4b's row and test list: the three refusals land with the first radio apply (`changes.assistant_apply` refuses a radio intent first thing; `changes.rollback` refuses every radio change, and F7 then makes a retune's a planned way back; `changes.delete` refuses a change a proof or an open ledger entry refers to), and their three tests moved from F7 to F4b. |
| R3-12 | Rule (3) speaks of a derived line "that removes an entry whose name matches the guard's name pattern", and telling a removal from another line needs RouterOS syntax. | **F3.** It reads "a derived line that holds a name the guard's name pattern matches": the pattern is the engine's own, and no vendor fact is needed. |
| R3-13 | Which parser takes the radio's export line apart was written in Rust ("the existing `kv` parser"). | **Above** (`line_parser` in the stack: `kv` for RouterOS and for the lab). B2a adds the field to the schema, E2 and C4 set it, F5 uses it. |
| R3-14 | The lab's maturity rule names a profile id (`nocturne_sim_radio`) in the engine. | **B2a, C4 and F1a.** The `radio:` section gains `simulated: true`, which the loader accepts only on a built-in profile (B2a); the lab profile sets it (C4); a plan on a lab device starts from `verified` when the device carries the lab marker and the profile in effect is the shipped one with `simulated: true` (F1a). In `the_shipped_lab_profile_on_a_lab_device_is_verified_and_a_users_override_is_not`. |
| R3-15 | A task may run an Ansible playbook with `check: false` over Ansible's own SSH, outside the pipeline (the exception `ansible.rs` documents) and outside `connect::hold`, so "any other automated session ... is refused at once" holds for Nocturne's own sessions only. | **Outside this plan**, for the owner of `nocturne-automation`: it is there today, and the radio design does not depend on it. In this build the hold covers Nocturne's own sessions, and it does not stop a playbook run against a radio during a radio change. |
| R3-16 | Two defaults stand in for a measurement and are not named under `assumed`: the 0.5 airtime split when a committed need exists and there is no traffic history, and 19:00 to 23:00 as the busy hours when there is no traffic history. | **I2a.** Each is named in `Basis.assumed` when it is used, so a proposal resting on either is a trial. In `every_assumption_is_named_and_makes_a_trial`. |
| R3-17 | The signal floor subtracts the row difference of the transmitter's own list, which for a master-only link's uplink cannot be read. | **F5**, as tower N33h. |
| R3-18 | `changes.guards.check` closes ledger entries, on which the engine's alerts and every later plan depend, and writes no audit entry. | **F4a.** It writes one audit entry for each entry it closes: the device, the guard's name and what the read showed. |
| R3-19 | The reach test's "two such answers in a row come only from a master reached without its link" has one exception: a link that comes back and drops again between the second request crossing it and the master answering it. | **Accepted**, and recorded here as an untested case of the reach test, beside those under "Not verified": it needs that flap and a walk that already said `direct` wrongly. R4 records when the table empties. |
| R3-20 | F0's review rule lists today's test files of the change engine, and r9 adds `tests/undo.rs`. | **F0.** The rule is general ("no test file that exists at that commit"), and `tests/undo.rs` is one of those files. |
| R3-21 | "No retry tonight" (`rollback.rs`, the way home) is the feature's ranking, not the engine's: a person can plan the same retune again at once. | **I2b.** The feature proposes no second move of that link the same night; the engine does not refuse a person's plan for it. |
| R3-22 | Every radio plan reads the tables of every device of the world that has a radio, twice; on a network of hundreds of radios that is hundreds of sessions per plan, each taking a device's one slot. | **F2.** Reads only, so within the rules. A radio whose read does not come in is "cannot be read" (tower N28a) and is listed under `not_seen`. |

### The buildability review

| Point | What it is | Who resolves it, and how |
|---|---|---|
| N1 | The walk reads ARP and bridge tables that only real traffic fills, and the lab and the fake carry none (sessions on the console, probes that leave ARP untouched), so every lab and fake radio would read `unknown` and no test could show the core case. | **D3, H, C1 and F2.** An in-band or wired session's exchanges are carried as simulated traffic between the management station and the node, one traced TCP exchange per command (`Lab::start_trace`, which the simulator has), so ARP and the bridges learn them as on a real network and no probe decides anything (D3 in `radio_world`, H in the lab's connector); the lab radio's `show interfaces` lists the bridge of its two ports with that bridge's MAC, the one step 4 looks for (C1); the fake's `ConnectionInfo` carries the station's address as the local address and the node's management address as the peer, for `lab: false` as for `lab: true` (D3); F2's tests through `App::rpc` assert the computed value and its reason, and F2 adds `a_wired_master_reads_direct_from_live_reads`, H `a_session_teaches_the_bridges_on_its_way`. |
| N2 | The two tests that tell "two answers in a row" from one cannot happen in the fake: a late "not registered" answer needs latency, and a slave back within two reads needs reads slower than the lab's follow. | **D3.** `radio_world` takes a one-way latency for in-band paths, in simulated milliseconds per message, and a message that entered a path before it went down is delivered; the latency alone makes the fast slave. |
| N3 | No named test makes a master-only retune end `applied`. | **F6a.** † `a_master_only_retune_whose_slave_follows_is_verified_from_the_master_and_applied`: the slave managed `none`; the slave end's floor from the master's `remote_signal` where the stack passes it on, `not_run` otherwise; `radio_slave_unseen` on the plan; the uplink's power term 0 (tower N33h). |
| N4 | Two suite tests made every delivery that registers a method red until `boundary.rs` (J's) lists it, J and W1 needed each other, J did not own `tests/tools.rs`, and K1b's end-to-end test needs a server with the radio feature. | **Above**, in the build plan: each delivery that registers a method adds it to `NEVER` in the same commit (under "What is still one piece's line in another's file"); J owns `tests/tools.rs` and lands after W1a; W1 is W1a (registering, with the radio feature's methods on `NEVER`) and W1b (the end-to-end run, the CI step, the status entry); K1b's end-to-end test needs W1a. |
| N5 | The interface's exhaustive maps over `Kind` (`device.ts`, `DeviceTypes.tsx`) and over the command class (`paste.ts`, `snippets.ts`, `PasteSheet.tsx`) and the change page's `ApplyStepper.tsx` were in no row, and the terminal receives `disruptive` from B1a on. | **Above**, in the build plan: K0, right after B1a, owns the command class in `terminal.ts` and the three terminal files; K1a owns `device.ts` and `DeviceTypes.tsx`; K2 owns `ApplyStepper.tsx`. |
| N6 | A new `NodeKind` or `CliKind` for the radio bridge, the natural way to write C1, breaks `nocturne-lab/src/devices.rs`, which matches both exhaustively, until H. | **Above**, in C1's row: the model is the existing `AccessPoint` with the existing `Ios` CLI. The lab makes it a `cisco_ios` device until H gives it `nocturne_sim_radio`, and H, which owns the lab's `every_example_loads_starts_and_stops_with_its_devices`, changes that test with the sites. |
| N7a | Named tests need faults the simulator does not list. | **C3.** `GuardRunsLate` prints `next-run` on time and fires late; `GuardDueEarly` and `GuardNoNextRun` (a `next-run` printed early, and none at all); `SettingNotTaken` (a setting accepted and not stored); `GuardRemoveKeepsEntry` (`schedule remove` leaves the entry); `SetAlsoChanges {property}` (a set that changes another property of the line); and a restart clears the marks, as RouterOS's memory log is remembered to. |
| N7b | Named tests need abilities the fake does not list. | **D3.** `speed: 0`; a second `App` built at the same data directory and attached to the same running world (F6b's restart tests); a transcript per device with the simulated instant of every exchange (`the_plan_reads_the_bridges_after_it_has_talked_to_the_device`, and the reach test's record in F6a). |
| N8 | The configuration check named the `kv` parser, which could not read the lab radio's export line (word pairs), and no test makes any one of its three parts fail. | **Above** (the lab's line is written as `name=value` pairs, read through `line_parser: kv`), built by C1 and C4; and **F5** adds `another_line_of_the_export_that_changed_fails_the_check`, `an_unplanned_key_on_the_radios_line_fails_the_check` (with C3's `SetAlsoChanges`) and `a_configuration_field_off_the_plan_fails_the_check`. |
| N9a | B3's `the_wep_key_names_are_redacted` tests patterns that E1 adds. | **E1** takes the test. |
| N9b | E2's `a_radio_that_reports_no_country_set_is_refused` needs the harness to evaluate a stack's `refuse` rules, which nothing in it does. | **B2b, B3 and E2.** B2b gives the drivers the one evaluation of a stack's `refuse` rules over a row, which the planner calls (F1b); B3 gives `expect.yml` a `refused:` field checked through it; E2's test uses that field. |
| N9c | F1b's `a_move_home_is_exempt_from_the_power_and_indoor_rules` and `only_a_return_may_be_planned_with_the_link_down`, F3's test of `radio_link_down_return` and F2's `a_master_whose_last_reach_test_failed_is_not_surveyed` need the engine's own record of a forward move or of a reach test, which nothing writes before F6a. | **F1b, F2 and F3.** Each seeds that record in the store through `tests/common` (F's). |
| N9d | From F4b on, every earlier test that plans a retune successfully needs a drill proof. | **F4b** gives `tests/common` a helper that seeds a valid proof, and those tests call it from then on. |
| N9e | F4a's `a_dead_man_that_ran_at_once_stops_the_change_before_anything_is_sent` has no change to stop before F4b's executor. | **F4a** names it for the guard function, `arming_reads_back_a_dead_man_that_ran_at_once_as_not_armed`; F4b's prelude is what stops the change. |
| N9f | B1a's `an_error_rate_is_not_a_quality_figure` has no behaviour in the drivers to test. | **B1a** drops it; G2's `the_quality_rule_never_reads_an_error_rate` is that test. |
| N10 | `a_lab_does_not_change_the_reach_of_a_real_device` and its converse pass with the world rule deleted, because a lab radio has learned none of a real device's addresses. | **F2.** Each fixture plants a radio of the other world whose table holds the device's stretch MACs on different sides. |
| N11 | `changes.reach` opens no session, and step 1 needs the address the device sees the session come from. | **D2 and F2.** `connect::seen_from` answers from the last session's `ConnectionInfo` and says its age (D2); `changes.reach` uses it and shows that age (F2). |
| N12a | The guard's four reads are written as the guard block's patterns, while "Operations and their reads" calls them steps shaped like collect steps. | **B2a.** They are the guard block's own patterns as written there (`fields`, `armed`, `present`, `name`), not collect-shaped steps. |
| N12b | A `per: row` command carries `{interface}`, and how a capture of one is named was not said. | **B1b and B3.** By the command's slug with the interface after it (`<slug>@<interface>.txt`). |
| N12c | `radio::read_operation(profile, stack, operation, role, text)` takes one text, while `allowed` is two commands merged on `control_mhz`. | **B2b.** It takes the texts of the operation's steps, in order. |
| N12d | "A node without one is a site of its own" would change the site of every lab device. | **H.** For radio nodes only; other lab devices keep `Lab: <lab name>`. |
| N12e | The lab enables polling for `cisco_ios` lab devices only (`devices.rs::monitoring`). | **H.** The radio profile's devices are polled too, so that G1's lab cadence sees them. |
| N12f | `Interface.parent` for RouterOS is one map line in the existing `/interface vlan print detail` step, and E1's row named `Route.active` and not it. | **E1** adds it; without it every management VLAN on a tower reads `unknown`. |
| N12g | The radio series read as raw names in the Charts tab without `ui/src/pages/monitoring/vocab.ts`. | **Above**, in the build plan: K1a owns `vocab.ts`. |
| N12h | `nocturne-changes/tests/secrets.rs` implements `SimConnector` too, which the list in the build plan missed. | **Above**, in that list; D2's default body keeps it compiling. |
| N12i | `changes.rollback` answers r9's `{change, job_id}`, while the contract table says `{change_id}`. | **F7** answers in r9's shape for the planned way back; the contract table is read with it. |
| N12j | The end-to-end run's "prepare ... its session lost" needs `list-change-restarts` on the tower link's non-root, and "the slave that never follows" needs a cause the planner allows on a `both` link. | **C4** sets `list-change-restarts` on that non-root in "Two towers"; **W1b**'s run makes the slave that never follows with a target late in a long list, past its follow deadline. |
| N12k | `nocturne-app` compiles `testing` under `cfg(any(test, feature = "testing"))`, so with `testing = ["dep:nocturne-netsim"]` the crate's own tests lack the simulator. | **D3** adds `nocturne-netsim` as a dev-dependency too. |
| N12l | A restart finds a lab change `applying` while its lab is not running. | **F6b.** It ends `failed`, "the lab is stopped"; its ledger entry stays open until a read of the device closes it once the lab runs again. |
| N13 | F3's `rolling_back_an_older_change_never_touches_a_radio_or_a_dead_man` described the flow before r9. | **F3**, with rules R3-1: restated for r9's undo (its plan keeps the radio's line as the device has it and says so, nothing radio is sent, and a device that holds a leftover is refused by rule (1)). |

### By piece

What each piece of the build plan resolves from the lists above, beside its
row and its tests:

* **A1**: rules R3-4; tower N29b.
* **B1a**: build N9f. **B1b**: build N12b. **B2a**: rules R3-13, R3-14;
  build N12a. **B2b**: build N9b, N12c. **B3**: build N9b, N12b.
* **C1**: build N1, N6, N8. **C3**: build N7a. **C4**: rules R3-13, R3-14;
  build N8, N12j.
* **D2**: build N11. **D3**: build N1, N2, N7b, N12k.
* **E1**: build N9a, N12f. **E2**: rules R3-13; build N9b.
* **F0**: rules R3-20. **F1a**: rules R3-14. **F1b**: build N9c.
* **F2**: tower N25 (the blocker), N28a, N28b, N28c, N33a, N33b; rules
  R3-22; build N1, N9c, N10, N11.
* **F3**: tower N30; rules R3-1, R3-3, R3-12; build N9c, N13.
* **F4a**: rules R3-6, R3-18; build N9e.
* **F4b**: tower N27, N33e; rules S13, R3-5, R3-6, R3-11; build N9d.
* **F5**: tower N33h; rules R3-13, R3-17; build N8.
* **F6a**: tower N29a, N29b, N29c, N33d; rules R3-4; build N3.
* **F6b**: tower N26, N33c, N33f; build N12l.
* **F7**: rules R3-2, R3-10; build N12i.
* **H**: build N1, N6, N12d, N12e.
* **I1**: rules S13, R3-7. **I2a**: rules R3-16. **I2b**: tower N31, N32;
  rules R3-6, R3-9, R3-21.
* **J, K0, K1a, K1b, K2, W1a**: build N4, N5, N12g, in their rows.
  **W1b**: build N4, N12j.
* **The owner**, in recording R4: tower N33g; rules R3-19.
* ***Later***: tower N33i. **Outside this plan**: rules R3-15.
