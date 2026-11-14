# n9k-version

`show version` of a Nexus 93180YC-FX3 running NX-OS 10.4(5). It is the only
command the profile reads both `facts` and `health` from, so it is run once
and its output serves both.

Two things the case pins down:

* NX-OS writes the release type into the version itself
  ("NXOS: version 10.4(5) [Maintenance Release]"); the bracket is a note, not
  part of the version, so `os_version` is cut back to `10.4(5)`;
* the memory size ("… with 32802700 kB of memory.") is in the output but not
  in the bundled template, so the profile reads it with a small regex of its
  own and turns the kilobytes into bytes.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_version.txt` | `tests/cisco_nxos/show_version/cisco_nxos_show_version5.raw` |

The expected values in `expect.yml` were written by reading these files.
