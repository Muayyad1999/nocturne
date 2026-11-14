//! Make cargo notice a new or edited device profile.
//!
//! `profile.rs` embeds `profiles/` with `include_dir!`, which cargo cannot see
//! into: it tracks the source files it compiles, and a YAML file is not one.
//! So adding `profiles/arista_eos.yaml` changed nothing until something else
//! forced the crate to rebuild — and the symptom is misleading rather than
//! absent. `Registry::builtin()` reports no errors, because as far as it knows
//! there is no such profile, and the test harness then says "no built-in
//! profile has the id `arista_eos`" about a file sitting right there. Two
//! people writing profiles lost time to it before anyone wrote this file.
//!
//! Every path printed here is one cargo will watch, so a profile added,
//! edited or removed rebuilds the crate by itself.

use std::path::Path;

fn main() {
    let profiles = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles");
    // The directory itself, so that a *new* file in it is noticed: watching
    // only the files that exist today would miss the case this is for.
    println!("cargo::rerun-if-changed={}", profiles.display());
    let Ok(entries) = std::fs::read_dir(&profiles) else {
        // No profiles directory is a problem for `include_dir!` to complain
        // about, with a better message than anything available here.
        return;
    };
    for entry in entries.flatten() {
        println!("cargo::rerun-if-changed={}", entry.path().display());
    }
}
