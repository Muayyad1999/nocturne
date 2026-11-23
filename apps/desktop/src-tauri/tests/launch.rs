//! The real program, started more than once.
//!
//! Everything here is about what happens *between* processes — who gets the
//! data directory, who comes forward, what the person is told — and none of
//! it can be seen from inside one. So these start the built binary, on a
//! display, and look at its windows.
//!
//! They are ignored by default, because they need what a build machine may
//! not have: a display, a session bus (that is how a second launch finds the
//! first on Linux) and `xdotool` to see and press things. Run them with
//!
//! ```bash
//! xvfb-run -a dbus-run-session -- cargo test -p nocturne-desktop --test launch -- --ignored
//! ```
//!
//! Linux only: the questions are the same on Windows and macOS, the way of
//! asking them is not.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_nocturne");
const IGNORED: &str = "needs a display, a session bus and xdotool: \
     xvfb-run -a dbus-run-session -- cargo test -p nocturne-desktop --test launch -- --ignored";

/// A started program is slow on a busy machine; a window that is not coming
/// is not coming after this either.
const STARTS_WITHIN: Duration = Duration::from_secs(180);
const SOON: Duration = Duration::from_secs(30);

const MAIN_WINDOW: &str = "^Nocturne$";
const CANNOT_START: &str = "^Nocturne cannot start$";
const ALREADY_OPEN: &str = "^Nocturne is already open$";

/// One instance per identifier per session bus: two of these at once would
/// each be the other's "first launch".
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A place for everything one test's programs write, and the programs.
struct Stage {
    home: tempfile::TempDir,
    running: Vec<Child>,
    _alone: std::sync::MutexGuard<'static, ()>,
}

impl Stage {
    fn new() -> Stage {
        let alone = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for needed in ["DISPLAY", "DBUS_SESSION_BUS_ADDRESS"] {
            assert!(
                std::env::var_os(needed).is_some(),
                "{needed} is not set — {IGNORED}"
            );
        }
        assert!(
            Command::new("xdotool")
                .arg("version")
                .output()
                .is_ok_and(|out| out.status.success()),
            "xdotool is not installed — {IGNORED}"
        );
        assert_eq!(
            windows(MAIN_WINDOW),
            Vec::<String>::new(),
            "a Nocturne window is already on this display; these tests need one of their own"
        );
        Stage {
            home: tempfile::tempdir().expect("a temporary directory"),
            running: Vec::new(),
            _alone: alone,
        }
    }

    fn folder(&self, name: &str) -> PathBuf {
        self.home.path().join(name)
    }

