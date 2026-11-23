//! Interactive terminal sessions, SFTP, snippets and session logs.
//!
//! Specification: `docs/dev/features/terminal.md` (conventions: `docs/dev/features/README.md`).
//!
//! A session is a [`nocturne_transport::Connection`] owned by this backend
//! plus a scrollback ring. The UI attaches to it by id, gets the retained
//! output and the number of the last event that output covers, and then
//! applies `terminal.output` events with a higher number. Closing the window
//! does not close the session; `terminal.list` finds it again.

mod api;
mod autologin;
mod hostkey;
mod keys;
mod logs;
mod model;
mod open;
mod pastemode;
mod platform;
mod ring;
mod session;
mod sessionlog;
mod settings;
mod sftp;
mod snippets;
mod ssh;
mod text;

use nocturne_app::AppBuilder;

/// The terminal type Nocturne announces to devices. Wide support, and
/// 256 colours so a device that offers them does not fall back to monochrome.
pub const TERM: &str = "xterm-256color";

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut AppBuilder) {
    nocturne_app::settings::declare(
        builder,
        settings::SECTION,
        &settings::TerminalSettings::default(),
    );
    builder.state(session::TerminalState::default());
    builder.state(sftp::SftpState::default());
    builder.state(snippets::Library::default());
    logs::init(builder);

    open::register(&mut builder.router);
    api::register(&mut builder.router);
    snippets::register(&mut builder.router);
    logs::register(&mut builder.router);
    sftp::register(&mut builder.router);
    keys::register(&mut builder.router);

    builder.on_start(|app| {
        app.spawn("terminal.idle", session::reap_idle(app.clone()));
        app.spawn("terminal.sftp_idle", sftp::reap_idle(app.clone()));
        let app = app.clone();
        // Not `app.spawn`: this task's whole purpose is to run *at* shutdown.
        tokio::spawn(async move {
            app.shutdown_token().cancelled().await;
            sftp::close_all(&app).await;
        });
    });
}
