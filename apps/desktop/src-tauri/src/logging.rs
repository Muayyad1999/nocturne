//! Logs to stderr and to a file in the data directory.
//!
//! Nothing here may record what a method was asked or what it answered: RPC
//! parameters and results carry device configurations and, briefly, secrets.
//! Method names and outcomes are enough to follow what happened.
//!
//! The file comes later than the logging. A launch does not know yet whether
//! it is the one that stays: a second launch hands over to the first and
//! ends, and until that is settled it must not write into the data directory
//! — not even a log. What is logged before then is kept in memory and goes
//! into the file when [`Logs::open_file`] is called, so the file still tells
//! the start from the beginning.

use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

/// How much is kept for a file that is not open yet. A start logs a few
/// lines; this is only so that a file which never opens cannot grow a
/// buffer for ever.
const HELD_MAX: usize = 256 * 1024;

/// The log file's name in `<data_dir>/logs`; older ones are `nocturne.1.log`
/// (the most recent) to `nocturne.4.log`.
const FILE_NAME: &str = "nocturne";
/// How many files are kept, the current one included.
const KEPT: usize = 5;
/// How big one file may grow before the next one is started: five of them
/// are 50 MiB at most, however long `--log-level trace` runs. Days went by
/// size before — one file a day, unbounded, `nocturne.<date>.log` — which
/// is neither a limit nor a name anybody can be asked to send.
const FILE_MAX: u64 = 10 * 1024 * 1024;

/// `nocturne.log`, moved along to `nocturne.1.log` and on as it fills, the
/// oldest deleted. Written to by the file's worker thread only, a line at a
/// time, so a line is never split between two files.
struct Rotating {
    folder: std::path::PathBuf,
    file: Option<std::fs::File>,
    written: u64,
    cap: u64,
}

impl Rotating {
    fn open(folder: &Path, cap: u64) -> io::Result<Rotating> {
        std::fs::create_dir_all(folder)?;
        let mut rotating = Rotating {
            folder: folder.to_path_buf(),
            file: None,
            written: 0,
            cap,
        };
        rotating.reopen()?;
        Ok(rotating)
    }

    fn path(&self, generation: usize) -> std::path::PathBuf {
        match generation {
            0 => self.folder.join(format!("{FILE_NAME}.log")),
            n => self.folder.join(format!("{FILE_NAME}.{n}.log")),
        }
    }

    /// Append to the current file, which another start may have begun.
    fn reopen(&mut self) -> io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(0))?;
        self.written = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Closed first: Windows renames no file that is open here.
        self.file = None;
        let _ = std::fs::remove_file(self.path(KEPT - 1));
        for generation in (0..KEPT - 1).rev() {
            let from = self.path(generation);
            if from.exists() {
                std::fs::rename(&from, self.path(generation + 1))?;
            }
        }
        self.reopen()
    }
}

impl Write for Rotating {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.written > 0 && self.written + bytes.len() as u64 > self.cap {
            // A file that cannot be moved along is written on rather than
            // lost: going over the size beats a log that stops.
            if self.rotate().is_err() && self.file.is_none() {
                self.reopen()?;
            }
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => return Err(io::Error::other("the log file is not open")),
        };
        file.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

/// The log file, before and after there is one.
enum File {
    /// Not open yet: what has been logged so far.
    Awaited(Vec<u8>),
    Open(NonBlocking),
    /// There is not going to be one, or it has been closed.
    None,
}

/// Where the file layer writes. Cheap to clone: every clone is the same file.
#[derive(Clone)]
struct LateFile(Arc<Mutex<File>>);

impl Write for LateFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match &mut *self.0.lock() {
            File::Awaited(held) => {
                if held.len() + bytes.len() <= HELD_MAX {
                    held.extend_from_slice(bytes);
                }
            }
            File::Open(file) => return file.write(bytes),
            File::None => {}
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut *self.0.lock() {
            File::Open(file) => file.flush(),
            File::Awaited(_) | File::None => Ok(()),
        }
    }
}

impl<'a> MakeWriter<'a> for LateFile {
    type Writer = LateFile;

    fn make_writer(&'a self) -> LateFile {
        self.clone()
    }
}

/// The running logs. Lives as long as the program; [`Logs::close`] is how it
/// ends, because the program ends with `std::process::exit`, which runs no
/// destructor.
pub struct Logs {
    file: LateFile,
    /// Dropping it is what writes out what the file's worker still holds.
    worker: Mutex<Option<WorkerGuard>>,
}

/// Start logging to the terminal, for the whole process. The file is opened
/// with [`Logs::open_file`] once this launch is known to be the one that
/// stays.
pub fn start(level: Option<&str>) -> Logs {
    let (logs, subscriber) = build(level);
    subscriber.init();
    logs
}

/// The logs and the subscriber that feeds them, not yet installed anywhere.
pub(crate) fn build(
    level: Option<&str>,
) -> (Logs, impl tracing::Subscriber + Send + Sync + 'static) {
    let filter = || {
        EnvFilter::try_from_env("NOCTURNE_LOG")
            .unwrap_or_else(|_| EnvFilter::new(level.unwrap_or("info")))
    };
    let file = LateFile(Arc::new(Mutex::new(File::Awaited(Vec::new()))));
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(io::stderr)
                .with_filter(filter()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file.clone())
                .with_ansi(false)
                .with_filter(filter()),
        );
    let logs = Logs {
        file,
        worker: Mutex::new(None),
    };
    (logs, subscriber)
}

