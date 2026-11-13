//! Telnet client.
//!
//! The client negotiates passively: it answers the server but never opens a
//! negotiation itself, so it can also talk to raw TCP services (console
//! servers) that would choke on unsolicited option commands.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

use crate::connection::{Backend, Connection, ConnectionInfo, Input, Output, TransportKind};
use crate::error::TransportError;
use crate::net;
use crate::pump::{Outcome, ReadEnd, WriteEnd, drive};
use crate::relay::Relay;

pub(crate) mod protocol;

use protocol::{
    AfterCr, BRK, Change, Event, IAC, Negotiator, OPT_ECHO, OPT_NAWS, OPT_SGA, OPT_TTYPE, Parser,
    TTYPE_SEND, Token, encode_data, encode_terminal_type, encode_window_size,
};

const READ_BUFFER: usize = 16 * 1024;
/// Unread output staged before the client stops reading from the socket.
const OUTPUT_HIGH_WATER: usize = 256 * 1024;
/// Negotiation answers waiting for the writer before reading pauses.
const ANSWER_HIGH_WATER: usize = 16 * 1024;
/// After asking to close, how long the server gets to close its side.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// How to reach a Telnet server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelnetParams {
    pub host: String,
    pub port: u16,
    /// Terminal type reported when the server asks (`TTYPE`).
    pub term: String,
    /// Window size reported when the server asks (`NAWS`).
    pub cols: u16,
    pub rows: u16,
    pub connect_timeout_ms: u64,
}

impl Default for TelnetParams {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 23,
            term: "xterm-256color".to_owned(),
            cols: 120,
            rows: 40,
            connect_timeout_ms: 10_000,
        }
    }
}

/// Connects to a Telnet server.
///
/// Returns as soon as the TCP connection is up; login prompts are part of
/// the data stream. `Input::Break` is sent as `IAC BRK`.
pub async fn connect_telnet(params: &TelnetParams) -> Result<Connection, TransportError> {
    let timeout = Duration::from_millis(params.connect_timeout_ms.max(1));
    let stream = net::connect(&params.host, params.port, timeout).await?;
    let info = ConnectionInfo {
        kind: Some(TransportKind::Telnet),
        peer: net::host_port(&params.host, params.port),
        ..ConnectionInfo::default()
    };
    let (connection, backend) = Connection::pair(info, Connection::DEFAULT_BUFFER);
    let terminal = Terminal {
        name: params.term.clone(),
        window: Window::new(params.cols, params.rows),
    };
    tokio::spawn(run(stream, backend, terminal));
    Ok(connection)
}

/// What the client tells the server about its terminal.
struct Terminal {
    name: String,
    window: Window,
}

/// The window size, shared by the half that learns the server wants it and
/// the half that sends it.
struct Window {
    /// `cols << 16 | rows`.
    size: AtomicU32,
    /// The server asked for the size (`DO NAWS`) and gets updates.
    reported: AtomicBool,
}

impl Window {
    fn new(cols: u16, rows: u16) -> Self {
        let window = Self {
            size: AtomicU32::new(0),
            reported: AtomicBool::new(false),
        };
        window.set(cols, rows);
        window
    }

    fn set(&self, cols: u16, rows: u16) {
        let packed = u32::from(cols) << 16 | u32::from(rows);
        self.size.store(packed, Ordering::Release);
    }

    fn encode(&self, out: &mut Vec<u8>) {
        let packed = self.size.load(Ordering::Acquire);
        // Both halves fit by construction.
        encode_window_size((packed >> 16) as u16, packed as u16, out);
    }
}

/// What the reading half asks the writing half to put on the wire.
enum Answer {
    Raw(Vec<u8>),
    /// The current window size (encoded when it is written, so that it can
    /// never be older than a resize that went out before it).
    WindowSize,
}

async fn run(stream: TcpStream, backend: Backend, terminal: Terminal) {
    let (mut socket_rx, mut socket_tx) = stream.into_split();
    let Backend { mut input, output } = backend;
    let mut relay = Relay::for_output(output.clone(), OUTPUT_HIGH_WATER);
    let terminal = Arc::new(terminal);
    let (answers_tx, mut answers_rx) = mpsc::channel(32);

    let outcome = drive(
        read_socket(&mut socket_rx, &mut relay, &output, answers_tx, &terminal),
        write_socket(
            &mut input,
            &mut answers_rx,
            &mut socket_tx,
            &terminal.window,
        ),
        CLOSE_GRACE,
    )
    .await;

    // Closing the socket comes before the farewell.
    drop((socket_rx, socket_tx, input, output));
    if let Outcome::Closed(reason) = outcome {
        relay.finish(reason).await;
    }
}

/// Options the client performs when the server asks.
fn performs(option: u8) -> bool {
    matches!(option, OPT_TTYPE | OPT_NAWS)
}

/// Options the client lets the server perform.
fn lets_server_perform(option: u8) -> bool {
    matches!(option, OPT_ECHO | OPT_SGA)
}

