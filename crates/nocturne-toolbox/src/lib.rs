//! RPC facade over the calculators and probes of the NOC toolbox.
//!
//! Specification: `docs/dev/features/toolbox.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! This is the drawer of small tools a network engineer opens twenty times a
//! day: subnet and VLSM maths, MAC and OUI lookup, the decoders for the weak
//! vendor encodings, converters, reference tables — and the probes, which
//! leave this machine. The arithmetic is `nocturne-tools`, the packets are
//! `nocturne-netprobe`, the key pair is `nocturne-transport`, the device
//! sessions are the core's `connect::exec`. Nothing here re-implements any of
//! that; what this crate adds is validation, the wire shapes, the inventory's
//! knowledge, and a record of what was run.
//!
//! # The two halves
//!
//! * [`calc`] — **offline.** Synchronous, deterministic, no socket, no device,
//!   no clock. Bad input is an `invalid` error carrying the engine's own
//!   message, never a default answer.
//! * [`probe`] and [`servers`] — **they touch the network.** Every one of them
//!   is a job or leaves a history record ([`probe::history`]), says where it
//!   ran (`from`) and how it was sent (`method`), and says plainly when it
//!   could not run at all.
//!
//! # Rules of engagement
//!
//! 1. **Nothing probes unless a person asks.** No background task, nothing at
//!    start-up, no "while we are here" packet from any other code path.
//! 2. **A probe that could not run says so.** A ping this machine may not send
//!    is an error naming the reason, never a report of 100 % loss; a probe that
//!    failed is a failed history record with the reason in it. There is no
//!    plausible-looking substitute for an answer anywhere in this crate.
//! 3. **The result says what was sent and from where.** `from` is "this
//!    computer" in the desktop app and "the Nocturne server" in server mode,
//!    which is what a person means when they ask whether the NOC can reach
//!    something; a ping also carries the method the operating system allowed
//!    (unprivileged ICMP sockets, raw sockets or the system `ping`).
//! 4. **A sweep is held to the same leash as discovery**: private address
//!    space unless the request *and* the settings allow more, a host cap, and
//!    the job log says when either bit.
//! 5. **Nothing is sent to a device outside the read-only gate.** The device
//!    probes build the platform's own ping command and hand it to
//!    `connect::exec`, so the profile's policy still decides whether it may be
//!    sent — and the target is validated character by character first, because
//!    that text ends up on a command line.
//! 6. **The servers are off unless asked.** [`servers`] binds nothing at
//!    start-up, binds where the settings say, stops when its job ends or the
//!    app shuts down, and reports a port it could not bind instead of
//!    appearing to work.
//! 7. **No secret reaches a log, a record or a model.** The decoders answer
//!    their caller and nothing else: a decoded password is never written to the
//!    audit trail, the job log or the probe history.

pub mod calc;
pub mod catalogue;
pub mod errors;
pub mod probe;
pub mod servers;
pub mod settings;
pub mod state;

#[cfg(test)]
mod tests;

/// Register this feature's settings, state, RPC methods and background tasks.
///
/// There is no background task and nothing is bound: every method here runs
/// because somebody called it.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(
        builder,
        settings::SECTION,
        &settings::ToolboxSettings::default(),
    );
    builder.state(state::State::default());
    // A probe or a server that was running when the process ended did not
    // finish; without this its record would sit in the history as running.
    probe::history::close_interrupted(builder.store());

    catalogue::register(&mut builder.router);
    calc::register(&mut builder.router);
    probe::register(&mut builder.router);
    servers::register(&mut builder.router);
}
