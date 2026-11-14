# procurve-arp

The ARP table. ProVision's has no age column at all, so `age` stays empty —
"the switch did not say" rather than a made-up number — and its `Type` column
(dynamic/static) has nowhere to go in the model's ARP row. The MAC addresses
are again in ProVision's two-group form.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_arp.txt` | `tests/hp_procurve/show_arp/hp_procurve_show_arp.raw` |

The expected values in `expect.yml` were written by reading these files.
