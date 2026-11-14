# procurve-ports

The two interface listings ProVision has, and what they do and do not say.

**The two files come from two different switches**, and that is the point of
the case. `show interfaces brief` was taken from a 24-port standalone switch,
which numbers its ports `1` to `24`; `show interfaces status` from a stack,
which numbers them `1/1` to `2/52`. No port name appears in both, so **none**
of the 17 rows of the `status` listing merges into a row of the `brief`
listing: the case proves that `merge_on` drops a row that matches nothing
rather than inventing an interface, and the collected table is the 24 ports of
the `brief` listing with no description and no VLAN. On a real switch both
commands name the same port the same way and the two halves do join up; there
is no capture pair from one switch to show that, which is written up in
`../NOTES.md`.

The `brief` listing is what carries the administrative state: port 5 is
`Enabled: No` while every port is `Status: Down`, so `admin_up` and `oper_up`
are genuinely different values. The Mode column is ProVision's one-word speed
and duplex (`1000FDx`, `100FDx`, `10GigFD`), which has to be turned into a
rate before it can be read as one — `1000FDx | bps` on its own would say
1000 bits per second.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_interfaces_brief.txt` | `tests/hp_procurve/show_interfaces_brief/show_interfaces_brief_bcast.raw` |
| `show_interfaces_status.txt` | `tests/hp_procurve/show_interfaces_status/show_interfaces_status3.raw` |

The expected values in `expect.yml` were written by reading these files.
