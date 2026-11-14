# ncs-lldp

LLDP neighbours of an 8000-series router: a Catalyst 9000, two other XR
routers and two Nexus 7700s. Worth testing because the neighbours identify
their ports three different ways — one advertises a port id that is its own
short name (`Fou1/0/22`) while the local port is `FourHundredGigE0/0/0/0`, two
advertise the full name, and the Nexus pair advertise `Eth1/4` / `Eth2/4`. The
port *description* is a sentence about the link
(`uplink:router1:FH0/0/0/0`), not a port name, so the profile only falls back
to it when there is no port id. Two neighbours advertise the same management
address (172.1.1.158).

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_lldp_neighbors_detail.txt` | `tests/cisco_xr/show_lldp_neighbors_detail/cisco_xr_show_lldp_neighbors_detail_01.raw` |

The expected values in `expect.yml` were written by reading these files.
