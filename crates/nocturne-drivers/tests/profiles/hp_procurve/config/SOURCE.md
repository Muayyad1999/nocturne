# Running configurations

`acc-sw-01.txt` and `acc-sw-01.again.txt` were **written by hand** in the
ProVision configuration format. No ProVision configuration was recorded for
this repository, and the ntc-templates corpus this profile's captures come
from holds `show` output only — so unlike the files under `captures/`, nothing
here is a copy of real device output. That is part of why
`hp_procurve.yaml` says `maturity: community`.

What the pair is for: the two files are the same configuration fetched twice
from a switch whose configuration nobody changed in between. They differ only
in lines that are not configuration —

* a console log line (`I 01/02/70 03:04:09 ports: port 24 is now on-line`)
  that ProVision printed into the listing while it was being fetched. This is
  the one that bites in practice: a port coming up during a nightly backup
  would otherwise look like a configuration change every time;
* the `; … Configuration Editor; Created on release #…` comment, which names
  the firmware that wrote the file and changes after an upgrade without any
  configuration changing.

Both are in `config.ignore`, along with the `Running configuration:` header
and the `; Ver #…` feature bitmask, so the two must come out identical.

The first file also has to parse as an `ios`-style tree, survive
parse → render → parse, and need no commands to be turned into itself. Three
things in it are there to make that non-trivial: the three-space indentation
ProVision uses (the renderer uses one space per level, so the round trip has to
go through the tree and not the text), the explicit `exit` that closes each
block, and `interface 24`'s name, `"uplink to core #1"`, which ends in a `#`
and must not be mistaken for a prompt or a comment.
