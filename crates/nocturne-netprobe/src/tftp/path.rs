//! Confines requested file names to the server's root directory.

use std::path::{Path, PathBuf};

use super::packet::code;

/// Why a request is refused: the TFTP error to send and the reason to show the operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Denial {
    pub code: u16,
    pub reason: String,
}

fn deny(code: u16, reason: impl Into<String>) -> Denial {
    Denial {
        code,
        reason: reason.into(),
    }
}

/// Names that are devices on Windows whatever their extension; refused everywhere so that a
/// root directory behaves the same on every platform.
const DEVICE_NAMES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];

/// The path components of a requested name, after the checks that need no file system:
/// no absolute paths, no `..`, no device names, no control characters.
fn components(name: &str) -> Result<Vec<&str>, Denial> {
    if name.is_empty() {
        return Err(deny(code::ACCESS_VIOLATION, "the file name is empty"));
    }
    if name.len() > 1024 || name.chars().any(char::is_control) {
        return Err(deny(
            code::ACCESS_VIOLATION,
            "the file name is too long or contains control characters",
        ));
    }
    let drive_letter =
        name.as_bytes().get(1) == Some(&b':') && name.as_bytes()[0].is_ascii_alphabetic();
    if name.starts_with(['/', '\\']) || drive_letter {
        return Err(deny(
            code::ACCESS_VIOLATION,
            "absolute paths are not allowed; request a path relative to the TFTP root",
        ));
    }
    let mut components = Vec::new();
    for component in name.split(['/', '\\']) {
        match component {
            "" | "." => continue,
            ".." => {
                return Err(deny(
                    code::ACCESS_VIOLATION,
                    "'..' is not allowed in file names; files are confined to the TFTP root",
                ));
            }
            _ => {}
        }
        let stem = component
            .split('.')
            .next()
            .unwrap_or_default()
            .trim_end()
            .to_ascii_uppercase();
        let numbered_port = (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit();
        if DEVICE_NAMES.contains(&stem.as_str()) || numbered_port {
            return Err(deny(
                code::ACCESS_VIOLATION,
                format!("'{component}' is a device name, not a file"),
            ));
        }
        if component.ends_with(['.', ' ']) || component.contains(':') {
            return Err(deny(
                code::ACCESS_VIOLATION,
                format!("'{component}' is not a portable file name"),
            ));
        }
        components.push(component);
    }
    if components.is_empty() {
        return Err(deny(code::ACCESS_VIOLATION, "the file name names no file"));
    }
    Ok(components)
}

/// The file a read request names, if it is a regular file inside `root` (which must be
/// canonical). Symbolic links are followed and must not lead out of the root.
pub(crate) async fn resolve_read(root: &Path, name: &str) -> Result<PathBuf, Denial> {
    let mut candidate = root.to_path_buf();
    candidate.extend(components(name)?);
    let resolved = match tokio::fs::canonicalize(&candidate).await {
        Ok(resolved) => resolved,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(deny(code::FILE_NOT_FOUND, "file not found"));
        }
        Err(error) => {
            return Err(deny(
                code::ACCESS_VIOLATION,
                format!("the file cannot be accessed ({error})"),
            ));
        }
    };
    if !resolved.starts_with(root) {
        return Err(deny(
            code::ACCESS_VIOLATION,
            "the name is a link that leads outside the TFTP root",
        ));
    }
    match tokio::fs::metadata(&resolved).await {
        Ok(metadata) if metadata.is_file() => Ok(resolved),
        Ok(metadata) if metadata.is_dir() => Err(deny(
            code::ACCESS_VIOLATION,
            "the name is a directory, not a file",
        )),
        Ok(_) => Err(deny(
            code::ACCESS_VIOLATION,
            "the name is not a regular file",
        )),
        Err(error) => Err(deny(
            code::ACCESS_VIOLATION,
            format!("the file cannot be accessed ({error})"),
        )),
    }
}

