//! Whether the program at the far end has asked for pastes to be bracketed.
//!
//! A shell or an editor that wants to tell pasted text from typed text says
//! so with DECSET 2004 (`ESC [ ? 2004 h`) and takes it back with `l`. The
//! terminal in the interface follows that for what a person pastes; this
//! follows it for what the backend types on a person's behalf (a snippet), so
//! the two agree about the same session. Network operating systems never ask.

/// Reads a session's output for the one mode it cares about.
///
/// The sequence can arrive split between two reads, so the position inside
/// it is kept from one [`feed`](PasteMode::feed) to the next.
#[derive(Default)]
pub(crate) struct PasteMode {
    on: bool,
    state: State,
}

#[derive(Default)]
enum State {
    #[default]
    Text,
    /// After `ESC`.
    Escape,
    /// Inside `ESC [ … `.
    Csi(Csi),
}

#[derive(Default)]
struct Csi {
    /// Nothing has followed the `[` yet: a `?` here marks a private mode.
    opened: bool,
    private: bool,
    /// The parameter being read; `None` once it is not a plain number.
    param: Option<u32>,
    /// One of the parameters read so far was 2004.
    names_paste: bool,
}

const ESC: u8 = 0x1b;
const BRACKETED_PASTE: u32 = 2004;

impl PasteMode {
    pub fn is_on(&self) -> bool {
        self.on
    }

    pub fn feed(&mut self, data: &[u8]) {
        let mut rest = data;
        while !rest.is_empty() {
            if matches!(self.state, State::Text) {
                // Nearly all output is text: skip to the next escape at once.
                let Some(at) = rest.iter().position(|byte| *byte == ESC) else {
                    return;
                };
                rest = &rest[at + 1..];
                self.state = State::Escape;
                continue;
            }
            let byte = rest[0];
            rest = &rest[1..];
            self.state = match std::mem::take(&mut self.state) {
                State::Text => State::Text,
                State::Escape => match byte {
                    b'[' => State::Csi(Csi {
                        opened: true,
                        param: Some(0),
                        ..Csi::default()
                    }),
                    ESC => State::Escape,
                    // RIS, a full reset: every mode is back to its default.
                    b'c' => {
                        self.on = false;
                        State::Text
                    }
                    _ => State::Text,
                },
                State::Csi(mut csi) => match byte {
                    b'?' if csi.opened => {
                        csi.opened = false;
                        csi.private = true;
                        State::Csi(csi)
                    }
                    b'0'..=b'9' => {
                        csi.opened = false;
                        csi.param = csi.param.and_then(|value| {
                            value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))
                        });
                        State::Csi(csi)
                    }
                    b';' => {
                        csi.opened = false;
                        csi.names_paste |= csi.param == Some(BRACKETED_PASTE);
                        csi.param = Some(0);
                        State::Csi(csi)
                    }
                    ESC => State::Escape,
                    // CAN and SUB abandon a sequence.
                    0x18 | 0x1a => State::Text,
                    // Other control characters act and leave the sequence open.
                    0x00..=0x1f => State::Csi(csi),
                    // The final byte: only `h` and `l` of a private sequence
                    // set and reset modes.
                    0x40..=0x7e => {
                        if csi.private && (csi.names_paste || csi.param == Some(BRACKETED_PASTE)) {
                            match byte {
                                b'h' => self.on = true,
                                b'l' => self.on = false,
                                _ => {}
                            }
                        }
                        State::Text
                    }
                    // Intermediate bytes and the other private markers: a
                    // sequence of another kind, read to its end and ignored.
                    _ => {
                        csi.opened = false;
                        csi.private = false;
                        csi.names_paste = false;
                        csi.param = None;
                        State::Csi(csi)
                    }
                },
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(chunks: &[&[u8]]) -> bool {
        let mut mode = PasteMode::default();
        for chunk in chunks {
            mode.feed(chunk);
        }
        mode.is_on()
    }

    #[test]
    fn the_mode_follows_what_the_far_end_last_asked_for() {
        assert!(!after(&[b"SW1#show clock\r\n12:00:00 UTC\r\nSW1#"]));
        assert!(after(&[b"\x1b[?2004huser@host:~$ "]));
        assert!(!after(&[b"\x1b[?2004h$ exit\r\n\x1b[?2004l\r\nlogout\r\n"]));
        // Set together with other modes, as a full-screen program does.
        assert!(after(&[b"\x1b[?1049;2004h"]));
        assert!(after(&[b"\x1b[?2004;1h"]));
        assert!(!after(&[b"\x1b[?2004h", b"\x1b[?1;2004l"]));
        // A full reset takes it back.
        assert!(!after(&[b"\x1b[?2004h\x1bc"]));
    }

    #[test]
    fn a_sequence_split_between_two_reads_still_counts() {
        assert!(after(&[b"prompt \x1b", b"[?2004h"]));
        assert!(after(&[b"\x1b[?20", b"04h"]));
        assert!(after(&[b"\x1b[", b"?", b"2", b"0", b"0", b"4", b"h"]));
        assert!(!after(&[b"\x1b[?2004h\x1b[?200", b"4l"]));
    }

    #[test]
    fn what_only_looks_like_the_request_is_not_one() {
        // Not private: SM 2004 is another mode altogether.
        assert!(!after(&[b"\x1b[2004h"]));
        // Another number, another final byte, a question mark in the wrong place.
        assert!(!after(&[b"\x1b[?2005h"]));
        assert!(!after(&[b"\x1b[?12004h"]));
        assert!(!after(&[b"\x1b[?2004m"]));
        assert!(!after(&[b"\x1b[2004?h"]));
        // Printed, not sent as a sequence: a device showing its own log.
        assert!(!after(&[b"[?2004h"]));
        // A sequence that was abandoned does not swallow the text after it.
        assert!(!after(&[b"\x1b[?2004\x18h"]));
        assert!(after(&[b"\x1b[?99\x1b[?2004h"]));
        // A number too long to be one is not 2004 either.
        assert!(!after(&[b"\x1b[?99999999999999999992004h"]));
    }
}
