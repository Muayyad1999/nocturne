//! Incremental parsers for the two streaming framings model APIs use:
//! Server-Sent Events and newline-delimited JSON.

/// One dispatched SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Splits a byte stream into lines, tolerating chunks that end mid-line or
/// mid-UTF-8-sequence.
#[derive(Default)]
pub(crate) struct LineBuffer {
    pending: Vec<u8>,
}

impl LineBuffer {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(offset) = self.pending[start..].iter().position(|&b| b == b'\n') {
            let end = start + offset;
            let mut line = &self.pending[start..end];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            lines.push(String::from_utf8_lossy(line).into_owned());
            start = end + 1;
        }
        self.pending.drain(..start);
        lines
    }

    /// Whatever is left when the stream ends without a trailing newline.
    pub fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let rest = std::mem::take(&mut self.pending);
        Some(
            String::from_utf8_lossy(&rest)
                .trim_end_matches('\r')
                .to_string(),
        )
    }
}

#[derive(Default)]
pub(crate) struct SseParser {
    lines: LineBuffer,
    event: String,
    data: String,
    has_data: bool,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for line in self.lines.push(chunk) {
            if let Some(event) = self.feed_line(&line) {
                events.push(event);
            }
        }
        events
    }

    pub fn finish(&mut self) -> Option<SseEvent> {
        if let Some(line) = self.lines.finish()
            && let Some(event) = self.feed_line(&line)
        {
            return Some(event);
        }
        self.dispatch()
    }

    fn feed_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None; // comment / keep-alive
        }
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_string(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        if !self.has_data {
            self.event.clear();
            return None;
        }
        self.has_data = false;
        Some(SseEvent {
            event: std::mem::take(&mut self.event),
            data: std::mem::take(&mut self.data),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_events_split_across_chunks() {
        let mut parser = SseParser::default();
        let mut events = parser.push(b"event: message_start\ndata: {\"a\":");
        assert!(events.is_empty());
        events.extend(parser.push(b"1}\n\nevent: ping\ndata: {}\n\n"));
        assert_eq!(
            events,
            vec![
                SseEvent {
                    event: "message_start".into(),
                    data: "{\"a\":1}".into()
                },
                SseEvent {
                    event: "ping".into(),
                    data: "{}".into()
                },
            ]
        );
    }

    #[test]
    fn joins_multi_line_data_and_skips_comments() {
        let mut parser = SseParser::default();
        let events = parser.push(b": keep-alive\r\ndata: one\r\ndata: two\r\n\r\n");
        assert_eq!(
            events,
            vec![SseEvent {
                event: String::new(),
                data: "one\ntwo".into()
            }]
        );
    }

    #[test]
    fn handles_utf8_split_mid_character() {
        let text = "data: h\u{e9}llo\n\n".as_bytes();
        let (first, second) = text.split_at(8); // splits inside the two-byte é
        let mut parser = SseParser::default();
        let mut events = parser.push(first);
        events.extend(parser.push(second));
        assert_eq!(events[0].data, "h\u{e9}llo");
    }

    #[test]
    fn flushes_final_event_without_blank_line() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"data: [DONE]").is_empty());
        assert_eq!(parser.finish().unwrap().data, "[DONE]");
    }

    #[test]
    fn line_buffer_keeps_partial_lines() {
        let mut lines = LineBuffer::default();
        assert_eq!(
            lines.push(b"{\"a\":1}\n{\"b\""),
            vec!["{\"a\":1}".to_string()]
        );
        assert_eq!(lines.push(b":2}\n"), vec!["{\"b\":2}".to_string()]);
        assert_eq!(lines.finish(), None);
    }
}
