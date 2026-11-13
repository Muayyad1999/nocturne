# c3750-stack-l3

`show interfaces` of a Catalyst 3750 stack (older output: `BW 100000 Kbit`, counters far beyond
32 bits), enriched with `show ip interface` and `show ipv6 interface brief`.

**The three captures come from three different devices.** The corpus has no device recorded
with all three commands; they are combined here because they share interface names (`Vlan1`,
`Vlan100`), which is what the enrichment steps merge on. Read the result as a test of parsing and
merging, not as a description of one real switch.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_interfaces.txt` | `tests/cisco_ios/show_interfaces/cisco_ios_show_interfaces2.raw` |
| `show_ip_interface.txt` | `tests/cisco_ios/show_ip_interface/cisco_ios_show_ip_interface2.raw` |
| `show_ipv6_interface_brief.txt` | `tests/cisco_ios/show_ipv6_interface_brief/cisco_ios_show_ipv6_interface_brief.raw` |

The expected values in `expect.yml` were written by reading these files.
