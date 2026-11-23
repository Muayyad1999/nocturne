//! The Nocturne desktop application.
//!
//! A thin shell: it starts the backend in this process, shows the interface
//! in the system WebView, and connects the two with one command (`rpc`) and
//! one event channel. No product logic lives here — the same backend answers
//! the same way when Nocturne runs as a server.

#[cfg(test)]
mod capabilities;
pub mod data_dir;
#[cfg(test)]
mod editions;
pub mod events;
pub mod logging;
pub mod navigation;
pub mod notify;
pub mod rpc;
pub mod startup;
pub mod stopping;
pub mod tray;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nocturne_suite::{App, AppError};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_notification::NotificationExt;

/// The window the interface lives in.
const MAIN_WINDOW: &str = "main";
/// Drawn before the interface paints, so starting does not flash white.
const BACKGROUND: &str = "#090d18";

/// Everything the commands need.
struct State {
    app: App,
    /// Whether the window is in front, as its own focus events last said.
    focused: AtomicBool,
    /// What closing the window does.
    keep_running: Arc<tray::KeepRunning>,
}

#[tauri::command]
async fn rpc(
    state: tauri::State<'_, Arc<State>>,
    method: String,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, AppError> {
    rpc::call(&state.app, &method, params).await
}

/// Emits to every window.
struct Windows(tauri::AppHandle);

impl events::Windows for Windows {
    fn emit(&self, payload: &events::Payload) {
        if let Err(err) = self.0.emit(events::CHANNEL, payload) {
            tracing::warn!(%err, topic = %payload.topic, "an event could not be delivered to the interface");
        }
    }
}

/// Start the application. Returns only for `--help`, `--version` and
/// arguments it refuses; otherwise the process ends inside the event loop.
pub fn run() -> ExitCode {
    let arguments = data_dir::parse_arguments(std::env::args().skip(1));
    let program = data_dir::program_name();
    if arguments.help {
        say(&program, &data_dir::usage(&program), false);
        return ExitCode::SUCCESS;
    }
    if !arguments.problems.is_empty() {
        say(
            &program,
            &data_dir::refusal(&program, &arguments.problems),
            true,
        );
        return ExitCode::from(2);
    }
    if arguments.version {
        say(&program, &data_dir::version(&program), false);
        return ExitCode::SUCCESS;
    }

    // To the terminal only, for now: the log file is in the data directory,
    // and this launch does not own that yet (see `startup`).
    let logs = Arc::new(logging::start(arguments.log_level.as_deref()));
    let wanted = data_dir::resolve(
        arguments.data_dir.as_deref(),
        std::env::var(data_dir::ENV).ok(),
    )
    .map_err(|err| format!("{err:#}"));

    // The backend owns background tasks, so it needs a runtime that outlives
    // this function; Tauri is told to use the same one.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => fail_without_a_window(&logs, &wanted, &format!("no async runtime: {err}")),
    };
    tauri::async_runtime::set(runtime.handle().clone());

    let builder = tauri::Builder::default()
        // First, and before anything that touches the data directory: a
        // plugin is set up when the application is built, in the order
        // given, and this one's setup is where a second launch finds the
        // first, hands over to it and ends. With the engine started any
        // earlier, a second launch meets the first one's lock on the data
        // directory instead, and gives up where nobody is looking.
        .plugin(tauri_plugin_single_instance::init({
            let open = wanted.clone().ok();
            move |handle, arguments, cwd| started_again(handle, &arguments, &cwd, open.as_deref())
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(tauri::generate_handler![rpc])
        .on_menu_event(|handle, event| tray::on_menu(handle, &event))
        .setup({
            let logs = logs.clone();
            let runtime = runtime.handle().clone();
            let wanted = wanted.clone();
            move |app| {
                // The event loop is running and this is the only copy: the
                // first moment at which the data directory may be taken, and
                // at which a dialog can be shown if it cannot.
                start(app.handle(), &runtime, wanted, &logs);
                Ok(())
            }
        });
    // A window system that cannot be reached is not an `Err` from `build`:
    // GTK without a display panics in there. Unwound, it ends as any other
    // start that cannot have a window — said in the terminal and in the
    // log file, with exit code 1 — rather than as a bare panic and 101.
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        builder.build(tauri::generate_context!())
    }));
    let app = match built {
        Ok(Ok(app)) => app,
        Ok(Err(err)) => fail_without_a_window(&logs, &wanted, &err.to_string()),
        Err(panic) => fail_without_a_window(
            &logs,
            &wanted,
            &format!(
                "the window system could not be started ({}); is there a display?",
                panic_text(panic.as_ref())
            ),
        ),
    };
    app.run(move |handle, event| match event {
        RunEvent::ExitRequested { .. } => {
            tracing::info!("stopping");
            if let Some(state) = handle.try_state::<Arc<State>>() {
                let left =
                    tauri::async_runtime::block_on(stopping::stop(&state.app, stopping::GRACE));
                if !left.is_empty() {
                    tracing::warn!(
                        jobs = ?left,
                        "quitting without waiting any longer for work that is still running"
                    );
                }
            }
        }
        // The Dock icon of a running Nocturne whose window is hidden.
        #[cfg(target_os = "macos")]
        RunEvent::Reopen {
            has_visible_windows: false,
            ..
        } => {
            bring_forward(handle);
        }
        // The last thing that happens: the process is exited from inside
        // the event loop, and nothing after `run` is ever reached.
        RunEvent::Exit => logs.close(),
        _ => {}
    });
    ExitCode::SUCCESS
}

