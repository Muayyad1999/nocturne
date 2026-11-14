# procurve-mac-table

The port address table. Two things worth a test: ProVision writes a MAC
address as two six-digit groups (`500000-260000`), which has to be normalised
to the colon form the model uses; and the same address appears twice, on the
same link aggregation (`Trk1`) in two different VLANs, so neither the address
nor the port can be treated as a key.

The listing has no "learned or configured" column, so `kind` is left empty.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_mac-address.txt` | `tests/hp_procurve/show_mac-address/hp_procurve_show_mac-address.raw` |

The expected values in `expect.yml` were written by reading these files.
