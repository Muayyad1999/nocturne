# nocturne-snmp — working notes (resume file)

Spec: `/home/user/nocturne/docs/dev/spec-snmp.md` (public API section is a contract).
Build: `cd /home/user/nocturne && CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/snmp cargo <cmd> -p nocturne-snmp`
Only touch files in this crate. No git. `cargo fmt -p nocturne-snmp` only (never bare `cargo fmt`).

## Environment facts (verified)
- rustc/cargo 1.97.0, edition 2024. net-snmp 5.9.4.pre2 (`/usr/sbin/snmpd`, `/usr/bin/snmp*`).
- Private snmpd recipe (works): `SNMP_PERSISTENT_DIR=<tmp> MIBS= snmpd -f -Lf <log> -C -c <conf> -p <pid> -I -smux udp:127.0.0.1:<port>`
  (do NOT also put `agentaddress` in the conf; `-I -smux` stops it binding TCP 199).
  CLI tools: set `SNMP_PERSISTENT_DIR` + `SNMPCONFPATH` to a temp dir or they write `/var/lib/snmp`.
- This net-snmp accepts ALL of: auth MD5|SHA|SHA-224|SHA-256|SHA-384|SHA-512 x priv DES|AES|AES-192|AES-256
  (verified with the CLI). `AES-192`/`AES-256` = Blumenthal key extension; `AES-192-C`/`AES-256-C` = Cisco/Reeder
  (CLI accepts the -C names; they differ for MD5/SHA-1).
- net-snmp behaviour: wrong AUTH passphrase -> Report usmStatsWrongDigests; unknown user -> Report;
  auth requested for a noauth user -> usmStatsUnsupportedSecLevels; noAuth/authNoPriv on a priv-only user ->
  Response authorizationError(16); wrong PRIV passphrase -> silently dropped (client sees Timeout!).
- Reference vectors captured from net-snmp are embedded in the unit tests (packets, traps, localized keys).
- RustCrypto: `hmac::{Hmac, KeyInit, Mac}`; `cfb_mode::{Encryptor,Decryptor}<aes::AesNNN>` + `KeyIvInit::new_from_slices`
  then `.encrypt(&mut buf)`; `cbc::{Encryptor,Decryptor}<des::Des>` + `encrypt_padded::<NoPadding>(buf,len)`.

## Design decisions (keep consistent)
- Modules: `ber` `oid` `value` `pdu` `error` `v3` (+`v3/usm`) `client` `trap` `names` `oids` `agent`; internals are
  `pub(crate)`; only the contract API (+ small additions) is public.
- `PrivProtocol::Aes192/Aes256` = Blumenthal (net-snmp `AES-192/256`); added `Aes192Cisco/Aes256Cisco` (Reeder).
- Client socket is UNCONNECTED; replies matched by request-id / msgID (multi-homed agents reply from other IPs).
- `Auth` gets a manual, redacting `Debug`.
- walk: limit 0 => empty; empty walk falls back to GET of the root (like snmpwalk).
- table: one multi-column GETBULK stream, rows sorted by index, `limit` = max rows.
- get/get_next: chunked (50 OIDs/PDU) and split in halves on tooBig; get_bulk/walk halve max-repetitions on tooBig.
- v3: lazy discovery on first request; one-shot resync on NotInTimeWindows; one-shot rediscovery on UnknownEngineID;
  keys cached per engine ID inside the Client; privacy-mismatch diagnosis probe after an authPriv timeout.
- Agent is `Clone` (Arc inside) so `set_view` works while `run()` is serving; `set_max_message_size` added.

## Status
### Done (last run: 88 unit tests + 4 doctests green; interop 10/11 (11th was a flaky assertion, fixed))
- All source modules: error, oid, value, ber, pdu, v3/{mod,usm}, client/{mod,transport,session,walk},
  agent, trap, names (+names/extra.rs), oids, lib.rs.
- Dictionary: 3857 objects (1000 consts), 1261 traps; generated from MIB modules (LibreNMS collection),
  verified 5117/5118 with snmptranslate + completeness checked against `snmptranslate -Tz`
  (scraper bug that dropped IF-MIB::ifIndex found by the live test and fixed).
- `tests/netsnmp_interop.rs` written and run live: v1/v2c get/walk/set/table, 43/43 v3 combinations pass
  (6 auth x {none, DES, AES, AES-192, AES-256, AES-192-C, AES-256-C} + noAuth), error mapping, privacy
  mismatch diagnosis, AES key-extension auto-detect, restart resync/rediscovery/failover, full mib-2 walk
  == snmpwalk, traps v1/v2c/v3-noauth + inform ack accepted by snmpinform.

### In progress
- `client/tests.rs`: scripted responders (tooBig halving, reports mapping, NotInTimeWindows resync,
  UnknownEngineID rediscovery, stray/late datagrams, privacy-mismatch diagnosis, retries same request id)

### Next
- `tests/loopback.rs` (Agent: get/get_next/bulk walk of several hundred OIDs/table pivot/tooBig/timeout+retry)
- `tests/robustness.rs` + in-crate fuzz of private decoders
- remove dead code (`peek_tag`, `is_no_such_name`...), clippy `-D warnings`, `cargo fmt -p nocturne-snmp`
- final report with matrix (logs in scratchpad work/interop*.log are NOT durable - rerun with --nocapture)
