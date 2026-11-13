# routes-v6

`/ip route print detail` of a RouterOS 6 router: connected, static and blackhole routes, two
routing marks, gateways reached directly, `via` an interface and recursively, and one route
with a comment.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `ip_route_print_detail.txt` | `tests/mikrotik_routeros/ip_route_print_detail/mikrotik_routeros_ip_route_print_detail.raw` |

The expected values in `expect.yml` were written by reading these files.
