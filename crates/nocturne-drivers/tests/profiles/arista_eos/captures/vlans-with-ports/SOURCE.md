# vlans-with-ports

The VLAN table of a small EOS switch: an active VLAN with one port, one with
two, and two suspended VLANs with no ports at all — so an empty port list has
to stay empty. The port names are EOS's short form (`Et1`), which
canonicalisation turns into `Ethernet1` so that they join the names
`show interfaces` prints.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_vlan.txt` | `tests/arista_eos/show_vlan/arista_eos_show_vlan.raw` |

The expected values in `expect.yml` were written by reading these files.
