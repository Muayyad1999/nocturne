//! The Telnet client against `serve_telnet`, and against a scripted raw TCP
//! peer that negotiates awkwardly.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::support::{Lab, PATIENCE, Tap, contains, pattern, soon, tap};
use crate::serve::{ServerHandle, serve_telnet};
use crate::telnet::protocol::{
    BRK, DO, DONT, IAC, OPT_ECHO, OPT_NAWS, OPT_SGA, OPT_TTYPE, SB, SE, TTYPE_SEND, WILL, WONT,
};
use crate::{Connection, TelnetParams, TransportKind, connect_telnet};

const NOP: u8 = 241;
const GA: u8 = 249;
/// NEW-ENVIRON and STATUS: options this client has no business with.
const OPT_NEW_ENVIRON: u8 = 39;
const OPT_STATUS: u8 = 5;

async fn server() -> (ServerHandle, Arc<Lab>) {
    let lab = Lab::new();
    let handle = serve_telnet("127.0.0.1:0".parse().unwrap(), lab.clone())
        .await
        .unwrap();
    (handle, lab)
}

fn params(address: SocketAddr) -> TelnetParams {
    TelnetParams {
        host: address.ip().to_string(),
        port: address.port(),
        ..TelnetParams::default()
    }
}

async fn connect(params: &TelnetParams) -> Tap<Connection> {
    tap(soon(connect_telnet(params)).await.unwrap())
}

// ------------------------------------------------- against `serve_telnet`

#[tokio::test]
async fn session_round_trip() {
    let (server, lab) = server().await;
    let params = TelnetParams {
        cols: 132,
        rows: 50,
        ..params(server.local_addr())
    };
    let mut conn = connect(&params).await;
    // The server learned the window size before it opened the session.
    let greeting = conn.until(b"lab> ").await;
    assert!(
        contains(&greeting, b"Welcome nobody 132x50"),
        "{:?}",
        String::from_utf8_lossy(&greeting)
    );
    assert_eq!(conn.info().kind, Some(TransportKind::Telnet));
    assert_eq!(conn.info().peer, server.local_addr().to_string());

    conn.send("hello\r").await.unwrap();
    assert!(contains(&conn.until(b"lab> ").await, b"hello"));

    conn.resize(100, 30).await.unwrap();
    conn.until(b"[resize 100x30]").await;
    conn.send("size\r").await.unwrap();
    conn.until(b"100x30\r\nlab> ").await;

    assert!(conn.supports_break());
    conn.send_break().await.unwrap();
    conn.until(b"[break]").await;

    conn.send("exit\r").await.unwrap();
    let (rest, reason) = conn.until_closed().await;
    assert!(contains(&rest, b"bye"));
    assert_eq!(reason.as_deref(), Some("closed by remote host"));
    lab.wait_idle().await;
    soon(server.shutdown()).await;
}

#[tokio::test]
async fn local_close_has_no_reason_and_ends_the_remote_session() {
    let (server, lab) = server().await;
    let (writer, reader) = connect(&params(server.local_addr())).await.inner.split();
    let mut reader = tap(reader);
    reader.until(b"lab> ").await;
    writer.close().await;
    let (_, reason) = reader.until_closed().await;
    assert_eq!(reason, None);
    lab.wait_idle().await;

    // Dropping works as well.
    let mut conn = connect(&params(server.local_addr())).await;
    conn.until(b"lab> ").await;
    drop(conn);
    lab.wait_idle().await;
    assert_eq!(lab.opened.load(Ordering::Acquire), 2);
}

