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
/// comment, which may itself run over several lines. Rows get `index`,
/// `flags` and `comment` in addition to the device's own properties (with
/// `-` turned into `_`).
///
/// RouterOS 7 lists items that cannot be edited (dynamic routes, for one)
/// without an index: those rows have an empty `index`. Its flag letters may
/// be lower-case (`DAc`, `As`) and include `+`. Log lines echoed into the
/// session (`17:20:06 echo: system,error …`) and the `Flags:` / `Columns:`
/// legend are not part of any item.
pub fn parse_kv(output: &str) -> Vec<Row> {
    let mut items: Vec<Item> = Vec::new();
    // A comment met before the item it belongs to (table-style listings).
    let mut pending_comment = String::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_legend(trimmed) || is_log_echo(trimmed) {
            continue;
        }
        // While an item has shown only its comment, everything up to its
        // first property still belongs to that comment.
        let in_comment = items
            .last()
            .is_some_and(|item| item.body.is_empty() && !item.comment.is_empty());
        if !in_comment && let Some(mut item) = Item::start(trimmed) {
            if !pending_comment.is_empty() && item.comment.is_empty() {
                item.comment = std::mem::take(&mut pending_comment);
            }
            items.push(item);
            continue;
        }
        match items.last_mut() {
            Some(item) if in_comment && !starts_with_pair(trimmed) => {
                item.comment.push(' ');
                item.comment
                    .push_str(trimmed.trim_start_matches(";;;").trim());
            }
            // A comment on a line of its own announces the next item.
            _ if trimmed.starts_with(";;;") => {
                pending_comment = trimmed.trim_start_matches(";;;").trim().to_string();
            }
            Some(item) => item.push(trimmed),
            None => {}
        }
    }

    items
        .into_iter()
        .filter_map(|item| {
            let pairs = scan_pairs(&item.body);
            // No property at all: a stray number or legend fragment, not an item.
            if pairs.is_empty() {
                return None;
            }
            let mut row = Row::new();
            row.insert("index".into(), Value::String(item.index));
            row.insert("flags".into(), Value::String(item.flags));
            for (key, value) in pairs {
                row.insert(field_name(&key), Value::String(value));
            }
            if !item.comment.is_empty() && !row.contains_key("comment") {
                row.insert("comment".into(), Value::String(item.comment));
            }
            Some(row)
        })
        .collect()
}

/// One listed item while its lines are being gathered.
struct Item {
    index: String,
    flags: String,
    /// The `key=value` text, joined over wrapped lines.
    body: String,
    comment: String,
}

impl Item {
    /// Recognise the first line of an item: `7 DRS ;;; comment`,
    /// `0 R name="ether1" …`, or without an index `DAc dst-address=…`.
    fn start(line: &str) -> Option<Item> {
        let (first, rest) = split_word(line);
        let (index, rest) = if first.chars().all(|c| c.is_ascii_digit()) {
            (first, rest)
        } else {
            ("", line)
        };
        let (flags, rest) = split_flags(rest);
        let begins_item = rest.starts_with(";;;") || starts_with_pair(rest);
        let recognised = if index.is_empty() {
            // Without an index, only flag letters tell an item from a wrapped line.
            !flags.is_empty() && begins_item
        } else {
            begins_item || rest.is_empty()
        };
        if !recognised {
            return None;
        }
        let mut item = Item {
            index: index.to_string(),
            flags,
            body: String::new(),
            comment: String::new(),
        };
        item.push(rest);
        Some(item)
    }

    /// Add a line (or the rest of the first line): properties, and a `;;;` remark if it has one.
    fn push(&mut self, text: &str) {
        let (text, remark) = match text.split_once(";;;") {
            Some((before, after)) => (before, Some(after.trim())),
            None => (text, None),
        };
        if let Some(remark) = remark {
            if !self.comment.is_empty() {
                self.comment.push(' ');
            }
            self.comment.push_str(remark);
        }
        let text = text.trim();
        if !text.is_empty() {
            if !self.body.is_empty() {
                self.body.push(' ');
            }
            self.body.push_str(text);
        }
    }
}

fn split_word(text: &str) -> (&str, &str) {
    let text = text.trim_start();
    match text.split_once(char::is_whitespace) {
        Some((word, rest)) => (word, rest.trim_start()),
        None => (text, ""),
    }
}

