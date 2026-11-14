# Running configurations

`leaf-switch.txt` is **written by hand**. The ntc-templates corpus has no
`show running-config` of a Nexus, and a configuration taken from a production
switch cannot be published here.

It follows the layout NX-OS gives the running configuration: the three
generated header lines (`!Command:`, `!Running configuration last done at:`
and `!Time:`, with no space after the `!`), then a blank line, then the
configuration with **two-space** indentation for a section's lines and four
for a section inside one (`router bgp` / `address-family` and
`router bgp` / `neighbor` / `address-family`), NX-OS's own spellings
(`feature …`, `vdc … id 1`, `vlan 1,10,20,99` before the per-VLAN sections,
`interface port-channel1` in lower case, `vrf member management`,
`no shutdown` written out per interface, an address written `10.0.10.1/24`),
and **no** `end` line, which NX-OS does not print. The hostname, addresses,
community string and the password hash are made up.

`leaf-switch.again.txt` is the same configuration fetched again the next
morning: the `!Time:` stamp has moved on while
`!Running configuration last done at:` has not — because nobody changed
anything — and a console line printed an NX-OS log message, complete with its
`%$ VDC-1 %$` marker, into the middle of the listing. After `config.ignore`
the two must be identical, or Nocturne would report a switch nobody touched
as changed.
