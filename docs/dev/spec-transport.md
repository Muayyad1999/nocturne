# Spec: `nocturne-transport` — SSH, Telnet, serial and local-shell transports

Every interactive session in Nocturne — a human typing in a terminal tab, the
CLI driver scraping `show` output, the change pipeline pushing configuration,
the AI agent investigating — runs over one abstraction: a `Connection`, a
duplex byte stream with resize/break side-channels. This crate provides that
abstraction and its real-world implementations. (The lab simulator plugs in
its own `Connection`s through `Connection::pair`.)

## Public API (contract)

```rust
use bytes::Bytes;

// ------------------------------------------------------------ the connection
#[derive(Debug)]
pub enum Input  { Data(Bytes), Resize { cols: u16, rows: u16 }, Break, Close }
#[derive(Debug, Clone)]
pub enum Output { Data(Bytes), Closed { reason: Option<String> } }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind { Ssh, Telnet, Serial, Local, Sim }

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub kind: Option<TransportKind>,
    pub peer: String,                       // "10.0.0.1:22", "COM3", "lab:R1"
    pub server_banner: Option<String>,      // SSH identification string / pre-auth banner
    pub kex: Option<String>, pub cipher: Option<String>, pub mac: Option<String>,
    pub host_key_algorithm: Option<String>,
    pub host_key_fingerprint: Option<String>,   // "SHA256:…"
    pub legacy_crypto: bool,                // negotiated only thanks to the legacy algorithm set
    pub via: Vec<String>,                   // jump hosts traversed
}

pub struct Connection { /* info + channel ends */ }
impl Connection {
    /// Build a connection and the backend half a custom transport drives.
    pub fn pair(info: ConnectionInfo, buffer: usize) -> (Connection, Backend);
    pub fn info(&self) -> &ConnectionInfo;
    pub async fn send(&self, data: impl Into<Bytes>) -> Result<(), TransportError>;
    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), TransportError>;
    pub async fn send_break(&self) -> Result<(), TransportError>;
    /// Next chunk of output. `None` once the connection is closed and drained;
    /// the last item before `None` is `Output::Closed`.
    pub async fn recv(&mut self) -> Option<Output>;
    pub fn try_recv(&mut self) -> Option<Output>;
    pub fn split(self) -> (ConnectionWriter, ConnectionReader);
    pub async fn close(self);
}
#[derive(Clone)] pub struct ConnectionWriter { /* send / resize / send_break / close(&self) */ }
pub struct ConnectionReader { /* recv / try_recv / info */ }
/// The transport side of a `Connection`.
pub struct Backend {
    pub input: tokio::sync::mpsc::Receiver<Input>,
    pub output: tokio::sync::mpsc::Sender<Output>,
}

// ------------------------------------------------------------------- errors
#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    Resolve(String), ConnectTimeout, Refused, Unreachable(String),
    HostKeyRejected(String),
    AuthFailed { tried: Vec<String>, server_methods: Vec<String> },
    NoCommonAlgorithm(String),              // message lists what the server offered
    Protocol(String), Io(std::io::Error), Closed, Timeout, Unsupported(String),
}
impl TransportError {
    /// Stable machine code for the UI: "resolve", "connect_timeout", "refused", "unreachable",
    /// "host_key", "auth", "algorithms", "protocol", "io", "closed", "timeout", "unsupported".
    pub fn code(&self) -> &'static str;
}

// ---------------------------------------------------------------------- SSH
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SshParams {
    pub host: String,
    pub port: u16,                           // default 22
    pub username: String,
    pub auth: Vec<SshAuth>,                  // tried in order
    pub jump: Vec<SshHop>,                   // ProxyJump chain, first hop first
    pub term: String,                        // default "xterm-256color"
    pub cols: u16, pub rows: u16,            // default 120 × 40
    pub connect_timeout_ms: u64,             // default 10_000
    pub keepalive_s: u64,                    // 0 = off, default 30
    pub legacy: LegacyCrypto,                // default Auto
}
pub struct SshHop { pub host: String, pub port: u16, pub username: String, pub auth: Vec<SshAuth> }
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SshAuth {
    Password { password: String },
    /// OpenSSH / PEM private key text (RSA, ECDSA, Ed25519), optionally encrypted.
    PrivateKey { key: String, passphrase: Option<String> },
    KeyFile { path: String, passphrase: Option<String> },
    /// Answers every keyboard-interactive prompt with `response`.
    KeyboardInteractive { response: String },
    /// Keys offered by the running ssh-agent (SSH_AUTH_SOCK / Pageant pipe).
    Agent,
    None,
}
pub enum LegacyCrypto {
    /// Modern algorithms first; if negotiation fails, retry once with the legacy set and flag the session.
    Auto,
    Never,
    Always,
}

pub struct PresentedKey { pub algorithm: String, pub fingerprint: String /* SHA256:… */, pub openssh: String }
#[async_trait::async_trait]
pub trait HostKeyVerifier: Send + Sync {
    /// Return Ok to continue, Err(reason) to abort the handshake.
    async fn verify(&self, host: &str, port: u16, key: &PresentedKey) -> Result<(), String>;
}
/// Accepts everything (tests, lab).
pub struct AcceptAnyHostKey;
/// Trust-on-first-use against an in-memory map; rejects changed keys.
pub struct MemoryKnownHosts { /* … */ }

/// An authenticated SSH connection that can open several channels.
pub struct SshSession { /* … */ }
impl SshSession {
    pub async fn connect(params: &SshParams, verifier: std::sync::Arc<dyn HostKeyVerifier>) -> Result<SshSession, TransportError>;
    pub fn info(&self) -> &ConnectionInfo;
    /// PTY + shell channel as a `Connection`.
    pub async fn shell(&self) -> Result<Connection, TransportError>;
    /// One-shot exec channel (no PTY).
    pub async fn exec(&self, command: &str, timeout: std::time::Duration) -> Result<ExecOutput, TransportError>;
    pub async fn sftp(&self) -> Result<Sftp, TransportError>;
    pub async fn close(self);
}
pub struct ExecOutput { pub stdout: Vec<u8>, pub stderr: Vec<u8>, pub exit_status: Option<u32> }
/// Shorthand: connect + shell. The session lives as long as the connection.
pub async fn connect_ssh(params: &SshParams, verifier: std::sync::Arc<dyn HostKeyVerifier>) -> Result<Connection, TransportError>;

pub struct Sftp { /* … */ }
impl Sftp {
    pub async fn list(&self, path: &str) -> Result<Vec<SftpEntry>, TransportError>;
    pub async fn stat(&self, path: &str) -> Result<SftpEntry, TransportError>;
    pub async fn read(&self, path: &str) -> Result<Vec<u8>, TransportError>;
    pub async fn write(&self, path: &str, data: &[u8]) -> Result<(), TransportError>;
    pub async fn download(&self, remote: &str, local: &std::path::Path, progress: Option<tokio::sync::mpsc::Sender<(u64, u64)>>) -> Result<u64, TransportError>;
    pub async fn upload(&self, local: &std::path::Path, remote: &str, progress: Option<tokio::sync::mpsc::Sender<(u64, u64)>>) -> Result<u64, TransportError>;
    pub async fn mkdir(&self, path: &str) -> Result<(), TransportError>;
    pub async fn remove(&self, path: &str) -> Result<(), TransportError>;   // file or empty dir
    pub async fn rename(&self, from: &str, to: &str) -> Result<(), TransportError>;
    pub async fn realpath(&self, path: &str) -> Result<String, TransportError>;
}
pub struct SftpEntry { pub name: String, pub path: String, pub is_dir: bool, pub is_link: bool, pub size: u64, pub modified: Option<i64>, pub permissions: Option<u32> }

/// Generate an Ed25519 or RSA key pair: (private key in OpenSSH format, public key line).
pub fn generate_keypair(kind: KeyKind, comment: &str) -> Result<(String, String), TransportError>;
pub enum KeyKind { Ed25519, Rsa3072, Rsa4096 }

// ------------------------------------------------------------------- Telnet
pub struct TelnetParams { pub host: String, pub port: u16 /* 23 */, pub term: String, pub cols: u16, pub rows: u16, pub connect_timeout_ms: u64 }
pub async fn connect_telnet(params: &TelnetParams) -> Result<Connection, TransportError>;

// ------------------------------------------------------------------- serial
pub struct SerialParams { pub port: String, pub baud: u32 /* 9600 */, pub data_bits: u8 /* 8 */, pub parity: Parity, pub stop_bits: u8 /* 1 */, pub flow: FlowControl }
pub enum Parity { None, Odd, Even }  pub enum FlowControl { None, Software, Hardware }
pub struct SerialPortInfo { pub name: String, pub description: String, pub manufacturer: Option<String>, pub usb_vid: Option<u16>, pub usb_pid: Option<u16> }
pub fn list_serial_ports() -> Vec<SerialPortInfo>;
pub async fn connect_serial(params: &SerialParams) -> Result<Connection, TransportError>;

// -------------------------------------------------------------- local shell
pub struct LocalParams { pub program: Option<String>, pub args: Vec<String>, pub cwd: Option<String>, pub cols: u16, pub rows: u16, pub env: Vec<(String, String)> }
/// The user's own shell in a PTY (ConPTY on Windows).
pub async fn connect_local(params: &LocalParams) -> Result<Connection, TransportError>;

// ------------------------------------------------- serving (lab exposure, tests)
pub mod serve {
    /// Called for every accepted session; return the connection to bridge it to, or None to refuse.
    #[async_trait::async_trait]
    pub trait SessionHandler: Send + Sync {
        async fn open(&self, peer: std::net::SocketAddr, user: Option<&str>, cols: u16, rows: u16) -> Option<super::Connection>;
    }
    pub struct SshServerConfig {
        pub bind: std::net::SocketAddr,
        /// user → password. Empty = accept any credentials.
        pub users: Vec<(String, String)>,
        /// OpenSSH private key; generated (Ed25519) when None.
        pub host_key: Option<String>,
        pub banner: Option<String>,
    }
    pub struct ServerHandle { /* local_addr(), shutdown().await ; also stops on drop */ }
    pub async fn serve_ssh(config: SshServerConfig, handler: std::sync::Arc<dyn SessionHandler>) -> Result<ServerHandle, super::TransportError>;
    pub async fn serve_telnet(bind: std::net::SocketAddr, handler: std::sync::Arc<dyn SessionHandler>) -> Result<ServerHandle, super::TransportError>;
}
```

