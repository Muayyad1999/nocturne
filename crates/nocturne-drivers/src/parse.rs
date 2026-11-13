//! Turning command output into rows.
//!
//! A profile picks one parser per command. TextFSM templates (the bundled
//! ntc-templates library, or a template written inline in the profile) cover
//! most table-style output; the parsers here cover the rest: RouterOS's
//! `key=value` listings, `key: value` property sheets, a plain regex with
//! named groups, and JSON for platforms that can emit it.
//!
//! Every parser yields rows of string-keyed JSON values, which is what the
//! mapping expressions in a profile are evaluated against.

use regex::Regex;
use serde_json::{Map, Value};

/// One parsed record.
pub type Row = Map<String, Value>;

/// Make a device's field name usable as an identifier in expressions:
/// `mac-address` → `mac_address`, `CPU Load` → `cpu_load`.
pub fn field_name(raw: &str) -> String {
    let mut name = String::with_capacity(raw.len());
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else if !name.ends_with('_') {
            name.push('_');
        }
    }
    let name = name.trim_matches('_').to_string();
    if name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_digit())
    {
        format!("_{name}")
    } else {
        name
    }
}

/// RouterOS `print detail` / `print terse` listings.
///
/// Each item starts with its index and optional flag letters, followed by
/// `key=value` pairs that may wrap over several lines; a `;;;` introduces a
/// comment. Rows get `index`, `flags` and `comment` in addition to the
/// device's own properties (with `-` turned into `_`).
pub fn parse_kv(output: &str) -> Vec<Row> {
    let mut items: Vec<(String, String, String)> = Vec::new(); // (index, body, comment)
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let starts_item = trimmed
            .split_once(char::is_whitespace)
            .map(|(first, _)| first)
            .or(Some(trimmed))
            .is_some_and(|first| !first.is_empty() && first.chars().all(|c| c.is_ascii_digit()));
        if starts_item {
            let (index, rest) = trimmed
                .split_once(char::is_whitespace)
                .unwrap_or((trimmed, ""));
            items.push((index.to_string(), String::new(), String::new()));
            push_body(&mut items, rest);
        } else if !items.is_empty() {
            push_body(&mut items, trimmed);
        }
        // Lines before the first item are the "Flags:" / "Columns:" legend.
    }

    items
        .into_iter()
        .map(|(index, body, comment)| {
            let mut row = Row::new();
            row.insert("index".into(), Value::String(index));
            let (flags, pairs) = split_flags(&body);
            row.insert("flags".into(), Value::String(flags));
            for (key, value) in scan_pairs(pairs) {
                row.insert(field_name(&key), Value::String(value));
            }
            if !comment.is_empty() && !row.contains_key("comment") {
                row.insert("comment".into(), Value::String(comment));
            }
            row
        })
        .collect()
}

fn push_body(items: &mut [(String, String, String)], text: &str) {
    let Some((_, body, comment)) = items.last_mut() else {
        return;
    };
    let (text, remark) = match text.split_once(";;;") {
        Some((before, after)) => (before, Some(after.trim())),
        None => (text, None),
    };
    if let Some(remark) = remark {
        if !comment.is_empty() {
            comment.push(' ');
        }
        comment.push_str(remark);
    }
    let text = text.trim();
    if !text.is_empty() {
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(text);
    }
}

/// Split leading flag letters (`R`, `XS`, `D S`) from the `key=value` part.
fn split_flags(body: &str) -> (String, &str) {
    let mut flags = String::new();
    let mut rest = body;
    loop {
        let trimmed = rest.trim_start();
        let Some((word, after)) = trimmed
            .split_once(char::is_whitespace)
            .or(Some((trimmed, "")))
        else {
            break;
        };
        let is_flags = !word.is_empty()
            && word.len() <= 6
            && word.chars().all(|c| c.is_ascii_uppercase() || c == '*');
        if !is_flags {
            rest = trimmed;
            break;
        }
        flags.push_str(word);
        rest = after;
        if rest.is_empty() {
            break;
        }
    }
    (flags, rest)
}

