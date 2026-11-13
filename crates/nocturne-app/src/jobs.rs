//! Jobs: anything that takes longer than a click.
//!
//! A discovery sweep, a backup of every device, a change being applied — each
//! runs as a job: it has a title, progress, a log, can be cancelled, and
//! leaves a record when it ends. Feature modules start jobs with [`start`]
//! and report through the [`JobCtx`] they are handed; the UI follows along
//! through the `jobs.changed` event and never has to poll.

use std::collections::HashMap;
use std::future::Future;

use futures::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::app::{App, AppBuilder};
use crate::error::{AppError, AppResult};
use crate::util::{new_id, now_ms};

const COLLECTION: &str = "jobs";
/// Finished jobs kept in history.
const HISTORY: usize = 300;
/// Log lines kept per job (the first and last halves when a job is chattier).
const LOG_LINES: usize = 400;
/// Minimum spacing of progress announcements per job.
const ANNOUNCE_EVERY_MS: i64 = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        self != JobState::Running
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobLine {
    pub ts: i64,
    /// `info`, `warn` or `error`.
    pub level: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
}

/// What a job is, for the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Job {
    pub id: String,
    /// Machine name of the operation, e.g. `discovery.scan`, `config.backup`.
    pub kind: String,
    pub title: String,
    pub state: JobState,
    /// Who started it: `user`, `schedule`, `assistant` or `system`.
    pub origin: String,
    pub device_ids: Vec<String>,
    pub done: u64,
    /// `0` while the amount of work is unknown.
    pub total: u64,
    /// What it is doing right now.
    pub message: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    /// Whatever the operation returned.
    pub result: Option<Value>,
    pub log: Vec<JobLine>,
    /// Log lines dropped from the middle of a very long log.
    pub log_dropped: u64,
}

impl Default for Job {
    fn default() -> Self {
        Job {
            id: String::new(),
            kind: String::new(),
            title: String::new(),
            state: JobState::Running,
            origin: "user".into(),
            device_ids: Vec::new(),
            done: 0,
            total: 0,
            message: String::new(),
            started_at: 0,
            finished_at: None,
            error: None,
            result: None,
            log: Vec::new(),
            log_dropped: 0,
        }
    }
}

/// How a job is described when it starts.
#[derive(Clone, Debug, Default)]
pub struct JobSpec {
    pub kind: String,
    pub title: String,
    pub origin: String,
    pub device_ids: Vec<String>,
    pub total: u64,
}

impl JobSpec {
    pub fn new(kind: &str, title: impl Into<String>) -> JobSpec {
        JobSpec {
            kind: kind.to_string(),
            title: title.into(),
            origin: "user".into(),
            ..JobSpec::default()
        }
    }

    pub fn origin(mut self, origin: &str) -> JobSpec {
        self.origin = origin.to_string();
        self
    }

    pub fn devices(mut self, device_ids: &[String]) -> JobSpec {
        self.device_ids = device_ids.to_vec();
        self.total = device_ids.len() as u64;
        self
    }

    pub fn total(mut self, total: u64) -> JobSpec {
        self.total = total;
        self
    }
}

struct Running {
    job: Job,
    cancel: CancellationToken,
    announced_at: i64,
}

#[derive(Default)]
pub struct JobsState {
    running: Mutex<HashMap<String, Running>>,
}

/// The handle a running job reports through.
#[derive(Clone)]
pub struct JobCtx {
    app: App,
    id: String,
    cancel: CancellationToken,
}

impl JobCtx {
    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Cancelled when the user cancels the job or the app shuts down.
    pub fn token(&self) -> &CancellationToken {
        &self.cancel
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// `Err(cancelled)` once the job has been cancelled: sprinkle between steps.
    pub fn check(&self) -> AppResult<()> {
        if self.is_cancelled() {
            Err(cancelled())
        } else {
            Ok(())
        }
    }

    /// Run `work`, giving up as soon as the job is cancelled.
    pub async fn race<T>(&self, work: impl Future<Output = AppResult<T>>) -> AppResult<T> {
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(cancelled()),
            result = work => result,
        }
    }

