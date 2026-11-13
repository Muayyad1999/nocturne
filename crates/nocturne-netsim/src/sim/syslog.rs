//! IOS-format log messages: formatting, the per-device buffer, and the
//! `LabEvent::Syslog` stream.

use std::collections::VecDeque;

use crate::api::LabEvent;
use crate::device::config::Timestamps;
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::util::clock::{DateTime, age_compact};

const BUFFER_LINES: usize = 500;

#[derive(Clone, Debug)]
pub(crate) struct LogEntry {
    pub(crate) seq: u64,
    pub(crate) severity: u8,
    pub(crate) line: String,
}

/// The device's logging buffer; consoles follow it by sequence number.
#[derive(Clone, Debug, Default)]
pub(crate) struct LogBuffer {
    pub(crate) entries: VecDeque<LogEntry>,
    pub(crate) next_seq: u64,
}

impl LogBuffer {
    /// Entries logged since `seq` (exclusive of older ones that scrolled away).
    pub(crate) fn since(&self, seq: u64) -> impl Iterator<Item = &LogEntry> {
        self.entries.iter().filter(move |entry| entry.seq >= seq)
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Node {
    /// The timestamp prefix for a log line, per `service timestamps log`.
    pub(crate) fn log_stamp(&self, now: u64) -> String {
        match self.cfg.services.timestamps_log {
            Timestamps::Off => String::new(),
            Timestamps::Uptime => format!("{}: ", age_compact(self.uptime(now))),
            Timestamps::Datetime { msec } => {
                // The asterisk marks a clock that was never set.
                let star = if self.clock_offset_ms.is_none() {
                    "*"
                } else {
                    ""
                };
                format!(
                    "{star}{}: ",
                    DateTime::from_unix_ms(self.clock_ms(now)).log_stamp(msec)
                )
            }
        }
    }

    /// Emit `%FACILITY-SEVERITY-MNEMONIC: message` to the buffer and the event stream.
    pub(crate) fn log(
        &mut self,
        fx: &mut Fx,
        severity: u8,
        facility: &str,
        mnemonic: &str,
        message: String,
    ) {
        if !self.model.is_network_os() {
            return;
        }
        let line = format!(
            "{}%{facility}-{severity}-{mnemonic}: {message}",
            self.log_stamp(fx.now)
        );
        let seq = self.log.next_seq;
        self.log.next_seq += 1;
        self.log.entries.push_back(LogEntry {
            seq,
            severity,
            line: line.clone(),
        });
        if self.log.entries.len() > BUFFER_LINES {
            self.log.entries.pop_front();
        }
        fx.events.push(LabEvent::Syslog {
            node: self.id,
            time: fx.now,
            severity,
            facility: facility.to_string(),
            mnemonic: mnemonic.to_string(),
            message,
            line,
        });
    }
}