/// Split leading flag letters (`R`, `XS`, `D S`, `DAc`, `As+`) from the rest of the line.
fn split_flags(text: &str) -> (String, &str) {
    let mut flags = String::new();
    let mut rest = text.trim_start();
    loop {
        let (word, after) = split_word(rest);
        // Flag letters come in short groups and always include a capital;
        // that keeps ordinary lower-case words from being taken for flags.
        let is_flags = !word.is_empty()
            && word.len() <= 6
            && word
                .chars()
                .all(|c| c.is_ascii_alphabetic() || matches!(c, '*' | '+'))
            && word
                .chars()
                .any(|c| c.is_ascii_uppercase() || matches!(c, '*' | '+'));
        if !is_flags {
            return (flags, rest);
        }
        flags.push_str(word);
        rest = after;
    }
}

/// Whether the text begins with a `key=` token.
fn starts_with_pair(text: &str) -> bool {
    let (word, _) = split_word(text);
    word.split_once('=').is_some_and(|(key, _)| is_key(key))
}

fn is_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// `Flags: X - disabled, …`, `Columns: …`, a table header, or a wrapped
/// piece of the flag legend (`C - connect, S - static`).
fn is_legend(line: &str) -> bool {
    if line.starts_with("Flags:") || line.starts_with("Columns:") || line.starts_with('#') {
        return true;
    }
    let mut chars = line.chars();
    matches!(
        (chars.next(), chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(' '), Some('-'), Some(' ')) if letter.is_ascii_alphabetic() || matches!(letter, '*' | '+')
    )
}

/// A log line echoed into the session: `17:20:06 echo: system,error,critical …`.
fn is_log_echo(line: &str) -> bool {
    let Some((before, _)) = line.split_once(" echo: ") else {
        return false;
    };
    // The time of day, optionally after a date (`jan/02 17:20:06`).
    before.split_whitespace().count() <= 2
        && before.split_whitespace().next_back().is_some_and(|time| {
            time.len() == 8
                && time.chars().enumerate().all(|(at, c)| {
                    if at == 2 || at == 5 {
                        c == ':'
                    } else {
                        c.is_ascii_digit()
                    }
                })
        })
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
        if !is_key(key) {
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
        if let Some(space) = before.rfind(char::is_whitespace)
            && is_key(&before[space + 1..])
        {
            return Some(from + space);
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

/// The bundled ntc-templates template(s) a step's parser stands for.
///
/// `name` is `auto` (resolve `command` through the ntc index for `platform`,
/// the way ntc-templates does) or the name of one template. The error says
/// what to change in the profile.
pub fn ntc_templates(
    platform: Option<&str>,
    command: &str,
    name: &str,
) -> Result<Vec<&'static str>, String> {
    use nocturne_textfsm::ntc;

    let name = name.trim();
    let names: Vec<&'static str> = if name == "auto" {
        let Some(platform) = platform.filter(|platform| !platform.is_empty()) else {
            return Err(
                "`auto` needs the profile's `ntc_platform` (the platform's name in the ntc-templates index, e.g. `cisco_ios`)"
                    .into(),
            );
        };
        match ntc::find_all(platform, command) {
            Some(names) => names.to_vec(),
            None if ntc::platforms().iter().any(|known| known == platform) => {
                return Err(format!(
                    "ntc-templates has no template for `{command}` on `{platform}`; name one with `{{ntc: <template>}}` or use another parser"
                ));
            }
            None => {
                return Err(format!(
                    "ntc-templates does not know the platform `{platform}` (set by `ntc_platform`); the nearest is `{}`",
                    nearest(platform, ntc::platforms().iter().map(String::as_str))
                ));
            }
        }
    } else {
        let name = name.strip_suffix(".textfsm").unwrap_or(name);
        match ntc::template_names().find(|known| *known == name) {
            Some(known) => vec![known],
            None => {
                return Err(format!(
                    "there is no bundled ntc-templates template called `{name}`; the nearest is `{}`",
                    nearest(name, ntc::template_names())
                ));
            }
        }
    };
    for name in &names {
        match ntc::template(name) {
            Ok(Some(_)) => {}
            Ok(None) => return Err(format!("the template `{name}` is missing from the bundle")),
            Err(err) => return Err(format!("the template `{name}` does not compile: {err}")),
        }
    }
    Ok(names)
}

/// The candidate closest to `wanted` by edit distance, for "did you mean".
fn nearest<'a>(wanted: &str, candidates: impl Iterator<Item = &'a str>) -> &'a str {
    candidates
        .min_by_key(|candidate| edit_distance(wanted, candidate))
        .unwrap_or_default()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous + usize::from(ca != *cb);
            previous = row[j + 1];
            row[j + 1] = substitution.min(previous + 1).min(row[j] + 1);
        }
    }
    row[b.len()]
}

