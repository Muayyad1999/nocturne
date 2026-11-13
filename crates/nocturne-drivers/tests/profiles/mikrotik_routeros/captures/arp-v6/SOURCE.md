# arp-v6

`/ip arp print terse without-paging`: one dynamic complete entry (flags `DC`) and one static
entry with no flags.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `ip_arp_print_terse_without-paging.txt` | `tests/mikrotik_routeros/ip_arp_print_terse_without-paging/mikrotik_routeros_ip_arp_print_terse_without-paging.raw` |

The expected values in `expect.yml` were written by reading these files.
