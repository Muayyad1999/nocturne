//! Drawings and data to take elsewhere.
//!
//! Every name in a graph may have come from an LLDP system name or an
//! interface description, which is to say from whatever a device on the
//! network chose to send. Nothing here interpolates such text without
//! escaping it first, and none of the formats uses a construct (CDATA, HTML
//! in labels, unquoted identifiers) that could be closed from inside a name.

mod dot;
mod drawio;
mod mermaid;
mod svg;

use nocturne_app::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::model::Graph;

/// What `topology.export` can write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Svg,
    Drawio,
    Mermaid,
    Dot,
    Json,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Drawio => "drawio",
            Format::Mermaid => "mermaid",
            Format::Dot => "dot",
            Format::Json => "json",
        }
    }

    /// The file name a download should use.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Svg => "svg",
            Format::Drawio => "drawio",
            Format::Mermaid => "mmd",
            Format::Dot => "dot",
            Format::Json => "json",
        }
    }
}

pub fn render(graph: &Graph, format: Format) -> AppResult<String> {
    Ok(match format {
        Format::Svg => svg::render(graph),
        Format::Drawio => drawio::render(graph),
        Format::Mermaid => mermaid::render(graph),
        Format::Dot => dot::render(graph),
        Format::Json => serde_json::to_string_pretty(graph)
            .map_err(|err| AppError::internal(format!("the graph cannot be written: {err}")))?,
    })
}

/// Escape text for XML character data: no markup can survive this.
pub fn xml_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len() + 8);
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\r' | '\n' | '\t' => escaped.push(' '),
            // XML 1.0 has no way to carry other control characters at all.
            character if (character as u32) < 0x20 => escaped.push(' '),
            character => escaped.push(character),
        }
    }
    escaped
}

/// Escape text for an XML attribute value. The same rules: attributes are
/// always written inside double quotes.
pub fn xml_attribute(text: &str) -> String {
    xml_text(text)
}

/// Collapse whitespace and cut over-long device names so a drawing stays
/// readable whatever a neighbour called itself.
pub fn clip(text: &str, limit: usize) -> String {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        return collapsed;
    }
    let kept: String = collapsed.chars().take(limit.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// A timestamp as `2026-11-13 09:12 UTC`, without pulling in a calendar
/// library (Howard Hinnant's civil-from-days).
pub fn timestamp(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let time = seconds.rem_euclid(86_400);
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        time / 3600,
        (time % 3600) / 60
    )
}

/// A short, safe identifier for a node, for formats with their own id syntax.
pub fn node_ids(graph: &Graph) -> std::collections::BTreeMap<String, String> {
    graph
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.id.clone(), format!("n{index}")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOSTILE: &str = "<script>alert(1)</script> \"quoted\" 'single' & ]]> \n\u{7}end";

    #[test]
    fn hostile_text_cannot_escape_xml() {
        let escaped = xml_text(HOSTILE);
        assert!(!escaped.contains('<'), "{escaped}");
        assert!(!escaped.contains('>'), "{escaped}");
        assert!(!escaped.contains('"'), "{escaped}");
        assert!(!escaped.contains('\''), "{escaped}");
        assert!(!escaped.contains('\n'), "{escaped}");
        assert!(!escaped.contains('\u{7}'), "{escaped}");
        assert!(escaped.contains("&lt;script&gt;"));
        // `]]>` only matters inside CDATA, which nothing here emits; the `>`
        // is escaped all the same.
        assert!(!escaped.contains("]]>"));
    }

    #[test]
    fn long_names_are_clipped_not_wrapped() {
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("a\nb   c", 10), "a b c");
        assert_eq!(clip("0123456789abc", 6), "01234…");
    }
}