/// Run bundled ntc-templates over the output (see [`ntc_templates`] for `name`).
pub fn parse_ntc(
    platform: Option<&str>,
    command: &str,
    name: &str,
    output: &str,
) -> Result<Vec<Row>, String> {
    use nocturne_textfsm::ntc;

    let names = ntc_templates(platform, command, name)?;
    let records = if name.trim() == "auto" {
        // Through the index, so that commands covered by several templates are merged as upstream does.
        ntc::parse(platform.unwrap_or_default(), command, output)
            .map_err(|err| format!("{}: {err}", names.join(" + ")))?
            .unwrap_or_default()
    } else {
        let mut records = Vec::new();
        for name in &names {
            let template = ntc::template(name)
                .map_err(|err| format!("{name}: {err}"))?
                .ok_or_else(|| format!("the template `{name}` is missing from the bundle"))?;
            records.extend(
                template
                    .run(output)
                    .map_err(|err| format!("{name}: {err}"))?,
            );
        }
        records
    };
    Ok(records.into_iter().map(textfsm_row).collect())
}

/// Run a TextFSM template written in the profile.
pub fn parse_textfsm(template: &str, output: &str) -> Result<Vec<Row>, String> {
    let template = nocturne_textfsm::Template::parse(template).map_err(|err| err.to_string())?;
    let records = template.run(output).map_err(|err| err.to_string())?;
    Ok(records.into_iter().map(textfsm_row).collect())
}