/// Start the engine and open the window on it — or say why not.
fn start(
    handle: &AppHandle,
    runtime: &tokio::runtime::Handle,
    wanted: Result<PathBuf, String>,
    logs: &logging::Logs,
) {
    let app = match startup::engine(runtime, wanted, logs) {
        Ok(app) => app,
        Err(reason) => return cannot_start(handle, &reason),
    };
    let state = Arc::new(State {
        app,
        focused: AtomicBool::new(false),
        keep_running: Arc::new(tray::KeepRunning::default()),
    });
    // Before the window exists, so the first call cannot find it missing.
    handle.manage(state.clone());
    tauri::async_runtime::spawn(events::forward(state.app.clone(), Windows(handle.clone())));
    // Before the first notification can be shown: one shown while nothing
    // listens reports no click, and on most Linux notification servers and
    // on Windows a click on it then does nothing at all.
    let forward = handle.clone();
    let _ = handle.notification().on_action(move |performed| {
        if notify::brings_the_window_forward(performed.action_id()) {
            bring_forward(&forward);
        }
    });
    let looking = state.clone();
    let notifier = handle.clone();
    tauri::async_runtime::spawn(notify::watch(
        &state.app,
        move || looking.focused.load(Ordering::SeqCst),
        move |notification| {
            let _ = notifier
                .notification()
                .builder()
                .title(notification.title)
                .body(notification.body)
                .show();
        },
    ));
    let tray_handle = handle.clone();
    tauri::async_runtime::spawn(tray::follow(
        state.app.clone(),
        state.keep_running.clone(),
        move |on| tray::keep_in_tray(&tray_handle, on),
    ));
    if let Err(err) = open_main_window(handle, &state) {
        tracing::error!(%err, "the window could not be created");
        cannot_start(handle, &format!("the window could not be created: {err}"));
    }
}

/// What the dialogs and messages call this program: the edition's own name.
fn product(handle: &AppHandle) -> String {
    handle
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| "Nocturne".to_string())
}

/// Show the window, restored and in front. Whatever hid it — the tray, the
/// person minimising it, a window on top — this is how it comes back: a
/// second start, the tray's Show, a click on a notification, the Dock.
fn bring_forward(handle: &AppHandle) -> bool {
    let Some(window) = handle.get_webview_window(MAIN_WINDOW) else {
        return false;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
    true
}

/// Say why the application is not starting, in a dialog, and end in failure
/// once it has been read. There is deliberately no window behind it: a blank
/// one would be worse than none.
///
/// Shown from inside the event loop and not before it: on a desktop a
/// program started from a launcher has no terminal, and on Windows a release
/// build has none at all, so a line on stderr is a start that silently does
/// nothing.
fn cannot_start(handle: &AppHandle, reason: &str) {
    let title = format!("{} cannot start", product(handle));
    eprintln!("{title}: {reason}");
    // This copy is not going to be the application: it has no engine and
    // no window, only a dialog waiting to be dismissed, and that is easily
    // behind something. While it still answered to the application's name
    // the next launch would hand over to it and end — nothing on the
    // screen again, and that even after whatever was in the way had been
    // closed. Giving the name up lets the next launch try for itself.
    tauri_plugin_single_instance::destroy(handle);
    let quitting = handle.clone();
    handle
        .dialog()
        .message(reason)
        .title(title)
        .kind(MessageDialogKind::Error)
        .show(move |_| quitting.exit(1));
}

/// A second launch has handed over to this copy and ended: come forward
/// instead of opening again.
fn started_again(handle: &AppHandle, arguments: &[String], cwd: &str, open: Option<&Path>) {
    let brought_forward = bring_forward(handle);
    tracing::info!(
        brought_forward,
        "Nocturne was started again; this copy stays the only one"
    );

    // The window that came forward shows this copy's data. If the launch
    // asked for other data, bringing it forward without a word would pass
    // one inventory off as another.
    let Some(open) = open else { return };
    let Some(other) = data_dir::asked_for_another(arguments, cwd, open) else {
        return;
    };
    tracing::warn!(
        open = %open.display(),
        asked_for = %other.display(),
        "the second launch asked for another data directory"
    );
    let name = product(handle);
    let mut dialog = handle
        .dialog()
        .message(format!(
            "{name} is working on the data in\n{}\n\nIt was started again for\n{}\n\nOnly one copy runs at a time. To work on the other folder, quit {name} first and start it again for that folder.",
            open.display(),
            other.display(),
        ))
        .title(format!("{name} is already open"))
        .kind(MessageDialogKind::Warning);
    if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
        dialog = dialog.parent(&window);
    }
    dialog.show(|_| {});
}

