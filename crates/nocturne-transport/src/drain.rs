//! A stream that is read whether or not its owner is reading.
//!
//! russh runs a session as a single loop that does one thing at a time:
//! with something to send, it writes all of it before it reads again. Its
//! peer is thereby kept waiting to be read, and where the peer is such a
//! loop too, the two can end up waiting for each other:
//!
//! * Two sessions at the two ends of a connection — this crate's client and
//!   its server, or either of them and anything else built on russh — each
//!   with more to send than the connection will hold. Neither reads, which
//!   is the one thing that would let the other finish writing, and nothing
//!   notices: the keep-alives come from the same loop. How much a connection
//!   holds is up to the operating system. Linux lets loopback buffers grow
//!   to megabytes, and four shells echoing megabytes froze only once those
//!   were held to a few hundred kilobytes; the same test freezing on a
//!   Windows CI runner, buffers as they were, is most likely this.
//! * Two sessions stacked on each other (a jump host). The inner one stops
//!   reading the tunnel while a write into it is pending (its window is
//!   used up), and the outer one stops handling window adjustments — the
//!   very thing that write waits for — as soon as tunnel data nobody reads
//!   has filled the channel's small queue.
//!
//! So what arrives for a session is always taken off its transport, by a
//! task of its own, into a [`Relay`]. The far end's writes then complete
//! whatever the session is busy with, so the far end gets back to reading,
//! and that is what the session's own writes are waiting for. What piles up
//! is bounded by the session's flow control: the far end can only send what
//! the session, which is not reading, has already made room for.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use crate::relay::Relay;

/// Room made for a read; an SSH packet carries up to 32 KiB.
const READ_BUFFER: usize = 64 * 1024;
/// Chunks queued for the reader, on top of what is staged.
const QUEUE: usize = 16;

/// What the draining task hands to the reader: data, or the error that
/// ended the stream.
type Chunk = io::Result<Bytes>;

/// Wraps `stream` so that what arrives on it is read as it comes and kept
/// for whoever reads the result. Writes go straight through.
///
/// Up to `high_water` bytes are kept; a reader that falls further behind
/// leaves the stream unread, as it would have without this. Dropping the
/// result lets go of the stream.
pub(crate) fn always_read<S>(
    stream: S,
    high_water: usize,
) -> impl AsyncRead + AsyncWrite + Send + Unpin
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (source, sink) = tokio::io::split(stream);
    let (chunks, queued) = mpsc::channel(QUEUE);
    tokio::spawn(drain(source, chunks, high_water));
    let reader = Reader {
        queued,
        current: Bytes::new(),
    };
    tokio::io::join(reader, sink)
}

/// Keeps `source` drained until it ends or the reader is gone.
async fn drain<R>(mut source: R, chunks: mpsc::Sender<Chunk>, high_water: usize)
where
    R: AsyncRead + Unpin,
{
    let mut relay: Relay<Chunk> = Relay::new(chunks.clone(), Ok, high_water);
    let mut buffer = BytesMut::new();
    loop {
        if buffer.capacity() < READ_BUFFER / 2 {
            buffer.reserve(READ_BUFFER);
        }
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !relay.is_empty() => {
                if forwarded.is_err() {
                    return;
                }
            }
            () = chunks.closed() => return,
            read = source.read_buf(&mut buffer), if relay.has_room() => match read {
                Ok(0) => break,
                Ok(_) => relay.push_data(buffer.split().freeze()),
                Err(error) => {
                    relay.push_item(Err(error));
                    break;
                }
            }
        }
    }
    // The far end is done: the reader gets the rest, then how it ended.
    let _ = relay.flush().await;
}

/// The reading half: the chunks the draining task hands over, as a stream.
struct Reader {
    queued: mpsc::Receiver<Chunk>,
    /// What is left of the chunk being read.
    current: Bytes,
}

