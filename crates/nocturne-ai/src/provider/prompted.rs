//! Tool calling for models that have no native tool support.
//!
//! The tools are described in the system prompt and the model is asked to
//! reply with fenced `tool` blocks; calls are then parsed out of the reply
//! text. The parser is also applied to native-mode replies, because local
//! models routinely "call" tools by printing JSON instead of using the API's
//! tool-call channel.

use serde_json::Value;

use crate::types::ToolSpec;

/// Instructions appended to the system prompt in prompted mode.
pub fn instructions(tools: &[ToolSpec]) -> String {
    let mut text = String::from(
        "\n\n# Tools\n\
         You can call tools. To call one, write a fenced block exactly like this:\n\
         ```tool\n{\"name\": \"<tool name>\", \"arguments\": {<arguments as JSON>}}\n```\n\
         Rules:\n\
         - One block per call; you may put several blocks in one reply.\n\
         - After your tool blocks, stop writing and wait: the results arrive in the next message.\n\
         - Never invent tool results, and never wrap anything else in a `tool` block.\n\
         - When you have what you need, answer normally without any tool block.\n\n\
         Available tools:\n",
    );
    for tool in tools {
        text.push_str(&format!(
            "\n## {}\n{}\nArguments (JSON Schema): {}\n",
            tool.name, tool.description, tool.input_schema
        ));
    }
    text
}

/// How an earlier tool call is shown back to a prompted-mode model.
pub fn render_call(name: &str, input: &Value) -> String {
    format!(
        "```tool\n{}\n```",
        serde_json::json!({"name": name, "arguments": input})
    )
}

/// How a tool result is shown to a prompted-mode model.
pub fn render_result(name: &str, content: &str, is_error: bool) -> String {
    let label = if is_error {
        "Tool error"
    } else {
        "Tool result"
    };
    format!("[{label} from {name}]\n{content}")
}

/// A tool call recovered from reply text.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedCall {
    pub name: String,
    pub arguments: Value,
}

/// Pull tool calls out of `text`, returning the remaining prose and the calls.
///
/// Only objects that name a tool in `tools` count, so ordinary JSON in an
/// answer (a config snippet, an API example) is left alone.
pub fn extract_calls(text: &str, tools: &[ToolSpec]) -> (String, Vec<ParsedCall>) {
    let mut calls = Vec::new();
    let mut remove: Vec<(usize, usize)> = Vec::new();

    for (start, end) in json_candidates(text) {
        let Ok(value) = serde_json::from_str::<Value>(&text[start..end]) else {
            continue;
        };
        let found = calls_in(&value, tools);
        if found.is_empty() {
            continue;
        }
        calls.extend(found);
        remove.push(widen_to_wrapper(text, start, end));
    }

    if calls.is_empty() {
        return (text.to_string(), calls);
    }
    let mut remaining = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in remove {
        if start >= cursor {
            remaining.push_str(&text[cursor..start]);
            cursor = end;
        }
    }
    remaining.push_str(&text[cursor..]);
    (remaining.trim().to_string(), calls)
}

/// Interpret one JSON value as zero or more tool calls.
fn calls_in(value: &Value, tools: &[ToolSpec]) -> Vec<ParsedCall> {
    match value {
        Value::Array(items) => items
            .iter()
            .flat_map(|item| calls_in(item, tools))
            .collect(),
        Value::Object(map) => {
            // {"function": {"name": …, "arguments": …}} and {"tool_calls": […]} wrappers.
            if let Some(inner) = map.get("function").filter(|inner| inner.is_object()) {
                return calls_in(inner, tools);
            }
            if let Some(inner) = map.get("tool_calls").filter(|inner| inner.is_array()) {
                return calls_in(inner, tools);
            }
            let name = ["name", "tool", "tool_name", "function"]
                .iter()
                .find_map(|key| map.get(*key)?.as_str());
            let Some(name) = name.filter(|name| tools.iter().any(|tool| tool.name == *name)) else {
                return Vec::new();
            };
            let arguments = ["arguments", "parameters", "args", "input"]
                .iter()
                .find_map(|key| map.get(*key))
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            // Some models JSON-encode the arguments a second time.
            let arguments = match arguments {
                Value::String(encoded) => {
                    serde_json::from_str(&encoded).unwrap_or(Value::String(encoded))
                }
                other => other,
            };
            vec![ParsedCall {
                name: name.to_string(),
                arguments,
            }]
        }
        _ => Vec::new(),
    }
}

/// Byte ranges of every top-level `{…}` or `[…]` in the text, found by
/// bracket matching that respects JSON strings.
fn json_candidates(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut ranges = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'{' && bytes[index] != b'[' {
            index += 1;
            continue;
        }
        match matching_close(bytes, index) {
            Some(end) => {
                ranges.push((index, end + 1));
                index = end + 1;
            }
            None => index += 1,
        }
    }
    ranges
}