/// Server → application: strips the protocol out of the stream and answers
/// negotiation.
async fn read_socket(
    socket: &mut OwnedReadHalf,
    relay: &mut Relay<Output>,
    output: &mpsc::Sender<Output>,
    answers: mpsc::Sender<Answer>,
    terminal: &Terminal,
) -> ReadEnd {
    let mut parser = Parser::new(AfterCr::DropNul);
    let mut options = Negotiator::new(performs, lets_server_perform);
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut tokens = Vec::new();
    // Answers not yet handed to the writing half, and their size.
    let mut pending: Vec<Answer> = Vec::new();
    let mut pending_bytes = 0;

    loop {
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !relay.is_empty() => {
                if forwarded.is_err() {
                    return ReadEnd::Abandoned;
                }
            }
            () = output.closed() => return ReadEnd::Abandoned,
            permit = answers.reserve(), if !pending.is_empty() => {
                // The writing half only goes away with the whole session.
                if let Ok(permit) = permit {
                    permit.send(pending.remove(0));
                }
                if pending.is_empty() {
                    pending_bytes = 0;
                }
            }
            read = socket.read(&mut buffer),
                if relay.has_room() && pending_bytes < ANSWER_HIGH_WATER =>
            {
                let count = match read {
                    Ok(0) => return ReadEnd::Finished(Some("closed by remote host".to_owned())),
                    Ok(count) => count,
                    Err(error) => {
                        return ReadEnd::Finished(Some(format!("connection lost: {error}")));
                    }
                };
                parser.feed(&buffer[..count], &mut tokens);
                for token in tokens.drain(..) {
                    let event = match token {
                        Token::Data(data) => {
                            relay.push_data(data.freeze());
                            continue;
                        }
                        Token::Event(event) => event,
                    };
                    for answer in answer_to(event, &mut options, terminal) {
                        pending_bytes += match &answer {
                            Answer::Raw(bytes) => bytes.len(),
                            Answer::WindowSize => 9,
                        };
                        pending.push(answer);
                    }
                }
            }
        }
    }
}

/// What the client says in response to one protocol event.
fn answer_to(event: Event, options: &mut Negotiator, terminal: &Terminal) -> Vec<Answer> {
    let mut raw = Vec::new();
    let mut window_size = false;
    match event {
        Event::Negotiation { verb, option } => match options.receive(verb, option, &mut raw) {
            Some(Change::LocalOn(OPT_NAWS)) => {
                terminal.window.reported.store(true, Ordering::Release);
                window_size = true;
            }
            Some(Change::LocalOff(OPT_NAWS)) => {
                terminal.window.reported.store(false, Ordering::Release);
            }
            _ => {}
        },
        Event::Subnegotiation {
            option: OPT_TTYPE,
            data,
        } if data.first() == Some(&TTYPE_SEND) && options.local_on(OPT_TTYPE) => {
            encode_terminal_type(&terminal.name, &mut raw);
        }
        // Other sub-negotiations and commands (NOP, GA, …) need no answer.
        Event::Subnegotiation { .. } | Event::Command(_) => {}
    }
    let mut answers = Vec::new();
    if !raw.is_empty() {
        answers.push(Answer::Raw(raw));
    }
    if window_size {
        answers.push(Answer::WindowSize);
    }
    answers
}

/// Application → server, plus the negotiation answers of the reading half.
async fn write_socket(
    input: &mut mpsc::Receiver<Input>,
    answers: &mut mpsc::Receiver<Answer>,
    socket: &mut OwnedWriteHalf,
    window: &Window,
) -> WriteEnd {
    let mut input_open = true;
    let mut wire = Vec::new();
    loop {
        wire.clear();
        tokio::select! {
            biased;
            answer = answers.recv() => match answer {
                Some(Answer::Raw(bytes)) => wire = bytes,
                Some(Answer::WindowSize) => window.encode(&mut wire),
                // The reading half is done, and so is the session.
                None => return WriteEnd::InputClosed,
            },
            item = input.recv(), if input_open => match item {
                Some(Input::Data(data)) => encode_data(&data, &mut wire),
                Some(Input::Resize { cols, rows }) => {
                    window.set(cols, rows);
                    if window.reported.load(Ordering::Acquire) {
                        window.encode(&mut wire);
                    }
                }
                Some(Input::Break) => wire.extend_from_slice(&[IAC, BRK]),
                Some(Input::Close) => {
                    let _ = socket.shutdown().await;
                    return WriteEnd::CloseRequested;
                }
                // No more input; negotiation still has to be answered.
                None => input_open = false,
            }
        }
        if !wire.is_empty()
            && let Err(error) = socket.write_all(&wire).await
        {
            return WriteEnd::Failed(format!("connection lost: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_default_and_partial_json() {
        let params: TelnetParams = serde_json::from_str(r#"{"host":"10.0.0.1"}"#).unwrap();
        assert_eq!(
            params,
            TelnetParams {
                host: "10.0.0.1".into(),
                ..TelnetParams::default()
            }
        );
        assert_eq!(params.port, 23);
        assert_eq!((params.cols, params.rows), (120, 40));
        assert_eq!(params.connect_timeout_ms, 10_000);
    }

    #[test]
    fn window_size_is_encoded_from_the_latest_resize() {
        let window = Window::new(120, 40);
        let mut out = Vec::new();
        window.encode(&mut out);
        assert_eq!(&out[3..7], [0, 120, 0, 40]);
        window.set(300, 255);
        out.clear();
        window.encode(&mut out);
        assert_eq!(&out[3..8], [1, 44, 0, IAC, IAC]);
    }
}
