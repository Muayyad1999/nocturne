# svi-and-vxlan

`show interfaces` of a cEOS switch that has only an SVI and a VXLAN
interface. Both are awkward on purpose:

* `Vlan888` says "No Internet protocol address assigned", so `ipv4` must come
  out as an empty list and not as a list holding an empty string;
* `Vxlan1` has no MAC address and no MTU line at all, so both fields must be
  left out rather than guessed. Its name is also not an IOS abbreviation, so
  it must survive interface-name canonicalisation unchanged.

**The two captures come from two different devices.** The corpus has no EOS
device recorded with both commands. `show_interfaces_status.txt` is here
because the profile's second interface step runs that command, and every step
has to be exercised by some capture: the real output is parsed and its rows
are matched against the rows the first step made — and since the two devices
share no port name, every one of its rows is dropped, which is what an
enrichment step must do with a row that matches nothing. The `count: 2` below
is that assertion.

It is **not** an assertion about what that step writes. Nothing here checks
the values it maps onto `mode`, `vlan`, `duplex` and `speed_bps`, and a wrong
expression in it would fail silently rather than fail this test. NOTES.md says
exactly how far the step is trusted and why.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_interfaces.txt` | `tests/arista_eos/show_interfaces/arista_eos_show_interfaces3.raw` |
| `show_interfaces_status.txt` | `tests/arista_eos/show_interfaces_status/arista_eos_show_interfaces_status_encap.raw` |

The expected values in `expect.yml` were written by reading these files.
