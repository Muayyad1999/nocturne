# n9k-arp

`show ip arp` of a Nexus, taken across all contexts. 23 entries are printed
and 22 are kept, which is the point of the case:

* the last entry, `10.5.6.10`, has `INCOMPLETE` where a hardware address
  belongs — NX-OS is still resolving it. It is dropped rather than recorded
  with the word INCOMPLETE as its MAC address;
* one entry's age is `-` rather than a clock, and is kept exactly as printed
  because `age` is free text in the model;
* several entries are learnt on sub-interfaces (`Ethernet1/1.2151`), whose
  names must survive canonicalisation with the dot intact;
* `Total number of entries: 22` in the capture's own header agrees with the
  22 rows the profile keeps.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_ip_arp.txt` | `tests/cisco_nxos/show_ip_arp/cisco_nxos_show_ip_arp.raw` |

The expected values in `expect.yml` were written by reading these files.
