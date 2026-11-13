//! End hosts: turning a `HostConfig` into the shared configuration model.

use std::net::Ipv4Addr;

use crate::api::{HostConfig, LabError};
use crate::catalog::{Model, Role};
use crate::device::config::{DeviceConfig, IfConfig, L3Addr, StaticRoute};
use crate::ifname::{IfId, IfKind};
use crate::sim::node::Node;
use crate::util::net::{IfAddr, Prefix, len_from_mask, parse_cidr, parse_ip};

pub(crate) const SERVICES: [&str; 8] = ["http", "dns", "dhcp", "tftp", "syslog", "ntp", "ssh", "telnet"];

/// `"8.8.8.8/32"`, `"8.8.8.8"` or `"203.0.113.1/30@Ethernet1"`.
fn parse_extra(entry: &str) -> Option<(IfAddr, Option<IfId>)> {
    let (addr, port) = match entry.split_once('@') {
        Some((addr, port)) => (addr, Some(IfId::parse(port)?)),
        None => (entry, None),
    };
    let (ip, len) = match parse_cidr(addr.trim()) {
        Some(parsed) => parsed,
        None => (parse_ip(addr.trim())?, 32),
    };
    Some((IfAddr::new(ip, len), port))
}

fn field(value: &Option<String>, what: &str) -> Result<Option<Ipv4Addr>, LabError> {
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(text) => parse_ip(text).map(Some).ok_or_else(|| LabError::InvalidAddress(format!("{what} \"{text}\""))),
    }
}

/// Prefix length for a host address: the mask if given, else the classful default.
fn prefix_len(ip: Ipv4Addr, mask: Option<Ipv4Addr>) -> Option<u8> {
    match mask {
        Some(mask) => len_from_mask(mask),
        None => Some(Prefix::classful(ip).len),
    }
}

pub(crate) fn validate(model: &Model, host: &HostConfig) -> Result<(), LabError> {
    let ip = field(&host.ip, "IP address")?;
    let mask = field(&host.mask, "subnet mask")?;
    field(&host.gateway, "default gateway")?;
    field(&host.dns, "DNS server")?;
    if let Some(ip) = ip
        && prefix_len(ip, mask).is_none()
    {
        return Err(LabError::InvalidAddress(format!("subnet mask \"{}\"", host.mask.clone().unwrap_or_default())));
    }
    for entry in &host.extra_ips {
        let (_, port) = parse_extra(entry).ok_or_else(|| LabError::InvalidAddress(format!("extra address \"{entry}\"")))?;
        if let Some(port) = port
            && !model.ports.contains(&port)
        {
            return Err(LabError::UnknownInterface { node: model.id.to_string(), interface: port.name() });
        }
    }
    match host.services.iter().find(|s| !SERVICES.contains(&s.to_ascii_lowercase().as_str())) {
        Some(bad) => Err(LabError::InvalidSpec(format!("unknown service \"{bad}\""))),
        None => Ok(()),
    }
}

/// Rebuild the node's configuration model from its host settings. Fields that
/// do not parse are skipped; `validate` rejects them before they are stored.
pub(crate) fn apply(node: &mut Node) {
    let host = node.host.clone().unwrap_or_default();
    let mut cfg = DeviceConfig::factory(node.model, &node.name);
    let parse = |v: &Option<String>| v.as_deref().and_then(|s| parse_ip(s.trim()));
    if let Some(port) = node.model.ports.first()
        && let Some(iface) = cfg.interfaces.get_mut(port)
    {
        if host.dhcp {
            iface.addr = Some(L3Addr::Dhcp);
        } else if let Some(ip) = parse(&host.ip)
            && let Some(len) = prefix_len(ip, parse(&host.mask))
        {
            iface.addr = Some(L3Addr::Static(IfAddr::new(ip, len)));
        }
    }
    if let Some(gateway) = parse(&host.gateway) {
        if node.model.role == Role::Cloud {
            cfg.static_routes.push(StaticRoute { prefix: Prefix::DEFAULT, via: Some(gateway), iface: None, distance: 1 });
        } else {
            cfg.default_gateway = Some(gateway);
        }
    }
    cfg.name_servers.extend(parse(&host.dns));
    let mut loopback = 0;
    for (addr, port) in host.extra_ips.iter().filter_map(|e| parse_extra(e)) {
        match port.and_then(|p| cfg.interfaces.get_mut(&p)) {
            Some(iface) if iface.addr.is_none() => iface.addr = Some(L3Addr::Static(addr)),
            Some(iface) => iface.secondary.push(addr),
            None => {
                let mut iface = IfConfig::new();
                iface.addr = Some(L3Addr::Static(addr));
                cfg.interfaces.insert(IfId::new(IfKind::Loopback, &[loopback]), iface);
                loopback += 1;
            }
        }
    }
    node.cfg = cfg;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::model;

    #[test]
    fn extra_addresses() {
        let (addr, port) = parse_extra("8.8.8.8/32").unwrap();
        assert_eq!((addr.to_string(), port), ("8.8.8.8/32".to_string(), None));
        let (addr, port) = parse_extra("203.0.113.1/30@Ethernet1").unwrap();
        assert_eq!((addr.to_string(), port.unwrap().name()), ("203.0.113.1/30".to_string(), "Ethernet1".to_string()));
        assert_eq!(parse_extra("1.1.1.1").unwrap().0.len, 32);
        assert!(parse_extra("nonsense").is_none());
    }

    #[test]
    fn validation() {
        let pc = model("pc").unwrap();
        let ok = HostConfig { ip: Some("192.168.1.10".into()), mask: Some("255.255.255.0".into()), ..Default::default() };
        assert!(validate(pc, &ok).is_ok());
        let bad_mask = HostConfig { mask: Some("255.0.255.0".into()), ..ok.clone() };
        assert!(validate(pc, &bad_mask).is_err());
        let bad_ip = HostConfig { ip: Some("300.1.1.1".into()), ..ok.clone() };
        assert!(validate(pc, &bad_ip).is_err());
        let bad_service = HostConfig { services: vec!["gopher".into()], ..ok.clone() };
        assert!(validate(pc, &bad_service).is_err());
        let bad_port = HostConfig { extra_ips: vec!["1.1.1.1/32@Ethernet3".into()], ..ok };
        assert!(validate(pc, &bad_port).is_err());
    }
}
