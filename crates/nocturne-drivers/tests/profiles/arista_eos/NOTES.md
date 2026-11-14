# Notes on the `arista_eos` profile

Written while building the profile from the recorded output in
`crates/nocturne-textfsm/tests/fixtures/arista_eos/` (the ntc-templates test
corpus, Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`).
**Nothing here has been pointed at a real Arista device**, which is why the
profile says `maturity: community` and must not be given a write path.

## Model fields that are left empty, and why

| field | why |
|---|---|
| `facts.hostname` | EOS does not print the hostname in `show version`. It needs `show hostname`, for which there is no capture, so no step reads it. The hostname the session reads off the prompt is unaffected. |
| `facts.extra.hardware_version` | The only `show version` capture is a cEOS container image, whose `Hardware version:` line is empty. The field is mapped and will fill on hardware. |
| `health.cpu_pct`, `health.temperature_c` | No capture. See the missing commands below. |
| `health.memory_*` on hardware | Read from `show version`, which prints them on cEOS. Whether a hardware EOS release prints `Total memory:`/`Free memory:` in the same place has not been checked against a capture. |
| `interfaces.speed_bps`, `duplex`, `mode`, `vlan` | Mapped from `show interfaces status`, but see "unverified mappings" below: no capture merges into a row. `show interfaces` states speed and duplex in prose the bundled template does not capture. |
| `interfaces.ipv6` | Nothing to read it from: there is neither a capture nor a bundled `arista_eos_show_ipv6_interface_brief` template, and the `show interfaces` template captures only IPv4. |
| `interfaces.counters.*` | The bundled `arista_eos_show_interfaces` template captures no counters at all, and the one capture (an SVI and a VXLAN interface) prints none, so there was nothing to write an inline template against. |
| `arp` VRF | `show ip arp` names the VRF of every entry and the profile parses it, but `ArpEntry` in `nocturne-drivers/src/model.rs` has no VRF field, so it is dropped. Two identical addresses in two VRFs are two indistinguishable rows. |
| `inventory.name` | EOS answers with a stack of tables and the bundled template drops which table a row came from. The only name left would be the bare slot or port number, which is `1` for the first power supply, the first fan and the first transceiver alike. Filling it would invent a distinction the output does not make. |
| `routes.vrf` | Empty in the capture, which has no `VRF:` heading. Mapped, so it fills when one is present. |

## Commands I had no capture for (the next things to do)

In rough order of worth:

1. **`show hostname`** — fills `facts.hostname`. A two-line capture is enough.
2. **`show lldp neighbors detail`** — the whole `neighbors` kind is missing, so
   topology sees nothing on an EOS device. `arista_eos_show_lldp_neighbors_detail`
   is bundled; only the recorded output is missing. There is **no** bundled
   `arista_eos_show_cdp_neighbors*` template, so CDP on EOS would need an
   inline one (or `{ntc: cisco_ios_show_cdp_neighbors_detail}`, which is a
   claim about EOS's output format that would need a capture to make).
3. **`show processes top once`** — `health.cpu_pct`. A bundled template exists.
4. **`show environment temperature`** — `health.temperature_c`. Bundled
   template exists.
5. **`show interfaces`** of a device with physical ports and counters — would
   let an inline template read the byte and error counters, as `cisco_ios.yaml`
   does, and would also give the merge target the `show interfaces status`
   step needs (below).
6. **`show ip route vrf all`** — a second, optional routes step for the other
   VRFs' tables, as `cisco_ios.yaml` has.
7. **`show ip interface brief`** — a capture exists in the corpus but is not
   used: `show interfaces` already carries the addresses, and as an enrichment
   step it would have the same unverifiable-merge problem as
   `show interfaces status`.

## Unverified mappings (exercised but not proved)

The second `interfaces` step runs `show interfaces status`. The corpus has a
capture of it, so the step is exercised, but be precise about what that buys.
The only EOS `show interfaces` capture (`Vlan888`, `Vxlan1`) and the only
`show interfaces status` capture (`Et1`–`Et7`, `Ma1`) are from different
devices and share no port name, so **every one of the step's rows is dropped
by the merge** and no expectation ever sees a field it wrote.

Proved by the test:

* the command resolves to a bundled template — that is checked when the
  profile loads, so a command with no template cannot ship;
* every expression in the step's `map` is syntactically valid and uses filters
  and tests that exist — also a load-time check (I confirmed this by putting
  `| nosuchfilter` in the step: the profile then refuses to load);
* the template parses the real recorded output without failing the step;
* the merge drops rows that match nothing instead of inventing interfaces —
  the case's `count: 2` is exactly that assertion.

**Not** proved: the values. `mode`, `vlan`, `duplex` and `speed_bps` rest on
my reading of the capture's columns and nothing else. And a *runtime*
expression failure in this step would not be caught either: I checked, by
replacing the `vlan` expression with `NO_SUCH_VALUE.nested` and with
`VLAN_ID * 2`, and in both cases the collection reported no problem at all —
an undefined value is simply left out of the row, which is the documented
behaviour ("a field whose expression gives nothing … is left out"). So a wrong
expression here fails silently. One capture of an EOS device recorded with
both commands would close the whole gap; until then this step is the least
trustworthy thing in the profile.

## Taken from vendor documentation rather than from a capture

Marked here because none of it is backed by recorded output:

* **`cli.init`: `terminal width 32767`.** EOS's documented maximum width.
  `docs/profiles.md` uses the same value in its worked EOS example.
* **`cli.config_prompt`: `\(config[^()]*\)#\s?$`.** Every EOS configuration
  sub-mode is spelled `(config…)`. The override exists because EOS's
  `routing-context vrf MGMT` leaves the prompt `sw1(vrf:MGMT)#`, which is
  still exec mode but which the base's `\([^()\s]+\)#` would read as a
  configuration prompt — and the driver would then try to `end` out of it.
  The `(vrf:MGMT)` spelling is from documentation; the `(config…)` prompts in
  `prompts.yml` are the ordinary ones.
* **`detect.sysdescr`: `Arista Networks EOS`** and **`detect.sysobjectid`:
  `1.3.6.1.4.1.30065.`** — Arista's IANA enterprise number is 30065. The
  sysDescr wording in `detect.yml` is the documented one.
* **`config.save`: `write memory`** (inherited from the base). EOS accepts it
  as an alias for `copy running-config startup-config`.
* **`config.ignore`: `^! Command: ` and `^! device: `** — the two comment
  lines EOS generates above the configuration. The `! device:` line carries
  the running EOS version, so ignoring it keeps a software upgrade from
  looking like a configuration change.
* **`policy.yml`'s `zerotouch enable`** and the EOS messages in `errors.yml`
  (`% Unavailable command (not supported on this hardware platform)`,
  `% Invalid input (at token 1: 'nonsense')`) are EOS's documented wordings.
* **The hand-written configuration** under `config/`, as its own `SOURCE.md`
  says. Its layout — the header comments, three-space indentation, EOS's
  spellings — is from documentation.

## Deliberately not claimed

* **No `detect.banner`.** EOS answers SSH with OpenSSH's own identification
  string, which says Linux as loudly as it says EOS, and its pre-login banner
  is whatever the operator configured. A pattern here would steal detection
  from the `linux` profile.
* **No `config.guard`.** EOS's dead-man switch is `configure session` with
  `commit timer`, which is not a command that can be armed at the exec prompt
  around a line-by-line change: it needs the whole change built inside a named
  session and committed. Writing IOS's `reload in {minutes}` here would claim
  a guard nobody has tried on an Arista device. `nocturne-changes` will need
  the session workflow before EOS has a guard.
* **No `snmp` section.** The OID usually quoted for EOS CPU,
  `1.3.6.1.4.1.2021.11.11.0`, is UCD-SNMP's `ssCpuIdle` — the percentage of
  CPU that is **idle**. Read as a utilisation it would be exactly wrong, so it
  is not here, and the memory OIDs beside it (`memTotalReal`, `memTotalFree`)
  report kilobytes where the model wants bytes. Someone with a device should
  settle these before they go in.
* **No `when` on `mac_table`.** `cisco_ios.yaml` filters the switch's own
  addresses out of the MAC table (`CPU`, `SWITCH`, `ROUTER`), and
  `cisco_nxos.yaml` does the same for `sup-eth1` and `Drop`. The EOS capture
  shows no such pseudo-port, so there was nothing to read a filter off and
  none is here. If a real EOS switch turns out to list one, the fix is one
  `when` line — but guessing its spelling now would drop rows for a reason
  nobody could check.
* **Nothing added to `policy.read_only` or `policy.dangerous`.** The IOS-style
  base covers every read an EOS operator makes daily; `policy.yml` proves that
  for EOS's own commands and, more importantly, that `copy running-config
  startup-config`, `zerotouch enable`, a bare `show` and a pipe that writes
  are all changes.

## What the profile format made awkward

1. **A child replaces a list, it cannot add to one.** This is the big one.
   `config.ignore` had to be restated in full to add EOS's two header lines,
   and two of the base's entries (the console log-message patterns) were
   copied across by hand because they are not IOS syntax but genuinely shared.
   The same rule is why `policy.dangerous` is **not** touched here even though
   EOS's `zerotouch enable` belongs in it: adding one pattern would mean
   copying the base's fourteen, where a transcription slip would quietly
   weaken the policy of a `verified` profile's sibling. The command stays a
   change by default deny, which is safe but less clear than it should be.
   An `append`-style merge for lists, or a separate `policy.dangerous_extra`,
   would fix this.
2. **Every collector step must be exercised by a capture.** Right, and it
   caught real gaps — but it also means a step can be "exercised" by a capture
   that merges into nothing (see unverified mappings), which looks the same in
   the summary line as a step whose output was checked row by row. A capture
   case cannot say "this step contributed these rows".
3. **A `facts` field that is always empty cannot be distinguished from one
   that was never mapped** in `expect.yml`: there is no way to assert that a
   key is absent from `extra`, only that a listed one matches.
4. **Rows are found by their first field**, so an expectation has to be
   written with a field that is unique in the table as its first key. For the
   EOS MAC table, where one address appears seven times, that meant choosing
   rows carefully rather than writing the obvious ones.
