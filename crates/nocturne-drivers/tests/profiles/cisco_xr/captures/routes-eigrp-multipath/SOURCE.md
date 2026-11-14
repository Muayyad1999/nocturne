# routes-eigrp-multipath

A routing table with a static default route, two BGP routes and one EIGRP
external prefix reached over four equal-cost paths, each of which has to become
a row of its own. The protocol column is the point of the case: `S*` carries
the `*` of a candidate default, which the profile strips before looking the code
up, and `D EX` is a two-letter code with a space in it.

The capture also shows IOS XR's habit of printing the date and time
(`Tue May 22 22:32:30.765 UTC`) in front of a command's output.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_route.txt` | `tests/cisco_xr/show_ip_route/cisco_xr_show_ip_route2.raw` |

The file is named after the command the profile runs, `show route`, which is
what IOS XR calls it; the corpus files it under the `show_ip_route` directory
because the bundled template's index entry accepts both spellings. The contents
are copied unchanged.

The expected values in `expect.yml` were written by reading these files.
