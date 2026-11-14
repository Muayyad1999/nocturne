# Running configurations

`edge-rtr.txt` and `edge-rtr.again.txt` were **written by hand** in the form
`show configuration | display set` prints. No Junos configuration was recorded
for this repository, and the ntc-templates corpus this profile's captures come
from holds `show` output only — so unlike the files under `captures/`, nothing
here is a copy of real device output. That is part of why
`juniper_junos.yaml` says `maturity: community`.

## Why `| display set` and not the curly-brace form

`show configuration` on its own prints Junos's native form:

```
system {
    host-name edge-rtr;
    services {
        ssh;
    }
}
```

Nothing in this repository parses that. `hier.rs` has four grammars — indented
blocks negated with `no` (IOS), indented blocks negated with `undo` (VRP), flat
`set` lines removed with `delete` (`Syntax::Set`), and one self-contained
command per line (RouterOS) — and a brace-delimited tree is none of them. Fed
to the IOS grammar it would parse by indentation, treat `}` as a configuration
line, and remediate by sending `no }`.

So `config.running` asks for `show configuration | display set`, which is the
same configuration as flat lines:

```
set system host-name edge-rtr
set system services ssh
```

That is exactly `Syntax::Set`, and a difference between two of them comes out
as `delete …` and `set …` commands that Junos accepts verbatim. The cost is
that the fetched text is not the form an engineer reads on the device; the
alternative was declaring a grammar that would mis-diff, which the brief ruled
out.

## What the pair is for

The two files are the same configuration fetched twice from a device nobody
touched in between. They differ in the two lines Junos regenerates:

* `## Last commit: …`, which carries the time of the fetch's last commit, and
* the `{master:0}` mastership line, which says which routing engine answered —
  it becomes `{master:1}` after a switchover, with no configuration change at
  all.

Both are in `config.ignore`, so the two must come out identical. The first file
also has to survive parse → render → parse and need no commands to be turned
into itself.
