# n9k-neighbors

Both neighbour protocols of a Nexus: five CDP neighbours and four LLDP ones,
nine rows in all. The two captures come from devices in the same lab (the CDP
one sees `dc-lf03` and `dc-sp01`/`dc-sp02`, the LLDP one `n9k2` and a 3750),
but they are **two different recordings** and are combined here only because
the profile runs both commands for the `neighbors` kind.

What the case is for:

* CDP on NX-OS puts the serial number in the device id
  (`dc-lf03(SAL19069XXX)`), so the profile prefers the `System Name` line and
  falls back to the device id. The first neighbour, `PERIMETER`, has **no**
  system name at all, so it is the fall-back that is tested;
* the first CDP neighbour is reached over `mgmt0`, whose name must not be
  mangled by canonicalisation, and its remote port is an IOS name
  (`FastEthernet1/0/32`) which belongs to the *other* device and so is kept
  exactly as advertised;
* LLDP advertises no platform, so the profile puts the neighbour's system
  description there. For the 3750 that is a whole IOS version line;
* LLDP's local port ids are short (`Eth1/1`), the remote ones long
  (`Ethernet1/1`): the local one is expanded, the remote one is not.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_cdp_neighbors_detail.txt` | `tests/cisco_nxos/show_cdp_neighbors_detail/cisco_nxos_show_cdp_neighbors_detail.raw` |
| `show_lldp_neighbors_detail.txt` | `tests/cisco_nxos/show_lldp_neighbors_detail/cisco_nxos_show_lldp_neighbors_detail.raw` |

The expected values in `expect.yml` were written by reading these files.
