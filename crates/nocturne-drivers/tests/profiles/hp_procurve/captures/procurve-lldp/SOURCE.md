# procurve-lldp

LLDP neighbours of a modular chassis (ports `A2`, `A8`, `A9`, `A17`, `D13`):
three Aruba access points, an Aruba switch and a Cisco access point.

The case is here for the port identifiers. ProVision says outright what kind
each one is, and all four kinds are in this capture:

* `A2` — `PortType: mac-address`, and the neighbour sent no port description
  and no system name either, so there is no port name to report and the
  chassis id has to stand in for the device name. `remote_interface` is
  empty, which is the honest answer;
* `A8` and `D13` — `PortType: mac-address` with a port description, so the
  description (`bond0`, `eth0`) is the port name and the port id, a MAC
  address written out with spaces, is not;
* `A9` — `PortType: interface-name`, so the port id (`1/1/12`) *is* the name;
* `A17` — `PortType: local`, where the port id is a local name (`emac0`) and
  the chassis id is a truncated location string (`AC_AP_GenericLoc...`) that
  is not a MAC address and must not be normalised into one.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_lldp_info_remote-device_detail.txt` | `tests/hp_procurve/show_lldp_info_remote-device_detail/hp_procurve_show_lldp_info_remote-device_detail2.raw` |

The command the profile runs, `show lldp info remote-device detail`, is the
one the corpus names its directory after; it has not been checked against a
switch (see `../NOTES.md`). The file's contents are copied unchanged.

The expected values in `expect.yml` were written by reading these files.
