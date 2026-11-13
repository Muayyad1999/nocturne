//! Global configuration mode.

use crate::catalog::Role;
use crate::device::cli::LineId;
use crate::device::cli::grammar::Args;
use crate::device::config::{
    Acl, DhcpPool, IfConfig, NatPool, NatProto, NatRule, NatStatic, NatTarget, OspfConfig, RipConfig, Secret, Slot,
    SnmpCommunity, StaticRoute, StpMode, SwMode, Switchport, Timestamps, Timezone, User, Vlan, VtpMode, can_create,
};
use crate::device::ios::{Cx, LineSel, Mode, Table, c, n};
use crate::ifname::{IfId, IfKind};
use crate::util::crypt;
use crate::util::net::{Prefix, len_from_mask};
use crate::util::rng;
use crate::util::text::parse_ranges;

pub(crate) fn table(role: Role) -> Table {
    let mut t: Table = vec![
        n("hostname ~ <word:name;This system's network name>", "Set system's network name", hostname),
        n("enable secret ~ [0 | 5] <word:secret;The ENABLE secret>", "Assign the privileged level secret", enable_secret),
        n("enable password ~ [0 | 7] <word:password;The ENABLE password>", "Assign the privileged level password", enable_password),
        n("service password-encryption", "Encrypt system passwords", service_encryption),
        n("service timestamps (debug | log) ~ [uptime | datetime [msec]]", "Timestamp debug/log messages", service_timestamps),
        c("version <word:version>", "Version of the saved configuration", |_, _| {}),
        c("boot-start-marker", "", |_, _| {}),
        c("boot-end-marker", "", |_, _| {}),
        n("ip domain-lookup", "Enable IP Domain Name System hostname translation", domain_lookup),
        n("ip domain lookup", "Enable IP Domain Name System hostname translation", domain_lookup),
        n("ip domain-name ~ <word:domain;Default domain name>", "Define the default domain name", domain_name),
        n("ip domain name ~ <word:domain;Default domain name>", "Define the default domain name", domain_name),
        n("ip name-server ~ <ip:server;Domain server IP address> {<ip:server>}", "Specify address of name server to use", name_server),
        n(
            "username <word:name;User name> ~ {privilege <0-15:level;User privilege level> | (secret | password) [0 | 5 | 7] <word:secret;The user's password>}",
            "Establish User Name Authentication",
            username,
        ),
        n("banner (motd | login) ~ <line:text;c banner-text c, where 'c' is a delimiting character>", "Define a login banner", banner),
        c("line (con | console) 0", "Configure a terminal line", |cx, _| cx.sh.mode = Mode::Line(LineSel::Console)),
        c("line vty <0-15:first;First Line number> [<0-15:last;Last Line number>]", "Configure a terminal line", line_vty),
        c("crypto key generate rsa {modulus <360-4096:bits;size of the key modulus> | general-keys}", "Generate RSA keys", crypto_key),
        c("crypto key zeroize rsa", "Remove RSA keys", |cx, _| {
            cx.n.rsa_bits = None;
            cx.say("% All keys will be removed.");
        }),
        n("ip ssh version ~ <1-2:version>", "Specify protocol version to be supported", |cx, a| {
            cx.cfg().ssh_version = if a.no { None } else { a.num("version").map(|v| v as u8) };
        }),
        n("ip default-gateway ~ <ip:gateway;IP address of default gateway>", "Specify default gateway (if not routing IP)", |cx, a| {
            cx.cfg().default_gateway = if a.no { None } else { a.ip("gateway") };
        }),
        n("ip default-network <ip:network;IP address of default network>", "Flags networks as candidates for default routes", default_network),
        n("ip access-list (standard | extended) <word:name;Access-list name>", "Named access-list", ip_access_list),
        n("access-list <1-2699:number;IP access list> ~ <line:entry>", "Add an access list entry", numbered_acl),
        n("ip dhcp excluded-address <ip:low;Low IP address> [<ip:high;High IP address>]", "Prevent DHCP from assigning certain addresses", dhcp_excluded),
        n("ip dhcp pool <word:name;Pool name>", "Configure DHCP address pools", dhcp_pool),
        n("logging host <ip:host;IP address of the syslog server>", "Set syslog server IP address and parameters", logging_host),
        n("logging <ip:host;IP address of the logging host>", "Modify message logging facilities", logging_host),
        n("logging buffered ~ [<4096-2147483647:size;Logging buffer size>]", "Set buffered logging parameters", |cx, a| {
            cx.cfg().logging.buffered = if a.no { None } else { Some(a.num("size").unwrap_or(4096) as u32) };
        }),
        n("logging console", "Set console logging parameters", |cx, a| cx.cfg().logging.console = !a.no),
        n("logging trap ~ <word:level;Logging severity level>", "Set syslog server logging level", |cx, a| {
            cx.cfg().logging.trap = if a.no { None } else { a.text("level").map(str::to_string) };
        }),
        n("ntp server <ip:server;IP address of peer>", "Configure NTP server", ntp_server),
        n("clock timezone ~ <word:zone;name of time zone> <word:hours;Hours offset from UTC> [<0-59:minutes;Minutes offset from UTC>]", "Configure time zone", clock_timezone),
        n("snmp-server community <word:name;SNMP community string> ~ [ro | rw] [<word:acl;Access-list name>]", "Enable SNMP; set community string and access privs", snmp_community),
        n("snmp-server location ~ <line:text;The physical location of this node>", "Text for mib object sysLocation", |cx, a| {
            cx.cfg().snmp.location = if a.no { None } else { a.text("text").map(str::to_string) };
        }),
        n("snmp-server contact ~ <line:text;identification of the contact person for this managed node>", "Text for mib object sysContact", |cx, a| {
            cx.cfg().snmp.contact = if a.no { None } else { a.text("text").map(str::to_string) };
        }),
        n("snmp-server host <line:rest>", "Specify hosts to receive SNMP notifications", |cx, a| snmp_stored(cx, a, "snmp-server host")),
        n("snmp-server enable traps ~ [<line:rest>]", "Enable SNMP Traps", |cx, a| snmp_stored(cx, a, "snmp-server enable traps")),
        n("cdp run", "Enable CDP", |cx, a| cx.cfg().cdp_run = !a.no),
        n("lldp run", "Enable LLDP", |cx, a| cx.cfg().lldp_run = !a.no),
        n("ip http server", "Enable http server", |cx, a| stored(cx, a, Slot::Http, "ip http server".to_string())),
        n("ip http secure-server", "Enable HTTP secure server", |cx, a| stored(cx, a, Slot::Http, "ip http secure-server".to_string())),
        n("aaa new-model", "Enable NEW access control commands and functions", |cx, a| {
            if a.no {
                cx.cfg().stored.retain(|(slot, _)| *slot != Slot::Aaa);
            } else {
                stored(cx, a, Slot::Aaa, "aaa new-model".to_string());
            }
        }),
        n("aaa <line:rest>", "Authentication, Authorization and Accounting", |cx, a| {
            let line = format!("aaa {}", a.text("rest").unwrap_or(""));
            if !a.no && !cx.n.cfg.stored.iter().any(|(_, l)| l == "aaa new-model") {
                return cx.invalid();
            }
            stored(cx, a, Slot::Aaa, line);
        }),
        n("archive", "Archive the configuration", |cx, a| stored(cx, a, Slot::Archive, "archive".to_string())),
        n("errdisable recovery <line:rest>", "Error disable recovery", |cx, a| {
            let line = format!("errdisable recovery {}", a.text("rest").unwrap_or(""));
            stored(cx, a, Slot::Errdisable, line);
        }),
        n("interface <if>", "Select an interface to configure", interface),
        c("interface range <line:ports;Interface range>", "interface range command", interface_range),
        c("default interface <if>", "Set a command to its defaults", default_interface),
        c("end", "Exit from configure mode", end),
        c("exit", "Exit from configure mode", end),
    ];
    if role != Role::L2Switch {
        t.extend(vec![
            n("ip routing", "Enable IP routing", |cx, a| cx.cfg().ip_routing = !a.no),
            n(
                "ip route <ip:prefix;Destination prefix> <ip:mask;Destination prefix mask> ~ (<ip:via;Forwarding router's address> | <if> [<ip:via>]) [<1-255:distance;Distance metric for this route>]",
                "Establish static routes",
                ip_route,
            ),
            n("router ospf <1-65535:pid;Process ID>", "Open Shortest Path First (OSPF)", router_ospf),
            n("router rip", "Routing Information Protocol (RIP)", router_rip),
            n("ip nat inside source static <ip:local;Inside local IP address> <ip:global;Inside global IP address>", "Specify static local->global mapping", nat_static),
            n(
                "ip nat inside source static (tcp | udp) <ip:local> <1-65535:lport;Local UDP/TCP port> <ip:global> <1-65535:gport;Global UDP/TCP port>",
                "Specify static local->global mapping",
                nat_static,
            ),
            n(
                "ip nat inside source list <word:acl;Access list name or number for local addresses> (interface <if> | pool <word:pool;Name pool of global addresses>) [overload]",
                "Specify access list describing local addresses",
                nat_rule,
            ),
            n(
                "ip nat pool <word:name;Pool name> ~ <ip:start;Start IP address> <ip:end;End IP address> (netmask <ip:mask;Network mask> | prefix-length <1-32:len;Prefix length>)",
                "Define pool of addresses",
                nat_pool,
            ),
        ]);
    }
    if role != Role::Router {
        t.extend(vec![
            n("vlan <word:list;VLAN IDs 1-4094, e.g. 10,20,30-35>", "Vlan commands", vlan),
            n("vtp mode ~ (server | client | transparent)", "Configure VTP device mode", vtp_mode),
            n("vtp domain ~ <word:name;The ascii name for the VTP administrative domain>", "Set the name of the VTP administrative domain", |cx, a| {
                cx.cfg().vtp.domain = if a.no { String::new() } else { a.text("name").unwrap_or("").to_string() };
            }),
            n("spanning-tree mode ~ (pvst | rapid-pvst)", "Spanning tree operating mode", |cx, a| {
                cx.cfg().stp.mode = if a.has("rapid-pvst") && !a.no { StpMode::RapidPvst } else { StpMode::Pvst };
            }),
            n("spanning-tree vlan <word:vlans;vlan range, example: 1,3-5,7,9-11> priority ~ <0-61440:priority;bridge priority in increments of 4096>", "VLAN Switch Spanning Tree", stp_priority),
            c("spanning-tree vlan <word:vlans> root (primary | secondary)", "Configure switch as root", stp_root),
            n("spanning-tree portfast [edge] default", "Enable portfast by default on all access ports", |cx, a| {
                cx.cfg().stp.portfast_default = !a.no;
            }),
            n("spanning-tree portfast [edge] bpduguard default", "Enable bpdu guard by default on all portfast ports", |cx, a| {
                cx.cfg().stp.bpduguard_default = !a.no;
            }),
            c("spanning-tree extend system-id", "Spanning Tree 802.1t extensions", |_, _| {}),
            n("ip dhcp snooping ~ [<line:rest>]", "DHCP Snooping", |cx, a| {
                let line = match a.text("rest") {
                    Some(rest) => format!("ip dhcp snooping {rest}"),
                    None => "ip dhcp snooping".to_string(),
                };
                stored(cx, a, Slot::IpEarly, line);
            }),
        ]);
    }
    t
}