/// Scan `key=value` pairs. Values may be quoted (with `\"` escapes) or bare;
/// a bare value runs until the next ` key=` so that terse listings with
/// spaces in a value survive.
fn scan_pairs(text: &str) -> Vec<(String, String)> {
    let bytes = text.as_bytes();
    let mut pairs = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        // Find the next "key=".
        let Some(equals) = text[position..].find('=').map(|offset| position + offset) else {
            break;
        };
        let key_start = text[position..equals]
            .rfind(char::is_whitespace)
            .map_or(position, |offset| position + offset + 1);
        let key = &text[key_start..equals];
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
        {
            position = equals + 1;
            continue;
        }
        let value_start = equals + 1;
        if bytes.get(value_start) == Some(&b'"') {
            let mut value = String::new();
            let mut index = value_start + 1;
            let mut escaped = false;
            let mut end = bytes.len();
            for (offset, ch) in text[index..].char_indices() {
                if escaped {
                    value.push(ch);
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    end = index + offset + 1;
                    break;
                } else {
                    value.push(ch);
                }
            }
            index = end;
            pairs.push((key.to_string(), value));
            position = index;
        } else {
            let end = next_key_start(text, value_start).unwrap_or(text.len());
            pairs.push((key.to_string(), text[value_start..end].trim().to_string()));
            position = end;
        }
    }
    pairs
}

/// Byte offset where the next ` key=` begins at or after `from`.
fn next_key_start(text: &str, from: usize) -> Option<usize> {
    let mut search = from;
    while let Some(equals) = text[search..].find('=').map(|offset| search + offset) {
        let before = &text[from..equals];
        if let Some(space) = before.rfind(char::is_whitespace) {
            let key = &before[space + 1..];
            if !key.is_empty()
                && key
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
            {
                return Some(from + space);
            }
        }
        search = equals + 1;
    }
    None
}

/// A property sheet of `key: value` lines (RouterOS `/system resource print`,
/// many `show … detail` outputs) → one row.
pub fn parse_colon(output: &str) -> Vec<Row> {
    let mut row = Row::new();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let name = field_name(key);
        if name.is_empty() || key.trim().contains("  ") {
            continue;
        }
        row.entry(name)
            .or_insert_with(|| Value::String(value.trim().to_string()));
    }
    if row.is_empty() {
        Vec::new()
    } else {
        vec![row]
    }
}

/// Every match of `pattern` becomes a row of its named groups.
pub fn parse_regex(pattern: &Regex, output: &str) -> Vec<Row> {
    let names: Vec<&str> = pattern.capture_names().flatten().collect();
    pattern
        .captures_iter(output)
        .map(|caps| {
            names
                .iter()
                .map(|name| {
                    let value = caps
                        .name(name)
                        .map(|found| found.as_str().trim())
                        .unwrap_or_default();
                    ((*name).to_string(), Value::String(value.to_string()))
                })
                .collect()
        })
        .collect()
}

/// JSON output. `path` selects the rows: dot-separated keys, `*` for "every
/// element / every value"; empty means the document itself.
pub fn parse_json(output: &str, path: &str) -> Result<Vec<Row>, String> {
    // Devices like to print a banner or the echoed command before the JSON.
    let start = output
        .find(['{', '['])
        .ok_or("the output contains no JSON")?;
    let document: Value = serde_json::from_str(output[start..].trim())
        .map_err(|err| format!("invalid JSON: {err}"))?;
    let mut selected = vec![document];
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        let mut next = Vec::new();
        for value in selected {
            match (segment, value) {
                ("*", Value::Array(items)) => next.extend(items),
                ("*", Value::Object(map)) => {
                    next.extend(map.into_iter().map(|(key, mut value)| {
                        // Keep the map key, which is usually the interesting name.
                        if let Value::Object(inner) = &mut value {
                            inner.entry("_key").or_insert(Value::String(key));
                        }
                        value
                    }))
                }
                (key, Value::Object(mut map)) => next.extend(map.remove(key)),
                (key, Value::Array(items)) => {
                    next.extend(items.into_iter().filter_map(|item| match item {
                        Value::Object(mut map) => map.remove(key),
                        _ => None,
                    }));
                }
                _ => {}
            }
        }
        selected = next;
    }
    let mut rows = Vec::new();
    for value in selected {
        match value {
            Value::Object(map) => rows.push(map),
            Value::Array(items) => rows.extend(items.into_iter().filter_map(|item| match item {
                Value::Object(map) => Some(map),
                _ => None,
            })),
            _ => {}
        }
    }
    Ok(rows)
}

