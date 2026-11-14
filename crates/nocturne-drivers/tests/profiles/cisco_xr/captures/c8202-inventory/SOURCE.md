# c8202-inventory

The chassis inventory of an 8202-32FH-M: rack, route processor, five optics, a
fan controller, four fans and two power modules. Note the two non-Cisco optics,
whose `VID` is `01` rather than `V01`, and the fan controller whose part number
carries a bracketed suffix (`8202-32FH-M[FB]`) — the profile passes part
numbers through as the router prints them.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_inventory.txt` | `tests/cisco_xr/show_inventory/cisco_xr_show_inventory_01.raw` |

The expected values in `expect.yml` were written by reading these files.
