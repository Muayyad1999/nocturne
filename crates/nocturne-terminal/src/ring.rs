//! The scrollback of a session: its most recent output, as raw bytes.

use std::collections::VecDeque;

/// A byte buffer that keeps the newest `capacity` bytes pushed into it.
pub(crate) struct Ring {
    bytes: VecDeque<u8>,
    capacity: usize,
    /// Older output has been pushed out, so the buffer may start mid-line.
    overflowed: bool,
}

impl Ring {
    pub fn new(capacity: usize) -> Ring {
        Ring {
            bytes: VecDeque::new(),
            capacity: capacity.max(1),
            overflowed: false,
        }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn push(&mut self, data: &[u8]) {
        if data.len() >= self.capacity {
            self.overflowed |= !self.bytes.is_empty() || data.len() > self.capacity;
            self.bytes.clear();
            self.grow_to(self.capacity);
            self.bytes.extend(&data[data.len() - self.capacity..]);
            return;
        }
        let excess = (self.bytes.len() + data.len()).saturating_sub(self.capacity);
        if excess > 0 {
            self.bytes.drain(..excess);
            self.overflowed = true;
        }
        self.grow_to(self.bytes.len() + data.len());
        self.bytes.extend(data);
    }

    /// Make room for `needed` bytes in total. Doubling as `VecDeque` does by
    /// itself would overshoot the capacity by up to a factor of two.
    fn grow_to(&mut self, needed: usize) {
        let allocated = self.bytes.capacity();
        if needed > allocated {
            let target = needed.max(allocated * 2).min(self.capacity);
            self.bytes.reserve_exact(target - self.bytes.len());
        }
    }

    /// The retained output without its newest `skip_tail` bytes.
    ///
    /// Once older output has been dropped the copy starts at the first line
    /// break, so that a terminal replaying it does not begin in the middle of
    /// an escape sequence or a multi-byte character.
    pub fn snapshot(&self, skip_tail: usize) -> Vec<u8> {
        let end = self.bytes.len().saturating_sub(skip_tail);
        let start = if self.overflowed {
            self.bytes
                .iter()
                .take(end)
                .position(|byte| *byte == b'\n')
                .map_or(0, |newline| newline + 1)
        } else {
            0
        };
        let (front, back) = self.bytes.as_slices();
        let mut copy = Vec::with_capacity(end - start);
        if start < front.len() {
            copy.extend_from_slice(&front[start..end.min(front.len())]);
        }
        if end > front.len() {
            copy.extend_from_slice(&back[start.saturating_sub(front.len())..end - front.len()]);
        }
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_everything_until_full() {
        let mut ring = Ring::new(16);
        ring.push(b"hello ");
        ring.push(b"world");
        assert_eq!(ring.snapshot(0), b"hello world");
        assert_eq!(ring.snapshot(5), b"hello ");
        assert_eq!(ring.snapshot(100), b"");
        assert_eq!(ring.len(), 11);
    }

    #[test]
    fn drops_the_oldest_bytes_and_resumes_at_a_line_start() {
        let mut ring = Ring::new(16);
        ring.push(b"line one\r\nline two\r\n");
        // 20 bytes into 16: "line" of the first line is gone.
        assert_eq!(ring.len(), 16);
        assert_eq!(ring.snapshot(0), b"line two\r\n");
        ring.push(b"3\r\n");
        assert_eq!(ring.snapshot(0), b"line two\r\n3\r\n");
        // Without any line break left there is nothing better than the raw tail.
        ring.push(b"0123456789abcdefXYZ");
        assert_eq!(ring.snapshot(0), b"3456789abcdefXYZ");
    }

    #[test]
    fn a_chunk_larger_than_the_buffer_keeps_its_tail() {
        let mut ring = Ring::new(4);
        ring.push(b"abcd");
        assert_eq!(ring.snapshot(0), b"abcd");
        ring.push(b"0123456789");
        assert_eq!(ring.snapshot(0), b"6789");
        assert_eq!(ring.snapshot(1), b"678");
    }

    #[test]
    fn wraps_around_without_losing_order_or_exceeding_its_capacity() {
        let capacity = 1000;
        let mut ring = Ring::new(capacity);
        let mut expected: Vec<u8> = Vec::new();
        for round in 0..500u32 {
            let chunk: Vec<u8> = (0..(round % 37 + 1))
                .map(|offset| b'a' + ((round + offset) % 26) as u8)
                .collect();
            ring.push(&chunk);
            expected.extend_from_slice(&chunk);
            assert!(ring.len() <= capacity);
            assert!(ring.bytes.capacity() <= capacity, "allocation overshoots");
        }
        assert_eq!(ring.len(), capacity);
        // No line break in the stream, so the snapshot is the exact tail.
        assert_eq!(ring.snapshot(0), expected[expected.len() - capacity..]);
        assert_eq!(
            ring.snapshot(10),
            expected[expected.len() - capacity..expected.len() - 10]
        );
    }
}