    fn update(&self, force: bool, change: impl FnOnce(&mut Job)) {
        let state = self.app.state::<JobsState>();
        let snapshot = {
            let mut running = state.running.lock();
            let Some(entry) = running.get_mut(&self.id) else {
                return;
            };
            change(&mut entry.job);
            let now = now_ms();
            if !force && now - entry.announced_at < ANNOUNCE_EVERY_MS {
                return;
            }
            entry.announced_at = now;
            summary(&entry.job)
        };
        self.app.publish("jobs.changed", &snapshot);
    }

    pub fn set_total(&self, total: u64) {
        self.update(false, |job| job.total = total);
    }

    pub fn progress(&self, done: u64, message: impl Into<String>) {
        let message = message.into();
        self.update(false, |job| {
            job.done = done;
            job.message = message;
        });
    }

    /// One more unit of work finished.
    pub fn advance(&self, message: impl Into<String>) {
        let message = message.into();
        self.update(false, |job| {
            job.done += 1;
            job.message = message;
        });
    }

    pub fn status(&self, message: impl Into<String>) {
        let message = message.into();
        self.update(false, |job| job.message = message);
    }

    fn line(&self, level: &str, device_id: Option<&str>, message: String) {
        let line = JobLine {
            ts: now_ms(),
            level: level.to_string(),
            message,
            device_id: device_id.map(str::to_string),
        };
        self.app.publish(
            "jobs.log",
            &serde_json::json!({"job_id": self.id, "line": line}),
        );
        self.update(false, |job| {
            if job.log.len() >= LOG_LINES {
                // Keep the beginning and the most recent lines; count what falls out.
                job.log.remove(LOG_LINES / 2);
                job.log_dropped += 1;
            }
            job.log.push(line);
        });
    }

    pub fn info(&self, message: impl Into<String>) {
        self.line("info", None, message.into());
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.line("warn", None, message.into());
    }

    pub fn error(&self, message: impl Into<String>) {
        self.line("error", None, message.into());
    }

    /// A log line about one device.
    pub fn device(&self, device_id: &str, level: &str, message: impl Into<String>) {
        self.line(level, Some(device_id), message.into());
    }

    /// Run `work` for every item with at most `concurrency` at a time,
    /// advancing the job after each. Results come back in input order; items
    /// not started because the job was cancelled are `Err(cancelled)`.
    pub async fn for_each<I, T, F, Fut>(
        &self,
        items: Vec<I>,
        concurrency: usize,
        work: F,
    ) -> Vec<AppResult<T>>
    where
        I: Send + 'static,
        T: Send + 'static,
        F: Fn(JobCtx, I) -> Fut,
        Fut: Future<Output = AppResult<T>> + Send,
    {
        let total = items.len();
        let mut slots: Vec<Option<AppResult<T>>> = (0..total).map(|_| None).collect();
        let mut stream = futures::stream::iter(items.into_iter().enumerate())
            .map(|(index, item)| {
                let ctx = self.clone();
                let future = (!ctx.is_cancelled()).then(|| work(ctx.clone(), item));
                async move {
                    let result = match future {
                        Some(future) => ctx.race(future).await,
                        None => Err(cancelled()),
                    };
                    (index, result)
                }
            })
            .buffer_unordered(concurrency.max(1));
        while let Some((index, result)) = stream.next().await {
            self.update(false, |job| job.done += 1);
            slots[index] = Some(result);
        }
        slots
            .into_iter()
            .map(|slot| slot.unwrap_or_else(|| Err(cancelled())))
            .collect()
    }
}

pub fn cancelled() -> AppError {
    AppError::new("cancelled", "the operation was cancelled")
}

/// A job without its log and result, as carried by `jobs.changed`.
fn summary(job: &Job) -> Job {
    Job {
        log: Vec::new(),
        result: None,
        ..job.clone()
    }
}

