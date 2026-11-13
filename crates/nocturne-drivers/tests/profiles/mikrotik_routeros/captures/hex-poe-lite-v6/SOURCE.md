# hex-poe-lite-v6

A RouterOS 6 board: hEX PoE lite (RouterBOARD 750UP r2), RouterOS 6.48.6 long-term.
`facts`, `health` and the RouterBOARD `inventory` come from these three property sheets.
The `/system resource print` and `/system routerboard print` captures agree on the board name,
so they were recorded on the same kind of device; the identity capture is the corpus's only one.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `system_resource_print.txt` | `tests/mikrotik_routeros/system_resource_print/mikrotik_routeros_system_resource_print_1.raw` |
| `system_identity_print.txt` | `tests/mikrotik_routeros/system_identity_print/mikrotik_routeros_system_identity_print.raw` |
| `system_routerboard_print.txt` | `tests/mikrotik_routeros/system_routerboard_print/mikrotik_routeros_system_routerboard_print3.raw` |

The expected values in `expect.yml` were written by reading these files.
