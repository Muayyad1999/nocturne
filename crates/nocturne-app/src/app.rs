//! The application object: storage, vault, event bus, RPC router and the
//! per-module state every feature hangs off.
//!
//! Feature modules do not know about each other's internals. Each one exposes
//! a single `init(&mut AppBuilder)` that registers its RPC methods, its state
//! and its background tasks; [`crate::start`] calls them in order. Adding a
//! feature is one new file and one line there.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::store::{LogEntry, Store};
use crate::util::now_ms;
use crate::vault::Vault;

/// Something that happened, pushed to every connected UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    /// Dotted name, e.g. `devices.changed`, `terminal.data`, `alerts.raised`.
    pub topic: String,
    pub payload: Value,
    /// Milliseconds since the epoch.
    pub at: i64,
}

type Handler =
    Arc<dyn Fn(App, Value) -> Pin<Box<dyn Future<Output = AppResult<Value>> + Send>> + Send + Sync>;

/// The RPC method table.
#[derive(Default)]
pub struct Router {
    handlers: HashMap<&'static str, Handler>,
}

impl Router {
    /// Register `name`. The handler takes the app and its deserialised
    /// parameters and returns anything serialisable.
    ///
    /// Parameters are deserialised strictly from the JSON object the UI
    /// sends; a method without parameters takes [`NoParams`].
    pub fn add<P, R, F, Fut>(&mut self, name: &'static str, handler: F)
    where
        P: DeserializeOwned + Send + 'static,
        R: Serialize,
        F: Fn(App, P) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AppResult<R>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let wrapped: Handler = Arc::new(move |app, params| {
            let handler = handler.clone();
            Box::pin(async move {
                // `null`/missing params are an empty object, so `NoParams` and all-optional structs accept them.
                let params = if params.is_null() {
                    Value::Object(Default::default())
                } else {
                    params
                };
                let params: P = serde_json::from_value(params).map_err(|err| {
                    AppError::invalid(format!("invalid parameters for `{name}`: {err}"))
                })?;
                let result = handler(app, params).await?;
                Ok(serde_json::to_value(result)?)
            })
        });
        if self.handlers.insert(name, wrapped).is_some() {
            // Two modules claiming one method name is a programming error worth failing loudly on.
            panic!("RPC method `{name}` registered twice");
        }
    }

    pub fn methods(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.handlers.keys().copied().collect();
        names.sort_unstable();
        names
    }
}

/// Parameters of a method that takes none.
#[derive(Debug, Default, Deserialize)]
pub struct NoParams {}

/// Where the application keeps its files.
#[derive(Clone, Debug)]
pub struct AppConfig {
    /// Directory for the database, vault key, user profiles and logs.
    /// `None` keeps everything in memory (tests).
    pub data_dir: Option<PathBuf>,
    /// Whether callers of the RPC surface sit at this machine. The desktop
    /// app: yes. A server reached over the network: no — features that touch
    /// the local machine (a local shell, reading and writing local files)
    /// are then refused.
    pub local_access: bool,
}

impl AppConfig {
    pub fn in_memory() -> AppConfig {
        AppConfig {
            data_dir: None,
            local_access: true,
        }
    }

    pub fn at(dir: impl Into<PathBuf>) -> AppConfig {
        AppConfig {
            data_dir: Some(dir.into()),
            local_access: true,
        }
    }

    /// For a backend whose users are remote (server mode).
    pub fn remote_users(mut self) -> AppConfig {
        self.local_access = false;
        self
    }
}

/// How many events the bus keeps for readers that are behind.
const EVENT_BACKLOG: usize = 1024;

type StateMap = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;
type Starter = Box<dyn FnOnce(&App) + Send>;

/// Collects what feature modules register before the app starts.
pub struct AppBuilder {
    pub router: Router,
    config: AppConfig,
    store: Arc<Store>,
    vault: Arc<Vault>,
    states: StateMap,
    starters: Vec<Starter>,
    /// Held for the life of the app: one process per data directory.
    lock: Option<std::fs::File>,
}

