# mac-table-unicast-and-multicast

`show mac address-table` of an EOS switch, with both tables EOS prints: the
unicast table (19 entries) and the multicast table (4). It is a deliberately
awkward capture from the corpus:

* the columns are not aligned the same way on every line (`55` has far more
  padding than its neighbours);
* a static entry has no "Moves"/"Last Move" columns at all, a dynamic one has;
* a multicast entry is flooded to **several** ports (`Po135 Po12 po5 eth1/1`),
  including one in lower case and one in another vendor's spelling, while the
  model holds one interface — the profile keeps the first;
* one entry's address, `jenn.3057.8423`, is not twelve hex digits, so MAC
  normalisation must leave it exactly as the device printed it instead of
  inventing an address.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_mac_address-table.txt` | `tests/arista_eos/show_mac_address-table/arista_eos_show_mac_address-table.raw` |

The expected values in `expect.yml` were written by reading these files.
