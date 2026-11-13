# c8000v-inventory

The inventory of a virtual Catalyst 8000V.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_inventory.txt` | `tests/cisco_ios/show_inventory/cisco_ios_show_inventory_c8000v.raw` |

The expected values in `expect.yml` were written by reading these files.
