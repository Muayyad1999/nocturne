//! Starting the engine, for a launch that is going to stay.
//!
//! The order matters and is the whole reason this is its own step. The data
//! directory belongs to one process at a time; taking it is the first thing
//! that can fail for a reason the person can do something about ("it is
//! already open over there"). So it happens only after a second launch has
//! had its chance to hand over to the first — which is the usual reason the
//! directory is taken — and what is left is said out loud: in the log file,
//! and by the caller in a dialog.

use std::path::PathBuf;

use nocturne_suite::{App, AppConfig};

use crate::logging::Logs;

/// Open the log file and start the engine on the data directory.
///
/// `data_dir` is the outcome of [`crate::data_dir::resolve`]: not knowing
/// where the data belongs is a reason not to start like any other, and is
/// reported the same way. The error is the sentence to show.
pub fn engine(
    runtime: &tokio::runtime::Handle,
    data_dir: Result<PathBuf, String>,
    logs: &Logs,
) -> Result<App, String> {
    let data_dir = data_dir.inspect_err(|reason| {
        tracing::error!(%reason, "there is no data directory to start on");
    })?;
    logs.open_file(&data_dir);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), data_dir = %data_dir.display(), "starting");
    // The engine starts its background tasks as it is built, and they
    // belong on the runtime that outlives this call.
    let _on_the_runtime = runtime.enter();
    nocturne_suite::start(AppConfig::at(&data_dir)).map_err(|err| {
        tracing::error!(code = err.code, message = %err.message, "the backend did not start");
        err.message
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn logged(data_dir: &Path) -> String {
        let mut text = String::new();
        for entry in std::fs::read_dir(data_dir.join("logs"))
            .into_iter()
            .flatten()
            .flatten()
        {
            text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
        }
        text
    }

    /// Logs that reach only this thread, so tests do not share a logger.
    fn with_logs<T>(body: impl FnOnce(&Logs) -> T) -> T {
        let (logs, subscriber) = crate::logging::build(Some("info"));
        tracing::subscriber::with_default(subscriber, || body(&logs))
    }

    #[test]
    fn a_data_directory_in_use_is_a_sentence_for_the_person_and_a_line_in_its_log() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let folder = data_dir.path().to_path_buf();

        // The other edition, or `nocturne-server`: whoever got there first.
        let holder = with_logs(|logs| engine(runtime.handle(), Ok(folder.clone()), logs))
            .expect("the first to ask gets the folder");

        let refused = with_logs(|logs| {
            let refused = engine(runtime.handle(), Ok(folder.clone()), logs);
            // An exit follows at once; the line has to be in the file by then.
            logs.close();
            refused
        });
        let message = refused.err().expect("the folder is taken");
        assert!(message.contains("already running"), "{message}");
        assert!(
            message.contains(&folder.display().to_string()),
            "which folder? {message}"
        );
        let log = logged(&folder);
        assert!(log.contains("the backend did not start"), "{log}");

        holder.shutdown();
    }

    /// The order this module depends on is in `run`, and what it buys is
    /// only visible between two processes (`tests/launch.rs`, which needs a
    /// display and is not run by default). Read from the source, so that the
    /// order cannot be undone without a test saying so.
    #[test]
    fn a_second_launch_gets_its_chance_before_the_data_directory_is_touched() {
        let shell = include_str!("lib.rs");
        let first_plugin = shell.find(".plugin(").expect("the shell registers plugins");
        assert!(
            shell[first_plugin..].starts_with(".plugin(tauri_plugin_single_instance::"),
            "plugins are set up in the order they are registered, and the single-instance one \
             has to be first: it is where a second launch hands over and ends"
        );
        let built = shell
            .find(".build(tauri::generate_context!())")
            .expect("the shell builds the application");
        let engine = shell
            .find("startup::engine(")
            .expect("the shell starts the engine through this module");
        assert!(
            engine > built && !shell.contains("nocturne_suite::start("),
            "the engine is started before the application is built, which is before a second \
             launch has been recognised: it will meet the first one's lock and fail instead"
        );
    }

    #[test]
    fn not_knowing_where_the_data_belongs_is_reported_and_nothing_is_created() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let refused = with_logs(|logs| {
            engine(
                runtime.handle(),
                Err("this system does not say where application data belongs".into()),
                logs,
            )
        });
        assert_eq!(
            refused.err().as_deref(),
            Some("this system does not say where application data belongs")
        );
    }
}
