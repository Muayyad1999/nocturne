# arp-ios

An ARP table, including an entry without an interface.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_ip_arp.txt` | `tests/cisco_ios/show_ip_arp/cisco_ios_show_ip_arp.raw` |

The expected values in `expect.yml` were written by reading these files.
