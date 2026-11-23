//! `tools.list` — what is in the drawer.
//!
//! The UI builds its tool list from this, and so does the assistant when it
//! wants to know what it can reach, so the catalogue is the one place that has
//! to stay honest about the surface. A test asserts both directions: every
//! entry is a method that exists, and every method this crate registers is an
//! entry. Drift is a failing test rather than a missing button.

use nocturne_app::{App, NoParams, Router};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// The RPC method name, which is also the tool's identifier.
    pub id: &'static str,
    pub title: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    /// `true` for arithmetic: no socket, no device, no clock, instant.
    pub offline: bool,
    /// `true` when the answer arrives as a job (`{job_id}`) rather than in the
    /// reply, so a caller knows to follow the events.
    pub job: bool,
}

const fn tool(
    id: &'static str,
    group: &'static str,
    title: &'static str,
    description: &'static str,
) -> Entry {
    Entry {
        id,
        title,
        group,
        description,
        offline: true,
        job: false,
    }
}

const fn live(
    id: &'static str,
    group: &'static str,
    title: &'static str,
    description: &'static str,
    job: bool,
) -> Entry {
    Entry {
        id,
        title,
        group,
        description,
        offline: false,
        job,
    }
}

/// Everything this crate offers, in the order a tool drawer would show it.
pub const CATALOGUE: &[Entry] = &[
    // ----------------------------------------------------------- addressing
    tool(
        "tools.subnet",
        "addressing",
        "Subnet calculator",
        "Network, broadcast, host range, counts, masks, class, scope and binary of an IPv4 or IPv6 prefix.",
    ),
    tool(
        "tools.subnet.split",
        "addressing",
        "Split a network",
        "Cut a prefix into equal subnets, by new prefix length or by how many are wanted.",
    ),
    tool(
        "tools.vlsm",
        "addressing",
        "VLSM planner",
        "Fit named host counts into a base network, largest first, correctly aligned.",
    ),
    tool(
        "tools.supernet",
        "addressing",
        "Summarise networks",
        "The smallest set of blocks covering exactly a list of networks, and the single supernet of them.",
    ),
    tool(
        "tools.contains",
        "addressing",
        "Is it inside?",
        "Whether a network contains an address, and whether two blocks overlap at all.",
    ),
    tool(
        "tools.overlaps",
        "addressing",
        "Find overlaps",
        "Which blocks in a list overlap one another — the first thing to check in an addressing plan.",
    ),
    tool(
        "tools.ip.range",
        "addressing",
        "Range to CIDR",
        "The minimal list of CIDR blocks covering a first-to-last address range.",
    ),
    tool(
        "tools.ip.convert",
        "addressing",
        "Address converter",
        "An address or number in every form: dotted, decimal, hex, binary, IPv6 expanded and compressed, reverse DNS.",
    ),
    tool(
        "tools.wildcard",
        "addressing",
        "Mask and wildcard",
        "Prefix length, netmask and wildcard mask, each from either of the others.",
    ),
    tool(
        "tools.wildcard.acl",
        "addressing",
        "ACL wildcard helper",
        "What an `address wildcard` pair matches, in words, and whether a given address is one of them.",
    ),
    // ------------------------------------------------------------- identity
    tool(
        "tools.mac",
        "identity",
        "MAC address",
        "Every vendor notation, the OUI vendor, unicast or multicast, local or global, and the SLAAC link-local address.",
    ),
    tool(
        "tools.oui",
        "identity",
        "OUI lookup",
        "A vendor from a MAC prefix, or the prefixes registered to a vendor, from the bundled IEEE registry.",
    ),
    tool(
        "tools.oid",
        "identity",
        "SNMP OID lookup",
        "An OID from an object name and a name from an OID, including notifications, from the built-in dictionary.",
    ),
    tool(
        "tools.interface",
        "identity",
        "Interface name",
        "The long and short spelling of an interface name (`Gi1/0/1` and `GigabitEthernet1/0/1`).",
    ),
    // -------------------------------------------------------------- secrets
    tool(
        "tools.secret.decode",
        "secrets",
        "Decode a vendor secret",
        "Recover a Cisco type 7 or Junos $9$ value: obfuscation, not encryption. Hashes are refused, by name.",
    ),
    tool(
        "tools.secret.encode",
        "secrets",
        "Encode a vendor secret",
        "Produce a type 7 or $9$ value — for comparing with a configuration, not for protecting anything.",
    ),
    tool(
        "tools.secret.identify",
        "secrets",
        "What kind of secret is this?",
        "Name the storage format of a secret in a configuration line, whether it is reversible, and what it is worth.",
    ),
    tool(
        "tools.password",
        "secrets",
        "Password generator",
        "Passwords from the operating system's random generator, in character classes a device CLI will accept.",
    ),
    // Not `offline`: generating a key is pure arithmetic, but it is seconds of
    // it for RSA, so a caller should not treat it as a keystroke calculator.
    live(
        "tools.keys.generate",
        "secrets",
        "SSH key pair",
        "An Ed25519 or RSA key pair in OpenSSH format, returned once and kept nowhere.",
        false,
    ),
    // ----------------------------------------------------------- converters
    tool(
        "tools.convert",
        "converters",
        "Unit converter",
        "Sizes, rates, durations and optical power (dBm and milliwatts), in every unit of the family.",
    ),
    tool(
        "tools.transfer_time",
        "converters",
        "Transfer time",
        "How long a file takes over a link, with protocol overhead.",
    ),
    tool(
        "tools.mtu",
        "converters",
        "MTU and MSS",
        "What a stack of encapsulations leaves of a link MTU, and the TCP MSS that follows.",
    ),
    // ------------------------------------------------------------------ qos
    tool(
        "tools.qos.marking",
        "qos",
        "QoS marking",
        "A DSCP value or name, a ToS byte or an IP precedence in every other form, with the ECN bits, the per-hop behaviour and what it is used for.",
    ),
    tool(
        "tools.qos.voice",
        "qos",
        "Voice bandwidth",
        "Bandwidth per call and in total for a codec, a number of calls and a link type, with compressed RTP and VAD, and the arithmetic shown.",
    ),
    // ------------------------------------------------------------ reference
    tool(
        "tools.reference",
        "reference",
        "Reference tables",
        "Ports, DSCP, protocol and EtherType numbers, ICMP types, administrative distances, optics, syslog levels, voice codecs and link overheads.",
    ),
    tool(
        "tools.reference.tables",
        "reference",
        "Which tables there are",
        "The reference tables available, with their size.",
    ),
    // ----------------------------------------------------------------- text
    tool(
        "tools.regex",
        "text",
        "Regex tester",
        "Matches and capture groups of a pattern against text — for writing profile and log patterns.",
    ),
    tool(
        "tools.diff",
        "text",
        "Text diff",
        "A line diff of two texts, the same one the configuration screens show.",
    ),
    tool(
        "tools.hash",
        "text",
        "Digests",
        "MD5, SHA-1, SHA-256 and SHA-512 of text or of Base64 bytes.",
    ),
    tool(
        "tools.base64",
        "text",
        "Base64",
        "Encode or decode Base64, saying so when the bytes are not text.",
    ),
    // --------------------------------------------------------------- probes
    live(
        "probe.ping",
        "probes",
        "Ping",
        "Echo requests from this machine, streamed as they come back. A count of 0 runs until cancelled.",
        true,
    ),
    live(
        "probe.traceroute",
        "probes",
        "Traceroute",
        "The path to a host, hop by hop, as each one is found.",
        true,
    ),
    live(
        "probe.ports",
        "probes",
        "Port check",
        "Which TCP ports are open, with the banner the server volunteers.",
        true,
    ),
    live(
        "probe.sweep",
        "probes",
        "Address sweep",
        "Which addresses in a range answer, with names, MAC addresses and the devices they belong to.",
        true,
    ),
    live(
        "probe.dns",
        "probes",
        "DNS lookup",
        "Any record type, from the system resolvers or a server you name.",
        false,
    ),
    live(
        "probe.whois",
        "probes",
        "WHOIS",
        "Who owns an address, a block, an AS number or a domain.",
        false,
    ),
    live(
        "probe.tls",
        "probes",
        "TLS certificate",
        "The certificate chain of a service, described even when it does not verify, with the days left.",
        false,
    ),
    live(
        "probe.http",
        "probes",
        "HTTP check",
        "Status, headers, redirects and the timing of each phase of a request.",
        false,
    ),
    live(
        "probe.ntp",
        "probes",
        "NTP query",
        "Stratum, reference, delay and how far a server's clock is from this one's.",
        false,
    ),
    live(
        "probe.local",
        "probes",
        "This machine's network",
        "The interfaces, default gateways and neighbour table of the machine the probes run from.",
        false,
    ),
    live(
        "probe.wol",
        "probes",
        "Wake on LAN",
        "Send a magic packet to a MAC address, and say which broadcast addresses it went to.",
        false,
    ),
    // --------------------------------------------------------------- device
    live(
        "probe.device.ping",
        "device",
        "Ping from a device",
        "Build the platform's own ping, run it through the read-only gate, and parse the summary.",
        false,
    ),
    live(
        "probe.device.traceroute",
        "device",
        "Traceroute from a device",
        "The path as the device sees it — usually the question that matters.",
        false,
    ),
    // -------------------------------------------------------------- history
    live(
        "probe.history",
        "history",
        "What was run",
        "The probes that were run, what came back, and whether they could run at all.",
        false,
    ),
    live(
        "probe.history.get",
        "history",
        "One run",
        "A single probe run with its full result.",
        false,
    ),
    live(
        "probe.history.delete",
        "history",
        "Forget runs",
        "Delete probe runs from the history.",
        false,
    ),
    // -------------------------------------------------------------- servers
    live(
        "tftp.start",
        "servers",
        "TFTP server",
        "Serve a directory to gear that speaks nothing else. Off unless asked; read-only unless told otherwise.",
        true,
    ),
    live(
        "tftp.stop",
        "servers",
        "Stop the TFTP server",
        "Stop serving and close the socket.",
        false,
    ),
    live(
        "tftp.status",
        "servers",
        "TFTP status",
        "Whether it is running, where it is bound, what it serves and the transfers so far.",
        false,
    ),
    live(
        "probe.syslog.start",
        "servers",
        "Staging syslog listener",
        "Listen for the messages of a device being staged. Nothing is stored: that is the logs feature's job.",
        true,
    ),
    live(
        "probe.syslog.stop",
        "servers",
        "Stop the syslog listener",
        "Stop listening and close the socket.",
        false,
    ),
    live(
        "probe.syslog.status",
        "servers",
        "Syslog listener status",
        "Whether it is listening, where, and how many messages have arrived.",
        false,
    ),
    live(
        "probe.syslog.messages",
        "servers",
        "Messages received",
        "The messages the staging listener has in memory, newest last.",
        false,
    ),
];

pub fn register(router: &mut Router) {
    router.add(
        "tools.list",
        |_: App, _: NoParams| async move { Ok(CATALOGUE) },
    );
}
