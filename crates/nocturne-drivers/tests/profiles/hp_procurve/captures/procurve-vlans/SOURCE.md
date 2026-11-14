# procurve-vlans

The VLAN table of a ProVision switch. Note what it does *not* have: a column
of member ports. ProVision lists the ports of one VLAN at a time
(`show vlans <id>`), so the profile leaves `interfaces` empty here rather than
guess, and the `Status` column is how the VLAN is classified
(`Port-based`), not whether it is up.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_vlans.txt` | `tests/hp_procurve/show_vlans/hp_procurve_show_vlans4.raw` |

The expected values in `expect.yml` were written by reading these files.
