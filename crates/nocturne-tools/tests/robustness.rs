//! Nothing in the crate may panic or run away, whatever it is fed.
//!
//! Every public function is called with ASCII junk, mutated near-valid
//! input, Unicode and megabyte-long strings. A panic fails the test by
//! itself; on top of that the results are checked for the bounds the API
//! promises (list limits, short error messages, JSON-serialisable values).

use nocturne_tools::qos::{MarkingForm, VoiceOptions};
use nocturne_tools::secrets::PasswordPolicy;
use nocturne_tools::vlsm::Request;
use nocturne_tools::{
    ToolError, convert, mac, qos, reference, secrets, subnet, text, vlsm, wildcard,
};

/// SplitMix64: small, deterministic, good enough to scatter test inputs.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// Inputs that are valid for one tool or another; mutating them reaches
/// far deeper into the parsers than pure noise does.
const SEEDS: &[&str] = &[
    "192.168.1.10/24",
    "192.168.1.10 255.255.255.0",
    "10.0.0.0 0.0.0.255",
    "2001:db8::1/64",
    "::ffff:192.168.1.1",
    "fe80::1%eth0",
    "0.0.0.0/0",
    "255.255.255.255",
    "aa:bb:cc:dd:ee:ff",
    "aabb.ccdd.eeff",
    "AA-BB-CC-DD-EE-FF",
    "0822455D0A16",
    "enable password 7 0822455D0A16",
    "$9$LbHX-wg4Z",
    "$1$mERr$hx5rVt7rPNoS4wqbXKX7m0",
    "$9$nhEmQVczB7dqsO$X.HsgL6x1il0RxkOSSvyQYwucySCt7qFm4v7pqCxkKM",
    "set password ENC SH2nlSm9QL9tapcHPXIqAXvX7vBJuuqu22hpa0JX0sBuKIo7z2g0Kz/+0KyH4E=",
    "password cipher %^%#JvZxR2g8c;a9~FPN~n%^%#",
    "snmp-server community public RO",
    "0xC0A80101",
    "0b1010",
    "3232235777",
    "700 MB",
    "4.7GiB",
    "100 Mbps",
    "1.5G",
    "100Mb/s",
    "2w3d",
    "1 day, 4 hours, 5 minutes",
    "12:34:56",
    "(8640000) 1 day, 0:00:00.00",
    "ipsec-esp-tunnel:aes-cbc-sha256,nat-t",
    "mpls:3",
    "vxlan",
    "Gi1/0/1",
    "GigabitEthernet1/0/1.100",
    "ef",
    "0xb8",
    "af41",
    "0b10111000",
    "flash-override",
    "g723.1-6.3",
    "G.729A",
    "frame-relay",
    "ilbc_mode_20",
    "53/udp",
    "Zm9vYmFy",
    "/24",
    "255.255.255.0",
    "0.0.255.0",
    r"^(?P<interface>\S+) is (up|down)$",
];

const ASCII: &[u8] = b" !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~\t\n\r\0";

const UNICODE: &[&str] = &[
    "é",
    "ß",
    "١٢٣",
    "１９２．１６８",
    "🔑",
    "👩‍💻",
    "\u{202e}",
    "\u{0301}",
    "\u{feff}",
    "中文",
    "\u{2028}",
    "：",
    "／",
    "Ａ",
    "\u{1b}[31m",
    "\u{7f}",
    "\u{a0}",
];

fn ascii_junk(rng: &mut Rng) -> String {
    (0..rng.below(40))
        .map(|_| char::from(ASCII[rng.below(ASCII.len())]))
        .collect()
}

fn mutated(rng: &mut Rng) -> String {
    let mut chars: Vec<char> = rng.pick(SEEDS).chars().collect();
    for _ in 0..1 + rng.below(4) {
        let position = rng.below(chars.len() + 1);
        match rng.below(4) {
            0 if position < chars.len() => {
                chars[position] = char::from(ASCII[rng.below(ASCII.len())])
            }
            1 if position < chars.len() => {
                chars.remove(position);
            }
            2 => chars.insert(position, char::from(ASCII[rng.below(ASCII.len())])),
            _ => {
                for inserted in rng.pick(UNICODE).chars().rev() {
                    chars.insert(position, inserted);
                }
            }
        }
    }
    chars.into_iter().collect()
}

