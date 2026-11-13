# Running configurations

`ccr-core-v7.txt` is **hand-written**. The ntc-templates corpus has no `/export` of a whole
RouterOS device (only `/ip address export verbose` of one menu), and a configuration taken from
a production router cannot be published here.

It follows the layout of the real export in the corpus,
`tests/mikrotik_routeros/ip_address_export_verbose/mikrotik_routeros_ip_address_export_verbose.raw`
(Apache-2.0): the five header comment lines RouterOS prints, then one command per line. The
difference is that `terse` — which is what the profile asks for, and what the RouterOS diff
needs — repeats the menu path on every line instead of printing it once above a group of
`add`/`set` lines.

The interfaces, VLAN tags, addresses, networks and comments are those of the device in
`captures/ccr-core-v7/`, so the configuration and the collected state describe the same router.

`ccr-core-v7.again.txt` is the same configuration fetched again a few hours later: the
timestamp in the header has moved on, and the console echoed a log line into the middle of the
output. After `config.ignore` the two must be identical, or Nocturne would report a device
that nobody touched as changed.
