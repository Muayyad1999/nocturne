//! The TCP/UDP ports a network engineer actually meets.

use serde::{Deserialize, Serialize};

/// One well-known port.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PortEntry {
    /// Port number.
    pub port: u16,
    /// Transport: `tcp`, `udp`, `tcp/udp` or `tcp/sctp`.
    pub protocol: String,
    /// Short service name, e.g. `bgp`.
    pub name: String,
    /// What runs there.
    pub description: String,
}

/// `(port, transport, name, description)`, sorted by port. Names are the
/// IANA service names where the registry has one for the use described, and
/// the name people use otherwise (vendor defaults, pre-IANA ports).
#[rustfmt::skip]
const PORTS: &[(u16, &str, &str, &str)] = &[
    (7, "tcp/udp", "echo", "Echo (a small server; should be off on routers)"),
    (9, "udp", "discard", "Discard; also the usual destination port of Wake-on-LAN magic packets"),
    (13, "tcp/udp", "daytime", "Daytime (a small server; should be off on routers)"),
    (17, "tcp/udp", "qotd", "Quote of the Day (a small server; used in amplification attacks)"),
    (19, "tcp/udp", "chargen", "Character generator (a small server; used in amplification attacks)"),
    (20, "tcp", "ftp-data", "FTP data connection in active mode"),
    (21, "tcp", "ftp", "FTP control"),
    (22, "tcp", "ssh", "SSH, SCP and SFTP"),
    (23, "tcp", "telnet", "Telnet (clear text)"),
    (25, "tcp", "smtp", "SMTP mail transfer between servers"),
    (37, "tcp/udp", "time", "Time protocol (RFC 868), the predecessor of NTP"),
    (43, "tcp", "nicname", "WHOIS"),
    (49, "tcp", "tacacs", "TACACS+"),
    (53, "tcp/udp", "domain", "DNS: UDP for queries, TCP for zone transfers and large answers"),
    (67, "udp", "bootps", "DHCP and BOOTP server (and relay agents)"),
    (68, "udp", "bootpc", "DHCP and BOOTP client"),
    (69, "udp", "tftp", "TFTP: image and configuration transfers, PXE boot"),
    (79, "tcp", "finger", "Finger"),
    (80, "tcp", "http", "HTTP"),
    (88, "tcp/udp", "kerberos", "Kerberos authentication"),
    (102, "tcp", "iso-tsap", "ISO transport over TCP: Siemens S7 PLCs, IEC 61850 MMS"),
    (104, "tcp", "acr-nema", "DICOM medical imaging"),
    (110, "tcp", "pop3", "POP3 mail retrieval"),
    (111, "tcp/udp", "sunrpc", "ONC RPC portmapper (rpcbind), used by NFS"),
    (113, "tcp", "ident", "Ident (RFC 1413)"),
    (123, "udp", "ntp", "NTP"),
    (135, "tcp", "epmap", "Microsoft RPC endpoint mapper"),
    (137, "udp", "netbios-ns", "NetBIOS name service"),
    (138, "udp", "netbios-dgm", "NetBIOS datagram service"),
    (139, "tcp", "netbios-ssn", "NetBIOS session service (SMB over NetBIOS)"),
    (143, "tcp", "imap", "IMAP mail access"),
    (161, "udp", "snmp", "SNMP requests to an agent"),
    (162, "udp", "snmptrap", "SNMP traps and informs to a manager"),
    (179, "tcp", "bgp", "BGP"),
    (319, "udp", "ptp-event", "PTP (IEEE 1588) event messages"),
    (320, "udp", "ptp-general", "PTP (IEEE 1588) general messages"),
    (323, "tcp", "rpki-rtr", "RPKI to Router protocol"),
    (389, "tcp/udp", "ldap", "LDAP (UDP for connectionless LDAP)"),
    (427, "tcp/udp", "svrloc", "Service Location Protocol (SLP)"),
    (443, "tcp/udp", "https", "HTTPS; HTTP/3 over QUIC on UDP; also RESTCONF and most SSL VPNs"),
    (445, "tcp", "microsoft-ds", "SMB file sharing (CIFS)"),
    (464, "tcp/udp", "kpasswd", "Kerberos password change"),
    (465, "tcp", "submissions", "SMTP mail submission over implicit TLS"),
    (496, "udp", "pim-rp-disc", "PIM Auto-RP announcements and discovery"),
    (500, "udp", "isakmp", "IKE, the IPsec key exchange"),
    (502, "tcp", "mbap", "Modbus/TCP industrial control"),
    (512, "tcp", "exec", "rexec remote execution"),
    (513, "tcp", "login", "rlogin"),
    (514, "tcp", "shell", "rsh remote shell and rcp; also syslog over plain TCP on many platforms"),
    (514, "udp", "syslog", "Syslog"),
    (515, "tcp", "printer", "LPD/LPR printing"),
    (520, "udp", "router", "RIP versions 1 and 2"),
    (521, "udp", "ripng", "RIPng"),
    (541, "tcp", "fgfm", "FortiGate to FortiManager management tunnel (FGFM)"),
    (546, "udp", "dhcpv6-client", "DHCPv6 client"),
    (547, "udp", "dhcpv6-server", "DHCPv6 server and relay agents"),
    (554, "tcp", "rtsp", "RTSP: IP cameras and streaming"),
    (587, "tcp", "submission", "SMTP mail submission (STARTTLS)"),
    (601, "tcp", "syslog-conn", "Reliable syslog over TCP (RFC 3195)"),
    (623, "udp", "asf-rmcp", "IPMI / RMCP out-of-band server management"),
    (631, "tcp", "ipp", "Internet Printing Protocol (CUPS)"),
    (636, "tcp", "ldaps", "LDAP over TLS"),
    (639, "tcp", "msdp", "Multicast Source Discovery Protocol"),
    (646, "tcp/udp", "ldp", "MPLS Label Distribution Protocol: UDP hellos, TCP sessions"),
    (647, "tcp", "dhcp-failover", "DHCP failover between servers"),
    (705, "tcp", "agentx", "AgentX: SNMP master agent to sub-agents"),
    (711, "tcp/udp", "cisco-tdp", "Cisco Tag Distribution Protocol, the predecessor of LDP"),
    (830, "tcp", "netconf-ssh", "NETCONF over SSH"),
    (848, "udp", "gdoi", "GDOI group key management (Cisco GETVPN)"),
    (853, "tcp/udp", "domain-s", "DNS over TLS; DNS over QUIC on UDP"),
    (861, "tcp", "owamp-control", "OWAMP control (one-way active measurement)"),
    (862, "tcp", "twamp-control", "TWAMP control (two-way active measurement)"),
    (873, "tcp", "rsync", "rsync"),
    (902, "tcp", "vmware-auth", "VMware ESXi host management and remote console"),
    (953, "tcp", "rndc", "BIND name server control (rndc)"),
    (989, "tcp", "ftps-data", "FTP data over implicit TLS"),
    (990, "tcp", "ftps", "FTP control over implicit TLS"),
    (993, "tcp", "imaps", "IMAP over TLS"),
    (995, "tcp", "pop3s", "POP3 over TLS"),
    (1080, "tcp", "socks", "SOCKS proxy"),
    (1194, "tcp/udp", "openvpn", "OpenVPN (UDP by default)"),
    (1433, "tcp", "ms-sql-s", "Microsoft SQL Server"),
    (1434, "udp", "ms-sql-m", "Microsoft SQL Server browser"),
    (1494, "tcp", "ica", "Citrix ICA remote desktop"),
    (1521, "tcp", "oracle-tns", "Oracle database listener"),
    (1645, "udp", "radius-old", "RADIUS authentication on its pre-IANA port (still the default on older Cisco IOS)"),
    (1646, "udp", "radacct-old", "RADIUS accounting on its pre-IANA port"),
    (1688, "tcp", "kms", "Microsoft KMS volume activation"),
    (1700, "udp", "radius-coa-cisco", "RADIUS change of authorization on Cisco's default port"),
    (1701, "udp", "l2tp", "L2TP"),
    (1719, "udp", "h323gatestat", "H.323 gatekeeper registration (RAS)"),
    (1720, "tcp", "h323hostcall", "H.323 call signalling (H.225)"),
    (1723, "tcp", "pptp", "PPTP control; the data travels as GRE (protocol 47)"),
    (1812, "udp", "radius", "RADIUS authentication"),
    (1813, "udp", "radius-acct", "RADIUS accounting"),
    (1883, "tcp", "mqtt", "MQTT"),
    (1900, "udp", "ssdp", "SSDP, the discovery protocol of UPnP"),
    (1967, "udp", "ip-sla", "Cisco IP SLA responder control"),
    (1985, "udp", "hsrp", "HSRP for IPv4 (version 1 to 224.0.0.2, version 2 to 224.0.0.102)"),
    (2000, "tcp", "cisco-sccp", "Cisco SCCP (Skinny) phone signalling; also the MikroTik bandwidth test server"),
    (2001, "tcp", "reverse-telnet", "Cisco terminal server reverse Telnet: port 2000 + line number (4000 + line for raw TCP)"),
    (2029, "udp", "hsrpv6", "HSRP for IPv6"),
    (2048, "udp", "wccp", "WCCP web cache redirection"),
    (2049, "tcp/udp", "nfs", "NFS"),
    (2055, "udp", "netflow", "NetFlow export (the most common collector port)"),
    (2083, "tcp", "radsec", "RADIUS over TLS (RadSec)"),
    (2123, "udp", "gtp-control", "GTP control plane (mobile core)"),
    (2152, "udp", "gtp-user", "GTP user plane (mobile core)"),
    (2404, "tcp", "iec-104", "IEC 60870-5-104 telecontrol (SCADA)"),
    (2427, "udp", "mgcp-gateway", "MGCP media gateway"),
    (2598, "tcp", "citrix-cgp", "Citrix ICA with session reliability (CGP)"),
    (2727, "udp", "mgcp-callagent", "MGCP call agent"),
    (3000, "tcp", "grafana", "Grafana (default)"),
    (3128, "tcp", "squid", "Squid HTTP proxy (default)"),
    (3222, "udp", "glbp", "GLBP"),
    (3260, "tcp", "iscsi-target", "iSCSI"),
    (3268, "tcp", "msft-gc", "Active Directory global catalog (LDAP)"),
    (3269, "tcp", "msft-gc-ssl", "Active Directory global catalog over TLS"),
    (3306, "tcp", "mysql", "MySQL and MariaDB"),
    (3389, "tcp/udp", "ms-wbt-server", "Remote Desktop (RDP)"),
    (3478, "tcp/udp", "stun", "STUN and TURN: NAT traversal for VoIP, WebRTC and conferencing"),
    (3503, "udp", "lsp-ping", "MPLS LSP ping and traceroute"),
    (3544, "udp", "teredo", "Teredo IPv6 tunnelling"),
    (3702, "udp", "ws-discovery", "WS-Discovery (printers, cameras, Windows devices)"),
    (3784, "udp", "bfd-control", "BFD control, single hop"),
    (3785, "udp", "bfd-echo", "BFD echo"),
    (3799, "udp", "radius-dynauth", "RADIUS dynamic authorization: CoA and Disconnect (RFC 5176)"),
    (3868, "tcp/sctp", "diameter", "Diameter AAA"),
    (3978, "tcp", "panorama", "Palo Alto firewall to Panorama management"),
    (4011, "udp", "altserviceboot", "PXE boot server"),
    (4172, "tcp/udp", "pcoip", "PCoIP remote desktop (VMware Horizon, Teradici)"),
    (4189, "tcp", "pcep", "Path Computation Element Protocol"),
    (4317, "tcp", "opentelemetry", "OpenTelemetry protocol (OTLP) over gRPC"),
    (4334, "tcp", "netconf-ch-ssh", "NETCONF call home over SSH"),
    (4335, "tcp", "netconf-ch-tls", "NETCONF call home over TLS"),
    (4341, "udp", "lisp-data", "LISP data plane"),
    (4342, "udp", "lisp-control", "LISP control plane"),
    (4343, "tcp", "aruba-webui", "ArubaOS controller web interface"),
    (4500, "udp", "ipsec-nat-t", "IPsec NAT traversal: IKE and ESP inside UDP"),
    (4569, "udp", "iax", "IAX2 (Asterisk trunks)"),
    (4739, "tcp/udp", "ipfix", "IPFIX flow export"),
    (4754, "udp", "gre-in-udp", "GRE in UDP (RFC 8086)"),
    (4784, "udp", "bfd-multi-ctl", "BFD control, multihop"),
    (4786, "tcp", "smart-install", "Cisco Smart Install (a known attack surface; disable with `no vstack`)"),
    (4789, "udp", "vxlan", "VXLAN"),
    (4790, "udp", "vxlan-gpe", "VXLAN-GPE"),
    (4840, "tcp", "opcua-tcp", "OPC UA industrial automation"),
    (5001, "tcp/udp", "iperf", "iperf 2 (default)"),
    (5060, "tcp/udp", "sip", "SIP signalling"),
    (5061, "tcp", "sips", "SIP over TLS"),
    (5201, "tcp/udp", "iperf3", "iperf 3 (default)"),
    (5222, "tcp", "xmpp-client", "XMPP client connections"),
    (5246, "udp", "capwap-control", "CAPWAP control between access points and a wireless controller"),
    (5247, "udp", "capwap-data", "CAPWAP data"),
    (5349, "tcp", "stuns", "STUN and TURN over TLS"),
    (5351, "udp", "pcp", "Port Control Protocol and its predecessor NAT-PMP"),
    (5353, "udp", "mdns", "Multicast DNS (Bonjour, Avahi)"),
    (5355, "udp", "llmnr", "Link-Local Multicast Name Resolution"),
    (5432, "tcp", "postgresql", "PostgreSQL"),
    (5601, "tcp", "kibana", "Kibana (default)"),
    (5665, "tcp", "icinga2", "Icinga 2 API and cluster"),
    (5666, "tcp", "nrpe", "Nagios NRPE"),
    (5672, "tcp", "amqp", "AMQP (RabbitMQ)"),
    (5678, "udp", "mndp", "MikroTik Neighbor Discovery Protocol"),
    (5900, "tcp", "rfb", "VNC remote desktop"),
    (5985, "tcp", "wsman", "Windows Remote Management over HTTP"),
    (5986, "tcp", "wsmans", "Windows Remote Management over HTTPS"),
    (6000, "tcp", "x11", "X Window System"),
    (6030, "tcp", "gnmi-arista", "gNMI on Arista EOS (default)"),
    (6080, "udp", "gue", "Generic UDP Encapsulation"),
    (6081, "udp", "geneve", "Geneve"),
    (6343, "udp", "sflow", "sFlow"),
    (6379, "tcp", "redis", "Redis"),
    (6443, "tcp", "kubernetes-api", "Kubernetes API server"),
    (6513, "tcp", "netconf-tls", "NETCONF over TLS"),
    (6514, "tcp", "syslog-tls", "Syslog over TLS (RFC 5425)"),
    (6633, "tcp", "openflow-old", "OpenFlow on its pre-IANA port"),
    (6635, "udp", "mpls-udp", "MPLS in UDP (RFC 7510)"),
    (6640, "tcp", "ovsdb", "Open vSwitch database management"),
    (6653, "tcp", "openflow", "OpenFlow"),
    (6667, "tcp", "irc", "IRC"),
    (6784, "udp", "bfd-lag", "BFD on LAG member links (micro-BFD, RFC 7130)"),
    (7547, "tcp", "cwmp", "TR-069 / CWMP management of customer premises equipment"),
    (7680, "tcp", "ms-do", "Windows Update Delivery Optimization (peer to peer)"),
    (7784, "udp", "s-bfd", "Seamless BFD"),
    (8080, "tcp", "http-alt", "HTTP alternate: proxies, application servers, UniFi device inform"),
    (8086, "tcp", "influxdb", "InfluxDB HTTP API"),
    (8088, "tcp", "splunk-hec", "Splunk HTTP Event Collector"),
    (8089, "tcp", "splunk-mgmt", "Splunk management"),
    (8291, "tcp", "winbox", "MikroTik WinBox"),
    (8443, "tcp", "https-alt", "HTTPS alternate: controllers and management interfaces"),
    (8472, "udp", "otv", "Cisco OTV; also the Linux kernel's legacy default port for VXLAN"),
    (8728, "tcp", "routeros-api", "MikroTik RouterOS API"),
    (8729, "tcp", "routeros-api-ssl", "MikroTik RouterOS API over TLS"),
    (8883, "tcp", "secure-mqtt", "MQTT over TLS"),
    (9090, "tcp", "prometheus", "Prometheus server (default)"),
    (9100, "tcp", "pdl-datastream", "Raw printing (JetDirect); also the Prometheus node exporter"),
    (9115, "tcp", "blackbox-exporter", "Prometheus blackbox exporter (default)"),
    (9116, "tcp", "snmp-exporter", "Prometheus SNMP exporter (default)"),
    (9200, "tcp", "elasticsearch", "Elasticsearch HTTP API"),
    (9339, "tcp", "gnmi-gnoi", "gNMI and gNOI (the IANA-assigned port)"),
    (9340, "tcp", "gribi", "gRIBI"),
    (9995, "udp", "netflow-alt", "NetFlow export (common alternative)"),
    (9996, "udp", "netflow-alt", "NetFlow export (common alternative)"),
    (9997, "tcp", "splunk-fwd", "Splunk forwarder to indexer"),
    (10000, "tcp", "ndmp", "NDMP backup; also Webmin"),
    (10001, "udp", "ubnt-discovery", "Ubiquiti device discovery"),
    (10050, "tcp", "zabbix-agent", "Zabbix agent"),
    (10051, "tcp", "zabbix-trapper", "Zabbix server"),
    (10161, "tcp/udp", "snmptls", "SNMP over TLS and DTLS"),
    (11211, "tcp/udp", "memcache", "memcached (an amplification vector when exposed over UDP)"),
    (12222, "udp", "lwapp-data", "LWAPP data (legacy Cisco lightweight access points)"),
    (12223, "udp", "lwapp-control", "LWAPP control (legacy Cisco lightweight access points)"),
    (16384, "udp", "rtp", "RTP media: the first port of the 16384-32767 range Cisco voice uses"),
    (16666, "udp", "wlc-mobility", "Cisco wireless controller mobility messaging"),
    (20000, "tcp/udp", "dnp", "DNP3 (utility SCADA)"),
    (20561, "udp", "mac-telnet", "MikroTik MAC-Telnet and MAC-WinBox"),
    (27017, "tcp", "mongodb", "MongoDB"),
    (32767, "tcp", "grpc-junos", "gRPC / gNMI on Junos (default)"),
    (33434, "udp", "traceroute", "Traceroute: the first of its default UDP destination ports"),
    (44818, "tcp/udp", "ethernet-ip", "EtherNet/IP explicit messaging (industrial)"),
    (47808, "udp", "bacnet", "BACnet/IP building automation"),
    (50051, "tcp", "grpc", "gRPC default; gNMI on Cisco NX-OS"),
    (51820, "udp", "wireguard", "WireGuard (conventional default)"),
    (57400, "tcp", "gnmi", "gNMI / gRPC on Nokia SR OS, SR Linux and Cisco IOS XR (default)"),
];

