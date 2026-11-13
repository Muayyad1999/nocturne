//! Running the operating system's own network tools (`ping`, `traceroute`, `ip`, `arp`, …).
//!
//! Programs are always spawned directly with an argument vector — never through a shell — and
//! with a C locale where the platform honours it, so their output is in the documented format.

use std::io;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// Directories searched after `PATH`: desktop sessions often leave the `sbin` directories out,
/// and that is where `traceroute`, `arp` and older `ip` builds live.
#[cfg(unix)]
const EXTRA_DIRS: &[&str] = &["/usr/sbin", "/sbin", "/usr/local/sbin", "/usr/bin", "/bin"];

/// Finds `program` in `PATH` or the usual system directories.
pub(crate) fn find(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    #[cfg(unix)]
    dirs.extend(EXTRA_DIRS.iter().map(PathBuf::from));
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{program}.exe"), program.to_owned()]
    } else {
        vec![program.to_owned()]
    };
    dirs.iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// A command for `program` with no stdin, piped output, and killed when dropped.
///
/// Returns `None` when the program is not installed.
pub(crate) fn command(program: &str) -> Option<Command> {
    let mut command = Command::new(find(program)?);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.env("LC_ALL", "C").env("LANG", "C");
    // CREATE_NO_WINDOW: a GUI application must not flash a console for every probe.
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    Some(command)
}

/// What a finished program printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Output {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

impl Output {
    /// stdout followed by stderr.
    pub(crate) fn combined(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

#[derive(Debug)]
pub(crate) enum RunError {
    /// The program is not installed.
    Missing,
    /// It did not finish within the limit and was killed.
    TimedOut,
    Io(io::Error),
}

/// Runs `program` to completion, killing it if it exceeds `limit` (or if the future is dropped).
pub(crate) async fn run(program: &str, args: &[&str], limit: Duration) -> Result<Output, RunError> {
    let mut command = command(program).ok_or(RunError::Missing)?;
    command.args(args);
    let child = command.spawn().map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => RunError::Missing,
        _ => RunError::Io(error),
    })?;
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Err(_) => Err(RunError::TimedOut),
        Ok(Err(error)) => Err(RunError::Io(error)),
        Ok(Ok(output)) => Ok(Output {
            // Console tools on Windows print in the OEM code page; the parsers only rely on the
            // ASCII parts, which survive a lossy decode.
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            success: output.status.success(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_programs_are_reported_as_missing() {
        assert!(find("nocturne-no-such-program-7f3a").is_none());
        assert!(command("nocturne-no-such-program-7f3a").is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_a_program_and_captures_its_output() {
        let output = run("sh", &["-c", "echo out; echo err >&2; exit 3"], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "err\n");
        assert!(!output.success);
        assert_eq!(output.combined(), "out\n\nerr\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_program_that_overruns_is_killed() {
        let started = std::time::Instant::now();
        let result = run("sleep", &["30"], Duration::from_millis(100)).await;
        assert!(matches!(result, Err(RunError::TimedOut)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn arguments_are_never_interpreted_by_a_shell() {
        // If a shell were involved, `$(...)` and `;` would be expanded.
        let output = run("echo", &["$(id); echo injected"], Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(output.stdout, "$(id); echo injected\n");
    }
}