// ------------------------------------------------------------------ helpers

/// Add or (with `no`) remove an accepted-and-stored line.
fn stored(cx: &mut Cx, a: &Args, slot: Slot, line: String) {
    let list = &mut cx.cfg().stored;
    list.retain(|(_, l)| *l != line);
    if !a.no {
        list.push((slot, line));
    }
}

fn snmp_stored(cx: &mut Cx, a: &Args, prefix: &str) {
    let line = match a.text("rest") {
        Some(rest) => format!("{prefix} {rest}"),
        None => prefix.to_string(),
    };
    let list = &mut cx.cfg().snmp.stored;
    if a.no && a.text("rest").is_none() {
        list.retain(|l| !l.starts_with(prefix));
        return;
    }
    list.retain(|l| *l != line);
    if !a.no {
        list.push(line);
    }
}

/// Hash a password the way "secret" passwords are stored.
pub(crate) fn hash_secret(cx: &mut Cx, plain: &str) -> Secret {
    let dice = rng::hash(&[cx.fx.seed, u64::from(cx.n.id), u64::from(cx.n.fresh_ident()), cx.fx.now]);
    let salt: String = (0..4).map(|i| crypt::salt_char(dice >> (i * 6))).collect();
    Secret::Md5(crypt::md5_crypt(plain, &salt))
}

