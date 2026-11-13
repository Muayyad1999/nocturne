//! What the tests against real daemons share: a throwaway system account, a
//! scratch directory, and a way to read a connection in exact pieces.
//!
//! The account is `noctest-<pid>`; nothing else on the system is touched. A
//! small watchdog process outlives the test binary just long enough to remove
//! the account, any daemon the tests started, and the scratch directory —
//! also when a test panics or the run is killed.

#![allow(dead_code)] // each test binary uses its own part of this

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use nocturne_transport::{Connection, ConnectionReader, Output};

/// Generous bound for anything a test waits for.
pub const PATIENCE: Duration = Duration::from_secs(30);

/// The throwaway account and the scratch directory of this test binary.
pub struct Account {
    pub name: String,
    pub password: String,
    pub home: PathBuf,
    /// Root-only directory for configuration, keys, logs and PID files.
    pub scratch: PathBuf,
}

/// The account shared by the tests of this binary, created on first use.
/// `None` when this machine cannot run them; the reason is printed once.
pub fn account() -> Option<&'static Account> {
    static ACCOUNT: OnceLock<Option<Account>> = OnceLock::new();
    ACCOUNT
        .get_or_init(|| match Account::create() {
            Ok(account) => Some(account),
            Err(reason) => {
                skipped(&format!(
                    "tests against real daemons cannot run here: {reason}"
                ));
                None
            }
        })
        .as_ref()
}

impl Account {
    fn create() -> Result<Account, String> {
        if run("id", &["-u"])?.trim() != "0" {
            return Err("not running as root (needed to create a test user)".to_owned());
        }
        for tool in ["useradd", "userdel", "chpasswd"] {
            if which(tool).is_none() {
                return Err(format!("`{tool}` is not installed"));
            }
        }
        remove_stale_accounts();

        let pid = std::process::id();
        let name = format!("noctest-{pid}");
        let scratch = scratch_root().join(format!("{SCRATCH_PREFIX}{pid}"));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&scratch)
            .map_err(|error| format!("cannot create {}: {error}", scratch.display()))?;

        // The watchdog comes first, so that nothing below can be left behind.
        spawn_watchdog(pid, &name, &scratch)?;

        run("useradd", &["-m", "-s", "/bin/sh", &name])?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let password = format!("Nt-{pid}-{nanos}");
        let mut chpasswd = Command::new("chpasswd")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run chpasswd: {error}"))?;
        if let Some(mut stdin) = chpasswd.stdin.take() {
            writeln!(stdin, "{name}:{password}").map_err(|error| error.to_string())?;
        }
        if !chpasswd.wait().is_ok_and(|status| status.success()) {
            return Err("chpasswd failed".to_owned());
        }
        let home = run(
            "sh",
            &["-c", &format!("getent passwd {name} | cut -d: -f6")],
        )?;
        Ok(Account {
            name,
            password,
            home: PathBuf::from(home.trim()),
            scratch,
        })
    }

    /// Writes a file into the account's home, owned by the account.
    pub fn install(&self, relative: &str, contents: &str) {
        let path = self.home.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, contents).unwrap();
        let top = relative.split('/').next().unwrap_or(relative);
        run(
            "chown",
            &[
                "-R",
                &format!("{}:", self.name),
                &self.home.join(top).to_string_lossy(),
            ],
        )
        .unwrap();
    }

    /// A path in the scratch directory that no other test uses.
    pub fn unique_path(&self, stem: &str, extension: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let number = COUNTER.fetch_add(1, Ordering::Relaxed);
        self.scratch.join(format!("{stem}-{number}.{extension}"))
    }
}

/// Removes accounts left behind by test runs that no longer exist.
fn remove_stale_accounts() {
    let Ok(passwd) = std::fs::read_to_string("/etc/passwd") else {
        return;
    };
    for name in passwd.lines().filter_map(|line| line.split(':').next()) {
        let Some(pid) = name.strip_prefix("noctest-") else {
            continue;
        };
        if pid.parse::<u32>().is_ok() && !Path::new(&format!("/proc/{pid}")).exists() {
            let _ = run("pkill", &["-KILL", "-u", name]);
            let _ = run("userdel", &["-r", "-f", name]);
        }
    }
}

/// Where the scratch directories of all runs live, and what they are called.
fn scratch_root() -> PathBuf {
    std::env::temp_dir()
}

const SCRATCH_PREFIX: &str = "nocturne-interop-";

/// Left behind by the run that created `/run/sshd`, for the last run to
/// finish to remove it again.
fn privsep_marker() -> PathBuf {
    scratch_root().join("nocturne-interop.created-run-sshd")
}

