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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use nocturne_suite::{App, AppError};
use parking_lot::Mutex;
use tauri::{AppHandle, Emitter, Listener, Manager, RunEvent, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

/// The window the interface lives in.
const MAIN_WINDOW: &str = "main";
/// Drawn before the interface paints, so starting does not flash white.
const BACKGROUND: &str = "#090d18";

/// Everything the commands need.
struct State {
    app: App,
    notifications: Mutex<notify::Throttle>,
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

/// Start the application. Returns only when the user quits.
pub fn run() {
    let arguments = data_dir::parse_arguments(std::env::args().skip(1));
    if arguments.help {
        println!("{}", data_dir::usage(&data_dir::program_name()));
        return;
    }
    if arguments.version {
        println!("{} {}", data_dir::program_name(), env!("CARGO_PKG_VERSION"));
        return;
    }

    // To the terminal only, for now: the log file is in the data directory,
    // and this launch does not own that yet (see `startup`).
    let logs = Arc::new(logging::start(arguments.log_level.as_deref()));
    if !arguments.unknown.is_empty() {
        tracing::debug!(arguments = ?arguments.unknown, "arguments the shell does not use");
    }
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

    let result = tauri::Builder::default()
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
        })
        .build(tauri::generate_context!());

    match result {
        Ok(app) => app.run(move |handle, event| match event {
            RunEvent::ExitRequested { .. } => {
                tracing::info!("stopping");
                if let Some(state) = handle.try_state::<Arc<State>>() {
                    state.app.shutdown();
                }
            }
            // The last thing that happens: the process is exited from inside
            // the event loop, and nothing after `run` is ever reached.
            RunEvent::Exit => logs.close(),
            _ => {}
        }),
        Err(err) => fail_without_a_window(&logs, &wanted, &err.to_string()),
    }
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
        notifications: Mutex::new(notify::Throttle::new()),
    });
    // Before the window exists, so the first call cannot find it missing.
    handle.manage(state.clone());
    tauri::async_runtime::spawn(events::forward(state.app.clone(), Windows(handle.clone())));
    watch_alerts(handle, state);
    if let Err(err) = open_main_window(handle) {
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
    let window = handle.get_webview_window(MAIN_WINDOW);
    if let Some(window) = &window {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    tracing::info!(
        brought_forward = window.is_some(),
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
    if let Some(window) = &window {
        dialog = dialog.parent(window);
    }
    dialog.show(|_| {});
}

/// Create the window the interface lives in.
fn open_main_window(handle: &tauri::AppHandle) -> tauri::Result<()> {
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
    // Closing the window quits: the backend has no reason to keep running
    // without an interface, and a half-stopped Nocturne would still be
    // polling devices.
    let quitting = handle.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { .. } = event {
            quitting.exit(0);
        }
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

/// Turn serious new alerts into system notifications while the window is away.
fn watch_alerts(handle: &tauri::AppHandle, state: Arc<State>) {
    let notifier = handle.clone();
    handle.listen(events::CHANNEL, move |event| {
        let Ok(payload) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
            return;
        };
        if payload.get("topic").and_then(|topic| topic.as_str()) != Some("alerts.changed") {
            return;
        }
        let focused = notifier
            .get_webview_window(MAIN_WINDOW)
            .and_then(|window| window.is_focused().ok())
            .unwrap_or(false);
        let shown = state.notifications.lock().consider(
            payload.get("payload").unwrap_or(&serde_json::Value::Null),
            focused,
            Instant::now(),
        );
        if let Some(notification) = shown {
            use tauri_plugin_notification::NotificationExt;
            let _ = notifier
                .notification()
                .builder()
                .title(notification.title)
                .body(notification.body)
                .show();
        }
    });
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
