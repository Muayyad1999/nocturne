# Captures of `cisco_ios`

Every directory here holds what one device printed for the commands the profile's collectors
run, what that must be read as (`expect.yml`), and where it came from (`SOURCE.md`).
`expect.yml` says what the files are with `origin:` — `recorded` (the default: a device of
the platform printed it), `documented` (a sample in the vendor's documentation) or `synthetic`
(written by hand, or printed by Nocturne's simulator). `docs/profiles.md` has the details.

## The QoS read still needs a recording

The profile lists `qos` under `unverified`: its reading of `show policy-map interface` is
tested against samples printed in Cisco's documentation (`qos-doc-*`: the command reference,
and two IOS XE configuration guides), against the output of Nocturne's simulator
(`qos-lab-*`) and against one hand-written case (`qos-synthetic-*`) —
and against nothing a real device printed, because no such recording exists here or in the
ntc-templates corpus.

If you have an IOS or IOS-XE device with a service policy attached, recording it is read-only
and takes a minute:

1. In Nocturne, open a terminal to the device (or use `devices.exec`), and run

       show policy-map interface

   The profile has already sent `terminal length 0`, so the answer comes in one piece. A
   device under load, a policy with a policy inside it, a policer and a class that drops are
   each worth more than an idle interface; so is a platform the samples do not cover (a
   Catalyst switch, an ASR with service instances).
2. Save the answer exactly as it came — from the first line after the command to the last line
   before the prompt, leading spaces included — as `show_policy-map_interface.txt`.

## Anonymising it

The recording must stay what the device printed, so change as little as possible and never
the shape:

* **Do not touch** spacing, indentation, line order, counters, rates or anything else that is
  a number. The parser is being tested on exactly these.
* **Replace names that identify somebody** — policy-map, class-map and access-list names, interface descriptions — with neutral ones, the same replacement
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

* the hardware model and the software release (the line of `show version` that states it),
* the date, and that it was recorded from a device — by whom is not needed,
* the exact command that was typed, if it differs from the profile's,
* everything that was changed while anonymising it, as a list.

Its `expect.yml` is written by reading the files, and has **no** `origin:` line: a recording is
the default. The profile test will then fail, on purpose, until `qos` is taken off
`unverified:` in `profiles/cisco_ios.yaml` — do that in the same change, and correct the
profile if the recording shows it to be wrong. That is the whole point of the recording.
