# n9k-mac-table

`show mac address-table` of a Nexus with three entries, two of which are not
hosts behind a port at all — which is the point of the case:

* `sup-eth1(R)` is the supervisor's inband port. The entry is flagged `G` in
  the first column, which the capture's own legend reads as "Gateway MAC":
  it is the switch's own address;
* `Drop` is a blackhole entry, not an interface;
* only `Po10` is a port with a host behind it, and `Po10` has to be expanded
  to `Port-channel10`.

So three entries are parsed and the profile's `when` keeps one. The columns in
this capture are also aligned differently on every line, and the first column
holds a flag rather than a VLAN id.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_mac_address-table.txt` | `tests/cisco_nxos/show_mac_address-table/cisco_nxos_show_mac_address-table.raw` |

The expected values in `expect.yml` were written by reading these files.
