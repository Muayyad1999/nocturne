//! A minimal TFTP client for the server tests. It has its own packet code on purpose: the
//! server is checked against an independent reading of the RFCs, not against itself.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::UdpSocket;

pub const RRQ: u16 = 1;
pub const WRQ: u16 = 2;
pub const DATA: u16 = 3;
pub const ACK: u16 = 4;
pub const ERROR: u16 = 5;
pub const OACK: u16 = 6;

/// How long the client waits for the server before giving up.
const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default)]
pub struct ClientOptions {
    pub blksize: Option<usize>,
    /// Ask for (download) or announce (upload) the transfer size.
    pub tsize: bool,
    pub timeout: Option<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ClientError {
    /// The server answered with an ERROR packet.
    Server {
        code: u16,
        message: String,
    },
    Timeout,
    Protocol(String),
}

/// A packet as the client understands it.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    Data { block: u16, data: Vec<u8> },
    Ack { block: u16 },
    Error { code: u16, message: String },
    OptionAck(Vec<(String, String)>),
}

pub fn request(opcode: u16, name: &str, mode: &str, options: &[(&str, String)]) -> Vec<u8> {
    let mut packet = opcode.to_be_bytes().to_vec();
    packet.extend_from_slice(name.as_bytes());
    packet.push(0);
    packet.extend_from_slice(mode.as_bytes());
    packet.push(0);
    for (option, value) in options {
        packet.extend_from_slice(option.as_bytes());
        packet.push(0);
        packet.extend_from_slice(value.as_bytes());
        packet.push(0);
    }
    packet
}

