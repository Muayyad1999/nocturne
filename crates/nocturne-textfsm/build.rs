//! Packs the bundled ntc-templates (`templates/`) into one compressed blob
//! that `src/ntc/bundle.rs` embeds with `include_bytes!`.
//!
//! Layout before compression: a manifest with one `name<TAB>byte length` line
//! per file (the `index` first, then every template by file stem, sorted), an
//! empty line, and the file contents back to back in manifest order.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::ZlibEncoder;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=templates");

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo"));
    let templates = manifest_dir.join("templates");

    let mut names: Vec<String> = fs::read_dir(&templates)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", templates.display()))
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "textfsm"))
        .map(|path| {
            let stem = path.file_stem().and_then(|stem| stem.to_str());
            stem.unwrap_or_else(|| panic!("{} is not valid UTF-8", path.display())).to_owned()
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no *.textfsm files in {}", templates.display());

    let mut manifest = String::new();
    let mut contents = String::new();
    let mut add = |name: &str, path: &Path| {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        assert!(!name.contains(['\t', '\n']), "unsupported file name {name:?}");
        manifest.push_str(&format!("{name}\t{}\n", text.len()));
        contents.push_str(&text);
    };
    add("index", &templates.join("index"));
    for name in &names {
        add(name, &templates.join(format!("{name}.textfsm")));
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(format!("{manifest}\n{contents}").as_bytes())
        .and_then(|()| encoder.finish())
        .and_then(|blob| fs::write(out_dir.join("ntc-templates.zlib"), blob))
        .expect("cannot write the template bundle");
}
