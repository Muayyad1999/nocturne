//! Where the application keeps its database, its vault key and its logs.
//!
//! The desktop app and `nocturne-server` must agree on this, or the two would
//! quietly manage different inventories. Both use the platform's data
//! directory plus `Nocturne`, and both honour `NOCTURNE_DATA_DIR`.

use std::path::{Path, PathBuf};

/// Environment variable that overrides the default location.
pub const ENV: &str = "NOCTURNE_DATA_DIR";

/// Where the data directory is, in order of precedence: the `--data-dir`
/// argument, then `NOCTURNE_DATA_DIR`, then the platform's own place.
pub fn resolve(argument: Option<&str>, from_env: Option<String>) -> anyhow::Result<PathBuf> {
    if let Some(path) = argument.map(str::trim).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = from_env
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
    {
        return Ok(PathBuf::from(path));
    }
    dirs::data_dir().map(|dir| dir.join("Nocturne")).ok_or_else(|| {
        anyhow::anyhow!(
            "this system does not say where application data belongs, so Nocturne cannot choose \
             a place for its database. Start it with --data-dir <folder>, or set {ENV}."
        )
    })
}

/// The arguments the shell understands. Everything else is reported rather
/// than ignored, so a typo does not silently start with the wrong settings.
#[derive(Debug, Default, PartialEq)]
pub struct Arguments {
    pub data_dir: Option<String>,
    pub log_level: Option<String>,
    /// `--version` / `-V`: print the version and exit.
    pub version: bool,
    pub help: bool,
    pub unknown: Vec<String>,
}

pub fn parse_arguments(args: impl IntoIterator<Item = String>) -> Arguments {
    let mut parsed = Arguments::default();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name.to_string(), Some(value.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next());
        match name.as_str() {
            "--data-dir" => parsed.data_dir = value(),
            "--log-level" => parsed.log_level = value(),
            "--version" | "-V" => parsed.version = true,
            "--help" | "-h" => parsed.help = true,
            // WebView and single-instance machinery pass arguments of their own.
            _ if name.starts_with('-') => parsed.unknown.push(arg),
            _ => parsed.unknown.push(arg),
        }
    }
    parsed
}

/// The folder a second launch named with `--data-dir`, when it is not the
/// one this copy has open.
///
/// A second launch never opens anything: it hands over to the copy that is
/// running, and that copy's window comes forward. When the launch asked for
/// other data than that window shows, coming forward quietly would pass one
/// inventory off as another, so the copy that stays has to say so — and this
/// is how it knows.
///
/// `arguments` and `cwd` are what the handover carries: the other process's
/// command line, program first, and the folder it was started in. Its
/// environment is not carried, so a launch that chose its folder with
/// `NOCTURNE_DATA_DIR` looks like one that chose none.
pub fn asked_for_another(arguments: &[String], cwd: &str, open: &Path) -> Option<PathBuf> {
    let named = parse_arguments(arguments.iter().skip(1).cloned()).data_dir?;
    let named = named.trim();
    if named.is_empty() {
        return None;
    }
    // Relative to where *that* launch was started, not to where this copy was.
    let asked = Path::new(cwd).join(named);
    (!same_folder(&asked, open)).then_some(asked)
}

