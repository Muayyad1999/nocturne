//! Cross-cutting tests: the ones that need a whole app, a scheduler or a
//! listener on a socket. The narrower ones live next to the code they check.

mod counters;
mod health;
mod lifecycle;
mod live;
mod notifications;
mod polling;
mod rpc;
mod scheduling;
mod snmpd;
