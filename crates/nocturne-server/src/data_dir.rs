//! The data directory: where the application keeps its database and where
//! the server keeps its token, its certificate and its instance file.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::private_file;

/// Environment variable that overrides the default location.
pub const ENV: &str = "NOCTURNE_DATA_DIR";

/// The platform's data directory plus `Nocturne`, e.g.
/// `~/.local/share/Nocturne` or `%APPDATA%\Nocturne`. `None` if the platform
/// does not say where data belongs (no home directory).
pub fn default_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("Nocturne"))
}

/// Make sure `dir` exists. A directory created here is private to the user;
/// an existing one is not touched.
pub fn ensure(dir: &Path) -> anyhow::Result<()> {
    private_file::create_dir(dir)
        .with_context(|| format!("could not create the data directory `{}`", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_location_ends_in_nocturne() {
        // Absent only where the platform has no notion of a home directory.
        if let Some(path) = default_path() {
            assert_eq!(path.file_name().unwrap(), "Nocturne");
            assert!(path.is_absolute(), "{}", path.display());
        }
    }

    #[test]
    fn ensure_creates_missing_directories_and_explains_failures() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a/b/Nocturne");
        ensure(&nested).unwrap();
        assert!(nested.is_dir());
        ensure(&nested).unwrap();

        let file = root.path().join("file");
        std::fs::write(&file, "x").unwrap();
        let err = format!("{:#}", ensure(&file.join("below")).unwrap_err());
        assert!(err.contains("could not create the data directory"), "{err}");
    }
}
