//! Runs a directory of ntc-templates test cases through the bundled library.
//!
//! The layout is the one of the upstream repository:
//! `<root>/<platform>/<command>/<case>.raw` holds the output of a device and
//! `<case>.yml` the rows upstream expects for it under `parsed_sample`, with
//! lower-cased keys. A case is checked the way upstream's
//! `test_structured_data_against_parsed_reference_files.py` does it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use nocturne_textfsm::{Record, Value, ntc};

/// The outcome of running every case below a directory.
#[derive(Default)]
pub struct Report {
    /// Per platform: (passed, failed).
    pub platforms: BTreeMap<String, (usize, usize)>,
    /// Failed cases with the first difference found.
    pub failures: Vec<(String, String)>,
    /// Cases whose command the index did not resolve, so that the template
    /// named after the directory was used instead.
    pub fallbacks: Vec<String>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.platforms.values().map(|(passed, _)| passed).sum()
    }

    pub fn total(&self) -> usize {
        self.platforms.values().map(|(passed, failed)| passed + failed).sum()
    }

    /// The per-platform table and the list of failures.
    pub fn summary(&self) -> String {
        let mut text = format!("{:<28} {:>6} {:>6}\n", "platform", "passed", "failed");
        for (platform, (passed, failed)) in &self.platforms {
            text.push_str(&format!("{platform:<28} {passed:>6} {failed:>6}\n"));
        }
        let failed = self.total() - self.passed();
        text.push_str(&format!("{:<28} {:>6} {failed:>6}\n", "total", self.passed()));
        for (case, reason) in &self.failures {
            text.push_str(&format!("FAILED {case}: {reason}\n"));
        }
        text
    }
}

/// Runs all cases below `root`, except those listed in `skip` (paths relative to `root`).
pub fn run_cases(root: &Path, skip: &[&str]) -> Report {
    let mut report = Report::default();
    for platform_dir in subdirectories(root) {
        let platform = file_name(&platform_dir);
        for command_dir in subdirectories(&platform_dir) {
            let command = file_name(&command_dir);
            for raw in files_with_extension(&command_dir, "raw") {
                let case = format!("{platform}/{command}/{}", file_name(&raw));
                if skip.contains(&case.as_str()) {
                    continue;
                }
                let tally = report.platforms.entry(platform.clone()).or_default();
                match check_case(&platform, &command, &raw, &mut report.fallbacks) {
                    Ok(()) => tally.0 += 1,
                    Err(reason) => {
                        tally.1 += 1;
                        report.failures.push((case, reason));
                    }
                }
            }
        }
    }
    report
}

fn check_case(
    platform: &str,
    command_dir: &str,
    raw: &Path,
    fallbacks: &mut Vec<String>,
) -> Result<(), String> {
    let output = fs::read_to_string(raw).map_err(|error| format!("cannot read the output: {error}"))?;
    let expected = expected_rows(&raw.with_extension("yml"))?;

    // Upstream derives the command from the directory name.
    let command = command_dir.replace('_', " ");
    let parsed = ntc::parse(platform, &command, &output).map_err(|error| error.to_string())?;
    let rows = match parsed {
        Some(rows) => rows,
        None => {
            // The index needs the original punctuation: use the template named
            // after the directory.
            let name = format!("{platform}_{command_dir}");
            fallbacks.push(name.clone());
            let template = ntc::template(&name)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("no template for {command:?}"))?;
            template.run(&output).map_err(|error| error.to_string())?
        }
    };
    compare(&rows, &expected)
}

fn expected_rows(yml: &Path) -> Result<Vec<Record>, String> {
    #[derive(serde::Deserialize)]
    struct Reference {
        parsed_sample: Vec<Record>,
    }
    let text = fs::read_to_string(yml).map_err(|error| format!("cannot read the reference: {error}"))?;
    serde_yaml_ng::from_str::<Reference>(&text)
        .map(|reference| reference.parsed_sample)
        .map_err(|error| format!("cannot parse the reference: {error}"))
}

/// The first difference between the parsed rows and the reference, if any.
fn compare(rows: &[Record], expected: &[Record]) -> Result<(), String> {
    if rows.len() != expected.len() {
        return Err(format!("{} rows, expected {}", rows.len(), expected.len()));
    }
    for (number, (row, expected)) in rows.iter().zip(expected).enumerate() {
        // ntc-templates reports the value names in lower case.
        let row: BTreeMap<String, &Value> =
            row.iter().map(|(name, value)| (name.to_lowercase(), value)).collect();
        let mut names: Vec<&str> = row.keys().map(String::as_str).collect();
        let mut expected_names: Vec<&str> = expected.keys().map(String::as_str).collect();
        expected_names.sort_unstable();
        names.sort_unstable();
        if names != expected_names {
            return Err(format!("row {number}: keys {names:?}, expected {expected_names:?}"));
        }
        for (name, expected) in expected {
            let value = row[name.as_str()];
            if value != expected {
                return Err(format!("row {number}, {name}: {value:?}, expected {expected:?}"));
            }
        }
    }
    Ok(())
}

fn subdirectories(directory: &Path) -> Vec<PathBuf> {
    entries(directory).into_iter().filter(|path| path.is_dir()).collect()
}

fn files_with_extension(directory: &Path, extension: &str) -> Vec<PathBuf> {
    entries(directory)
        .into_iter()
        .filter(|path| path.is_file() && path.extension().is_some_and(|found| found == extension))
        .collect()
}

/// The entries of a directory, sorted by name.
fn entries(directory: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", directory.display()))
        .map(|entry| entry.expect("readable directory entry").path())
        .collect();
    paths.sort();
    paths
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}
