# mac-table-c3850

The MAC address table of a Catalyst 3850 in today's layout, including the switch's own `CPU` entries.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_mac_address-table.txt` | `tests/cisco_ios/show_mac-address-table/cisco_ios_show_mac-address-table6.raw` |

The expected values in `expect.yml` were written by reading these files.
