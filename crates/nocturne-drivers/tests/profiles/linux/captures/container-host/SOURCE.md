# container-host

What `uname`, `/etc/os-release`, `/proc/uptime` and `/proc/meminfo` printed on the machine this
profile was written on (Ubuntu 24.04.5 LTS, kernel 6.18.44, 16 GiB of memory), recorded
verbatim. The ntc-templates corpus has no capture of any of these four.

| file | recorded with |
|---|---|
| `uname_-snrvm.txt` | `uname -snrvm` |
| `cat_etc_os-release.txt` | `cat /etc/os-release` |
| `cat_proc_uptime.txt` | `cat /proc/uptime` |
| `cat_proc_meminfo.txt` | `cat /proc/meminfo` |

The expected values in `expect.yml` were written by reading these files.