/// Look up well-known ports.
///
/// The query can be a port number (`179`), a port with a transport
/// (`53/udp`, `tcp/443`), a range (`8000-9000`), a transport on its own
/// (`udp`) or any text, which is matched case-insensitively against names
/// and descriptions (`bgp`, `radius`, `mikrotik`). An empty query lists the
/// whole table. Results are in port order.
pub fn ports(query: &str) -> Vec<PortEntry> {
    // Longer than any description, so it cannot match; do not even lower-case it.
    if query.len() > 256 {
        return Vec::new();
    }
    let query = query.trim().to_ascii_lowercase();
    let is_transport = |text: &str| matches!(text, "tcp" | "udp" | "sctp");
    let selected: Vec<&(u16, &str, &str, &str)> = if query.is_empty() {
        PORTS.iter().collect()
    } else if let Ok(port) = query.parse::<u16>() {
        PORTS.iter().filter(|entry| entry.0 == port).collect()
    } else if query.bytes().all(|b| b.is_ascii_digit()) {
        // A number beyond 65535 is not a port.
        Vec::new()
    } else if is_transport(&query) {
        PORTS
            .iter()
            .filter(|entry| entry.1.split('/').any(|transport| transport == query))
            .collect()
    } else if let Some((port, transport)) = port_and_transport(&query, is_transport) {
        PORTS
            .iter()
            .filter(|entry| {
                entry.0 == port && entry.1.split('/').any(|candidate| candidate == transport)
            })
            .collect()
    } else if let Some((first, last)) = query.split_once('-').and_then(|(first, last)| {
        Some((
            first.trim().parse::<u16>().ok()?,
            last.trim().parse::<u16>().ok()?,
        ))
    }) {
        PORTS
            .iter()
            .filter(|entry| (first..=last).contains(&entry.0))
            .collect()
    } else {
        PORTS
            .iter()
            .filter(|entry| {
                entry.2.contains(&query) || entry.3.to_ascii_lowercase().contains(&query)
            })
            .collect()
    };
    selected
        .into_iter()
        .map(|(port, protocol, name, description)| PortEntry {
            port: *port,
            protocol: (*protocol).to_owned(),
            name: (*name).to_owned(),
            description: (*description).to_owned(),
        })
        .collect()
}

