//! "Where should I start?" — the networks this machine is itself attached to.
//!
//! The first useful thing discovery can say, before the user has typed
//! anything, is which networks are within reach. That is read from this
//! machine's interfaces and routing table: in the desktop app it is the
//! operator's workstation, and in server mode it is the server, which the
//! result says plainly rather than pretending to know the user's own
//! networks.

use nocturne_app::{App, AppResult, NoParams, Router};
use serde::Serialize;

use crate::db;
use crate::model::{SuggestedGateway, SuggestedNetwork};
use crate::settings::DiscoverySettings;

#[derive(Debug, Serialize)]
pub struct Suggestions {
    pub networks: Vec<SuggestedNetwork>,
    pub gateways: Vec<SuggestedGateway>,
    /// False in server mode: these are the server's networks, not the
    /// networks of the machine the browser is on.
    pub local: bool,
    /// The largest block a scan may cover under the current settings.
    pub max_hosts: u32,
}

pub async fn suggest(app: &App) -> AppResult<Suggestions> {
    let settings = DiscoverySettings::load(app);
    let probes = db::state(app).probes.get();
    let mut networks = Vec::new();
    for interface in probes.interfaces().await {
        if interface.is_loopback || !interface.is_up {
            continue;
        }
        for address in &interface.addresses {
            let Ok(net) = address.parse::<ipnet::IpNet>() else {
                continue;
            };
            let block = net.trunc();
            // Every network this machine is attached to is a plausible place
            // to start, whoever the addresses belong to — but only blocks
            // small enough to mean something: a /64 is not a sweep.
            let hosts = crate::targets::host_count(&block);
            if hosts == 0 {
                continue;
            }
            let cidr = block.to_string();
            if networks
                .iter()
                .any(|known: &SuggestedNetwork| known.cidr == cidr)
            {
                continue;
            }
            networks.push(SuggestedNetwork {
                needs_allow_public: !crate::targets::is_internal(block.addr()),
                cidr,
                interface: interface.name.clone(),
                hosts,
                address: net.addr().to_string(),
                too_large: hosts > u64::from(settings.max_hosts),
            });
        }
    }
    networks.sort_by(|a, b| a.hosts.cmp(&b.hosts).then(a.cidr.cmp(&b.cidr)));

    let gateways = probes
        .gateways()
        .await
        .into_iter()
        .filter(|route| !route.gateway.is_empty())
        .map(|route| SuggestedGateway {
            address: route.gateway,
            interface: route.interface,
            destination: route.destination,
        })
        .collect();

    Ok(Suggestions {
        networks,
        gateways,
        local: app.local_access(),
        max_hosts: settings.max_hosts,
    })
}

pub fn register(router: &mut Router) {
    router.add("discovery.suggest", |app: App, _: NoParams| async move {
        suggest(&app).await
    });
}
