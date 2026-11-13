//! Telnet server front-end for in-process sessions.
//!
//! The server puts the client in character-at-a-time mode (`WILL ECHO`,
//! `WILL SGA`) and asks for its window size (`DO NAWS`); everything else a
//! client proposes is refused. `IAC BRK` reaches the device as
//! `Input::Break` and `IAC IP` as Ctrl-C. A client that closes its sending
//! side (`nc -N`) is still sent what the device has to say to the input it
//! got.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::{ServerHandle, SessionHandler, ShutdownSignal, spawn_server};
use crate::connection::{ConnectionReader, Input, Output};
use crate::error::TransportError;
use crate::relay::{Relay, Staging};
use crate::telnet::protocol::{
    AfterCr, BRK, Event, IP, Negotiator, OPT_ECHO, OPT_NAWS, OPT_SGA, Parser, Token, Verb,
    decode_window_size, encode_data,
};

/// Terminal size assumed for clients that do not report theirs.
const DEFAULT_SIZE: (u16, u16) = (80, 24);
/// How long a new client gets to report its window size before the session
/// is opened with the default.
const WINDOW_SIZE_WAIT: Duration = Duration::from_millis(250);
const READ_BUFFER: usize = 8 * 1024;
/// Unconsumed client input staged before the server stops reading.
const INPUT_HIGH_WATER: usize = 256 * 1024;
/// Negotiation answers waiting for the writer before reading pauses.
const ANSWER_HIGH_WATER: usize = 16 * 1024;
/// How long ending the bridged session may take, and how long a device gets
/// for its last words once a client stopped sending.
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a device may leave input untouched once the client stopped
/// sending (and may be gone for good) before the session is given up.
const STALL_LIMIT: Duration = Duration::from_secs(10);
/// Ctrl-C, which is what "interrupt process" means to a device console.
const INTERRUPT: u8 = 0x03;

/// Serves Telnet on `bind`, bridging every connection to the connection the
/// handler opens for it (`user` is always `None`: Telnet has no login of its
/// own).
pub async fn serve_telnet(
    bind: SocketAddr,
    handler: Arc<dyn SessionHandler>,
) -> Result<ServerHandle, TransportError> {
    let listener = TcpListener::bind(bind).await?;
    let local_addr = listener.local_addr()?;
    Ok(spawn_server(
        listener,
        local_addr,
        move |stream, peer, shutdown| {
            serve_connection(stream, peer, Arc::clone(&handler), shutdown)
        },
    ))
}

/// Options the server performs.
fn performs(option: u8) -> bool {
    matches!(option, OPT_ECHO | OPT_SGA)
}

/// Options the server lets the client perform.
fn lets_client_perform(option: u8) -> bool {
    option == OPT_NAWS
}

async fn serve_connection(
    stream: TcpStream,
    peer: SocketAddr,
    handler: Arc<dyn SessionHandler>,
    mut shutdown: ShutdownSignal,
) {
    let (socket_rx, mut socket_tx) = stream.into_split();
    let mut client = Client {
        socket: socket_rx,
        parser: Parser::new(AfterCr::DropNulOrLf),
        options: Negotiator::new(performs, lets_client_perform),
        buffer: vec![0u8; READ_BUFFER],
        tokens: Vec::new(),
        size: None,
        size_withheld: false,
        finished: false,
    };

    let mut hello = Vec::new();
    client.options.offer(OPT_ECHO, &mut hello);
    client.options.offer(OPT_SGA, &mut hello);
    client.options.request(OPT_NAWS, &mut hello);
    if socket_tx.write_all(&hello).await.is_err() {
        return;
    }

    // Learn the window size first, so the session starts out right.
    let mut early = Staging::for_input(INPUT_HIGH_WATER);
    let deadline = Instant::now() + WINDOW_SIZE_WAIT;
    while client.size.is_none() && !client.size_withheld && !client.finished && early.has_room() {
        let Ok(received) = tokio::time::timeout_at(deadline, client.read()).await else {
            break;
        };
        for input in received.inputs {
            // The size is passed to `open`; a resize from before is history.
            if !matches!(input, Input::Resize { .. }) {
                early.push(input);
            }
        }
        if socket_tx.write_all(&received.answer).await.is_err() {
            return;
        }
    }
    let (cols, rows) = client.size.unwrap_or(DEFAULT_SIZE);

    let Some(connection) = handler.open(peer, None, cols, rows).await else {
        return;
    };
    let (writer, mut device) = connection.split();
    let mut relay = Relay::with_staged(writer.input.clone(), early);
    let (answers_tx, mut answers_rx) = mpsc::channel(32);

    tokio::select! {
        // The client is gone, and the device session was ended.
        () = client_to_device(&mut client, &mut relay, answers_tx) => {}
        // The device session is over, or the client cannot be reached.
        () = device_to_client(&mut device, &mut answers_rx, &mut socket_tx) => {}
        () = shutdown.requested() => {
            relay.push(Input::Close);
            let _ = tokio::time::timeout(GOODBYE_TIMEOUT, relay.flush()).await;
        }
    }
}

