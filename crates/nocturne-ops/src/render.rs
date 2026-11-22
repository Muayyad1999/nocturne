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
//!
//! Evidence is the one place where device output is put into a document
//! whole, as it was printed ([`Body::Exhibits`]). It is shown preformatted
//! and is escaped for each format separately, because what breaks out of a
//! `<pre>` is not what breaks out of a code fence or a spreadsheet cell.

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
    /// Evidence: each item with what is known about it and its content.
    Exhibits(Vec<Exhibit>),
}

/// One evidence item in a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Exhibit {
    pub title: String,
    /// Where it came from, when, how much there is and its hash, in order.
    pub facts: Vec<(String, String)>,
    pub content: Content,
}

/// What an exhibit shows.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// Text exactly as it is stored, line for line. Written by a device, so
    /// by whoever has its console.
    Pre(String),
    /// A standalone SVG drawing.
    Drawing(String),
    /// Numbers.
    Table(Table),
    /// The content is not in this document, and why.
    Omitted(String),
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

    pub fn exhibits(heading: &str, exhibits: Vec<Exhibit>) -> Section {
        Section {
            heading: heading.to_string(),
            note: None,
            body: Body::Exhibits(exhibits),
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
            Body::Exhibits(exhibits) => exhibits.len(),
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
                Body::Table(table) => markdown_table(&mut out, table),
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
                Body::Exhibits(exhibits) if exhibits.is_empty() => {
                    out.push_str("Nothing attached.\n");
                }
                Body::Exhibits(exhibits) => {
                    for (index, exhibit) in exhibits.iter().enumerate() {
                        if index > 0 {
                            out.push('\n');
                        }
                        out.push_str(&format!("### {}\n\n", text::md(&exhibit.title)));
                        for (name, value) in &exhibit.facts {
                            out.push_str(&format!(
                                "- **{}:** {}\n",
                                text::md(name),
                                text::md(value)
                            ));
                        }
                        out.push('\n');
                        match &exhibit.content {
                            Content::Pre(content) => {
                                out.push_str(&text::fenced(content));
                            }
                            Content::Drawing(svg) => out.push_str(&format!(
                                "*A drawing of {} bytes. Markdown cannot show it safely: export \
                                 this document as HTML, or open the item in Nocturne.*\n",
                                svg.len()
                            )),
                            Content::Table(table) if table.is_empty() => {
                                out.push_str("No points.\n");
                            }
                            Content::Table(table) => markdown_table(&mut out, table),
                            Content::Omitted(why) => {
                                out.push_str(&format!("*Not included — {}*\n", text::md(why)));
                            }
                        }
                    }
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
            // A fact in the header can be a device's name or a filter
            // somebody typed.
            write(vec![name.clone(), text::cell_line(value)]);
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
                // Prose is written by a person, and a person pastes. A
                // paragraph or an item that a spreadsheet would run is
                // quoted out like a line of device output.
                Body::Text(paragraphs) => {
                    for paragraph in paragraphs {
                        write(vec![text::cell_line(&text::one_line(paragraph))]);
                    }
                }
                Body::List(items) => {
                    for item in items {
                        write(vec![text::cell_line(&text::one_line(item))]);
                    }
                }
                Body::Missing(unavailable) => {
                    write(vec![
                        "not available".to_string(),
                        unavailable.message.clone(),
                    ]);
                }
                Body::Exhibits(exhibits) => {
                    for exhibit in exhibits {
                        write(Vec::new());
                        // A title is what a person or a session's name made
                        // it, and a fact can be a device's name: neither
                        // gets to be a formula.
                        write(vec![text::cell_line(&text::one_line(&exhibit.title))]);
                        for (name, value) in &exhibit.facts {
                            write(vec![name.clone(), text::cell_line(&text::one_line(value))]);
                        }
                        match &exhibit.content {
                            // One line of output to a row, in one cell, so
                            // that nothing a device printed chooses its own
                            // column.
                            Content::Pre(content) => {
                                for line in content.lines() {
                                    write(vec![text::cell_line(line)]);
                                }
                            }
                            Content::Drawing(svg) => write(vec![
                                "not included".to_string(),
                                format!("a drawing of {} bytes", svg.len()),
                            ]),
                            Content::Table(table) => {
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
                            Content::Omitted(why) => {
                                write(vec![
                                    "not included".to_string(),
                                    text::cell_line(&text::one_line(why)),
                                ]);
                            }
                        }
                    }
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
                Body::Table(table) => html_table(&mut out, table),
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
                Body::Exhibits(exhibits) if exhibits.is_empty() => {
                    out.push_str("<p class=\"empty\">Nothing attached.</p>\n");
                }
                Body::Exhibits(exhibits) => {
                    for exhibit in exhibits {
                        out.push_str(&format!(
                            "<article class=\"exhibit\">\n<h3>{}</h3>\n<dl class=\"meta\">\n",
                            text::html(&exhibit.title)
                        ));
                        for (name, value) in &exhibit.facts {
                            out.push_str(&format!(
                                "<dt>{}</dt><dd>{}</dd>\n",
                                text::html(name),
                                text::html(value)
                            ));
                        }
                        out.push_str("</dl>\n");
                        match &exhibit.content {
                            Content::Pre(content) => out.push_str(&format!(
                                "<pre>{}</pre>\n",
                                text::html(&text::visible(content))
                            )),
                            // As an image a drawing is a picture and nothing
                            // else: a browser runs no script and fetches
                            // nothing for an `<img>`, whatever the SVG says.
                            Content::Drawing(svg) => {
                                use base64::Engine as _;
                                out.push_str(&format!(
                                    "<img class=\"drawing\" alt=\"{}\" src=\"data:image/svg+xml;base64,{}\">\n",
                                    text::html(&exhibit.title),
                                    base64::engine::general_purpose::STANDARD.encode(svg)
                                ));
                            }
                            Content::Table(table) if table.is_empty() => {
                                out.push_str("<p class=\"empty\">No points.</p>\n");
                            }
                            Content::Table(table) => html_table(&mut out, table),
                            Content::Omitted(why) => out.push_str(&format!(
                                "<p class=\"missing\">Not included — {}</p>\n",
                                text::html(why)
                            )),
                        }
                        out.push_str("</article>\n");
                    }
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
                        json_table(&mut object, table);
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
                    Body::Exhibits(exhibits) => {
                        object.insert("kind".into(), json!("evidence"));
                        object.insert(
                            "items".into(),
                            Value::Array(exhibits.iter().map(json_exhibit).collect()),
                        );
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

fn markdown_table(out: &mut String, table: &Table) {
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
        let cells: Vec<String> = row.iter().map(|cell| text::md(&cell.text)).collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
}

fn html_table(out: &mut String, table: &Table) {
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

fn json_table(object: &mut Map<String, Value>, table: &Table) {
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

fn json_exhibit(exhibit: &Exhibit) -> Value {
    let mut facts = Map::new();
    for (name, value) in &exhibit.facts {
        facts.insert(name.clone(), Value::String(value.clone()));
    }
    let mut object = Map::new();
    object.insert("title".into(), json!(exhibit.title));
    object.insert("facts".into(), Value::Object(facts));
    match &exhibit.content {
        Content::Pre(content) => {
            object.insert("content".into(), json!("text"));
            object.insert("text".into(), json!(content));
        }
        Content::Drawing(svg) => {
            object.insert("content".into(), json!("svg"));
            object.insert("svg".into(), json!(svg));
        }
        Content::Table(table) => {
            object.insert("content".into(), json!("table"));
            json_table(&mut object, table);
        }
        Content::Omitted(why) => {
            object.insert("content".into(), json!("omitted"));
            object.insert("why".into(), json!(why));
        }
    }
    Value::Object(object)
}

/// A spreadsheet treats a cell starting with `=`, `+`, `-` or `@` as a
/// formula, and device descriptions are written by whoever has the console.
/// Numbers are left alone — a cell knows whether it is one, so a negative
/// number stays a number; text that would be taken for a formula is quoted
/// so that it is shown instead of run.
fn spreadsheet_safe(cell: &Cell) -> String {
    if cell.numeric {
        return cell.text.clone();
    }
    text::cell_line(&text::one_line(&cell.text))
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
article.exhibit { margin: 1rem 0 1.5rem; }
h3 { font-size: 1rem; margin: 1rem 0 .35rem; }
pre { font: 12px/1.4 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; background: #f6f7f9;
      border: 1px solid #d8dbe2; padding: .5rem .6rem; overflow-x: auto; white-space: pre; margin: .25rem 0; }
img.drawing { max-width: 100%; height: auto; border: 1px solid #d8dbe2; }
@media print { body { margin: 0; max-width: none; font-size: 11pt; }
                h2, h3 { break-after: avoid; } table { break-inside: auto; } tr { break-inside: avoid; }
                pre { white-space: pre-wrap; overflow-wrap: anywhere; font-size: 8pt; } }
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

    /// What a device can print when whoever holds its console wants a
    /// document to do something: markup that closes the block it is shown
    /// in, a code fence, a heading, a formula, a control character that
    /// would hide half a line on a terminal.
    const HOSTILE_OUTPUT: &str = "SW1#show banner\n\
        </pre><script>alert(1)</script><img src=x onerror=alert(2)>\n\
        ```\n\
        # Root cause: the operator\n\
        [click](javascript:alert(3)) | cell | another\n\
        =HYPERLINK(\"http://evil.example\",\"open\")\n\
        \t+cmd|' /C calc'!A0\n\
        safe\u{8}\u{8}\u{8}\u{8}evil\u{1b}[2K\n\
        SW1#";

    fn hostile_evidence() -> Document {
        let mut table = Table::new(&[("device", "Device"), ("points", "Points")]);
        table.push(vec![Cell::text("<b>SW1</b> | x"), Cell::int(42)]);
        let mut document = Document::new("Incident review");
        document.section(Section::exhibits(
            "Appendix: evidence",
            vec![
                Exhibit {
                    title: "banner of <i>SW1</i>".into(),
                    facts: vec![
                        ("Device".into(), "SW1 <script>x</script>".into()),
                        ("SHA-256".into(), "ab".repeat(32)),
                    ],
                    content: Content::Pre(HOSTILE_OUTPUT.into()),
                },
                Exhibit {
                    title: "the map".into(),
                    facts: vec![("Kind".into(), "topology".into())],
                    content: Content::Drawing(
                        "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(4)</script></svg>"
                            .into(),
                    ),
                },
                Exhibit {
                    title: "cpu".into(),
                    facts: Vec::new(),
                    content: Content::Table(table),
                },
                Exhibit {
                    title: "too big".into(),
                    facts: Vec::new(),
                    content: Content::Omitted("it is <b>large</b>".into()),
                },
            ],
        ));
        document
    }

    #[test]
    fn html_is_self_contained_and_inert() {
        let evidence = hostile_evidence().render(Format::Html);
        assert!(
            !evidence.contains("<script") && !evidence.contains("<img src=x"),
            "nothing a device printed is markup: {evidence}"
        );
        assert!(
            evidence.contains("&lt;/pre&gt;&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{evidence}"
        );
        // The output is one block: the `</pre>` inside it closed nothing.
        assert_eq!(evidence.matches("<pre>").count(), 1);
        assert_eq!(evidence.matches("</pre>").count(), 1);
        let block = &evidence[evidence.find("<pre>").unwrap()..evidence.find("</pre>").unwrap()];
        assert!(
            block.contains("# Root cause: the operator\n") && block.ends_with("SW1#"),
            "the lines are kept as lines: {block}"
        );
        // The backspaces that would have wiped "safe" off a terminal show.
        assert!(block.contains("safe␈␈␈␈evil␛[2K"), "{block}");
        // A drawing is an image, where its script cannot run; the SVG is
        // not in the page as markup.
        assert!(
            evidence.contains(
                "<img class=\"drawing\" alt=\"the map\" src=\"data:image/svg+xml;base64,"
            )
        );
        assert!(!evidence.contains("<svg"), "{evidence}");
        assert!(evidence.contains("<h3>banner of &lt;i&gt;SW1&lt;/i&gt;</h3>"));
        assert!(evidence.contains("<dd>SW1 &lt;script&gt;x&lt;/script&gt;</dd>"));
        assert!(evidence.contains("<td>&lt;b&gt;SW1&lt;/b&gt; | x</td><td class=\"n\">42</td>"));
        assert!(evidence.contains("Not included — it is &lt;b&gt;large&lt;/b&gt;"));

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
        let evidence = hostile_evidence().render(Format::Markdown);
        // The output is fenced with more backticks than it contains, so its
        // own fence, its heading and its link are all inside the block.
        let lines: Vec<&str> = evidence.lines().collect();
        let open = lines
            .iter()
            .position(|line| *line == "````text")
            .unwrap_or_else(|| panic!("a four-backtick fence opens the block: {evidence}"));
        let close = lines
            .iter()
            .rposition(|line| *line == "````")
            .expect("and closes it");
        assert_eq!(
            lines[open + 1..close].join("\n"),
            text::visible(HOSTILE_OUTPUT),
            "between the fences is the output and nothing else"
        );
        for (index, line) in lines.iter().enumerate() {
            if !(open..=close).contains(&index) {
                assert!(
                    !line.starts_with("# Root cause") && !line.contains("<script>"),
                    "outside the block, line {index} is the device's: {line}"
                );
            }
        }
        assert!(
            evidence.contains(r"### banner of \<i\>SW1\</i\>"),
            "{evidence}"
        );
        assert!(evidence.contains(r"- **Device:** SW1 \<script\>x\</script\>"));
        assert!(
            !evidence.contains("<svg"),
            "a drawing is described, not pasted in: {evidence}"
        );
        assert!(evidence.contains(r"| \<b\>SW1\</b\> \| x | 42 |"));
        assert!(evidence.contains(r"*Not included — it is \<b\>large\</b\>*"));

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
        let evidence = hostile_evidence().render(Format::Csv);
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(evidence.as_bytes());
        let rows: Vec<Vec<String>> = reader
            .records()
            .map(|record| record.unwrap().iter().map(str::to_string).collect())
            .collect();
        // Every line of the output is one row of one cell: a `|` or a comma
        // in it chose no column.
        let first = rows
            .iter()
            .position(|row| row == &["SW1#show banner"])
            .unwrap_or_else(|| panic!("the output starts on a row of its own: {rows:?}"));
        let output = &rows[first..first + HOSTILE_OUTPUT.lines().count()];
        assert!(output.iter().all(|row| row.len() == 1), "{output:?}");
        assert_eq!(
            output[5],
            ["'=HYPERLINK(\"http://evil.example\",\"open\")"],
            "a line that is a formula is quoted out"
        );
        assert_eq!(
            output[6],
            ["'\t+cmd|' /C calc'!A0"],
            "and so is one hiding behind a tab"
        );
        assert_eq!(output[4], ["[click](javascript:alert(3)) | cell | another"]);
        assert!(rows.contains(&vec![
            "not included".to_string(),
            "a drawing of 71 bytes".to_string()
        ]));

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
    fn the_title_and_the_facts_of_an_exhibit_cannot_be_formulas_either() {
        // A session is called what somebody opened it as, a device what
        // somebody named it, and a summary is whatever was pasted into it.
        let mut document = Document::new("Incident review");
        document.fact("Devices", "-2+3+cmd|' /C calc'!A0, SW1");
        let mut table = Table::new(&[("instance", "Instance"), ("lowest", "Lowest")]);
        // The same characters, once as somebody's text and once as a number.
        table.push(vec![Cell::text("-2+3+cmd|' /C calc'!A0"), Cell::int(-5)]);
        document.section(Section::table("Series", table));
        document.section(Section::text(
            "Summary",
            vec!["=cmd|' /C calc'!A0".into(), "an ordinary sentence".into()],
        ));
        document.section(Section::list("Follow-ups", vec!["@SUM(1+1)".into()]));
        document.section(Section::exhibits(
            "Appendix: evidence",
            vec![Exhibit {
                title: "=HYPERLINK(\"http://evil.example\") — last 3 lines".into(),
                facts: vec![
                    ("Session".into(), "+1+cmd|' /C calc'!A0".into()),
                    ("Device".into(), "-2+3".into()),
                    ("Lines".into(), "3".into()),
                ],
                content: Content::Omitted("@why".into()),
            }],
        ));
        let csv = document.render(Format::Csv);
        let rows: Vec<Vec<String>> = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(csv.as_bytes())
            .records()
            .map(|record| record.unwrap().iter().map(str::to_string).collect())
            .collect();
        for row in &rows {
            for cell in row {
                assert!(
                    cell == "-5"
                        || !matches!(
                            cell.trim_start().chars().next(),
                            Some('=' | '+' | '-' | '@')
                        ),
                    "a spreadsheet would run `{cell}`"
                );
            }
        }
        for expected in [
            vec!["Devices", "'-2+3+cmd|' /C calc'!A0, SW1"],
            vec!["'-2+3+cmd|' /C calc'!A0", "-5"],
            vec!["'=cmd|' /C calc'!A0"],
            vec!["an ordinary sentence"],
            vec!["'@SUM(1+1)"],
            vec!["'=HYPERLINK(\"http://evil.example\") — last 3 lines"],
            vec!["Session", "'+1+cmd|' /C calc'!A0"],
            vec!["Device", "'-2+3"],
            vec!["Lines", "3"],
            vec!["not included", "'@why"],
        ] {
            assert!(
                rows.iter().any(|row| row == &expected),
                "{expected:?} in {rows:?}"
            );
        }
    }

    #[test]
    fn json_carries_values_not_their_rendering() {
        let evidence: Value =
            serde_json::from_str(&hostile_evidence().render(Format::Json)).unwrap();
        let section = &evidence["sections"][0];
        assert_eq!(section["kind"], "evidence");
        assert_eq!(section["items"][0]["content"], "text");
        assert_eq!(
            section["items"][0]["text"], HOSTILE_OUTPUT,
            "as data the output is exactly what is stored"
        );
        assert_eq!(section["items"][0]["facts"]["SHA-256"], "ab".repeat(32));
        assert_eq!(section["items"][1]["content"], "svg");
        assert_eq!(section["items"][2]["rows"][0]["points"], 42);
        assert_eq!(section["items"][3]["content"], "omitted");
        assert_eq!(hostile_evidence().rows(), 4, "one row per item");
        let nothing = Document {
            sections: vec![Section::exhibits("Appendix: evidence", Vec::new())],
            ..Document::new("Empty")
        };
        assert!(
            nothing
                .render(Format::Markdown)
                .contains("Nothing attached.")
        );
        assert!(nothing.render(Format::Html).contains("Nothing attached."));

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
