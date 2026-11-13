# hex-v6-interfaces

`/interface print detail` of a RouterOS 6 board: enabled, disabled (X), running (R), slave (S)
and dynamic (D) ports, a tunnel with no MAC address, and two comments — one of which runs over
two lines.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `interface_print_detail.txt` | `tests/mikrotik_routeros/interface_print_detail/mikrotik_routeros_interface_print_detail.raw` |

The expected values in `expect.yml` were written by reading these files.
