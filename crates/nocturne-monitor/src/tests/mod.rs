//! Cross-cutting tests: the ones that need a whole app, a scheduler or a
//! listener on a socket. The narrower ones live next to the code they check.

mod certificates;
mod check_definitions;
mod check_engines;
mod check_history;
mod check_watching;
mod checks;
mod counters;
mod health;
mod ifindex;
mod lifecycle;
mod listeners;
mod live;
mod notifications;
mod owed;
mod polling;
mod routing;
mod rpc;
mod scheduling;
mod snmpd;