/// Start a job. Returns at once with the job as it starts; `work` runs in the
/// background and its `Ok` value becomes the job's result.
pub fn start<F, Fut>(app: &App, spec: JobSpec, work: F) -> Job
where
    F: FnOnce(JobCtx) -> Fut + Send + 'static,
    Fut: Future<Output = AppResult<Value>> + Send + 'static,
{
    let job = Job {
        id: new_id(),
        kind: spec.kind,
        title: spec.title,
        origin: if spec.origin.is_empty() {
            "user".into()
        } else {
            spec.origin
        },
        device_ids: spec.device_ids,
        total: spec.total,
        started_at: now_ms(),
        ..Job::default()
    };
    let cancel = app.shutdown_token().child_token();
    let state = app.state::<JobsState>();
    state.running.lock().insert(
        job.id.clone(),
        Running {
            job: job.clone(),
            cancel: cancel.clone(),
            announced_at: now_ms(),
        },
    );
    persist(app, &job);
    app.publish("jobs.changed", &summary(&job));

    let ctx = JobCtx {
        app: app.clone(),
        id: job.id.clone(),
        cancel,
    };
    let app = app.clone();
    let id = job.id.clone();
    tokio::spawn(async move {
        // A panic in a job must end the job, not leave it "running" forever.
        let outcome = tokio::spawn(work(ctx.clone())).await;
        let outcome = match outcome {
            Ok(result) => result,
            Err(join) if join.is_panic() => Err(AppError::internal(
                "the operation stopped unexpectedly (internal error)",
            )),
            Err(_) => Err(cancelled()),
        };
        finish(&app, &id, outcome, ctx.is_cancelled());
    });
    job
}

fn finish(app: &App, id: &str, outcome: AppResult<Value>, was_cancelled: bool) {
    let state = app.state::<JobsState>();
    let Some(mut entry) = state.running.lock().remove(id) else {
        return;
    };
    let job = &mut entry.job;
    job.finished_at = Some(now_ms());
    match outcome {
        Ok(result) => {
            job.state = JobState::Succeeded;
            job.result = (!result.is_null()).then_some(result);
        }
        Err(err) if err.code == "cancelled" || was_cancelled => {
            job.state = JobState::Cancelled;
            job.error = (err.code != "cancelled").then_some(err.message);
        }
        Err(err) => {
            job.state = JobState::Failed;
            job.error = Some(err.message);
        }
    }
    persist(app, job);
    prune(app);
    app.publish("jobs.changed", &summary(job));
}

fn persist(app: &App, job: &Job) {
    if let Err(err) = app.store().put(COLLECTION, &job.id, job) {
        tracing::error!(%err, job = %job.id, "job record could not be saved");
    }
}

fn prune(app: &App) {
    let Ok(mut jobs) = app.store().list::<Job>(COLLECTION) else {
        return;
    };
    jobs.retain(|job| job.state.is_finished());
    if jobs.len() <= HISTORY {
        return;
    }
    jobs.sort_by_key(|job| job.finished_at.unwrap_or(job.started_at));
    for job in &jobs[..jobs.len() - HISTORY] {
        let _ = app.store().delete(COLLECTION, &job.id);
    }
}

/// Wait for a job to end and return its final record.
pub async fn wait(app: &App, id: &str) -> AppResult<Job> {
    let mut events = app.subscribe();
    loop {
        let job = get(app, id)?;
        if job.state.is_finished() {
            return Ok(job);
        }
        // Wake on any event; a lagged receiver just re-checks.
        tokio::select! {
            _ = events.recv() => {}
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
        }
    }
}

pub fn get(app: &App, id: &str) -> AppResult<Job> {
    if let Some(entry) = app.state::<JobsState>().running.lock().get(id) {
        return Ok(entry.job.clone());
    }
    app.store().require(COLLECTION, id, "job")
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct JobFilter {
    /// `running`, `finished`, or a single state name.
    pub state: Option<String>,
    /// Exact kind or a prefix ending in `.` / `*` (`config.`).
    pub kind: Option<String>,
    pub device_id: Option<String>,
    pub limit: Option<usize>,
}

/// Newest first, without logs and results.
pub fn list(app: &App, filter: &JobFilter) -> AppResult<Vec<Job>> {
    let running: Vec<Job> = app
        .state::<JobsState>()
        .running
        .lock()
        .values()
        .map(|entry| summary(&entry.job))
        .collect();
    let live: std::collections::HashSet<String> =
        running.iter().map(|job| job.id.clone()).collect();
    let mut jobs: Vec<Job> = app
        .store()
        .list::<Job>(COLLECTION)?
        .into_iter()
        .filter(|job| !live.contains(&job.id))
        .map(|job| summary(&job))
        .chain(running)
        .filter(|job| {
            let state_ok = match filter.state.as_deref() {
                None | Some("") => true,
                Some("finished") => job.state.is_finished(),
                Some(name) => serde_json::to_value(job.state)
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s == name))
                    .unwrap_or(false),
            };
            let kind_ok = match filter.kind.as_deref() {
                None | Some("") => true,
                Some(kind) => match kind
                    .strip_suffix('*')
                    .or_else(|| kind.strip_suffix('.').map(|_| kind))
                {
                    Some(prefix) => job.kind.starts_with(prefix),
                    None => job.kind == kind,
                },
            };
            let device_ok = filter
                .device_id
                .as_ref()
                .is_none_or(|id| job.device_ids.contains(id));
            state_ok && kind_ok && device_ok
        })
        .collect();
    jobs.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    jobs.truncate(filter.limit.unwrap_or(100).clamp(1, 1000));
    Ok(jobs)
}

