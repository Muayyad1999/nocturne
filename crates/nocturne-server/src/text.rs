//! Making text that arrived over the network safe to quote in a message or a log line.

/// `value` cut to `max` characters, with control characters replaced, so a
/// client cannot forge log lines or terminal escapes through a header or a
/// method name.
pub(crate) fn printable(value: &str, max: usize) -> String {
    let mut text: String = value
        .chars()
        .take(max)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if value.chars().nth(max).is_some() {
        text.push('…');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_characters_are_replaced_and_long_values_are_cut() {
        assert_eq!(printable("devices.list", 64), "devices.list");
        assert_eq!(printable("a\u{1b}[31mb\r\nc", 64), "a?[31mb??c");
        assert_eq!(printable("abcdef", 3), "abc…");
        assert_eq!(printable("abc", 3), "abc");
        assert_eq!(printable("", 3), "");
    }
}
