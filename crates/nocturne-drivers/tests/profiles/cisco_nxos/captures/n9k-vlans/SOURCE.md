# n9k-vlans

`show vlan` of a two-module Nexus. The point of the case is the port list of
VLAN 1, which wraps over **eighteen** lines: every one of its 57 ports has to
end up in the row, in order, with the short names (`Eth1/2`) expanded to the
long ones (`Ethernet1/2`) so that they join the names `show interface` prints.

`Eth1/33` is missing from the list — the capture really does skip it — and
four VLANs have no ports at all, so an empty port list must stay empty.

NX-OS prints two more tables after the port list (the VLAN types and the
remote-SPAN VLANs); neither adds rows.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_vlan.txt` | `tests/cisco_nxos/show_vlan/cisco_nxos_show_vlan.raw` |

The expected values in `expect.yml` were written by reading these files: the
port list was transcribed from the capture's Ports column, not from the
parser's output.
