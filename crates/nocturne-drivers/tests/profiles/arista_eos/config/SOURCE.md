# Running configurations

`leaf-switch.txt` is **written by hand**. The ntc-templates corpus has no
`show running-config` of an Arista device, and a configuration taken from a
production switch cannot be published here.

It follows the layout EOS gives the running configuration: the two generated
comment lines (`! Command:` and `! device:`, the second carrying the model and
the running EOS version), `!` separators between sections, **three-space**
indentation for a section's lines and six for a section inside one
(`management ssh` / `vrf MGMT` / `no shutdown`), EOS's own spellings
(`Port-Channel1`, `switchport mode trunk`, an address written `10.0.255.1/32`
rather than with a netmask, `service routing protocols model multi-agent`) and
`end` at the bottom. The hostname, addresses, community string and the
password hash are made up.

`leaf-switch.again.txt` is the same configuration as a second fetch would show
it after a software upgrade: the EOS version in the `! device:` line has moved
from 4.26.1F to 4.28.3M, and a console line printed a log message into the
middle of the listing. After `config.ignore` the two must be identical, or
Nocturne would report a switch nobody touched as changed.
