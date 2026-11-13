//! The Telnet wire protocol (RFC 854/855): an incremental command parser,
//! option negotiation after RFC 1143 (the "Q method", which cannot loop), and
//! the encoding of outgoing data.

use bytes::BytesMut;

pub(crate) const IAC: u8 = 255;
pub(crate) const DONT: u8 = 254;
pub(crate) const DO: u8 = 253;
pub(crate) const WONT: u8 = 252;
pub(crate) const WILL: u8 = 251;
pub(crate) const SB: u8 = 250;
pub(crate) const IP: u8 = 244;
pub(crate) const BRK: u8 = 243;
pub(crate) const SE: u8 = 240;

pub(crate) const OPT_ECHO: u8 = 1;
pub(crate) const OPT_SGA: u8 = 3;
pub(crate) const OPT_TTYPE: u8 = 24;
pub(crate) const OPT_NAWS: u8 = 31;

const TTYPE_IS: u8 = 0;
pub(crate) const TTYPE_SEND: u8 = 1;

/// Longest sub-negotiation payload kept; anything beyond is discarded.
const MAX_SUBNEGOTIATION: usize = 1024;

/// The four negotiation commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verb {
    Will,
    Wont,
    Do,
    Dont,
}

impl Verb {
    fn code(self) -> u8 {
        match self {
            Self::Will => WILL,
            Self::Wont => WONT,
            Self::Do => DO,
            Self::Dont => DONT,
        }
    }
}

/// Something other than data found in the incoming stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Negotiation {
        verb: Verb,
        option: u8,
    },
    /// `IAC SB <option> <data> IAC SE`, with `data` unescaped.
    Subnegotiation {
        option: u8,
        data: Vec<u8>,
    },
    /// A two-byte command such as `IAC BRK` or `IAC NOP`.
    Command(u8),
}

/// A piece of the incoming stream, in the order it arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Token {
    Data(BytesMut),
    Event(Event),
}

/// What to do with the byte that follows a carriage return in incoming data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterCr {
    /// `CR NUL` is a bare carriage return (what a client does with data from
    /// a server).
    DropNul,
    /// Additionally `CR LF` is one "Enter" (what a server does with data from
    /// a client, which may send either form for the Return key).
    DropNulOrLf,
}

#[derive(Clone, Copy, Debug)]
enum State {
    Data,
    Iac,
    Negotiate(Verb),
    SubOption,
    Sub(u8),
    SubIac(u8),
}

/// Separates data from commands. Fed with whatever the network delivers, it
/// copes with sequences split at any byte.
#[derive(Debug)]
pub(crate) struct Parser {
    state: State,
    after_cr: AfterCr,
    last_was_cr: bool,
    sub: Vec<u8>,
}

impl Parser {
    pub fn new(after_cr: AfterCr) -> Self {
        Self {
            state: State::Data,
            after_cr,
            last_was_cr: false,
            sub: Vec::new(),
        }
    }

    /// Consumes `input`, appending what it contains to `out` in order;
    /// consecutive data bytes end up in one [`Token::Data`].
    pub fn feed(&mut self, input: &[u8], out: &mut Vec<Token>) {
        for &byte in input {
            self.state = match self.state {
                State::Data if byte == IAC => State::Iac,
                State::Data => {
                    self.data_byte(byte, out);
                    State::Data
                }
                State::Iac => self.command(byte, out),
                State::Negotiate(verb) => {
                    out.push(Token::Event(Event::Negotiation { verb, option: byte }));
                    State::Data
                }
                State::SubOption => {
                    self.sub.clear();
                    State::Sub(byte)
                }
                State::Sub(option) if byte == IAC => State::SubIac(option),
                State::Sub(option) => {
                    self.sub_byte(byte);
                    State::Sub(option)
                }
                State::SubIac(option) => match byte {
                    SE => {
                        out.push(Token::Event(Event::Subnegotiation {
                            option,
                            data: std::mem::take(&mut self.sub),
                        }));
                        State::Data
                    }
                    IAC => {
                        self.sub_byte(IAC);
                        State::Sub(option)
                    }
                    // Not a valid way to end a sub-negotiation: drop it and
                    // treat the byte as the command it is.
                    _ => {
                        self.sub.clear();
                        self.command(byte, out)
                    }
                },
            };
        }
    }

