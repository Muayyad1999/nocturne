//! Local shells in a pseudo-terminal.

use super::support::soon;
use crate::{LocalParams, connect_local};

#[tokio::test]
async fn a_missing_program_is_an_error() {
    let params = LocalParams {
        program: Some("/nonexistent/nocturne-no-such-program".to_owned()),
        ..LocalParams::default()
    };
    let error = soon(connect_local(&params)).await.unwrap_err();
    assert_eq!(error.code(), "io", "{error}");
}

#[tokio::test]
async fn a_missing_working_directory_is_an_error() {
    let params = LocalParams {
        cwd: Some("/nonexistent/nocturne-no-such-dir".to_owned()),
        ..LocalParams::default()
    };
    let error = soon(connect_local(&params)).await.unwrap_err();
    assert_eq!(error.code(), "io", "{error}");
    assert!(error.to_string().contains("not a directory"), "{error}");
}

#[cfg(unix)]
mod unix {
    use std::path::Path;
    use std::time::Duration;

    use super::super::support::{Tap, contains, soon, tap};
    use crate::{Connection, LocalParams, TransportKind, connect_local};

    fn sh(script: &str) -> LocalParams {
        LocalParams {
            program: Some("/bin/sh".to_owned()),
            args: vec!["-c".to_owned(), script.to_owned()],
            ..LocalParams::default()
        }
    }

    async fn run(params: &LocalParams) -> Tap<Connection> {
        tap(soon(connect_local(params)).await.unwrap())
    }

    #[tokio::test]
    async fn printf_output_and_exit_status() {
        let mut conn = run(&sh("printf 'hello %s\\n' world; exit 3")).await;
        assert_eq!(conn.info().kind, Some(TransportKind::Local));
        assert_eq!(conn.info().peer, "/bin/sh");
        let (output, reason) = conn.until_closed().await;
        // The terminal turns the line feed into CR LF.
        assert_eq!(output, b"hello world\r\n");
        assert_eq!(reason.as_deref(), Some("exit status 3"));
    }

    #[tokio::test]
    async fn echo_runs_directly() {
        let params = LocalParams {
            program: Some("echo".to_owned()),
            args: vec!["one".to_owned(), "two".to_owned()],
            ..LocalParams::default()
        };
        let (output, reason) = run(&params).await.until_closed().await;
        assert_eq!(output, b"one two\r\n");
        assert_eq!(reason.as_deref(), Some("exit status 0"));
    }

    #[tokio::test]
    async fn interactive_round_trip() {
        let params = LocalParams {
            program: Some("/bin/sh".to_owned()),
            env: vec![("PS1".to_owned(), "nocturne$ ".to_owned())],
            ..LocalParams::default()
        };
        let mut conn = run(&params).await;
        conn.until(b"nocturne$ ").await;
        conn.send("echo $((6 * 7))\n").await.unwrap();
        // First the terminal's echo of what was typed, then the answer.
        conn.until(b"echo $((6 * 7))").await;
        conn.until(b"42\r\n").await;
        conn.until(b"nocturne$ ").await;
        conn.send("exit 7\n").await.unwrap();
        let (_, reason) = conn.until_closed().await;
        assert_eq!(reason.as_deref(), Some("exit status 7"));
    }

    #[tokio::test]
    async fn the_terminal_has_the_requested_size_and_can_be_resized() {
        let params = LocalParams {
            cols: 132,
            rows: 50,
            ..sh("stty size; read line; stty size")
        };
        let mut conn = run(&params).await;
        conn.until(b"50 132\r\n").await;
        conn.resize(100, 30).await.unwrap();
        conn.send("\n").await.unwrap();
        conn.until(b"30 100\r\n").await;
        let (_, reason) = conn.until_closed().await;
        assert_eq!(reason.as_deref(), Some("exit status 0"));
    }

