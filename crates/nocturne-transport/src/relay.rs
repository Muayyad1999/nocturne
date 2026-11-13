//! Ordered hand-off between a producer that must not be stalled and a
//! bounded channel.
//!
//! russh delivers channel data from *inside* its session loop and re-opens the
//! SSH window on its own, so a channel reader that stops reading freezes the
//! whole session: keep-alives, sibling channels, jump hops. A [`Relay`] lets a
//! reader keep draining: what the consumer has not taken yet is staged here,
//! small chunks are merged while they wait, and only once the staging area
//! holds more than its high-water mark does the producer get told to stop
//! ([`Relay::has_room`]), which turns into ordinary TCP back-pressure. Nothing
//! is ever dropped.

use std::collections::VecDeque;

use bytes::{Bytes, BytesMut};
use tokio::sync::mpsc;

use crate::connection::{Input, Output};

/// Largest chunk produced by merging staged data.
const COALESCE_LIMIT: usize = 64 * 1024;
/// What a staged item that is not data counts for against the high-water
/// mark, so that a flood of them cannot grow the staging area without bound.
const ITEM_WEIGHT: usize = 64;

/// The consuming side of the channel is gone; nobody will read what is staged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReceiverGone;

/// What waits for room in a channel, in order. Usable on its own while the
/// channel it is meant for does not exist yet.
pub(crate) struct Staging<T> {
    queue: VecDeque<Staged<T>>,
    /// The staged bytes, plus [`ITEM_WEIGHT`] for every other item.
    weight: usize,
    wrap: fn(Bytes) -> T,
    high_water: usize,
}

enum Staged<T> {
    Data(Bytes),
    Item(T),
}

impl<T> Staging<T> {
    /// `wrap` turns a chunk of bytes into a channel item; `high_water` is the
    /// weight above which [`has_room`](Self::has_room) turns false.
    pub fn new(wrap: fn(Bytes) -> T, high_water: usize) -> Self {
        Self {
            queue: VecDeque::new(),
            weight: 0,
            wrap,
            high_water,
        }
    }

    /// Stage a chunk of data behind everything already staged.
    pub fn push_data(&mut self, data: Bytes) {
        if data.is_empty() {
            return;
        }
        self.weight += data.len();
        self.queue.push_back(Staged::Data(data));
    }

    /// Stage a non-data item; it keeps its place relative to the data.
    pub fn push_item(&mut self, item: T) {
        self.weight += ITEM_WEIGHT;
        self.queue.push_back(Staged::Item(item));
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The staged bytes, plus a nominal weight for every other item.
    pub fn weight(&self) -> usize {
        self.weight
    }

    /// Whether the producer may keep reading.
    pub fn has_room(&self) -> bool {
        self.weight < self.high_water
    }

    /// Drops everything that is staged.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.weight = 0;
    }

    /// Takes the oldest element, merging the data chunks that follow a chunk
    /// into it for as long as they fit.
    fn pop(&mut self) -> Option<T> {
        let first = match self.queue.pop_front()? {
            Staged::Item(item) => {
                self.weight -= ITEM_WEIGHT;
                return Some(item);
            }
            Staged::Data(first) => first,
        };
        self.weight -= first.len();
        if !self.next_fits(first.len()) {
            return Some((self.wrap)(first));
        }
        let mut merged = BytesMut::with_capacity(COALESCE_LIMIT.min(first.len() + self.weight));
        merged.extend_from_slice(&first);
        while self.next_fits(merged.len()) {
            if let Some(Staged::Data(next)) = self.queue.pop_front() {
                self.weight -= next.len();
                merged.extend_from_slice(&next);
            }
        }
        Some((self.wrap)(merged.freeze()))
    }

    /// Whether the next staged element is data that can be merged into a
    /// chunk of `len` bytes.
    fn next_fits(&self, len: usize) -> bool {
        matches!(self.queue.front(), Some(Staged::Data(next)) if len + next.len() <= COALESCE_LIMIT)
    }
}

impl Staging<Input> {
    /// A staging area for what a client sends to a session.
    pub fn for_input(high_water: usize) -> Self {
        Self::new(Input::Data, high_water)
    }

    pub fn push(&mut self, input: Input) {
        match input {
            Input::Data(data) => self.push_data(data),
            other => self.push_item(other),
        }
    }
}