    /// Handles the byte after an `IAC`.
    fn command(&mut self, byte: u8, out: &mut Vec<Token>) -> State {
        match byte {
            IAC => {
                self.data_byte(IAC, out);
                State::Data
            }
            WILL => State::Negotiate(Verb::Will),
            WONT => State::Negotiate(Verb::Wont),
            DO => State::Negotiate(Verb::Do),
            DONT => State::Negotiate(Verb::Dont),
            SB => State::SubOption,
            SE => State::Data, // stray end marker
            other => {
                out.push(Token::Event(Event::Command(other)));
                State::Data
            }
        }
    }

    fn data_byte(&mut self, byte: u8, out: &mut Vec<Token>) {
        if std::mem::take(&mut self.last_was_cr) {
            let padding = byte == 0 || (byte == b'\n' && self.after_cr == AfterCr::DropNulOrLf);
            if padding {
                return;
            }
        }
        self.last_was_cr = byte == b'\r';
        match out.last_mut() {
            Some(Token::Data(data)) => data.extend_from_slice(&[byte]),
            _ => out.push(Token::Data(BytesMut::from(&[byte][..]))),
        }
    }

    fn sub_byte(&mut self, byte: u8) {
        if self.sub.len() < MAX_SUBNEGOTIATION {
            self.sub.push(byte);
        }
    }
}

/// RFC 1143 option state. (The `WANTNO` state of the RFC never arises here:
/// neither end of this crate asks to turn an option off on its own.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Q {
    No,
    Yes,
    WantYes,
}

/// An option that became active or inactive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// We now perform the option (the peer said `DO`, or agreed to our `WILL`).
    LocalOn(u8),
    LocalOff(u8),
    /// The peer now performs the option.
    RemoteOn(u8),
    RemoteOff(u8),
}

/// Tracks which options are in effect on each side and answers the peer's
/// requests: an answer is only sent when a request changes something or has
/// to be refused, so two negotiators can never ping-pong.
pub(crate) struct Negotiator {
    /// Options we perform.
    local: [Q; 256],
    /// Options the peer performs.
    remote: [Q; 256],
    /// Options we agree to perform when asked (`DO`).
    accept_local: fn(u8) -> bool,
    /// Options we let the peer perform when it offers (`WILL`).
    accept_remote: fn(u8) -> bool,
}

impl Negotiator {
    pub fn new(accept_local: fn(u8) -> bool, accept_remote: fn(u8) -> bool) -> Self {
        Self {
            local: [Q::No; 256],
            remote: [Q::No; 256],
            accept_local,
            accept_remote,
        }
    }

    pub fn local_on(&self, option: u8) -> bool {
        self.local[usize::from(option)] == Q::Yes
    }

    pub fn remote_on(&self, option: u8) -> bool {
        self.remote[usize::from(option)] == Q::Yes
    }

    /// Announce that we want to perform `option` (`WILL`).
    pub fn offer(&mut self, option: u8, out: &mut Vec<u8>) {
        let state = &mut self.local[usize::from(option)];
        if *state == Q::No {
            *state = Q::WantYes;
            out.extend_from_slice(&[IAC, WILL, option]);
        }
    }

    /// Ask the peer to perform `option` (`DO`).
    pub fn request(&mut self, option: u8, out: &mut Vec<u8>) {
        let state = &mut self.remote[usize::from(option)];
        if *state == Q::No {
            *state = Q::WantYes;
            out.extend_from_slice(&[IAC, DO, option]);
        }
    }

