//! One TFTP transfer: the lock-step exchange of DATA and ACK packets on its own socket.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::UnboundedSender;

use super::TftpEvent;
use super::packet::{self, Packet, code};

/// The block size when the client negotiates none (RFC 1350).
pub(crate) const DEFAULT_BLOCK_SIZE: usize = 512;
/// The largest block that fits a UDP datagram (RFC 2348).
pub(crate) const MAX_BLOCK_SIZE: usize = 65_464;
/// How often the progress of a transfer is reported.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// What client and server agreed on for one transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Terms {
    pub block_size: usize,
    /// How long to wait for the peer before retransmitting.
    pub timeout: Duration,
    pub retries: u32,
    /// The options to acknowledge; empty when the client sent none we accept.
    pub acknowledged: Vec<(String, String)>,
}

/// Why a transfer ended early. The text is shown to the operator.
#[derive(Debug)]
pub(crate) struct Failure(pub String);

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure(match error.kind() {
            // ICMP port unreachable: the client's socket is gone.
            io::ErrorKind::ConnectionRefused => {
                "the client stopped listening (port unreachable)".to_owned()
            }
            _ => error.to_string(),
        })
    }
}

/// What a received packet means for the exchange in progress.
enum Step<T> {
    /// The awaited answer.
    Done(T),
    /// The peer did not get our last packet: send it again now.
    Repeat,
    /// Unrelated or stale: keep waiting.
    Ignore,
}

/// The transfer's socket (connected to the client) with its retransmission policy.
pub(crate) struct Link {
    pub socket: UdpSocket,
    pub terms: Terms,
    pub id: u64,
    pub events: UnboundedSender<TftpEvent>,
}

impl Link {
    /// Sends `packet` and waits for the answer `accept` recognises, retransmitting on timeout.
    ///
    /// `accept` sees every packet that arrives and decides what it means (see [`Step`]). An
    /// ERROR packet from the client ends the transfer.
    async fn exchange<T>(
        &self,
        packet: &[u8],
        buffer: &mut [u8],
        mut accept: impl FnMut(&Packet<'_>) -> Step<T>,
    ) -> Result<T, Failure> {
        for _attempt in 0..=self.terms.retries {
            self.socket.send(packet).await?;
            let deadline = tokio::time::Instant::now() + self.terms.timeout;
            loop {
                let received = tokio::time::timeout_at(deadline, self.socket.recv(buffer)).await;
                let length = match received {
                    Err(_) => break, // timed out: retransmit
                    Ok(result) => result?,
                };
                match packet::parse(&buffer[..length]) {
                    Ok(Packet::Error { code, message }) => {
                        return Err(Failure(format!(
                            "the client aborted the transfer: {message} (error {code})"
                        )));
                    }
                    Ok(parsed) => match accept(&parsed) {
                        Step::Done(value) => return Ok(value),
                        Step::Repeat => self.socket.send(packet).await.map(drop)?,
                        Step::Ignore => {}
                    },
                    Err(_) => {}
                }
            }
        }
        Err(Failure(format!(
            "the client stopped answering (no reply after {} attempts of {} ms)",
            self.terms.retries + 1,
            self.terms.timeout.as_millis()
        )))
    }

    /// Tells the client the transfer is over; best effort.
    async fn abort(&self, code: u16, message: &str) {
        let _ = self.socket.send(&packet::error(code, message)).await;
    }

    fn progress(&self, bytes: u64, last_report: &mut Instant) {
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            *last_report = Instant::now();
            let _ = self.events.send(TftpEvent::Progress { id: self.id, bytes });
        }
    }
}

/// Serves a read request: sends `file` block by block. Returns the bytes sent.
pub(crate) async fn send_file(link: &Link, mut file: File) -> Result<u64, Failure> {
    let block_size = link.terms.block_size;
    // Answers are ACK or ERROR packets: a few bytes.
    let mut reply = [0u8; 1024];
    if !link.terms.acknowledged.is_empty() {
        // The client confirms the negotiated options by acknowledging block 0.
        let oack = packet::option_ack(&link.terms.acknowledged);
        link.exchange(&oack, &mut reply, |packet| match packet {
            Packet::Ack { block: 0 } => Step::Done(()),
            _ => Step::Ignore,
        })
        .await?;
    }
    let mut data = packet::data_buffer(block_size);
    let mut sent: u64 = 0;
    let mut block: u64 = 0;
    let mut last_report = Instant::now();
    loop {
        block += 1;
        // Block numbers are 16 bits on the wire and simply wrap for large files.
        let wire_block = block as u16;
        let length = read_block(&mut file, &mut data[4..]).await?;
        packet::set_data_header(&mut data, wire_block);
        // Only the matching ACK moves the transfer on. A duplicate ACK of an earlier block is
        // ignored rather than answered with a retransmission (the "sorcerer's apprentice" fix).
        link.exchange(&data[..4 + length], &mut reply, |packet| match packet {
            Packet::Ack { block } if *block == wire_block => Step::Done(()),
            _ => Step::Ignore,
        })
        .await?;
        sent += length as u64;
        link.progress(sent, &mut last_report);
        if length < block_size {
            return Ok(sent);
        }
    }
}

