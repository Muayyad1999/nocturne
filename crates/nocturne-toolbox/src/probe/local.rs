//! `probe.local` and `probe.wol` — the machine the probes run from, and the
//! one packet that is sent to wake something up.
//!
//! `probe.local` is how a person checks what "from here" means before trusting
//! a result: which interfaces this machine has, which gateway it would use,
//! and who is in its neighbour table. In server mode it reports the server's,
//! and says so — that is the honest answer to "can the NOC reach it?".
//!
//! The neighbour table is labelled with the inventory on the way out, so a
//! staging laptop plugged into a switch can see "that MAC at 10.0.0.2 is SW1"
//! without opening another screen.

use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_netprobe::local::{self, LocalInterface, LocalRoute, Neighbour};
use nocturne_tools::mac;
use serde::{Deserialize, Serialize};

use crate::errors::probe as probe_error;
use crate::probe::history::{self, Run};
use crate::probe::known::{DeviceHint, Known};

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct WolParams {
    /// The MAC address to wake, in any notation.
    pub mac: String,
    /// The broadcast address to send to; left out, every broadcast address of
    /// this machine.
    pub broadcast: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LocalReport {
    /// `this computer` or `the Nocturne server`.
    pub from: &'static str,
    pub interfaces: Vec<LocalInterface>,
    pub gateways: Vec<LocalRoute>,
    pub neighbours: Vec<KnownNeighbour>,
}

#[derive(Debug, Serialize)]
pub struct KnownNeighbour {
    #[serde(flatten)]
    pub neighbour: Neighbour,
    /// The vendor of the MAC address, from the bundled IEEE registry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// The device of the inventory at that address, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
}

#[derive(Debug, Serialize)]
pub struct WolResult {
    /// The MAC address as it was understood, normalised.
    pub mac: String,
    /// The broadcast addresses the magic packet was sent to.
    pub sent_to: Vec<String>,
    pub from: &'static str,
    pub run_id: String,
}

async fn report(app: &App) -> AppResult<LocalReport> {
    let known = Known::load(app);
    let neighbours = local::neighbours()
        .await
        .into_iter()
        .map(|neighbour| KnownNeighbour {
            vendor: mac::vendor(&neighbour.mac),
            device: known.of_text(&neighbour.address),
            neighbour,
        })
        .collect();
    Ok(LocalReport {
        from: history::where_from(app),
        interfaces: local::interfaces(),
        gateways: local::default_gateways().await,
        neighbours,
    })
}

async fn wake(app: &App, params: WolParams) -> AppResult<WolResult> {
    // The engine takes the MAC as text; running it through the calculator
    // first means a typo is a clear message instead of a silent no-op.
    let address = mac::info(&params.mac).map_err(crate::errors::tool)?;
    if address.is_multicast {
        return Err(AppError::invalid(format!(
            "{} is a multicast address; Wake-on-LAN wakes one machine, so it needs that machine's own address",
            address.colon
        )));
    }
    let mut run = Run::plain(
        "wol",
        format!("Wake {}", address.colon),
        history::where_from(app),
        &address.colon,
    );
    history::begin(app, &run);

    match local::wake_on_lan(&address.colon, params.broadcast.as_deref())
        .await
        .map_err(probe_error)
    {
        Ok(sent_to) => {
            let result = WolResult {
                mac: address.colon.clone(),
                from: history::where_from(app),
                run_id: run.id.clone(),
                sent_to,
            };
            let summary = if result.sent_to.is_empty() {
                "sent to nothing: this machine has no broadcast address".to_string()
            } else {
                format!("sent to {}", result.sent_to.join(", "))
            };
            app.audit(
                "probe.wol",
                None,
                format!("Sent a Wake-on-LAN packet to {}", result.mac),
                serde_json::json!({"sent_to": result.sent_to, "vendor": address.vendor}),
            );
            history::succeeded(app, &mut run, summary, &result);
            Ok(result)
        }
        Err(error) => {
            history::failed(app, &mut run, &error);
            Err(error)
        }
    }
}

pub fn register(router: &mut Router) {
    router.add(
        "probe.local",
        |app: App, _: nocturne_app::NoParams| async move { report(&app).await },
    );
    router.add("probe.wol", |app: App, params: WolParams| async move {
        wake(&app, params).await
    });
}
