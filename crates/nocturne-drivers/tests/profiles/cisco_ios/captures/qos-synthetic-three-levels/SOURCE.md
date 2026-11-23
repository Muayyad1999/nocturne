# qos-synthetic-three-levels

A policy three levels deep whose inner policies are *followed* by more classes of the outer
ones, then a second interface with a policy in each direction, one of them a policer that drops
whatever it sees (`conform-action drop exceed-action drop`).

**`show_policy-map_interface.txt` is `origin: synthetic`: written by hand.** No device and no
simulator printed it, and it is not copied from any page. The lines are those of the samples
in Cisco's command reference that the documented cases beside this one hold (the layout since
hierarchical queueing: classes 4 columns in, a nested `Service-policy : NAME` and its classes 4
further each time); the names and every number are invented.

It exists because neither the documentation nor the simulator's own samples show the
things a reader of this output gets wrong first: coming back out of a nested policy to the
classes after it, a third level, and a match-any class whose match statements each repeat
`N packets, M bytes` with numbers that differ from the class's own (the documented samples
only have the case where they are equal).

The expected values in `expect.yml` were written by reading the file.