/// Store a clear-text password, obfuscated if `service password-encryption` is on.
pub(crate) fn store_password(cx: &mut Cx, plain: &str) -> Secret {
    if cx.n.cfg.services.password_encryption {
        let seed = (rng::hash(&[cx.fx.seed, u64::from(cx.n.id), plain.len() as u64]) % 16) as u8;
        Secret::Type7(crypt::type7_encode(plain, seed))
    } else {
        Secret::Plain(plain.to_string())
    }
}

/// Interpret `[0|5|7] <word>` as typed after `secret` / `password`.
pub(crate) fn typed_secret(cx: &mut Cx, a: &Args, text: &str, is_secret: bool) -> Option<Secret> {
    if a.has("5") {
        if !text.starts_with("$1$") {
            cx.fail("ERROR: The secret you entered is not a valid encrypted secret.");
            return None;
        }
        return Some(Secret::Md5(text.to_string()));
    }
    if a.has("7") {
        if crypt::type7_decode(text).is_none() {
            cx.fail("Invalid encrypted password");
            return None;
        }
        return Some(Secret::Type7(text.to_string()));
    }
    Some(if is_secret { hash_secret(cx, text) } else { store_password(cx, text) })
}

/// Persist the VLAN database the way a switch writes `vlan.dat` on every change.
pub(crate) fn sync_vlan_dat(cx: &mut Cx) {
    cx.n.vlan_dat = Some(cx.n.cfg.vlans.clone());
}

fn end(cx: &mut Cx, _: &Args) {
    let reply = cx.sh.leave_config(cx.n, cx.fx);
    cx.out.push_str(&reply.out);
}