#[tokio::test]
async fn bytes_that_need_escaping_survive_both_ways() {
    let (server, _lab) = server().await;
    let mut conn = connect(&params(server.local_addr())).await;
    conn.until(b"lab> ").await;

    // 0xFF is the Telnet escape byte; it must come back as one byte.
    conn.send(vec![b'a', 0xFF, b'b']).await.unwrap();
    assert_eq!(conn.exactly(3).await, [b'a', 0xFF, b'b']);

    // A bare CR and a CR LF are each one "Enter" for the device: the echo of
    // the line, then exactly one new prompt.
    conn.send("\r").await.unwrap();
    assert_eq!(conn.exactly(8).await, b"\r\r\nlab> ");
    conn.send("x\r\n").await.unwrap();
    assert_eq!(conn.exactly(9).await, b"x\r\r\nlab> ");

    // From the device: 0xFF, NUL and a bare CR arrive untouched.
    conn.send("raw\r").await.unwrap();
    conn.until(b"raw\r\r\n").await;
    assert_eq!(conn.exactly(5).await, [0xFF, 0x00, 0xFF, b'\r', b'x']);
    conn.until(b"\r\nlab> ").await;
    conn.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bulk_data_in_both_directions() {
    let (server, _lab) = server().await;
    let mut conn = connect(&params(server.local_addr())).await;
    conn.until(b"lab> ").await;

    // A large answer…
    const TOTAL: usize = 3_000_000;
    let command = format!("flood {TOTAL} 4000\r");
    conn.send(command.clone()).await.unwrap();
    conn.until(command.as_bytes()).await;
    let payload = conn.exactly(TOTAL).await;
    for (index, byte) in payload.iter().enumerate() {
        assert_eq!(*byte, pattern(index), "byte {index} is wrong");
    }
    conn.until(b"\r\nlab> ").await;

    // …and a large paste whose echo is only read afterwards.
    const CHUNK: usize = 500;
    const CHUNKS: usize = 400;
    let chunk: Vec<u8> = (0..CHUNK).map(pattern).collect();
    for _ in 0..CHUNKS {
        soon(conn.send(chunk.clone())).await.unwrap();
    }
    let echoed = conn.exactly(CHUNK * CHUNKS).await;
    for (index, byte) in echoed.iter().enumerate() {
        assert_eq!(*byte, pattern(index % CHUNK), "byte {index} is wrong");
    }
    conn.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_sessions_run_concurrently() {
    const SESSIONS: usize = 24;
    let (server, lab) = server().await;
    let mut clients = tokio::task::JoinSet::new();
    for index in 0..SESSIONS {
        let params = params(server.local_addr());
        clients.spawn(async move {
            let mut conn = connect(&params).await;
            conn.until(b"lab> ").await;
            let command = format!("flood {} 100\r", 10_000 + index);
            conn.send(command.clone()).await.unwrap();
            conn.until(command.as_bytes()).await;
            conn.exactly(10_000 + index).await;
            conn.until(b"\r\nlab> ").await;
            conn.close().await;
        });
    }
    while let Some(joined) = soon(clients.join_next()).await {
        joined.unwrap();
    }
    assert_eq!(lab.opened.load(Ordering::Acquire), SESSIONS);
    lab.wait_idle().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_last_words_of_a_device_survive_input_it_never_reads() {
    if !cfg!(target_os = "linux") {
        // The scenario cannot be staged here. Windows and the BSDs — macOS
        // among them — reset a socket that is closed while unread data is
        // still in its receive buffer, and a reset throws away what was
        // already in flight the other way —
        // including the "bye" this test exists to catch. The operating
        // system destroys the evidence before the reader has a chance to be
        // wrong about it. On a real network it is the *device* that decides
        // this when it hangs up, and a switch is not Windows; what is being
        // tested here is our reader, and it is tested on the platforms where
        // the last words survive long enough to look at.
        eprintln!(
            "skipping: this platform resets a socket closed with unread input, which \
             discards the device's last words before anything can read them"
        );
        return;
    }
    let (server, lab) = server().await;
    for _ in 0..20 {
        let mut conn = connect(&params(server.local_addr())).await;
        conn.until(b"lab> ").await;
        conn.send("exit\r").await.unwrap();
        for _ in 0..50 {
            if conn.send("typed ahead\r").await.is_err() {
                break;
            }
        }
        let (rest, reason) = conn.until_closed().await;
        assert!(
            rest.ends_with(b"\r\nbye\r\n"),
            "{:?}",
            String::from_utf8_lossy(&rest)
        );
        assert_eq!(reason.as_deref(), Some("closed by remote host"));
    }
    lab.wait_idle().await;
}

#[tokio::test]
async fn a_device_that_stopped_listening_is_still_heard() {
    let (server, lab) = server().await;
    let mut conn = connect(&params(server.local_addr())).await;
    conn.until(b"lab> ").await;
    conn.send("monologue\r").await.unwrap();
    for _ in 0..50 {
        conn.send("anyone there?\r").await.unwrap();
    }
    let (rest, reason) = conn.until_closed().await;
    assert!(
        rest.ends_with(b"\r\none\r\ntwo\r\nthree"),
        "{:?}",
        String::from_utf8_lossy(&rest)
    );
    assert_eq!(reason.as_deref(), Some("closed by remote host"));
    lab.wait_idle().await;
}

/// Sends `script` over a plain TCP connection, closes the sending side as
/// `nc -N` and friends do, and returns everything the server answers.
async fn pipe_into(server: &ServerHandle, script: &[u8]) -> Vec<u8> {
    let mut socket = TcpStream::connect(server.local_addr()).await.unwrap();
    socket.write_all(script).await.unwrap();
    socket.shutdown().await.unwrap();
    let mut answer = Vec::new();
    soon(socket.read_to_end(&mut answer)).await.unwrap();
    answer
}

#[tokio::test]
async fn a_client_that_only_closes_its_sending_side_gets_the_answer() {
    let (server, lab) = server().await;
    // The script is on the wire, and the sending side closed, before the
    // session even exists.
    let answer = pipe_into(&server, b"size\r\nflood 100000 1000\r\n").await;
    assert!(
        contains(&answer, b"Welcome nobody 80x24"),
        "{:?}",
        String::from_utf8_lossy(&answer)
    );
    assert!(contains(&answer, b"\r\n80x24\r\nlab> "));
    let flood: Vec<u8> = (0..100_000).map(pattern).collect();
    assert!(contains(&answer, &flood), "the output was cut short");
    assert!(answer.ends_with(b"\r\nlab> "));
    lab.wait_idle().await;

    // A device that says goodbye itself is heard to the end, too.
    let answer = pipe_into(&server, b"exit\r\nnever read\r\n").await;
    assert!(
        answer.ends_with(b"\r\nbye\r\n"),
        "{:?}",
        String::from_utf8_lossy(&answer)
    );
    lab.wait_idle().await;
}

#[tokio::test]
async fn server_shutdown_closes_the_sessions() {
    let (server, lab) = server().await;
    let mut conn = connect(&params(server.local_addr())).await;
    conn.until(b"lab> ").await;
    soon(server.shutdown()).await;
    let (_, reason) = conn.until_closed().await;
    assert_eq!(reason.as_deref(), Some("closed by remote host"));
    lab.wait_idle().await;
}

#[tokio::test]
async fn a_refused_session_is_just_closed() {
    let lab = Arc::new(Lab {
        closed_for_business: true,
        ..Lab::default()
    });
    let server = serve_telnet("127.0.0.1:0".parse().unwrap(), lab)
        .await
        .unwrap();
    let mut conn = connect(&params(server.local_addr())).await;
    let (seen, reason) = conn.until_closed().await;
    assert!(seen.is_empty());
    assert_eq!(reason.as_deref(), Some("closed by remote host"));
}

#[tokio::test]
async fn a_client_that_does_not_negotiate_still_gets_a_session() {
    let (server, lab) = server().await;
    // Plain TCP, like `nc`: never answers the server's option commands.
    let mut raw = TcpStream::connect(server.local_addr()).await.unwrap();
    let mut seen = Vec::new();
    let mut buffer = [0u8; 1024];
    while !contains(&seen, b"lab> ") {
        let count = soon(raw.read(&mut buffer)).await.unwrap();
        assert_ne!(count, 0, "closed early");
        seen.extend_from_slice(&buffer[..count]);
    }
    // The server's opening bid, then the session with the default size.
    assert!(seen.starts_with(&[IAC, WILL, OPT_ECHO, IAC, WILL, OPT_SGA, IAC, DO, OPT_NAWS]));
    assert!(contains(&seen, b"Welcome nobody 80x24"));

    raw.write_all(b"size\r\n").await.unwrap();
    seen.clear();
    while !contains(&seen, b"80x24\r\nlab> ") {
        let count = soon(raw.read(&mut buffer)).await.unwrap();
        assert_ne!(count, 0, "closed early");
        seen.extend_from_slice(&buffer[..count]);
    }
    drop(raw);
    lab.wait_idle().await;
}

// ------------------------------------------------------ against a raw peer

/// The far end of a client connection, played by the test.
struct Peer {
    socket: TcpStream,
}

impl Peer {
    /// Accepts the connection `connect_telnet` makes.
    async fn accept(params: TelnetParams) -> (Peer, Tap<Connection>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let params = TelnetParams {
            host: address.ip().to_string(),
            port: address.port(),
            ..params
        };
        let conn = connect(&params).await;
        let (socket, _) = soon(listener.accept()).await.unwrap();
        socket.set_nodelay(true).unwrap();
        (Peer { socket }, conn)
    }

    /// Sends each piece in its own TCP segment.
    async fn send_in_pieces(&mut self, pieces: &[&[u8]]) {
        for piece in pieces {
            self.socket.write_all(piece).await.unwrap();
            self.socket.flush().await.unwrap();
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }

    /// Reads exactly what the client is expected to have sent.
    async fn expect(&mut self, expected: &[u8]) {
        let mut received = vec![0u8; expected.len()];
        match tokio::time::timeout(PATIENCE, self.socket.read_exact(&mut received)).await {
            Ok(Ok(_)) => assert_eq!(received, expected, "unexpected bytes from the client"),
            other => panic!("the client did not send {expected:?}: {other:?}"),
        }
    }
}

#[tokio::test]
async fn negotiation_split_across_segments() {
    let (mut peer, mut conn) = Peer::accept(TelnetParams::default()).await;

    // Option commands cut at every awkward place, with data in between.
    peer.send_in_pieces(&[
        &[IAC],
        &[WILL],
        &[OPT_ECHO, IAC, WILL, OPT_SGA, b'l', b'o'],
        &[b'g', IAC],
        &[DO, OPT_TTYPE],
        &[b'i', b'n', IAC, DO],
        &[
            OPT_NAWS,
            IAC,
            DO,
            OPT_NEW_ENVIRON,
            IAC,
            WILL,
            OPT_STATUS,
            b':',
        ],
    ])
    .await;
    peer.expect(&[IAC, DO, OPT_ECHO]).await;
    peer.expect(&[IAC, DO, OPT_SGA]).await;
    peer.expect(&[IAC, WILL, OPT_TTYPE]).await;
    peer.expect(&[IAC, WILL, OPT_NAWS]).await;
    peer.expect(&[IAC, SB, OPT_NAWS, 0, 120, 0, 40, IAC, SE])
        .await;
    peer.expect(&[IAC, WONT, OPT_NEW_ENVIRON]).await;
    peer.expect(&[IAC, DONT, OPT_STATUS]).await;
    assert_eq!(conn.exactly(6).await, b"login:");

    // A sub-negotiation in four pieces.
    peer.send_in_pieces(&[&[IAC, SB], &[OPT_TTYPE], &[TTYPE_SEND, IAC], &[SE, b' ']])
        .await;
    let mut terminal_type = vec![IAC, SB, OPT_TTYPE, 0];
    terminal_type.extend_from_slice(b"xterm-256color");
    terminal_type.extend_from_slice(&[IAC, SE]);
    peer.expect(&terminal_type).await;

    // Repeating what is already agreed gets no answer at all (no loop): the
    // next thing the client says is the refusal of the new option.
    peer.send_in_pieces(&[
        &[IAC, WILL, OPT_ECHO, IAC, WILL, OPT_SGA, IAC, DO, OPT_NAWS],
        &[IAC, DO, OPT_TTYPE, IAC, DONT, OPT_STATUS, IAC, WONT, 77],
        &[IAC, DO, 99],
    ])
    .await;
    peer.expect(&[IAC, WONT, 99]).await;

    // Escapes and padding in the data stream, split too.
    peer.send_in_pieces(&[
        &[b'a', IAC],
        &[IAC, b'b', IAC, NOP, b'\r'],
        &[0, b'c', IAC, GA, b'\r', b'\n', IAC],
        &[BRK, b'd'],
    ])
    .await;
    assert_eq!(
        conn.exactly(8).await,
        [b' ', b'a', IAC, b'b', b'\r', b'c', b'\r', b'\n']
    );
    assert_eq!(conn.exactly(1).await, b"d");

    // Client to server: IAC doubled, a bare CR padded with NUL, CR LF kept.
    conn.send(vec![b'x', IAC, b'y', b'\r']).await.unwrap();
    peer.expect(&[b'x', IAC, IAC, b'y', b'\r', 0]).await;
    conn.send("line\r\n").await.unwrap();
    peer.expect(b"line\r\n").await;

    // A resize is reported because the server asked for the window size.
    conn.resize(80, 255).await.unwrap();
    peer.expect(&[IAC, SB, OPT_NAWS, 0, 80, 0, IAC, IAC, IAC, SE])
        .await;

    conn.send_break().await.unwrap();
    peer.expect(&[IAC, BRK]).await;

    // The server turns the window size reports off again.
    peer.send_in_pieces(&[&[IAC, DONT, OPT_NAWS]]).await;
    peer.expect(&[IAC, WONT, OPT_NAWS]).await;
    conn.resize(90, 20).await.unwrap();
    conn.send("z").await.unwrap();
    peer.expect(b"z").await;

    drop(peer);
    let (rest, reason) = conn.until_closed().await;
    assert!(rest.is_empty());
    assert_eq!(reason.as_deref(), Some("closed by remote host"));
}

#[tokio::test]
async fn window_size_is_only_sent_when_asked_and_is_current() {
    let params = TelnetParams {
        cols: 100,
        rows: 30,
        term: "vt100".to_owned(),
        ..TelnetParams::default()
    };
    let (mut peer, conn) = Peer::accept(params).await;

    // Nobody asked yet: a resize sends nothing, the data after it does.
    conn.resize(132, 43).await.unwrap();
    conn.send("a").await.unwrap();
    peer.expect(b"a").await;

    // Once asked, the size reported is the latest one.
    peer.send_in_pieces(&[&[IAC, DO, OPT_NAWS]]).await;
    peer.expect(&[IAC, WILL, OPT_NAWS]).await;
    peer.expect(&[IAC, SB, OPT_NAWS, 0, 132, 0, 43, IAC, SE])
        .await;

    // The terminal type is only given after the option was agreed on.
    peer.send_in_pieces(&[
        &[IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE],
        &[IAC, DO, OPT_TTYPE],
    ])
    .await;
    peer.expect(&[IAC, WILL, OPT_TTYPE]).await;
    peer.send_in_pieces(&[&[IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE]])
        .await;
    let mut terminal_type = vec![IAC, SB, OPT_TTYPE, 0];
    terminal_type.extend_from_slice(b"vt100");
    terminal_type.extend_from_slice(&[IAC, SE]);
    peer.expect(&terminal_type).await;
}

#[tokio::test]
async fn the_client_never_starts_a_negotiation() {
    let (mut peer, conn) = Peer::accept(TelnetParams::default()).await;
    // A raw TCP service sees nothing but the data.
    conn.send("GET / HTTP/1.0\r\n\r\n").await.unwrap();
    peer.expect(b"GET / HTTP/1.0\r\n\r\n").await;
}

#[tokio::test]
async fn connection_failures_are_classified() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = soon(connect_telnet(&params(address))).await.unwrap_err();
    // Refused where the host answers a connection to a port with nothing on
    // it, timed out where it drops it instead — Windows does the latter. The
    // mapping itself is asserted exactly, on a constructed error, in
    // `error.rs`; what matters here is that a connection that cannot be made
    // fails and says which, rather than hanging or being called something
    // else.
    assert!(matches!(error.code(), "refused" | "timeout"), "{error:?}");

    let params = TelnetParams {
        host: "no-such-host.invalid".to_owned(),
        ..TelnetParams::default()
    };
    let error = soon(connect_telnet(&params)).await.unwrap_err();
    assert_eq!(error.code(), "resolve");
}

#[tokio::test]
async fn a_reset_connection_is_reported() {
    let (peer, mut conn) = Peer::accept(TelnetParams::default()).await;
    // Closing with unread data pending makes the kernel send a reset.
    conn.send("unread").await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    drop(peer);
    let (_, reason) = conn.until_closed().await;
    let reason = reason.unwrap();
    assert!(
        reason.starts_with("connection lost") || reason == "closed by remote host",
        "{reason}"
    );
}
