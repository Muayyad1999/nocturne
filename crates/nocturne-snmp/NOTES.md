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

## Status: COMPLETE (pending only the lead's review)
- Source: error, oid, value, ber, pdu, v3/{mod,usm}, client/{mod,transport,session,walk,tests}, agent, trap,
  names (+names/extra.rs), oids, robustness (cfg(test)), lib.rs (`#![forbid(unsafe_code)]`).
- Tests (all green, also when three copies of every test binary run concurrently):
  126 unit, 12 loopback (+1 ignored timing report), 4 robustness, 11 net-snmp interop, 4 doctests.
- `cargo clippy -p nocturne-snmp -- -D warnings` clean (also with `--all-targets`); `cargo fmt -p nocturne-snmp`
  applied; `cargo doc -p nocturne-snmp --no-deps` without warnings.
- Interop matrix vs net-snmp 5.9.4.pre2: v1, v2c, v3 noAuthNoPriv, 6 authNoPriv, 36 authPriv
  (MD5|SHA|SHA-224|SHA-256|SHA-384|SHA-512 x DES|AES|AES-192|AES-256|AES-192-C|AES-256-C): all pass.
- Dictionary: 3857 objects (1000 with constants), 1261 notifications; generated from the MIB modules and
  verified 5117/5118 against `snmptranslate` (the one exception is the hand-added egpNeighborLoss).

This file was requested by the coordinator as a resume aid; it can be deleted once the crate is committed.
