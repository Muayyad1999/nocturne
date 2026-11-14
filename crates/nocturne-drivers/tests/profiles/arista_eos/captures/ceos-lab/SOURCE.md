# ceos-lab

`show version` of a cEOS lab switch: the container image, so the hardware
version and the model are the lab image's, not a chassis's. The case is here
for two awkward things: the `Hardware version:` line is **empty**, and the
memory figures are printed in kilobytes while the template captures the digits
only, so the profile has to multiply them itself.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_version.txt` | `tests/arista_eos/show_version/arista_eos_show_version_ceos.raw` |

The expected values in `expect.yml` were written by reading these files.
