# health-temperature

What a board with temperature sensors answers to `/system health print`, so that the optional
health step is exercised.

**`system_health_print.txt` is `synthetic: true`.** The ntc-templates corpus has no
`/system health print` capture. The file was written by hand from RouterOS 7's documented
layout: a `Columns:` line, a header row, and one numbered row per sensor whose value carries
its unit (`46C`). RouterOS 6 prints the same information as a property sheet
(`temperature: 30C`); the profile's pattern reads both, but only the RouterOS 7 form is
covered here.

`system_resource_print.txt` is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus (Apache-2.0),
`tests/mikrotik_routeros/system_resource_print/mikrotik_routeros_system_resource_print_1.raw` —
the same file as in `captures/hex-poe-lite-v6/`. It is here because `/system resource print` is
the first step of the `health` kind and supplies the CPU, memory and uptime figures the
temperature step is merged into.

The expected values in `expect.yml` were written by reading these files.