    /// Processes a negotiation command from the peer, appending the answer
    /// (if one is due) to `out`.
    pub fn receive(&mut self, verb: Verb, option: u8, out: &mut Vec<u8>) -> Option<Change> {
        let index = usize::from(option);
        match verb {
            Verb::Will | Verb::Wont => {
                let wanted = verb == Verb::Will;
                let acceptable = (self.accept_remote)(option);
                let (state, answer, changed) =
                    step(self.remote[index], wanted, acceptable, Verb::Do, Verb::Dont);
                self.remote[index] = state;
                reply(answer, option, out);
                changed.map(|on| {
                    if on {
                        Change::RemoteOn(option)
                    } else {
                        Change::RemoteOff(option)
                    }
                })
            }
            Verb::Do | Verb::Dont => {
                let wanted = verb == Verb::Do;
                let acceptable = (self.accept_local)(option);
                let (state, answer, changed) = step(
                    self.local[index],
                    wanted,
                    acceptable,
                    Verb::Will,
                    Verb::Wont,
                );
                self.local[index] = state;
                reply(answer, option, out);
                changed.map(|on| {
                    if on {
                        Change::LocalOn(option)
                    } else {
                        Change::LocalOff(option)
                    }
                })
            }
        }
    }
}

/// One RFC 1143 transition for a request to turn an option on (`wanted`) or
/// off. Returns the new state, the answer to send, and whether the option
/// just went on (`Some(true)`) or off (`Some(false)`).
fn step(
    state: Q,
    wanted: bool,
    acceptable: bool,
    agree: Verb,
    refuse: Verb,
) -> (Q, Option<Verb>, Option<bool>) {
    match (state, wanted) {
        // A request to enable.
        (Q::No, true) if acceptable => (Q::Yes, Some(agree), Some(true)),
        (Q::No, true) => (Q::No, Some(refuse), None),
        (Q::Yes, true) => (Q::Yes, None, None),
        // The peer agreed to what we asked for.
        (Q::WantYes, true) => (Q::Yes, None, Some(true)),
        // A request to disable, or the refusal of what we asked for.
        (Q::No, false) => (Q::No, None, None),
        (Q::Yes, false) => (Q::No, Some(refuse), Some(false)),
        (Q::WantYes, false) => (Q::No, None, None),
    }
}

fn reply(answer: Option<Verb>, option: u8, out: &mut Vec<u8>) {
    if let Some(verb) = answer {
        out.extend_from_slice(&[IAC, verb.code(), option]);
    }
}

/// Encodes application data for the wire: `IAC` is doubled, and a carriage
/// return that is not followed by a line feed becomes `CR NUL`.
pub(crate) fn encode_data(data: &[u8], out: &mut Vec<u8>) {
    out.reserve(data.len() + 2);
    let mut bytes = data.iter().copied().peekable();
    while let Some(byte) = bytes.next() {
        out.push(byte);
        match byte {
            IAC => out.push(IAC),
            b'\r' if bytes.peek() != Some(&b'\n') => out.push(0),
            _ => {}
        }
    }
}

/// `IAC SB NAWS <cols> <rows> IAC SE`.
pub(crate) fn encode_window_size(cols: u16, rows: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(&[IAC, SB, OPT_NAWS]);
    for byte in cols.to_be_bytes().into_iter().chain(rows.to_be_bytes()) {
        out.push(byte);
        if byte == IAC {
            out.push(IAC);
        }
    }
    out.extend_from_slice(&[IAC, SE]);
}

/// The payload of a NAWS sub-negotiation as `(cols, rows)`.
pub(crate) fn decode_window_size(data: &[u8]) -> Option<(u16, u16)> {
    match *data {
        [c1, c0, r1, r0] => Some((u16::from_be_bytes([c1, c0]), u16::from_be_bytes([r1, r0]))),
        _ => None,
    }
}

