//! The public API exactly as `docs/dev/spec-tools.md` promises it to the
//! application crate: every name, signature, field and derive, plus the
//! examples the specification gives. If this file stops compiling, the
//! contract was broken.

use std::fmt::Debug;

use nocturne_tools::{ToolError, convert, mac, reference, secrets, subnet, text, vlsm, wildcard};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

fn s(text: &str) -> String {
    text.to_owned()
}

/// "All result types derive Serialize, Deserialize, Clone, Debug, PartialEq."
fn assert_result_type<T: Serialize + DeserializeOwned + Clone + Debug + PartialEq>(
    value: &T,
) -> serde_json::Value {
    let json = serde_json::to_value(value).expect("serialises");
    let back: T = serde_json::from_value(json.clone()).expect("deserialises");
    assert_eq!(&back, value);
    assert_eq!(&value.clone(), value);
    json
}

fn keys(json: &serde_json::Value) -> Vec<&str> {
    json.as_object()
        .expect("a JSON object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn function_signatures() {
    let _: fn(&str) -> Result<subnet::SubnetInfo, ToolError> = subnet::info;
    let _: fn(&str, u8, usize) -> Result<subnet::SplitResult, ToolError> = subnet::split;
    let _: fn(&[String]) -> Result<Vec<String>, ToolError> = subnet::summarize;
    let _: fn(&[String]) -> Result<String, ToolError> = subnet::supernet;
    let _: fn(&str, &str) -> Result<bool, ToolError> = subnet::contains;
    let _: fn(&str, &str) -> Result<bool, ToolError> = subnet::overlaps;
    let _: fn(&str, &str) -> Result<Vec<String>, ToolError> = subnet::range_to_cidrs;
    let _: fn(&str, u8, &[String]) -> Result<Option<String>, ToolError> = subnet::next_free;
    let _: fn(&str, &[String]) -> Result<Vec<String>, ToolError> = subnet::free_space;

    let _: fn(&str, &[vlsm::Request], bool) -> Result<vlsm::Plan, ToolError> = vlsm::plan;

    let _: fn(&str) -> Result<wildcard::MaskInfo, ToolError> = wildcard::convert;
    let _: fn(&str, &str, &str) -> Result<bool, ToolError> = wildcard::matches;
    let _: fn(&str, &str) -> Result<String, ToolError> = wildcard::explain;

    let _: fn(&str) -> Result<mac::MacInfo, ToolError> = mac::info;
    let _: fn(&str) -> Option<String> = mac::vendor;
    let _: fn() -> (usize, &'static str) = mac::registry_info;

    let _: fn(&str) -> Result<String, ToolError> = secrets::cisco7_decode;
    let _: fn(&str, u8) -> Result<String, ToolError> = secrets::cisco7_encode;
    let _: fn(&str) -> Result<String, ToolError> = secrets::junos9_decode;
    let _: fn(&str) -> secrets::SecretKind = secrets::identify;
    let _: fn(&secrets::PasswordPolicy, &[u8; 32]) -> Result<String, ToolError> =
        secrets::generate_password;

    let _: fn(&str) -> Result<convert::NumberForms, ToolError> = convert::number;
    let _: fn(&str, &str, f64) -> Result<convert::TransferEstimate, ToolError> =
        convert::transfer_time;
    let _: fn(&str) -> Result<u64, ToolError> = convert::parse_size_bytes;
    let _: fn(&str) -> Result<u64, ToolError> = convert::parse_rate_bps;
    let _: fn(f64) -> String = convert::format_rate;
    let _: fn(f64) -> String = convert::format_bytes;
    let _: fn(&str) -> Option<u64> = convert::parse_duration_s;
    let _: fn(u64) -> String = convert::format_duration;
    let _: fn(u32, &[String]) -> Result<convert::MtuResult, ToolError> = convert::mtu;

    let _: fn(&str) -> Vec<reference::PortEntry> = reference::ports;
    let _: fn() -> Vec<reference::DscpEntry> = reference::dscp_table;
    let _: fn(&str) -> Option<reference::DscpEntry> = reference::dscp;
    let _: fn() -> Vec<reference::NamedNumber> = reference::ip_protocols;
    let _: fn() -> Vec<reference::NamedNumber> = reference::ethertypes;
    let _: fn() -> Vec<reference::NamedNumber> = reference::icmp_types;
    let _: fn() -> Vec<reference::AdEntry> = reference::administrative_distances;
    let _: fn() -> Vec<reference::CableEntry> = reference::ethernet_standards;
    let _: fn() -> Vec<reference::SyslogLevel> = reference::syslog_levels;
    let _: fn(&str) -> String = reference::interface_long_name;
    let _: fn(&str) -> String = reference::interface_short_name;
    let _: fn(&str) -> Vec<(String, u64)> = reference::interface_sort_key;

    let _: fn(&str, &str, bool, bool) -> text::RegexResult = text::regex_test;
    let _: fn(&[u8]) -> text::Hashes = text::hashes;
    let _: fn(&[u8]) -> String = text::base64_encode;
    let _: fn(&str) -> Result<Vec<u8>, ToolError> = text::base64_decode;
}

#[test]
fn result_types_have_the_promised_fields_and_derives() {
    let info = subnet::SubnetInfo {
        version: 4,
        address: s("192.168.1.10"),
        network: s("192.168.1.0"),
        prefix: 24,
        cidr: s("192.168.1.0/24"),
        netmask: s("255.255.255.0"),
        wildcard: s("0.0.0.255"),
        broadcast: s("192.168.1.255"),
        first_host: s("192.168.1.1"),
        last_host: s("192.168.1.254"),
        usable_hosts: s("254"),
        total_addresses: s("256"),
        class: s("C"),
        scope: s("private (RFC 1918)"),
        binary_address: s("11000000.10101000.00000001.00001010"),
        binary_netmask: s("11111111.11111111.11111111.00000000"),
        hex: s("0xC0A8010A"),
        integer: s("3232235786"),
        reverse_dns: s("10.1.168.192.in-addr.arpa"),
        expanded: s("192.168.1.10"),
        compressed: s("192.168.1.10"),
        is_network_address: false,
        is_broadcast_address: false,
    };
    // The literal above is, field for field, what the calculator returns.
    assert_eq!(subnet::info("192.168.1.10/24").unwrap(), info);
    assert_eq!(
        keys(&assert_result_type(&info)),
        [
            "version",
            "address",
            "network",
            "prefix",
            "cidr",
            "netmask",
            "wildcard",
            "broadcast",
            "first_host",
            "last_host",
            "usable_hosts",
            "total_addresses",
            "class",
            "scope",
            "binary_address",
            "binary_netmask",
            "hex",
            "integer",
            "reverse_dns",
            "expanded",
            "compressed",
            "is_network_address",
            "is_broadcast_address"
        ]
    );
    let split = subnet::SplitResult {
        total: s("1"),
        subnets: vec![info],
        truncated: false,
    };
    assert_eq!(
        keys(&assert_result_type(&split)),
        ["total", "subnets", "truncated"]
    );

    let request = vlsm::Request {
        name: s("LAN"),
        hosts: 100,
    };
    assert_eq!(
        assert_result_type(&request),
        json!({"name": "LAN", "hosts": 100})
    );
    let allocation = vlsm::Allocation {
        name: s("LAN"),
        hosts_requested: 100,
        cidr: s("10.0.0.0/25"),
        netmask: s("255.255.255.128"),
        first_host: s("10.0.0.1"),
        last_host: s("10.0.0.126"),
        broadcast: s("10.0.0.127"),
        usable_hosts: 126,
        spare: 26,
    };
    let plan = vlsm::Plan {
        allocations: vec![allocation],
        unallocated: vec![s("10.0.0.128/25")],
        used_addresses: 128,
        total_addresses: 256,
    };
    assert_eq!(vlsm::plan("10.0.0.0/24", &[request], false).unwrap(), plan);
    let plan_json = assert_result_type(&plan);
    assert_eq!(
        keys(&plan_json),
        [
            "allocations",
            "unallocated",
            "used_addresses",
            "total_addresses"
        ]
    );
    assert_eq!(
        keys(&plan_json["allocations"][0]),
        [
            "name",
            "hosts_requested",
            "cidr",
            "netmask",
            "first_host",
            "last_host",
            "broadcast",
            "usable_hosts",
            "spare"
        ]
    );

    let mask = wildcard::MaskInfo {
        prefix: Some(24),
        netmask: s("255.255.255.0"),
        wildcard: s("0.0.0.255"),
        contiguous: true,
        addresses: s("256"),
    };
    assert_eq!(wildcard::convert("/24").unwrap(), mask);
    assert_eq!(
        assert_result_type(&mask),
        json!({"prefix": 24, "netmask": "255.255.255.0", "wildcard": "0.0.0.255", "contiguous": true, "addresses": "256"})
    );
    assert_eq!(
        assert_result_type(&wildcard::convert("0.0.255.0").unwrap())["prefix"],
        json!(null)
    );

    let mac_info = mac::MacInfo {
        colon: s("aa:bb:cc:dd:ee:ff"),
        hyphen: s("AA-BB-CC-DD-EE-FF"),
        dotted: s("aabb.ccdd.eeff"),
        bare: s("aabbccddeeff"),
        oui: s("AA:BB:CC"),
        vendor: None,
        is_multicast: false,
        is_broadcast: false,
        is_locally_administered: true,
        eui64: s("a8bb:ccff:fedd:eeff"),
        link_local_ipv6: s("fe80::a8bb:ccff:fedd:eeff"),
    };
    assert_eq!(mac::info("AABB.CCDD.EEFF").unwrap(), mac_info);
    assert_eq!(
        keys(&assert_result_type(&mac_info)),
        [
            "colon",
            "hyphen",
            "dotted",
            "bare",
            "oui",
            "vendor",
            "is_multicast",
            "is_broadcast",
            "is_locally_administered",
            "eui64",
            "link_local_ipv6"
        ]
    );

    let kind = secrets::SecretKind {
        kind: s("k"),
        name: s("n"),
        reversible: true,
        strength: s("none"),
        advice: s("a"),
    };
    assert_eq!(
        keys(&assert_result_type(&kind)),
        ["kind", "name", "reversible", "strength", "advice"]
    );
    let policy = secrets::PasswordPolicy {
        length: 16,
        upper: true,
        lower: true,
        digits: true,
        symbols: false,
        avoid_ambiguous: true,
    };
    assert_eq!(
        keys(&assert_result_type(&policy)),
        [
            "length",
            "upper",
            "lower",
            "digits",
            "symbols",
            "avoid_ambiguous"
        ]
    );

    let forms = convert::NumberForms {
        decimal: s("10"),
        hex: s("0xA"),
        binary: s("0b1010"),
        octal: s("0o12"),
        ipv4: Some(s("0.0.0.10")),
    };
    assert_eq!(convert::number("0b1010").unwrap(), forms);
    assert_eq!(
        keys(&assert_result_type(&forms)),
        ["decimal", "hex", "binary", "octal", "ipv4"]
    );
    let estimate = convert::TransferEstimate {
        seconds: 56.0,
        human: s("56 s"),
        effective_bps: 1e8,
    };
    assert_eq!(
        convert::transfer_time("700 MB", "100 Mbps", 0.0).unwrap(),
        estimate
    );
    assert_eq!(
        assert_result_type(&estimate),
        json!({"seconds": 56.0, "human": "56 s", "effective_bps": 1e8})
    );
    let mtu = convert::MtuResult {
        payload_mtu: 1476,
        tcp_mss_v4: 1436,
        tcp_mss_v6: 1416,
        overhead: 24,
        breakdown: vec![(s("gre (IPv4 20 + GRE 4)"), 24)],
    };
    assert_eq!(convert::mtu(1500, &[s("gre")]).unwrap(), mtu);
    assert_eq!(
        assert_result_type(&mtu),
        json!({"payload_mtu": 1476, "tcp_mss_v4": 1436, "tcp_mss_v6": 1416, "overhead": 24, "breakdown": [["gre (IPv4 20 + GRE 4)", 24]]})
    );

    let port = reference::PortEntry {
        port: 179,
        protocol: s("tcp"),
        name: s("bgp"),
        description: s("BGP"),
    };
    assert_eq!(reference::ports("179"), std::slice::from_ref(&port));
    assert_eq!(
        keys(&assert_result_type(&port)),
        ["port", "protocol", "name", "description"]
    );
    let dscp = reference::DscpEntry {
        name: s("EF"),
        dscp: 46,
        binary: s("101110"),
        tos: 184,
        precedence: 5,
        phb: s("p"),
        typical_use: s("u"),
    };
    assert_eq!(
        keys(&assert_result_type(&dscp)),
        [
            "name",
            "dscp",
            "binary",
            "tos",
            "precedence",
            "phb",
            "typical_use"
        ]
    );
    let number = reference::NamedNumber {
        number: 6,
        name: s("TCP"),
        description: s("Transmission Control Protocol"),
    };
    assert_eq!(
        reference::ip_protocols()
            .iter()
            .find(|entry| entry.number == 6),
        Some(&number)
    );
    assert_eq!(
        keys(&assert_result_type(&number)),
        ["number", "name", "description"]
    );
    let distance = reference::AdEntry {
        source: s("Static"),
        cisco: Some(1),
        juniper: Some(5),
        mikrotik: Some(1),
        huawei: Some(60),
        arista: Some(1),
    };
    assert_eq!(
        reference::administrative_distances()
            .iter()
            .find(|entry| entry.source == "Static"),
        Some(&distance)
    );
    assert_eq!(
        assert_result_type(&distance),
        json!({"source": "Static", "cisco": 1, "juniper": 5, "mikrotik": 1, "huawei": 60, "arista": 1})
    );
    assert_eq!(
        assert_result_type(&reference::administrative_distances()[2])["juniper"],
        json!(null)
    );
    let cable = reference::CableEntry {
        standard: s("10GBASE-LR"),
        medium: s("Single-mode fibre, 1310 nm"),
        speed: s("10 Gb/s"),
        max_distance: s("10 km"),
        notes: s(""),
    };
    assert_eq!(
        reference::ethernet_standards()
            .iter()
            .find(|entry| entry.standard == "10GBASE-LR"),
        Some(&cable)
    );
    assert_eq!(
        keys(&assert_result_type(&cable)),
        ["standard", "medium", "speed", "max_distance", "notes"]
    );
    let level = reference::SyslogLevel {
        level: 4,
        name: s("Warning"),
        cisco_keyword: s("warnings"),
        meaning: s("Warning conditions"),
    };
    assert_eq!(reference::syslog_levels()[4], level);
    assert_eq!(
        keys(&assert_result_type(&level)),
        ["level", "name", "cisco_keyword", "meaning"]
    );

    let found = text::RegexMatch {
        start: 0,
        end: 2,
        text: s("ab"),
        groups: vec![Some(s("a")), None],
        named: vec![(s("first"), Some(s("a"))), (s("second"), None)],
    };
    let result = text::RegexResult {
        matches: vec![found],
        error: None,
    };
    assert_eq!(
        text::regex_test("(?P<first>a)b|(?P<second>c)", "ab", false, false),
        result
    );
    assert_eq!(
        assert_result_type(&result),
        json!({"matches": [{"start": 0, "end": 2, "text": "ab", "groups": ["a", null], "named": [["first", "a"], ["second", null]]}], "error": null})
    );
    let hashes = text::Hashes {
        md5: s("m"),
        sha1: s("s"),
        sha256: s("t"),
        sha512: s("u"),
    };
    assert_eq!(
        keys(&assert_result_type(&hashes)),
        ["md5", "sha1", "sha256", "sha512"]
    );
}

#[test]
fn the_error_type() {
    fn assert_error<T: std::error::Error + Debug + Clone + PartialEq + Send + Sync + 'static>() {}
    assert_error::<ToolError>();

    let invalid = ToolError::Invalid {
        what: s("IPv4 address"),
        input: s("10.0.300.1"),
        reason: s("`300` is not a valid octet (0-255)"),
    };
    assert_eq!(subnet::info("10.0.300.1"), Err(invalid.clone()));
    assert_eq!(
        invalid.to_string(),
        "invalid IPv4 address `10.0.300.1`: `300` is not a valid octet (0-255)"
    );
    // The three variants the specification names are all there are.
    let describe = |error: &ToolError| match error {
        ToolError::Invalid {
            what,
            input,
            reason,
        } => format!("{what}|{input}|{reason}"),
        ToolError::TooLarge(message) | ToolError::DoesNotFit(message) => message.clone(),
    };
    assert_eq!(
        describe(&invalid),
        "IPv4 address|10.0.300.1|`300` is not a valid octet (0-255)"
    );
    assert!(matches!(
        convert::number(&"9".repeat(60)),
        Err(ToolError::TooLarge(_))
    ));
    assert!(matches!(
        vlsm::plan("10.0.0.0/30", &[vlsm::Request { name: s("big"), hosts: 100 }], false),
        Err(ToolError::DoesNotFit(message)) if message.contains("`big`")
    ));
    // Errors travel to the UI as JSON too.
    assert_eq!(
        serde_json::to_value(ToolError::DoesNotFit(s("no room"))).unwrap(),
        json!({"does_not_fit": "no room"})
    );
    assert_eq!(
        keys(&serde_json::to_value(&invalid).unwrap()["invalid"]),
        ["what", "input", "reason"]
    );
}

#[test]
fn the_examples_in_the_specification() {
    // subnet::info input forms
    for input in [
        "192.168.1.10/24",
        "192.168.1.10 255.255.255.0",
        "192.168.1.10/255.255.255.0",
        "192.168.1.10 0.0.0.255",
    ] {
        assert_eq!(
            subnet::info(input).unwrap().cidr,
            "192.168.1.0/24",
            "{input}"
        );
    }
    assert_eq!(subnet::info("192.168.1.10").unwrap().prefix, 32);
    assert_eq!(subnet::info("2001:db8::1").unwrap().prefix, 128);
    let v6 = subnet::info("2001:db8::1/64").unwrap();
    assert_eq!(
        (
            v6.version,
            v6.netmask.as_str(),
            v6.class.as_str(),
            v6.broadcast.as_str()
        ),
        (6, "", "", "")
    );
    // "/31 -> 2, /32 -> 1"; broadcast "" for /31 and /32
    let p2p = subnet::info("10.0.0.0/31").unwrap();
    assert_eq!(
        (p2p.usable_hosts.as_str(), p2p.broadcast.as_str()),
        ("2", "")
    );
    let host = subnet::info("10.0.0.0/32").unwrap();
    assert_eq!(
        (host.usable_hosts.as_str(), host.broadcast.as_str()),
        ("1", "")
    );
    // scopes named in the specification
    for (address, scope) in [
        ("10.1.2.3", "private (RFC 1918)"),
        ("8.8.8.8", "public"),
        ("127.0.0.1", "loopback"),
        ("169.254.0.1", "link-local"),
        ("100.64.0.1", "CGNAT (RFC 6598)"),
        ("192.0.2.1", "documentation"),
        ("224.0.0.1", "multicast"),
        ("240.0.0.1", "reserved"),
        ("2606:4700::1", "global unicast"),
        ("fd00::1", "unique local"),
    ] {
        assert_eq!(subnet::info(address).unwrap().scope, scope, "{address}");
    }
    assert_eq!(
        subnet::range_to_cidrs("10.0.0.5", "10.0.1.20")
            .unwrap()
            .len(),
        10
    );

    // vlsm: "point-to-point 2 -> /30; p2p31: true -> /31"
    let link = [vlsm::Request {
        name: s("p2p"),
        hosts: 2,
    }];
    assert_eq!(
        vlsm::plan("10.0.0.0/24", &link, false).unwrap().allocations[0].cidr,
        "10.0.0.0/30"
    );
    assert_eq!(
        vlsm::plan("10.0.0.0/24", &link, true).unwrap().allocations[0].cidr,
        "10.0.0.0/31"
    );

    // wildcard::convert: "a prefix length ("24", "/24"), a netmask or a wildcard mask"
    for input in ["24", "/24", "255.255.255.0", "0.0.0.255"] {
        assert_eq!(
            wildcard::convert(input).unwrap().prefix,
            Some(24),
            "{input}"
        );
    }
    assert!(wildcard::matches("10.0.0.0", "0.0.255.0", "10.0.77.0").unwrap());
    assert!(
        wildcard::explain("10.0.0.0", "0.0.0.255")
            .unwrap()
            .contains("256 addresses")
    );

    // secrets
    assert_eq!(secrets::cisco7_decode("0822455D0A16").unwrap(), "cisco");
    assert_eq!(secrets::cisco7_encode("cisco", 8).unwrap(), "0822455D0A16");
    assert_eq!(secrets::junos9_decode("$9$LbHX-wg4Z").unwrap(), "lc");
    for (input, kind) in [
        ("enable password 0 cisco", "cisco_type0"),
        (
            "enable secret 4 Rv4kArhts7yA2xd8BD2YTVbts7yA2xd8BD2YTVbts7y",
            "cisco_type4",
        ),
        (
            "enable secret 5 $1$mERr$hx5rVt7rPNoS4wqbXKX7m0",
            "cisco_type5",
        ),
        ("key 6 FLgBaJHXdYY_AcHZZMgQ_RhTDJXHUBAAB", "cisco_type6"),
        ("password 7 0822455D0A16", "cisco_type7"),
        (
            "$8$dsYGNam3K1SIJO$7nv/35M/qr6t.dVc7UY9zrJDWRVqncHub1PE9UlMQFs",
            "cisco_type8",
        ),
        (
            "$9$nhEmQVczB7dqsO$X.HsgL6x1il0RxkOSSvyQYwucySCt7qFm4v7pqCxkKM",
            "cisco_type9",
        ),
        ("$1$mERr$hx5rVt7rPNoS4wqbXKX7m0", "md5crypt"),
        (
            "$5$saltstring$5B8vYYiY.CVt1RlTTf8KbXBH3hsxY/GNooZaBBGWEc5",
            "sha256crypt",
        ),
        (
            "$6$saltstring$svn8UoSVapNtMuq1ukKS4tPQd8iKwSMHWjl/O817G3uBnIFNjnQJuesI68u4OTLiBFdcbYEdFCoEOfaS35inz1",
            "sha512crypt",
        ),
        (
            "$2b$12$R9h/cIPz0gi.URNNX3kh2OPST9/PgBkqquzi.Ss7KIUgO2t0jWMUW",
            "bcrypt",
        ),
        ("$9$LbHX-wg4Z", "junos_9"),
        (
            "password cipher %^%#JvZxR2g8c;a9~FPN~n%^%#",
            "huawei_cipher",
        ),
        (
            "set psksecret ENC bHl0aGl1bSBwb2x5bWVyIGJhdHRlcnk=",
            "fortios_enc",
        ),
        ("/user add name=admin password=secret", "routeros_plain"),
        ("snmp-server community public RO", "snmp_community"),
    ] {
        assert_eq!(secrets::identify(input).kind, kind, "{input}");
    }
    let policy = secrets::PasswordPolicy {
        length: 20,
        upper: true,
        lower: true,
        digits: true,
        symbols: true,
        avoid_ambiguous: true,
    };
    let password = secrets::generate_password(&policy, &[7; 32]).unwrap();
    assert_eq!(
        password,
        secrets::generate_password(&policy, &[7; 32]).unwrap(),
        "deterministic for a given seed"
    );
    assert_eq!(password.len(), 20);

    // convert
    for input in [
        "0xC0A80101",
        "3232235777",
        "0b11000000101010000000000100000001",
        "192.168.1.1",
    ] {
        assert_eq!(convert::number(input).unwrap().hex, "0xC0A80101", "{input}");
    }
    assert_eq!(convert::parse_size_bytes("700 MB"), Ok(700_000_000));
    assert_eq!(convert::parse_size_bytes("4.7GiB"), Ok(5_046_586_573));
    for (input, bps) in [
        ("1G", 1_000_000_000),
        ("10 Gbps", 10_000_000_000),
        ("100Mb/s", 100_000_000),
        ("9600", 9_600),
        ("1.5G", 1_500_000_000),
    ] {
        assert_eq!(convert::parse_rate_bps(input), Ok(bps), "{input}");
    }
    assert_eq!(convert::format_rate(1.25e9), "1.25 Gbps");
    assert_eq!(convert::format_bytes(4.7e9), "4.38 GiB");
    let duration = 2 * 604_800 + 3 * 86_400 + 4 * 3_600 + 5 * 60;
    assert_eq!(convert::format_duration(duration), "2w 3d 4h 5m");
    for (input, seconds) in [
        ("2w3d", 17 * 86_400),
        ("1 day, 4 hours, 5 minutes", 86_400 + 4 * 3_600 + 300),
        ("3d04h", 3 * 86_400 + 4 * 3_600),
        ("12:34:56", 45_296),
    ] {
        assert_eq!(convert::parse_duration_s(input), Some(seconds), "{input}");
    }
    let every_encapsulation = [
        "dot1q",
        "qinq",
        "mpls",
        "gre",
        "gre-key",
        "ipsec-esp-tunnel",
        "vxlan",
        "geneve",
        "pppoe",
        "l2tp",
        "wireguard",
        "ipip",
        "6in4",
    ]
    .map(s);
    let stacked = convert::mtu(9000, &every_encapsulation).unwrap();
    assert_eq!(stacked.breakdown.len(), 13);
    assert_eq!(stacked.overhead, 9000 - stacked.payload_mtu);
    assert_eq!(
        (stacked.tcp_mss_v4, stacked.tcp_mss_v6),
        (stacked.payload_mtu - 40, stacked.payload_mtu - 60)
    );

    // reference
    let ports = reference::ports("").len();
    assert!(
        (200..=300).contains(&ports),
        "about 250 ports, not all of IANA: {ports}"
    );
    for query in ["ef", "46", "0xb8"] {
        assert_eq!(reference::dscp(query).unwrap().name, "EF", "{query}");
    }
    assert_eq!(reference::dscp("af41").unwrap().dscp, 34);
    for (short, long) in [
        ("Gi1/0/1", "GigabitEthernet1/0/1"),
        ("Te1/1/1", "TenGigabitEthernet1/1/1"),
        ("Po1", "Port-channel1"),
        ("Vl10", "Vlan10"),
        ("Lo0", "Loopback0"),
        ("Fa0/1", "FastEthernet0/1"),
        ("Eth1/1", "Ethernet1/1"),
        ("Hu1/0/1", "HundredGigE1/0/1"),
    ] {
        assert_eq!(reference::interface_long_name(short), long);
        assert_eq!(reference::interface_short_name(long), short);
    }
    assert!(reference::interface_sort_key("Gi1/0/2") < reference::interface_sort_key("Gi1/0/10"));
    assert_eq!(
        reference::ethernet_standards().first().unwrap().standard,
        "10BASE5"
    );
    assert!(
        reference::ethernet_standards()
            .iter()
            .any(|entry| entry.standard.starts_with("400GBASE-"))
    );
    assert_eq!(reference::syslog_levels().len(), 8);

    // mac
    for notation in [
        "aa:bb:cc:dd:ee:ff",
        "AA-BB-CC-DD-EE-FF",
        "aabb.ccdd.eeff",
        "aabbccddeeff",
        "AABBCCDDEEFF",
    ] {
        assert_eq!(
            mac::info(notation).unwrap().bare,
            "aabbccddeeff",
            "{notation}"
        );
    }
    assert_eq!(mac::vendor("00:00:0c").as_deref(), Some("Cisco Systems"));
    let (assignments, date) = mac::registry_info();
    assert!(assignments > 30_000 && date.starts_with("20"));

    // text
    assert_eq!(
        text::base64_decode(" Zm9v\nYmE ").unwrap(),
        b"fooba",
        "tolerant of whitespace and missing padding"
    );
    assert_eq!(text::hashes(b"abc").md5, "900150983cd24fb0d6963f7d28e17f72");
}
