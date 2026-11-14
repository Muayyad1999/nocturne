# routes-bgp-and-connected

`show ip route` of an EOS router: eBGP routes with a distance and metric, and
connected routes. Two things make it worth a case of its own:

* EOS's protocol code can be **two words** (`B E` for eBGP, `B I` for iBGP,
  `A O` for an OSPF summary). The mapping in the profile uses exactly the
  codes this capture prints in its own legend above the table;
* a connected route prints the word `connected` where a next hop would be, so
  `next_hop` has to come out empty rather than holding the word.

The table has no `VRF:` heading, so `vrf` is left empty on every row.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_ip_route.txt` | `tests/arista_eos/show_ip_route/arista_eos_show_ip_route.raw` |

The expected values in `expect.yml` were written by reading these files.
