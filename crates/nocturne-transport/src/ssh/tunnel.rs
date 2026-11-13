//! The byte stream through a jump host that the session with the next hop
//! runs over.
//!
//! russh's own stream over a channel would do, were it not for the way two
//! russh sessions stacked on each other can wait for one another: the inner
//! one stops reading the tunnel while a write into it is pending (its window
//! is used up), and the outer one stops handling window adjustments — the
//! very thing that write waits for — as soon as tunnel data nobody reads has
//! filled the channel's small queue. With several busy channels on the inner
//! session that is a deadlock.
//!
//! So the outer session's channel is always drained, by a task of its own,
//! into a [`Relay`]: the outer session stays live whatever the inner one is
//! doing. What piles up there is bounded by the inner session's own flow
//! control, since the far end can only send what the inner session, which
//! is not reading, has already made room for.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use bytes::{Bytes, BytesMut};
use russh::client::Msg;
use russh::{Channel, ChannelStream};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf, ReadHalf};
use tokio::sync::mpsc;

use crate::relay::Relay;

/// Room made for a read; a channel message carries up to 32 KiB.
const READ_BUFFER: usize = 64 * 1024;
/// Chunks queued for the inner session, on top of what is staged.
const QUEUE: usize = 16;
/// Tunnel data staged for an inner session that is not reading. Far above
/// what the windows of a handful of busy channels allow to be in flight;
/// beyond it the outer session is held up after all.
const HIGH_WATER: usize = 32 * 1024 * 1024;

/// Turns a `direct-tcpip` channel into the stream for the next session.
///
/// The channel is closed once that session has let go of the stream.
pub(crate) fn stream(channel: Channel<Msg>) -> impl AsyncRead + AsyncWrite + Send + Unpin {
    let (source, sink) = tokio::io::split(channel.into_stream());
    let (chunks, queued) = mpsc::channel(QUEUE);
    tokio::spawn(drain(source, chunks));
    let reader = Reader {
        queued,
        current: Bytes::new(),
    };
    tokio::io::join(reader, sink)
}

/// Keeps the channel drained until either end is gone.
async fn drain(mut source: ReadHalf<ChannelStream<Msg>>, chunks: mpsc::Sender<Bytes>) {
    let mut relay = Relay::new(chunks.clone(), std::convert::identity, HIGH_WATER);
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
                Ok(count) if count > 0 => relay.push_data(buffer.split().freeze()),
                // The far end is done: the reader gets the rest, then the
                // end of the stream.
                _ => {
                    let _ = relay.flush().await;
                    return;
                }
            }
        }
    }
}

/// The reading half: the chunks the draining task hands over, as a stream.
struct Reader {
    queued: mpsc::Receiver<Bytes>,
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
                Some(chunk) => self.current = chunk,
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
    use tokio::io::AsyncReadExt;

    use super::*;

    #[tokio::test]
    async fn the_reader_yields_the_chunks_as_one_stream_and_then_ends() {
        let (chunks, queued) = mpsc::channel(4);
        let mut reader = Reader {
            queued,
            current: Bytes::new(),
        };
        chunks.send(Bytes::from_static(b"hello ")).await.unwrap();
        chunks.send(Bytes::from_static(b"world")).await.unwrap();

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
}
