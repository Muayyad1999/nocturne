# bridge-hosts

`/interface bridge host print terse without-paging`: eight entries of one bridge — two hosts
learned behind ports, two of the bridge's own addresses (`L`), four invalid or disabled ones,
and one with a VLAN id.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `interface_bridge_host_print_terse_without-paging.txt` | `tests/mikrotik_routeros/interface_bridge_host_print_terse_without-paging/interface_bridge_host_print_terse_without-paging.raw` |

The expected values in `expect.yml` were written by reading these files.
