# hp_procurve: what is checked, what is not, and what did not fit

`hp_procurve.yaml` is `maturity: community`, and of the three profiles added
alongside it, it is the one with the most written from documentation rather
than from a device. This file says which parts.

## Checked against recorded device output

Everything under `captures/`. Each of the five cases is a verbatim copy of an
ntc-templates test capture, cited file by file in its own `SOURCE.md`, and
every value in its `expect.yml` was written by reading the `.txt`.

That covers all five collectors (`interfaces`, `vlans`, `mac_table`, `arp`,
`neighbors`): the commands, the templates, the field mappings and the
expressions — including the one real piece of arithmetic in the profile, the
rewriting of ProVision's one-word `1000FDx` / `10GigFD` mode column into a
rate and a duplex.

## Not checked against a device at all

* **Prompts** (`prompts.yml`), including the factory system name with spaces in
  it (`ProCurve Switch 2824#`). This is the riskiest guess in the profile: the
  hostname part of `cli.prompt` allows up to three extra space-separated words,
  which is looser than any other profile here, and the first thing to check on
  a real switch is that no line of output ending in `#` or `>` gets through it.
  `not_prompts` is stocked with candidates from the real captures for that
  reason.
* **Error messages** (`errors.yml`, `rejected` section). ProVision's refusals
  are plain sentences — `Invalid input: nonsense`, `Incomplete input: show`,
  `Port 99 does not exist.` — typed from the platform's documented messages.
  This is the single most important thing to verify on real hardware: these
  patterns are the only thing standing between a refused command and an empty
  interface table being reported as the truth, and unlike every Cisco-like
  platform there is no `%` to fall back on. The `clean` samples are copied out
  of the real captures.
* **Detection** (`detect.yml`). The sysDescr samples are written from
  documented output; the `show version` probe samples too, because the one
  `show version` capture in the ntc-templates corpus for this platform is an
  **Aruba access point's**, not a ProVision switch's, and using it would have
  made the profile claim the wrong device.
* **The paging and session commands.** `no page`, `terminal`-less and all, and
  the `-- MORE --` pattern.
* **The whole configuration path.** `config/acc-sw-01.txt` is hand-written (see
  `config/SOURCE.md`), and so is `session/ssh-configure-save.yml`.

## Deliberately left out

* **`facts`.** There is no ProVision `show version` or `show system` capture to
  check a step against, and `facts` is the one collector whose output is shown
  to a human as the identity of the device — a hostname or serial number read
  with a guessed template would be wrong in the most visible possible place.
  The commands to add when a switch is available are `show system` (name,
  uptime, serial, ROM and software version) and `show version`.
* **`routes`, `health` and `inventory`.** Same reason: no captures.
  `show ip route`, `show system`, `show modules` and `show tech transceivers`
  are where they come from.
* **`config.guard`.** ProVision's running configuration does not survive a
  reboot until `write memory` has been run, so `reload after <dd:hh:mm>` is a
  real dead-man switch of the IOS kind. It is not declared, because the command
  and its exact argument format are documented only and arming a guard that
  does not really arm is worse than having none — the change pipeline would
  report a change as protected when it was not.
* **Three fields the model cannot hold.** `show interfaces brief` reports
  `Intrusion Alert`, `Flow Ctrl` and `Bcast Limit` per port; the ARP table
  reports a `Type` (dynamic/static); the MAC table has no learned/configured
  column at all. `Interface` and `ArpEntry` in `model.rs` have no field for
  any of those and no free-form `extra` map, so they are dropped rather than
  squeezed into a field that means something else.
* **`config.errors`.** Left empty so that `cli.errors` apply in configuration
  mode too. `find_error` uses `config.errors` *instead of* `cli.errors` when
  they are non-empty rather than in addition, so a config-only pattern would
  mean retyping all of them.

## The two steps of `interfaces` are not checked against one switch

`captures/procurve-ports/` holds `show interfaces brief` from a 24-port
standalone switch and `show interfaces status` from a stack. They are the only
recorded output of each command, and they are from different switches: the
first numbers its ports `1`–`24`, the second `1/1`–`2/52`. So no row of the
second merges into a row of the first.

The case is written to make that explicit and to assert the behaviour that
matters — `merge_on` drops a row that matches nothing rather than inventing an
interface — but it means the **second step's mapping (`description`, `mode`,
`vlan`) has never been seen to produce a value**. On a real switch both
commands name the same port identically and it will. Pointing the profile at
a switch and checking that `description` and `vlan` come out filled is the
other thing to do first.