    /// Start the program on a data directory. Its own files — the WebView's
    /// data, the remembered window position — go under the stage, not into
    /// the home directory of whoever runs the tests.
    fn launch(&mut self, data_dir: &Path) -> usize {
        let home = self.home.path();
        let child = Command::new(BIN)
            .arg("--data-dir")
            .arg(data_dir)
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("share"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env_remove("NOCTURNE_DATA_DIR")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the built program starts");
        self.running.push(child);
        self.running.len() - 1
    }

    /// The exit status of a program, once it has one.
    fn exit_of(&mut self, which: usize, within: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.running[which]
                .try_wait()
                .expect("a child to ask about")
            {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn is_running(&mut self, which: usize) -> bool {
        self.running[which]
            .try_wait()
            .expect("a child to ask about")
            .is_none()
    }

    /// A window that program is expected to show. A program that has ended
    /// is not going to show one, so the wait ends with it.
    fn window_from(&mut self, which: usize, title: &str, within: Duration) -> Option<String> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(id) = windows(title).into_iter().next() {
                return Some(id);
            }
            if !self.is_running(which) || Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        for child in &mut self.running {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The engine, holding a data directory in this process.
struct Holder {
    engine: nocturne_suite::App,
    runtime: Option<tokio::runtime::Runtime>,
}

impl Holder {
    fn of(data_dir: &Path) -> Holder {
        let runtime = tokio::runtime::Runtime::new().expect("a runtime");
        let engine = runtime.block_on(async {
            nocturne_suite::start(nocturne_suite::AppConfig::at(data_dir))
                .expect("the engine starts")
        });
        Holder {
            engine,
            runtime: Some(runtime),
        }
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        self.engine.shutdown();
        // Not a plain drop, which waits for every blocking task for ever —
        // and a failed assertion would then hang instead of failing.
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// The visible windows whose title matches.
fn windows(title: &str) -> Vec<String> {
    let found = Command::new("xdotool")
        .args(["search", "--onlyvisible", "--name", title])
        .output()
        .expect("xdotool runs");
    String::from_utf8_lossy(&found.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// The first visible window with that title, waiting for one to appear.
fn window(title: &str, within: Duration) -> Option<String> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(id) = windows(title).into_iter().next() {
            return Some(id);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn xdotool(arguments: &[&str]) {
    let status = Command::new("xdotool")
        .args(arguments)
        .status()
        .expect("xdotool runs");
    assert!(status.success(), "xdotool {arguments:?}");
}

/// Everything in the folder's log files.
fn logged(data_dir: &Path) -> String {
    let mut text = String::new();
    if let Ok(entries) = std::fs::read_dir(data_dir.join("logs")) {
        for entry in entries.flatten() {
            text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    text
}

fn entries(folder: &Path) -> Vec<String> {
    match std::fs::read_dir(folder) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
#[ignore = "needs a display, a session bus and xdotool; see the top of this file"]
fn a_second_launch_brings_the_first_window_forward_and_ends_well() {
    let mut stage = Stage::new();
    let data = stage.folder("data");

    let first = stage.launch(&data);
    let first = stage
        .window_from(first, MAIN_WINDOW, STARTS_WITHIN)
        .expect("the first launch opens its window");
    // Out of sight, as a minimised window or one behind others is.
    xdotool(&["windowunmap", &first]);
    assert_eq!(windows(MAIN_WINDOW), Vec::<String>::new());

    // What a second click on the launcher does: the same program, the same
    // data directory.
    let second = stage.launch(&data);
    let status = stage
        .exit_of(second, STARTS_WITHIN)
        .expect("the second launch hands over and ends");
    assert_eq!(
        status.code(),
        Some(0),
        "handing over to the copy that is running is not a failure"
    );
    assert!(
        window(MAIN_WINDOW, SOON).is_some(),
        "the window of the copy that was already running did not come back"
    );
    assert_eq!(
        windows(CANNOT_START),
        Vec::<String>::new(),
        "nothing went wrong, so nothing should say so"
    );
    assert!(
        logged(&data).contains("started again"),
        "the copy that stayed did not record that it was asked to come forward:\n{}",
        logged(&data)
    );
}

#[test]
#[ignore = "needs a display, a session bus and xdotool; see the top of this file"]
fn a_second_launch_for_another_folder_leaves_it_untouched_and_the_person_is_told_which_is_open() {
    let mut stage = Stage::new();
    let open = stage.folder("open");
    let other = stage.folder("other");

    let first = stage.launch(&open);
    stage
        .window_from(first, MAIN_WINDOW, STARTS_WITHIN)
        .expect("the first launch opens its window");

    let second = stage.launch(&other);
    let status = stage
        .exit_of(second, STARTS_WITHIN)
        .expect("the second launch hands over and ends");
    assert_eq!(status.code(), Some(0));
    assert_eq!(
        entries(&other),
        Vec::<String>::new(),
        "a launch that only handed over made itself a database, a vault key or a log here"
    );
    assert!(
        window(ALREADY_OPEN, SOON).is_some(),
        "the window that came forward shows another folder than the one asked for, \
         and nothing said so"
    );
}

#[test]
#[ignore = "needs a display, a session bus and xdotool; see the top of this file"]
fn a_data_directory_somebody_else_holds_is_said_in_a_dialog_and_in_the_log_and_ends_in_failure() {
    let mut stage = Stage::new();
    let data = stage.folder("data");

    // Whoever it is — `nocturne-server`, or the other edition, which keeps
    // its data in the same place — it is not a copy this program can hand
    // over to. Here it is the engine itself, in this process.
    let _holder = Holder::of(&data);

    let launched = stage.launch(&data);
    let dialog = stage.window_from(launched, CANNOT_START, STARTS_WITHIN);
    assert!(
        dialog.is_some(),
        "no dialog said why Nocturne did not start (still running: {})",
        stage.is_running(launched)
    );
    assert_eq!(
        windows(MAIN_WINDOW),
        Vec::<String>::new(),
        "an interface window was opened with no engine behind it"
    );
    assert!(
        stage.is_running(launched),
        "the program ended before anybody could have read the dialog"
    );
    let log = logged(&data);
    assert!(
        log.contains("the backend did not start"),
        "the failure is not in the log file:\n{log}"
    );

    // Read, and dismissed.
    let dialog = dialog.expect("checked above");
    xdotool(&["windowfocus", "--sync", &dialog]);
    xdotool(&["key", "Return"]);
    let status = stage
        .exit_of(launched, SOON)
        .expect("the program ends when the dialog is dismissed");
    assert_eq!(status.code(), Some(1), "not starting is a failure");
    // The process is gone, so this is everything it will ever write, and
    // the last thing it said is in it. (Seen from here the line can only be
    // missed when the timing is unlucky; that it is never left behind is
    // `Logs::close`, tested in `src/logging.rs`.)
    let log = logged(&data);
    assert!(
        log.contains("stopping"),
        "the end of the log was lost with the process:\n{log}"
    );
}

#[test]
#[ignore = "needs a display, a session bus and xdotool; see the top of this file"]
fn a_launch_that_could_not_start_does_not_answer_for_the_one_after_it() {
    let mut stage = Stage::new();
    let data = stage.folder("data");
    let _holder = Holder::of(&data);

    let first = stage.launch(&data);
    stage
        .window_from(first, CANNOT_START, STARTS_WITHIN)
        .expect("the first launch says why it cannot start");

    // That dialog is easily behind something, and the obvious thing to do
    // about a program that did not appear is to start it again. The copy
    // that is nothing but a dialog must not be the one a new launch hands
    // over to: the new launch would end well with nothing on the screen,
    // which is the silent start all over again — and it would do so even
    // after whatever held the data directory had been closed.
    let second = stage.launch(&data);
    let deadline = Instant::now() + STARTS_WITHIN;
    let said_so = loop {
        if windows(CANNOT_START).len() >= 2 {
            break true;
        }
        if !stage.is_running(second) || Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    assert!(
        said_so,
        "the second launch did not start and did not say why (it ended with {:?}): \
         it handed over to a copy that has no window to bring forward",
        stage.exit_of(second, Duration::ZERO)
    );
    assert!(
        stage.is_running(second),
        "the second launch ended before anybody could have read its dialog"
    );
}