/// sshd refuses to work without its privilege separation directory, which
/// only exists where the system's own sshd service has been started. Called
/// before every daemon start, since a previous run's watchdog may just have
/// removed it.
pub fn ensure_sshd_privsep_dir() -> Result<(), String> {
    let directory = Path::new("/run/sshd");
    if !directory.exists() {
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        std::fs::write(privsep_marker(), "").map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Starts the process that cleans up once this test binary is gone.
fn spawn_watchdog(pid: u32, account: &str, scratch: &Path) -> Result<(), String> {
    let scratch = scratch.display();
    let root = scratch_root();
    let root = root.display();
    let marker = privsep_marker();
    let marker = marker.display();
    // The privilege separation directory goes only when no other run (which
    // would have its scratch directory by now) can still be using it.
    let script = format!(
        r#"while kill -0 {pid} 2>/dev/null; do sleep 0.2; done
for file in "{scratch}"/*.pid; do
    [ -f "$file" ] && kill "$(cat "$file")" 2>/dev/null
done
sleep 0.3
pkill -KILL -u {account} 2>/dev/null
sleep 0.2
userdel -r -f {account} >/dev/null 2>&1
rm -rf "{scratch}"
if [ -f "{marker}" ] && ! ls -d "{root}"/{SCRATCH_PREFIX}[0-9]* >/dev/null 2>&1; then
    rmdir /run/sshd 2>/dev/null
    rm -f "{marker}"
fi
"#
    );
    Command::new("sh")
        .args(["-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group: a Ctrl-C aimed at the tests must not stop it.
        .process_group(0)
        .spawn()
        .map(drop)
        .map_err(|error| format!("cannot start the clean-up watchdog: {error}"))
}

/// Reports that a test (or part of one) is skipped. Written straight to the
/// standard error stream so that it is visible although the test "passes".
pub fn skipped(reason: &str) {
    let _ = writeln!(std::io::stderr(), "SKIPPED: {reason}");
}

/// Runs a command to completion; its standard output, or why it failed.
pub fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// The full path of an installed program.
pub fn which(program: &str) -> Option<PathBuf> {
    [
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
        "/usr/local/sbin",
        "/usr/local/bin",
    ]
    .iter()
    .map(|directory| Path::new(directory).join(program))
    .find(|path| path.is_file())
}

/// A TCP port on the loopback interface that is free right now.
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Writes a file only its owner can read.
pub fn write_private(path: &Path, contents: &str) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(contents.as_bytes()).unwrap();
}

/// Bounds a future that should complete promptly.
pub async fn soon<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, future)
        .await
        .expect("timed out")
}

/// Something output can be read from.
pub trait Receive {
    fn next(&mut self) -> impl Future<Output = Option<Output>>;
}

impl Receive for Connection {
    async fn next(&mut self) -> Option<Output> {
        self.recv().await
    }
}

impl Receive for ConnectionReader {
    async fn next(&mut self) -> Option<Output> {
        self.recv().await
    }
}

/// A connection (or its reading half) together with the output that was
/// received but not asked for yet, so that tests can consume the byte stream
/// in exact pieces whatever the chunking was.
pub struct Tap<S> {
    pub inner: S,
    pending: Vec<u8>,
}

/// Wraps a connection for exact reading.
pub fn tap<S: Receive>(inner: S) -> Tap<S> {
    Tap {
        inner,
        pending: Vec::new(),
    }
}

impl<S> std::ops::Deref for Tap<S> {
    type Target = S;

    fn deref(&self) -> &S {
        &self.inner
    }
}

impl<S> std::ops::DerefMut for Tap<S> {
    fn deref_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl Tap<Connection> {
    pub async fn close(self) {
        soon(self.inner.close()).await;
    }
}

impl<S: Receive> Tap<S> {
    async fn more(&mut self, waiting_for: &str) {
        match tokio::time::timeout(PATIENCE, self.inner.next()).await {
            Ok(Some(Output::Data(data))) => self.pending.extend_from_slice(&data),
            Ok(other) => panic!(
                "session ended ({other:?}) while waiting for {waiting_for}; got {}",
                preview(&self.pending)
            ),
            Err(_) => panic!(
                "timed out waiting for {waiting_for}; got {}",
                preview(&self.pending)
            ),
        }
    }

    /// Consumes the output up to and including the first `needle`.
    pub async fn until(&mut self, needle: &[u8]) -> Vec<u8> {
        loop {
            if let Some(start) = find(&self.pending, needle) {
                return self.pending.drain(..start + needle.len()).collect();
            }
            self.more(&format!("{:?}", String::from_utf8_lossy(needle)))
                .await;
        }
    }

    /// Consumes exactly `count` bytes of output.
    pub async fn exactly(&mut self, count: usize) -> Vec<u8> {
        while self.pending.len() < count {
            self.more(&format!("{count} bytes")).await;
        }
        self.pending.drain(..count).collect()
    }

    /// Consumes the rest of the session: its output and the close reason.
    /// Checks that nothing follows `Closed`.
    pub async fn until_closed(&mut self) -> (Vec<u8>, Option<String>) {
        loop {
            match tokio::time::timeout(PATIENCE, self.inner.next()).await {
                Ok(Some(Output::Data(data))) => self.pending.extend_from_slice(&data),
                Ok(Some(Output::Closed { reason })) => {
                    let after = tokio::time::timeout(PATIENCE, self.inner.next()).await;
                    assert!(
                        matches!(after, Ok(None)),
                        "nothing may follow Closed: {after:?}"
                    );
                    return (std::mem::take(&mut self.pending), reason);
                }
                Ok(None) => panic!("the stream ended without Closed"),
                Err(_) => panic!(
                    "timed out waiting for the session to close; got {}",
                    preview(&self.pending)
                ),
            }
        }
    }
}

/// The end of what was received, for failure messages.
fn preview(seen: &[u8]) -> String {
    const TAIL: usize = 400;
    let tail = &seen[seen.len().saturating_sub(TAIL)..];
    format!(
        "{} bytes ending in {:?}",
        seen.len(),
        String::from_utf8_lossy(tail)
    )
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len().max(1))
        .position(|window| window == needle)
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}
