//! The ntc-templates `index` file: which template parses which command.
//!
//! This reproduces how `textfsm.clitable` reads and searches the file:
//!
//! * The first line that is not a comment names the columns. After it, lines
//!   starting with `#` are comments, cells are separated by commas and
//!   trimmed, and lines with the wrong number of cells are ignored.
//! * In the `Command` column `abc[[xyz]]` stands for `abc(x(y(z)?)?)?`.
//! * `Platform` and `Command` are regular expressions applied with `re.match`,
//!   so they only have to match a prefix of the platform and of the command.
//!   The first row that matches both wins.
//! * The `Template` cell may list several templates separated by `:`.

use std::sync::OnceLock;

use crate::pyregex::{self, Regex};

/// Stop enumerating the platforms a `Platform` regex matches beyond this many.
const MAX_PLATFORMS_PER_ROW: usize = 16;

pub(crate) struct Index<'a> {
    rows: Vec<Row<'a>>,
}

struct Row<'a> {
    /// File stems of the row's templates, the main one first.
    templates: Vec<&'a str>,
    platform: Matcher<'a>,
    command: Matcher<'a>,
}

/// A cell that is matched against an attribute of the request.
enum Matcher<'a> {
    /// An empty cell matches everything.
    Any,
    /// A cell without regex syntax: a plain prefix test.
    Prefix(&'a str),
    /// Compiled on first use; `None` if the cell is not a valid regex.
    Pattern { source: String, compiled: OnceLock<Option<Regex>> },
}

impl<'a> Matcher<'a> {
    fn new(cell: &'a str, expand_completion: bool) -> Matcher<'a> {
        if cell.is_empty() {
            Matcher::Any
        } else if cell.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            Matcher::Prefix(cell)
        } else {
            let source =
                if expand_completion { expand_completions(cell) } else { cell.to_owned() };
            Matcher::Pattern { source, compiled: OnceLock::new() }
        }
    }

    fn regex(&self) -> Option<&Regex> {
        match self {
            Matcher::Pattern { source, compiled } => {
                compiled.get_or_init(|| Regex::new(source).ok()).as_ref()
            }
            _ => None,
        }
    }

    fn matches(&self, text: &str) -> bool {
        match self {
            Matcher::Any => true,
            Matcher::Prefix(prefix) => text.starts_with(prefix),
            // A row whose regex cannot be evaluated matches nothing.
            Matcher::Pattern { .. } => {
                self.regex().is_some_and(|regex| matches!(regex.match_start(text), Ok(Some(_))))
            }
        }
    }
}

impl<'a> Index<'a> {
    pub fn parse(text: &'a str) -> Index<'a> {
        let mut lines = text.lines();
        // The header is the first line with content before any `#`.
        let header: Vec<&str> = lines
            .by_ref()
            .map(|line| line.split('#').next().unwrap_or_default().trim())
            .find(|line| !line.is_empty())
            .map(|line| line.split(',').map(str::trim).collect())
            .unwrap_or_default();
        let column = |name: &str| header.iter().position(|cell| *cell == name);
        let (template, platform, command) =
            (column("Template"), column("Platform"), column("Command"));

        let mut rows = Vec::new();
        for line in lines {
            if line.starts_with('#') {
                continue;
            }
            let cells: Vec<&str> = line.split(',').map(str::trim).collect();
            if cells.len() != header.len() {
                continue;
            }
            let Some(template) = template.map(|column| cells[column]) else { continue };
            let cell = |column: Option<usize>| column.map_or("", |column| cells[column]);
            rows.push(Row {
                templates: template
                    .split(':')
                    .map(|file| file.strip_suffix(".textfsm").unwrap_or(file))
                    .collect(),
                platform: Matcher::new(cell(platform), false),
                command: Matcher::new(cell(command), true),
            });
        }
        Index { rows }
    }

    /// The templates of the first row matching the platform and the command.
    pub fn find(&self, platform: &str, command: &str) -> Option<&[&'a str]> {
        self.rows
            .iter()
            .find(|row| row.platform.matches(platform) && row.command.matches(command))
            .map(|row| row.templates.as_slice())
    }

    /// Every platform name the index knows, sorted.
    ///
    /// A `Platform` cell is a regex. Plain names stand for themselves; a regex
    /// with a handful of matches such as `(hp|aruba)_procurve` contributes all
    /// of them; an open-ended one such as `.*vyos.*` contributes the platform
    /// its template files are named after (`vyatta_vyos`).
    pub fn platforms(&self) -> Vec<String> {
        let mut platforms: Vec<String> = Vec::new();
        for row in &self.rows {
            match &row.platform {
                Matcher::Any => {}
                Matcher::Prefix(name) => platforms.push((*name).to_owned()),
                Matcher::Pattern { source, .. } => {
                    match pyregex::finite_language(source, MAX_PLATFORMS_PER_ROW) {
                        Some(names) => platforms.extend(names),
                        None => platforms.extend(self.platform_of_template(row)),
                    }
                }
            }
        }
        platforms.sort();
        platforms.dedup();
        platforms
    }

    /// The shortest `_`-separated prefix of the row's template name that the
    /// row's platform regex accepts.
    fn platform_of_template(&self, row: &Row<'_>) -> Option<String> {
        let template = row.templates.first()?;
        template
            .match_indices('_')
            .map(|(end, _)| &template[..end])
            .chain([*template])
            .find(|prefix| row.platform.matches(prefix))
            .map(str::to_owned)
    }

    #[cfg(test)]
    fn rows(&self) -> impl Iterator<Item = &Row<'a>> {
        self.rows.iter()
    }
}

