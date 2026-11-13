//! Password storage as network operating systems do it: salted MD5-crypt
//! ("type 5", `$1$salt$hash`) and the reversible "type 7" obfuscation.

const K: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// RFC 1321.
pub(crate) fn md5(data: &[u8]) -> [u8; 16] {
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&(data.len() as u64).wrapping_mul(8).to_le_bytes());

    for block in msg.chunks_exact(64) {
        let mut words = [0u32; 16];
        for (word, bytes) in words.iter_mut().zip(block.chunks_exact(4)) {
            *word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(words[g])
                .rotate_left(S[i]);
            (a, d, c, b) = (d, c, b, b.wrapping_add(rotated));
        }
        state = [
            state[0].wrapping_add(a),
            state[1].wrapping_add(b),
            state[2].wrapping_add(c),
            state[3].wrapping_add(d),
        ];
    }
    let mut out = [0u8; 16];
    for (chunk, word) in out.chunks_exact_mut(4).zip(state) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    out
}

const ITOA64: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The salt alphabet, exposed so callers can draw a salt from the lab PRNG.
pub(crate) fn salt_char(n: u64) -> char {
    ITOA64[(n % 64) as usize] as char
}

/// MD5-crypt (`$1$`), as used for "secret 5" passwords.
pub(crate) fn md5_crypt(password: &str, salt: &str) -> String {
    let pw = password.as_bytes();
    let salt_bytes: Vec<u8> = salt.bytes().take(8).collect();

    let mut alt_input = pw.to_vec();
    alt_input.extend_from_slice(&salt_bytes);
    alt_input.extend_from_slice(pw);
    let alt = md5(&alt_input);

    let mut ctx = pw.to_vec();
    ctx.extend_from_slice(b"$1$");
    ctx.extend_from_slice(&salt_bytes);
    let mut remaining = pw.len();
    while remaining > 0 {
        let take = remaining.min(16);
        ctx.extend_from_slice(&alt[..take]);
        remaining -= take;
    }
    let mut bits = pw.len();
    while bits != 0 {
        ctx.push(if bits & 1 == 1 {
            0
        } else {
            pw.first().copied().unwrap_or(0)
        });
        bits >>= 1;
    }
    let mut digest = md5(&ctx);

    for round in 0..1000 {
        let mut input = Vec::with_capacity(64);
        if round & 1 == 1 {
            input.extend_from_slice(pw);
        } else {
            input.extend_from_slice(&digest);
        }
        if round % 3 != 0 {
            input.extend_from_slice(&salt_bytes);
        }
        if round % 7 != 0 {
            input.extend_from_slice(pw);
        }
        if round & 1 == 1 {
            input.extend_from_slice(&digest);
        } else {
            input.extend_from_slice(pw);
        }
        digest = md5(&input);
    }

    let mut out = format!("$1${}$", String::from_utf8_lossy(&salt_bytes));
    let mut push = |value: u32, chars: usize| {
        let mut v = value;
        for _ in 0..chars {
            out.push(ITOA64[(v & 0x3f) as usize] as char);
            v >>= 6;
        }
    };
    let triple = |a: usize, b: usize, c: usize| {
        (u32::from(digest[a]) << 16) | (u32::from(digest[b]) << 8) | u32::from(digest[c])
    };
    push(triple(0, 6, 12), 4);
    push(triple(1, 7, 13), 4);
    push(triple(2, 8, 14), 4);
    push(triple(3, 9, 15), 4);
    push(triple(4, 10, 5), 4);
    push(u32::from(digest[11]), 2);
    out
}

/// Does `password` hash to the stored `$1$salt$hash` string?
pub(crate) fn md5_crypt_verify(password: &str, stored: &str) -> bool {
    let Some(rest) = stored.strip_prefix("$1$") else {
        return false;
    };
    let Some((salt, _)) = rest.split_once('$') else {
        return false;
    };
    md5_crypt(password, salt) == stored
}

const TYPE7_KEY: &[u8] = b"dsfd;kfoA,.iyewrkldJKDHSUBsgvca69834ncxv9873254k;fg87";

/// "Type 7" obfuscation; `seed` (0..=15) selects the starting key offset.
pub(crate) fn type7_encode(password: &str, seed: u8) -> String {
    let seed = usize::from(seed % 16);
    let mut out = format!("{seed:02}");
    for (i, byte) in password.bytes().enumerate() {
        out.push_str(&format!(
            "{:02X}",
            byte ^ TYPE7_KEY[(seed + i) % TYPE7_KEY.len()]
        ));
    }
    out
}

pub(crate) fn type7_decode(encoded: &str) -> Option<String> {
    if !encoded.is_ascii() || encoded.len() < 2 || !encoded.len().is_multiple_of(2) {
        return None;
    }
    let seed: usize = encoded[..2].parse().ok()?;
    let mut out = Vec::new();
    for (i, pair) in encoded.as_bytes()[2..].chunks_exact(2).enumerate() {
        let byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        out.push(byte ^ TYPE7_KEY[(seed + i) % TYPE7_KEY.len()]);
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: [u8; 16]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_vectors() {
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hex(md5(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn md5_crypt_vectors() {
        // `openssl passwd -1 -salt xxxxxxxx password`
        assert_eq!(
            md5_crypt("password", "xxxxxxxx"),
            "$1$xxxxxxxx$UYCIxa628.9qXjpQCjM4a."
        );
        assert!(md5_crypt_verify(
            "password",
            "$1$xxxxxxxx$UYCIxa628.9qXjpQCjM4a."
        ));
        assert!(!md5_crypt_verify(
            "Password",
            "$1$xxxxxxxx$UYCIxa628.9qXjpQCjM4a."
        ));
        assert!(!md5_crypt_verify("password", "garbage"));
    }

    #[test]
    fn type7_round_trip() {
        // A widely published example.
        assert_eq!(type7_decode("0822455D0A16").as_deref(), Some("cisco"));
        assert_eq!(type7_encode("cisco", 8), "0822455D0A16");
        for seed in 0..16 {
            assert_eq!(
                type7_decode(&type7_encode("Nocturne!23", seed)).as_deref(),
                Some("Nocturne!23")
            );
        }
        assert_eq!(type7_decode("zz"), None);
        assert_eq!(type7_decode("081"), None);
    }
}
