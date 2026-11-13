//! TFTP packet codec (RFC 1350, options per RFC 2347).

const RRQ: u16 = 1;
const WRQ: u16 = 2;
const DATA: u16 = 3;
const ACK: u16 = 4;
const ERROR: u16 = 5;
const OACK: u16 = 6;

/// TFTP error codes.
pub(crate) mod code {
    pub const NOT_DEFINED: u16 = 0;
    pub const FILE_NOT_FOUND: u16 = 1;
    pub const ACCESS_VIOLATION: u16 = 2;
    pub const DISK_FULL: u16 = 3;
    pub const ILLEGAL_OPERATION: u16 = 4;
    pub const UNKNOWN_TRANSFER_ID: u16 = 5;
    pub const FILE_EXISTS: u16 = 6;
}

/// A read or write request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    pub write: bool,
    pub filename: String,
    /// Lower-cased transfer mode (`octet`, `netascii`, `mail`).
    pub mode: String,
    /// Option names (lower-cased) and values, in the order sent.
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Packet<'a> {
    Request(Request),
    Data { block: u16, data: &'a [u8] },
    Ack { block: u16 },
    Error { code: u16, message: String },
    OptionAck { options: Vec<(String, String)> },
}

/// Why a datagram is not a TFTP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Malformed {
    /// Too short, or an opcode TFTP does not have.
    NotTftp,
    /// A request whose strings are missing, unterminated or not text.
    BadRequest(&'static str),
}

pub(crate) fn parse(datagram: &[u8]) -> Result<Packet<'_>, Malformed> {
    let [high, low, body @ ..] = datagram else {
        return Err(Malformed::NotTftp);
    };
    match u16::from_be_bytes([*high, *low]) {
        opcode @ (RRQ | WRQ) => {
            let mut strings = strings(body)?.into_iter();
            let filename = strings
                .next()
                .ok_or(Malformed::BadRequest("the file name is missing"))?;
            let mode = strings
                .next()
                .ok_or(Malformed::BadRequest("the transfer mode is missing"))?;
            let mut options = Vec::new();
            while let (Some(name), Some(value)) = (strings.next(), strings.next()) {
                options.push((name.to_ascii_lowercase(), value));
            }
            Ok(Packet::Request(Request {
                write: opcode == WRQ,
                filename,
                mode: mode.to_ascii_lowercase(),
                options,
            }))
        }
        DATA => match body {
            [high, low, data @ ..] => Ok(Packet::Data {
                block: u16::from_be_bytes([*high, *low]),
                data,
            }),
            _ => Err(Malformed::NotTftp),
        },
        ACK => match body {
            [high, low, ..] => Ok(Packet::Ack {
                block: u16::from_be_bytes([*high, *low]),
            }),
            _ => Err(Malformed::NotTftp),
        },
        ERROR => match body {
            [high, low, message @ ..] => Ok(Packet::Error {
                code: u16::from_be_bytes([*high, *low]),
                message: String::from_utf8_lossy(
                    message.split(|byte| *byte == 0).next().unwrap_or_default(),
                )
                .into_owned(),
            }),
            _ => Err(Malformed::NotTftp),
        },
        OACK => {
            let mut strings = strings(body).map_err(|_| Malformed::NotTftp)?.into_iter();
            let mut options = Vec::new();
            while let (Some(name), Some(value)) = (strings.next(), strings.next()) {
                options.push((name.to_ascii_lowercase(), value));
            }
            Ok(Packet::OptionAck { options })
        }
        _ => Err(Malformed::NotTftp),
    }
}

/// The NUL-terminated strings of a request.
fn strings(body: &[u8]) -> Result<Vec<String>, Malformed> {
    let Some(terminated) = body.strip_suffix(&[0]) else {
        return Err(Malformed::BadRequest("the request is not NUL-terminated"));
    };
    terminated
        .split(|byte| *byte == 0)
        .map(|field| {
            std::str::from_utf8(field)
                .map(str::to_owned)
                .map_err(|_| Malformed::BadRequest("the request contains bytes that are not text"))
        })
        .collect()
}

/// A DATA packet header followed by room for `capacity` data bytes; the caller fills the data
/// in after the 4 header bytes and truncates.
pub(crate) fn data_buffer(capacity: usize) -> Vec<u8> {
    vec![0u8; 4 + capacity]
}

/// Writes the DATA header for `block` into the first 4 bytes of `buffer`.
pub(crate) fn set_data_header(buffer: &mut [u8], block: u16) {
    buffer[..2].copy_from_slice(&DATA.to_be_bytes());
    buffer[2..4].copy_from_slice(&block.to_be_bytes());
}

pub(crate) fn ack(block: u16) -> [u8; 4] {
    let [op_high, op_low] = ACK.to_be_bytes();
    let [block_high, block_low] = block.to_be_bytes();
    [op_high, op_low, block_high, block_low]
}

pub(crate) fn error(code: u16, message: &str) -> Vec<u8> {
    let mut packet = Vec::with_capacity(5 + message.len());
    packet.extend_from_slice(&ERROR.to_be_bytes());
    packet.extend_from_slice(&code.to_be_bytes());
    packet.extend(message.bytes().filter(|byte| *byte != 0));
    packet.push(0);
    packet
}