/// Create the window the interface lives in.
fn open_main_window(handle: &tauri::AppHandle, state: &Arc<State>) -> tauri::Result<()> {
    let dev_server = navigation::dev_server(handle.config());
    let window =
        tauri::WebviewWindowBuilder::new(handle, MAIN_WINDOW, tauri::WebviewUrl::default())
            .title("Nocturne")
            .inner_size(1440.0, 900.0)
            .min_inner_size(1100.0, 700.0)
            .background_color(colour(BACKGROUND))
            // Drops belong to the page. Tauri's own handler is for programs
            // that want file paths as events; left on, it takes every drop
            // away from the page (a file dropped on Import never arrived)
            // and on Windows it replaces WebView2's handler altogether, so
            // dragging a terminal tab to another pane did nothing.
            .disable_drag_drop_handler()
            // …and with the page handling drops, one that misses a drop zone
            // must not become "go to that file".
            .on_navigation(move |url| {
                let stays = navigation::stays_in_the_application(url, dev_server.as_ref());
                if !stays {
                    tracing::warn!(
                        scheme = url.scheme(),
                        "the window was asked to leave the application, and stayed"
                    );
                }
                stays
            })
            .build()?;
    tracing::info!(window = MAIN_WINDOW, "the interface window is open");
    let handle = handle.clone();
    let state = state.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Focused(focused) => state.focused.store(*focused, Ordering::SeqCst),
        // Closing quits: without an interface the backend has no reason to
        // keep running, and a half-stopped Nocturne would still be polling
        // devices — unless the person asked for exactly that, in Settings.
        // Then the window is only hidden, and the engine goes on.
        WindowEvent::CloseRequested { api, .. } => match state.keep_running.closing() {
            tray::Closing::HideToTray => {
                api.prevent_close();
                state.focused.store(false, Ordering::SeqCst);
                if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
                    let _ = window.hide();
                }
                tracing::info!("the window is hidden; Nocturne keeps running in the tray");
            }
            tray::Closing::Quit => handle.exit(0),
        },
        _ => {}
    });
    Ok(())
}

fn colour(hex: &str) -> tauri::window::Color {
    let value = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0);
    tauri::window::Color(
        ((value >> 16) & 0xff) as u8,
        ((value >> 8) & 0xff) as u8,
        (value & 0xff) as u8,
        255,
    )
}

/// Something went wrong where no dialog can be shown: the toolkit that would
/// show it is what failed. The terminal and the log file are all there is.
fn fail_without_a_window(
    logs: &logging::Logs,
    data_dir: &Result<PathBuf, String>,
    reason: &str,
) -> ! {
    // A failure is worth a line in the log file even from a launch that
    // never came to own the data directory.
    if let Ok(data_dir) = data_dir {
        logs.open_file(data_dir);
    }
    tracing::error!(%reason, "Nocturne cannot start");
    eprintln!("Nocturne cannot start: {reason}");
    logs.close();
    std::process::exit(1)
}

/// The message a panic was given, when it was given one.
fn panic_text(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no detail")
}

/// Say something to whoever started the program: on the terminal — or, for
/// a Windows release build started without one, in a message box. A program
/// built for the Windows subsystem gets no console from the one it was
/// typed in, and what it prints there is dropped without an error: `--help`
/// and `--version` printed nothing at all.
fn say(program: &str, text: &str, failure: bool) {
    #[cfg(windows)]
    if !has_a_terminal(failure) {
        rfd::MessageDialog::new()
            .set_title(program)
            .set_description(text)
            .set_level(if failure {
                rfd::MessageLevel::Error
            } else {
                rfd::MessageLevel::Info
            })
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
        return;
    }
    #[cfg(not(windows))]
    let _ = program;
    if failure {
        eprintln!("{text}");
    } else {
        println!("{text}");
    }
}

/// Whether what is printed goes anywhere. A handle that is not there reads
/// as null; one redirected to a file or a pipe — `| more`, a release
/// check reading `--version` — is there, and is written to as usual.
#[cfg(windows)]
fn has_a_terminal(failure: bool) -> bool {
    use std::os::windows::io::AsRawHandle;
    if failure {
        !std::io::stderr().as_raw_handle().is_null()
    } else {
        !std::io::stdout().as_raw_handle().is_null()
    }
}