/// Where a write request may store its file: an existing directory inside `root` (which must
/// be canonical) plus a new name, or a regular file that may be replaced.
pub(crate) async fn resolve_write(
    root: &Path,
    name: &str,
    allow_overwrite: bool,
) -> Result<PathBuf, Denial> {
    let mut components = components(name)?;
    let file_name = components
        .pop()
        .expect("components() never returns an empty list");
    let mut directory = root.to_path_buf();
    directory.extend(components);
    let directory = match tokio::fs::canonicalize(&directory).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(deny(
                code::FILE_NOT_FOUND,
                "the directory does not exist on the server",
            ));
        }
        Err(error) => {
            return Err(deny(
                code::ACCESS_VIOLATION,
                format!("the directory cannot be accessed ({error})"),
            ));
        }
    };
    if !directory.starts_with(root) {
        return Err(deny(
            code::ACCESS_VIOLATION,
            "the directory is a link that leads outside the TFTP root",
        ));
    }
    let target = directory.join(file_name);
    // The target itself is looked at without following links: a link is never written through.
    match tokio::fs::symlink_metadata(&target).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::metadata(&directory).await {
                Ok(metadata) if metadata.is_dir() => Ok(target),
                _ => Err(deny(
                    code::ACCESS_VIOLATION,
                    "the parent of the file is not a directory",
                )),
            }
        }
        Err(error) => Err(deny(
            code::ACCESS_VIOLATION,
            format!("the file cannot be accessed ({error})"),
        )),
        Ok(metadata) if metadata.file_type().is_symlink() => Err(deny(
            code::ACCESS_VIOLATION,
            "the name is a symbolic link; refusing to write through it",
        )),
        Ok(metadata) if !metadata.is_file() => Err(deny(
            code::ACCESS_VIOLATION,
            "the name exists and is not a regular file",
        )),
        Ok(_) if !allow_overwrite => Err(deny(
            code::FILE_EXISTS,
            "the file already exists and overwriting is disabled on this server",
        )),
        Ok(_) => Ok(target),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn names_are_split_into_safe_components() {
        assert_eq!(components("startup-config").unwrap(), ["startup-config"]);
        assert_eq!(
            components("configs/sw1.cfg").unwrap(),
            ["configs", "sw1.cfg"]
        );
        assert_eq!(
            components("configs\\core\\r1.cfg").unwrap(),
            ["configs", "core", "r1.cfg"]
        );
        assert_eq!(components("./a//b/./c").unwrap(), ["a", "b", "c"]);
        assert_eq!(
            components("c9300-universalk9.17.09.04a.SPA.bin")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(components("comet.cfg").unwrap(), ["comet.cfg"]);
        assert_eq!(components("console.log").unwrap(), ["console.log"]);
    }

    #[test]
    fn escapes_and_devices_are_refused() {
        let cases = [
            ("", "empty"),
            ("/etc/passwd", "absolute paths"),
            ("\\windows\\system32\\config\\sam", "absolute paths"),
            ("C:\\boot.ini", "absolute paths"),
            ("c:boot.ini", "absolute paths"),
            ("../secret", "'..'"),
            ("configs/../../secret", "'..'"),
            ("configs\\..\\..\\secret", "'..'"),
            ("a/..", "'..'"),
            ("NUL", "device name"),
            ("nul.txt", "device name"),
            ("con", "device name"),
            ("configs/COM1", "device name"),
            ("LPT9.cfg", "device name"),
            ("aux .txt", "device name"),
            ("file.", "portable"),
            ("file ", "portable"),
            ("file.txt:stream", "portable"),
            ("a\nb", "control characters"),
            ("./", "names no file"),
            ("///", "absolute paths"),
        ];
        for (name, expected) in cases {
            let denial = components(name).unwrap_err();
            assert_eq!(denial.code, code::ACCESS_VIOLATION, "{name:?}");
            assert!(
                denial.reason.contains(expected),
                "{name:?}: {}",
                denial.reason
            );
        }
        let long = "a".repeat(2000);
        assert!(components(&long).is_err());
    }

    #[tokio::test]
    async fn reads_are_confined_to_regular_files_under_the_root() {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "top secret").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("configs")).unwrap();
        std::fs::write(root.join("configs/sw1.cfg"), "hostname sw1\n").unwrap();

        assert_eq!(
            resolve_read(&root, "configs/sw1.cfg").await.unwrap(),
            root.join("configs/sw1.cfg")
        );
        assert_eq!(
            resolve_read(&root, "configs//./sw1.cfg").await.unwrap(),
            root.join("configs/sw1.cfg")
        );

        let missing = resolve_read(&root, "configs/sw2.cfg").await.unwrap_err();
        assert_eq!(
            (missing.code, missing.reason.as_str()),
            (code::FILE_NOT_FOUND, "file not found")
        );
        let directory = resolve_read(&root, "configs").await.unwrap_err();
        assert_eq!(directory.code, code::ACCESS_VIOLATION);
        assert!(
            directory.reason.contains("directory"),
            "{}",
            directory.reason
        );
        assert_eq!(
            resolve_read(&root, "../secret").await.unwrap_err().code,
            code::ACCESS_VIOLATION
        );

        #[cfg(unix)]
        {
            // A link inside the root that points out of it, to a file and to a directory.
            std::os::unix::fs::symlink(outside.path().join("secret"), root.join("link-to-secret"))
                .unwrap();
            std::os::unix::fs::symlink(outside.path(), root.join("link-to-dir")).unwrap();
            std::os::unix::fs::symlink(root.join("configs/sw1.cfg"), root.join("link-inside"))
                .unwrap();
            std::os::unix::fs::symlink("/dev/zero", root.join("zero")).unwrap();
            for name in ["link-to-secret", "link-to-dir/secret", "zero"] {
                let denial = resolve_read(&root, name).await.unwrap_err();
                assert_eq!(denial.code, code::ACCESS_VIOLATION, "{name}");
                assert!(
                    denial.reason.contains("outside the TFTP root"),
                    "{name}: {}",
                    denial.reason
                );
            }
            // A link that stays inside is fine.
            assert_eq!(
                resolve_read(&root, "link-inside").await.unwrap(),
                root.join("configs/sw1.cfg")
            );
        }
    }

    #[tokio::test]
    async fn writes_go_to_existing_directories_and_never_through_links() {
        let outside = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(root.join("backups")).unwrap();
        std::fs::write(root.join("backups/old.cfg"), "old").unwrap();

        assert_eq!(
            resolve_write(&root, "new.cfg", false).await.unwrap(),
            root.join("new.cfg")
        );
        assert_eq!(
            resolve_write(&root, "backups/new.cfg", false)
                .await
                .unwrap(),
            root.join("backups/new.cfg")
        );

        let exists = resolve_write(&root, "backups/old.cfg", false)
            .await
            .unwrap_err();
        assert_eq!(exists.code, code::FILE_EXISTS);
        assert_eq!(
            resolve_write(&root, "backups/old.cfg", true).await.unwrap(),
            root.join("backups/old.cfg")
        );

        let no_directory = resolve_write(&root, "no-such-dir/new.cfg", true)
            .await
            .unwrap_err();
        assert_eq!(no_directory.code, code::FILE_NOT_FOUND);
        let onto_directory = resolve_write(&root, "backups", true).await.unwrap_err();
        assert_eq!(onto_directory.code, code::ACCESS_VIOLATION);
        assert_eq!(
            resolve_write(&root, "../escape", true)
                .await
                .unwrap_err()
                .code,
            code::ACCESS_VIOLATION
        );
        assert_eq!(
            resolve_write(&root, "/tmp/escape", true)
                .await
                .unwrap_err()
                .code,
            code::ACCESS_VIOLATION
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.join("link-to-dir")).unwrap();
            std::os::unix::fs::symlink(outside.path().join("target"), root.join("link-to-file"))
                .unwrap();
            std::os::unix::fs::symlink(root.join("backups/old.cfg"), root.join("link-inside"))
                .unwrap();
            let through_directory = resolve_write(&root, "link-to-dir/new.cfg", true)
                .await
                .unwrap_err();
            assert!(
                through_directory.reason.contains("outside the TFTP root"),
                "{}",
                through_directory.reason
            );
            for name in ["link-to-file", "link-inside"] {
                let denial = resolve_write(&root, name, true).await.unwrap_err();
                assert_eq!(denial.code, code::ACCESS_VIOLATION, "{name}");
                assert!(
                    denial.reason.contains("symbolic link"),
                    "{name}: {}",
                    denial.reason
                );
            }
            assert!(!outside.path().join("target").exists());
        }
    }
}