    #[tokio::test]
    async fn environment_and_working_directory_are_applied() {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().canonicalize().unwrap();
        let params = LocalParams {
            cwd: Some(cwd.to_string_lossy().into_owned()),
            env: vec![("NOCTURNE_TEST".to_owned(), "from the test".to_owned())],
            ..sh("echo \"$NOCTURNE_TEST|$TERM|$(pwd -P)\"")
        };
        let (output, _) = run(&params).await.until_closed().await;
        let expected = format!("from the test|xterm-256color|{}\r\n", cwd.display());
        assert_eq!(String::from_utf8_lossy(&output), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn large_output_arrives_complete() {
        const TOTAL: usize = 3_000_000;
        let script = format!("head -c {TOTAL} /dev/zero | tr '\\0' 'x'; echo; echo done");
        let (output, reason) = run(&sh(&script)).await.until_closed().await;
        assert_eq!(reason.as_deref(), Some("exit status 0"));
        assert_eq!(output.len(), TOTAL + "\r\ndone\r\n".len());
        assert!(output[..TOTAL].iter().all(|&byte| byte == b'x'));
        assert!(output.ends_with(b"\r\ndone\r\n"));
    }

    /// Whether a process with this id exists (and is not a zombie).
    fn is_running(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .is_ok_and(|stat| !stat.contains(") Z "))
    }

    #[tokio::test]
    async fn closing_hangs_up_on_the_program() {
        // `exec` keeps the process id while turning the shell into `sleep`.
        let mut conn = run(&sh("echo pid=$$.; exec sleep 600")).await;
        let seen = conn.until(b".").await;
        let text = String::from_utf8_lossy(&seen);
        let pid: u32 = text
            .trim()
            .trim_start_matches("pid=")
            .trim_end_matches('.')
            .parse()
            .unwrap_or_else(|_| panic!("no pid in {text:?}"));
        if Path::new("/proc").is_dir() {
            assert!(is_running(pid));
        }

        let started = std::time::Instant::now();
        conn.close().await;
        assert!(started.elapsed() < Duration::from_secs(5));
        if Path::new("/proc").is_dir() {
            for _ in 0..100 {
                if !is_running(pid) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("process {pid} survived the close");
        }
    }

    #[tokio::test]
    async fn dropping_the_connection_hangs_up_too() {
        let mut conn = run(&sh("echo pid=$$.; exec sleep 600")).await;
        let seen = conn.until(b".").await;
        let text = String::from_utf8_lossy(&seen);
        let pid: u32 = text
            .trim()
            .trim_start_matches("pid=")
            .trim_end_matches('.')
            .parse()
            .unwrap();
        drop(conn);
        if Path::new("/proc").is_dir() {
            for _ in 0..200 {
                if !is_running(pid) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            panic!("process {pid} survived the drop");
        }
    }

    #[tokio::test]
    async fn a_local_close_has_no_reason() {
        let (writer, reader) = run(&sh("echo ready; sleep 600")).await.inner.split();
        let mut reader = tap(reader);
        reader.until(b"ready").await;
        writer.close().await;
        let (_, reason) = reader.until_closed().await;
        assert_eq!(reason, None);
    }

    #[tokio::test]
    async fn break_is_not_available() {
        let mut conn = run(&sh("echo ready; sleep 600")).await;
        conn.until(b"ready").await;
        assert!(!conn.supports_break());
        assert_eq!(conn.send_break().await.unwrap_err().code(), "unsupported");
        conn.close().await;
    }

    #[tokio::test]
    async fn the_default_shell_starts() {
        let mut conn = run(&LocalParams::default()).await;
        // Whatever the user's shell is, it understands this.
        conn.send("echo nocturne-$((40 + 2))\n").await.unwrap();
        conn.until(b"nocturne-42").await;
        assert!(contains(conn.info().peer.as_bytes(), b"sh"));
        conn.close().await;
    }

    #[tokio::test]
    async fn killed_programs_are_reported() {
        let mut conn = run(&sh("echo ready; kill -TERM $$; sleep 5")).await;
        let (_, reason) = conn.until_closed().await;
        let reason = reason.unwrap();
        assert!(reason.contains("terminated by signal"), "{reason}");
    }
}
