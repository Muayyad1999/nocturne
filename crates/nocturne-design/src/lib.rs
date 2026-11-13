//! Automated network design: requirements in, a complete design out.
//!
//! > *"We are opening a branch with 120 people, voice, guest Wi-Fi and two
//! > uplinks — what do I build?"*
//!
//! [`generate`] answers with the topology, the VLAN and addressing plan, the
//! device list and the full configuration of every device, as a
//! [`LabSpec`](nocturne_netsim::LabSpec) that boots in Nocturne's simulator.
//! Each design carries the reachability matrix it must satisfy
//! ([`Check`]), which is how this crate's own tests prove that what it
//! produces actually works rather than merely looking plausible.
//!
//! ```
//! use nocturne_design::{Requirements, Site, generate};
//!
//! let design = generate(&Requirements {
//!     name: "Acme branch".into(),
//!     sites: vec![Site { name: "Branch".into(), users: 120, voice: true, guest_wifi: true,
//!                        ..Site::default() }],
//!     ..Requirements::default()
//! })?;
//!
//! let mut lab = nocturne_netsim::Lab::from_spec(&design.lab)?;
//! lab.settle(120_000_000);
//! assert!(!design.plan.checks.is_empty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The crate is pure: no I/O, no clock, no randomness. The same
//! [`Requirements`] always produce the same [`Design`], byte for byte, and
//! [`Requirements::seed`] only reaches the simulator, which derives MAC
//! addresses and serial numbers from it.
//!
//! # Where the design comes from
//!
//! Nothing here is a template. Port counts, switch counts, subnet sizes and
//! the shape of the topology are all computed from the requirements, and
//! every choice is recorded in [`Design::decisions`] with the arithmetic
//! behind it. Anything that could not be met exactly is in
//! [`Design::warnings`], in the words a network engineer would use.
//!
//! Generated configurations contain placeholder passwords — never anything
//! that looks like a real secret — and [`document`] tells the reader to
//! replace them.

#![forbid(unsafe_code)]

mod assemble;
mod checks;
mod config;
mod design;
mod document;
mod error;
mod examples;
mod model;
mod requirements;
mod topology;
mod util;

pub use design::{
    BomLine, Check, Decision, Design, DevicePlan, DeviceRole, Expect, InterfaceUse, LinkKind,
    LinkPlan, Plan, SitePlan, VlanPlan,
};
pub use document::document;
pub use error::DesignError;
pub use examples::examples;
pub use requirements::{
    Addressing, InternetEdge, Purpose, Redundancy, Requirements, Routing, Services, Site, VlanReq,
    Wan, WanTopology,
};

/// Design the network.
///
/// Fails only when the requirements themselves cannot be built, and then with
/// every problem at once — the same sentences [`validate`] returns.
pub fn generate(requirements: &Requirements) -> Result<Design, DesignError> {
    let mut model = model::prepare(requirements).map_err(DesignError::Requirements)?;
    let topology = topology::build(&mut model).map_err(DesignError::Internal)?;
    Ok(assemble::finish(model, topology))
}

/// Everything wrong with the requirements, in user terms, all at once.
///
/// An empty result means [`generate`] will succeed.
pub fn validate(requirements: &Requirements) -> Vec<String> {
    match model::prepare(requirements) {
        Err(problems) => problems,
        Ok(mut model) => match topology::build(&mut model) {
            Ok(_) => Vec::new(),
            Err(problem) => vec![problem],
        },
    }
}
