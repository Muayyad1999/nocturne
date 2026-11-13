# Configurations

| file | origin |
|---|---|
| `asr-subinterfaces.txt` | **Real.** Verbatim copy of `tests/cisco_ios/show_running-config_interface/cisco_ios_show_running-config_interface.raw` from the [ntc-templates](https://github.com/networktocode/ntc-templates) corpus (Apache-2.0): the sub-interface sections of an ASR, with the `Building configuration...` / `Current configuration` header a real fetch starts with. |
| `access-switch.txt` | **Written by hand** in the layout IOS 15.2 gives the running configuration of a Catalyst 2960-X access switch (section order, `!` separators, one-space indentation, a multi-line banner, the `Last configuration change` and `NVRAM config last updated` comments, `ntp clock-period`). The corpus has no complete running configuration. Names, addresses and hashes are made up. |
| `access-switch.again.txt` | The same configuration as a second fetch of the unchanged device would show it: later timestamps, a drifted `ntp clock-period`, the two lines `exec prompt timestamp` puts in front of every command, and a log message that a console line printed into the middle of the listing. After cleaning it must equal `access-switch.txt`. |
