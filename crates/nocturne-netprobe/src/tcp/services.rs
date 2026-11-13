//! Port lists and service names for the TCP scanner.

/// The 100 TCP ports most often found open (the set `nmap --top-ports 100` scans), ascending.
pub(crate) const TOP_100: [u16; 100] = [
    7, 9, 13, 21, 22, 23, 25, 26, 37, 53, 79, 80, 81, 88, 106, 110, 111, 113, 119, 135, 139, 143,
    144, 179, 199, 389, 427, 443, 444, 445, 465, 513, 514, 515, 543, 544, 548, 554, 587, 631, 646,
    873, 990, 993, 995, 1025, 1026, 1027, 1028, 1029, 1110, 1433, 1720, 1723, 1755, 1900, 2000,
    2001, 2049, 2121, 2717, 3000, 3128, 3306, 3389, 3986, 4899, 5000, 5009, 5051, 5060, 5101, 5190,
    5357, 5432, 5631, 5666, 5800, 5900, 6000, 6001, 6646, 7070, 8000, 8008, 8009, 8080, 8081, 8443,
    8888, 9100, 9999, 10000, 32768, 49152, 49153, 49154, 49155, 49156, 49157,
];

/// What a network administrator looks for first: device management, routing, AAA, directory,
/// monitoring, storage and the usual application servers.
pub(crate) const COMMON: [u16; 72] = [
    21, 22, 23, 25, 49, 53, 80, 88, 110, 111, 135, 139, 143, 179, 389, 443, 445, 465, 514, 515,
    548, 554, 587, 623, 636, 646, 830, 873, 902, 993, 995, 1080, 1433, 1521, 1723, 1883, 2000,
    2049, 2222, 2375, 3128, 3260, 3306, 3389, 4786, 5000, 5060, 5061, 5432, 5900, 5985, 5986, 6379,
    6443, 7547, 8000, 8080, 8081, 8291, 8443, 8728, 8729, 8880, 8883, 9000, 9090, 9100, 9200,
    10000, 10050, 10051, 27017,
];

/// Well-known TCP services by port, ascending.
const SERVICES: &[(u16, &str)] = &[
    (7, "echo"),
    (9, "discard"),
    (13, "daytime"),
    (20, "ftp-data"),
    (21, "ftp"),
    (22, "ssh"),
    (23, "telnet"),
    (25, "smtp"),
    (26, "rsftp"),
    (37, "time"),
    (43, "whois"),
    (49, "tacacs"),
    (53, "domain"),
    (79, "finger"),
    (80, "http"),
    (81, "http"),
    (88, "kerberos"),
    (102, "iso-tsap"),
    (106, "pop3pw"),
    (110, "pop3"),
    (111, "rpcbind"),
    (113, "ident"),
    (119, "nntp"),
    (135, "msrpc"),
    (139, "netbios-ssn"),
    (143, "imap"),
    (144, "news"),
    (179, "bgp"),
    (199, "smux"),
    (389, "ldap"),
    (427, "svrloc"),
    (443, "https"),
    (444, "snpp"),
    (445, "microsoft-ds"),
    (465, "smtps"),
    (502, "modbus"),
    (513, "login"),
    (514, "shell"),
    (515, "printer"),
    (543, "klogin"),
    (544, "kshell"),
    (548, "afp"),
    (554, "rtsp"),
    (587, "submission"),
    (623, "ipmi"),
    (631, "ipp"),
    (636, "ldaps"),
    (646, "ldp"),
    (830, "netconf-ssh"),
    (853, "domain-s"),
    (873, "rsync"),
    (902, "vmware-auth"),
    (990, "ftps"),
    (992, "telnets"),
    (993, "imaps"),
    (995, "pop3s"),
    (1025, "msrpc"),
    (1026, "msrpc"),
    (1027, "msrpc"),
    (1028, "msrpc"),
    (1029, "msrpc"),
    (1080, "socks"),
    (1110, "nfsd-status"),
    (1194, "openvpn"),
    (1433, "ms-sql-s"),
    (1521, "oracle"),
    (1720, "h323q931"),
    (1723, "pptp"),
    (1755, "wms"),
    (1883, "mqtt"),
    (1900, "upnp"),
    (2000, "cisco-sccp"),
    (2001, "dc"),
    (2049, "nfs"),
    (2121, "ccproxy-ftp"),
    (2222, "ssh"),
    (2375, "docker"),
    (2376, "docker-s"),
    (2717, "pn-requester"),
    (3000, "http"),
    (3128, "http-proxy"),
    (3260, "iscsi"),
    (3306, "mysql"),
    (3389, "ms-wbt-server"),
    (3986, "mapper-ws_ethd"),
    (4786, "smart-install"),
    (4899, "radmin"),
    (5000, "upnp"),
    (5009, "airport-admin"),
    (5051, "ida-agent"),
    (5060, "sip"),
    (5061, "sips"),
    (5101, "admdog"),
    (5190, "aol"),
    (5357, "wsdapi"),
    (5432, "postgresql"),
    (5631, "pcanywheredata"),
    (5666, "nrpe"),
    (5800, "vnc-http"),
    (5900, "vnc"),
    (5985, "winrm"),
    (5986, "winrm-s"),
    (6000, "x11"),
    (6001, "x11"),
    (6379, "redis"),
    (6443, "kubernetes-api"),
    (6646, "unknown"),
    (7070, "realserver"),
    (7547, "cwmp"),
    (8000, "http"),
    (8008, "http"),
    (8009, "ajp13"),
    (8080, "http-proxy"),
    (8081, "http"),
    (8088, "http"),
    (8291, "winbox"),
    (8443, "https"),
    (8728, "routeros-api"),
    (8729, "routeros-api-s"),
    (8880, "http"),
    (8883, "mqtt-s"),
    (8888, "http"),
    (9000, "http"),
    (9090, "http"),
    (9100, "jetdirect"),
    (9200, "elasticsearch"),
    (9443, "https"),
    (9999, "abyss"),
    (10000, "webmin"),
    (10050, "zabbix-agent"),
    (10051, "zabbix-trapper"),
    (27017, "mongodb"),
    (32768, "filenet-tms"),
    (49152, "msrpc"),
    (49153, "msrpc"),
    (49154, "msrpc"),
    (49155, "msrpc"),
    (49156, "msrpc"),
    (49157, "msrpc"),
];

