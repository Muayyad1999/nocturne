//! One document model rendered four ways.
//!
//! A report, a handover and a post-incident review are all the same thing: a
//! title, a few header facts, and then sections that are either a table, some
//! prose, or an honest statement that a source was not available. Building
//! that once means the four output formats are written once, and a section
//! that could not be filled cannot be forgotten by one of them.
//!
//! Everything that comes from the network — device names, interface
//! descriptions, alert titles — passes through [`crate::text`] on the way
//! out, so a device called `<script>` is inert in the HTML and cannot break
//! out of a Markdown table cell.

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::sources::{Reason, Unavailable};
use crate::text;
use crate::timefmt::{self, Zone};

/// The four shapes a report, handover or review can be asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Markdown,
    Csv,
    Html,
    Json,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Markdown => "markdown",
            Format::Csv => "csv",
            Format::Html => "html",
            Format::Json => "json",
        }
    }

    /// The extension a saved file would get.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Markdown => "md",
            Format::Csv => "csv",
            Format::Html => "html",
            Format::Json => "json",
        }
    }
}

/// One value in a table: how it reads, and what it is in JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    text: String,
    value: Value,
    numeric: bool,
}

impl Cell {
    /// Text from the network or from a person: escaped per format.
    pub fn text(text: impl Into<String>) -> Cell {
        let text = text.into();
        Cell {
            value: Value::String(text.clone()),
            text,
            numeric: false,
        }
    }

    pub fn int(value: i64) -> Cell {
        Cell {
            text: value.to_string(),
            value: json!(value),
            numeric: true,
        }
    }

    pub fn count(value: usize) -> Cell {
        Cell::int(value as i64)
    }

    /// A share of 1 as a percentage with one decimal.
    pub fn percent(share: f64) -> Cell {
        Cell {
            text: format!("{:.1}%", share * 100.0),
            value: json!((share * 1000.0).round() / 10.0),
            numeric: true,
        }
    }

    pub fn time(zone: &Zone, at: i64) -> Cell {
        Cell {
            text: zone.datetime(at),
            value: json!(at),
            numeric: false,
        }
    }

    pub fn duration(ms: i64) -> Cell {
        Cell {
            text: timefmt::duration(ms),
            value: json!(ms),
            numeric: false,
        }
    }

    /// Nothing is known. Never `0`, never an empty string: in JSON it is
    /// `null` and in a table it is an em dash.
    pub fn unknown() -> Cell {
        Cell {
            text: "—".into(),
            value: Value::Null,
            numeric: false,
        }
    }

    pub fn maybe(cell: Option<Cell>) -> Cell {
        cell.unwrap_or_else(Cell::unknown)
    }

    pub fn as_text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// The JSON field name.
    pub key: String,
    /// The heading a reader sees.
    pub title: String,
}