/// `IAC SB TTYPE IS <term> IAC SE`.
pub(crate) fn encode_terminal_type(term: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&[IAC, SB, OPT_TTYPE, TTYPE_IS]);
    // The name is plain NVT ASCII; anything else cannot be represented.
    out.extend(term.bytes().filter(|byte| byte.is_ascii_graphic()));
    out.extend_from_slice(&[IAC, SE]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses `input`, returning the data and the events separately.
    fn parse(parser: &mut Parser, input: &[u8]) -> (Vec<u8>, Vec<Event>) {
        let mut tokens = Vec::new();
        parser.feed(input, &mut tokens);
        split(tokens)
    }

    fn split(tokens: Vec<Token>) -> (Vec<u8>, Vec<Event>) {
        let mut data = Vec::new();
        let mut events = Vec::new();
        for token in tokens {
            match token {
                Token::Data(bytes) => data.extend_from_slice(&bytes),
                Token::Event(event) => events.push(event),
            }
        }
        (data, events)
    }

    fn client_parser() -> Parser {
        Parser::new(AfterCr::DropNul)
    }

    #[test]
    fn plain_data_passes_through() {
        let (data, events) = parse(&mut client_parser(), b"Router> show ip\r\n");
        assert_eq!(data, b"Router> show ip\r\n");
        assert!(events.is_empty());
    }

    #[test]
    fn commands_are_separated_from_data() {
        let input = [
            b'a', IAC, WILL, OPT_ECHO, b'b', IAC, DO, OPT_NAWS, IAC, 241, b'c', IAC, IAC, b'd',
        ];
        let (data, events) = parse(&mut client_parser(), &input);
        assert_eq!(data, [b'a', b'b', b'c', IAC, b'd']);
        assert_eq!(
            events,
            vec![
                Event::Negotiation {
                    verb: Verb::Will,
                    option: OPT_ECHO
                },
                Event::Negotiation {
                    verb: Verb::Do,
                    option: OPT_NAWS
                },
                Event::Command(241),
            ]
        );
    }

    #[test]
    fn subnegotiations_are_collected_and_unescaped() {
        let input = [
            b'x', IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE, b'y', IAC, SB, OPT_NAWS, 0, IAC, IAC, 0,
            24, IAC, SE, b'z',
        ];
        let (data, events) = parse(&mut client_parser(), &input);
        assert_eq!(data, b"xyz");
        assert_eq!(
            events,
            vec![
                Event::Subnegotiation {
                    option: OPT_TTYPE,
                    data: vec![TTYPE_SEND]
                },
                Event::Subnegotiation {
                    option: OPT_NAWS,
                    data: vec![0, IAC, 0, 24]
                },
            ]
        );
    }

    #[test]
    fn every_split_point_gives_the_same_result() {
        let input = [
            b'h', b'i', IAC, WILL, OPT_SGA, b'\r', 0, IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE, IAC,
            IAC, b'!', IAC, BRK, b'\r', b'\n', IAC, DONT, 200, b'.',
        ];
        let (expected_data, expected_events) = parse(&mut client_parser(), &input);
        assert_eq!(
            expected_data,
            [b'h', b'i', b'\r', IAC, b'!', b'\r', b'\n', b'.']
        );
        assert_eq!(expected_events.len(), 4);

        for first in 0..=input.len() {
            for second in first..=input.len() {
                let mut parser = client_parser();
                let mut tokens = Vec::new();
                for part in [&input[..first], &input[first..second], &input[second..]] {
                    parser.feed(part, &mut tokens);
                }
                let (data, events) = split(tokens);
                assert_eq!(data, expected_data, "split at {first}/{second}");
                assert_eq!(events, expected_events, "split at {first}/{second}");
            }
        }
    }

    #[test]
    fn tokens_keep_the_order_of_the_stream() {
        let input = [b'a', b'b', IAC, BRK, b'c', IAC, IP, IAC, IP, b'd'];
        let mut tokens = Vec::new();
        client_parser().feed(&input, &mut tokens);
        assert_eq!(
            tokens,
            vec![
                Token::Data(BytesMut::from(&b"ab"[..])),
                Token::Event(Event::Command(BRK)),
                Token::Data(BytesMut::from(&b"c"[..])),
                Token::Event(Event::Command(IP)),
                Token::Event(Event::Command(IP)),
                Token::Data(BytesMut::from(&b"d"[..])),
            ]
        );
    }

    #[test]
    fn one_byte_at_a_time() {
        let input = [IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE, b'o', b'k'];
        let mut parser = client_parser();
        let mut tokens = Vec::new();
        for byte in input {
            parser.feed(&[byte], &mut tokens);
        }
        let (data, events) = split(tokens);
        assert_eq!(data, b"ok");
        assert_eq!(
            events,
            vec![Event::Subnegotiation {
                option: OPT_NAWS,
                data: vec![0, 80, 0, 24]
            }]
        );
    }

    #[test]
    fn carriage_return_padding_depends_on_the_side() {
        let input = b"a\r\0b\r\nc\rd";
        let (data, _) = parse(&mut Parser::new(AfterCr::DropNul), input);
        assert_eq!(data, b"a\rb\r\nc\rd");
        let (data, _) = parse(&mut Parser::new(AfterCr::DropNulOrLf), input);
        assert_eq!(data, b"a\rb\rc\rd");
        // Only the byte right after the CR is padding.
        let (data, _) = parse(&mut Parser::new(AfterCr::DropNulOrLf), b"\r\n\n\r\0\0");
        assert_eq!(data, b"\r\n\r\0");
    }

    #[test]
    fn malformed_sequences_do_not_derail_the_parser() {
        // A stray SE, and a sub-negotiation cut short by another command.
        let input = [
            b'a', IAC, SE, b'b', IAC, SB, OPT_NAWS, 1, 2, IAC, WILL, OPT_ECHO, b'c',
        ];
        let (data, events) = parse(&mut client_parser(), &input);
        assert_eq!(data, b"abc");
        assert_eq!(
            events,
            vec![Event::Negotiation {
                verb: Verb::Will,
                option: OPT_ECHO
            }]
        );
    }

    #[test]
    fn oversized_subnegotiations_are_truncated_not_fatal() {
        let mut input = vec![IAC, SB, 39];
        input.extend(std::iter::repeat_n(b'x', MAX_SUBNEGOTIATION * 3));
        input.extend([IAC, SE, b'!']);
        let (data, events) = parse(&mut client_parser(), &input);
        assert_eq!(data, b"!");
        let [Event::Subnegotiation { option: 39, data }] = &events[..] else {
            panic!("unexpected events: {events:?}");
        };
        assert_eq!(data.len(), MAX_SUBNEGOTIATION);
    }

    fn accept_tty_and_naws(option: u8) -> bool {
        matches!(option, OPT_TTYPE | OPT_NAWS)
    }

    fn accept_echo_and_sga(option: u8) -> bool {
        matches!(option, OPT_ECHO | OPT_SGA)
    }

    fn client() -> Negotiator {
        Negotiator::new(accept_tty_and_naws, accept_echo_and_sga)
    }

    #[test]
    fn acceptable_offers_are_accepted_once() {
        let mut options = client();
        let mut out = Vec::new();
        let change = options.receive(Verb::Will, OPT_ECHO, &mut out);
        assert_eq!(change, Some(Change::RemoteOn(OPT_ECHO)));
        assert_eq!(out, [IAC, DO, OPT_ECHO]);
        assert!(options.remote_on(OPT_ECHO));

        // Repeating the offer changes nothing and gets no answer: no loop.
        out.clear();
        assert_eq!(options.receive(Verb::Will, OPT_ECHO, &mut out), None);
        assert!(out.is_empty());
    }

    #[test]
    fn requests_for_supported_options_are_granted() {
        let mut options = client();
        let mut out = Vec::new();
        assert_eq!(
            options.receive(Verb::Do, OPT_NAWS, &mut out),
            Some(Change::LocalOn(OPT_NAWS))
        );
        assert_eq!(out, [IAC, WILL, OPT_NAWS]);
        assert!(options.local_on(OPT_NAWS));
        out.clear();
        assert_eq!(options.receive(Verb::Do, OPT_NAWS, &mut out), None);
        assert!(out.is_empty());
    }

    #[test]
    fn everything_else_is_refused() {
        let mut options = client();
        let mut out = Vec::new();
        assert_eq!(options.receive(Verb::Will, 5, &mut out), None);
        assert_eq!(options.receive(Verb::Do, OPT_ECHO, &mut out), None);
        assert_eq!(options.receive(Verb::Do, 0, &mut out), None);
        assert_eq!(out, [IAC, DONT, 5, IAC, WONT, OPT_ECHO, IAC, WONT, 0]);
        assert!(!options.remote_on(5));
        assert!(!options.local_on(OPT_ECHO));
    }

    #[test]
    fn disabling_is_acknowledged_once() {
        let mut options = client();
        let mut out = Vec::new();
        options.receive(Verb::Will, OPT_ECHO, &mut out);
        options.receive(Verb::Do, OPT_NAWS, &mut out);
        out.clear();

        assert_eq!(
            options.receive(Verb::Wont, OPT_ECHO, &mut out),
            Some(Change::RemoteOff(OPT_ECHO))
        );
        assert_eq!(
            options.receive(Verb::Dont, OPT_NAWS, &mut out),
            Some(Change::LocalOff(OPT_NAWS))
        );
        assert_eq!(out, [IAC, DONT, OPT_ECHO, IAC, WONT, OPT_NAWS]);

        // Already off: silence.
        out.clear();
        assert_eq!(options.receive(Verb::Wont, OPT_ECHO, &mut out), None);
        assert_eq!(options.receive(Verb::Dont, OPT_NAWS, &mut out), None);
        assert_eq!(options.receive(Verb::Wont, 77, &mut out), None);
        assert!(out.is_empty());
    }

    #[test]
    fn our_own_requests_complete_without_an_extra_round() {
        let mut server = Negotiator::new(accept_echo_and_sga, |option| option == OPT_NAWS);
        let mut out = Vec::new();
        server.offer(OPT_ECHO, &mut out);
        server.offer(OPT_ECHO, &mut out); // not repeated while pending
        server.request(OPT_NAWS, &mut out);
        assert_eq!(out, [IAC, WILL, OPT_ECHO, IAC, DO, OPT_NAWS]);
        assert!(!server.local_on(OPT_ECHO));

        // The peer agrees: nothing more to say.
        out.clear();
        assert_eq!(
            server.receive(Verb::Do, OPT_ECHO, &mut out),
            Some(Change::LocalOn(OPT_ECHO))
        );
        assert_eq!(
            server.receive(Verb::Will, OPT_NAWS, &mut out),
            Some(Change::RemoteOn(OPT_NAWS))
        );
        assert!(out.is_empty());

        // The peer refuses: accepted silently.
        let mut server = Negotiator::new(accept_echo_and_sga, |option| option == OPT_NAWS);
        server.offer(OPT_SGA, &mut out);
        server.request(OPT_NAWS, &mut out);
        out.clear();
        assert_eq!(server.receive(Verb::Dont, OPT_SGA, &mut out), None);
        assert_eq!(server.receive(Verb::Wont, OPT_NAWS, &mut out), None);
        assert!(out.is_empty());
        assert!(!server.local_on(OPT_SGA) && !server.remote_on(OPT_NAWS));
    }

    #[test]
    fn two_negotiators_settle_instead_of_looping() {
        let mut server = Negotiator::new(accept_echo_and_sga, |option| option == OPT_NAWS);
        let mut client = client();
        let mut to_client = Vec::new();
        server.offer(OPT_ECHO, &mut to_client);
        server.offer(OPT_SGA, &mut to_client);
        server.request(OPT_NAWS, &mut to_client);
        server.request(OPT_TTYPE, &mut to_client);

        let mut rounds = 0;
        while !to_client.is_empty() {
            rounds += 1;
            assert!(rounds < 5, "negotiation does not settle");
            let mut to_server = Vec::new();
            for triple in to_client.chunks(3) {
                let verb = match triple[1] {
                    WILL => Verb::Will,
                    WONT => Verb::Wont,
                    DO => Verb::Do,
                    _ => Verb::Dont,
                };
                client.receive(verb, triple[2], &mut to_server);
            }
            to_client.clear();
            for triple in to_server.chunks(3) {
                let verb = match triple[1] {
                    WILL => Verb::Will,
                    WONT => Verb::Wont,
                    DO => Verb::Do,
                    _ => Verb::Dont,
                };
                server.receive(verb, triple[2], &mut to_client);
            }
        }
        assert!(client.remote_on(OPT_ECHO) && client.remote_on(OPT_SGA));
        assert!(client.local_on(OPT_NAWS) && client.local_on(OPT_TTYPE));
        assert!(server.local_on(OPT_ECHO) && server.remote_on(OPT_NAWS));
    }

    #[test]
    fn outgoing_data_is_escaped() {
        let mut out = Vec::new();
        encode_data(&[b'a', IAC, b'b'], &mut out);
        assert_eq!(out, [b'a', IAC, IAC, b'b']);

        out.clear();
        encode_data(b"show version\r", &mut out);
        assert_eq!(out, b"show version\r\0");

        out.clear();
        encode_data(b"line1\r\nline2\rX\n", &mut out);
        assert_eq!(out, b"line1\r\nline2\r\0X\n");

        out.clear();
        encode_data(b"\r\r\n", &mut out);
        assert_eq!(out, b"\r\0\r\n");
    }

    #[test]
    fn encoded_data_survives_a_round_trip() {
        let original: Vec<u8> = (0..=255u8)
            .chain([b'\r', b'x', b'\r', b'\n', IAC, IAC])
            .collect();
        let mut wire = Vec::new();
        encode_data(&original, &mut wire);
        let (data, events) = parse(&mut client_parser(), &wire);
        assert_eq!(data, original);
        assert!(events.is_empty());
    }

    #[test]
    fn window_size_round_trips_even_with_255() {
        let mut out = Vec::new();
        encode_window_size(120, 40, &mut out);
        assert_eq!(out, [IAC, SB, OPT_NAWS, 0, 120, 0, 40, IAC, SE]);

        out.clear();
        encode_window_size(255, 0xFFFF, &mut out);
        assert_eq!(
            out,
            [IAC, SB, OPT_NAWS, 0, IAC, IAC, IAC, IAC, IAC, IAC, IAC, SE]
        );
        let (_, events) = parse(&mut client_parser(), &out);
        let [Event::Subnegotiation { data, .. }] = &events[..] else {
            panic!("unexpected events: {events:?}");
        };
        assert_eq!(decode_window_size(data), Some((255, 0xFFFF)));
        assert_eq!(decode_window_size(&[0, 80, 0]), None);
    }

    #[test]
    fn terminal_type_is_sent_as_given() {
        let mut out = Vec::new();
        encode_terminal_type("xterm-256color", &mut out);
        let mut expected = vec![IAC, SB, OPT_TTYPE, TTYPE_IS];
        expected.extend_from_slice(b"xterm-256color");
        expected.extend_from_slice(&[IAC, SE]);
        assert_eq!(out, expected);

        out.clear();
        encode_terminal_type("vt 100\u{ff}", &mut out);
        assert_eq!(&out[4..out.len() - 2], b"vt100");
    }
}
