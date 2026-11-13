//! What must still be true after the change.
//!
//! The configuration is compared with the plan's own prediction — not with
//! "something changed", but with *exactly what was predicted* — and the
//! device's view of the network is compared with the one taken minutes
//! earlier over the same session: a neighbour that was there before must
//! still be there, an interface that was up must still be up unless the
//! change was meant to shut it down, and anything the operator asked the
//! device to be able to ping must still answer.
//!
//! A check that cannot be run (a platform that does not collect neighbours,
//! say) is reported as not run. It never passes by default, and it never
//! fails the change by itself.

use std::collections::BTreeSet;

use nocturne_app::App;
use nocturne_app::collect;
use nocturne_app::connect::Cli;
use nocturne_drivers::hier::{ConfigTree, Syntax};
use nocturne_drivers::ifname;
use nocturne_drivers::model::{Interface, Kind, Neighbor, Route};
use regex::Regex;
use std::sync::LazyLock;

use crate::compare;
use crate::model::{Check, Plan, Verify};

/// What the device looked like before the change.
#[derive(Clone, Debug, Default)]
pub struct Baseline {
    pub neighbors: Option<Vec<Neighbor>>,
    pub interfaces: Option<Vec<Interface>>,
    pub routes: Option<Vec<Route>>,
    /// Why a kind is missing, for the step detail.
    pub notes: Vec<String>,
}

fn wanted_kinds(verify: &Verify) -> Vec<Kind> {
    let mut kinds = Vec::new();
    if verify.neighbors {
        kinds.push(Kind::Neighbors);
    }
    if verify.interfaces {
        kinds.push(Kind::Interfaces);
    }
    if verify.routes {
        kinds.push(Kind::Routes);
    }
    kinds
}

/// Read what verification will need, over the session that is about to make
/// the change. Best effort: what cannot be read is noted, not fatal.
pub async fn snapshot(app: &App, cli: &mut Cli, verify: &Verify) -> Baseline {
    let kinds = wanted_kinds(verify);
    if kinds.is_empty() {
        return Baseline::default();
    }
    match collect::run_on(app, cli, &kinds).await {
        Ok(snapshot) => {
            let mut baseline = Baseline {
                neighbors: snapshot.neighbors.clone(),
                interfaces: snapshot.interfaces.clone(),
                routes: snapshot.routes.clone(),
                notes: Vec::new(),
            };
            for problem in &snapshot.problems {
                if kinds.contains(&problem.kind) {
                    baseline.notes.push(format!(
                        "{} could not be read before the change ({}), so that check is skipped",
                        problem.kind.as_str(),
                        problem.message
                    ));
                }
            }
            baseline
        }
        Err(err) => Baseline {
            notes: vec![format!(
                "nothing could be read before the change ({}), so the neighbour, interface and route checks are skipped",
                err.message
            )],
            ..Baseline::default()
        },
    }
}

/// Interfaces this change shuts down on purpose: they are allowed to go down.
fn shut_down_by(plan: &Plan, syntax: Syntax) -> BTreeSet<String> {
    crate::protect::locate(&plan.commands, syntax)
        .into_iter()
        .filter(|item| item.line == "shutdown")
        .flat_map(|item| item.sections.clone())
        .filter_map(|section| section.strip_prefix("interface ").map(str::to_string))
        .map(|name| ifname::long(&name).to_ascii_lowercase())
        .collect()
}

