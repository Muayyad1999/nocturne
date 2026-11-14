# qfx-vlans

Three VLANs of an ELS switch with their member interfaces.

The point of the case is the trailing `*`. Junos marks a member that is
currently forwarding with one — `ae0.0*` — which is state, not part of the
name: no interface anywhere else in the model is called `ae0.0*`, so a
topology built from these names would not join up. The profile strips it, and
`expect.yml` lists the bare names.

Also here: the member list is a continuation block under each VLAN rather than
a column, `v109-TEMP-Project-Data` is long enough to run into the tag column,
and the file ends with `{master:0}` with no newline before it.

The `Routing instance` column (`default-switch`) has nowhere to go in the
model's VLAN row, and this listing has no up/down column, so `status` is left
empty.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_vlans.txt` | `tests/juniper_junos/show_vlans/juniper_junos_show_vlans.raw` |

The expected values in `expect.yml` were written by reading these files.
