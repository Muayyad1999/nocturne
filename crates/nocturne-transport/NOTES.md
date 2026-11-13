# nocturne-transport — working notes

Running log so the work can be resumed after an interruption.
Spec: `docs/dev/spec-transport.md`.

## How to build

Real workspace loads again (coordinator restored the sibling stubs):

```
cd /home/user/nocturne && CARGO_TARGET_DIR=/home/user/.cache/nocturne-target/transport cargo <cmd> -p nocturne-transport
```

ALWAYS scope with `-p nocturne-transport` (also `cargo fmt -p nocturne-transport`, never a bare `cargo fmt`).
Fallback only if another crate's manifest breaks the workspace again (mention in report): shadow workspace at
`/tmp/claude-0/-home-user-netops-ai/862b16f9-1524-5810-bc17-cca256bcf369/scratchpad/shadow`
(copy of root Cargo.toml with `members = ["crates/nocturne-transport"]`, internal path deps removed,
`crates/nocturne-transport` symlinked, run cargo from that dir with the same CARGO_TARGET_DIR).

## Status

- [x] research russh 0.65.1 / russh-sftp 3.0.1 / serialport 4.10.1 / portable-pty 0.9.0 APIs
- [x] Cargo.toml deps (russh-sftp, serialport no-default-features, portable-pty, socket2)
- [x] connection.rs / error.rs / relay.rs / pump.rs / net.rs  (compiled, 34 unit tests pass)
- [x] ssh/{mod(params),algorithms,keys,auth,client,jump,sftp} written + compiles (`cargo check --tests` ok); NOT yet exercised
- [x] telnet/{mod(client),protocol} written + compiles; NOT yet exercised
- [x] serve/{mod,ssh,telnet} written + compiles; NOT yet exercised
- [x] serial.rs, local.rs written + compile; NOT yet exercised
- [x] in-process SSH tests: src/tests/ssh.rs — 44 pass (login/auth matrix, kbd-int fallback, keys, banner, resize,
      6 MB integrity, close reasons, keepalive timeout, jump 1+2 hops + failures, legacy matrix vs own server,
      slow-reader/keepalive, bulk echo no-deadlock, 24 MiB stalled reader, concurrency). Verified the back-pressure
      tests FAIL when the relay high-water mark is set to 1 (naive pump) → they have teeth.
- [ ] in-process telnet / local / serial tests (src/tests/{telnet,local,serial}.rs are stubs)
- [ ] OpenSSH interop tests (modern + legacy matrix)
- [ ] telnetd interop
- [ ] remove `#![allow(dead_code)]` from lib.rs, clippy clean, fmt, windows cross-check
- [ ] final report

### Test plan (in-process, crate-internal `#[cfg(test)]` modules so pub(crate) server options are usable)
SSH: password ok/fail; key auth (ed25519, rsa file, encrypted); kbd-int only server (Password fallback +
KeyboardInteractive + wrong pw); resize; initial size; large output integrity (5 MB); close reasons (device exit →
"exit status 0", local close → None, server shutdown → "disconnected by the server: ...", TCP loss); verifier
accept/reject/TOFU-changed (restart server with new key on same port); banner; exec refused; jump 1 hop + 2 hops,
jump errors (target unreachable, forwarding prohibited, hop auth failure, >5 hops); legacy matrix against own server
(kex g1/g14/gex-sha1, aes-cbc/3des, hmac-sha1, ssh-rsa, ssh-dss) Auto flags / Never NoCommonAlgorithm / Always+modern
not flagged; slow reader + server keepalive (many small chunks, reader paused) survives; bulk echo with late reader
does not deadlock; 20 concurrent sessions; session released after close; two shells on one SshSession.
Telnet: client<->serve_telnet (NAWS size at open, echo, resize, break, exit, close); CR/IAC data both ways;
raw TCP fake with split IAC sequences; no duplicate replies. Local: sh -c printf, interactive, resize (stty size),
env/cwd, exit status, close kills child. Serial: pty pair (unix).

## Design decisions

- Back-pressure: russh delivers channel data with `chan.send().await` *inside its session loop* and
  re-opens the SSH window automatically, so a consumer that stops calling `channel.wait()` freezes the
  whole session (keepalives, other channels, jump hops). Therefore every russh channel reader (client
  shell, server session) drains continuously into a `Relay` (ordered staging queue, coalesces small
  chunks, bounded by a byte high-water mark) which feeds the bounded mpsc. Below the mark the russh
  loop is never blocked; above it we stop draining (TCP back-pressure, nothing dropped).
- Each direction of every transport is an independent task/thread → no duplex deadlock.
- RFC 4335 break: russh 0.65.1 has NO way to send a custom channel request ("break") →
  `Connection::send_break()` on an SSH shell returns `TransportError::Unsupported`; `supports_break()` accessor.
- `ConnectionInfo.server_banner` = SSH identification string, then "\n" + pre-auth banner if any.
- LegacyCrypto: modern set ≈ OpenSSH defaults (incl. ecdh-nistp), legacy adds group14-sha1, gex-sha1,
  group1-sha1, ssh-rsa, ssh-dss, aes*-cbc, 3des-cbc, hmac-sha1(-etm). Auto retries once on NoCommonAlgo
  (or remote DISCONNECT KeyExchangeFailed during kex). SHA-1 RSA *user* signatures also count as legacy
  (Never → NoCommonAlgorithm). Time-based rekey disabled when the legacy set is in use.
