# Captures of `mikrotik_routeros`

Every directory here holds what one device printed for the commands the profile's collectors
run, what that must be read as (`expect.yml`), and where it came from (`SOURCE.md`).
`expect.yml` says what the files are with `origin:` — `recorded` (the default: a device of
the platform printed it), `documented` (a sample in the vendor's documentation) or `synthetic`
(written by hand). `docs/profiles.md` has the details.

## The queue statistics still need a recording

The profile lists `qos` under `unverified`: its reading of the two queue listings is tested
against the one sample in MikroTik's documentation (`qos-doc-queue-tree`, which is of
`queue tree print stats`) and against hand-written output (`qos-synthetic-*`) — and against
nothing a real device printed, because no such recording exists here or in the ntc-templates
corpus. In particular nobody has seen what a device answers to the exact commands below, nor
a simple-queue listing with counters at all.

If you have a RouterOS device with queues, recording them is read-only and takes a minute:

1. In Nocturne, open a terminal to the device (or use `devices.exec`), and run both

       /queue simple print stats detail without-paging
       /queue tree print stats detail without-paging

   A router with traffic through its queues, a queue with a parent, a queue on an interface,
   a dynamic queue (hotspot, PPP, DHCP) and a disabled one are each worth having; so is the
   same from both RouterOS 6 and RouterOS 7.
2. Save each answer exactly as it came — from the first line after the command to the last
   line before the prompt — as `queue_simple_print_stats_detail_without-paging.txt` and
   `queue_tree_print_stats_detail_without-paging.txt`. A menu with no queues answers with the
   `Flags:` line alone; save that too.

## Anonymising it

The recording must stay what the device printed, so change as little as possible and never
the shape:

* **Do not touch** spacing, indentation, line order, counters, rates or anything else that is
  a number. The parser is being tested on exactly these.
* **Replace names that identify somebody** — queue names, comments, packet marks, interface names that name a customer — with neutral ones, the same replacement
  for the same name everywhere in the case. Keep the kind of name it was (no spaces where
  there were none, a `-` where there was one), because a name with a space or a `<` in it is
  itself something the parser has to cope with.
* **Replace public addresses** with documentation addresses (192.0.2.0/24, 198.51.100.0/24,
  203.0.113.0/24, 2001:db8::/32), keeping the prefix length.
* Nothing in this output is a secret, but look before you commit: a description or a comment
  can hold anything.

## Citing it

Put the files in a directory of their own under `captures/`, one file per command, named after
the command (`docs/profiles.md`, "Testing a profile"). Its `SOURCE.md` says:

* the hardware model and the software release (the line of `/system resource print` that states it),
* the date, and that it was recorded from a device — by whom is not needed,
* the exact command that was typed, if it differs from the profile's,
* everything that was changed while anonymising it, as a list.

Its `expect.yml` is written by reading the files, and has **no** `origin:` line: a recording is
the default. The profile test will then fail, on purpose, until `qos` is taken off
`unverified:` in `profiles/mikrotik_routeros.yaml` — do that in the same change, and correct the
profile if the recording shows it to be wrong. That is the whole point of the recording.
