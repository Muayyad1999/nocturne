# chr-x86-v7

RouterOS 7.12.2 on CHR (x86_64, no RouterBOARD). `/system routerboard print` is not recorded
here: the step is optional and a machine without a board has nothing to add, which is what the
expectations check (no serial number, and no problem reported).

Note the capture's first line is indented with tab characters, as the device printed it.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `system_resource_print.txt` | `tests/mikrotik_routeros/system_resource_print/mikrotik_routeros_system_resource_print_3.raw` |
| `system_identity_print.txt` | `tests/mikrotik_routeros/system_identity_print/mikrotik_routeros_system_identity_print.raw` |

The expected values in `expect.yml` were written by reading these files.
