//! Small shared helpers.

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// A random 16-character identifier (80 bits), safe in URLs and file names.
pub fn new_id() -> String {
    let mut bytes = [0u8; 10];
    random_bytes(&mut bytes);
    // 10 bytes = 80 bits = 16 base32 characters exactly.
    let mut id = String::with_capacity(16);
    let mut buffer = 0u32;
    let mut bits = 0;
    for byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            id.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    id
}

/// Fill `buffer` from the operating system's CSPRNG.
pub fn random_bytes(buffer: &mut [u8]) {
    // The OS generator failing is unrecoverable for a process that encrypts secrets.
    getrandom::fill(buffer).expect("the operating system random number generator is unavailable");
}

/// Case-insensitive "does `haystack` contain every whitespace-separated word of `query`".
pub fn matches_query(haystack: &str, query: &str) -> bool {
    let haystack = haystack.to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_well_formed_and_distinct() {
        let a = new_id();
        let b = new_id();
        assert_eq!(a.len(), 16);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        assert_ne!(a, b);
    }

    #[test]
    fn query_matching() {
        assert!(matches_query("CORE-SW1 Cisco 10.0.0.1 hq", "core cisco"));
        assert!(matches_query("anything", ""));
        assert!(!matches_query("CORE-SW1", "edge"));
    }
}
