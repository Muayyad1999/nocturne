//! Decides which web UI is compiled into the binary.
//!
//! The real UI lives in `ui/dist` and exists only after `pnpm --dir ui build`.
//! When it is there, `cfg(nocturne_ui_built)` is set and `src/ui.rs` embeds it.
//! When it is not, the crate still builds and serves the small page in
//! `fallback-ui/` instead. This script only looks: it never creates, changes
//! or removes anything under `ui/`.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR for build scripts"),
    );
    let dist = manifest_dir.join("../../ui/dist");

    println!("cargo::rustc-check-cfg=cfg(nocturne_ui_built)");
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=fallback-ui");
    // Watched even while it does not exist. Cargo treats a missing path as
    // always changed, so until the UI has been built once this crate is
    // recompiled by every build; that is the price of picking the UI up on the
    // first build after it appears instead of silently shipping the fallback.
    println!("cargo::rerun-if-changed={}", dist.display());

    if dist.join("index.html").is_file() {
        println!("cargo::rustc-cfg=nocturne_ui_built");
    }
}
