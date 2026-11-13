//! The bundled ntc-templates library.
//!
//! The templates and the `index` file of
//! [networktocode/ntc-templates](https://github.com/networktocode/ntc-templates)
//! are embedded in the crate. Commands are resolved through the index exactly
//! like `ntc_templates.parse.parse_output` does (which delegates to
//! `textfsm.clitable`), and templates are compiled on first use.

mod bundle;
mod index;
mod merge;

use std::sync::{Arc, OnceLock};

pub use merge::MergeError;

use bundle::Bundle;
use index::Index;

use crate::{Error, Record, Template, TemplateError};

fn index() -> &'static Index<'static> {
    static INDEX: OnceLock<Index<'static>> = OnceLock::new();
    INDEX.get_or_init(|| Index::parse(Bundle::get().index()))
}

/// Platforms present in the index, e.g. "cisco_ios", "arista_eos", "mikrotik_routeros".
///
/// The list is sorted. The index describes platforms with regular expressions;
/// alternatives are listed individually (`hp_procurve` and `aruba_procurve`),
/// and a platform given to [`find`] only has to start with one of these names.
pub fn platforms() -> &'static [String] {
    static PLATFORMS: OnceLock<Vec<String>> = OnceLock::new();
    PLATFORMS.get_or_init(|| index().platforms())
}

/// All bundled template names (file stem, e.g. "cisco_ios_show_version"), sorted.
pub fn template_names() -> impl Iterator<Item = &'static str> {
    Bundle::get().names()
}

/// The source text of a bundled template.
pub fn source(name: &str) -> Option<&'static str> {
    Bundle::get().source(name)
}

/// A bundled template by name. Compiled lazily, cached for the life of the process.
pub fn template(name: &str) -> Result<Option<Arc<Template>>, TemplateError> {
    Bundle::get().template(name)
}

/// Resolve via the ntc `index` file: platform + the command as typed.
///
/// Abbreviations resolve as upstream: `sh ip int br` finds
/// `cisco_ios_show_ip_interface_brief`. Like upstream, the index entries are
/// matched against the *start* of the command, so trailing arguments or
/// output modifiers do not prevent a match, and the first matching entry wins.
///
/// A few commands are covered by several templates whose columns are
/// combined; this returns the first one (see [`find_all`]).
pub fn find(platform: &str, command: &str) -> Option<&'static str> {
    find_all(platform, command).and_then(|templates| templates.first().copied())
}

/// Like [`find`], but returns every template the index lists for the command.
///
/// [`parse`] runs all of them and merges their columns.
pub fn find_all(platform: &str, command: &str) -> Option<&'static [&'static str]> {
    // Python's `$` also matches in front of a final newline.
    let command = command.strip_suffix('\n').unwrap_or(command);
    index().find(platform, command)
}

/// Convenience: find + run. `Ok(None)` when no template covers the command.
///
/// When the index lists several templates for the command, the columns the
/// later ones add are merged into the rows of the first, matched on the
/// values declared as `Key` — the same table `parse_output` returns upstream.
pub fn parse(platform: &str, command: &str, output: &str) -> Result<Option<Vec<Record>>, Error> {
    let Some(names) = find_all(platform, command) else {
        return Ok(None);
    };
    let mut templates = Vec::with_capacity(names.len());
    for name in names {
        match template(name)? {
            Some(template) => templates.push(template),
            None => return Ok(None),
        }
    }
    let Some((first, rest)) = templates.split_first() else {
        return Ok(None);
    };
    let mut rows = first.run(output)?;
    let mut header = first.header().to_vec();
    let mut keys: Vec<String> = first.keys().map(str::to_owned).collect();
    for template in rest {
        let other = template.run(output)?;
        if keys.is_empty() {
            keys = template.keys().map(str::to_owned).collect();
        }
        merge::extend(&mut rows, &mut header, &other, template.header(), &keys)?;
    }
    Ok(Some(rows))
}

/// Upstream commit the bundle was taken from.
pub fn upstream_revision() -> &'static str {
    bundle::UPSTREAM_REVISION
}
