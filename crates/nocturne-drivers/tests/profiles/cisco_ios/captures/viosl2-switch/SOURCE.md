# viosl2-switch

A virtual IOS layer-2 switch (vIOS-L2). The two captures are separate corpus files that
describe the same set of ports (`Gi0/1`–`Gi0/3`, `Gi1/0`). The `show interfaces` capture ends with
one extra interface, a second `GigabitEthernet0/2`, pasted in from an ASR 1001: upstream uses it to
test a different duplex line. It is kept, since two rows with one name are worth testing.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_interfaces.txt` | `tests/cisco_ios/show_interfaces/cisco_ios_show_interfaces.raw` |
| `show_interfaces_switchport.txt` | `tests/cisco_ios/show_interfaces_switchport/cisco_ios_show_interfaces_switchport.raw` |

The expected values in `expect.yml` were written by reading these files.
