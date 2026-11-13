# health-composite

CPU, memory, uptime and temperature. **The four captures come from four different devices**
(the corpus has one capture of each command); together they exercise every step of the health
collector.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_processes_cpu.txt` | `tests/cisco_ios/show_processes_cpu/cisco_ios_show_processes_cpu.raw` |
| `show_processes_memory_sorted.txt` | `tests/cisco_ios/show_processes_memory_sorted/cisco_ios_show_processes_memory_sorted.raw` |
| `show_version.txt` | `tests/cisco_ios/show_version/cisco_ios_show_version.raw` |
| `show_environment_temperature.txt` | `tests/cisco_ios/show_environment_temperature/cisco_ios_show_environment_temperature.raw` |

The expected values in `expect.yml` were written by reading these files.