/// Run every check the options ask for. `before` is the configuration the
/// drift check confirmed the plan was computed against, `after` is the one
/// the device shows now (both already fetched by the caller).
pub async fn run_checks(
    app: &App,
    cli: &mut Cli,
    plan: &Plan,
    verify: &Verify,
    baseline: &Baseline,
    before: &str,
    after: &str,
    syntax: Syntax,
) -> Vec<Check> {
    let mut checks = Vec::new();

    if verify.config {
        checks.push(config_check(plan, before, after, syntax));
    }

    let kinds = wanted_kinds(verify);
    let now = if kinds.is_empty() {
        None
    } else {
        match collect::run_on(app, cli, &kinds).await {
            Ok(snapshot) => Some(snapshot),
            Err(err) => {
                checks.push(Check {
                    name: "state".into(),
                    ok: false,
                    detail: format!(
                        "the device could not be read after the change: {}",
                        err.message
                    ),
                });
                None
            }
        }
    };

    if verify.neighbors {
        checks.push(neighbor_check(baseline, now.as_ref()));
    }
    if verify.interfaces {
        checks.push(interface_check(
            baseline,
            now.as_ref(),
            &shut_down_by(plan, syntax),
        ));
    }
    if verify.routes {
        checks.push(route_check(baseline, now.as_ref()));
    }
    for address in &verify.pings {
        checks.push(ping_check(cli, address).await);
    }
    checks
}

/// The one check that makes a change honest: is the device what the plan said
/// it would be?
fn config_check(plan: &Plan, before: &str, after: &str, syntax: Syntax) -> Check {
    if plan.predicted_sha256.is_empty() {
        return Check {
            name: "configuration".into(),
            ok: false,
            detail: "the result of this change could not be predicted, so the configuration cannot be checked against it".into(),
        };
    }
    let (predicted, _) = crate::plan::predict(before, &plan.commands, syntax);
    let differences = compare::differences(&predicted, &ConfigTree::parse(after, syntax));
    if differences.is_empty() {
        Check {
            name: "configuration".into(),
            ok: true,
            detail: "the running configuration is exactly what the plan predicted".into(),
        }
    } else {
        Check {
            name: "configuration".into(),
            ok: false,
            detail: format!(
                "the running configuration is not what the plan predicted — {}",
                differences.describe(6)
            ),
        }
    }
}

fn neighbor_check(baseline: &Baseline, now: Option<&collect::Snapshot>) -> Check {
    let name = "neighbours".to_string();
    let (Some(before), Some(after)) = (
        &baseline.neighbors,
        now.and_then(|now| now.neighbors.as_ref()),
    ) else {
        return Check {
            name,
            ok: true,
            detail: not_run("neighbours", baseline),
        };
    };
    let key = |neighbor: &Neighbor| {
        (
            ifname::long(&neighbor.local_interface).to_ascii_lowercase(),
            neighbor.device.trim().to_ascii_lowercase(),
        )
    };
    let after_keys: BTreeSet<_> = after.iter().map(key).collect();
    let lost: Vec<String> = before
        .iter()
        .filter(|neighbor| !after_keys.contains(&key(neighbor)))
        .map(|neighbor| format!("{} on {}", neighbor.device, neighbor.local_interface))
        .collect();
    if lost.is_empty() {
        Check {
            name,
            ok: true,
            detail: format!("all {} neighbours are still there", before.len()),
        }
    } else {
        Check {
            name,
            ok: false,
            detail: format!(
                "neighbours that were there before are gone: {}",
                lost.join(", ")
            ),
        }
    }
}

fn interface_check(
    baseline: &Baseline,
    now: Option<&collect::Snapshot>,
    expected_down: &BTreeSet<String>,
) -> Check {
    let name = "interfaces".to_string();
    let (Some(before), Some(after)) = (
        &baseline.interfaces,
        now.and_then(|now| now.interfaces.as_ref()),
    ) else {
        return Check {
            name,
            ok: true,
            detail: not_run("interfaces", baseline),
        };
    };
    let state_now: std::collections::HashMap<String, Option<bool>> = after
        .iter()
        .map(|interface| {
            (
                ifname::long(&interface.name).to_ascii_lowercase(),
                interface.oper_up,
            )
        })
        .collect();
    let mut down: Vec<String> = Vec::new();
    for interface in before
        .iter()
        .filter(|interface| interface.oper_up == Some(true))
    {
        let key = ifname::long(&interface.name).to_ascii_lowercase();
        if expected_down.contains(&key) {
            continue;
        }
        if state_now.get(&key) == Some(&Some(false)) {
            down.push(interface.name.clone());
        }
    }
    if down.is_empty() {
        Check {
            name,
            ok: true,
            detail: match expected_down.len() {
                0 => "every interface that was up is still up".into(),
                shut => format!(
                    "every interface that was up is still up (apart from the {shut} this change shuts down)"
                ),
            },
        }
    } else {
        Check {
            name,
            ok: false,
            detail: format!("interfaces that were up are now down: {}", down.join(", ")),
        }
    }
}

