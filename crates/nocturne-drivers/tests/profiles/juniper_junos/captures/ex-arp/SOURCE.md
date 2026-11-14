# ex-arp

An ARP table of nine entries, all learned on the same IRB (`vlan.20`).

Junos's ARP table has **no age column**, so `age` stays empty — "the device
did not say" rather than a made-up number — and the `Flags` column (`none`
here, `permanent` on a static entry) has nowhere to go in the model's ARP row.
The command is `show arp no-resolve`: without `no-resolve` Junos attempts a
reverse lookup on every entry and prints names in place of the addresses.

The trailing `Total entries: 9` line is a summary, not an entry, and must not
become a tenth row.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_arp_no-resolve.txt` | `tests/juniper_junos/show_arp_no-resolve/juniper_junos_show_arp_no-resolve.raw` |

The expected values in `expect.yml` were written by reading these files.