/// How long a held lock is waited for before it counts as another instance.
///
/// An instance that is closing can hold the directory a moment after it has
/// let go of it: macOS CI once refused the second of two opens in one test,
/// where the first had already been dropped. An instance that really is
/// running still holds the lock when this has passed, and is reported then.
const LOCK_PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

/// Take the exclusive lock on a data directory, creating the directory if needed.
fn lock_data_dir(dir: &Path) -> AppResult<std::fs::File> {
    std::fs::create_dir_all(dir).map_err(|err| {
        AppError::internal(format!(
            "the data directory {} cannot be created: {err}",
            dir.display()
        ))
    })?;
    let path = dir.join("nocturne.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|err| AppError::internal(format!("{} cannot be opened: {err}", path.display())))?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < LOCK_PATIENCE => {
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(AppError::conflict(format!(
                    "Nocturne is already running with the data directory {} (the desktop app and the server cannot share one). Close the other instance, or start this one with a different data directory.",
                    dir.display()
                )));
            }
            Err(std::fs::TryLockError::Error(err)) => {
                return Err(AppError::internal(format!(
                    "{} cannot be locked: {err}",
                    path.display()
                )));
            }
        }
    }
}

impl AppBuilder {
    pub fn new(config: AppConfig) -> AppResult<AppBuilder> {
        let lock = match &config.data_dir {
            Some(dir) => Some(lock_data_dir(dir)?),
            None => None,
        };
        let store = Arc::new(match &config.data_dir {
            Some(dir) => Store::open(&dir.join("nocturne.db"))?,
            None => Store::in_memory()?,
        });
        let key_path = config.data_dir.as_ref().map(|dir| dir.join("vault.key"));
        let vault = Arc::new(Vault::open(store.clone(), key_path.as_deref())?);
        Ok(AppBuilder {
            router: Router::default(),
            config,
            store,
            vault,
            states: HashMap::new(),
            starters: Vec::new(),
            lock,
        })
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn data_dir(&self) -> Option<&Path> {
        self.config.data_dir.as_deref()
    }

    /// Make a module's runtime state available through [`App::state`].
    pub fn state<T: Send + Sync + 'static>(&mut self, state: T) {
        self.states.insert(TypeId::of::<T>(), Arc::new(state));
    }

    /// A state registered earlier, for modules that configure one another at init time.
    pub fn get<T: Send + Sync + 'static>(&self) -> Arc<T> {
        self.states
            .get(&TypeId::of::<T>())
            .and_then(|state| state.clone().downcast::<T>().ok())
            .unwrap_or_else(|| {
                panic!(
                    "module state `{}` must be registered first",
                    std::any::type_name::<T>()
                )
            })
    }

    /// Run `starter` once the app is assembled (spawn background tasks here).
    pub fn on_start(&mut self, starter: impl FnOnce(&App) + Send + 'static) {
        self.starters.push(Box::new(starter));
    }

    pub fn build(self) -> App {
        // A broadcast channel keeps its whole ring resident, so this bound is
        // not "how far a slow reader may fall behind" but "how much memory the
        // bus costs". Most events are a few hundred bytes; terminal output is
        // capped at 32 KiB each, which is what makes the size matter. A reader
        // that falls further behind is told it missed events and refetches.
        let (events, _) = broadcast::channel(EVENT_BACKLOG);
        let app = App {
            inner: Arc::new(Inner {
                store: self.store,
                vault: self.vault,
                router: self.router,
                states: self.states,
                events,
                shutdown: CancellationToken::new(),
                local_access: self.config.local_access,
                data_dir: self.config.data_dir,
                _lock: self.lock,
            }),
        };
        for starter in self.starters {
            starter(&app);
        }
        app
    }
}

struct Inner {
    store: Arc<Store>,
    vault: Arc<Vault>,
    router: Router,
    states: StateMap,
    events: broadcast::Sender<Event>,
    shutdown: CancellationToken,
    data_dir: Option<PathBuf>,
    local_access: bool,
    _lock: Option<std::fs::File>,
}