/// Whether two paths are one folder, as far as can be told: a folder that
/// does not exist yet has no links to follow and is compared as written.
fn same_folder(one: &Path, other: &Path) -> bool {
    let settled = |path: &Path| {
        std::fs::canonicalize(path)
            .or_else(|_| std::path::absolute(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    settled(one) == settled(other)
}

/// What this program is called where it is installed. The two editions are
/// one program under two names — `nocturne` and `nocturne-isp` — so that
/// both can be installed, and what is printed should be the name that works.
pub fn program_name() -> String {
    std::env::args_os()
        .next()
        .as_deref()
        .map(Path::new)
        .and_then(Path::file_stem)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("nocturne")
        .to_string()
}

pub fn usage(program: &str) -> String {
    format!(
        "\
Nocturne — the NOC that never sleeps.

Usage: {program} [--data-dir <folder>] [--log-level <level>]

  --data-dir <folder>   Where to keep the database, vault key and logs.
                        Default: the platform's data folder plus `Nocturne`.
  --log-level <level>   error, warn, info (default), debug or trace.
  -V, --version         Print the version and exit.
  -h, --help            Print this text and exit.

One copy runs at a time: starting it again brings the window of the copy that
is running forward. The same data folder cannot be used by two programs at
once, and that includes `nocturne-server` and the other edition of Nocturne."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_argument_wins_over_the_environment_which_wins_over_the_platform() {
        assert_eq!(
            resolve(Some("/tmp/a"), Some("/tmp/b".into())).unwrap(),
            PathBuf::from("/tmp/a")
        );
        assert_eq!(
            resolve(None, Some("/tmp/b".into())).unwrap(),
            PathBuf::from("/tmp/b")
        );
        // Blank is as good as absent: an empty variable must not mean "the current folder".
        assert_eq!(
            resolve(Some("  "), Some("  ".into())).unwrap(),
            dirs::data_dir().unwrap().join("Nocturne")
        );
        assert_eq!(
            resolve(None, None).unwrap().file_name().unwrap(),
            "Nocturne"
        );
    }

    #[test]
    fn arguments_are_parsed_in_both_spellings_and_the_rest_is_kept() {
        let parse = |line: &str| parse_arguments(line.split_whitespace().map(str::to_string));
        assert_eq!(
            parse("--data-dir /tmp/x").data_dir.as_deref(),
            Some("/tmp/x")
        );
        assert_eq!(
            parse("--data-dir=/tmp/x").data_dir.as_deref(),
            Some("/tmp/x")
        );
        assert_eq!(
            parse("--log-level debug").log_level.as_deref(),
            Some("debug")
        );
        assert!(parse("--version").version && parse("-V").version);
        assert!(parse("--help").help);
        assert_eq!(
            parse("--nonsense x").unknown,
            vec!["--nonsense".to_string(), "x".to_string()]
        );
        assert_eq!(parse("").data_dir, None);
    }

    fn handed_over(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_second_launch_that_names_another_folder_is_noticed_and_one_that_names_this_one_is_not() {
        let here = tempfile::tempdir().unwrap();
        let open = here.path().join("open");
        std::fs::create_dir(&open).unwrap();
        let cwd = here.path().to_str().unwrap();
        let elsewhere = "/somewhere/else";

        // The launcher clicked twice: no folder named at all.
        assert_eq!(
            asked_for_another(&handed_over("/usr/bin/nocturne"), elsewhere, &open),
            None
        );
        assert_eq!(asked_for_another(&[], elsewhere, &open), None);

        // The folder that is open, however it was spelt.
        for line in [
            format!("nocturne --data-dir {}", open.display()),
            format!("nocturne --data-dir={}/", open.display()),
            format!(
                "nocturne --log-level debug --data-dir {}/../open",
                open.display()
            ),
        ] {
            assert_eq!(
                asked_for_another(&handed_over(&line), elsewhere, &open),
                None,
                "{line}"
            );
        }
        // Relative to where the second launch was started.
        assert_eq!(
            asked_for_another(&handed_over("nocturne --data-dir open"), cwd, &open),
            None
        );
        assert_eq!(
            asked_for_another(&handed_over("nocturne --data-dir ./open"), cwd, &open),
            None
        );

        // Another one — existing or not.
        assert_eq!(
            asked_for_another(&handed_over("nocturne --data-dir lab"), cwd, &open),
            Some(here.path().join("lab"))
        );
        assert_eq!(
            asked_for_another(&handed_over("nocturne --data-dir=/srv/other"), cwd, &open),
            Some(PathBuf::from("/srv/other"))
        );
        // The same name under another folder is another folder.
        assert_eq!(
            asked_for_another(&handed_over("nocturne --data-dir open"), elsewhere, &open),
            Some(PathBuf::from("/somewhere/else/open"))
        );
        // The program's own path is not an argument, whatever it looks like.
        assert_eq!(
            asked_for_another(&handed_over("--data-dir /srv/other"), cwd, &open),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_the_open_folder_is_the_open_folder() {
        let here = tempfile::tempdir().unwrap();
        let open = here.path().join("open");
        std::fs::create_dir(&open).unwrap();
        let link = here.path().join("link");
        std::os::unix::fs::symlink(&open, &link).unwrap();
        let line = handed_over(&format!("nocturne --data-dir {}", link.display()));
        assert_eq!(asked_for_another(&line, "/", &open), None);
    }

    #[test]
    fn the_help_names_the_program_as_it_is_installed() {
        assert!(usage("nocturne-isp").contains("Usage: nocturne-isp [--data-dir"));
        assert!(!program_name().is_empty());
    }
}