/// A [`Staging`] area in front of a bounded `mpsc::Sender<T>`.
pub(crate) struct Relay<T> {
    tx: mpsc::Sender<T>,
    staging: Staging<T>,
}

impl<T> Relay<T> {
    /// See [`Staging::new`].
    pub fn new(tx: mpsc::Sender<T>, wrap: fn(Bytes) -> T, high_water: usize) -> Self {
        Self::with_staged(tx, Staging::new(wrap, high_water))
    }

    /// A relay that starts out with what was staged before `tx` existed.
    pub fn with_staged(tx: mpsc::Sender<T>, staging: Staging<T>) -> Self {
        Self { tx, staging }
    }

    /// See [`Staging::push_data`].
    pub fn push_data(&mut self, data: Bytes) {
        self.staging.push_data(data);
    }

    /// See [`Staging::push_item`].
    pub fn push_item(&mut self, item: T) {
        self.staging.push_item(item);
    }

    pub fn is_empty(&self) -> bool {
        self.staging.is_empty()
    }

    /// See [`Staging::weight`].
    pub fn weight(&self) -> usize {
        self.staging.weight()
    }

    /// See [`Staging::has_room`].
    pub fn has_room(&self) -> bool {
        self.staging.has_room()
    }

    /// Drops everything that is staged: the consumer will not take it.
    pub fn clear(&mut self) {
        self.staging.clear();
    }

    /// Hands the oldest staged element to the channel, waiting for capacity.
    /// Cancel-safe: dropping the future loses nothing.
    pub async fn forward_one(&mut self) -> Result<(), ReceiverGone> {
        let permit = self.tx.reserve().await.map_err(|_| ReceiverGone)?;
        if let Some(item) = self.staging.pop() {
            permit.send(item);
        }
        Ok(())
    }

    /// Forwards everything that is staged.
    pub async fn flush(&mut self) -> Result<(), ReceiverGone> {
        while !self.is_empty() {
            self.forward_one().await?;
        }
        Ok(())
    }
}

impl Relay<Input> {
    pub fn push(&mut self, input: Input) {
        self.staging.push(input);
    }
}

impl Relay<Output> {
    /// A relay feeding a connection's output channel.
    pub fn for_output(tx: mpsc::Sender<Output>, high_water: usize) -> Self {
        Self::new(tx, Output::Data, high_water)
    }

    /// Delivers what is staged, then the final `Closed`, then hangs up.
    pub async fn finish(mut self, reason: Option<String>) {
        self.push_item(Output::Closed { reason });
        // An error only means the application stopped listening.
        let _ = self.flush().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(output: Option<Output>) -> Bytes {
        match output {
            Some(Output::Data(data)) => data,
            other => panic!("expected data, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_single_chunk_is_forwarded_untouched() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut relay = Relay::for_output(tx, 1024);
        let chunk = Bytes::from_static(b"hello");
        relay.push_data(chunk.clone());
        relay.forward_one().await.unwrap();
        let received = data(rx.recv().await);
        assert_eq!(received, chunk);
        assert_eq!(
            received.as_ptr(),
            chunk.as_ptr(),
            "no copy on the fast path"
        );
        assert!(relay.is_empty());
    }

    #[tokio::test]
    async fn waiting_chunks_are_merged_in_order() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut relay = Relay::for_output(tx, 1024);
        for part in [&b"one "[..], b"two ", b"three"] {
            relay.push_data(Bytes::copy_from_slice(part));
        }
        relay.push_data(Bytes::new());
        relay.flush().await.unwrap();
        assert_eq!(&data(rx.recv().await)[..], b"one two three");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn merging_respects_the_chunk_limit_and_items() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut relay = Relay::for_output(tx, usize::MAX);
        relay.push_data(Bytes::from(vec![1u8; COALESCE_LIMIT - 10]));
        relay.push_data(Bytes::from(vec![2u8; 10]));
        relay.push_data(Bytes::from(vec![3u8; 5]));
        relay.push_item(Output::Closed { reason: None });
        relay.push_data(Bytes::from(vec![4u8; 5]));
        relay.flush().await.unwrap();

        assert_eq!(data(rx.recv().await).len(), COALESCE_LIMIT);
        assert_eq!(&data(rx.recv().await)[..], &[3u8; 5]);
        assert!(matches!(rx.recv().await, Some(Output::Closed { .. })));
        assert_eq!(&data(rx.recv().await)[..], &[4u8; 5]);
    }

    #[tokio::test]
    async fn high_water_mark_tracks_staged_bytes() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut relay = Relay::for_output(tx, 8);
        assert!(relay.has_room());
        relay.push_data(Bytes::from_static(b"12345"));
        assert!(relay.has_room());
        relay.push_data(Bytes::from_static(b"678"));
        assert!(!relay.has_room());
        relay.forward_one().await.unwrap();
        assert!(relay.has_room());
        assert_eq!(&data(rx.recv().await)[..], b"12345678");
    }

    #[tokio::test]
    async fn forwarding_waits_for_capacity_and_is_cancel_safe() {
        let (tx, mut rx) = mpsc::channel(1);
        let mut relay = Relay::for_output(tx, 1024);
        relay.push_data(Bytes::from_static(b"a"));
        relay.forward_one().await.unwrap();
        relay.push_data(Bytes::from_static(b"b"));
        // The channel is full: forwarding must wait, and giving up keeps "b" staged.
        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(30), relay.forward_one()).await;
        assert!(waited.is_err());
        assert!(!relay.is_empty());
        assert_eq!(&data(rx.recv().await)[..], b"a");
        relay.forward_one().await.unwrap();
        assert_eq!(&data(rx.recv().await)[..], b"b");
    }

