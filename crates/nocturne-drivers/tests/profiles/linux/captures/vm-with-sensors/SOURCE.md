# vm-with-sensors

The two optional steps that the machine this profile was written on cannot answer: it runs
without systemd (`hostnamectl` reports "System has not been booted with systemd as init
system") and it exposes no thermal zone.

**`hostnamectl.txt` and `cat_sys_class_thermal_thermal_zone0_temp.txt` are `synthetic: true`.**
Both were written by hand:

* `hostnamectl.txt` follows the layout systemd's `hostnamectl status` prints — one
  right-aligned `Key: value` per line. The values describe a plausible virtual machine.
* `cat_sys_class_thermal_thermal_zone0_temp.txt` is one number, the thermal zone's temperature
  in thousandths of a degree Celsius, as the kernel's thermal sysfs ABI defines it.

`uname_-snrvm.txt`, `cat_proc_uptime.txt` and `cat_proc_meminfo.txt` are the real recordings
from `captures/container-host/`, copied here because they are the required steps of the `facts`
and `health` kinds: without them there would be no row for the optional steps to add to.

The expected values in `expect.yml` were written by reading these files.
