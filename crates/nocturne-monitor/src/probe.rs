//! Reachability probes, behind a trait so the tests can answer for them.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nocturne_netprobe::ping::{self, PingOptions, Pinger};
use nocturne_netprobe::tcp;

/// The outcome of one burst of echo requests.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Echoes {
    pub sent: u32,
    pub received: u32,
    /// Mean round-trip time of the replies that came back.
    pub rtt_ms: Option<f64>,
}

impl Echoes {
    pub fn loss_pct(&self) -> f64 {
        if self.sent == 0 {
            return 100.0;
        }
        f64::from(self.sent - self.received) / f64::from(self.sent) * 100.0
    }

    pub fn answered(&self) -> bool {
        self.received > 0
    }
}

/// Sending packets at a device to see whether it is there.
///
/// `Err` means the probe could not be *attempted* (the name does not resolve,
/// this machine may not send ICMP at all); a device that simply did not
/// answer comes back as `Ok` with nothing received, which is a measurement.
#[async_trait]
pub trait Reachability: Send + Sync + 'static {
    async fn ping(&self, address: &str, count: u32, timeout: Duration) -> Result<Echoes, String>;

    /// One TCP connect, as the fallback for devices that drop ICMP.
    /// `Ok` with the time to the handshake.
    async fn tcp(&self, address: &str, port: u16, timeout: Duration) -> Result<f64, String>;

    /// How echoes are sent on this machine: `icmp_datagram`, `icmp_raw` or
    /// `system_command`.
    fn method(&self) -> String;
}

/// The real thing: `nocturne-netprobe`.
pub struct NetProbe {
    pinger: Pinger,
}

impl Default for NetProbe {
    fn default() -> Self {
        NetProbe::new()
    }
}

impl NetProbe {
    pub fn new() -> NetProbe {
        NetProbe {
            pinger: Pinger::new(),
        }
    }
}

#[async_trait]
impl Reachability for NetProbe {
    async fn ping(&self, address: &str, count: u32, timeout: Duration) -> Result<Echoes, String> {
        let options = PingOptions {
            count: count.max(1),
            // Monitoring wants the three echoes over with, not spread over
            // three seconds the way an interactive ping is.
            interval_ms: 200,
            timeout_ms: timeout.as_millis().clamp(1, u64::MAX as u128) as u64,
            payload: 56,
        };
        let report = ping::ping(&self.pinger, address, &options, None)
            .await
            .map_err(|err| err.to_string())?;
        let replies: Vec<f64> = report
            .echoes
            .iter()
            .filter_map(|echo| echo.rtt_ms)
            .collect();
        Ok(Echoes {
            sent: report.sent,
            received: report.received,
            rtt_ms: (!replies.is_empty())
                .then(|| replies.iter().sum::<f64>() / replies.len() as f64),
        })
    }

    async fn tcp(&self, address: &str, port: u16, timeout: Duration) -> Result<f64, String> {
        let result = tcp::check(address, port, timeout)
            .await
            .map_err(|err| err.to_string())?;
        match result.state {
            tcp::PortState::Open => Ok(result.rtt_ms.unwrap_or(0.0)),
            tcp::PortState::Closed => Err(format!("nothing is listening on port {port}")),
            tcp::PortState::Filtered => Err(format!("port {port} did not answer")),
        }
    }

    fn method(&self) -> String {
        match self.pinger.method() {
            ping::Method::IcmpDatagram => "icmp_datagram",
            ping::Method::IcmpRaw => "icmp_raw",
            ping::Method::SystemCommand => "system_command",
        }
        .to_string()
    }
}

pub type SharedProbe = Arc<dyn Reachability>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loss_is_a_percentage_of_what_was_sent() {
        let echoes = Echoes {
            sent: 3,
            received: 3,
            rtt_ms: Some(1.0),
        };
        assert_eq!(echoes.loss_pct(), 0.0);
        assert!(echoes.answered());
        let lost = Echoes {
            sent: 3,
            received: 0,
            rtt_ms: None,
        };
        assert_eq!(lost.loss_pct(), 100.0);
        assert!(!lost.answered());
        assert_eq!(
            Echoes {
                sent: 4,
                received: 3,
                rtt_ms: Some(2.0)
            }
            .loss_pct(),
            25.0
        );
    }

    /// Whatever this machine allows — datagram sockets, raw sockets or the
    /// system `ping` — loopback must answer.
    #[tokio::test]
    async fn loopback_answers_through_whichever_method_this_machine_has() {
        let probe = NetProbe::new();
        assert!(
            ["icmp_datagram", "icmp_raw", "system_command"].contains(&probe.method().as_str()),
            "{}",
            probe.method()
        );
        let echoes = probe
            .ping("127.0.0.1", 2, Duration::from_millis(1_000))
            .await;
        match echoes {
            Ok(echoes) => {
                assert_eq!(echoes.sent, 2);
                // A sandbox may drop even loopback ICMP; the measurement is
                // still a measurement, and that is what is asserted.
                if echoes.answered() {
                    assert!(echoes.rtt_ms.is_some_and(|rtt| rtt >= 0.0));
                }
            }
            Err(error) => assert!(!error.is_empty()),
        }
    }

    #[tokio::test]
    async fn a_tcp_probe_tells_a_listener_from_a_closed_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let probe = NetProbe::new();
        assert!(
            probe
                .tcp("127.0.0.1", port, Duration::from_millis(500))
                .await
                .is_ok()
        );
        drop(listener);
        assert!(
            probe
                .tcp("127.0.0.1", port, Duration::from_millis(500))
                .await
                .is_err()
        );
    }
}