All params structs implement `Default` with the defaults noted and
`#[serde(default)]`, so the UI can send partial JSON.

## Behaviour requirements

**SSH (`russh` 0.65, `ring` backend — read its source under
`~/.cargo/registry/src/` for the current API; it has changed a lot across
releases).**
* Old network gear is the whole point of `LegacyCrypto`. Find out exactly which
  legacy algorithms this russh build can negotiate (kex
  `diffie-hellman-group1-sha1` / `group14-sha1` / `group-exchange-sha1`, host
  keys `ssh-rsa` (SHA-1) / `ssh-dss`, ciphers `aes*-cbc` / `3des-cbc`, MACs
  `hmac-sha1` / `hmac-md5`…), enable every one it supports in the legacy
  preference set, and document which common legacy algorithms are *not*
  available. `Auto` must retry transparently and set
  `ConnectionInfo::legacy_crypto`; `NoCommonAlgorithm` must say what the
  server offered so the user can act.
* Auth methods tried in order; if the server only offers
  keyboard-interactive, a `Password` entry is used to answer it (very common
  on network devices). `AuthFailed` reports what was tried and what the
  server advertised.
* Jump hosts: each hop authenticated independently, next hop reached through
  `direct-tcpip`; up to 5 hops.
* Shell: request PTY with the given term/size, then shell; `Input::Resize`
  → window-change; `Input::Break` → RFC 4335 break request; keepalives;
  clean `Output::Closed { reason }` on EOF/exit/failure (include the exit
  status or disconnect message when known). Back-pressure must not deadlock:
  a slow reader must not stall keepalives.