// ----------------------------------------------------------------- commands

fn hostname(cx: &mut Cx, a: &Args) {
    if a.no {
        let default = if cx.n.model.is_switch() { "Switch" } else { "Router" };
        cx.cfg().hostname = default.to_string();
        return;
    }
    let name = a.text("name").unwrap_or("");
    let valid = name.len() <= 63
        && name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if !valid {
        return cx.fail("% Hostname contains one or more illegal characters.");
    }
    cx.cfg().hostname = name.to_string();
}

fn enable_secret(cx: &mut Cx, a: &Args) {
    if a.no {
        cx.cfg().enable_secret = None;
        return;
    }
    let text = a.text("secret").unwrap_or("").to_string();
    if let Some(secret) = typed_secret(cx, a, &text, true) {
        cx.cfg().enable_secret = Some(secret);
    }
}

fn enable_password(cx: &mut Cx, a: &Args) {
    if a.no {
        cx.cfg().enable_password = None;
        return;
    }
    let text = a.text("password").unwrap_or("").to_string();
    if let Some(secret) = typed_secret(cx, a, &text, false) {
        cx.cfg().enable_password = Some(secret);
    }
}

fn service_encryption(cx: &mut Cx, a: &Args) {
    cx.cfg().services.password_encryption = !a.no;
    if a.no {
        return;
    }
    // Existing clear-text passwords are obfuscated in place.
    let seed = (rng::hash(&[cx.fx.seed, u64::from(cx.n.id)]) % 16) as u8;
    let encrypt = |secret: &mut Option<Secret>| {
        if let Some(Secret::Plain(plain)) = secret {
            *secret = Some(Secret::Type7(crypt::type7_encode(plain, seed)));
        }
    };
    let cfg = cx.cfg();
    encrypt(&mut cfg.enable_password);
    encrypt(&mut cfg.console.password);
    cfg.vty.iter_mut().for_each(|line| encrypt(&mut line.password));
    cfg.users.iter_mut().for_each(|user| encrypt(&mut user.secret));
}

fn service_timestamps(cx: &mut Cx, a: &Args) {
    let value = if a.no {
        Timestamps::Off
    } else if a.has("uptime") {
        Timestamps::Uptime
    } else {
        // `service timestamps log` alone means datetime without milliseconds.
        Timestamps::Datetime { msec: a.has("msec") }
    };
    let services = &mut cx.cfg().services;
    if a.has("debug") {
        services.timestamps_debug = value;
    } else {
        services.timestamps_log = value;
    }
}

fn domain_lookup(cx: &mut Cx, a: &Args) {
    cx.cfg().domain_lookup = !a.no;
}

fn domain_name(cx: &mut Cx, a: &Args) {
    cx.cfg().domain_name = if a.no { None } else { a.text("domain").map(str::to_string) };
}

fn name_server(cx: &mut Cx, a: &Args) {
    let servers = a.ips("server");
    let list = &mut cx.cfg().name_servers;
    if a.no {
        if servers.is_empty() {
            list.clear();
        } else {
            list.retain(|s| !servers.contains(s));
        }
        return;
    }
    for server in servers {
        if !list.contains(&server) && list.len() < 6 {
            list.push(server);
        }
    }
}

fn username(cx: &mut Cx, a: &Args) {
    let name = a.text("name").unwrap_or("").to_string();
    if a.no {
        cx.cfg().users.retain(|u| u.name != name);
        return;
    }
    let secret = match a.text("secret").map(str::to_string) {
        Some(text) => match typed_secret(cx, a, &text, a.has("secret")) {
            Some(secret) => Some(secret),
            None => return,
        },
        None => None,
    };
    let privilege = a.num("level").map(|l| l as u8);
    let users = &mut cx.cfg().users;
    match users.iter_mut().find(|u| u.name == name) {
        Some(user) => {
            if let Some(level) = privilege {
                user.privilege = level;
            }
            if secret.is_some() {
                user.secret = secret;
            }
        }
        None => users.push(User { name, privilege: privilege.unwrap_or(1), secret }),
    }
}

fn banner(cx: &mut Cx, a: &Args) {
    let login = a.has("login");
    if a.no {
        let cfg = cx.cfg();
        if login {
            cfg.banner_login = None;
        } else {
            cfg.banner_motd = None;
        }
        return;
    }
    let reply = cx.sh.begin_banner(cx.n, login, a.text("text").unwrap_or(""));
    cx.out.push_str(&reply.out);
    cx.touched |= reply.touched;
}

fn line_vty(cx: &mut Cx, a: &Args) {
    let first = a.num("first").unwrap_or(0) as u8;
    let last = a.num("last").map_or(first, |l| l as u8);
    let lines = cx.n.cfg.vty.len() as u8;
    if first > last || last >= lines {
        return cx.invalid();
    }
    cx.sh.mode = Mode::Line(LineSel::Vty(first, last));
}