/// The usual service on `port`, or `""` when the port has no well-known use.
pub(crate) fn service_name(port: u16) -> &'static str {
    match SERVICES.binary_search_by_key(&port, |(port, _)| *port) {
        Ok(index) if SERVICES[index].1 != "unknown" => SERVICES[index].1,
        _ => "",
    }
}

/// The port a service name stands for (`ssh` -> 22). Names used for several ports give the lowest.
pub(crate) fn service_port(name: &str) -> Option<u16> {
    SERVICES
        .iter()
        .find(|(_, service)| service.eq_ignore_ascii_case(name) && *service != "unknown")
        .map(|(port, _)| *port)
}

/// Ports where a server speaks plain HTTP first, so a `HEAD` request is the way to identify it.
pub(crate) fn speaks_plain_http(port: u16) -> bool {
    matches!(service_name(port), "http" | "http-proxy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_and_unique() {
        assert!(TOP_100.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(COMMON.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(SERVICES.windows(2).all(|pair| pair[0].0 < pair[1].0));
        // Every listed port has a name to show next to it (6646 is the one nmap itself calls unknown).
        for port in TOP_100.iter().chain(COMMON.iter()) {
            assert!(
                SERVICES
                    .binary_search_by_key(port, |(port, _)| *port)
                    .is_ok(),
                "port {port} has no service entry"
            );
        }
    }

    #[test]
    fn names_and_ports() {
        assert_eq!(service_name(22), "ssh");
        assert_eq!(service_name(443), "https");
        assert_eq!(service_name(8291), "winbox");
        assert_eq!(service_name(6646), "");
        assert_eq!(service_name(12345), "");
        assert_eq!(service_port("SSH"), Some(22));
        assert_eq!(service_port("https"), Some(443));
        assert_eq!(service_port("http"), Some(80));
        assert_eq!(service_port("unknown"), None);
        assert_eq!(service_port("no-such-service"), None);
        assert!(speaks_plain_http(80) && speaks_plain_http(8080) && speaks_plain_http(3128));
        assert!(!speaks_plain_http(443) && !speaks_plain_http(22));
    }
}