/// Rewrites `abc[[xyz]]` to `abc(x(y(z)?)?)?` (`CliTable._Completion`).
fn expand_completions(command: &str) -> String {
    let mut expanded = String::with_capacity(command.len() * 2);
    let mut rest = command;
    // Python substitutes the regex `\[\[.+?\]\]`: at least one character
    // between the brackets, up to the first `]]` after it.
    while let Some(open) = rest.find("[[") {
        let after_first = rest[open + 2..].chars().next().map_or(0, char::len_utf8);
        let search_from = open + 2 + after_first;
        let Some(close) = rest[search_from..].find("]]").filter(|_| after_first > 0) else {
            // Not a completion: keep one bracket and look again after it.
            expanded.push_str(&rest[..open + 1]);
            rest = &rest[open + 1..];
            continue;
        };
        let word = &rest[open + 2..search_from + close];
        expanded.push_str(&rest[..open]);
        for c in word.chars() {
            expanded.push('(');
            expanded.push(c);
        }
        for _ in word.chars() {
            expanded.push_str(")?");
        }
        rest = &rest[search_from + close + 2..];
    }
    expanded.push_str(rest);
    expanded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ntc::bundle::Bundle;

    #[test]
    fn expands_completions_like_clitable() {
        // Expected values produced by textfsm.clitable.CliTable._PreParse.
        assert_eq!(expand_completions("sh[[ow]] ver[[sion]]"), "sh(o(w)?)? ver(s(i(o(n)?)?)?)?");
        assert_eq!(expand_completions("show vlan"), "show vlan");
        assert_eq!(expand_completions("sh[[ow]] port-[[security]]"), "sh(o(w)?)? port-(s(e(c(u(r(i(t(y)?)?)?)?)?)?)?)?");
        assert_eq!(expand_completions("a[[b]]]c"), "a(b)?]c");
        assert_eq!(expand_completions("a[[]]]]"), "a(])?]");
        assert_eq!(expand_completions("a[[]]"), "a[[]]");
        assert_eq!(expand_completions("a[[b"), "a[[b");
        assert_eq!(expand_completions("[[[x]]"), "([(x)?)?");
        assert_eq!(expand_completions("x[[é]]"), "x(é)?");
    }

    #[test]
    fn parses_like_texttable() {
        let index = Index::parse(
            "# comment\n\
             \n\
             Template, Hostname, Platform, Command # trailing comment\n\
             \n\
             a_show_x.textfsm, .*, a, sh[[ow]] x\n\
             # a comment\n\
             too, few, cells\n\
             b_one.textfsm:b_two.textfsm, .*, (b|c)_os, show (one|two)$\n\
             any_platform.textfsm, .*, , ping\n",
        );
        assert_eq!(index.rows.len(), 3);
        assert_eq!(index.find("a", "sh x"), Some(&["a_show_x"][..]));
        assert_eq!(index.find("a", "show x detail"), Some(&["a_show_x"][..]));
        // `re.match` semantics: a prefix of the platform is enough.
        assert_eq!(index.find("a_variant", "show x"), Some(&["a_show_x"][..]));
        assert_eq!(index.find("a", "s x"), None);
        assert_eq!(index.find("c_os", "show two"), Some(&["b_one", "b_two"][..]));
        assert_eq!(index.find("c_os", "show twofold"), None);
        assert_eq!(index.find("anything", "ping 10.0.0.1"), Some(&["any_platform"][..]));
        assert_eq!(index.platforms(), ["a", "b_os", "c_os"]);
    }

    #[test]
    fn every_row_of_the_bundled_index_is_usable() {
        let bundle = Bundle::get();
        let index = Index::parse(bundle.index());
        assert_eq!(index.rows().count(), 970);
        for row in index.rows() {
            for matcher in [&row.platform, &row.command] {
                if let Matcher::Pattern { source, .. } = matcher {
                    assert!(matcher.regex().is_some(), "cannot compile {source:?}");
                }
            }
            for template in &row.templates {
                assert!(bundle.source(template).is_some(), "{template} is not in the bundle");
            }
        }
    }

    #[test]
    fn lists_platforms() {
        let platforms = Index::parse(Bundle::get().index()).platforms();
        for expected in [
            "arista_eos",
            "aruba_aoscx",
            "aruba_procurve",
            "cisco_asa",
            "cisco_ftd",
            "cisco_ios",
            "cisco_nxos",
            "cisco_xr",
            "fortinet",
            "hp_comware",
            "hp_procurve",
            "huawei_vrp",
            "juniper_junos",
            "linux",
            "mikrotik_routeros",
            "paloalto_panos",
            "vyatta_vyos",
        ] {
            assert!(platforms.iter().any(|platform| platform == expected), "{expected} is missing");
        }
        assert!(platforms.iter().all(|name| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')));
        assert!(platforms.is_sorted());
    }
}