fn route_check(baseline: &Baseline, now: Option<&collect::Snapshot>) -> Check {
    let name = "routes".to_string();
    let (Some(before), Some(after)) = (&baseline.routes, now.and_then(|now| now.routes.as_ref()))
    else {
        return Check {
            name,
            ok: true,
            detail: not_run("routes", baseline),
        };
    };
    let key = |route: &Route| {
        (
            route.prefix.trim().to_ascii_lowercase(),
            route.next_hop.trim().to_ascii_lowercase(),
        )
    };
    let after_keys: BTreeSet<_> = after.iter().map(key).collect();
    let lost: Vec<String> = before
        .iter()
        .filter(|route| !after_keys.contains(&key(route)))
        .map(|route| {
            if route.next_hop.trim().is_empty() {
                route.prefix.clone()
            } else {
                format!("{} via {}", route.prefix, route.next_hop)
            }
        })
        .collect();
    if lost.is_empty() {
        Check {
            name,
            ok: true,
            detail: format!("all {} routes are still there", before.len()),
        }
    } else {
        Check {
            name,
            ok: false,
            detail: format!(
                "routes that were there before are gone: {}{}",
                lost.iter().take(6).cloned().collect::<Vec<_>>().join(", "),
                if lost.len() > 6 {
                    format!(" (and {} more)", lost.len() - 6)
                } else {
                    String::new()
                }
            ),
        }
    }
}

fn not_run(kind: &str, baseline: &Baseline) -> String {
    match baseline
        .notes
        .iter()
        .find(|note| note.starts_with(kind))
        .cloned()
    {
        Some(note) => format!("not checked: {note}"),
        None => format!(
            "not checked: this device type does not collect {kind}, so there is nothing to compare"
        ),
    }
}

static SUCCESS_RATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)success rate is (\d+)\s*percent").expect("ping success pattern")
});
static PACKET_LOSS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\d+)(?:\.\d+)?%\s*packet loss").expect("packet loss pattern")
});

/// Ask the device itself to ping something it must still be able to reach.
async fn ping_check(cli: &mut Cli, address: &str) -> Check {
    let name = format!("ping {address}");
    let output = match cli.read(&format!("ping {address}")).await {
        Ok(output) => output,
        Err(err) => {
            return Check {
                name,
                ok: false,
                detail: format!("the ping could not be run: {}", err.message),
            };
        }
    };
    if let Some(error) = output.error {
        return Check {
            name,
            ok: false,
            detail: format!("the device refused the ping: {error}"),
        };
    }
    let text = output.output;
    let verdict = SUCCESS_RATE
        .captures(&text)
        .and_then(|caps| caps[1].parse::<u32>().ok())
        .map(|percent| (percent > 0, format!("{percent}% of the packets came back")))
        .or_else(|| {
            PACKET_LOSS
                .captures(&text)
                .and_then(|caps| caps[1].parse::<u32>().ok())
                .map(|loss| (loss < 100, format!("{loss}% packet loss")))
        });
    match verdict {
        Some((ok, detail)) => Check { name, ok, detail },
        // Not being able to tell is not the same as success.
        None => Check {
            name,
            ok: false,
            detail: format!(
                "Nocturne could not tell from the device's answer whether the ping worked: {}",
                text.lines()
                    .rfind(|line| !line.trim().is_empty())
                    .unwrap_or("(no output)")
                    .trim()
            ),
        },
    }
}
