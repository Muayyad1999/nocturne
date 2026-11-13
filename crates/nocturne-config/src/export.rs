//! Exporting the latest configurations to a folder on this machine.
//!
//! The folder is either new, empty, or one an earlier export wrote (it then
//! holds a marker file). Anything else is refused: the export overwrites and
//! tidies up after itself, and must never do that among someone's own files.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use nocturne_app::inventory::DeviceFilter;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::redact::Redaction;
use crate::scope::{self, Target};
use crate::versions::{self, Kind};

const MARKER: &str = ".nocturne-export.json";
/// Files the operating system drops into folders by itself; they do not make a folder "in use".
const OS_FILES: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];

/// What an export leaves in its folder so that the next one recognises it.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Marker {
    exported_at: i64,
    redacted: bool,
    /// The files this export wrote, so that the next one can remove what it no longer writes.
    files: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SkippedDevice {
    device_id: String,
    name: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct ExportResult {
    dir: String,
    /// Files written.
    files: usize,
    /// Devices that have at least one file.
    devices: usize,
    skipped: Vec<SkippedDevice>,
    /// Files of the previous export removed because nothing produces them any more.
    removed: usize,
    redacted: bool,
}

/// A device name as a file name that every desktop operating system accepts.
fn file_stem(name: &str) -> String {
    let mut stem: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    // No hidden files, no `..`, and nothing Windows would silently trim.
    stem = stem.trim_matches([' ', '.']).to_string();
    while stem.len() > 180 {
        stem.pop();
    }
    let device = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (device.len() == 4
            && (device.starts_with("COM") || device.starts_with("LPT"))
            && device.ends_with(|c: char| c.is_ascii_digit() && c != '0'));
    if stem.is_empty() || reserved {
        stem.insert(0, '_');
    }
    stem
}

fn io_error(what: &str, path: &Path, err: std::io::Error) -> AppError {
    AppError::invalid(format!("cannot {what} `{}`: {err}", path.display()))
}

/// Check that `dir` may be exported into (creating it if needed) and return
/// what the previous export left there.
fn prepare(dir: &Path) -> AppResult<Marker> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|err| io_error("create", dir, err))?;
        return Ok(Marker::default());
    }
    if !dir.is_dir() {
        return Err(AppError::invalid(format!(
            "`{}` is a file, not a folder",
            dir.display()
        )));
    }
    let marker = dir.join(MARKER);
    if marker.is_file() {
        let text =
            std::fs::read_to_string(&marker).map_err(|err| io_error("read", &marker, err))?;
        // A damaged marker still marks the folder as ours; it just lists nothing to tidy up.
        return Ok(serde_json::from_str(&text).unwrap_or_default());
    }
    let occupied = std::fs::read_dir(dir)
        .map_err(|err| io_error("read", dir, err))?
        .filter_map(Result::ok)
        .any(|entry| !OS_FILES.contains(&entry.file_name().to_string_lossy().as_ref()));
    if occupied {
        return Err(AppError::conflict(format!(
            "`{}` is not empty and was not written by an earlier export. Choose an empty folder, or a new one (it is created).",
            dir.display()
        )));
    }
    Ok(Marker::default())
}

fn write_private(path: &Path, content: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    // Configurations hold credentials: readable by the exporting user only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(content.as_bytes())
}

fn export(app: &App, dir: &Path, targets: &[Target], redact: bool) -> AppResult<ExportResult> {
    let previous = prepare(dir)?;
    let redaction = redact.then(|| {
        Redaction::for_profiles(app, targets.iter().map(|target| target.profile.as_str()))
    });

    let mut written: Vec<String> = Vec::new();
    // Lower-cased, because most desktop file systems do not tell `SW1` from `sw1`.
    let mut taken: HashSet<String> = HashSet::new();
    let mut skipped = Vec::new();
    let mut devices = 0;
    for target in targets {
        let device = &target.device;
        let mut stem = file_stem(&device.name);
        if taken.contains(&stem.to_lowercase()) {
            stem = format!("{stem}~{}", device.id);
        }
        taken.insert(stem.to_lowercase());

        let mut any = false;
        for kind in [Kind::Running, Kind::Startup] {
            let Some(version) = versions::latest(app, &device.id, kind)? else {
                continue;
            };
            let (_, text) = app.store().config_get(version.id)?;
            let text = match &redaction {
                Some(redaction) => redaction.text(&text),
                None => text,
            };
            let name = format!("{stem}.{}.cfg", kind.as_str());
            let path = dir.join(&name);
            write_private(&path, &text).map_err(|err| io_error("write", &path, err))?;
            written.push(name);
            any = true;
        }
        if any {
            devices += 1;
        } else {
            skipped.push(SkippedDevice {
                device_id: device.id.clone(),
                name: device.name.clone(),
                reason: "no backup yet".into(),
            });
        }
    }

    // Files of the last export that nothing produces any more (a deleted or renamed device).
    let mut removed = 0;
    for stale in previous.files.iter().filter(|name| !written.contains(name)) {
        // Only plain file names this feature wrote; never follow a path out of the folder.
        if Path::new(stale).file_name().and_then(|name| name.to_str()) == Some(stale.as_str())
            && std::fs::remove_file(dir.join(stale)).is_ok()
        {
            removed += 1;
        }
    }
    let marker = Marker {
        exported_at: now_ms(),
        redacted: redact,
        files: written.clone(),
    };
    let marker_path = dir.join(MARKER);
    write_private(&marker_path, &serde_json::to_string_pretty(&marker)?)
        .map_err(|err| io_error("write", &marker_path, err))?;

    Ok(ExportResult {
        dir: dir.display().to_string(),
        files: written.len(),
        devices,
        skipped,
        removed,
        redacted: redact,
    })
}

#[derive(Deserialize)]
struct ExportParams {
    dir: String,
    filter: Option<DeviceFilter>,
    device_ids: Option<Vec<String>>,
    redact: Option<bool>,
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "config.export",
        |app: App, params: ExportParams| async move {
            app.require_local_access("Exporting configurations to a folder")?;
            let dir = PathBuf::from(params.dir.trim());
            if !dir.is_absolute() {
                return Err(AppError::invalid(
                    "`dir` must be the full path of the folder to export into",
                ));
            }
            let targets =
                scope::targets(&app, params.device_ids.as_deref(), params.filter.as_ref())?;
            let redact = params.redact.unwrap_or(false);
            let result = {
                let app = app.clone();
                tokio::task::spawn_blocking(move || export(&app, &dir, &targets, redact))
                    .await
                    .map_err(|err| {
                        AppError::internal(format!("the export stopped unexpectedly: {err}"))
                    })??
            };
            app.audit(
                "config.export",
                None,
                format!(
                    "Exported the {}configurations of {} device{} to `{}`",
                    if redact { "redacted " } else { "unredacted " },
                    result.devices,
                    if result.devices == 1 { "" } else { "s" },
                    result.dir
                ),
                json!({"dir": result.dir, "files": result.files, "redacted": redact}),
            );
            Ok(result)
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_become_safe_file_names() {
        assert_eq!(file_stem("CORE-SW1"), "CORE-SW1");
        assert_eq!(file_stem("sw1.lab.example"), "sw1.lab.example");
        assert_eq!(file_stem("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(file_stem("rack 3: sw*2?"), "rack 3_ sw_2_");
        assert_eq!(file_stem(" . "), "_");
        assert_eq!(file_stem("nul"), "_nul");
        assert_eq!(file_stem("COM1.core"), "_COM1.core");
        assert_eq!(file_stem("COMET"), "COMET");
        assert_eq!(file_stem("tab\tname"), "tab_name");
    }
}