/// The whole output as a single row with one field, `output`.
pub fn parse_raw(output: &str) -> Vec<Row> {
    let mut row = Row::new();
    row.insert("output".into(), Value::String(output.to_string()));
    vec![row]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn routeros_detail_listing_with_wrapped_lines_flags_and_comments() {
        let output = r#"Flags: D - dynamic, X - disabled, R - running, S - slave
 0  R  ;;; uplink to ISP
       name="ether1" default-name="ether1" mtu=1500 l2mtu=1598
       mac-address=4C:5E:0C:AA:BB:01 arp=enabled

 1   S name="ether2" default-name="ether2" mtu=1500
       mac-address=4C:5E:0C:AA:BB:02

 2 XS  name="ether 5 \"spare\"" mtu=1500
"#;
        let rows = parse_kv(output);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["index"], "0");
        assert_eq!(rows[0]["flags"], "R");
        assert_eq!(rows[0]["comment"], "uplink to ISP");
        assert_eq!(rows[0]["name"], "ether1");
        assert_eq!(rows[0]["default_name"], "ether1");
        assert_eq!(rows[0]["mac_address"], "4C:5E:0C:AA:BB:01");
        assert_eq!(rows[0]["arp"], "enabled");
        assert_eq!(rows[1]["flags"], "S");
        assert_eq!(rows[2]["flags"], "XS");
        assert_eq!(rows[2]["name"], "ether 5 \"spare\"");
    }

    #[test]
    fn routeros_terse_listing_keeps_spaces_in_bare_values() {
        let output = " 0 D address=10.0.20.1/24 network=10.0.20.0 interface=vlan20 actual-interface=vlan20\n 1   address=192.168.88.1/24 comment=default LAN address network=192.168.88.0 interface=bridge\n";
        let rows = parse_kv(output);
        assert_eq!(rows[0]["flags"], "D");
        assert_eq!(rows[0]["address"], "10.0.20.1/24");
        assert_eq!(rows[0]["actual_interface"], "vlan20");
        assert_eq!(rows[1]["flags"], "");
        assert_eq!(rows[1]["comment"], "default LAN address");
        assert_eq!(rows[1]["network"], "192.168.88.0");
        assert_eq!(rows[1]["interface"], "bridge");
    }

    #[test]
    fn property_sheets_become_one_row() {
        let output = "                   uptime: 2w3d4h5m6s\n                  version: 7.15.3 (stable)\n               build-time: 2024-07-24 10:39:01\n              free-memory: 1010.0MiB\n                 cpu-load: 5%\n             architecture-name: arm\n";
        let rows = parse_colon(output);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["uptime"], "2w3d4h5m6s");
        assert_eq!(rows[0]["version"], "7.15.3 (stable)");
        assert_eq!(rows[0]["build_time"], "2024-07-24 10:39:01");
        assert_eq!(rows[0]["cpu_load"], "5%");
        assert!(parse_colon("no properties here").is_empty());
    }

    #[test]
    fn regex_rows_use_named_groups() {
        let pattern =
            Regex::new(r"(?m)^(?P<VLAN>\d+)\s+(?P<NAME>\S+)\s+(?P<STATUS>active|suspended)")
                .unwrap();
        let rows = parse_regex(
            &pattern,
            "1    default   active\n30   Voice     active\nxx\n",
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(
            Value::Object(rows[1].clone()),
            json!({"VLAN": "30", "NAME": "Voice", "STATUS": "active"})
        );
    }

    #[test]
    fn json_paths_select_rows() {
        let output = "show interface | json\n{\"interfaces\": {\"Ethernet1\": {\"mtu\": 1500}, \"Ethernet2\": {\"mtu\": 9214}}, \"list\": [{\"a\": 1}, {\"a\": 2}]}";
        let by_name = parse_json(output, "interfaces.*").unwrap();
        assert_eq!(by_name.len(), 2);
        assert_eq!(by_name[0]["_key"], "Ethernet1");
        assert_eq!(by_name[1]["mtu"], 9214);
        assert_eq!(parse_json(output, "list").unwrap().len(), 2);
        assert_eq!(parse_json(output, "").unwrap().len(), 1);
        assert!(parse_json("no json", "").is_err());
        assert!(parse_json("{broken", "").is_err());
    }

    #[test]
    fn field_names_are_identifier_safe() {
        assert_eq!(field_name("mac-address"), "mac_address");
        assert_eq!(field_name("  CPU Load (%) "), "cpu_load");
        assert_eq!(field_name("2ghz-channel"), "_2ghz_channel");
    }

    #[test]
    fn junk_does_not_panic() {
        for junk in [
            "",
            "=",
            "= = =",
            "a=\"unterminated",
            "0",
            "0 ",
            "  ;;; only comment",
            "\u{0}\u{1f}=x",
            "1 R name=\"a\\",
        ] {
            let _ = parse_kv(junk);
            let _ = parse_colon(junk);
        }
    }
}