fn unicode_junk(rng: &mut Rng) -> String {
    (0..rng.below(12))
        .map(|_| {
            if rng.below(3) == 0 {
                rng.pick(SEEDS)
            } else {
                rng.pick(UNICODE)
            }
        })
        .collect()
}

/// Megabyte-sized inputs of the shapes most likely to hurt a parser.
fn long_inputs() -> Vec<String> {
    const LEN: usize = 1 << 20;
    let mut inputs: Vec<String> = [
        "A", "9", ":", ".", "1.", "0:", "/", " ", "(", "$9$", "%", "aa:", "é", "🔑", "1w", "x99",
        "0 ",
    ]
    .iter()
    .map(|unit| unit.repeat(LEN / unit.len()))
    .collect();
    inputs.push(format!("10.0.0.0/{}", "8".repeat(LEN)));
    inputs.push(format!("0x{}", "f".repeat(LEN)));
    inputs.push(format!("{} MB", "7".repeat(LEN)));
    inputs.push(format!("$9${}", "LbHX-wg4Z".repeat(LEN / 9)));
    inputs.push(format!("08{}", "22".repeat(LEN / 2)));
    inputs.push(format!("Gi{}", "1/".repeat(LEN / 2)));
    inputs
}

/// An error message must stay readable: it never echoes megabytes of input.
fn check_error(error: &ToolError) {
    let message = error.to_string();
    assert!(
        message.chars().count() < 700,
        "error message of {} characters: {message:.200}",
        message.chars().count()
    );
    assert!(!message.is_empty());
    if let ToolError::Invalid { input, .. } = error {
        assert!(
            input.chars().count() <= 83,
            "echoed input of {} characters",
            input.chars().count()
        );
        assert!(
            !input.chars().any(char::is_control),
            "control characters echoed: {input:?}"
        );
    }
}

fn settle<T: serde::Serialize>(result: Result<T, ToolError>) {
    match result {
        Ok(value) => {
            serde_json::to_string(&value).expect("every result can be sent to the UI as JSON");
        }
        Err(error) => check_error(&error),
    }
}

