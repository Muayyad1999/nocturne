//! What the WebView may reach, pinned.
//!
//! `capabilities/default.json` is configuration, and configuration drifts
//! quietly: a permission set that "just makes it work" is one line, and
//! nothing fails when it is added. These tests fail when the interface is
//! given more than the specification says (`docs/dev/spec-desktop.md`, "What
//! the shell lends the interface") — or less than it needs, which is how the
//! Terminal screen's Paste button and every link in the application came to
//! do nothing in the installed program while each half had passing tests.

use serde_json::Value;

/// The permissions of the one capability file, as they are written.
fn permissions() -> Vec<Value> {
    let capability: Value = serde_json::from_str(include_str!("../capabilities/default.json"))
        .expect("capabilities/default.json is JSON");
    capability["permissions"]
        .as_array()
        .expect("a capability lists its permissions")
        .clone()
}

/// A permission is its identifier, or an object carrying one and a scope.
fn identifier(permission: &Value) -> &str {
    permission
        .as_str()
        .or_else(|| permission["identifier"].as_str())
        .expect("a permission is an identifier or carries one")
}

fn of_plugin<'a>(permissions: &'a [Value], plugin: &str) -> Vec<&'a Value> {
    permissions
        .iter()
        .filter(|permission| identifier(permission).starts_with(plugin))
        .collect()
}

/// The whole list, not the two plugins the interface needs: a grant nobody
/// asked for is the one that is never looked at again. `core:default` once
/// let a script in the page emit `nocturne://event` — and the shell turned
/// that into an operating-system notification for an alert no device ever
/// raised — and `dialog:default` let it open native file dialogs.
#[test]
fn the_interface_is_granted_exactly_what_the_specification_lists() {
    let permissions = permissions();
    let granted: Vec<&str> = permissions.iter().map(identifier).collect();
    assert_eq!(
        granted,
        [
            // Receiving `nocturne://event`, and stopping.
            "core:event:allow-listen",
            "core:event:allow-unlisten",
            // The keyboard shortcut for the inspector; it opens nothing in
            // a release build, which has no inspector.
            "core:webview:allow-internal-toggle-devtools",
            // Asked by the notification plugin's own script on every page
            // load; refused, it puts an error in the console. It answers
            // yes or no and shows nothing.
            "notification:allow-is-permission-granted",
            "clipboard-manager:allow-read-text",
            "opener:allow-open-url",
        ]
    );
}

#[test]
fn the_interface_may_read_text_from_the_clipboard_and_nothing_more() {
    let permissions = permissions();
    let clipboard: Vec<&str> = of_plugin(&permissions, "clipboard-manager:")
        .into_iter()
        .map(identifier)
        .collect();

    // Reading text is what Paste needs. Writing, images and HTML are not
    // granted: copying works through the WebView's own clipboard.
    assert_eq!(clipboard, ["clipboard-manager:allow-read-text"]);
}

#[test]
fn a_link_opens_in_the_system_browser_only_when_it_is_http_or_https() {
    let permissions = permissions();
    let opener = of_plugin(&permissions, "opener:");
    assert_eq!(
        opener.iter().map(|p| identifier(p)).collect::<Vec<_>>(),
        ["opener:allow-open-url"],
        "opening an address is the only thing the opener is for here: no paths, no revealing files"
    );

    // The command without a scope refuses every address, which is what it did.
    let allowed: Vec<&str> = opener[0]["allow"]
        .as_array()
        .expect("opener:allow-open-url needs a scope: without one the plugin refuses every address")
        .iter()
        .map(|entry| {
            entry["url"]
                .as_str()
                .expect("a scope entry names a url pattern")
        })
        .collect();
    assert_eq!(allowed, ["http://*", "https://*"]);
}

/// A permission is half of it. The other half is the plugin being in the
/// builder at all: without it the interface is told "plugin not found" the
/// first time it asks, and only the installed program ever shows that.
///
/// Read from the source on purpose. The real `tauri::Builder` needs a display
/// to be built, and a mock one would be given a list written a second time
/// here, which proves nothing about the list `run` uses.
#[test]
fn every_plugin_the_interface_is_lent_is_registered_with_the_shell() {
    let shell = include_str!("lib.rs");
    let permissions = permissions();
    for (plugin, registration) in [
        (
            "clipboard-manager:",
            ".plugin(tauri_plugin_clipboard_manager::init())",
        ),
        ("opener:", ".plugin(tauri_plugin_opener::init())"),
    ] {
        assert!(
            !of_plugin(&permissions, plugin).is_empty(),
            "nothing of `{plugin}` is granted, so this test no longer says anything about it"
        );
        assert!(
            shell.contains(registration),
            "`{plugin}` is granted to the interface but `run` does not register the plugin: \
             `{registration}` is not in lib.rs"
        );
    }
}
