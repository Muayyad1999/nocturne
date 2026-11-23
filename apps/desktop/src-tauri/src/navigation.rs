//! Keeping the window on the application.
//!
//! The WebView is a browser, and a browser's answer to "something was
//! dropped on me" or "a link was followed" is to go there. Nocturne has one
//! page and no Back button: a spreadsheet let go of an inch beside the
//! Import dialog's drop zone would replace the whole application with the
//! spreadsheet, until it is restarted.
//!
//! Tauri's own drag-and-drop handler would prevent that, by taking every
//! drop away from the page — which is why it is off: nothing can be dropped
//! *on* the page either (`docs/dev/spec-desktop.md`, "Drag and drop"). With
//! the page handling its own drops, this is what keeps a drop that misses
//! from turning into a navigation. Links meant for the system browser do
//! not pass through here: they are opened by the opener plugin, not
//! followed.

use tauri::Url;

/// Where the interface is served from when it is not inside the program.
///
/// `cargo tauri dev` does not build the interface into the binary: it
/// serves the built files from an address of its own on this machine and
/// tells the program so (`build.devUrl`). The window then *starts* at that
/// address, and a rule that only knew the built-in pages refused the
/// application's own first page — a blank window in the one way of running
/// it that exists for working on it. Read under the same condition Tauri
/// reads it under: a build that serves from itself ignores the setting, and
/// so must this.
pub fn dev_server(config: &tauri::Config) -> Option<Url> {
    if tauri::is_dev() {
        config.build.dev_url.clone()
    } else {
        None
    }
}

/// Whether a navigation stays inside the application. `dev_server` is
/// [`dev_server`]'s answer: the one other address that is the application.
pub fn stays_in_the_application(url: &Url, dev_server: Option<&Url>) -> bool {
    match url.scheme() {
        // The interface's own pages, as Linux and macOS address them…
        "tauri" => true,
        // …and as Windows does, where WebView2 is given the same pages
        // under a host name because it cannot be given a scheme. Or the
        // development server: that scheme, host and port, and no other.
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || dev_server.is_some_and(|server| url.origin() == server.origin())
        }
        // Nowhere else: the empty page a WebView starts from, and something
        // the page made itself and is handing over to be saved.
        "about" | "blob" => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As the installed program sees it: the interface is inside it.
    fn stays(address: &str) -> bool {
        stays_in_the_application(&Url::parse(address).expect(address), None)
    }

    #[test]
    fn the_interface_served_by_the_development_server_is_the_application_too() {
        // What `cargo tauri dev` puts in the configuration.
        let config: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "app.nocturne.desktop",
            "build": {"devUrl": "http://127.0.0.1:1430"},
        }))
        .expect("a configuration");
        let server = dev_server(&config).expect("a build made for development has a server");
        let from_there =
            |address: &str| stays_in_the_application(&Url::parse(address).unwrap(), Some(&server));

        // The window's first page, and a reload of any other.
        assert!(from_there("http://127.0.0.1:1430/"));
        assert!(from_there("http://127.0.0.1:1430/index.html#/devices"));
        // That address and no other: another port on this machine is
        // another program, and another name for it is not known to be it.
        for elsewhere in [
            "http://127.0.0.1:8080/",
            "http://127.0.0.1/",
            "https://127.0.0.1:1430/",
            "http://localhost:1430/",
            "http://127.0.0.1.example.com:1430/",
            "https://example.com/",
            "file:///home/someone/devices.csv",
        ] {
            assert!(!from_there(elsewhere), "{elsewhere}");
        }
        // And only while there is such a server at all.
        assert!(!stays("http://127.0.0.1:1430/"));

        let built_in: tauri::Config =
            serde_json::from_value(serde_json::json!({"identifier": "app.nocturne.desktop"}))
                .expect("a configuration");
        assert_eq!(dev_server(&built_in), None);
    }

    #[test]
    fn the_interface_may_reload_itself_on_every_platform() {
        for own in [
            "tauri://localhost",
            "tauri://localhost/",
            "tauri://localhost/index.html#/devices",
            "http://tauri.localhost/",
            "https://tauri.localhost/index.html#/terminal",
            "about:blank",
        ] {
            assert!(stays(own), "{own}");
        }
    }

    #[test]
    fn a_file_dropped_beside_a_drop_zone_does_not_replace_the_application() {
        for dropped in [
            "file:///home/someone/devices.csv",
            "file:///C:/Users/someone/Desktop/devices.csv",
        ] {
            assert!(!stays(dropped), "{dropped}");
        }
    }

    #[test]
    fn a_link_dragged_in_or_followed_does_not_take_the_window_to_another_site() {
        for elsewhere in [
            "https://example.com/",
            "http://192.0.2.10/admin",
            // Looks like home, is not.
            "https://tauri.localhost.example.com/",
            "http://localhost:5173/",
            "data:text/html,<h1>not Nocturne</h1>",
            "javascript:void(0)",
            "ftp://example.com/file",
        ] {
            assert!(!stays(elsewhere), "{elsewhere}");
        }
    }

    /// What these rules are for happens in the window's builder, and only a
    /// real WebView with a real drag shows whether it works (that was done
    /// by hand; `docs/dev/spec-desktop.md`, "Drag and drop"). So that the
    /// two lines cannot quietly go, they are read from the source: without
    /// the first, nothing can be dropped on the page; with the first and
    /// without the second, a drop that misses replaces the application.
    #[test]
    fn the_window_leaves_drops_to_the_page_and_does_not_follow_one_that_misses() {
        let shell = include_str!("lib.rs");
        assert!(
            shell.contains(".disable_drag_drop_handler()"),
            "Tauri's drag-and-drop handler is back on: it takes every drop away from the page"
        );
        assert!(
            shell.contains(".on_navigation(")
                && shell.contains("stays_in_the_application(url, dev_server.as_ref())"),
            "the window no longer asks whether a navigation stays in the application"
        );
        assert!(
            shell.contains("navigation::dev_server(handle.config())"),
            "the window is no longer told where `cargo tauri dev` serves the interface from: \
             its own first page is then somewhere else, and it stays blank"
        );
    }

    #[test]
    fn a_file_the_interface_offers_to_save_is_not_a_place_to_go() {
        // An export is a link to a blob the page made; refusing it here
        // would cancel the download.
        assert!(stays(
            "blob:tauri://localhost/0b5a4f0c-7d43-4c1e-9d0b-0f7f6c1f2a11"
        ));
    }
}