/// Feed one string to every function that takes text.
fn exercise(input: &str, rng: &mut Rng) {
    let list = [input.to_owned(), "10.0.0.0/24".to_owned(), input.to_owned()];
    let prefix = rng.below(140) as u8;
    let limit = [0, 1, 7, 300][rng.below(4)];

    // subnet
    settle(subnet::info(input));
    match subnet::split(input, prefix, limit) {
        Ok(split) => assert!(split.subnets.len() <= limit),
        Err(error) => check_error(&error),
    }
    settle(subnet::summarize(&list));
    settle(subnet::supernet(&list));
    settle(subnet::contains(input, "10.0.0.1"));
    settle(subnet::contains("10.0.0.0/8", input));
    settle(subnet::overlaps(input, input));
    settle(subnet::range_to_cidrs(input, "10.0.0.9"));
    settle(subnet::range_to_cidrs("10.0.0.1", input));
    settle(subnet::next_free(input, prefix, &list));
    settle(subnet::next_free("10.0.0.0/16", prefix, &list));
    settle(subnet::free_space(input, &list));
    settle(subnet::free_space("10.0.0.0/16", &list));

    // vlsm
    let requests = [
        Request {
            name: input.to_owned(),
            hosts: rng.next() >> rng.below(64),
        },
        Request {
            name: String::new(),
            hosts: 2,
        },
    ];
    settle(vlsm::plan(input, &requests, rng.below(2) == 0));
    settle(vlsm::plan("10.0.0.0/8", &requests, rng.below(2) == 0));

    // wildcard
    settle(wildcard::convert(input));
    settle(wildcard::matches(input, "0.0.0.255", "10.0.0.1"));
    settle(wildcard::matches("10.0.0.0", input, "10.0.0.1"));
    settle(wildcard::matches("10.0.0.0", "0.0.0.255", input));
    settle(wildcard::explain(input, "0.0.255.0"));
    settle(wildcard::explain("10.0.0.0", input));

    // mac
    settle(mac::info(input));
    let _ = mac::vendor(input);

    // secrets
    settle(secrets::cisco7_decode(input));
    settle(secrets::cisco7_encode(input, (rng.below(20)) as u8));
    settle(secrets::junos9_decode(input));
    settle(secrets::junos9_encode(input, &[rng.below(256) as u8; 32]));
    let kind = secrets::identify(input);
    assert!(!kind.kind.is_empty() && !kind.name.is_empty() && !kind.advice.is_empty());
    assert!(["none", "weak", "moderate", "strong", "unknown"].contains(&kind.strength.as_str()));

    // convert
    settle(convert::number(input));
    settle(convert::parse_size_bytes(input));
    settle(convert::parse_rate_bps(input));
    settle(convert::transfer_time(input, "1G", 0.0));
    settle(convert::transfer_time("1 GB", input, 5.0));
    let _ = convert::parse_duration_s(input);
    match convert::mtu(1500, &[input.to_owned(), "vxlan".to_owned()]) {
        Ok(result) => assert!(result.payload_mtu <= 1500 && result.breakdown.len() <= 65),
        Err(error) => check_error(&error),
    }

    // qos
    for form in [
        MarkingForm::Dscp,
        MarkingForm::Tos,
        MarkingForm::Precedence,
        MarkingForm::Name,
    ] {
        match qos::marking(input, form) {
            Ok(marking) => {
                assert_eq!(marking.tos >> 2, marking.dscp);
                assert_eq!(marking.tos & 3, marking.ecn);
                serde_json::to_string(&marking).expect("a marking can be sent to the UI as JSON");
            }
            Err(error) => check_error(&error),
        }
    }
    if let Err(error) = input.parse::<MarkingForm>() {
        check_error(&error);
    }
    let voice_options = VoiceOptions {
        crtp: rng.below(2) == 0,
        vad: rng.below(2) == 0,
        packet_ms: [None, Some(rng.next() as u32 >> rng.below(32))][rng.below(2)],
        l2_bytes: [None, Some(rng.next() as u32 >> rng.below(32))][rng.below(2)],
    };
    let calls = rng.next() as u32 >> rng.below(32);
    for (codec, link) in [
        (input, "ppp"),
        ("g729", input),
        (input, input),
        ("g711", "custom"),
    ] {
        match qos::voice(codec, calls, link, &voice_options) {
            Ok(answer) => {
                // The answer is the packet times the rate, whatever was asked.
                let bytes: u32 = answer.packet.iter().map(|part| part.bytes).sum();
                assert_eq!(bytes, answer.packet_bytes);
                assert!(answer.per_call_bps > 0 && answer.total_bps >= answer.per_call_bps);
                assert!(answer.arithmetic.len() <= 6 && answer.notes.len() <= 6);
                serde_json::to_string(&answer).expect("an answer can be sent to the UI as JSON");
            }
            Err(error) => check_error(&error),
        }
    }

    // reference
    assert!(reference::ports(input).len() <= 300);
    let _ = reference::dscp(input);
    let long = reference::interface_long_name(input);
    let short = reference::interface_short_name(input);
    assert!(long.len() <= input.len() + 32 && short.len() <= input.len() + 32);
    assert!(reference::interface_sort_key(input).len() <= input.len() + 1);

    // text
    let result = text::regex_test(
        input,
        "GigabitEthernet0/1 is up\nGigabitEthernet0/2 is down",
        rng.below(2) == 0,
        rng.below(2) == 0,
    );
    assert!(result.matches.len() <= text::MAX_REGEX_MATCHES);
    let result = text::regex_test(r"\w+|", input, true, false);
    assert!(result.matches.len() <= text::MAX_REGEX_MATCHES);
    for found in &result.matches {
        assert_eq!(&input[found.start..found.end], found.text);
    }
    let digests = text::hashes(input.as_bytes());
    assert_eq!(
        (
            digests.md5.len(),
            digests.sha1.len(),
            digests.sha256.len(),
            digests.sha512.len()
        ),
        (32, 40, 64, 128)
    );
    assert_eq!(
        text::base64_decode(&text::base64_encode(input.as_bytes())).as_deref(),
        Ok(input.as_bytes())
    );
    settle(text::base64_decode(input));
}

