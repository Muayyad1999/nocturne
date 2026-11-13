//! The note that lets the `call` client find a running server:
//! `<data-dir>/server.json`, `{"url", "pid"}`.
//!
//! That there is only one server per data directory is the application's
//! business (it locks the directory when it starts); this module only keeps
//! the record.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::private_file;

/// Name of the instance file inside the data directory.
pub const FILE_NAME: &str = "server.json";

/// A running server, as recorded in the data directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instance {
    /// Where it listens, e.g. `http://127.0.0.1:7878/`. Never contains the token.
    pub url: String,
    /// Its process id.
    pub pid: u32,
}

/// The server recorded in `data_dir`, if any. The record can be stale when a
/// server was killed; the only proof that it is alive is that it answers.
pub fn find(data_dir: &Path) -> anyhow::Result<Option<Instance>> {
    let path = data_dir.join(FILE_NAME);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("could not read `{}`", path.display()));
        }
    };
    let instance = serde_json::from_str(&text)
        .with_context(|| format!("`{}` is not a valid instance file", path.display()))?;
    Ok(Some(instance))
}

/// Record that this process serves `data_dir` at `url`, replacing whatever an
/// earlier server left behind. The record is removed when the returned value
/// is dropped.
pub fn publish(data_dir: &Path, url: &str) -> anyhow::Result<Published> {
    let path = data_dir.join(FILE_NAME);
    let instance = Instance {
        url: url.to_string(),
        pid: std::process::id(),
    };
    let mut text = serde_json::to_string(&instance)?;
    text.push('\n');
    private_file::replace(&path, text.as_bytes())
        .with_context(|| format!("could not write `{}`", path.display()))?;
    Ok(Published { path })
}

/// The instance file of this process, for as long as it runs.
#[derive(Debug)]
pub struct Published {
    path: PathBuf,
}

impl Drop for Published {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(file = %self.path.display(), %err, "could not remove the instance file");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_exists_exactly_while_it_is_held() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find(dir.path()).unwrap(), None);

        let published = publish(dir.path(), "http://127.0.0.1:4242/").unwrap();
        let text = fs::read_to_string(dir.path().join(FILE_NAME)).unwrap();
        let record: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            record,
            serde_json::json!({"url": "http://127.0.0.1:4242/", "pid": std::process::id()})
        );
        assert_eq!(
            find(dir.path()).unwrap(),
            Some(Instance {
                url: "http://127.0.0.1:4242/".into(),
                pid: std::process::id()
            })
        );

        drop(published);
        assert_eq!(find(dir.path()).unwrap(), None);
        assert!(!dir.path().join(FILE_NAME).exists());
    }

    #[test]
    fn a_record_left_by_a_killed_server_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(FILE_NAME),
            r#"{"url": "http://127.0.0.1:1/", "pid": 1}"#,
        )
        .unwrap();
        let _published = publish(dir.path(), "http://127.0.0.1:4242/").unwrap();
        assert_eq!(
            find(dir.path()).unwrap().unwrap().url,
            "http://127.0.0.1:4242/"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_record_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let _published = publish(dir.path(), "http://127.0.0.1:4242/").unwrap();
        let mode = fs::metadata(dir.path().join(FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_damaged_record_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(FILE_NAME), "{").unwrap();
        let err = format!("{:#}", find(dir.path()).unwrap_err());
        assert!(err.contains(FILE_NAME), "{err}");
    }
}
