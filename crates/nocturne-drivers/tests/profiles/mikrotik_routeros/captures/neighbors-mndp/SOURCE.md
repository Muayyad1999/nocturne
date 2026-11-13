# neighbors-mndp

`/ip neighbor print detail` with three neighbours: RouterOS 6 and RouterOS 7 peers, one with
IPv4 and IPv6 addresses, one with no address at all, one whose `system-caps-enabled` property
wrapped onto the next line, and interface lists of the form `ether10,bridge_255_mgmt`.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `ip_neighbor_print_detail.txt` | `tests/mikrotik_routeros/ip_neighbor_print_detail/mikrotik_routeros_ip_neighbor_print_detail_line_wrap.raw` |

The expected values in `expect.yml` were written by reading these files.