/// `53/udp` or `udp/53`.
fn port_and_transport(query: &str, is_transport: impl Fn(&str) -> bool) -> Option<(u16, &str)> {
    let (left, right) = query.split_once('/')?;
    let (left, right) = (left.trim(), right.trim());
    match (left.parse::<u16>(), right.parse::<u16>()) {
        (Ok(port), _) if is_transport(right) => Some((port, right)),
        (_, Ok(port)) if is_transport(left) => Some((port, left)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(query: &str) -> Vec<String> {
        ports(query)
            .into_iter()
            .map(|entry| format!("{}/{} {}", entry.port, entry.protocol, entry.name))
            .collect()
    }

    #[test]
    fn lookup_by_number() {
        assert_eq!(names("179"), ["179/tcp bgp"]);
        assert_eq!(names(" 22 "), ["22/tcp ssh"]);
        assert_eq!(names("514"), ["514/tcp shell", "514/udp syslog"]);
        assert_eq!(names("53"), ["53/tcp/udp domain"]);
        // An exact number never matches longer numbers or text.
        assert_eq!(names("80"), ["80/tcp http"]);
        assert_eq!(names("4"), Vec::<String>::new());
        assert_eq!(names("65536"), Vec::<String>::new());
        assert_eq!(names("99999999999999999999"), Vec::<String>::new());
    }

    #[test]
    fn lookup_by_number_and_transport() {
        assert_eq!(names("514/udp"), ["514/udp syslog"]);
        assert_eq!(names("tcp/514"), ["514/tcp shell"]);
        assert_eq!(names("53/udp"), ["53/tcp/udp domain"]);
        assert_eq!(names("53 / TCP"), ["53/tcp/udp domain"]);
        assert_eq!(names("3868/sctp"), ["3868/tcp/sctp diameter"]);
        assert_eq!(names("179/udp"), Vec::<String>::new());
    }

    #[test]
    fn lookup_by_range_and_transport() {
        assert_eq!(
            names("3784-3799"),
            [
                "3784/udp bfd-control",
                "3785/udp bfd-echo",
                "3799/udp radius-dynauth"
            ]
        );
        assert_eq!(names("9-7"), Vec::<String>::new());
        let udp = ports("udp");
        assert!(udp.len() > 60 && udp.iter().all(|entry| entry.protocol.contains("udp")));
        assert!(ports("sctp").iter().any(|entry| entry.name == "diameter"));
    }

    #[test]
    fn lookup_by_text() {
        assert_eq!(names("bgp"), ["179/tcp bgp"]);
        assert_eq!(names("RADIUS").len(), 7);
        assert!(names("mikrotik").contains(&"8291/tcp winbox".to_owned()));
        assert!(names("vxlan").contains(&"4789/udp vxlan".to_owned()));
        assert!(names("netconf").len() >= 4);
        assert_eq!(names("no such service"), Vec::<String>::new());
        assert_eq!(names(&"bgp".repeat(1000)), Vec::<String>::new());
    }

    #[test]
    fn the_table_is_complete_and_tidy() {
        let all = ports("");
        assert_eq!(all.len(), PORTS.len());
        assert!((200..=300).contains(&all.len()), "{} entries", all.len());
        // Sorted by port (then transport), no duplicates.
        assert!(
            PORTS
                .windows(2)
                .all(|pair| (pair[0].0, pair[0].1) < (pair[1].0, pair[1].1))
        );
        for (port, protocol, name, description) in PORTS {
            assert!(*port > 0);
            assert!(
                matches!(*protocol, "tcp" | "udp" | "tcp/udp" | "tcp/sctp"),
                "{port} {protocol}"
            );
            assert!(
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{name}"
            );
            assert!(
                !description.is_empty() && !description.ends_with('.'),
                "{description}"
            );
        }
    }

    #[test]
    fn the_ports_everyone_knows() {
        for (port, name) in [
            (20, "ftp-data"),
            (21, "ftp"),
            (22, "ssh"),
            (23, "telnet"),
            (25, "smtp"),
            (49, "tacacs"),
            (67, "bootps"),
            (69, "tftp"),
            (80, "http"),
            (123, "ntp"),
            (161, "snmp"),
            (162, "snmptrap"),
            (179, "bgp"),
            (389, "ldap"),
            (443, "https"),
            (500, "isakmp"),
            (520, "router"),
            (646, "ldp"),
            (830, "netconf-ssh"),
            (1812, "radius"),
            (1813, "radius-acct"),
            (1985, "hsrp"),
            (3784, "bfd-control"),
            (4500, "ipsec-nat-t"),
            (4789, "vxlan"),
            (6081, "geneve"),
            (8291, "winbox"),
        ] {
            assert_eq!(ports(&port.to_string())[0].name, name, "{port}");
        }
    }
}
