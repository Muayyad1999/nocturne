# neighbors-cdp-only

A switch that was asked for its CDP neighbours only: LLDP is not recorded, as on a device where it is switched off.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_cdp_neighbors_detail.txt` | `tests/cisco_ios/show_cdp_neighbors_detail/cisco_ios_show_cdp_neighbors_detail2.raw` |

The expected values in `expect.yml` were written by reading these files.
