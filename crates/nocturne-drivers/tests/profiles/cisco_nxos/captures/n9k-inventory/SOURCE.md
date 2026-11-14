# n9k-inventory

`show inventory` of a Nexus 9396PX. Fourteen parts: the chassis, two line-card
slots, two power supplies, three fans, three slots of a *different* chassis
(the capture is a composite from the corpus and mixes in Nexus 7009 parts) and
three transceivers.

What the case is for:

* NX-OS quotes the name and description of a chassis part
  (`NAME: "Slot 1",  DESCR: "1/10G SFP+ Ethernet Module"`) but **not** those
  of a transceiver (`NAME: Ethernet1/46,  DESCR: CISCO-AVAGO`), and both
  layouts have to produce a row;
* the spacing between the `PID`, `VID` and `SN` columns is not the same on
  every line;
* the fans report the serial number `N/A`, which is kept as printed rather
  than turned into an empty field — the device did answer.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_inventory.txt` | `tests/cisco_nxos/show_inventory/cisco_nxos_show_inventory1.raw` |

The expected values in `expect.yml` were written by reading these files.
