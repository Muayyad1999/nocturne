# n9k-interfaces

`show interface` of a Nexus 93180YC: an SVI whose VLAN is down, the
out-of-band management port and one access port. Each is awkward in its own
way, which is why they are kept together:

* `Vlan20` is printed in IOS's layout ("…, line protocol is down"), the other
  two in NX-OS's ("… is up" followed by "admin state is up"). The bundled
  template puts the line-protocol state of the first and the administrative
  state of the others into the *same* field, so the profile reads the
  administrative state from the first state line instead — where NX-OS writes
  "(Administratively down)" — and the operational state from the same line;
* `Vlan20` has no speed, no duplex and no counters at all: a routing metric
  ("BW 1000000 Kbit") is not a link rate, so `speed_bps` stays empty;
* `mgmt0` uses NX-OS's older counter layout, where the byte totals sit on the
  broadcast line of the Rx and Tx blocks. The profile's inline template reads
  only the current layout, so `mgmt0`'s octet counters are left **empty**
  while `Ethernet1/1`'s are filled. Empty is the honest answer here; see
  NOTES.md;
* `Ethernet1/1` is the only one with a layer-2 role ("Port mode is access").

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_interface.txt` | `tests/cisco_nxos/show_interface/cisco_nxos_show_interface.raw` |

The expected values in `expect.yml` were written by reading these files.