/// Ask a running job to stop. Returns whether it was running.
pub fn cancel(app: &App, id: &str) -> AppResult<bool> {
    let token = app
        .state::<JobsState>()
        .running
        .lock()
        .get(id)
        .map(|entry| entry.cancel.clone());
    match token {
        Some(token) => {
            token.cancel();
            Ok(true)
        }
        // Distinguish "already finished" from "no such job".
        None => app
            .store()
            .require::<Job>(COLLECTION, id, "job")
            .map(|_| false),
    }
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Serialize)]
struct Cancelled {
    cancelled: bool,
}

#[derive(Serialize)]
struct Cleared {
    removed: usize,
}

pub fn init(builder: &mut AppBuilder) {
    builder.state(JobsState::default());
    // Jobs that were running when the previous process ended did not finish.
    if let Ok(jobs) = builder.store().list::<Job>(COLLECTION) {
        for mut job in jobs.into_iter().filter(|job| !job.state.is_finished()) {
            job.state = JobState::Failed;
            job.error = Some("Nocturne was closed while this was running".into());
            job.finished_at = Some(now_ms());
            let _ = builder.store().put(COLLECTION, &job.id, &job);
        }
    }
    let router = &mut builder.router;
    router.add("jobs.list", |app: App, filter: JobFilter| async move {
        list(&app, &filter)
    });
    router.add("jobs.get", |app: App, params: IdParams| async move {
        get(&app, &params.id)
    });
    router.add("jobs.cancel", |app: App, params: IdParams| async move {
        Ok(Cancelled {
            cancelled: cancel(&app, &params.id)?,
        })
    });
    router.add(
        "jobs.clear",
        |app: App, _: crate::app::NoParams| async move {
            let finished: Vec<Job> = app
                .store()
                .list::<Job>(COLLECTION)?
                .into_iter()
                .filter(|job| job.state.is_finished())
                .collect();
            for job in &finished {
                app.store().delete(COLLECTION, &job.id)?;
            }
            app.publish(
                "jobs.changed",
                &serde_json::json!({"cleared": finished.len()}),
            );
            Ok(Cleared {
                removed: finished.len(),
            })
        },
    );
}