#[test]
fn ascii_junk_never_panics() {
    let mut rng = Rng(1);
    for _ in 0..2000 {
        let input = ascii_junk(&mut rng);
        exercise(&input, &mut rng);
    }
}

#[test]
fn mutated_valid_input_never_panics() {
    let mut rng = Rng(2);
    for _ in 0..4000 {
        let input = mutated(&mut rng);
        exercise(&input, &mut rng);
    }
    for seed in SEEDS {
        exercise(seed, &mut rng);
        // Every prefix and suffix, as when the input is being typed or deleted.
        let boundaries: Vec<usize> = seed.char_indices().map(|(index, _)| index).collect();
        for boundary in boundaries {
            exercise(&seed[..boundary], &mut rng);
            exercise(&seed[boundary..], &mut rng);
        }
    }
}

#[test]
fn unicode_never_panics() {
    let mut rng = Rng(3);
    for _ in 0..2000 {
        let input = unicode_junk(&mut rng);
        exercise(&input, &mut rng);
    }
    for sample in UNICODE {
        exercise(sample, &mut rng);
    }
}

#[test]
fn very_long_input_is_handled_in_bounded_time_and_space() {
    let mut rng = Rng(4);
    let started = std::time::Instant::now();
    for input in long_inputs() {
        exercise(&input, &mut rng);
    }
    // Generous: this is a guard against quadratic behaviour, not a benchmark.
    assert!(
        started.elapsed().as_secs() < 120,
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn lists_honour_their_limits() {
    // Splits: the caller's limit, and the hard cap for absurd limits.
    for (network, new_prefix) in [
        ("0.0.0.0/0", 32),
        ("::/0", 128),
        ("10.0.0.0/8", 30),
        ("2001:db8::/32", 64),
    ] {
        for limit in [0usize, 1, 10, 1000] {
            let split = subnet::split(network, new_prefix, limit).unwrap();
            assert_eq!(split.subnets.len(), limit);
            assert!(split.truncated);
        }
    }
    let capped = subnet::split("::/0", 128, usize::MAX).unwrap();
    assert_eq!(capped.subnets.len(), subnet::MAX_SPLIT_SUBNETS);
    assert_eq!(capped.total, "340282366920938463463374607431768211456");

    // A million consecutive addresses summarise into seven blocks:
    // 1,000,000 = 2^19 + 2^18 + 2^17 + 2^16 + 2^14 + 2^9 + 2^6.
    let many: Vec<String> = (0..1_000_000u32)
        .map(|index| format!("10.{}.{}.{}", index >> 16, (index >> 8) & 255, index & 255))
        .collect();
    assert_eq!(
        subnet::summarize(&many).unwrap(),
        [
            "10.0.0.0/13",
            "10.8.0.0/14",
            "10.12.0.0/15",
            "10.14.0.0/16",
            "10.15.0.0/18",
            "10.15.64.0/23",
            "10.15.66.0/26"
        ]
    );
    assert_eq!(subnet::supernet(&many).unwrap(), "10.0.0.0/12");

    // The worst case for free space: every other address used.
    let alternating: Vec<String> = (0..65_536u32)
        .step_by(2)
        .map(|index| format!("10.9.{}.{}", index >> 8, index & 255))
        .collect();
    let free = subnet::free_space("10.9.0.0/16", &alternating).unwrap();
    assert_eq!(free.len(), 32_768);
    assert_eq!(
        subnet::next_free("10.9.0.0/16", 32, &alternating)
            .unwrap()
            .as_deref(),
        Some("10.9.0.1/32")
    );
    assert_eq!(
        subnet::next_free("10.9.0.0/16", 31, &alternating).unwrap(),
        None
    );

    // Passwords, MTU stacks and regex matches are capped too.
    let policy = PasswordPolicy {
        length: usize::MAX,
        upper: true,
        lower: true,
        digits: true,
        symbols: true,
        avoid_ambiguous: false,
    };
    assert!(matches!(
        secrets::generate_password(&policy, &[0; 32]),
        Err(ToolError::TooLarge(_))
    ));
    assert!(convert::mtu(u32::MAX, &["mpls:4294967295".to_owned()]).is_err());
    let stack: Vec<String> = std::iter::repeat_n("mpls".to_owned(), 100_000).collect();
    assert!(matches!(
        convert::mtu(1500, &stack),
        Err(ToolError::DoesNotFit(_))
    ));
    assert_eq!(
        text::regex_test("", &"x".repeat(200_000), false, false)
            .matches
            .len(),
        text::MAX_REGEX_MATCHES
    );
}

#[test]
fn numeric_arguments_at_their_extremes() {
    let floats = [
        0.0,
        -0.0,
        1.0,
        -1.0,
        0.5,
        999.999,
        1e3,
        1e9,
        1e18,
        1e30,
        1e300,
        -1e300,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        f64::EPSILON,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for value in floats {
        for formatted in [convert::format_rate(value), convert::format_bytes(value)] {
            assert!(
                !formatted.is_empty() && formatted.len() < 400,
                "{value}: {formatted}"
            );
        }
        settle(convert::transfer_time("1 GB", "1 Gbps", value));
    }
    for seconds in [
        0,
        1,
        59,
        60,
        3599,
        86_399,
        604_800,
        u64::from(u32::MAX),
        u64::MAX - 1,
        u64::MAX,
    ] {
        let formatted = convert::format_duration(seconds);
        assert_eq!(
            convert::parse_duration_s(&formatted),
            Some(seconds),
            "{formatted}"
        );
    }
    for link_mtu in [
        0,
        1,
        67,
        68,
        575,
        576,
        1279,
        1280,
        1500,
        9000,
        65_535,
        u32::MAX,
    ] {
        for stack in [
            vec![],
            vec!["ipsec-esp-tunnel".to_owned()],
            vec!["vxlan:v6".to_owned(), "gre".to_owned(), "mpls:8".to_owned()],
        ] {
            if let Ok(result) = convert::mtu(link_mtu, &stack) {
                assert_eq!(result.payload_mtu + result.overhead, link_mtu);
                assert!(
                    result.tcp_mss_v4 <= result.payload_mtu
                        && result.tcp_mss_v6 <= result.tcp_mss_v4
                );
            }
        }
    }
    let mut rng = Rng(5);
    for _ in 0..2000 {
        let policy = PasswordPolicy {
            length: [0, 1, 2, 3, 4, 5, 16, 64, 1024, 1025, usize::MAX][rng.below(11)],
            upper: rng.below(2) == 0,
            lower: rng.below(2) == 0,
            digits: rng.below(2) == 0,
            symbols: rng.below(2) == 0,
            avoid_ambiguous: rng.below(2) == 0,
        };
        let mut seed = [0u8; 32];
        seed[..8].copy_from_slice(&rng.next().to_le_bytes());
        match secrets::generate_password(&policy, &seed) {
            Ok(password) => assert_eq!(password.chars().count(), policy.length),
            Err(error) => check_error(&error),
        }
        settle(subnet::split(
            "10.0.0.0/8",
            rng.below(256) as u8,
            rng.below(5),
        ));
        settle(subnet::next_free("10.0.0.0/8", rng.below(256) as u8, &[]));
        settle(secrets::cisco7_encode("cisco", rng.below(256) as u8));
    }
}

#[test]
fn reference_tables_serialise() {
    assert!(serde_json::to_string(&reference::ports("")).unwrap().len() > 10_000);
    for json in [
        serde_json::to_string(&reference::dscp_table()).unwrap(),
        serde_json::to_string(&reference::ip_protocols()).unwrap(),
        serde_json::to_string(&reference::ethertypes()).unwrap(),
        serde_json::to_string(&reference::icmp_types()).unwrap(),
        serde_json::to_string(&reference::icmpv6_types()).unwrap(),
        serde_json::to_string(&reference::administrative_distances()).unwrap(),
        serde_json::to_string(&reference::ethernet_standards()).unwrap(),
        serde_json::to_string(&reference::syslog_levels()).unwrap(),
        serde_json::to_string(&convert::encapsulations()).unwrap(),
        serde_json::to_string(&qos::voice_codecs()).unwrap(),
        serde_json::to_string(&qos::voice_links()).unwrap(),
    ] {
        assert!(json.starts_with('[') && json.len() > 300);
    }
    let (count, date) = mac::registry_info();
    assert!(count > 30_000 && date.len() == 10);
}