- Auth: probe with `none` first to learn server methods; Password → `password` method if offered, else
  keyboard-interactive; kbd-int answers every prompt, stops answering when the same prompt repeats.
- Do not add fields to contract structs (ConnectionInfo, Backend, params) — others may build them literally.
- `ConnectionWriter::close(&self)` returns `()` like `Connection::close`.

## Research findings (russh 0.65.1, read from ~/.cargo/registry/src/*/russh-0.65.1)

Client API facts:
- `client::Handler::check_server_key(&mut self, &PublicKeyOrCertificate) -> Result<bool, Self::Error>`
  (`russh::keys::PublicKeyOrCertificate`, `.public_key()` gives `ssh_key::PublicKey`).
- `client::Handler::kex_done(&mut self, shared_secret, names: &russh::Names, session)` — negotiated
  `names.kex/.key/.cipher/.client_mac/.server_mac`. Called on rekey too. `session.remote_sshid()`.
- `client::Handler::auth_banner(&mut self, banner, session)`; `disconnected(reason: DisconnectReason<E>)`
  with `ReceivedDisconnect(RemoteDisconnectInfo{reason_code,message,lang_tag})` or `Error(E)`; called
  before the channels are dropped (so `channel.wait()==None` ⇒ reason already recorded).
- `client::connect_stream(Arc<Config>, stream, handler) -> Result<Handle<H>, H::Error>`.
  Pre-kex errors are returned from it (`Error::NoCommonAlgo{kind,ours,theirs}`, `UnknownKey`, `Disconnect`...).
- Session ends when Handle + all channel write halves are dropped (no DISCONNECT sent) → send
  `handle.disconnect(ByApplication, ..)` for a clean close.
- kbd-int: while an InfoRequest is pending the session loop only accepts `AuthInfoResponse` → must always respond.
- `GexParams::new(min, preferred, max)` rejects min < 2048 → 1024-bit GEX groups impossible.
- publickey auth always probes first (no signature) then signs. `best_supported_rsa_hash()`:
  `Some(Some(h))`, `Some(None)` (ext-info without rsa-sha2), `None` (no ext-info).
- Server: `server::run_stream(config, stream, handler) -> RunningSession` (Future + `.handle()`);
  handler callbacks get data AND the `Channel<Msg>` read half gets it via `chan.send().await` → either
  drop the Channel or drain it. Unknown channel requests (incl. "break") are auto-rejected.
- Agent: `AgentClient::connect_env()` (unix), `connect_named_pipe(path)` / `connect_pageant()` (windows),
  `.dynamic()`, `request_identities()`, `handle.authenticate_publickey_with(user, key, hash, &mut agent)`.
- Keys: `russh::keys::decode_secret_key(text, passphrase)`, `PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519)`,
  `ssh_key::private::RsaKeypair::random(rng, bits)`, `to_openssh(LineEnding::LF)`, `public_key().to_openssh()`,
  `fingerprint(HashAlg::Sha256).to_string()` = "SHA256:…".

Algorithms compiled into this build (features ring, rsa, dsa, des, flate2):
- kex: mlkem768x25519-sha256, curve25519-sha256(+@libssh.org), dh-gex-sha256, dh-group14-sha256,
  dh-group15/16/17/18-sha512, ecdh-sha2-nistp256/384/521, **dh-gex-sha1, dh-group1-sha1, dh-group14-sha1**.
- ciphers: chacha20-poly1305@openssh.com, aes128/256-gcm@openssh.com, aes128/192/256-ctr,
  **aes128/192/256-cbc, 3des-cbc**.
- MACs: hmac-sha2-256/512 (+etm), **hmac-sha1, hmac-sha1-etm@openssh.com**. No hmac-md5, no *-96, no umac.
- host keys: ed25519, ecdsa p256/384/521, rsa-sha2-512/256, **ssh-rsa (SHA-1)**, **ssh-dss** (dsa feature).
- NOT available: hmac-md5(-96), hmac-sha1-96, umac-*, hmac-ripemd160, arcfour*, blowfish-cbc, cast128-cbc,
  sntrup761x25519, GEX groups < 2048 bit, SSH protocol 1.

Other crates:
- russh-sftp 3.0.1 (Apache-2.0): `SftpSession::new(channel.into_stream())`; `write()` does NOT create/truncate →
  use `create()`; `File` is AsyncRead/AsyncWrite, `shutdown()` confirms pending writes.
- serialport 4.10.1 (MPL-2.0) without libudev enumerates from /sys/class/tty (panics if that dir is missing → guard).
  `TTYPort::pair()` gives a pty pair for tests. Reads honour `timeout` (TimedOut error).
- portable-pty 0.9.0 (MIT): blocking reader/writer; master is Send but not Sync; cloned killer only sends SIGHUP.

## Environment

- OpenSSH_9.6p1 at /usr/sbin/sshd, /usr/sbin/telnetd (inetutils), inetutils-inetd present.
- Port 2024 is used by something else on this machine: do not bind it.
- No stray daemons / `noctest-*` users existed at restart (checked).