fn crypto_key(cx: &mut Cx, a: &Args) {
    if cx.n.cfg.domain_name.is_none() {
        return cx.fail("% Please define a domain-name first.");
    }
    let bits = a.num("bits").unwrap_or(1024) as u16;
    let name = format!("{}.{}", cx.n.cfg.hostname, cx.n.cfg.domain_name.clone().unwrap_or_default());
    cx.say(format!("The name for the keys will be: {name}"));
    cx.say(format!("% The key modulus size is {bits} bits"));
    cx.say(format!("% Generating {bits} bit RSA keys, keys will be non-exportable...[OK]"));
    cx.n.rsa_bits = Some(bits);
    cx.n.log(cx.fx, 5, "SSH", "ENABLED", "SSH 1.99 has been enabled".to_string());
}

fn default_network(cx: &mut Cx, a: &Args) {
    let Some(network) = a.ip("network") else { return };
    let list = &mut cx.cfg().default_networks;
    list.retain(|n| *n != network);
    if !a.no {
        list.push(network);
    }
}

fn ip_access_list(cx: &mut Cx, a: &Args) {
    let name = a.text("name").unwrap_or("").to_string();
    let extended = a.has("extended");
    if a.no {
        cx.cfg().acls.retain(|acl| acl.name != name);
        cx.n.acl_hits.retain(|(list, _), _| *list != name);
        return;
    }
    // A number used as a name must be in the right range for its type.
    if let Ok(number) = name.parse::<u32>()
        && Acl::is_extended_number(number) != Some(extended)
    {
        return cx.invalid();
    }
    match cx.n.cfg.acl(&name).map(|acl| acl.extended) {
        Some(existing) if existing != extended => {
            return cx.fail(format!(
                "%Access-list type conflicts with prior definition\n% A named {} ip access list with this name already exists",
                if existing { "extended" } else { "standard" }
            ));
        }
        Some(_) => {}
        None => cx.cfg().acls.push(Acl { name: name.clone(), extended, items: Vec::new() }),
    }
    cx.sh.mode = if extended { Mode::AclExt(name) } else { Mode::AclStd(name) };
}

fn numbered_acl(cx: &mut Cx, a: &Args) {
    let number = a.num("number").unwrap_or(0) as u32;
    let Some(extended) = Acl::is_extended_number(number) else { return cx.invalid() };
    let name = number.to_string();
    let Some(entry) = a.text("entry").map(str::to_string) else {
        // `no access-list N` removes the whole list.
        cx.cfg().acls.retain(|acl| acl.name != name);
        cx.n.acl_hits.retain(|(list, _), _| *list != name);
        return;
    };
    if cx.n.cfg.acl(&name).is_none() {
        cx.cfg().acls.push(Acl { name: name.clone(), extended, items: Vec::new() });
    }
    crate::device::ios::run_acl_entry(cx, extended, &name, &entry, a.no);
    // An entry that failed to parse must not leave an empty list behind.
    if cx.error {
        cx.n.cfg.acls.retain(|acl| acl.name != name || !acl.items.is_empty());
    }
}

fn dhcp_excluded(cx: &mut Cx, a: &Args) {
    let Some(low) = a.ip("low") else { return };
    let high = a.ip("high").unwrap_or(low);
    if u32::from(high) < u32::from(low) {
        return cx.fail("% Invalid address range");
    }
    let list = &mut cx.cfg().dhcp_excluded;
    list.retain(|range| *range != (low, high));
    if !a.no {
        list.push((low, high));
    }
}

fn dhcp_pool(cx: &mut Cx, a: &Args) {
    let name = a.text("name").unwrap_or("").to_string();
    if a.no {
        cx.cfg().dhcp_pools.retain(|p| p.name != name);
        return;
    }
    if !cx.n.cfg.dhcp_pools.iter().any(|p| p.name == name) {
        cx.cfg().dhcp_pools.push(DhcpPool::new(&name));
    }
    cx.sh.mode = Mode::DhcpPool(name);
}

fn logging_host(cx: &mut Cx, a: &Args) {
    let Some(host) = a.ip("host") else { return };
    let hosts = &mut cx.cfg().logging.hosts;
    hosts.retain(|h| *h != host);
    if !a.no {
        hosts.push(host);
    }
}

fn ntp_server(cx: &mut Cx, a: &Args) {
    let Some(server) = a.ip("server") else { return };
    let servers = &mut cx.cfg().ntp_servers;
    servers.retain(|s| *s != server);
    if !a.no {
        servers.push(server);
    }
}

