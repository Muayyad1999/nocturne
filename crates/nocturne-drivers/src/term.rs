//! Turning raw terminal output into plain text.
//!
//! Device CLIs are written for humans at a terminal: they redraw lines with
//! carriage returns, erase `--More--` with backspaces, colour output and move
//! the cursor. [`Screen`] interprets just enough of that to recover the text a
//! person would have read, incrementally, so the driver can look for a prompt
//! on the line still being drawn.

/// An incremental terminal-to-text converter.
#[derive(Default)]
pub struct Screen {
    /// Completed lines not yet taken by the caller.
    done: Vec<String>,
    line: Vec<char>,
    column: usize,
    state: State,
    /// Parameter bytes of the escape sequence being read.
    params: String,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum State {
    #[default]
    Text,
    Escape,
    Csi,
    /// Operating-system command: runs to BEL or ESC \.
    Osc,
    OscEscape,
    /// One more byte to swallow (charset selection and similar).
    Skip,
}

impl Screen {
    pub fn new() -> Screen {
        Screen::default()
    }

    /// Feed more output. Text is decoded as UTF-8, falling back to Latin-1
    /// style replacement for invalid bytes.
    pub fn feed(&mut self, data: &[u8]) {
        for ch in String::from_utf8_lossy(data).chars() {
            self.feed_char(ch);
        }
    }

    pub fn feed_str(&mut self, text: &str) {
        for ch in text.chars() {
            self.feed_char(ch);
        }
    }

    /// Lines completed since the last call.
    pub fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.done)
    }

    /// The line currently being drawn (no newline seen yet) — where a prompt sits.
    pub fn current_line(&self) -> String {
        self.line.iter().collect::<String>().trim_end().to_string()
    }

    /// Discard the line being drawn (after answering a pager, for example).
    pub fn clear_current_line(&mut self) {
        self.line.clear();
        self.column = 0;
    }

    /// Finish: completed lines plus whatever is on the current line.
    pub fn finish(mut self) -> Vec<String> {
        if !self.line.is_empty() {
            self.commit_line();
        }
        self.done
    }

    fn feed_char(&mut self, ch: char) {
        match self.state {
            State::Text => self.text(ch),
            State::Escape => self.escape(ch),
            State::Csi => self.csi(ch),
            State::Osc => match ch {
                '\u{7}' => self.state = State::Text,
                '\u{1b}' => self.state = State::OscEscape,
                _ => {}
            },
            State::OscEscape => self.state = if ch == '\\' { State::Text } else { State::Osc },
            State::Skip => self.state = State::Text,
        }
    }

    fn text(&mut self, ch: char) {
        match ch {
            '\u{1b}' => self.state = State::Escape,
            '\n' => self.commit_line(),
            '\r' => self.column = 0,
            '\u{8}' => self.column = self.column.saturating_sub(1),
            '\t' => {
                let next_stop = (self.column / 8 + 1) * 8;
                while self.column < next_stop {
                    self.put(' ');
                }
            }
            // NUL padding, bells and other C0 controls carry no text.
            ch if ch.is_control() => {}
            ch => self.put(ch),
        }
    }

    fn put(&mut self, ch: char) {
        if self.column < self.line.len() {
            self.line[self.column] = ch;
        } else {
            while self.line.len() < self.column {
                self.line.push(' ');
            }
            self.line.push(ch);
        }
        self.column += 1;
    }

    fn commit_line(&mut self) {
        let text: String = self.line.iter().collect();
        self.done.push(text.trim_end().to_string());
        self.line.clear();
        self.column = 0;
    }

    fn escape(&mut self, ch: char) {
        self.state = match ch {
            '[' => {
                self.params.clear();
                State::Csi
            }
            ']' | 'P' | '^' | '_' => State::Osc,
            '(' | ')' | '*' | '+' | '#' | '%' => State::Skip,
            _ => State::Text,
        };
    }

    fn csi(&mut self, ch: char) {
        match ch {
            // Parameter and intermediate bytes.
            '0'..='9' | ';' | '?' | ':' | '<' | '=' | '>' | ' ' | '!' | '"' | '$' | '\'' => {
                if self.params.len() < 32 {
                    self.params.push(ch);
                }
            }
            final_byte => {
                let count = self
                    .params
                    .split(';')
                    .next()
                    .and_then(|first| first.parse::<usize>().ok());
                match final_byte {
                    // Erase in line: 0 = cursor to end, 1 = start to cursor, 2 = whole line.
                    'K' => match count.unwrap_or(0) {
                        0 => self.line.truncate(self.column),
                        1 => {
                            let upto = self.column.min(self.line.len());
                            self.line[..upto].fill(' ');
                        }
                        _ => {
                            self.line.clear();
                            self.column = 0;
                        }
                    },
                    'G' | '`' => self.column = count.unwrap_or(1).saturating_sub(1),
                    'C' => self.column += count.unwrap_or(1).max(1),
                    'D' => self.column = self.column.saturating_sub(count.unwrap_or(1).max(1)),
                    // Erase display: whatever was on the current line is gone too.
                    'J' => {
                        self.line.clear();
                        self.column = 0;
                    }
                    // Cursor addressing: treat a jump to another row as a fresh line.
                    'H' | 'f' if !self.line.is_empty() => {
                        self.commit_line();
                    }
                    _ => {}
                }
                self.state = State::Text;
            }
        }
    }
}

