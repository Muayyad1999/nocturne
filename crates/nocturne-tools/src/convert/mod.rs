//! Conversions: number bases, sizes and rates, transfer time, durations and
//! what is left of the MTU inside a tunnel.

mod duration;
mod mtu;
mod number;
mod units;

pub use duration::{format_duration, parse_duration_s};
pub use mtu::{Encapsulation, MtuResult, encapsulations, mtu};
pub use number::{NumberForms, number};
pub use units::{
    TransferEstimate, format_bytes, format_rate, parse_rate_bps, parse_size_bytes, transfer_time,
};
