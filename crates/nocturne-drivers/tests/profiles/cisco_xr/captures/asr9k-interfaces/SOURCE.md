# asr9k-interfaces

An ASR 9000 with an RSP pair: a loopback, a null interface, an MPLS-TE tunnel,
two management ports (one administratively down), a 40G and two 10G ports, a
bundle and an 802.1Q sub-interface.

Three things here are the point of the case:

* the bundled template reads `<name> is <X>, line protocol is <Y>` into
  `LINK_STATUS` and `ADMIN_STATE` **in that order**, so `ADMIN_STATE` holds the
  line-protocol state and `LINK_STATUS` the administrative one. The names are
  the wrong way round and the profile's mapping has to be read accordingly;
* `MgmtEth0/RSP0/CPU0/1` is administratively down and IOS XR prints
  `Duplex unknown, 0Kb/s` for it. Zero is not a speed, so `speed_bps` is left
  out and `duplex` comes out empty;
* interfaces with no address print "Internet address is Unknown", which must
  not become an address. `TenGigE0/3/0/0` keeps its name exactly as the router
  spells it — the profile sets `interface_names: verbatim` so the Cisco
  abbreviation rules cannot rewrite it to `TenGigabitEthernet0/3/0/0`, which
  IOS XR would not accept.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_interfaces.txt` | `tests/cisco_xr/show_interfaces/cisco_xr_show_interfaces.raw` |

The expected values in `expect.yml` were written by reading these files.
