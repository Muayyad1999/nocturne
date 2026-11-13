//! What every request handler shares.

use std::sync::Arc;

use nocturne_suite::App;
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

use crate::auth::TokenCheck;
use crate::guard::OriginPolicy;
use crate::options::Timing;
use crate::ui::UiSource;

/// More event streams than this at once is a client bug, not a busy day.
const MAX_EVENT_STREAMS: usize = 256;

pub(crate) type Shared = Arc<ServerState>;

pub(crate) struct ServerState {
    pub(crate) app: App,
    pub(crate) token: TokenCheck,
    pub(crate) origin: OriginPolicy,
    pub(crate) ui: UiSource,
    pub(crate) timing: Timing,
    /// Cancelled when the server starts shutting down.
    pub(crate) shutdown: CancellationToken,
    /// RPC calls and event streams that outlive the connection that started them.
    pub(crate) in_flight: InFlight,
    pub(crate) event_streams: Arc<Semaphore>,
}

impl ServerState {
    pub(crate) fn new(
        app: App,
        token: &str,
        origin: OriginPolicy,
        ui: UiSource,
        timing: Timing,
        shutdown: CancellationToken,
    ) -> Shared {
        Arc::new(ServerState {
            app,
            token: TokenCheck::new(token),
            origin,
            ui,
            timing,
            shutdown,
            in_flight: InFlight::default(),
            event_streams: Arc::new(Semaphore::new(MAX_EVENT_STREAMS)),
        })
    }
}

/// Counts work in progress so that shutdown can wait for it.
#[derive(Clone)]
pub(crate) struct InFlight {
    count: Arc<watch::Sender<usize>>,
}

impl Default for InFlight {
    fn default() -> Self {
        InFlight {
            count: Arc::new(watch::channel(0).0),
        }
    }
}

impl InFlight {
    /// Register one piece of work; it counts until the returned guard is dropped.
    pub(crate) fn enter(&self) -> InFlightGuard {
        self.count.send_modify(|count| *count += 1);
        InFlightGuard {
            count: self.count.clone(),
        }
    }

    /// Resolves once nothing is in progress.
    pub(crate) async fn drained(&self) {
        let mut count = self.count.subscribe();
        // Cannot fail: `self` keeps the sender alive.
        let _ = count.wait_for(|count| *count == 0).await;
    }
}

pub(crate) struct InFlightGuard {
    count: Arc<watch::Sender<usize>>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.count.send_modify(|count| *count -= 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn drained_waits_for_every_guard() {
        let in_flight = InFlight::default();
        in_flight.drained().await;

        let first = in_flight.enter();
        let second = in_flight.enter();
        let waiter = tokio::spawn({
            let in_flight = in_flight.clone();
            async move { in_flight.drained().await }
        });
        drop(first);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!waiter.is_finished(), "one guard is still alive");
        drop(second);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("drained() resolves once the last guard is gone")
            .unwrap();
    }
}
