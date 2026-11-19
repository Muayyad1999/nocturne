//! Helpers shared by the integration tests.
#![allow(dead_code)]

pub mod tftp_client;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

/// The path of `program` if it is installed (looked up in `PATH` and the `sbin` directories).
pub fn find_program(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/sbin", "/usr/local/sbin"].map(PathBuf::from))
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Like [`find_program`], but prints why the test is skipped when the program is missing.
pub fn require_program(program: &str) -> Option<PathBuf> {
    let found = find_program(program);
    if found.is_none() {
        eprintln!("skipping: `{program}` is not installed");
    }
    found
}

/// A `logger` that can send to a network host, or `None` after saying why
/// not.
///
/// Installed is not enough. util-linux's `logger` takes `-n host -P port`;
/// the BSD one macOS ships has neither ("illegal option -- n") and can only
/// write to the local log socket. What these tests are for is
/// interoperability with the util-linux tool's wire output, so where that is
/// not the tool installed they say so and pass, rather than failing for
/// something no code in this repository controls.
pub fn require_networked_logger() -> Option<PathBuf> {
    let found = require_program("logger")?;
    let usable = std::process::Command::new(&found)
        .args(["--help"])
        .output()
        .map(|out| {
            let text = String::from_utf8_lossy(&out.stdout).to_lowercase()
                + &String::from_utf8_lossy(&out.stderr).to_lowercase();
            // `-n` on its own is too short to look for safely; the long
            // option only the util-linux tool has is the reliable tell.
            text.contains("--server") || text.contains("--udp")
        })
        .unwrap_or(false);
    if !usable {
        eprintln!(
            "skipping: `{}` cannot send to a network host — this is the BSD \
             `logger`, not util-linux's",
            found.display()
        );
        return None;
    }
    Some(found)
}

/// Whether the tests run as root.
#[cfg(unix)]
pub fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|meta| meta.uid() == 0)
        .unwrap_or(false)
}

/// A helper process (a real server or exporter) that is killed when the test ends, pass or fail.
pub struct Helper(Child);

impl Helper {
    pub fn spawn(command: &mut Command) -> std::io::Result<Helper> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(Helper)
    }

    pub fn id(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A UDP port on loopback that was free a moment ago.
pub fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .and_then(|socket| socket.local_addr())
        .map(|address| address.port())
        .expect("loopback has a free UDP port")
}

/// A TCP port on loopback that was free a moment ago.
pub fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .expect("loopback has a free TCP port")
}
