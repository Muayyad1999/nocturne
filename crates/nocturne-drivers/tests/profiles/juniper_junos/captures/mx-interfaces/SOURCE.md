# mx-interfaces

`show interfaces` from an MX: one Ethernet port with a tagged logical unit on
it, and seven tunnel and services interfaces (GRE, IP-over-IP, link services,
logical tunnel, multicast GRE, adaptive services).

This is the case that earns the profile its second collector step. The bundled
template captures the name, the state, the description, the link-level type,
the MTU and the address — but **not** the speed, the MAC address, the duplex or
the rates, all four of which this output prints. The profile reads those with
an inline TextFSM template over the same command, so one capture exercises both
steps and the merge between them.

The awkward parts:

* `MTU: Unlimited` on the five tunnel interfaces. That is not a number, so
  `mtu` is left out rather than set to 0;
* `ge-0/0/0.2036`, the logical unit, has no state of its own in this output —
  the template leaves `admin_state` empty — so `admin_up` is `null`, "the
  device did not say", not `false`;
* its address is printed as two separate values,
  `Destination: 10.10.199.176/29, Local: 10.10.199.180`. The address of the
  interface is `10.10.199.180/29`: the prefix length belongs to the local
  address, and neither field is the answer on its own;
* `Link-mode: Half-duplex` — a real half-duplex gigabit port, which is
  unusual enough to be worth keeping as a test;
* `sp-0/0/0` prints no `Input rate` line at all, so its rate counters stay
  empty instead of reading 0;
* the physical interface's lines contain `BPDU Error: None` and
  `MAC-REWRITE Error: None`. Those are counters. They are why `cli.errors` is
  case-sensitive, and they appear again in `../errors.yml` as `clean` input.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_interfaces.txt` | `tests/juniper_junos/show_interfaces/juniper_junos_show_interfaces.raw` |

The expected values in `expect.yml` were written by reading these files.