fn clock_timezone(cx: &mut Cx, a: &Args) {
    if a.no {
        cx.cfg().clock_timezone = None;
        return;
    }
    let Some(hours) = a.text("hours").and_then(|h| h.parse::<i8>().ok()).filter(|h| (-23..=23).contains(h)) else {
        return cx.invalid();
    };
    cx.cfg().clock_timezone = Some(Timezone {
        name: a.text("zone").unwrap_or("UTC").to_string(),
        hours,
        minutes: a.num("minutes").unwrap_or(0) as u8,
    });
}

fn snmp_community(cx: &mut Cx, a: &Args) {
    let name = a.text("name").unwrap_or("").to_string();
    let list = &mut cx.cfg().snmp.communities;
    list.retain(|c| c.name != name);
    if !a.no {
        list.push(SnmpCommunity { name, rw: a.has("rw"), acl: a.text("acl").map(str::to_string) });
    }
}

fn new_logical(cx: &Cx, id: IfId) -> IfConfig {
    let mut cfg = IfConfig::new();
    if id.kind == IfKind::PortChannel && cx.n.model.is_switch() {
        cfg.l2 = Some(Switchport::new(SwMode::DynamicAuto));
    }
    cfg
}

fn interface(cx: &mut Cx, a: &Args) {
    let Some(id) = a.iface("if") else { return };
    let exists = cx.n.cfg.interfaces.contains_key(&id);
    if a.no {
        if id.is_physical() {
            return cx.fail("% Removal of physical interfaces is not permitted");
        }
        if !exists {
            return cx.invalid();
        }
        cx.cfg().interfaces.remove(&id);
        // Members of a deleted port-channel leave the channel.
        if id.kind == IfKind::PortChannel {
            for cfg in cx.n.cfg.interfaces.values_mut() {
                if cfg.channel_group.is_some_and(|(g, _)| g == id.number()) {
                    cfg.channel_group = None;
                }
            }
        }
        return;
    }
    if !exists {
        if !can_create(cx.n.model, id) {
            return cx.invalid();
        }
        let cfg = new_logical(cx, id);
        cx.cfg().interfaces.insert(id, cfg);
    }
    cx.sh.mode = Mode::If(vec![id]);
}

/// `fa0/1 - 4 , gi0/1 - 2`, with or without the spaces.
fn parse_range(spec: &str) -> Option<Vec<IfId>> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let compact: String = part.chars().filter(|c| !c.is_whitespace()).collect();
        let (first, last) = match compact.rsplit_once('-') {
            // "Port-channel1" contains a dash that is not a range separator.
            Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) && IfId::parse(head).is_some() => {
                (IfId::parse(head)?, tail.parse::<u32>().ok()?)
            }
            _ => {
                let id = IfId::parse(&compact)?;
                (id, id.port_number())
            }
        };
        if last < first.port_number() || last - first.port_number() > 255 {
            return None;
        }
        out.extend((first.port_number()..=last).map(|n| first.with_port_number(n)));
    }
    out.sort();
    out.dedup();
    (!out.is_empty()).then_some(out)
}

fn interface_range(cx: &mut Cx, a: &Args) {
    let Some(ids) = parse_range(a.text("ports").unwrap_or("")) else { return cx.invalid() };
    for id in &ids {
        if cx.n.cfg.interfaces.contains_key(id) {
            continue;
        }
        if !can_create(cx.n.model, *id) {
            return cx.invalid();
        }
        let cfg = new_logical(cx, *id);
        cx.cfg().interfaces.insert(*id, cfg);
    }
    cx.sh.mode = Mode::If(ids);
}

fn default_interface(cx: &mut Cx, a: &Args) {
    let Some(id) = a.iface("if") else { return };
    if !cx.n.cfg.interfaces.contains_key(&id) {
        return cx.invalid();
    }
    let fresh = if id.is_physical() {
        crate::device::config::DeviceConfig::factory(cx.n.model, "").interfaces.remove(&id).map(|mut cfg| {
            // Defaulting a port leaves it enabled.
            cfg.shutdown = false;
            cfg
        })
    } else {
        Some(new_logical(cx, id))
    };
    if let Some(fresh) = fresh {
        cx.cfg().interfaces.insert(id, fresh);
        cx.say(format!("Interface {} set to default configuration", id.name()));
    }
}