pub(crate) fn option_ack(options: &[(String, String)]) -> Vec<u8> {
    let mut packet = OACK.to_be_bytes().to_vec();
    for (name, value) in options {
        packet.extend_from_slice(name.as_bytes());
        packet.push(0);
        packet.extend_from_slice(value.as_bytes());
        packet.push(0);
    }
    packet
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn request(
        write: bool,
        filename: &str,
        mode: &str,
        options: &[(&str, &str)],
    ) -> Packet<'static> {
        Packet::Request(Request {
            write,
            filename: filename.into(),
            mode: mode.into(),
            options: options
                .iter()
                .map(|(name, value)| ((*name).into(), (*value).into()))
                .collect(),
        })
    }

    #[test]
    fn requests() {
        // RFC 1350 figure 5-1: opcode, filename, 0, mode, 0.
        assert_eq!(
            parse(b"\x00\x01running-config\x00octet\x00"),
            Ok(request(false, "running-config", "octet", &[]))
        );
        assert_eq!(
            parse(b"\x00\x02configs/sw1.cfg\x00NETASCII\x00"),
            Ok(request(true, "configs/sw1.cfg", "netascii", &[]))
        );
        // RFC 2347: options follow as name/value pairs; names are case-insensitive.
        assert_eq!(
            parse(b"\x00\x01ios.bin\x00octet\x00blksize\x001428\x00TSIZE\x000\x00timeout\x005\x00"),
            Ok(request(
                false,
                "ios.bin",
                "octet",
                &[("blksize", "1428"), ("tsize", "0"), ("timeout", "5")]
            ))
        );
        // An option name without a value is dropped.
        assert_eq!(
            parse(b"\x00\x01f\x00octet\x00blksize\x00"),
            Ok(request(false, "f", "octet", &[]))
        );
    }

    #[test]
    fn malformed_requests() {
        assert_eq!(
            parse(b"\x00\x01"),
            Err(Malformed::BadRequest("the request is not NUL-terminated"))
        );
        assert_eq!(
            parse(b"\x00\x01file"),
            Err(Malformed::BadRequest("the request is not NUL-terminated"))
        );
        assert_eq!(
            parse(b"\x00\x01file\x00"),
            Err(Malformed::BadRequest("the transfer mode is missing"))
        );
        assert_eq!(
            parse(b"\x00\x01file\x00octet"),
            Err(Malformed::BadRequest("the request is not NUL-terminated"))
        );
        assert_eq!(
            parse(b"\x00\x02\xff\xfe\x00octet\x00"),
            Err(Malformed::BadRequest(
                "the request contains bytes that are not text"
            ))
        );
        for junk in [
            &b""[..],
            b"\x00",
            b"\x00\x00",
            b"\x00\x07abc",
            b"\xff\xff\xff\xff",
            b"\x00\x03\x00",
            b"\x00\x04\x01",
            b"\x00\x05\x00",
        ] {
            assert_eq!(parse(junk), Err(Malformed::NotTftp), "{junk:?}");
        }
    }

    #[test]
    fn data_ack_error_and_option_ack() {
        assert_eq!(
            parse(b"\x00\x03\x00\x01hello"),
            Ok(Packet::Data {
                block: 1,
                data: b"hello"
            })
        );
        assert_eq!(
            parse(b"\x00\x03\xff\xff"),
            Ok(Packet::Data {
                block: 65535,
                data: b""
            })
        );
        assert_eq!(parse(b"\x00\x04\x01\x00"), Ok(Packet::Ack { block: 256 }));
        assert_eq!(
            parse(b"\x00\x05\x00\x01File not found\x00"),
            Ok(Packet::Error {
                code: 1,
                message: "File not found".into()
            })
        );
        assert_eq!(
            parse(b"\x00\x05\x00\x08"),
            Ok(Packet::Error {
                code: 8,
                message: String::new()
            })
        );
        assert_eq!(
            parse(b"\x00\x06blksize\x001024\x00tsize\x00300\x00"),
            Ok(Packet::OptionAck {
                options: vec![
                    ("blksize".into(), "1024".into()),
                    ("tsize".into(), "300".into())
                ]
            })
        );
    }

    #[test]
    fn encoding() {
        assert_eq!(ack(0), [0, 4, 0, 0]);
        assert_eq!(ack(0x1234), [0, 4, 0x12, 0x34]);
        assert_eq!(
            error(code::FILE_NOT_FOUND, "File not found"),
            b"\x00\x05\x00\x01File not found\x00"
        );
        assert_eq!(error(code::NOT_DEFINED, "a\0b"), b"\x00\x05\x00\x00ab\x00");
        assert_eq!(
            option_ack(&[
                ("blksize".into(), "1024".into()),
                ("tsize".into(), "300".into())
            ]),
            b"\x00\x06blksize\x001024\x00tsize\x00300\x00"
        );
        let mut buffer = data_buffer(3);
        set_data_header(&mut buffer, 7);
        buffer[4..].copy_from_slice(b"abc");
        assert_eq!(buffer, b"\x00\x03\x00\x07abc");
        // What is encoded parses back.
        assert_eq!(
            parse(&buffer),
            Ok(Packet::Data {
                block: 7,
                data: b"abc"
            })
        );
        assert_eq!(parse(&ack(9)), Ok(Packet::Ack { block: 9 }));
        assert_eq!(
            parse(&error(3, "full")),
            Ok(Packet::Error {
                code: 3,
                message: "full".into()
            })
        );
    }

    #[test]
    fn random_datagrams_never_panic() {
        let mut state = 0x1234_5678_9abc_def1u64;
        for round in 0..100_000u32 {
            let mut junk = vec![0u8; (state % 64) as usize];
            for byte in &mut junk {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            // Make most of them start with a valid opcode so every branch is exercised.
            if junk.len() >= 2 && round % 4 != 0 {
                junk[0] = 0;
                junk[1] = (round % 7) as u8;
            }
            let _ = parse(&junk);
        }
    }
}