/// Shorthand for the usual shape of an RPC that starts a job: returns `{job_id}`.
pub fn started(job: &Job) -> Value {
    serde_json::json!({"job_id": job.id})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    fn app() -> App {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        init(&mut builder);
        builder.build()
    }

    #[tokio::test]
    async fn a_job_reports_progress_and_keeps_its_result() {
        let app = app();
        let mut events = app.subscribe();
        let job = start(
            &app,
            JobSpec::new("test.count", "Counting").total(3),
            |ctx| async move {
                for step in 1..=3u64 {
                    ctx.progress(step, format!("step {step}"));
                    ctx.info(format!("did {step}"));
                }
                ctx.device("dev1", "warn", "slow");
                Ok(json!({"counted": 3}))
            },
        );
        assert_eq!(job.state, JobState::Running);
        let done = wait(&app, &job.id).await.unwrap();
        assert_eq!(done.state, JobState::Succeeded);
        assert_eq!((done.done, done.total), (3, 3));
        assert_eq!(done.result, Some(json!({"counted": 3})));
        assert_eq!(done.log.len(), 4);
        assert_eq!(done.log[3].device_id.as_deref(), Some("dev1"));
        assert!(done.finished_at.is_some());

        // First and last announcements always go out; events never carry the log.
        let mut states = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.topic == "jobs.changed" {
                assert!(event.payload["log"].as_array().unwrap().is_empty());
                states.push(event.payload["state"].as_str().unwrap().to_string());
            }
        }
        assert_eq!(states.first().map(String::as_str), Some("running"));
        assert_eq!(states.last().map(String::as_str), Some("succeeded"));

        let listed = app
            .rpc("jobs.list", json!({"kind": "test.", "state": "finished"}))
            .await
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(
            app.rpc("jobs.get", json!({"id": job.id})).await.unwrap()["result"]["counted"],
            3
        );
        assert_eq!(
            app.rpc("jobs.get", json!({"id": "nope"}))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[tokio::test]
    async fn failures_and_panics_end_the_job() {
        let app = app();
        let failed = start(&app, JobSpec::new("test.fail", "Failing"), |_| async {
            Err(AppError::unavailable("device did not answer"))
        });
        let failed = wait(&app, &failed.id).await.unwrap();
        assert_eq!(
            (failed.state, failed.error.as_deref()),
            (JobState::Failed, Some("device did not answer"))
        );

        let panicked = start(&app, JobSpec::new("test.panic", "Panicking"), |_| async {
            if true {
                panic!("boom");
            }
            Ok(Value::Null)
        });
        let panicked = wait(&app, &panicked.id).await.unwrap();
        assert_eq!(panicked.state, JobState::Failed);
        assert!(panicked.error.unwrap().contains("unexpectedly"));
    }

    #[tokio::test]
    async fn cancelling_stops_the_work() {
        let app = app();
        let job = start(
            &app,
            JobSpec::new("test.slow", "Waiting"),
            |ctx| async move {
                ctx.race(async {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(Value::Null)
                })
                .await
            },
        );
        assert_eq!(
            app.rpc("jobs.list", json!({"state": "running"}))
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            app.rpc("jobs.cancel", json!({"id": job.id})).await.unwrap()["cancelled"],
            true
        );
        let done = wait(&app, &job.id).await.unwrap();
        assert_eq!(done.state, JobState::Cancelled);
        assert_eq!(done.error, None);
        // Cancelling again reports that nothing was running; unknown ids are errors.
        assert_eq!(
            app.rpc("jobs.cancel", json!({"id": job.id})).await.unwrap()["cancelled"],
            false
        );
        assert_eq!(
            app.rpc("jobs.cancel", json!({"id": "nope"}))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    #[tokio::test]
    async fn for_each_bounds_concurrency_and_keeps_order() {
        let app = app();
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (peak2, live2) = (peak.clone(), live.clone());
        let job = start(
            &app,
            JobSpec::new("test.each", "Each").total(10),
            move |ctx| async move {
                let results = ctx
                    .for_each((0..10u64).collect(), 3, move |_, n| {
                        let (peak, live) = (peak2.clone(), live2.clone());
                        async move {
                            use std::sync::atomic::Ordering::SeqCst;
                            let now = live.fetch_add(1, SeqCst) + 1;
                            peak.fetch_max(now, SeqCst);
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            live.fetch_sub(1, SeqCst);
                            if n == 4 {
                                Err(AppError::timeout("no answer"))
                            } else {
                                Ok(n * 2)
                            }
                        }
                    })
                    .await;
                let values: Vec<Value> = results
                    .into_iter()
                    .map(|result| match result {
                        Ok(n) => json!(n),
                        Err(err) => json!(err.code),
                    })
                    .collect();
                Ok(json!(values))
            },
        );
        let done = wait(&app, &job.id).await.unwrap();
        assert_eq!(
            done.result.unwrap(),
            json!([0, 2, 4, 6, "timeout", 10, 12, 14, 16, 18])
        );
        assert_eq!(done.done, 10);
        assert!(peak.load(std::sync::atomic::Ordering::SeqCst) <= 3);
    }

    #[tokio::test]
    async fn interrupted_jobs_are_marked_failed_on_restart_and_history_is_cleared() {
        let dir = tempfile::tempdir().unwrap();
        {
            let builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
            let orphan = Job {
                id: "orphan".into(),
                kind: "x".into(),
                title: "X".into(),
                started_at: 1,
                ..Job::default()
            };
            builder.store().put(COLLECTION, "orphan", &orphan).unwrap();
        }
        let mut builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
        init(&mut builder);
        let app = builder.build();
        let orphan = get(&app, "orphan").unwrap();
        assert_eq!(orphan.state, JobState::Failed);
        assert!(orphan.error.unwrap().contains("closed"));
        assert_eq!(
            app.rpc("jobs.clear", Value::Null).await.unwrap()["removed"],
            1
        );
        assert!(
            app.rpc("jobs.list", Value::Null)
                .await
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
