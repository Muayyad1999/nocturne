# asr9k-arp

An ARP table collected from two line cards: the output repeats its header under
a `0/0/CPU0` and a `0/RP0/CPU0` banner, and both halves have to end up in one
table. Two entries are the router's own addresses (`State: Interface`,
`Age: -`); they are kept, because an interface address is a real ARP entry and
the age column says plainly that it does not expire.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_arp.txt` | `tests/cisco_xr/show_arp/cisco_xr_show_arp2.raw` |

The expected values in `expect.yml` were written by reading these files.
