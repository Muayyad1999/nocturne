//! The Nocturne desktop application.
//!
//! A thin shell: it starts the backend in this process, shows the interface
//! in the system WebView, and connects the two with one command (`rpc`) and
//! one event channel. No product logic lives here — the same backend answers
//! the same way when Nocturne runs as a server.

pub mod data_dir;
pub mod events;
pub mod logging;
pub mod notify;
pub mod rpc;

use std::sync::Arc;
use std::time::Instant;

use nocturne_suite::{App, AppConfig, AppError};
use parking_lot::Mutex;
use tauri::{Emitter, Listener, Manager, WindowEvent};

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
        println!("{}", data_dir::USAGE);
        return;
    }
    if arguments.version {
        println!("nocturne {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let data_dir = match data_dir::resolve(
        arguments.data_dir.as_deref(),
        std::env::var(data_dir::ENV).ok(),
    ) {
        Ok(dir) => dir,
        Err(err) => fail_before_start("Nocturne cannot start", &format!("{err:#}")),
    };
    let _logging = logging::start(&data_dir, arguments.log_level.as_deref());
    if !arguments.unknown.is_empty() {
        tracing::debug!(arguments = ?arguments.unknown, "arguments the shell does not use");
    }

    // The backend owns background tasks, so it needs a runtime that outlives
    // this function; Tauri is told to use the same one.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => fail_before_start("Nocturne cannot start", &format!("no async runtime: {err}")),
    };
    tauri::async_runtime::set(runtime.handle().clone());

    tracing::info!(version = env!("CARGO_PKG_VERSION"), data_dir = %data_dir.display(), "starting");
    let backend = runtime.block_on(async { nocturne_suite::start(AppConfig::at(&data_dir)) });

    let state = match backend {
        Ok(app) => Arc::new(State {
            app,
            notifications: Mutex::new(notify::Throttle::new()),
        }),
        Err(err) => {
            tracing::error!(code = err.code, message = %err.message, "the backend did not start");
            fail_before_start("Nocturne cannot start", &err.message)
        }
    };

    let shutdown = state.app.clone();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_single_instance::init(|handle, _args, _cwd| {
            // A second launch brings the first one forward instead of opening again.
            if let Some(window) = handle.get_webview_window(MAIN_WINDOW) {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![rpc])
        .setup(move |app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(events::forward(
                state.app.clone(),
                Windows(handle.clone()),
            ));
            watch_alerts(&handle, state.clone());
            open_main_window(&handle)?;
            Ok(())
        })
        .build(tauri::generate_context!());

    match result {
        Ok(app) => app.run(move |_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                tracing::info!("stopping");
                shutdown.shutdown();
            }
        }),
        Err(err) => {
            tracing::error!(%err, "the window could not be created");
            fail_before_start(
                "Nocturne cannot start",
                &format!("the window could not be created: {err}"),
            );
        }
    }
}

/// Create the window the interface lives in.
fn open_main_window(handle: &tauri::AppHandle) -> tauri::Result<()> {
    let window =
        tauri::WebviewWindowBuilder::new(handle, MAIN_WINDOW, tauri::WebviewUrl::default())
            .title("Nocturne")
            .inner_size(1440.0, 900.0)
            .min_inner_size(1100.0, 700.0)
            .background_color(colour(BACKGROUND))
            .build()?;
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

/// Something went wrong before there is a window to say it in. Tell the
/// terminal, and exit with a failure: a blank window would be worse.
fn fail_before_start(title: &str, message: &str) -> ! {
    eprintln!("{title}: {message}");
    std::process::exit(1)
}
