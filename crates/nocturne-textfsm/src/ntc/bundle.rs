//! The embedded copy of `ntc_templates/templates/`.
//!
//! `build.rs` packs the `index` file and every template into one zlib stream.
//! It is inflated on first use and then lives for the rest of the process, so
//! template names and sources can be handed out as `&'static str`.

use std::io::Read;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

use flate2::read::ZlibDecoder;

use crate::error::TemplateError;
use crate::template::Template;

/// The upstream commit the bundle was copied from (see `templates/README.md`).
pub(crate) const UPSTREAM_REVISION: &str = "84ae82fd022807810d73a19655cca113b734de58";

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ntc-templates.zlib"));

pub(crate) struct Bundle {
    /// Manifest and file contents, as laid out by `build.rs`.
    text: String,
    index: Range<usize>,
    /// Sorted by name.
    templates: Vec<Entry>,
}

struct Entry {
    name: Range<usize>,
    source: Range<usize>,
    compiled: OnceLock<Result<Arc<Template>, TemplateError>>,
}

impl Bundle {
    pub fn get() -> &'static Bundle {
        static BUNDLE: OnceLock<Bundle> = OnceLock::new();
        BUNDLE.get_or_init(|| Bundle::unpack().expect("the bundle written by build.rs is well formed"))
    }

    fn unpack() -> Option<Bundle> {
        let mut text = String::new();
        ZlibDecoder::new(BLOB).read_to_string(&mut text).ok()?;
        let manifest_len = text.find("\n\n")? + 1;
        let mut offset = manifest_len + 1;
        let mut files = Vec::new();
        let mut line_start = 0;
        for line in text[..manifest_len].lines() {
            let (name, len) = line.split_once('\t')?;
            let source = offset..offset + len.parse::<usize>().ok()?;
            offset = source.end;
            files.push((line_start..line_start + name.len(), source));
            line_start += line.len() + 1;
        }
        if offset != text.len() {
            return None;
        }
        let mut files = files.into_iter();
        let (_, index) = files.next()?;
        let templates =
            files.map(|(name, source)| Entry { name, source, compiled: OnceLock::new() }).collect();
        Some(Bundle { text, index, templates })
    }

    /// The contents of the `index` file.
    pub fn index(&self) -> &str {
        &self.text[self.index.clone()]
    }

    /// Template names (file stems) in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.templates.iter().map(|entry| &self.text[entry.name.clone()])
    }

    pub fn source(&self, name: &str) -> Option<&str> {
        self.entry(name).map(|entry| &self.text[entry.source.clone()])
    }

    /// The compiled template, compiled on first use.
    pub fn template(&self, name: &str) -> Result<Option<Arc<Template>>, TemplateError> {
        let Some(entry) = self.entry(name) else {
            return Ok(None);
        };
        entry
            .compiled
            .get_or_init(|| Template::parse(&self.text[entry.source.clone()]).map(Arc::new))
            .clone()
            .map(Some)
    }

    fn entry(&self, name: &str) -> Option<&Entry> {
        self.templates
            .binary_search_by(|entry| self.text[entry.name.clone()].cmp(name))
            .ok()
            .map(|position| &self.templates[position])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpacks_every_file() {
        let bundle = Bundle::get();
        assert_eq!(bundle.names().count(), 979);
        assert!(bundle.names().is_sorted());
        assert!(bundle.index().contains("Template, Hostname, Platform, Command"));
        let source = bundle.source("cisco_ios_show_version").unwrap();
        assert!(source.starts_with("Value "));
        assert_eq!(bundle.source("index"), None);
        assert_eq!(bundle.source("no_such_template"), None);
    }

    #[test]
    fn bundle_matches_the_files_on_disk() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
        let bundle = Bundle::get();
        assert_eq!(bundle.index(), std::fs::read_to_string(directory.join("index")).unwrap());
        for name in bundle.names() {
            let on_disk = std::fs::read_to_string(directory.join(format!("{name}.textfsm"))).unwrap();
            assert_eq!(bundle.source(name), Some(on_disk.as_str()), "{name}");
        }
    }

    #[test]
    fn readme_states_the_bundled_revision() {
        let readme = include_str!("../../templates/README.md");
        assert!(readme.contains(UPSTREAM_REVISION));
    }

    #[test]
    fn compiles_lazily_and_caches() {
        let bundle = Bundle::get();
        let first = bundle.template("cisco_ios_show_clock").unwrap().unwrap();
        let second = bundle.template("cisco_ios_show_clock").unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(bundle.template("no_such_template").unwrap().is_none());
    }
}
