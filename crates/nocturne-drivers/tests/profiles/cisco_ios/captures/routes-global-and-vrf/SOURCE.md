# routes-global-and-vrf

The global routing table and a VRF table. **The two captures come from two different
routers.** The global one is upstream's collection of every layout `show ip route` has had over the
years (routes of several devices pasted together); the VRF one is recorded with `show ip route vrf TRI`,
whose output has the same shape as one table of `show ip route vrf *`.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_ip_route.txt` | `tests/cisco_ios/show_ip_route/cisco_ios_show_ip_route.raw` |
| `show_ip_route_vrf.txt` | `tests/cisco_ios/show_ip_route/cisco_ios_show_ip_route_vrf.raw` |

The expected values in `expect.yml` were written by reading these files.
