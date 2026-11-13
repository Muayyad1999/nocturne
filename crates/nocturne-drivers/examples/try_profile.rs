//! Try a profile's collectors on recorded command output.
//!
//! ```text
//! cargo run -p nocturne-drivers --example try_profile -- <profile> <directory> [kind ...]
//! ```
//!
//! `<profile>` is the id of a built-in profile, or the path of a profile
//! file (loaded on top of the built-in ones, so it may `extends:` them).
//! `<directory>` holds one text file per command, named after the command
//! (`show version` → `show_version.txt`; run without a directory to list the
//! names the profile expects). The collected rows, the problems and what
//! each step did are printed as YAML.
//!
//! This is for looking at what a profile makes of some output while writing
//! it. The expectations in `tests/profiles/` are written by reading the raw
//! output, not copied from here.

use std::path::Path;
use std::process::ExitCode;

use nocturne_drivers::collect::{Collector, Recorded, command_slug};
use nocturne_drivers::model::Kind;
use nocturne_drivers::profile::{Origin, Registry};

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(profile) = args.first() else {
        eprintln!("usage: try_profile <profile id | profile.yaml> [directory [kind ...]]");
        return ExitCode::FAILURE;
    };

    let (mut registry, errors) = Registry::builtin();
    for error in errors {
        eprintln!("warning: {error}");
    }
    let platform = if Path::new(profile).is_file() {
        let yaml = match std::fs::read_to_string(profile) {
            Ok(yaml) => yaml,
            Err(err) => {
                eprintln!("{profile}: {err}");
                return ExitCode::FAILURE;
            }
        };
        match registry.add(&yaml, Origin::User) {
            Ok(platform) => platform,
            Err(err) => {
                eprintln!("{profile} does not load:");
                for problem in err.problems {
                    eprintln!("  {problem}");
                }
                return ExitCode::FAILURE;
            }
        }
    } else {
        match registry.get(profile) {
            Some(platform) => platform,
            None => {
                let known: Vec<String> = registry.list().into_iter().map(|p| p.id).collect();
                eprintln!("no profile `{profile}`; built in: {}", known.join(", "));
                return ExitCode::FAILURE;
            }
        }
    };

    let commands: Vec<(Kind, String)> = platform
        .profile
        .collect
        .iter()
        .flat_map(|(kind, steps)| {
            steps
                .iter()
                .map(|step| (*kind, step.command.trim().to_string()))
        })
        .collect();
    let Some(directory) = args.get(1) else {
        println!(
            "`{}` collects with these commands; record each one's output as:",
            platform.id()
        );
        for (kind, command) in &commands {
            println!(
                "  {:<44} {kind}: {command}",
                format!("{}.txt", command_slug(command))
            );
        }
        return ExitCode::SUCCESS;
    };

    let mut device = Recorded::new();
    let mut found = 0;
    for (_, command) in &commands {
        let file = Path::new(directory).join(format!("{}.txt", command_slug(command)));
        if let Ok(output) = std::fs::read_to_string(&file) {
            device = device.with(command, &output);
            found += 1;
        }
    }
    if found == 0 {
        eprintln!(
            "{directory}: none of the files this profile's commands would be recorded as is there (run without a directory to list them)"
        );
        return ExitCode::FAILURE;
    }

    let mut kinds: Vec<Kind> = Vec::new();
    for name in &args[2..] {
        match Kind::ALL.into_iter().find(|kind| kind.as_str() == name) {
            Some(kind) => kinds.push(kind),
            None => {
                eprintln!(
                    "`{name}` is not a kind; the kinds are {}",
                    Kind::ALL.map(Kind::as_str).join(", ")
                );
                return ExitCode::FAILURE;
            }
        }
    }
    if kinds.is_empty() {
        kinds = platform.profile.collect.keys().copied().collect();
    }

    let snapshot = Collector::new(registry.expressions())
        .collect(&mut device, &platform, &kinds)
        .await;
    match serde_yaml_ng::to_string(&snapshot) {
        Ok(yaml) => {
            println!("{yaml}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
