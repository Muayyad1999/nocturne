# vlans-v7

`/interface vlan print detail` of a RouterOS 7 aggregation router: eight VLAN interfaces, two
pairs of which carry the same tag on different parents (vlan11 on bond1 and on vlan975,
vlan60 on sfp-sfpplus1 and on bond1).

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `interface_vlan_print_detail.txt` | `tests/mikrotik_routeros/interface_vlan_print_detail/mikrotik_routeros_interface_vlan_print_detail_01.raw` |

The expected values in `expect.yml` were written by reading these files.