fn ip_route(cx: &mut Cx, a: &Args) {
    let (Some(prefix), Some(mask)) = (a.ip("prefix"), a.ip("mask")) else { return };
    let Some(len) = len_from_mask(mask) else { return cx.fail("%Inconsistent address and mask") };
    let prefix_net = Prefix::new(prefix, len);
    if prefix_net.net != prefix {
        return cx.fail("%Inconsistent address and mask");
    }
    let (via, iface) = (a.ip("via"), a.iface("if"));
    if a.no {
        // Without a next hop every route to the prefix goes.
        cx.cfg().static_routes.retain(|r| {
            r.prefix != prefix_net || (via.is_some() || iface.is_some()) && (r.via, r.iface) != (via, iface)
        });
        return;
    }
    if let Some(iface) = iface
        && !cx.n.cfg.interfaces.contains_key(&iface)
    {
        return cx.invalid();
    }
    if via.is_some_and(|v| cx.n.cfg.interfaces.values().any(|c| c.static_addr().is_some_and(|addr| addr.ip == v))) {
        return cx.fail("%Invalid next hop address (it's this router)");
    }
    let route = StaticRoute { prefix: prefix_net, via, iface, distance: a.num("distance").unwrap_or(1) as u8 };
    let routes = &mut cx.cfg().static_routes;
    routes.retain(|r| (r.prefix, r.via, r.iface) != (route.prefix, route.via, route.iface));
    routes.push(route);
}

fn router_ospf(cx: &mut Cx, a: &Args) {
    let pid = a.num("pid").unwrap_or(1) as u16;
    if a.no {
        if cx.n.cfg.ospf.as_ref().is_some_and(|o| o.pid == pid) {
            cx.cfg().ospf = None;
        }
        return;
    }
    match cx.n.cfg.ospf.as_ref().map(|o| o.pid) {
        Some(existing) if existing != pid => {
            return cx.fail(format!("% Only one OSPF process is supported; process {existing} is already configured"));
        }
        Some(_) => {}
        None => {
            if !cx.n.cfg.ip_routing {
                return cx.fail("% IP routing not enabled");
            }
            cx.cfg().ospf = Some(OspfConfig::new(pid));
        }
    }
    cx.sh.mode = Mode::Ospf;
}

fn router_rip(cx: &mut Cx, a: &Args) {
    if a.no {
        cx.cfg().rip = None;
        return;
    }
    if cx.n.cfg.rip.is_none() {
        if !cx.n.cfg.ip_routing {
            return cx.fail("% IP routing not enabled");
        }
        cx.cfg().rip = Some(RipConfig::new());
    }
    cx.sh.mode = Mode::Rip;
}

fn nat_static(cx: &mut Cx, a: &Args) {
    let (Some(local), Some(global)) = (a.ip("local"), a.ip("global")) else { return };
    let ports = match (a.num("lport"), a.num("gport")) {
        (Some(l), Some(g)) => Some((if a.has("tcp") { NatProto::Tcp } else { NatProto::Udp }, l as u16, g as u16)),
        _ => None,
    };
    let entry = NatStatic { local, global, ports };
    let list = &mut cx.cfg().nat.statics;
    list.retain(|e| *e != entry);
    if !a.no {
        list.push(entry);
    }
}

fn nat_rule(cx: &mut Cx, a: &Args) {
    let acl = a.text("acl").unwrap_or("").to_string();
    let target = match (a.iface("if"), a.text("pool")) {
        (Some(iface), _) => NatTarget::Interface(iface),
        (None, Some(pool)) => NatTarget::Pool(pool.to_string()),
        (None, None) => return,
    };
    if let NatTarget::Pool(pool) = &target
        && !a.no
        && !cx.n.cfg.nat.pools.iter().any(|p| p.name == *pool)
    {
        return cx.fail(format!("%Pool {pool} not found"));
    }
    let list = &mut cx.cfg().nat.rules;
    list.retain(|r| r.acl != acl);
    if !a.no {
        list.push(NatRule { acl, target, overload: a.has("overload") });
    }
}

fn nat_pool(cx: &mut Cx, a: &Args) {
    let name = a.text("name").unwrap_or("").to_string();
    if a.no {
        if cx.n.cfg.nat.rules.iter().any(|r| r.target == NatTarget::Pool(name.clone())) {
            return cx.fail(format!("%Pool {name} in use, cannot destroy"));
        }
        cx.cfg().nat.pools.retain(|p| p.name != name);
        return;
    }
    let (Some(start), Some(end)) = (a.ip("start"), a.ip("end")) else { return };
    let prefix_len = match (a.ip("mask").map(len_from_mask), a.num("len")) {
        (Some(Some(len)), _) => len,
        (None, Some(len)) => len as u8,
        _ => return cx.fail("%Invalid netmask"),
    };
    if u32::from(end) < u32::from(start) || Prefix::new(start, prefix_len) != Prefix::new(end, prefix_len) {
        return cx.fail("%End address less than start address, or not in the same subnet");
    }
    let pools = &mut cx.cfg().nat.pools;
    pools.retain(|p| p.name != name);
    pools.push(NatPool { name, start, end, prefix_len, as_prefix: a.has("prefix-length") });
}

