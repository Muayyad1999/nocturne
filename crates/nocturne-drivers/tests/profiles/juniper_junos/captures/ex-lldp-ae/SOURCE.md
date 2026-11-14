# ex-lldp-ae

LLDP neighbours where all four links go to **one** neighbour — the four members
of an aggregate (`ae7`) landing on four ports of the same chassis. Every row
has the same `Chassis Id` and the same `System Name`, and only the local and
remote port names differ, so neither the device name nor the chassis id can be
treated as a key.

The listing is also the limit of what this command gives: there is no
management address, no capability list and no platform string in it, so the
profile leaves those three fields empty rather than inventing them.
`show lldp neighbors interface <name>` has them, one interface at a time, and
there is no recorded output of that command to check a step against.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_lldp_neighbors.txt` | `tests/juniper_junos/show_lldp_neighbors/juniper_junos_show_lldp_neighbors_ae7.raw` |

The expected values in `expect.yml` were written by reading these files.
