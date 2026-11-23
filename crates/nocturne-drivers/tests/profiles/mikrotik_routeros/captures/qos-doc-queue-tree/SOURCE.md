# qos-doc-queue-tree

One queue of the queue tree, attached to `global`, with its counters.

`queue_tree_print_stats_detail_without-paging.txt` is a sample printed in MikroTik's
documentation, not a recording from a device: *RouterOS documentation*, page "Queues", section
"Queue Tree", "Configuration example", <https://help.mikrotik.com/docs/spaces/ROS/pages/328088/Queues>
(fetched 2026-11-22). It is the listing the page introduces with "Check Queue tree stats to be
sure traffic is matched", under the line

    [admin@MikroTik] > queue tree print stats

and the file holds what the page shows beneath that line, byte for byte.

**The page typed `print stats`; the profile types `print stats detail without-paging`.** The
two extra words are documented on the page "Command Line Interface"
(<https://help.mikrotik.com/docs/spaces/ROS/pages/328134/Command+Line+Interface>: `detail` "forces the print
command to use property=value output form", `without-paging` "prints the output without
stopping after each screenful"), and the sample is already in property=value form, so the
profile expects the same listing — but no page shows the output of the longer command, and
nobody has run it on a device for this test.

Hence `origin: documented` in `expect.yml`: the sample shows the format MikroTik describes, but
the page does not say which release printed it and it has been through an editor. It does
**not** count as a capture of real device output, and `qos` stays on the profile's `unverified`
list until one exists (`../SOURCE.md` says how to record one).

**There is no simple-queue listing in this case**, because the documentation prints none with
counters. The profile reads both menus, so the collection reports that half as missing; that
is the `partial` problem `expect.yml` expects.

The expected values in `expect.yml` were written by reading the file.
