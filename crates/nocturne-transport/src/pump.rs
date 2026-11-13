//! Supervision of the two directions of a session.
//!
//! Every transport runs its "remote → application" and "application → remote"
//! halves as two independent futures, so that neither can stall the other (a
//! full queue in one direction must never stop the other from draining — that
//! is how duplex pumps deadlock). [`drive`] polls both and decides when the
//! session is over.

use std::future::Future;
use std::time::Duration;

use tokio::time::Instant;

/// How the remote → application half ended.
#[derive(Debug)]
pub(crate) enum ReadEnd {
    /// The application dropped its reader: nobody is left to notify.
    Abandoned,
    /// The remote side is done; the payload is the close reason.
    Finished(Option<String>),
}

/// How the application → remote half ended.
#[derive(Debug)]
pub(crate) enum WriteEnd {
    /// `Input::Close` was received and passed on to the remote side.
    CloseRequested,
    /// Every writer handle was dropped; the session carries on read-only.
    InputClosed,
    /// Writing to the remote side failed.
    Failed(String),
}

/// The verdict of [`drive`].
#[derive(Debug)]
pub(crate) enum Outcome {
    /// The application went away; skip the farewell.
    Abandoned,
    /// Deliver `Output::Closed { reason }`.
    Closed(Option<String>),
}

/// Runs both halves until the session is over.
///
/// The session normally ends when `reader` finishes. After the writer reports
/// a close request (or a failure), the reader gets `grace` to observe the
/// remote side closing before the session is declared over anyway. A locally
/// requested close always yields `Closed(None)`.
pub(crate) async fn drive<R, W>(reader: R, writer: W, grace: Duration) -> Outcome
where
    R: Future<Output = ReadEnd>,
    W: Future<Output = WriteEnd>,
{
    tokio::pin!(reader, writer);
    let mut writer_running = true;
    let mut local_close = false;
    let mut write_failure = None;
    let mut deadline: Option<Instant> = None;

    loop {
        tokio::select! {
            end = &mut reader => {
                return match end {
                    ReadEnd::Abandoned => Outcome::Abandoned,
                    ReadEnd::Finished(_) if local_close => Outcome::Closed(None),
                    ReadEnd::Finished(reason) => Outcome::Closed(reason.or(write_failure)),
                };
            }
            end = &mut writer, if writer_running => {
                writer_running = false;
                match end {
                    WriteEnd::InputClosed => {}
                    WriteEnd::CloseRequested => {
                        local_close = true;
                        deadline = Some(Instant::now() + grace);
                    }
                    WriteEnd::Failed(error) => {
                        write_failure = Some(error);
                        deadline = Some(Instant::now() + grace);
                    }
                }
            }
            () = wait_until(deadline), if deadline.is_some() => {
                return Outcome::Closed(if local_close { None } else { write_failure });
            }
        }
    }
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;

    use super::*;

    const GRACE: Duration = Duration::from_millis(40);

    #[tokio::test]
    async fn remote_close_wins() {
        let outcome = drive(
            async { ReadEnd::Finished(Some("exit status 0".into())) },
            pending::<WriteEnd>(),
            GRACE,
        )
        .await;
        assert!(matches!(outcome, Outcome::Closed(Some(r)) if r == "exit status 0"));
    }

    #[tokio::test]
    async fn abandoned_reader_ends_the_session() {
        let outcome = drive(async { ReadEnd::Abandoned }, pending::<WriteEnd>(), GRACE).await;
        assert!(matches!(outcome, Outcome::Abandoned));
    }

    #[tokio::test]
    async fn input_closing_keeps_the_reader_running() {
        let reader = async {
            tokio::time::sleep(GRACE * 3).await;
            ReadEnd::Finished(Some("late".into()))
        };
        let outcome = drive(reader, async { WriteEnd::InputClosed }, GRACE).await;
        assert!(matches!(outcome, Outcome::Closed(Some(r)) if r == "late"));
    }

    #[tokio::test]
    async fn local_close_has_no_reason_even_if_the_remote_confirms() {
        let reader = async {
            tokio::time::sleep(GRACE / 4).await;
            ReadEnd::Finished(Some("exit status 129".into()))
        };
        let outcome = drive(reader, async { WriteEnd::CloseRequested }, GRACE).await;
        assert!(matches!(outcome, Outcome::Closed(None)));
    }

    #[tokio::test]
    async fn local_close_does_not_wait_forever_for_the_remote() {
        let started = Instant::now();
        let outcome = drive(pending::<ReadEnd>(), async { WriteEnd::CloseRequested }, GRACE).await;
        assert!(matches!(outcome, Outcome::Closed(None)));
        assert!(started.elapsed() >= GRACE);
    }

    #[tokio::test]
    async fn write_failure_is_reported_when_the_reader_knows_nothing_better() {
        let outcome = drive(
            pending::<ReadEnd>(),
            async { WriteEnd::Failed("broken pipe".into()) },
            GRACE,
        )
        .await;
        assert!(matches!(outcome, Outcome::Closed(Some(r)) if r == "broken pipe"));

        let reader = async {
            tokio::time::sleep(GRACE / 4).await;
            ReadEnd::Finished(Some("connection reset".into()))
        };
        let outcome = drive(reader, async { WriteEnd::Failed("broken pipe".into()) }, GRACE).await;
        assert!(matches!(outcome, Outcome::Closed(Some(r)) if r == "connection reset"));
    }
}
