//! Keeping Nocturne running with its window closed.
//!
//! `general.keep_running_in_tray` is the engine's setting — the core
//! declares it and the Settings screen offers it — and what it changes is
//! the shell's: whether closing the window quits, or only hides it while the
//! engine goes on monitoring, receiving logs and running schedules. Hidden,
//! the window is still where serious alerts come from as notifications (a
//! click on one brings it back), and a tray icon with Show and Quit is what
//! is left of Nocturne on the screen. Starting it again brings the window
//! back too: that is the way back on a desktop that shows no tray icons.
//!
//! The shell learns the setting the way any client does, through the one
//! door: `settings.get` once, then every `settings.changed`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nocturne_suite::App;
use serde_json::{Value, json};
use tauri::menu::{Menu, MenuEvent, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};
use tokio::sync::broadcast::error::RecvError;

/// The settings section and key, as the core declares them.
pub const SECTION: &str = "general";
pub const KEY: &str = "keep_running_in_tray";

/// What closing the window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    Quit,
    HideToTray,
}

/// The setting, as last heard from the engine.
#[derive(Debug, Default)]
pub struct KeepRunning(AtomicBool);

impl KeepRunning {
    pub fn is_on(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn closing(&self) -> Closing {
        if self.is_on() {
            Closing::HideToTray
        } else {
            Closing::Quit
        }
    }

    /// Whether this changed it.
    fn set(&self, on: bool) -> bool {
        self.0.swap(on, Ordering::SeqCst) != on
    }
}

/// The setting in a `settings.get` answer, or in the `value` of a
/// `settings.changed` event, which is the whole section.
fn from_section(section: &Value) -> Option<bool> {
    section.get(KEY)?.as_bool()
}

/// Read the setting, then follow it until the engine stops. `apply` is
/// called whenever it changes — first when a stored "on" is read at the
/// start — so the tray icon comes and goes with it.
pub async fn follow(
    app: App,
    keep: Arc<KeepRunning>,
    apply: impl Fn(bool) + Send + Sync + 'static,
) {
    // Subscribed before the first read, so a change in between is not lost.
    let mut events = app.subscribe();
    let shutdown = app.shutdown_token();
    let read = |section: Value| {
        if let Some(on) = from_section(&section)
            && keep.set(on)
        {
            apply(on);
        }
    };
    let ask = || app.rpc("settings.get", json!({ "section": SECTION }));
    match ask().await {
        Ok(section) => read(section),
        Err(err) => tracing::warn!(
            code = err.code,
            message = %err.message,
            "the tray setting could not be read; closing the window quits"
        ),
    }
    loop {
        let received = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            received = events.recv() => received,
        };
        match received {
            Ok(event) if event.topic == "settings.changed" => {
                if event.payload["section"] == SECTION {
                    read(event.payload["value"].clone());
                }
            }
            Ok(_) => {}
            // Whatever was missed, the setting is what it is now.
            Err(RecvError::Lagged(_)) => {
                if let Ok(section) = ask().await {
                    read(section);
                }
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// The tray icon's id, and its menu's.
const TRAY: &str = "nocturne";
const SHOW: &str = "tray.show";
const QUIT: &str = "tray.quit";

/// What the tray menu's two items do. Registered once, on the application's
/// builder: one given to the icon's builder joins the application-wide
/// handlers anyway, once more each time an icon is made.
pub fn on_menu(handle: &AppHandle, event: &MenuEvent) {
    match event.id().as_ref() {
        SHOW => {
            crate::bring_forward(handle);
        }
        QUIT => handle.exit(0),
        _ => {}
    }
}

/// Put the tray icon up, or take it down.
pub fn keep_in_tray(handle: &AppHandle, on: bool) {
    let main = handle.clone();
    // On the main thread itself, so that a failure inside the toolkit is
    // caught here (see `put_up`) and does not unwind through the event loop.
    let _ = handle.run_on_main_thread(move || {
        if on {
            put_up(&main);
        } else {
            take_down(&main);
        }
    });
}

fn put_up(handle: &AppHandle) {
    // Made once and then only shown and hidden. An icon made a second time
    // under the same name was never seen on Linux: libayatana-appindicator
    // does not take the first one's objects off the session bus when it is
    // dropped, so the second cannot be put there ("an object is already
    // exported"), and the panel is left with an icon whose menu does not
    // answer.
    if let Some(icon) = handle.tray_by_id(TRAY) {
        match icon.set_visible(true) {
            Ok(()) => tracing::info!("the tray icon is up: closing the window hides it"),
            Err(err) => tracing::warn!(%err, "the tray icon could not be shown again"),
        }
        return;
    }
    // On Linux the icon is drawn by libayatana-appindicator, which is opened
    // when the first icon is made — and a library that is not there is a
    // panic in there, not an error. The packages depend on it; a build run
    // elsewhere may not have it, and that must cost the icon, not the
    // program.
    let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| make(handle)));
    match made {
        Ok(Ok(_)) => tracing::info!("the tray icon is up: closing the window hides it"),
        Ok(Err(err)) => tracing::warn!(
            %err,
            "no tray icon; closing the window still hides it, and starting Nocturne again brings it back"
        ),
        Err(_) => tracing::warn!(
            "no tray icon: the system's indicator library (libayatana-appindicator3) could not be \
             loaded; closing the window still hides it, and starting Nocturne again brings it back"
        ),
    }
}

fn make(handle: &AppHandle) -> tauri::Result<tauri::tray::TrayIcon> {
    let product = crate::product(handle);
    let show = MenuItem::with_id(handle, SHOW, format!("Show {product}"), true, None::<&str>)?;
    let quit = MenuItem::with_id(handle, QUIT, format!("Quit {product}"), true, None::<&str>)?;
    let menu = Menu::with_items(handle, &[&show, &quit])?;
    let mut icon = TrayIconBuilder::with_id(TRAY)
        .menu(&menu)
        .tooltip(&product)
        // A click on a menu-bar icon opens its menu on macOS. On Windows a
        // click on the icon brings the window back and the right button
        // opens the menu; on Linux the indicator only ever opens the menu.
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_tray_icon_event(|icon, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::bring_forward(icon.app_handle());
            }
        });
    if let Some(image) = handle.default_window_icon() {
        icon = icon.icon(image.clone());
    }
    icon.build(handle)
}