/// A cheap, cloneable handle to the running application.
#[derive(Clone)]
pub struct App {
    inner: Arc<Inner>,
}

impl App {
    pub fn store(&self) -> &Arc<Store> {
        &self.inner.store
    }

    pub fn vault(&self) -> &Arc<Vault> {
        &self.inner.vault
    }

    pub fn data_dir(&self) -> Option<&Path> {
        self.inner.data_dir.as_deref()
    }

    /// Whether RPC callers are at this machine (see [`AppConfig::local_access`]).
    pub fn local_access(&self) -> bool {
        self.inner.local_access
    }

    /// `Err(forbidden)` unless callers are at this machine.
    pub fn require_local_access(&self, what: &str) -> AppResult<()> {
        if self.inner.local_access {
            Ok(())
        } else {
            Err(AppError::forbidden(format!(
                "{what} is only available in the desktop app, not when Nocturne runs as a server"
            )))
        }
    }

    /// A module's runtime state, registered with [`AppBuilder::state`].
    pub fn state<T: Send + Sync + 'static>(&self) -> Arc<T> {
        self.inner
            .states
            .get(&TypeId::of::<T>())
            .and_then(|state| state.clone().downcast::<T>().ok())
            // Missing state means a module was used without being initialised: a wiring bug.
            .unwrap_or_else(|| {
                panic!(
                    "module state `{}` was never registered",
                    std::any::type_name::<T>()
                )
            })
    }

    /// Call an RPC method by name.
    pub async fn rpc(&self, method: &str, params: Value) -> AppResult<Value> {
        let handler = self
            .inner
            .router
            .handlers
            .get(method)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    "unknown_method",
                    format!("there is no method named `{method}`"),
                )
            })?;
        let started = std::time::Instant::now();
        let result = handler(self.clone(), params).await;
        let elapsed = started.elapsed();
        match &result {
            Err(err) if err.code == "internal" => tracing::error!(method, %err, "rpc failed"),
            Err(err) => tracing::debug!(method, code = err.code, %err, "rpc refused"),
            Ok(_) if elapsed.as_millis() > 1500 => {
                tracing::debug!(method, ms = elapsed.as_millis() as u64, "slow rpc")
            }
            Ok(_) => {}
        }
        result
    }

    pub fn methods(&self) -> Vec<&'static str> {
        self.inner.router.methods()
    }

    /// Announce something to every connected UI. Nobody listening is fine.
    pub fn publish<T: Serialize>(&self, topic: &str, payload: &T) {
        match serde_json::to_value(payload) {
            Ok(payload) => {
                let _ = self.inner.events.send(Event {
                    topic: topic.to_string(),
                    payload,
                    at: now_ms(),
                });
            }
            Err(err) => tracing::error!(topic, %err, "event payload could not be serialised"),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.inner.events.subscribe()
    }

    /// Record a user-visible action in the audit trail.
    pub fn audit(
        &self,
        action: &str,
        device_id: Option<&str>,
        message: impl Into<String>,
        detail: Value,
    ) {
        let entry = LogEntry {
            id: 0,
            ts: now_ms(),
            stream: "audit".into(),
            device_id: device_id.map(str::to_string),
            severity: Some(6),
            source: action.to_string(),
            message: message.into(),
            body: if detail.is_null() { None } else { Some(detail) },
        };
        match self.inner.store.log_append(&entry) {
            Ok(id) => self.publish("audit.appended", &LogEntry { id, ..entry }),
            Err(err) => tracing::error!(%err, "audit entry could not be written"),
        }
    }

    /// Cancelled when the application is shutting down.
    pub fn shutdown_token(&self) -> CancellationToken {
        self.inner.shutdown.clone()
    }

    /// Run a background task until it finishes or the app shuts down.
    pub fn spawn<F>(&self, name: &'static str, task: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let token = self.inner.shutdown.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = token.cancelled() => tracing::debug!(name, "background task stopped"),
                _ = task => {}
            }
        });
    }

    /// Stop background tasks. The handle stays usable for in-flight calls.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Deserialize)]
    struct AddParams {
        a: i64,
        #[serde(default)]
        b: i64,
    }

    struct Counter(std::sync::atomic::AtomicU64);

    fn app() -> App {
        let mut builder = AppBuilder::new(AppConfig::in_memory()).unwrap();
        builder.state(Counter(Default::default()));
        builder
            .router
            .add("math.add", |app: App, params: AddParams| async move {
                app.state::<Counter>()
                    .0
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                app.publish("math.added", &json!({"sum": params.a + params.b}));
                Ok(params.a + params.b)
            });
        builder
            .router
            .add("math.fail", |_app: App, _params: NoParams| async move {
                Err::<(), _>(AppError::conflict("already exists"))
            });
        builder.build()
    }

    #[tokio::test]
    async fn rpc_dispatches_validates_and_reports_errors() {
        let app = app();
        let mut events = app.subscribe();
        assert_eq!(
            app.rpc("math.add", json!({"a": 2, "b": 3})).await.unwrap(),
            json!(5)
        );
        assert_eq!(
            app.rpc("math.add", json!({"a": 2})).await.unwrap(),
            json!(2)
        );
        assert_eq!(
            app.state::<Counter>()
                .0
                .load(std::sync::atomic::Ordering::Relaxed),
            2
        );
        let event = events.recv().await.unwrap();
        assert_eq!(
            (event.topic.as_str(), &event.payload),
            ("math.added", &json!({"sum": 5}))
        );

        let invalid = app.rpc("math.add", json!({"b": 1})).await.unwrap_err();
        assert_eq!(invalid.code, "invalid");
        assert!(
            invalid.message.contains("math.add") && invalid.message.contains("missing field `a`"),
            "{}",
            invalid.message
        );
        assert_eq!(
            app.rpc("math.fail", Value::Null).await.unwrap_err().code,
            "conflict"
        );
        assert_eq!(
            app.rpc("nope", Value::Null).await.unwrap_err().code,
            "unknown_method"
        );
        assert_eq!(app.methods(), vec!["math.add", "math.fail"]);
    }

    #[tokio::test]
    async fn audit_entries_are_stored_and_announced() {
        let app = app();
        let mut events = app.subscribe();
        app.audit(
            "device.create",
            Some("dev1"),
            "Created device CORE-1",
            json!({"address": "10.0.0.1"}),
        );
        let event = events.recv().await.unwrap();
        assert_eq!(event.topic, "audit.appended");
        let stored = app
            .store()
            .log_query(&crate::store::LogQuery {
                stream: Some("audit".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].source, "device.create");
        assert_eq!(stored[0].device_id.as_deref(), Some("dev1"));
    }

    #[tokio::test]
    async fn background_tasks_stop_at_shutdown() {
        let app = app();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        app.spawn("forever", async move {
            let _keep = tx;
            std::future::pending::<()>().await;
        });
        app.shutdown();
        // The task is dropped, which drops the sender.
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), rx)
                .await
                .unwrap()
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_data_directory_belongs_to_one_instance() {
        let dir = tempfile::tempdir().unwrap();
        let first = AppBuilder::new(AppConfig::at(dir.path())).unwrap().build();
        let second = AppBuilder::new(AppConfig::at(dir.path()))
            .err()
            .expect("the directory is in use");
        assert_eq!(second.code, "conflict");
        assert!(
            second.message.contains("already running"),
            "{}",
            second.message
        );
        // A builder that is dropped, or an app whose last handle is gone, releases it.
        drop(first);
        drop(AppBuilder::new(AppConfig::at(dir.path())).unwrap());
        AppBuilder::new(AppConfig::at(dir.path())).unwrap();
    }

    #[tokio::test]
    async fn an_instance_that_lets_go_a_moment_later_is_waited_for_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        let closing = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
        let letting_go = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            drop(closing);
        });
        AppBuilder::new(AppConfig::at(dir.path()))
            .expect("a lock released while the second instance starts is not a conflict");
        letting_go.join().unwrap();
    }
}
