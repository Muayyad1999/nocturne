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