/// Convert a complete chunk of terminal output to plain text.
pub fn clean(data: &[u8]) -> String {
    let mut screen = Screen::new();
    screen.feed(data);
    screen.finish().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_passes_through_with_crlf_normalised() {
        assert_eq!(
            clean(b"line one\r\nline two\r\n\r\nlast"),
            "line one\nline two\n\nlast"
        );
    }

    #[test]
    fn pager_prompt_erased_with_backspaces_disappears() {
        // IOS erases " --More-- " with backspace, space, backspace runs.
        let mut output = b"Vlan1   up\r\n --More-- ".to_vec();
        output.extend(std::iter::repeat_n(b'\x08', 10));
        output.extend(std::iter::repeat_n(b' ', 10));
        output.extend(std::iter::repeat_n(b'\x08', 10));
        output.extend(b"Vlan10  up\r\n");
        assert_eq!(clean(&output), "Vlan1   up\nVlan10  up");
    }

    #[test]
    fn carriage_return_redraw_overwrites_the_line() {
        assert_eq!(clean(b"--More--\r        \rnext line\r\n"), "next line");
        assert_eq!(clean(b"progress 10%\rprogress 100%\r\n"), "progress 100%");
    }

    #[test]
    fn strips_colours_and_cursor_controls() {
        let coloured =
            b"\x1b[1;32mup\x1b[0m   \x1b[31mdown\x1b[m\r\n\x1b]0;window title\x07ok\x1b(B\r\n";
        assert_eq!(clean(coloured), "up   down\nok");
        assert_eq!(clean(b"abcdef\x1b[3D\x1b[Kxy\r\n"), "abcxy");
        assert_eq!(clean(b"junk\x1b[2Kclean\r\n"), "clean");
    }

    #[test]
    fn incremental_feeding_exposes_the_prompt_line() {
        let mut screen = Screen::new();
        screen.feed(b"show clock\r\n*12:00:00.000 UTC\r\nSW1");
        assert_eq!(screen.take_lines(), vec!["show clock", "*12:00:00.000 UTC"]);
        assert_eq!(screen.current_line(), "SW1");
        screen.feed(b"#");
        assert_eq!(screen.current_line(), "SW1#");
        assert!(screen.take_lines().is_empty());
        // An escape sequence split across two reads is still recognised.
        screen.feed(b"\r\n\x1b[");
        screen.feed(b"1mbold\x1b[0m\r\n");
        assert_eq!(screen.take_lines(), vec!["SW1#", "bold"]);
    }

    #[test]
    fn tabs_and_control_bytes() {
        assert_eq!(clean(b"a\tb\x00\x07c\r\n"), "a       bc");
    }
}