* SFTP via `russh-sftp` (add it to the crate's `Cargo.toml`).
* Agent auth: implement if russh exposes a usable agent client on both Unix
  and Windows; otherwise return `Unsupported` on the missing platform.

**Telnet.** Proper option negotiation state (RFC 854/855/1143-style: never
loop): accept server `WILL ECHO` / `WILL SGA`, answer `DO TTYPE` with the
terminal type and `DO NAWS` with the window size (and send NAWS again on
resize), refuse everything else; strip IAC sequences and sub-negotiations from
the data stream even when split across TCP segments; escape `0xFF` in outgoing
data; send `\r\0`/`\r\n` correctly; `Input::Break` → `IAC BRK`.

**Serial.** `serialport` crate (add it; avoid a hard libudev requirement —
prefer `default-features = false` if port listing still works, and say what
you chose). Blocking IO isolated on dedicated threads. `Input::Break` → line
break for 250 ms (how Cisco ROMMON/password recovery is entered).

**Local shell.** `portable-pty` (add it). Default program: `$SHELL` or
`/bin/bash` on Unix; `powershell.exe` on Windows.

**Serving.** `serve_ssh` uses russh's server side: password auth against
`users`, PTY + shell requests, window-change forwarded as `Input::Resize`,
bytes pumped both ways until either side closes. `serve_telnet` negotiates
`WILL ECHO` + `WILL SGA` (character mode) and pumps. Both must handle many
concurrent sessions.

## Validation you must do

1. In-process tests (run anywhere): client ↔ `serve_ssh` round trips (password
   auth ok/fail, key auth, resize propagation, large output, close reasons,
   host-key verifier accept/reject/TOFU-changed); `connect_telnet` ↔
   `serve_telnet`, plus a raw TCP fake that sends awkward IAC sequences split
   across writes; `connect_local` running `printf`/`echo`; `Connection::pair`
   semantics; a jump-host test chaining two in-process servers.
2. **Interoperability with real daemons on this machine** (skip, don't fail,
   when they are missing): start a private OpenSSH `sshd` on a high port
   (`/usr/sbin/sshd -D -e -f <temp config> -p <port>`, own host keys, a
   temporary system user you create and delete as root, or `PermitRootLogin`
   with an `AuthorizedKeysFile` in a temp dir) and test password,
   public-key, keyboard-interactive, exec, SFTP (list/upload/download/rename/
   remove), and jump-host through the same daemon. Then restart it with
   **only legacy algorithms enabled** (`KexAlgorithms`, `Ciphers`, `MACs`,
   `HostKeyAlgorithms`, `PubkeyAcceptedAlgorithms` restricted to the old
   sets, one family at a time) and prove `LegacyCrypto::Auto` connects and
   flags it while `Never` fails with a helpful `NoCommonAlgorithm`. Also test
   Telnet against `inetutils-telnetd` if you can get it to run on a high port
   (e.g. via a tiny inetd-style wrapper), otherwise say so.
3. `cargo clippy -p nocturne-transport -- -D warnings` clean; no `unsafe`
   (the serial/pty crates' own unsafe is fine).

## Constraints

* Dependencies already declared: `tokio`, `russh`, `thiserror`, `async-trait`,
  `bytes`, `tracing`, `serde`. You will need to add `russh-sftp`,
  `serialport`, `portable-pty` (explicit versions, checked with `cargo
  search`) and may add small pure-Rust helpers if essential. Everything must
  build on Windows, macOS and Linux without a C toolchain beyond what `ring`
  needs — verify at least `cargo check --target x86_64-pc-windows-gnu -p
  nocturne-transport` (run `rustup target add x86_64-pc-windows-gnu`;
  `mingw-w64` can be installed with apt) and report the outcome.
