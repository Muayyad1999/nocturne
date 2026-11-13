# neighbors-cdp-lldp

CDP and LLDP neighbours. **The two captures come from two different switches**; they are
combined to test that both protocols end up in one table. The LLDP neighbours are a Juniper EX, an
HPE switch and an Aruba access point, which identify their ports in three different ways.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_cdp_neighbors_detail.txt` | `tests/cisco_ios/show_cdp_neighbors_detail/cisco_ios_show_cdp_neighbors_detail.raw` |
| `show_lldp_neighbors_detail.txt` | `tests/cisco_ios/show_lldp_neighbors_detail/cisco_ios_show_lldp_neighbors_detail1.raw` |

The expected values in `expect.yml` were written by reading these files.
