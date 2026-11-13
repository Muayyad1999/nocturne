//! Where the application keeps its database, its vault key and its logs.
//!
//! The desktop app and `nocturne-server` must agree on this, or the two would
//! quietly manage different inventories. Both use the platform's data
//! directory plus `Nocturne`, and both honour `NOCTURNE_DATA_DIR`.

use std::path::PathBuf;

/// Environment variable that overrides the default location.
pub const ENV: &str = "NOCTURNE_DATA_DIR";

/// Where the data directory is, in order of precedence: the `--data-dir`
/// argument, then `NOCTURNE_DATA_DIR`, then the platform's own place.
pub fn resolve(argument: Option<&str>, from_env: Option<String>) -> anyhow::Result<PathBuf> {
    if let Some(path) = argument.map(str::trim).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = from_env.map(|path| path.trim().to_string()).filter(|path| !path.is_empty()) {
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

pub const USAGE: &str = "\
Nocturne — the NOC that never sleeps.

Usage: nocturne [--data-dir <folder>] [--log-level <level>]

  --data-dir <folder>   Where to keep the database, vault key and logs.
                        Default: the platform's data folder plus `Nocturne`.
  --log-level <level>   error, warn, info (default), debug or trace.
  -V, --version         Print the version and exit.
  -h, --help            Print this text and exit.

The same data folder cannot be used by two instances at once, including by
`nocturne-server`.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_argument_wins_over_the_environment_which_wins_over_the_platform() {
        assert_eq!(resolve(Some("/tmp/a"), Some("/tmp/b".into())).unwrap(), PathBuf::from("/tmp/a"));
        assert_eq!(resolve(None, Some("/tmp/b".into())).unwrap(), PathBuf::from("/tmp/b"));
        // Blank is as good as absent: an empty variable must not mean "the current folder".
        assert_eq!(resolve(Some("  "), Some("  ".into())).unwrap(), dirs::data_dir().unwrap().join("Nocturne"));
        assert_eq!(resolve(None, None).unwrap().file_name().unwrap(), "Nocturne");
    }

    #[test]
    fn arguments_are_parsed_in_both_spellings_and_the_rest_is_kept() {
        let parse = |line: &str| parse_arguments(line.split_whitespace().map(str::to_string));
        assert_eq!(parse("--data-dir /tmp/x").data_dir.as_deref(), Some("/tmp/x"));
        assert_eq!(parse("--data-dir=/tmp/x").data_dir.as_deref(), Some("/tmp/x"));
        assert_eq!(parse("--log-level debug").log_level.as_deref(), Some("debug"));
        assert!(parse("--version").version && parse("-V").version);
        assert!(parse("--help").help);
        assert_eq!(parse("--nonsense x").unknown, vec!["--nonsense".to_string(), "x".to_string()]);
        assert_eq!(parse("").data_dir, None);
    }
}
