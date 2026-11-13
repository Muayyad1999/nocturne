//! Files that hold secrets (the access token, the TLS private key).
//!
//! On Unix they are created with mode `0600` from the first byte on. Windows
//! has no mode bits: a file there inherits the access control list of its
//! directory, and the data directory lives under the user's own profile.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Create `dir` and its missing parents, readable by the owner only.
/// An existing directory is left exactly as it is.
pub(crate) fn create_dir(dir: &Path) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Create `path` with `contents`. Fails with `AlreadyExists` instead of
/// touching a file that is already there.
pub(crate) fn create_new(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(contents)?;
    file.sync_all()
}

/// Replace `path` with `contents` in one step, so a reader never sees half a file.
pub(crate) fn replace(path: &Path, contents: &[u8]) -> io::Result<()> {
    let temporary = temporary_sibling(path);
    // Left behind by a process that died between the two steps below.
    let _ = fs::remove_file(&temporary);
    create_new(&temporary, contents)?;
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

/// Take away group and world access from an existing secret file.
/// Returns whether anything had to change.
#[cfg(unix)]
pub(crate) fn restrict(path: &Path) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 == 0 {
        return Ok(false);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(true)
}

/// Nothing to do: see the module documentation.
#[cfg(not(unix))]
pub(crate) fn restrict(_path: &Path) -> io::Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn create_new_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        create_new(&path, b"first").unwrap();
        let again = create_new(&path, b"second").unwrap_err();
        assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&path).unwrap(), b"first");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn replace_swaps_the_content_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        replace(&path, b"one").unwrap();
        replace(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["secret"]);
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn directories_are_private_and_existing_ones_are_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        create_dir(&nested).unwrap();
        assert_eq!(mode(&nested), 0o700);
        assert_eq!(mode(&dir.path().join("a")), 0o700);

        let open = dir.path().join("open");
        fs::create_dir(&open).unwrap();
        fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
        create_dir(&open).unwrap();
        assert_eq!(mode(&open), 0o755);
    }

    #[cfg(unix)]
    #[test]
    fn restrict_closes_a_file_that_others_could_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret");
        fs::write(&path, b"x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(restrict(&path).unwrap());
        assert_eq!(mode(&path), 0o600);
        assert!(!restrict(&path).unwrap());
    }
}
