//! Stopping the engine when the program ends.
//!
//! `App::shutdown` only says "stop": every job — a backup being written, a
//! change being applied — is cancelled, and reacts at its next step by
//! recording that it was. The process used to end a fraction of a second
//! later, inside the event loop, before any of them had their turn, so the
//! next start found them still "running" and could only write "Nocturne was
//! closed while this was running" over them. Quitting now waits for that,
//! briefly, the way `nocturne-server` gives its work a moment.

use std::time::{Duration, Instant};

use nocturne_suite::App;
use serde_json::{Value, json};

/// How long quitting waits for work that was running to stop.
pub const GRACE: Duration = Duration::from_secs(3);

/// How often it looks.
const POLL: Duration = Duration::from_millis(50);

/// Stop the engine's background work, then wait — for at most `grace` —
/// until every job that was running has recorded how it ended. Returns the
/// ids of those that had not when the time was up.
///
/// A job counts as stopped once `jobs.get` reads it back finished, which is
/// once its record is saved, not merely once it has left the list of running
/// ones.
pub async fn stop(app: &App, grace: Duration) -> Vec<String> {
    let running = running_jobs(app).await;
    app.shutdown();
    let deadline = Instant::now() + grace;
    let mut left = running;
    loop {
        let mut still = Vec::new();
        for id in left {
            if is_running(app, &id).await {
                still.push(id);
            }
        }
        left = still;
        if left.is_empty() || Instant::now() >= deadline {
            return left;
        }
        tokio::time::sleep(POLL.min(deadline.saturating_duration_since(Instant::now()))).await;
    }
}

async fn running_jobs(app: &App) -> Vec<String> {
    match app.rpc("jobs.list", json!({ "state": "running" })).await {
        Ok(Value::Array(jobs)) => jobs
            .iter()
            .filter_map(|job| job["id"].as_str().map(str::to_string))
            .collect(),
        Ok(_) => Vec::new(),
        Err(err) => {
            tracing::warn!(code = %err.code, message = %err.message, "could not ask which jobs are running");
            Vec::new()
        }
    }
}

async fn is_running(app: &App, id: &str) -> bool {
    match app.rpc("jobs.get", json!({ "id": id })).await {
        Ok(job) => job["state"] == "running",
        // Gone from the record altogether: nothing left to wait for.
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::jobs::{self, JobSpec};
    use nocturne_suite::{AppBuilder, AppConfig};
    use serde_json::{Value, json};

    /// The engine, with one more method: a job that takes `tidy_ms` to
    /// stop once it is told to, as a backup does that finishes writing the
    /// file it is in the middle of, or ignores being told altogether.
    fn engine_with_a_slow_job() -> App {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        nocturne_suite::init(&mut builder);
        builder
            .router
            .add("test.slow_job", |app: App, params: Value| async move {
                let tidy = Duration::from_millis(params["tidy_ms"].as_u64().unwrap_or(0));
                let listens = params["listens"].as_bool().unwrap_or(true);
                let job = jobs::start(
                    &app,
                    JobSpec::new("test.slow", "Slow"),
                    move |ctx| async move {
                        if listens {
                            ctx.token().cancelled().await;
                        } else {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                        }
                        tokio::time::sleep(tidy).await;
                        Err(jobs::cancelled())
                    },
                );
                Ok(job)
            });
        builder.build()
    }

    async fn state_of(app: &App, job: &Value) -> String {
        let job = app
            .rpc("jobs.get", json!({ "id": job["id"] }))
            .await
            .expect("jobs.get");
        job["state"].as_str().unwrap_or_default().to_string()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_job_that_is_running_when_the_program_ends_has_time_to_say_it_was_stopped() {
        let app = engine_with_a_slow_job();
        let job = app
            .rpc("test.slow_job", json!({ "tidy_ms": 300 }))
            .await
            .unwrap();
        assert_eq!(state_of(&app, &job).await, "running");

        let left = stop(&app, GRACE).await;

        assert_eq!(left, Vec::<String>::new());
        // Recorded by the job itself, not "Nocturne was closed while this
        // was running" written over it at the next start.
        assert_eq!(state_of(&app, &job).await, "cancelled");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn work_that_does_not_stop_is_not_waited_for_beyond_the_grace() {
        let app = engine_with_a_slow_job();
        let job = app
            .rpc("test.slow_job", json!({ "listens": false }))
            .await
            .unwrap();

        let grace = Duration::from_millis(400);
        let started = std::time::Instant::now();
        let left = stop(&app, grace).await;

        assert!(started.elapsed() >= grace, "it did not wait at all");
        assert!(
            started.elapsed() < grace + Duration::from_secs(2),
            "it waited {:?}",
            started.elapsed()
        );
        assert_eq!(left, vec![job["id"].as_str().unwrap().to_string()]);
    }
}
