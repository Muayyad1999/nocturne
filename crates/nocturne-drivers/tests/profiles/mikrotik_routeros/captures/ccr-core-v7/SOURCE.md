# ccr-core-v7

One RouterOS 7 core router: 27 interfaces (SFP+ and SFP28 ports, two loopback bridges, a bond,
a PPPoE client and nine VLAN interfaces), its VLAN tags, and its addresses.

**`ip_address_print_detail.txt` and `ipv6_address_print_detail.txt` are `synthetic: true`.**
The corpus has no `/ip address print detail` or `/ipv6 address print detail` capture, so the
two files were written by hand in the `print detail` layout the other captures in this directory
show (` <index> <flags> key=value ...`, a `;;;` comment on its own line above the properties).
The IPv4 addresses, networks, interfaces, comments and the dynamic flag are **copied from a real
capture of the same device** — `tests/mikrotik_routeros/ip_address_print/mikrotik_routeros_ip_address_print_3.raw`,
whose interface names are exactly the ones in `interface_print_detail_01.raw`; only the layout
is hand-written. The IPv6 addresses are documentation addresses (RFC 3849) invented for this
test: RouterOS prints the same `address=`/`interface=`/`actual-interface=` properties there.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `interface_print_detail.txt` | `tests/mikrotik_routeros/interface_print_detail/mikrotik_routeros_interface_print_detail_01.raw` |
| `interface_vlan_print_detail.txt` | `tests/mikrotik_routeros/interface_vlan_print_detail/mikrotik_routeros_interface_vlan_print_detail.raw` |

The expected values in `expect.yml` were written by reading these files.
