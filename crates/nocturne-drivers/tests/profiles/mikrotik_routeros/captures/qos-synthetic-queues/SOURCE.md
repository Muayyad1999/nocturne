# qos-synthetic-queues

Simple queues (a subnet, a dynamic queue made by a DHCP lease, a queue on an interface, a
disabled one) and a small queue tree (a parent on `ether1` with two children, and a disabled
queue).

**Both files are `origin: synthetic`: written by hand, not printed by any device and not
copied from any page.** Nothing here was recorded.

* `queue_simple_print_stats_detail_without-paging.txt` — MikroTik's documentation prints no
  simple-queue listing with counters. The property names are those of the "Statistics" list on
  the page "Queues" (<https://help.mikrotik.com/docs/spaces/ROS/pages/328088/Queues>):
  `rate`, `packet-rate`, `queued-bytes`, `queued-packets`, `bytes`, `packets`, `dropped`, each
  described there as a pair (`read-only/read-only`, upload/download), and the `total-`
  counterparts the page lists after them. The layout — index, flag letters, `name="…"`,
  `target=…`, long items wrapped onto an indented second line — is that of the `/queue simple
  print` samples on the page "DHCP"
  (<https://help.mikrotik.com/docs/spaces/ROS/pages/24805500/DHCP>), from which the dynamic
  queue's name and target are also taken. Those samples list one queue each; the blank line
  between items is how the recorded `print detail` listings of this profile separate theirs
  (`hex-v6-interfaces`). **Assumed, not documented:** that the statistics view
  prints `target`, the order of the properties, and the `bps` suffix on `rate`. The profile
  reads `name`, `target`, `packets`, `bytes`, `dropped` and `queued-packets`.
* `queue_tree_print_stats_detail_without-paging.txt` — written in the layout of the one
  documented sample (`qos-doc-queue-tree`), with more than one queue so that parents, children
  and a disabled queue are covered.

All names, addresses and counters are invented for this test.

The expected values in `expect.yml` were written by reading these files.
