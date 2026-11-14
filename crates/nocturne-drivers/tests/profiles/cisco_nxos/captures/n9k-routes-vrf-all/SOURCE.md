# n9k-routes-vrf-all

`show ip route vrf all` of a Nexus with two VRFs. Unlike IOS, NX-OS accepts
`vrf all` on every release and includes the default VRF in the answer, so the
profile reads every routing table in one step.

What the case is for:

* a prefix with **two** next hops (`44.55.0.11/32` via a static route and via
  iBGP) becomes two rows that share a prefix and differ in next hop, distance
  and protocol;
* NX-OS names the protocol with its instance attached (`bgp-2244`,
  `bgp-64512`), which the profile cuts back to `bgp`;
* a route out of `Null0` has an interface and **no** next-hop address, while a
  recursive static route has a next-hop address and **no** interface. Neither
  field is filled in for the other;
* the VRF each row belongs to carries down from the `IP Route Table for VRF
  "…"` heading.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_ip_route_vrf_all.txt` | `tests/cisco_nxos/show_ip_route/cisco_nxos_show_ip_route_vrf4.raw` |

The expected values in `expect.yml` were written by reading these files.
