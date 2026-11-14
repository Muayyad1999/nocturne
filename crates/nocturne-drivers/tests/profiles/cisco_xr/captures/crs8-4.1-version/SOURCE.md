# crs8-4.1-version

A CRS-8/S running IOS XR 4.1.0, for `facts` and `health`.

Awkward on purpose: the bundled `cisco_xr_show_version` template captures no
hostname and no serial number. IOS XR prints the hostname only in the
"<hostname> uptime is …" line, which is why the profile reads it with a second,
`raw` step over the same output; the serial number is not in `show version` at
all and has to come from `show inventory`. `LOCATION`, `BUILD_HOST` and `LABEL`
belong to the newer "Build Information" shape of the command and stay empty
here, so the profile leaves those `extra.*` fields out rather than setting them
to "".

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_version.txt` | `tests/cisco_xr/show_version/cisco_xr_show_version_crs.raw` |

The expected values in `expect.yml` were written by reading these files.
