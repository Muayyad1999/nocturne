//! Interactive device transports: SSH, Telnet, serial console and local shell.
//!
//! Every interactive session — a human typing in a terminal tab, a driver
//! scraping `show` output, a change being pushed — runs over one abstraction:
//! a [`Connection`], a duplex byte stream with resize and break
//! side-channels.
//!
//! # Using a connection
//!
//! [`Connection::split`] gives a cloneable [`ConnectionWriter`] and a
//! [`ConnectionReader`]. A terminal view runs two tasks: one forwards what
//! [`ConnectionReader::recv`] yields to the screen until it sees
//! [`Output::Closed`] (always the last item, exactly once, with the reason
//! when there is one worth showing); the other passes keystrokes to
//! [`ConnectionWriter::send`], and size changes to
//! [`ConnectionWriter::resize`].
//!
//! ```no_run
//! # async fn demo() -> Result<(), nocturne_transport::TransportError> {
//! use nocturne_transport::{Output, TelnetParams, connect_telnet};
//!
//! let params = TelnetParams { host: "10.0.0.1".into(), ..TelnetParams::default() };
//! let (writer, mut reader) = connect_telnet(&params).await?.split();
//! tokio::spawn(async move {
//!     while let Some(output) = reader.recv().await {
//!         match output {
//!             Output::Data(bytes) => { /* feed the terminal emulator */ }
//!             Output::Closed { reason } => { /* show why, if `Some` */ }
//!         }
//!     }
//! });
//! writer.send("show version\r").await?;
//! writer.close().await; // or `Connection::close`, which also waits
//! # Ok(()) }
//! ```
//!
//! # Flow control
//!
//! Both directions are bounded (64 items each, see
//! [`Connection::DEFAULT_BUFFER`]), and nothing is ever dropped. Output the
//! application has not taken is staged by the transport — up to 4 MiB for an
//! SSH shell, 1 MiB for a serial port or a local shell, 256 KiB for Telnet —
//! and beyond that the remote end is made to wait. Two consequences:
//!
//! * Keep reading while sending. `send` waits when the remote end does not
//!   take the data, and a device that echoes will not take more until its
//!   echo is read: a task that sends megabytes before it reads anything
//!   waits for itself.
//! * An SSH connection has a single reader for all its channels (a limit of
//!   the SSH library, which also acknowledges data before anybody took it).
//!   A shell whose output is left unread beyond those 4 MiB therefore holds
//!   up everything else on its [`SshSession`] — other shells, `exec`, SFTP,
//!   keep-alives — until the application reads again.
//!
//! # Closing
//!
//! [`Connection::close`] asks the transport to hang up and waits (a bounded
//! time) until the socket, serial port or child process is released; unread
//! output is discarded. Dropping the reading half is the hard stop. Dropping
//! all writers only means "no more input": output keeps arriving until the
//! remote end closes. An [`SshSession`] stays connected for as long as it,
//! or anything opened from it (shells, [`Sftp`]), is alive.

mod connection;
mod drain;
mod error;
mod local;
mod net;
mod pump;
mod relay;
mod serial;
pub mod serve;
mod ssh;
mod telnet;

#[cfg(test)]
mod tests;

pub use connection::{
    Backend, Connection, ConnectionInfo, ConnectionReader, ConnectionWriter, Input, Output,
    TransportKind,
};
pub use error::TransportError;
pub use local::{LocalParams, connect_local};
pub use serial::{
    FlowControl, Parity, SerialParams, SerialPortInfo, connect_serial, list_serial_ports,
};
pub use ssh::{
    AcceptAnyHostKey, ExecOutput, HostKeyVerifier, KeyKind, LegacyCrypto, MemoryKnownHosts,
    PresentedKey, Sftp, SftpEntry, SshAuth, SshHop, SshParams, SshSession, connect_ssh,
    generate_keypair,
};
pub use telnet::{TelnetParams, connect_telnet};
