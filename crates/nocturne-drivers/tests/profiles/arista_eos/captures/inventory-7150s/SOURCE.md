# inventory-7150s

`show inventory` of a DCS-7150S-52-CL. EOS answers with a stack of tables —
chassis, power supply slots, fan modules, port counts, transceiver slots,
storage — and the bundled template emits one row per line of each of them.

Two consequences the case pins down:

* the three rows the "System has 53 ports" table produces (`Management 1`,
  `Switched 50`, `SwitchedBootstrap 2`) are **counts, not parts**: they carry
  no serial number, and the profile's `when: SN` drops them. 24 rows are
  parsed, 21 are kept;
* the template drops which table a row came from, so `name` is left empty
  rather than filled with the bare slot or port number, which is `1` for the
  first power supply, the first fan and the first transceiver alike
  (see NOTES.md).

Four fan modules report the serial number `N/A`, which is kept as printed.

Every `.txt` file here is a verbatim copy of a real device capture from the
[ntc-templates](https://github.com/networktocode/ntc-templates) test corpus
(Apache-2.0, commit `84ae82fd022807810d73a19655cca113b734de58`):

| file | copied from |
|---|---|
| `show_inventory.txt` | `tests/arista_eos/show_inventory/arista_eos_show_inventory7.raw` |

The expected values in `expect.yml` were written by reading these files.