fn matching_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &byte) in bytes[open..].iter().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extend a JSON range to swallow the wrapper the model put around it: a
/// code fence, `<tool_call>` tags or a `[TOOL_CALLS]` marker.
fn widen_to_wrapper(text: &str, start: usize, end: usize) -> (usize, usize) {
    let before = text[..start].trim_end();
    let after = text[end..].trim_start();
    let after_offset = text.len() - after.len();

    if let Some(fence_at) = before.rfind("```") {
        // The fence opener is "```" plus an optional language tag on the same line.
        let tag = &before[fence_at + 3..];
        if !tag.contains('\n')
            && tag
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && after.starts_with("```")
        {
            return (fence_at, after_offset + 3);
        }
    }
    for (open, close) in [
        ("<tool_call>", "</tool_call>"),
        ("<function_call>", "</function_call>"),
        ("<|tool_call|>", "<|/tool_call|>"),
    ] {
        if before.ends_with(open) {
            let new_end = if after.starts_with(close) {
                after_offset + close.len()
            } else {
                end
            };
            return (before.len() - open.len(), new_end);
        }
    }
    if before.ends_with("[TOOL_CALLS]") {
        return (before.len() - "[TOOL_CALLS]".len(), end);
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tools() -> Vec<ToolSpec> {
        ["run_command", "list_devices"]
            .iter()
            .map(|name| ToolSpec {
                name: (*name).to_string(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
                parallel_safe: false,
            })
            .collect()
    }

    #[test]
    fn extracts_fenced_tool_blocks_and_keeps_prose() {
        let text = "I'll check the VLANs.\n```tool\n{\"name\": \"run_command\", \"arguments\": {\"device\": \"sw1\", \"command\": \"show vlan brief\"}}\n```\nThen the trunks.\n```tool\n{\"name\": \"list_devices\", \"arguments\": {}}\n```";
        let (rest, calls) = extract_calls(text, &tools());
        assert_eq!(rest, "I'll check the VLANs.\n\nThen the trunks.");
        assert_eq!(calls.len(), 2);
        assert_eq!(
            calls[0],
            ParsedCall {
                name: "run_command".into(),
                arguments: json!({"device": "sw1", "command": "show vlan brief"})
            }
        );
        assert_eq!(calls[1].name, "list_devices");
    }

    #[test]
    fn understands_common_local_model_formats() {
        let hermes = "<tool_call>\n{\"name\": \"list_devices\", \"arguments\": {\"site\": \"hq\"}}\n</tool_call>";
        let (rest, calls) = extract_calls(hermes, &tools());
        assert_eq!((rest.as_str(), calls.len()), ("", 1));

        let mistral = "[TOOL_CALLS] [{\"name\": \"run_command\", \"arguments\": {\"command\": \"show clock\"}}]";
        let (rest, calls) = extract_calls(mistral, &tools());
        assert_eq!(
            (rest.as_str(), calls[0].arguments["command"].as_str()),
            ("", Some("show clock"))
        );

        let openai_like = "```json\n{\"function\": {\"name\": \"run_command\", \"arguments\": \"{\\\"command\\\": \\\"show ip route\\\"}\"}}\n```";
        let (_, calls) = extract_calls(openai_like, &tools());
        assert_eq!(calls[0].arguments, json!({"command": "show ip route"}));

        let params = "{\"tool\": \"list_devices\", \"parameters\": {}}";
        assert_eq!(extract_calls(params, &tools()).1.len(), 1);
    }

    #[test]
    fn ignores_json_that_is_not_a_known_tool_call() {
        let text = "Here is the payload: {\"name\": \"vlan30\", \"id\": 30} and a list [1, 2, 3].\n```json\n{\"interface\": \"Gi0/1\"}\n```";
        let (rest, calls) = extract_calls(text, &tools());
        assert!(calls.is_empty());
        assert_eq!(rest, text);
    }

    #[test]
    fn survives_braces_inside_strings_and_unbalanced_text() {
        let text = "{\"name\": \"run_command\", \"arguments\": {\"command\": \"show run | include }\"}} trailing { unbalanced";
        let (rest, calls) = extract_calls(text, &tools());
        assert_eq!(calls[0].arguments["command"], "show run | include }");
        assert_eq!(rest, "trailing { unbalanced");
    }

    #[test]
    fn instructions_list_every_tool() {
        let text = instructions(&tools());
        assert!(
            text.contains("## run_command")
                && text.contains("## list_devices")
                && text.contains("```tool")
        );
    }
}
