# mx240-version

`show version` from an MX240, for `facts`.

Two things the case is here to pin down. There is **no uptime** in Junos's
`show version` — it is in `show system uptime` — so `facts.uptime_s` stays
empty rather than being filled with a guess. And the output ends with
`{master:0}`, the routing-engine mastership line Junos prints after every
command on a dual-engine chassis; it is not output, it is not a prompt, and it
has to survive being parsed.

The box is a QFabric member, so it answers with a `QFabric System ID` as well
as a serial number; on everything else that field is empty and the profile
leaves `extra.qfabric_system_id` out.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_version.txt` | `tests/juniper_junos/show_version/juniper_junos_show_version.raw` |

The expected values in `expect.yml` were written by reading these files.
