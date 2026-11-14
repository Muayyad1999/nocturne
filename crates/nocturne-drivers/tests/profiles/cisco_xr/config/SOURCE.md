# Running configurations

`edge-rtr.txt` and `edge-rtr.again.txt` were **written by hand** in the IOS XR
configuration format. No IOS XR running configuration was recorded for this
repository, and the ntc-templates corpus this profile's captures come from
holds `show` output only, no configurations — so unlike the captures under
`captures/`, nothing here is a copy of real device output. That is also why
`cisco_xr.yaml` says `maturity: community`.

What the pair is for: the two files are the same configuration fetched twice
from a device nobody touched in between. The only line that differs is the one
IOS XR regenerates on every command — the date and time it prints in front of
the output (`Thu Jan 16 13:30:53.501 AEST` the first time,
`Thu Jan 16 14:02:11.088 AEST` the second) — so the two must come out identical
once `config.ignore` has done its work. The file also carries the other volatile
lines that `config.ignore` drops (`Building configuration...`,
`!! IOS XR Configuration 7.3.2`, `!! Last configuration change at …`), which is
what proves those patterns are not dead. The
first file also has to parse as an `ios`-style tree, survive
parse → render → parse, and need no commands to be turned into itself.