/// The receiving side of one client connection.
struct Client {
    socket: OwnedReadHalf,
    parser: Parser,
    options: Negotiator,
    buffer: Vec<u8>,
    tokens: Vec<Token>,
    /// The window size the client reported, if it did.
    size: Option<(u16, u16)>,
    /// The client will not tell: it refused `NAWS`, or reported no size.
    size_withheld: bool,
    /// The client will not send anything more: it closed its side of the
    /// connection, if not all of it.
    finished: bool,
}

/// What one read from the client amounts to.
#[derive(Default)]
struct Received {
    /// For the device, in order.
    inputs: Vec<Input>,
    /// Negotiation answers for the client.
    answer: Vec<u8>,
}

impl Client {
    /// Reads from the socket once; the end of the stream sets `finished`.
    /// Cancel-safe: nothing is consumed unless the read completes.
    async fn read(&mut self) -> Received {
        let mut received = Received::default();
        let count = match self.socket.read(&mut self.buffer).await {
            Ok(0) | Err(_) => {
                self.finished = true;
                return received;
            }
            Ok(count) => count,
        };
        self.parser.feed(&self.buffer[..count], &mut self.tokens);
        for token in std::mem::take(&mut self.tokens) {
            match token {
                Token::Data(data) => received.inputs.push(Input::Data(data.freeze())),
                Token::Event(Event::Negotiation { verb, option }) => {
                    self.options.receive(verb, option, &mut received.answer);
                    if option == OPT_NAWS && verb == Verb::Wont {
                        self.size_withheld = true;
                    }
                }
                Token::Event(Event::Subnegotiation {
                    option: OPT_NAWS,
                    data,
                }) if self.options.remote_on(OPT_NAWS) => match decode_window_size(&data) {
                    // RFC 1073: zero means "not telling".
                    Some((cols, rows)) if cols > 0 && rows > 0 => {
                        self.size = Some((cols, rows));
                        received.inputs.push(Input::Resize { cols, rows });
                    }
                    _ => self.size_withheld = true,
                },
                Token::Event(Event::Command(BRK)) => received.inputs.push(Input::Break),
                Token::Event(Event::Command(IP)) => {
                    received
                        .inputs
                        .push(Input::Data(bytes::Bytes::from_static(&[INTERRUPT])));
                }
                Token::Event(_) => {}
            }
        }
        received
    }
}

/// Client → device, until the client is gone and the device session was told
/// to end.
async fn client_to_device(
    client: &mut Client,
    relay: &mut Relay<Input>,
    answers: mpsc::Sender<Vec<u8>>,
) {
    // Answers not yet handed to the writing half, and their size.
    let mut pending: VecDeque<Vec<u8>> = VecDeque::new();
    let mut pending_bytes = 0;
    // A device that stopped taking input may still have things to say: the
    // session goes on, and what the client sends is dropped.
    let mut deaf = false;

    while !client.finished {
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !deaf && !relay.is_empty() => {
                deaf = forwarded.is_err();
            }
            permit = answers.reserve(), if !pending.is_empty() => {
                match (permit, pending.pop_front()) {
                    (Ok(permit), Some(answer)) => permit.send(answer),
                    // Nobody is left to write them.
                    _ => pending.clear(),
                }
                if pending.is_empty() {
                    pending_bytes = 0;
                }
            }
            received = client.read(),
                if (deaf || relay.has_room()) && pending_bytes < ANSWER_HIGH_WATER =>
            {
                if !deaf {
                    for input in received.inputs {
                        relay.push(input);
                    }
                }
                if !received.answer.is_empty() {
                    pending_bytes += received.answer.len();
                    pending.push_back(received.answer);
                }
            }
        }
    }

    // The client is done sending — or gone altogether, which looks the same
    // from here. Either way the device gets what was sent, is told to end the
    // session, and may say its last words (the other direction is still
    // running); but a device that does not move is not waited for.
    relay.push(Input::Close);
    while !deaf && !relay.is_empty() {
        let forwarded = tokio::time::timeout(STALL_LIMIT, relay.forward_one()).await;
        if !matches!(forwarded, Ok(Ok(()))) {
            return;
        }
    }
    tokio::time::sleep(GOODBYE_TIMEOUT).await;
}

/// Device → client (plus negotiation answers), until the device session ends
/// or the client is gone.
async fn device_to_client(
    device: &mut ConnectionReader,
    answers: &mut mpsc::Receiver<Vec<u8>>,
    socket: &mut OwnedWriteHalf,
) {
    let mut wire = Vec::new();
    loop {
        wire.clear();
        tokio::select! {
            biased;
            answer = answers.recv() => match answer {
                Some(answer) => wire = answer,
                None => return,
            },
            output = device.recv() => match output {
                Some(Output::Data(data)) => encode_data(&data, &mut wire),
                Some(Output::Closed { .. }) | None => {
                    let _ = socket.shutdown().await;
                    return;
                }
            }
        }
        if socket.write_all(&wire).await.is_err() {
            return;
        }
    }
}