/// Fills `buffer` from the file; a short count means the end of the file.
async fn read_block(file: &mut File, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]).await? {
            0 => break,
            count => filled += count,
        }
    }
    Ok(filled)
}

/// Serves a write request: receives blocks into a temporary file next to `target` and moves it
/// into place when the last block has arrived and been acknowledged.
pub(crate) async fn receive_file(
    link: &Link,
    target: &Path,
    max_bytes: u64,
) -> Result<Received, Failure> {
    let block_size = link.terms.block_size;
    let mut partial = PartialFile::create(target, link.id).await?;
    // One byte more than a full block, so that an oversized block is seen rather than cut.
    let mut buffer = vec![0u8; 4 + block_size + 1];

    // The answer to the request (OACK or ACK 0), then the ACK of each block, is what gets
    // retransmitted when the next DATA does not arrive.
    let mut answer = if link.terms.acknowledged.is_empty() {
        packet::ack(0).to_vec()
    } else {
        packet::option_ack(&link.terms.acknowledged)
    };
    let mut received: u64 = 0;
    let mut block: u64 = 0;
    let mut last_report = Instant::now();
    loop {
        block += 1;
        let wire_block = block as u16;
        // The client repeats a block when our acknowledgement of it got lost.
        let repeated_block = (block > 1).then(|| wire_block.wrapping_sub(1));
        let length = link
            .exchange(&answer, &mut buffer, |packet| match packet {
                Packet::Data { block, data } if *block == wire_block => Step::Done(data.len()),
                Packet::Data { block, .. } if Some(*block) == repeated_block => Step::Repeat,
                _ => Step::Ignore,
            })
            .await?;
        if length > block_size {
            link.abort(
                code::ILLEGAL_OPERATION,
                "data block larger than the negotiated block size",
            )
            .await;
            return Err(Failure(format!(
                "the client sent a {length}-byte block; the negotiated block size is {block_size}"
            )));
        }
        received += length as u64;
        if received > max_bytes {
            link.abort(code::DISK_FULL, "file larger than this server accepts")
                .await;
            return Err(Failure(format!(
                "the upload exceeds the server's limit of {max_bytes} bytes"
            )));
        }
        partial.file.write_all(&buffer[4..4 + length]).await?;
        link.progress(received, &mut last_report);
        answer = packet::ack(wire_block).to_vec();
        if length < block_size {
            break;
        }
    }
    partial.commit(target).await?;
    link.socket.send(&answer).await?;
    Ok(Received {
        bytes: received,
        final_ack: answer,
        final_block: block as u16,
    })
}

/// A completed upload, whose last acknowledgement may still need repeating.
pub(crate) struct Received {
    pub bytes: u64,
    final_ack: Vec<u8>,
    final_block: u16,
}

impl Received {
    /// Stays on the transfer's port for one timeout. If the final acknowledgement was lost the
    /// client repeats its last block; answering those repeats lets the client finish too.
    pub(crate) async fn linger(self, link: &Link) {
        let mut buffer = vec![0u8; 4 + link.terms.block_size + 1];
        let answer_repeats = async {
            while let Ok(length) = link.socket.recv(&mut buffer).await {
                let repeat = matches!(
                    packet::parse(&buffer[..length]),
                    Ok(Packet::Data { block, .. }) if block == self.final_block
                );
                if repeat && link.socket.send(&self.final_ack).await.is_err() {
                    return;
                }
            }
        };
        let _ = tokio::time::timeout(link.terms.timeout, answer_repeats).await;
    }
}

/// An upload in progress. The data goes to a hidden file beside the target, so that a failed
/// or aborted upload never leaves a truncated file under the requested name; dropping this
/// (on error, or when the transfer task is cancelled) removes the partial file.
struct PartialFile {
    file: File,
    path: PathBuf,
    committed: bool,
}

impl PartialFile {
    async fn create(target: &Path, id: u64) -> io::Result<Self> {
        let name = target.file_name().unwrap_or_default().to_string_lossy();
        let path = target.with_file_name(format!(".{name}.tftp-{id}.part"));
        // A leftover of a crashed server with the same transfer number is of no use to anyone.
        let _ = tokio::fs::remove_file(&path).await;
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        Ok(PartialFile {
            file,
            path,
            committed: false,
        })
    }

    /// Flushes the data and moves the file to its final name.
    async fn commit(&mut self, target: &Path) -> io::Result<()> {
        self.file.flush().await?;
        self.file.sync_all().await?;
        tokio::fs::rename(&self.path, target).await?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