    #[tokio::test]
    async fn a_dropped_receiver_is_reported() {
        let (tx, rx) = mpsc::channel(1);
        let mut relay = Relay::for_output(tx, 1024);
        relay.push_data(Bytes::from_static(b"a"));
        drop(rx);
        assert_eq!(relay.forward_one().await, Err(ReceiverGone));
        assert_eq!(relay.flush().await, Err(ReceiverGone));
    }

    #[tokio::test]
    async fn finish_delivers_the_tail_then_closed() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut relay = Relay::for_output(tx, 1024);
        relay.push_data(Bytes::from_static(b"tail"));
        relay.finish(Some("exit status 0".into())).await;
        assert_eq!(&data(rx.recv().await)[..], b"tail");
        assert!(
            matches!(rx.recv().await, Some(Output::Closed { reason: Some(r) }) if r == "exit status 0")
        );
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn items_weigh_against_the_high_water_mark() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut relay = Relay::new(tx, Input::Data, 3 * ITEM_WEIGHT);
        relay.push(Input::Break);
        relay.push(Input::Resize { cols: 80, rows: 24 });
        assert!(relay.has_room());
        relay.push(Input::Break);
        assert!(!relay.has_room(), "a flood of items must hit the mark too");
        assert_eq!(relay.weight(), 3 * ITEM_WEIGHT);
        relay.forward_one().await.unwrap();
        assert!(relay.has_room());
        relay.flush().await.unwrap();
        assert_eq!(relay.weight(), 0);
        assert!(matches!(rx.recv().await, Some(Input::Break)));
        assert!(matches!(
            rx.recv().await,
            Some(Input::Resize { cols: 80, rows: 24 })
        ));
        assert!(matches!(rx.recv().await, Some(Input::Break)));
    }

    #[tokio::test]
    async fn clearing_drops_what_is_staged() {
        let (tx, mut rx) = mpsc::channel(4);
        let mut relay = Relay::new(tx, Input::Data, 4);
        relay.push(Input::Data(Bytes::from_static(b"nobody listens")));
        relay.push(Input::Close);
        assert!(!relay.has_room());
        relay.clear();
        assert!(relay.is_empty() && relay.has_room());
        assert_eq!(relay.weight(), 0);
        relay.flush().await.unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn what_was_staged_early_goes_out_first() {
        let mut early = Staging::for_input(1024);
        early.push(Input::Data(Bytes::from_static(b"typed ")));
        early.push(Input::Data(Bytes::from_static(b"ahead")));
        early.push(Input::Close);
        assert!(!early.is_empty() && early.has_room());

        let (tx, mut rx) = mpsc::channel(4);
        let mut relay = Relay::with_staged(tx, early);
        relay.push(Input::Data(Bytes::from_static(b"later")));
        relay.flush().await.unwrap();
        assert!(matches!(rx.recv().await, Some(Input::Data(data)) if &data[..] == b"typed ahead"));
        assert!(matches!(rx.recv().await, Some(Input::Close)));
        assert!(matches!(rx.recv().await, Some(Input::Data(data)) if &data[..] == b"later"));
    }
}