impl Logs {
    /// Open the log file under the data directory and write into it what was
    /// logged before it existed. If it cannot be opened the terminal is all
    /// there is, and that is said there.
    pub fn open_file(&self, data_dir: &Path) {
        self.open_file_capped(data_dir, FILE_MAX)
    }

    fn open_file_capped(&self, data_dir: &Path, cap: u64) {
        let mut file = self.file.0.lock();
        if matches!(*file, File::Open(_)) {
            return;
        }
        let logs = data_dir.join("logs");
        let appender = Rotating::open(&logs, cap).map_err(|err| err.to_string());
        match appender {
            Ok(appender) => {
                let (mut writer, worker) = tracing_appender::non_blocking(appender);
                if let File::Awaited(held) = &*file {
                    let _ = writer.write_all(held);
                }
                *file = File::Open(writer);
                *self.worker.lock() = Some(worker);
            }
            Err(err) => {
                *file = File::None;
                // Not while holding the file: this line is on its way there too.
                drop(file);
                tracing::warn!(%err, path = %logs.display(), "no log file: logging to the terminal only");
            }
        }
    }

    /// Write out everything that is still on its way to the file. Call it
    /// before every exit: the last lines of a start that failed are the ones
    /// somebody will look for, and they are the ones still in the queue.
    pub fn close(&self) {
        *self.file.0.lock() = File::None;
        // The worker writes what it holds when it is dropped.
        drop(self.worker.lock().take());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn everything_in(folder: &Path) -> String {
        let mut text = String::new();
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        text
    }

    #[test]
    fn what_is_logged_before_the_file_exists_is_in_it_once_it_does_and_nothing_is_lost_at_the_end()
    {
        let data_dir = tempfile::tempdir().unwrap();
        let (logs, subscriber) = build(Some("info"));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("before there is a file");
            assert!(
                !data_dir.path().join("logs").exists(),
                "logging alone must not write into the data directory"
            );

            logs.open_file(data_dir.path());
            tracing::error!("the last thing said before the exit");
            // No wait: an exit follows a failure at once, and the line has
            // to be in the file anyway.
            logs.close();
        });

        let written = everything_in(&data_dir.path().join("logs"));
        let before = written.find("before there is a file");
        let last = written.find("the last thing said before the exit");
        assert!(before.is_some() && last.is_some(), "{written}");
        assert!(before < last, "out of order:\n{written}");
        assert_eq!(written.matches("before there is a file").count(), 1);
    }

    /// `close` only helps where it is called, and from outside that cannot
    /// be told: with the call gone the lines are lost only when the process
    /// happens to end before the file's worker has had its turn — which is
    /// how a start that failed came to leave nothing in the log. So it is
    /// read from the source: the program ends by itself after
    /// `RunEvent::Exit`, or by an exit of its own, and closes the log first
    /// either way.
    #[test]
    fn the_log_is_closed_wherever_the_program_ends() {
        let shell = include_str!("lib.rs");
        assert!(
            shell.contains("RunEvent::Exit => logs.close()"),
            "the event loop ends the process right after `RunEvent::Exit`: \
             that is the last moment at which the log can be written out"
        );
        let lines: Vec<&str> = shell.lines().map(str::trim).collect();
        for (at, line) in lines.iter().enumerate() {
            if line.contains("process::exit(") {
                assert_eq!(
                    lines[at.saturating_sub(1)],
                    "logs.close();",
                    "lib.rs:{}: an exit that leaves the last lines of the log behind",
                    at + 1
                );
            }
        }
    }

    /// What somebody is asked to send is `logs/nocturne.log`, and a day of
    /// `--log-level trace` must not fill the disk on the way.
    #[test]
    fn the_log_is_nocturne_log_and_five_files_of_a_bounded_size_are_kept() {
        let data_dir = tempfile::tempdir().unwrap();
        let cap = 4 * 1024;
        let (logs, subscriber) = build(Some("info"));
        tracing::subscriber::with_default(subscriber, || {
            logs.open_file_capped(data_dir.path(), cap);
            for line in 0..2_000 {
                tracing::info!(line, "a line long enough to fill a small file quickly");
            }
            tracing::info!("the last line");
            logs.close();
        });

        let folder = data_dir.path().join("logs");
        let mut names: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "nocturne.1.log",
                "nocturne.2.log",
                "nocturne.3.log",
                "nocturne.4.log",
                "nocturne.log"
            ]
        );
        for name in &names {
            let size = std::fs::metadata(folder.join(name)).unwrap().len();
            assert!(size <= cap, "{name} is {size} bytes, over {cap}");
        }
        let newest = std::fs::read_to_string(folder.join("nocturne.log")).unwrap();
        assert!(newest.contains("the last line"), "{newest}");
        // The oldest went, rather than the newest being refused.
        assert!(!everything_in(&folder).contains("line=0\n"));
    }

    #[test]
    fn a_launch_that_never_opens_the_file_leaves_nothing_in_the_data_directory() {
        let data_dir = tempfile::tempdir().unwrap();
        let (logs, subscriber) = build(Some("info"));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("a launch that hands over to the first");
            logs.close();
            tracing::info!("and anything after that");
        });
        assert_eq!(std::fs::read_dir(data_dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_folder_that_cannot_hold_a_log_file_does_not_stop_the_logging() {
        let data_dir = tempfile::tempdir().unwrap();
        // A file where the `logs` folder would go.
        std::fs::write(data_dir.path().join("logs"), b"").unwrap();
        let (logs, subscriber) = build(Some("info"));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("before");
            logs.open_file(data_dir.path());
            tracing::info!("after");
            logs.close();
        });
    }
}
