//! The checks themselves: one function per entry in the catalogue.
//!
//! A check is a pure function of the [`World`], the settings and a
//! [`CheckReport`]. It has three jobs, and the third is the one that decides
//! whether this feature is worth having:
//!
//! 1. report what it found, with the evidence it decided from;
//! 2. mark every device it reached a verdict for ([`CheckReport::ok`]);
//! 3. mark every device it could *not* decide for, and why
//!    ([`CheckReport::skip`]) — never silence.
//!
//! Each check's own file carries, next to the code, the reasoning for every
//! look-alike it deliberately stays quiet about. Those comments are the
//! specification of the check; the tests next to them are the proof.

pub mod config;
pub mod fleet;
pub mod l2;
pub mod l3;
pub mod physical;
pub mod qos;
pub mod security;

use nocturne_drivers::model::Interface;

use crate::links::Link;
use crate::report::CheckReport;
use crate::world::{DeviceData, World};

/// Both ends of a link, with the collected interface of each.
pub(crate) type Ends<'a> = (&'a DeviceData, &'a Interface, &'a DeviceData, &'a Interface);

pub(crate) const NO_INTERFACES: &str = "no interface data collected for one end of the link";
pub(crate) const PORT_NOT_COLLECTED: &str =
    "the port at one end of the link is not in that device's collected interfaces";
pub(crate) const NO_LINK: &str = "no link to another managed device";

/// Resolve a link to the two interfaces it joins, or say why it cannot be.
pub(crate) fn link_ends<'a>(world: &'a World, link: &'a Link) -> Result<Ends<'a>, &'static str> {
    let (Some(a), Some(b)) = (
        world.device(&link.a.device_id),
        world.device(&link.b.device_id),
    ) else {
        // A device deleted between loading and now.
        return Err(NO_INTERFACES);
    };
    if a.interfaces.is_none() || b.interfaces.is_none() {
        return Err(NO_INTERFACES);
    }
    let (Some(a_port), Some(b_port)) = (
        a.interface(&link.a.interface),
        b.interface(&link.b.interface),
    ) else {
        return Err(PORT_NOT_COLLECTED);
    };
    Ok((a, a_port, b, b_port))
}

/// Mark both ends of a link as not decidable.
pub(crate) fn skip_link(report: &mut CheckReport, link: &Link, reason: &'static str) {
    report.skip(&link.a.device_id, reason);
    report.skip(&link.b.device_id, reason);
}

/// Every target device the check neither decided for nor explained is
/// explained now. Called at the end of a check so that the coverage report
/// accounts for every device, every time.
pub(crate) fn skip_rest(world: &World, report: &mut CheckReport, reason: &'static str) {
    let ids: Vec<String> = world.targets().map(|data| data.id().to_string()).collect();
    for id in ids {
        report.skip(&id, reason);
    }
}

/// The device of a pair that owns a finding about both: the lower id, so that
/// the finding keeps its identity whichever end a scan starts from.
pub(crate) fn owner<'a>(one: &'a str, other: &'a str) -> (&'a str, &'a str) {
    if one <= other {
        (one, other)
    } else {
        (other, one)
    }
}