impl Column {
    pub fn new(key: &str, title: &str) -> Column {
        Column {
            key: key.to_string(),
            title: title.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    /// `columns` as `("key", "Heading")` pairs.
    pub fn new(columns: &[(&str, &str)]) -> Table {
        Table {
            columns: columns
                .iter()
                .map(|(key, title)| Column::new(key, title))
                .collect(),
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: Vec<Cell>) {
        debug_assert_eq!(
            row.len(),
            self.columns.len(),
            "a table row must have one cell per column"
        );
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// A part of a document.
#[derive(Clone, Debug, PartialEq)]
pub enum Body {
    /// Rows of data.
    Table(Table),
    /// Prose, one entry per paragraph.
    Text(Vec<String>),
    /// A list of plain statements.
    List(Vec<String>),
    /// The section exists but the source behind it does not.
    Missing(Unavailable),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub heading: String,
    /// A sentence under the heading: what was counted, how it was measured,
    /// why a row is missing.
    pub note: Option<String>,
    pub body: Body,
}

impl Section {
    pub fn table(heading: &str, table: Table) -> Section {
        Section {
            heading: heading.to_string(),
            note: None,
            body: Body::Table(table),
        }
    }

    pub fn text(heading: &str, paragraphs: Vec<String>) -> Section {
        Section {
            heading: heading.to_string(),
            note: None,
            body: Body::Text(paragraphs),
        }
    }

    pub fn list(heading: &str, items: Vec<String>) -> Section {
        Section {
            heading: heading.to_string(),
            note: None,
            body: Body::List(items),
        }
    }

    pub fn missing(heading: &str, unavailable: Unavailable) -> Section {
        Section {
            heading: heading.to_string(),
            note: None,
            body: Body::Missing(unavailable),
        }
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Section {
        self.note = Some(note.into());
        self
    }

    pub fn rows(&self) -> usize {
        match &self.body {
            Body::Table(table) => table.rows.len(),
            Body::List(items) => items.len(),
            Body::Text(_) | Body::Missing(_) => 0,
        }
    }
}

/// A document to be rendered: what it is, when it was made, and its sections.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub title: String,
    /// Header facts, in order: the generation time, the period, the filter.
    pub meta: Vec<(String, String)>,
    pub sections: Vec<Section>,
}

impl Document {
    pub fn new(title: impl Into<String>) -> Document {
        Document {
            title: title.into(),
            meta: Vec::new(),
            sections: Vec::new(),
        }
    }

    pub fn fact(&mut self, name: &str, value: impl Into<String>) -> &mut Document {
        self.meta.push((name.to_string(), value.into()));
        self
    }

    pub fn section(&mut self, section: Section) -> &mut Document {
        self.sections.push(section);
        self
    }

    /// Every data row of the document, which is what `rows` reports.
    pub fn rows(&self) -> usize {
        self.sections.iter().map(Section::rows).sum()
    }

    /// The sections that had no source, for the result's `unavailable` list.
    pub fn unavailable(&self) -> Vec<Unavailable> {
        self.sections
            .iter()
            .filter_map(|section| match &section.body {
                Body::Missing(unavailable) => Some(unavailable.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Markdown => self.markdown(),
            Format::Csv => self.csv(),
            Format::Html => self.html(),
            Format::Json => self.json(),
        }
    }

    // --------------------------------------------------------------- markdown

    fn markdown(&self) -> String {
        let mut out = format!("# {}\n", text::md(&self.title));
        if !self.meta.is_empty() {
            out.push('\n');
            for (name, value) in &self.meta {
                out.push_str(&format!("- **{}:** {}\n", text::md(name), text::md(value)));
            }
        }
        for section in &self.sections {
            out.push_str(&format!("\n## {}\n", text::md(&section.heading)));
            if let Some(note) = &section.note {
                out.push_str(&format!("\n{}\n", text::md(note)));
            }
            out.push('\n');
            match &section.body {
                Body::Table(table) if table.is_empty() => {
                    out.push_str("Nothing in this period.\n");
                }
                Body::Table(table) => {
                    let titles: Vec<String> = table
                        .columns
                        .iter()
                        .map(|column| text::md(&column.title))
                        .collect();
                    out.push_str(&format!("| {} |\n", titles.join(" | ")));
                    out.push_str(&format!(
                        "|{}|\n",
                        table
                            .columns
                            .iter()
                            .map(|_| "---")
                            .collect::<Vec<_>>()
                            .join("|")
                    ));
                    for row in &table.rows {
                        let cells: Vec<String> =
                            row.iter().map(|cell| text::md(&cell.text)).collect();
                        out.push_str(&format!("| {} |\n", cells.join(" | ")));
                    }
                }
                Body::Text(paragraphs) if paragraphs.is_empty() => {
                    out.push_str("Nothing written.\n");
                }
                Body::Text(paragraphs) => {
                    for paragraph in paragraphs {
                        out.push_str(&format!("{}\n\n", text::md(paragraph)));
                    }
                    out.truncate(out.trim_end().len());
                    out.push('\n');
                }
                Body::List(items) if items.is_empty() => {
                    out.push_str("Nothing in this period.\n");
                }
                Body::List(items) => {
                    for item in items {
                        out.push_str(&format!("- {}\n", text::md(item)));
                    }
                }
                Body::Missing(unavailable) => {
                    out.push_str(&format!(
                        "*Not available — {}*\n",
                        text::md(&unavailable.message)
                    ));
                }
            }
        }
        out
    }

    // -------------------------------------------------------------------- csv

    /// Sections one after another, each with its heading on a line of its
    /// own, so that several tables survive a paste into a spreadsheet.
    fn csv(&self) -> String {
        let mut writer = csv::WriterBuilder::new()
            .flexible(true)
            .from_writer(Vec::new());
        let mut write = |record: Vec<String>| {
            // Writing into a `Vec` cannot fail; a failure would be a bug.
            let _ = writer.write_record(&record);
        };
        write(vec![self.title.clone()]);
        for (name, value) in &self.meta {
            write(vec![name.clone(), value.clone()]);
        }
        for section in &self.sections {
            write(Vec::new());
            write(vec![section.heading.clone()]);
            if let Some(note) = &section.note {
                write(vec![note.clone()]);
            }
            match &section.body {
                Body::Table(table) => {
                    write(
                        table
                            .columns
                            .iter()
                            .map(|column| column.title.clone())
                            .collect(),
                    );
                    for row in &table.rows {
                        write(row.iter().map(spreadsheet_safe).collect());
                    }
                }
                Body::Text(paragraphs) => {
                    for paragraph in paragraphs {
                        write(vec![text::one_line(paragraph)]);
                    }
                }
                Body::List(items) => {
                    for item in items {
                        write(vec![text::one_line(item)]);
                    }
                }
                Body::Missing(unavailable) => {
                    write(vec![
                        "not available".to_string(),
                        unavailable.message.clone(),
                    ]);
                }
            }
        }
        let bytes = writer.into_inner().unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    // ------------------------------------------------------------------- html

    fn html(&self) -> String {
        let mut out = String::with_capacity(4096);
        out.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
        out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        out.push_str(&format!("<title>{}</title>\n", text::html(&self.title)));
        out.push_str(STYLE);
        out.push_str("</head>\n<body>\n");
        out.push_str(&format!("<h1>{}</h1>\n", text::html(&self.title)));
        if !self.meta.is_empty() {
            out.push_str("<dl class=\"meta\">\n");
            for (name, value) in &self.meta {
                out.push_str(&format!(
                    "<dt>{}</dt><dd>{}</dd>\n",
                    text::html(name),
                    text::html(value)
                ));
            }
            out.push_str("</dl>\n");
        }
        for section in &self.sections {
            out.push_str(&format!(
                "<section>\n<h2>{}</h2>\n",
                text::html(&section.heading)
            ));
            if let Some(note) = &section.note {
                out.push_str(&format!("<p class=\"note\">{}</p>\n", text::html(note)));
            }
            match &section.body {
                Body::Table(table) if table.is_empty() => {
                    out.push_str("<p class=\"empty\">Nothing in this period.</p>\n");
                }
                Body::Table(table) => {
                    out.push_str("<table>\n<thead>\n<tr>");
                    for column in &table.columns {
                        out.push_str(&format!("<th>{}</th>", text::html(&column.title)));
                    }
                    out.push_str("</tr>\n</thead>\n<tbody>\n");
                    for row in &table.rows {
                        out.push_str("<tr>");
                        for cell in row {
                            let class = if cell.numeric { " class=\"n\"" } else { "" };
                            out.push_str(&format!("<td{class}>{}</td>", text::html(&cell.text)));
                        }
                        out.push_str("</tr>\n");
                    }
                    out.push_str("</tbody>\n</table>\n");
                }
                Body::Text(paragraphs) if paragraphs.is_empty() => {
                    out.push_str("<p class=\"empty\">Nothing written.</p>\n");
                }
                Body::Text(paragraphs) => {
                    for paragraph in paragraphs {
                        out.push_str(&format!("<p>{}</p>\n", text::html(paragraph)));
                    }
                }
                Body::List(items) if items.is_empty() => {
                    out.push_str("<p class=\"empty\">Nothing in this period.</p>\n");
                }
                Body::List(items) => {
                    out.push_str("<ul>\n");
                    for item in items {
                        out.push_str(&format!("<li>{}</li>\n", text::html(item)));
                    }
                    out.push_str("</ul>\n");
                }
                Body::Missing(unavailable) => {
                    out.push_str(&format!(
                        "<p class=\"missing\">Not available — {}</p>\n",
                        text::html(&unavailable.message)
                    ));
                }
            }
            out.push_str("</section>\n");
        }
        out.push_str("</body>\n</html>\n");
        out
    }

    // ------------------------------------------------------------------- json

    fn json(&self) -> String {
        serde_json::to_string_pretty(&self.as_json()).unwrap_or_else(|_| "{}".into())
    }

    /// The document as data, which is also what the `json` format renders.
    pub fn as_json(&self) -> Value {
        let mut meta = Map::new();
        for (name, value) in &self.meta {
            meta.insert(name.clone(), Value::String(value.clone()));
        }
        let sections: Vec<Value> = self
            .sections
            .iter()
            .map(|section| {
                let mut object = Map::new();
                object.insert("heading".into(), json!(section.heading));
                if let Some(note) = &section.note {
                    object.insert("note".into(), json!(note));
                }
                match &section.body {
                    Body::Table(table) => {
                        object.insert("kind".into(), json!("table"));
                        object.insert(
                            "columns".into(),
                            Value::Array(
                                table
                                    .columns
                                    .iter()
                                    .map(|column| json!({"key": column.key, "title": column.title}))
                                    .collect(),
                            ),
                        );
                        object.insert(
                            "rows".into(),
                            Value::Array(
                                table
                                    .rows
                                    .iter()
                                    .map(|row| {
                                        let mut fields = Map::new();
                                        for (column, cell) in table.columns.iter().zip(row) {
                                            fields.insert(column.key.clone(), cell.value.clone());
                                        }
                                        Value::Object(fields)
                                    })
                                    .collect(),
                            ),
                        );
                    }
                    Body::Text(paragraphs) => {
                        object.insert("kind".into(), json!("text"));
                        object.insert("text".into(), json!(paragraphs));
                    }
                    Body::List(items) => {
                        object.insert("kind".into(), json!("list"));
                        object.insert("items".into(), json!(items));
                    }
                    Body::Missing(unavailable) => {
                        object.insert("kind".into(), json!("unavailable"));
                        object.insert("reason".into(), json!(unavailable.reason));
                        object.insert("message".into(), json!(unavailable.message));
                    }
                }
                Value::Object(object)
            })
            .collect();
        json!({
            "title": self.title,
            "meta": Value::Object(meta),
            "sections": sections,
        })
    }
}

/// A spreadsheet treats a cell starting with `=`, `+` or `@` as a formula, and
/// device descriptions are written by whoever has the console. Numbers are
/// left alone; text that would be taken for a formula is quoted so that it is
/// shown instead of run.
fn spreadsheet_safe(cell: &Cell) -> String {
    if cell.numeric {
        return cell.text.clone();
    }
    let text = text::one_line(&cell.text);
    match text.chars().next() {
        Some('=' | '+' | '@') => format!("'{text}"),
        _ => text,
    }
}

const STYLE: &str = r#"<style>
:root { color-scheme: light; }
body { font: 14px/1.5 -apple-system, "Segoe UI", Roboto, system-ui, sans-serif;
       margin: 2rem auto; max-width: 60rem; padding: 0 1rem; color: #16181d; background: #fff; }
h1 { font-size: 1.6rem; margin: 0 0 .5rem; }
h2 { font-size: 1.1rem; margin: 2rem 0 .5rem; border-bottom: 1px solid #d8dbe2; padding-bottom: .25rem; }
dl.meta { display: grid; grid-template-columns: max-content 1fr; gap: .15rem 1rem; margin: 0 0 1rem;
          font-size: .85rem; color: #4a4f5a; }
dl.meta dt { font-weight: 600; }
dl.meta dd { margin: 0; }
table { border-collapse: collapse; width: 100%; font-size: .85rem; }
th, td { border: 1px solid #d8dbe2; padding: .3rem .5rem; text-align: left; vertical-align: top; }
th { background: #f2f4f7; font-weight: 600; }
td.n { text-align: right; font-variant-numeric: tabular-nums; }
tbody tr:nth-child(even) { background: #fafbfc; }
p.note { color: #4a4f5a; font-size: .85rem; margin: .25rem 0 .75rem; }
p.empty { color: #6b7280; font-style: italic; }
p.missing { color: #8a5a00; background: #fff7e6; border-left: 3px solid #d9a200; padding: .4rem .6rem; }
ul { margin: .25rem 0; padding-left: 1.25rem; }
@media print { body { margin: 0; max-width: none; font-size: 11pt; }
                h2 { break-after: avoid; } table { break-inside: auto; } tr { break-inside: avoid; } }
</style>
"#;

/// The header fact every document carries.
pub fn generated_fact(zone: &Zone, at: i64) -> String {
    format!("{} ({})", zone.datetime(at), zone.label())
}

/// `2026-11-13 08:00 → 2026-11-13 16:00 (8 h)`.
pub fn period_fact(zone: &Zone, since: i64, until: i64) -> String {
    format!(
        "{} → {} ({})",
        zone.datetime(since),
        zone.datetime(until),
        timefmt::duration(until - since)
    )
}

/// A filter as a header fact, so that a report cannot be mistaken for the
/// whole estate. `None` when nothing was filtered.
pub fn filter_fact(filter: &Value) -> Option<String> {
    let fields = filter.as_object()?;
    let described: Vec<String> = fields
        .iter()
        .filter(|(_, value)| !value.is_null() && value != &&Value::String(String::new()))
        .map(|(key, value)| match value {
            Value::String(text) => format!("{key} = {text}"),
            other => format!("{key} = {other}"),
        })
        .collect();
    (!described.is_empty()).then(|| described.join(", "))
}

/// A section for a source that was not there, with the reason spelled out.
pub fn missing(heading: &str, section: &str, reason: Reason, message: &str) -> Section {
    Section::missing(
        heading,
        Unavailable {
            section: section.to_string(),
            reason,
            message: message.to_string(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn hostile() -> Document {
        let mut table = Table::new(&[("device", "Device"), ("count", "Alerts")]);
        table.push(vec![Cell::text("<script>alert(1)</script>"), Cell::int(3)]);
        table.push(vec![
            Cell::text("pipe | and \"quote\", comma"),
            Cell::unknown(),
        ]);
        table.push(vec![Cell::text("=HYPERLINK(\"http://x\")"), Cell::int(0)]);
        let mut document = Document::new("Alerts & <b>more</b>");
        document.fact("Generated", "2026-11-13 14:05 (UTC)");
        document.section(Section::table("Top devices", table).with_note("Counted by rule."));
        document.section(missing(
            "Changes",
            "changes",
            Reason::NotInstalled,
            "`changes.list` is not part of this build",
        ));
        document
    }

    #[test]
    fn html_is_self_contained_and_inert() {
        let html = hostile().render(Format::Html);
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<style>"), "the CSS is inline");
        assert!(!html.contains("<script"), "nothing can run: {html}");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("<title>Alerts &amp; &lt;b&gt;more&lt;/b&gt;</title>"));
        assert!(html.contains("2026-11-13 14:05 (UTC)"), "the header time");
        assert!(html.contains("Not available — "));
        assert!(html.contains("@media print"));
        // Numbers are right-aligned, text is not.
        assert!(html.contains("<td class=\"n\">3</td>"));
    }

    #[test]
    fn markdown_tables_cannot_be_broken_by_a_cell() {
        let markdown = hostile().render(Format::Markdown);
        assert!(markdown.starts_with("# Alerts \\& \\<b\\>more\\</b\\>\n"));
        assert!(markdown.contains("| Device | Alerts |\n|---|---|\n"));
        assert!(
            markdown.contains(r"| \<script\>alert(1)\</script\> | 3 |"),
            "{markdown}"
        );
        assert!(
            markdown.contains(r#"| pipe \| and "quote", comma | — |"#),
            "a pipe in a name does not add a column: {markdown}"
        );
        assert!(markdown.contains("*Not available — "));
    }

    #[test]
    fn csv_quotes_separators_and_defuses_formulas() {
        let csv = hostile().render(Format::Csv);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "Alerts & <b>more</b>");
        assert!(
            csv.contains("<script>alert(1)</script>,3"),
            "no separator, no quoting needed: {csv}"
        );
        assert!(
            csv.contains("\"pipe | and \"\"quote\"\", comma\","),
            "a quote is doubled and the field is quoted: {csv}"
        );
        assert!(
            csv.contains("\"'=HYPERLINK(\"\"http://x\"\")\",0"),
            "a cell that looks like a formula is quoted out: {csv}"
        );
        assert!(csv.contains("not available,"));
        // An unknown value is an em dash, never a zero.
        assert!(csv.contains(",—") || csv.contains("—"));
    }

    #[test]
    fn json_carries_values_not_their_rendering() {
        let value: Value = serde_json::from_str(&hostile().render(Format::Json)).unwrap();
        assert_eq!(value["title"], "Alerts & <b>more</b>");
        assert_eq!(value["meta"]["Generated"], "2026-11-13 14:05 (UTC)");
        let section = &value["sections"][0];
        assert_eq!(section["kind"], "table");
        assert_eq!(section["note"], "Counted by rule.");
        assert_eq!(section["rows"][0]["device"], "<script>alert(1)</script>");
        assert_eq!(section["rows"][0]["count"], 3);
        assert_eq!(
            section["rows"][1]["count"],
            Value::Null,
            "unknown is null, not 0"
        );
        assert_eq!(value["sections"][1]["kind"], "unavailable");
        assert_eq!(value["sections"][1]["reason"], "not_installed");
    }

    #[test]
    fn counts_rows_and_collects_what_was_missing() {
        let document = hostile();
        assert_eq!(document.rows(), 3);
        let unavailable = document.unavailable();
        assert_eq!(unavailable.len(), 1);
        assert_eq!(unavailable[0].section, "changes");
        assert_eq!(unavailable[0].reason, Reason::NotInstalled);
    }

    #[test]
    fn an_empty_table_says_so_in_every_format() {
        let mut document = Document::new("Empty");
        document.section(Section::table("Rows", Table::new(&[("a", "A")])));
        assert!(
            document
                .render(Format::Markdown)
                .contains("Nothing in this period.")
        );
        assert!(
            document
                .render(Format::Html)
                .contains("Nothing in this period.")
        );
        assert_eq!(document.rows(), 0);
        let value: Value = serde_json::from_str(&document.render(Format::Json)).unwrap();
        assert_eq!(value["sections"][0]["rows"], json!([]));
    }

    #[test]
    fn percentages_and_durations_read_and_serialise() {
        assert_eq!(Cell::percent(0.9994).as_text(), "99.9%");
        assert_eq!(Cell::percent(0.9994).value, json!(99.9));
        assert_eq!(Cell::duration(3_661_000).as_text(), "1 h 1 min");
        assert_eq!(Cell::duration(3_661_000).value, json!(3_661_000));
        assert_eq!(Cell::unknown().value, Value::Null);
        assert_eq!(Cell::maybe(None).as_text(), "—");
    }

    #[test]
    fn a_filter_becomes_a_header_fact() {
        assert_eq!(
            filter_fact(&json!({"site": "HQ", "role": ""})),
            Some("site = HQ".to_string())
        );
        assert_eq!(filter_fact(&json!({})), None);
        assert_eq!(filter_fact(&Value::Null), None);
        assert_eq!(
            filter_fact(&json!({"lab": false})),
            Some("lab = false".to_string())
        );
    }
}
