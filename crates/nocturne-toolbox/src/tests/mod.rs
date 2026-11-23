//! The toolbox's tests.
//!
//! `calculators` and `catalogue` need nothing but the app. `probes` and
//! `servers` put real packets on loopback, against servers the tests start
//! themselves; where a machine may not be allowed to send at all (ICMP,
//! Wake-on-LAN) the test asserts that the refusal is reported as a refusal,
//! which is the behaviour that matters.

mod calculators;
mod catalogue;
mod device;
mod history;
mod probes;
mod qos;
mod servers;
mod support;