fn vlan(cx: &mut Cx, a: &Args) {
    let Some(ids) = parse_ranges(a.text("list").unwrap_or(""), 1, 4094) else { return cx.invalid() };
    if cx.n.cfg.vtp.mode == VtpMode::Client {
        return cx.fail("VTP VLAN configuration not allowed when device is in CLIENT mode.");
    }
    if let Some(reserved) = ids.iter().find(|id| (1002..=1005).contains(*id)) {
        return cx.fail(format!("%Default VLAN {reserved} may not be {} .", if a.no { "deleted" } else { "configured" }));
    }
    if a.no {
        if ids.contains(&1) {
            return cx.fail("%Default VLAN 1 may not be deleted.");
        }
        for id in ids {
            cx.cfg().vlans.remove(&id);
        }
        sync_vlan_dat(cx);
        return;
    }
    for id in &ids {
        if !cx.n.cfg.vlans.contains_key(id) {
            cx.cfg().vlans.insert(*id, Vlan::new(*id));
        }
    }
    sync_vlan_dat(cx);
    cx.sh.mode = Mode::Vlan(ids);
}

fn vtp_mode(cx: &mut Cx, a: &Args) {
    let mode = if a.no || a.has("server") {
        VtpMode::Server
    } else if a.has("client") {
        VtpMode::Client
    } else {
        VtpMode::Transparent
    };
    let word = match mode {
        VtpMode::Server => "SERVER",
        VtpMode::Client => "CLIENT",
        VtpMode::Transparent => "TRANSPARENT",
    };
    if cx.n.cfg.vtp.mode == mode {
        cx.say(format!("Device mode already VTP {word}."));
    } else {
        cx.cfg().vtp.mode = mode;
        cx.say(format!("Setting device to VTP {word} mode."));
    }
}

fn stp_priority(cx: &mut Cx, a: &Args) {
    let Some(vlans) = parse_ranges(a.text("vlans").unwrap_or(""), 1, 4094) else { return cx.invalid() };
    if a.no {
        for vlan in vlans {
            cx.cfg().stp.priority.remove(&vlan);
        }
        return;
    }
    let priority = a.num("priority").unwrap_or(32768) as u16;
    if priority % 4096 != 0 {
        cx.say("% Bridge Priority must be in increments of 4096.");
        cx.say("% Allowed values are:");
        cx.say("  0     4096  8192  12288 16384 20480 24576 28672");
        cx.fail("  32768 36864 40960 45056 49152 53248 57344 61440");
        return;
    }
    for vlan in vlans {
        if priority == 32768 {
            cx.cfg().stp.priority.remove(&vlan);
        } else {
            cx.cfg().stp.priority.insert(vlan, priority);
        }
    }
}

fn stp_root(cx: &mut Cx, a: &Args) {
    let Some(vlans) = parse_ranges(a.text("vlans").unwrap_or(""), 1, 4094) else { return cx.invalid() };
    for vlan in vlans {
        // The macro picks a priority that beats the current root where it can.
        let current_root = cx.n.stp.vlans.get(&vlan).map(|v| v.root.priority.saturating_sub(vlan));
        let own = cx.n.cfg.stp.priority_of(vlan);
        let priority = if a.has("secondary") {
            28672
        } else {
            match current_root {
                Some(root) if root < 24576 + 1 && root != own => root.saturating_sub(4096) / 4096 * 4096,
                _ => 24576,
            }
        };
        cx.cfg().stp.priority.insert(vlan, priority);
    }
}

/// Which line(s) a `line` sub-mode command applies to.
pub(crate) fn selected_lines(cx: &Cx) -> Vec<LineId> {
    match cx.sh.mode {
        Mode::Line(LineSel::Console) => vec![LineId::Console],
        Mode::Line(LineSel::Vty(first, last)) => (first..=last).map(LineId::Vty).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_ranges() {
        let names = |spec: &str| parse_range(spec).map(|ids| ids.iter().map(|i| i.short()).collect::<Vec<_>>().join(","));
        assert_eq!(names("fa0/1 - 4").as_deref(), Some("Fa0/1,Fa0/2,Fa0/3,Fa0/4"));
        assert_eq!(names("fa0/1-2,gi0/1").as_deref(), Some("Fa0/1,Fa0/2,Gi0/1"));
        assert_eq!(names("FastEthernet0/23 - 24 , GigabitEthernet0/1 - 2").as_deref(), Some("Fa0/23,Fa0/24,Gi0/1,Gi0/2"));
        assert_eq!(names("gi1/0/1-3").as_deref(), Some("Gi1/0/1,Gi1/0/2,Gi1/0/3"));
        assert_eq!(names("po1").as_deref(), Some("Po1"));
        assert_eq!(names("Port-channel1").as_deref(), Some("Po1"));
        assert_eq!(names("vlan 10 - 12").as_deref(), Some("Vl10,Vl11,Vl12"));
        assert_eq!(names("fa0/4 - 1"), None);
        assert_eq!(names("bogus"), None);
        assert_eq!(names("fa0/1 - 9999"), None);
    }
}