fn textfsm_row(record: nocturne_textfsm::Record) -> Row {
    record
        .into_iter()
        .map(|(name, value)| {
            let value = match value {
                nocturne_textfsm::Value::Text(text) => Value::String(text),
                nocturne_textfsm::Value::List(items) => {
                    Value::Array(items.into_iter().map(Value::String).collect())
                }
            };
            (name, value)
        })
        .collect()
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
    fn routeros_v7_items_without_an_index_and_lower_case_flags() {
        // Taken from the ntc-templates capture of `ip route print terse`: RouterOS 6 and 7
        // lines side by side, with a log line echoed into the session.
        let output = " 0 A S  dst-address=0.0.0.0/0 gateway=23.90.232.243 gateway-status=23.90.232.243 reachable via  pppoe-out1 distance=10 scope=30 target-scope=10 \n 1  Db  dst-address=0.0.0.0/0 gateway=23.90.232.243 distance=20 scope=40 target-scope=10 bgp-origin=incomplete received-from=23.90.232.243-peer \n17:20:06 echo: system,error,critical login failure for user admin from 65.160.140.13 via ssh\n 2  DS  dst-address=0.0.0.0/0 gateway=pppoe-out1 gateway-status=pppoe-out1 reachable distance=100 scope=30 target-scope=10 \nDAc dst-address=192.168.1.0/24 routing-table=main gateway=bridge-lan immediate-gw=bridge-lan distance=0 scope=10 suppress-hw-offload=no local-address=192.168.1.254%bridge-lan\nDAv dst-address=0.0.0.0/0 routing-table=main pref-src= gateway=pppoe-out1 immediate-gw=pppoe-out1 distance=100 scope=30 target-scope=10 vrf-interface=pppoe-out1 suppress-hw-offload=no\n";
        let rows = parse_kv(output);
        assert_eq!(rows.len(), 5, "{rows:#?}");
        assert_eq!(rows[0]["flags"], "AS");
        assert_eq!(
            rows[0]["gateway_status"],
            "23.90.232.243 reachable via  pppoe-out1"
        );
        assert_eq!(rows[1]["flags"], "Db");
        assert_eq!(
            rows[1]["received_from"], "23.90.232.243-peer",
            "the echoed log line is not part of the item"
        );
        assert_eq!(
            (&rows[2]["index"], &rows[2]["flags"]),
            (&json!("2"), &json!("DS"))
        );
        assert_eq!(
            (&rows[3]["index"], &rows[3]["flags"]),
            (&json!(""), &json!("DAc"))
        );
        assert_eq!(rows[3]["dst_address"], "192.168.1.0/24");
        assert_eq!(rows[3]["local_address"], "192.168.1.254%bridge-lan");
        assert_eq!(rows[4]["flags"], "DAv");
        assert_eq!(rows[4]["pref_src"], "");
        assert_eq!(rows[4]["gateway"], "pppoe-out1");

        // The same in `print detail` layout, with the legend wrapped by a narrow terminal.
        let detail = "Flags: D - dynamic; X - disabled, I - inactive, A - active; \nc - connect, s - static, r - rip, b - bgp, o - ospf, d - dhcp; + - ecmp \n   DAd   dst-address=0.0.0.0/0 routing-table=main gateway=192.168.88.1 \n         immediate-gw=192.168.88.1%ether1 distance=1 scope=30 target-scope=10 \n\n   DAc+  dst-address=192.168.88.0/24 routing-table=main gateway=bridge \n         immediate-gw=bridge local-address=192.168.88.2%bridge distance=0 \n\n 0  As   dst-address=10.0.0.0/8 routing-table=main gateway=192.168.88.254 \n         immediate-gw=192.168.88.254%bridge distance=1 scope=30 \n";
        let rows = parse_kv(detail);
        assert_eq!(rows.len(), 3, "{rows:#?}");
        assert_eq!(rows[0]["flags"], "DAd");
        assert_eq!(rows[0]["immediate_gw"], "192.168.88.1%ether1");
        assert_eq!(rows[1]["flags"], "DAc+");
        assert_eq!(rows[1]["distance"], "0");
        assert_eq!(
            (&rows[2]["index"], &rows[2]["flags"]),
            (&json!("0"), &json!("As"))
        );
    }

    #[test]
    fn routeros_comments_may_span_lines() {
        // From the ntc-templates capture of `interface print detail`.
        let output = " 6 D S name=\"lte1\" type=\"lte\" mtu=1500\n\n 7 DRS ;;; very very long\nmultiline description \n       name=\"pptp-out1\" type=\"pptp-out\" mtu=1450 actual-mtu=1450\n \n 8  RS ;;; Free Wi-Fi HTTPS\n       name=\"pptp-to-AH1100-HS\" type=\"pptp-out\" mtu=1596 \n";
        let rows = parse_kv(output);
        assert_eq!(rows.len(), 3, "{rows:#?}");
        assert_eq!(rows[0]["flags"], "DS");
        assert_eq!(rows[1]["comment"], "very very long multiline description");
        assert_eq!(rows[1]["name"], "pptp-out1");
        assert_eq!(rows[1]["actual_mtu"], "1450");
        assert_eq!(rows[2]["comment"], "Free Wi-Fi HTTPS");
        assert_eq!(rows[2]["name"], "pptp-to-AH1100-HS");

        // A comment line that happens to start with a number or capitals is still a comment.
        let tricky = " 0 R ;;; uplink\n3 switches behind this port\nTO BE REPLACED\n     name=\"ether1\" mtu=1500\n 1 X name=\"ether2\" mtu=1500\n";
        let rows = parse_kv(tricky);
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert_eq!(
            rows[0]["comment"],
            "uplink 3 switches behind this port TO BE REPLACED"
        );
        assert_eq!(rows[1]["name"], "ether2");
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
    fn ntc_templates_resolve_or_explain() {
        assert_eq!(
            ntc_templates(Some("cisco_ios"), "show version", "auto").unwrap(),
            vec!["cisco_ios_show_version"]
        );
        assert_eq!(
            ntc_templates(None, "anything", "cisco_ios_show_vlan.textfsm").unwrap(),
            vec!["cisco_ios_show_vlan"]
        );
        let no_platform = ntc_templates(None, "show version", "auto").unwrap_err();
        assert!(no_platform.contains("ntc_platform"), "{no_platform}");
        let unknown_platform =
            ntc_templates(Some("cicso_ios"), "show version", "auto").unwrap_err();
        assert!(
            unknown_platform.contains("nearest is `cisco_ios`"),
            "{unknown_platform}"
        );
        let no_template = ntc_templates(Some("cisco_ios"), "show nonsense", "auto").unwrap_err();
        assert!(
            no_template.contains("no template for `show nonsense` on `cisco_ios`"),
            "{no_template}"
        );
        let typo = ntc_templates(None, "", "cisco_ios_show_interface").unwrap_err();
        assert!(
            typo.contains("nearest is `cisco_ios_show_interfaces`"),
            "{typo}"
        );

        let rows = parse_ntc(
            Some("cisco_ios"),
            "sh ip int br",
            "auto",
            "Interface              IP-Address      OK? Method Status                Protocol\nVlan1                  10.0.0.2        YES NVRAM  up                    up\n",
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["INTERFACE"], "Vlan1");
        assert_eq!(rows[0]["IP_ADDRESS"], "10.0.0.2");

        let inline = "Value NAME (\\S+)\nValue List TAGS (\\w+)\n\nStart\n  ^tag ${TAGS}\n  ^${NAME} done -> Record\n";
        let rows = parse_textfsm(inline, "tag a\ntag b\nfirst done\n").unwrap();
        assert_eq!(
            Value::Object(rows[0].clone()),
            json!({"NAME": "first", "TAGS": ["a", "b"]})
        );
        assert!(
            parse_textfsm("Value X (\\S+)\n", "")
                .unwrap_err()
                .contains("line")
        );
        assert_eq!(edit_distance("kitten", "sitting"), 3);
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