impl AsyncRead for Reader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.current.is_empty() {
            match ready!(self.queued.poll_recv(cx)) {
                Some(Ok(chunk)) => self.current = chunk,
                Some(Err(error)) => return Poll::Ready(Err(error)),
                None => return Poll::Ready(Ok(())), // end of the stream
            }
        }
        let count = buffer.remaining().min(self.current.len());
        buffer.put_slice(&self.current.split_to(count));
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Long enough for a task that could run to have run.
    const SETTLE: Duration = Duration::from_millis(200);

    #[tokio::test]
    async fn the_reader_yields_the_chunks_as_one_stream_and_then_ends() {
        let (chunks, queued) = mpsc::channel(4);
        let mut reader = Reader {
            queued,
            current: Bytes::new(),
        };
        chunks
            .send(Ok(Bytes::from_static(b"hello ")))
            .await
            .unwrap();
        chunks.send(Ok(Bytes::from_static(b"world"))).await.unwrap();

        // A buffer smaller than a chunk takes it in pieces.
        let mut small = [0u8; 4];
        assert_eq!(reader.read(&mut small).await.unwrap(), 4);
        assert_eq!(&small, b"hell");
        drop(chunks);
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest, b"o world");
        assert_eq!(reader.read(&mut small).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_writer_is_not_kept_waiting_by_an_owner_that_does_not_read() {
        // The pipe holds 1 KiB; the far end has 300 times that to say.
        let (near, mut far) = tokio::io::duplex(1024);
        let mut near = always_read(near, 1024 * 1024);
        let sent: Vec<u8> = (0..300 * 1024).map(|index| (index % 251) as u8).collect();

        let writing = far.write_all(&sent);
        tokio::time::timeout(SETTLE * 10, writing)
            .await
            .expect("the write should not wait for the owner to read")
            .unwrap();

        // Nothing was lost or reordered on the way.
        drop(far);
        let mut received = Vec::new();
        near.read_to_end(&mut received).await.unwrap();
        assert!(received == sent, "{} bytes arrived", received.len());
    }

    #[tokio::test]
    async fn an_owner_far_behind_leaves_the_stream_unread_and_catches_up_later() {
        const HIGH_WATER: usize = 64 * 1024;
        let (near, mut far) = tokio::io::duplex(1024);
        let mut near = always_read(near, HIGH_WATER);
        let sent: Vec<u8> = (0..1024 * 1024).map(|index| (index % 251) as u8).collect();

        let payload = sent.clone();
        let writer = tokio::spawn(async move {
            far.write_all(&payload).await.unwrap();
            far
        });
        tokio::time::sleep(SETTLE).await;
        assert!(
            !writer.is_finished(),
            "a megabyte was taken for an owner that reads nothing"
        );

        let mut received = vec![0; sent.len()];
        near.read_exact(&mut received).await.unwrap();
        assert!(received == sent);
        drop(writer.await.unwrap());
        assert_eq!(near.read(&mut received).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn what_is_written_goes_straight_through() {
        let (near, mut far) = tokio::io::duplex(1024);
        let mut near = always_read(near, 1024);
        near.write_all(b"ping").await.unwrap();
        let mut seen = [0u8; 4];
        far.read_exact(&mut seen).await.unwrap();
        assert_eq!(&seen, b"ping");

        // Closing the sending side reaches the far end as the end of input.
        near.shutdown().await.unwrap();
        assert_eq!(far.read(&mut seen).await.unwrap(), 0);
    }

    /// A stream that yields a few bytes and then fails.
    struct Failing {
        said: bool,
    }

    impl AsyncRead for Failing {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            if self.said {
                return Poll::Ready(Err(io::ErrorKind::ConnectionReset.into()));
            }
            self.said = true;
            buffer.put_slice(b"last words");
            Poll::Ready(Ok(()))
        }
    }

    impl AsyncWrite for Failing {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_read_error_follows_the_data_that_came_before_it() {
        let mut stream = always_read(Failing { said: false }, 1024);
        let mut seen = [0u8; 10];
        stream.read_exact(&mut seen).await.unwrap();
        assert_eq!(&seen, b"last words");
        let error = stream.read(&mut seen).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    }

    #[tokio::test]
    async fn dropping_the_stream_lets_go_of_what_it_wraps() {
        let (near, mut far) = tokio::io::duplex(1024);
        let near = always_read(near, 1024);
        drop(near);
        // Both halves are gone once the draining task has noticed.
        let mut seen = [0u8; 1];
        let end = tokio::time::timeout(SETTLE * 10, far.read(&mut seen)).await;
        assert_eq!(end.expect("the stream should have been closed").unwrap(), 0);
    }
}
