# nocturne-netprobe — working notes (resume file)

Spec: `/home/user/nocturne/docs/dev/spec-netprobe.md` (the "Public API (contract)" section is a contract).
Build: `cd /home/user/nocturne && CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/netprobe cargo <cmd> -p nocturne-netprobe`
Only touch files in this crate. No git. `cargo fmt -p nocturne-netprobe` only (never a bare `cargo fmt`).
Scratch: `/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/netprobe/`

## Environment facts (verified)
- rustc/cargo 1.97.0, edition 2024. Windows target `x86_64-pc-windows-gnu` + `gcc-mingw-w64-x86-64` installed.
- Machine: root with CAP_NET_RAW; `net.ipv4.ping_group_range = 1 0` (ICMP datagram sockets denied -> raw is detected).
  `unshare -n` gives a private netns where ping_group_range can be changed to test the datagram path.
- **No IPv6 in this kernel** (AF_INET6 -> EAFNOSUPPORT): every ::1 test must skip; IPv6 code is only unit-tested.
- Outbound: UDP DNS to 8.8.8.8 / 1.1.1.1 works; direct TCP is refused (HTTPS only through a proxy) -> whois/TLS/HTTP
  internet tests must detect and skip.
- apt-installed test tools: iputils-ping, traceroute, tftp-hpa (`tftp`), softflowd, nfdump/nfcapd, dnsutils (`dig`),
  whois, net-tools (`arp`). `curl` and `logger` were already present. No daemons were left running.
- hickory 0.26: `hickory_resolver::proto::op::Message` has public fields (`metadata`, `queries`, `answers`, …);
  `system_conf::read_system_conf()` gives the resolvers. nix 0.31.3: `recvmsg(fd, iov, Option<&mut [u8]>, flags)`,
  `ControlMessageOwned::{Ipv4RecvErr, Ipv6RecvErr, Ipv4Ttl, Ipv6HopLimit}`.

## Design decisions (keep consistent)
- `ProbeError` has exactly the nine contract variants (`Resolve`, `Timeout`, `Refused` are unit variants).
- Extra deps: `sha2`, `getrandom` (workspace), `nix` (Linux only, safe errqueue/TTL access), dev: `rcgen`.
- No `unsafe` anywhere.
- Ping: datagram path = one socket per echo (Linux: IP_RECVERR + IP_RECVTTL through nix); raw path = one shared
  socket per family with a demultiplexing reader task; system path = `ping` binary, bounded by a semaphore.
- TLS: the accept-all verifier is private to `tls`; `http` with `verify_tls = true` uses the strict platform verifier.
- Servers: accept loop task owns a `JoinSet`; `stop()`/drop abort it, which releases every socket.

## Status
### Done (tests green)
- `error.rs`, `util.rs`, `process.rs`, `sys/linux.rs` (nix: errqueue, TTL cmsg, BPF filter for raw ICMP, rcvbuf)
- `dns.rs` (own UDP/TCP exchange on hickory-proto messages; unit + fake-server tests)
- `ping/` (`icmp.rs` codec, `socket.rs` datagram + raw demux, `system.rs` ping parser) + `tests/ping.rs`
- `local/` (interfaces, neighbours, gateways, WoL; parsers for Linux/macOS/BSD/Windows)
- `tcp/` (ports, services, scan, banners) + `tests/tcp.rs` (nmap cross-check)
- `sweep.rs` + `tests/sweep.rs`
- `traceroute/` (native Linux UDP+errqueue, system traceroute/tracert incremental parser) + `tests/traceroute.rs`
- `tests/lab.rs` + `tests/lab/run.sh`: netns topology client-r1-r2-target (root only). Run: `crates/nocturne-netprobe/tests/lab/run.sh`
  (11 integration + 1 lib test green). Facts learned: Linux routers rate-limit route errors (burst 5, 1/s, global
  sysctl, not per-netns) -> tests ask for <=2 errors per router and sleep 5 s first.
- `examples/probe.rs` (manual CLI)

- `ntp.rs` + `tests/ntp.rs` (chronyd interop: `chronyd -n -x -U -u root -f conf`, never touches the clock)
- `whois.rs` (fake-server chain tests; `whois_at` added for tests / direct registry queries)
- `tls.rs` + `tls/test_support.rs` (rcgen PKI, TestCa, tls_server) + `tests/tls.rs` (openssl s_server/x509 interop)
- `http.rs` (own HTTP/1.1 client) + `tests/http.rs` (python http.server, openssl s_server -www, curl cross-check)
- `tests/support/mod.rs`: `Helper` (kill-on-drop child), `require_program`, free ports

- `tftp/` (packet, path, transfer, server) + `tests/tftp.rs` (own client in `tests/support/tftp_client.rs`) +
  `tests/tftp_interop.rs` (curl get/put incl. blksize / no-options, tftp-hpa get/put). Note: curl rejects `tsize=0`
  from any server (empty file needs `--tftp-no-options`).

### In progress
- syslog

### Next
- flow
- interop tests: curl/tftp-hpa (TFTP), logger (syslog), softflowd (flow), dig (DNS)
- `tests/internet.rs` (public DNS / whois / TLS / HTTP with skip detection)
- robustness tests (random/truncated datagrams) for syslog/flow/tftp/sntp
- Windows `cargo check --target x86_64-pc-windows-gnu`, clippy `-D warnings`, `cargo fmt -p nocturne-netprobe`, final report
