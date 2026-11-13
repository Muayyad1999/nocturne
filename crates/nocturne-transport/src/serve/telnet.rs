//! Telnet server front-end for in-process sessions.
//!
//! The server puts the client in character-at-a-time mode (`WILL ECHO`,
//! `WILL SGA`) and asks for its window size (`DO NAWS`); everything else a
//! client proposes is refused.

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
use crate::relay::Relay;
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
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(2);
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
        naws_refused: false,
    };

    let mut hello = Vec::new();
    client.options.offer(OPT_ECHO, &mut hello);
    client.options.offer(OPT_SGA, &mut hello);
    client.options.request(OPT_NAWS, &mut hello);
    if socket_tx.write_all(&hello).await.is_err() {
        return;
    }

    // Learn the window size first, so the session starts out right.
    let mut early = Vec::new();
    let deadline = Instant::now() + WINDOW_SIZE_WAIT;
    while client.size.is_none() && !client.naws_refused {
        let Ok(received) = tokio::time::timeout_at(deadline, client.read()).await else {
            break;
        };
        let Some(received) = received else {
            return; // gone already
        };
        early.extend(received.inputs);
        if socket_tx.write_all(&received.answer).await.is_err() {
            return;
        }
    }
    let (cols, rows) = client.size.unwrap_or(DEFAULT_SIZE);
    // The size is passed to `open`; what was collected before is history.
    early.retain(|input| !matches!(input, Input::Resize { .. }));

    let Some(connection) = handler.open(peer, None, cols, rows).await else {
        return;
    };
    let (writer, mut device) = connection.split();
    let mut relay = Relay::new(writer.input.clone(), Input::Data, INPUT_HIGH_WATER);
    for input in early {
        stage(&mut relay, input);
    }
    let (answers_tx, mut answers_rx) = mpsc::channel(32);

    let client_left = tokio::select! {
        () = client_to_device(&mut client, &mut relay, answers_tx) => true,
        () = device_to_client(&mut device, &mut answers_rx, &mut socket_tx) => false,
        () = shutdown.requested() => true,
    };
    if client_left {
        // End the device session, then let go of it.
        relay.push_item(Input::Close);
        let _ = tokio::time::timeout(GOODBYE_TIMEOUT, relay.flush()).await;
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
    naws_refused: bool,
}

/// What one read from the client amounts to.
struct Received {
    /// For the device, in order.
    inputs: Vec<Input>,
    /// Negotiation answers for the client.
    answer: Vec<u8>,
}

impl Client {
    /// Reads from the socket once. `None` when the client is gone.
    /// Cancel-safe: nothing is consumed unless the read completes.
    async fn read(&mut self) -> Option<Received> {
        let count = match self.socket.read(&mut self.buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(count) => count,
        };
        self.parser.feed(&self.buffer[..count], &mut self.tokens);
        let mut received = Received {
            inputs: Vec::new(),
            answer: Vec::new(),
        };
        for token in std::mem::take(&mut self.tokens) {
            match token {
                Token::Data(data) => received.inputs.push(Input::Data(data.freeze())),
                Token::Event(Event::Negotiation { verb, option }) => {
                    self.options.receive(verb, option, &mut received.answer);
                    if option == OPT_NAWS && verb == Verb::Wont {
                        self.naws_refused = true;
                    }
                }
                Token::Event(Event::Subnegotiation {
                    option: OPT_NAWS,
                    data,
                }) if self.options.remote_on(OPT_NAWS) => {
                    if let Some((cols, rows)) = decode_window_size(&data) {
                        self.size = Some((cols, rows));
                        received.inputs.push(Input::Resize { cols, rows });
                    }
                }
                Token::Event(Event::Command(BRK)) => received.inputs.push(Input::Break),
                Token::Event(Event::Command(IP)) => {
                    received
                        .inputs
                        .push(Input::Data(bytes::Bytes::from_static(&[INTERRUPT])));
                }
                Token::Event(_) => {}
            }
        }
        Some(received)
    }
}

fn stage(relay: &mut Relay<Input>, input: Input) {
    match input {
        Input::Data(data) => relay.push_data(data),
        other => relay.push_item(other),
    }
}

/// Client → device, until the client disconnects or the device stops taking
/// input.
async fn client_to_device(
    client: &mut Client,
    relay: &mut Relay<Input>,
    answers: mpsc::Sender<Vec<u8>>,
) {
    // Answers not yet handed to the writing half, and their size.
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut pending_bytes = 0;
    loop {
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !relay.is_empty() => {
                if forwarded.is_err() {
                    return;
                }
            }
            permit = answers.reserve(), if !pending.is_empty() => {
                let Ok(permit) = permit else { return };
                permit.send(pending.remove(0));
                if pending.is_empty() {
                    pending_bytes = 0;
                }
            }
            received = client.read(),
                if relay.has_room() && pending_bytes < ANSWER_HIGH_WATER =>
            {
                let Some(received) = received else { return };
                for input in received.inputs {
                    stage(relay, input);
                }
                if !received.answer.is_empty() {
                    pending_bytes += received.answer.len();
                    pending.push(received.answer);
                }
            }
        }
    }
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