pub fn data(block: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = DATA.to_be_bytes().to_vec();
    packet.extend_from_slice(&block.to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

pub fn ack(block: u16) -> Vec<u8> {
    let mut packet = ACK.to_be_bytes().to_vec();
    packet.extend_from_slice(&block.to_be_bytes());
    packet
}

pub fn decode(datagram: &[u8]) -> Result<Received, ClientError> {
    let malformed = || ClientError::Protocol(format!("malformed packet: {datagram:02x?}"));
    if datagram.len() < 4 {
        return Err(malformed());
    }
    let opcode = u16::from_be_bytes([datagram[0], datagram[1]]);
    let number = u16::from_be_bytes([datagram[2], datagram[3]]);
    match opcode {
        DATA => Ok(Received::Data {
            block: number,
            data: datagram[4..].to_vec(),
        }),
        ACK if datagram.len() == 4 => Ok(Received::Ack { block: number }),
        ERROR => {
            let text = datagram[4..].strip_suffix(&[0]).ok_or_else(malformed)?;
            Ok(Received::Error {
                code: number,
                message: String::from_utf8_lossy(text).into_owned(),
            })
        }
        OACK => {
            let body = datagram[2..].strip_suffix(&[0]).ok_or_else(malformed)?;
            let fields: Vec<String> = body
                .split(|b| *b == 0)
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .collect();
            if fields.len() % 2 != 0 {
                return Err(malformed());
            }
            Ok(Received::OptionAck(
                fields
                    .chunks(2)
                    .map(|pair| (pair[0].clone(), pair[1].clone()))
                    .collect(),
            ))
        }
        _ => Err(malformed()),
    }
}

/// A client socket and the transfer address of the server once it is known.
pub struct Session {
    pub socket: UdpSocket,
    pub server: SocketAddr,
    /// The address the server's transfer socket answers from (its transfer ID).
    pub peer: Option<SocketAddr>,
}

impl Session {
    pub async fn open(server: SocketAddr) -> Session {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        Session {
            socket,
            server,
            peer: None,
        }
    }

    /// Sends to the transfer address, or to the request port before the server has answered.
    pub async fn send(&self, packet: &[u8]) {
        self.socket
            .send_to(packet, self.peer.unwrap_or(self.server))
            .await
            .unwrap();
    }

    /// The next packet from the server within `patience`; `None` when nothing arrives.
    pub async fn receive_within(
        &mut self,
        patience: Duration,
    ) -> Option<Result<Received, ClientError>> {
        let mut buffer = vec![0u8; 65_536];
        loop {
            let (length, from) = tokio::time::timeout(patience, self.socket.recv_from(&mut buffer))
                .await
                .ok()?
                .unwrap();
            match self.peer {
                None => self.peer = Some(from),
                // RFC 1350: packets from another transfer ID are not part of this transfer.
                Some(peer) if peer != from => continue,
                Some(_) => {}
            }
            return Some(decode(&buffer[..length]));
        }
    }

    pub async fn receive(&mut self) -> Result<Received, ClientError> {
        match self.receive_within(PATIENCE).await {
            Some(Ok(Received::Error { code, message })) => {
                Err(ClientError::Server { code, message })
            }
            Some(result) => result,
            None => Err(ClientError::Timeout),
        }
    }
}

fn option_list(options: &ClientOptions, tsize: Option<u64>) -> Vec<(&'static str, String)> {
    let mut list = Vec::new();
    if let Some(blksize) = options.blksize {
        list.push(("blksize", blksize.to_string()));
    }
    if let Some(timeout) = options.timeout {
        list.push(("timeout", timeout.to_string()));
    }
    if let (true, Some(size)) = (options.tsize, tsize) {
        list.push(("tsize", size.to_string()));
    }
    list
}

fn block_size(acknowledged: &[(String, String)]) -> usize {
    acknowledged
        .iter()
        .find(|(name, _)| name == "blksize")
        .map_or(512, |(_, value)| value.parse().unwrap())
}

#[derive(Debug)]
pub struct Download {
    pub data: Vec<u8>,
    /// The options the server acknowledged.
    pub acknowledged: Vec<(String, String)>,
    pub blocks: u64,
}

pub async fn get(
    server: SocketAddr,
    name: &str,
    options: &ClientOptions,
) -> Result<Download, ClientError> {
    let mut session = Session::open(server).await;
    session
        .send(&request(RRQ, name, "octet", &option_list(options, Some(0))))
        .await;
    let mut acknowledged = Vec::new();
    let mut first = session.receive().await?;
    if let Received::OptionAck(granted) = first {
        acknowledged = granted;
        session.send(&ack(0)).await;
        first = session.receive().await?;
    }
    let block_size = block_size(&acknowledged);
    let mut download = Download {
        data: Vec::new(),
        acknowledged,
        blocks: 0,
    };
    let mut next = first;
    loop {
        let expected = (download.blocks + 1) as u16;
        match next {
            Received::Data { block, data } if block == expected => {
                let last = data.len() < block_size;
                if data.len() > block_size {
                    return Err(ClientError::Protocol(format!(
                        "block of {} bytes exceeds {block_size}",
                        data.len()
                    )));
                }
                download.data.extend_from_slice(&data);
                download.blocks += 1;
                session.send(&ack(block)).await;
                if last {
                    return Ok(download);
                }
            }
            // A retransmission of the block before: acknowledge it again.
            Received::Data { block, .. } if block == expected.wrapping_sub(1) => {
                session.send(&ack(block)).await
            }
            other => {
                return Err(ClientError::Protocol(format!(
                    "expected DATA {expected}, got {other:?}"
                )));
            }
        }
        next = session.receive().await?;
    }
}

#[derive(Debug)]
pub struct Upload {
    pub acknowledged: Vec<(String, String)>,
    pub blocks: u64,
}

pub async fn put(
    server: SocketAddr,
    name: &str,
    content: &[u8],
    options: &ClientOptions,
) -> Result<Upload, ClientError> {
    let mut session = Session::open(server).await;
    session
        .send(&request(
            WRQ,
            name,
            "octet",
            &option_list(options, Some(content.len() as u64)),
        ))
        .await;
    let acknowledged = match session.receive().await? {
        Received::Ack { block: 0 } => Vec::new(),
        Received::OptionAck(granted) => granted,
        other => {
            return Err(ClientError::Protocol(format!(
                "expected ACK 0 or OACK, got {other:?}"
            )));
        }
    };
    let block_size = block_size(&acknowledged);
    let mut blocks: u64 = 0;
    let mut offset = 0;
    loop {
        blocks += 1;
        let end = (offset + block_size).min(content.len());
        let chunk = &content[offset..end];
        session.send(&data(blocks as u16, chunk)).await;
        loop {
            match session.receive().await? {
                Received::Ack { block } if block == blocks as u16 => break,
                Received::Ack { .. } => continue,
                other => {
                    return Err(ClientError::Protocol(format!(
                        "expected ACK {blocks}, got {other:?}"
                    )));
                }
            }
        }
        offset = end;
        if chunk.len() < block_size {
            return Ok(Upload {
                acknowledged,
                blocks,
            });
        }
    }
}