fn take_down(handle: &AppHandle) {
    if let Some(icon) = handle.tray_by_id(TRAY) {
        match icon.set_visible(false) {
            Ok(()) => tracing::info!("the tray icon is down: closing the window quits"),
            Err(err) => tracing::warn!(%err, "the tray icon could not be hidden"),
        }
    }
    // Hidden with nothing left to bring it back but a second start: the
    // setting was turned off from somewhere other than this window.
    let hidden = handle
        .get_webview_window(crate::MAIN_WINDOW)
        .is_some_and(|window| !window.is_visible().unwrap_or(true));
    if hidden {
        crate::bring_forward(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_suite::AppConfig;
    use serde_json::json;
    use std::time::Duration;
    use tokio::sync::mpsc;

    async fn next(changes: &mut mpsc::UnboundedReceiver<bool>) -> bool {
        tokio::time::timeout(Duration::from_secs(5), changes.recv())
            .await
            .expect("the shell heard about the setting")
            .expect("the channel is open")
    }

    async fn set(app: &App, value: serde_json::Value) {
        app.rpc(
            "settings.set",
            json!({ "section": "general", "value": value }),
        )
        .await
        .expect("settings.set");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_setting_the_settings_screen_saves_is_what_closing_the_window_does() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        let keep = Arc::new(KeepRunning::default());
        let (sender, mut changes) = mpsc::unbounded_channel();
        let follower = tokio::spawn(follow(app.clone(), keep.clone(), move |on| {
            let _ = sender.send(on);
        }));
        assert_eq!(
            keep.closing(),
            Closing::Quit,
            "off unless somebody turns it on"
        );

        // What the Settings screen sends.
        set(&app, json!({ "keep_running_in_tray": true })).await;
        assert!(next(&mut changes).await);
        assert_eq!(keep.closing(), Closing::HideToTray);

        // Another key of the same section changes nothing here…
        set(&app, json!({ "theme": "day" })).await;
        set(&app, json!({ "keep_running_in_tray": false })).await;
        // …so the next thing heard is the switch going off.
        assert!(!next(&mut changes).await);
        assert_eq!(keep.closing(), Closing::Quit);

        app.shutdown();
        tokio::time::timeout(Duration::from_secs(5), follower)
            .await
            .expect("following stops with the engine")
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_setting_saved_in_an_earlier_session_holds_from_the_start() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        set(&app, json!({ "keep_running_in_tray": true })).await;

        let keep = Arc::new(KeepRunning::default());
        let (sender, mut changes) = mpsc::unbounded_channel();
        tokio::spawn(follow(app.clone(), keep.clone(), move |on| {
            let _ = sender.send(on);
        }));
        assert!(
            next(&mut changes).await,
            "the tray icon is put up at the start"
        );
        assert_eq!(keep.closing(), Closing::HideToTray);
        app.shutdown();
    }

    /// What `closing` decides is carried out in the window's close handler,
    /// which only a real window manager can drive (that was done by hand:
    /// `docs/dev/spec-desktop.md`, "Closing"). Read from the source so the
    /// handler cannot quietly go back to quitting whatever the setting says.
    #[test]
    fn the_close_handler_asks_the_setting_and_hides_instead_of_closing() {
        let shell = include_str!("lib.rs");
        let handler = shell
            .split("WindowEvent::CloseRequested")
            .nth(1)
            .expect("the window handles a close request");
        let handler = &handler[..handler.find("WindowEvent::").unwrap_or(handler.len())];
        for needed in [
            ".closing()",
            "Closing::HideToTray",
            "prevent_close()",
            ".hide()",
        ] {
            assert!(
                handler.contains(needed),
                "the close handler no longer does `{needed}`:\n{handler}"
            );
        }
    }
}
