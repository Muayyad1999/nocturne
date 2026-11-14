# arp-two-vrfs

`show ip arp` of an EOS switch with two VRFs. The awkward parts:

* the Age column is a clock (`0:00:00`) in one VRF, a bare number in the
  other, and `N/A` for an entry whose age EOS does not know — all three are
  kept as the device printed them, because `age` is free text in the model;
* two entries name **two** interfaces (`Vlan101, Port-Channel2`): the routed
  interface and the port the address was learnt on. The model holds one, so
  the profile keeps the routed interface;
* one entry says `Vlan1, not learned`, which is a note and not a second
  interface — it is cut off at the comma the same way;
* the VRF each entry belongs to is parsed but **dropped**: `ArpEntry` has no
  VRF field (see NOTES.md).

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_ip_arp.txt` | `tests/arista_eos/show_ip_arp/arista_eos_show_ip_arp1.raw` |

The expected values in `expect.yml` were written by reading these files.
