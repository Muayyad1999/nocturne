//! Syslog and SNMP trap reception, flow collection, log search, live tail,
//! pattern rules, device log buffers and forwarding to a log platform.
//!
//! Specification: `docs/dev/features/logs.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! What the network says about itself — syslog over UDP and TCP, SNMP
//! notifications over UDP — attributed to the device that sent it, stored
//! with retention, searchable over a time range, followed live, and turned
//! into signals the rest of the application reacts to.
//!
//! Three things run through the whole crate:
//!
//! * **A gap is a gap.** A line nobody sent is not a line. Nothing here
//!   invents an entry, a timestamp or a device: a line from an address no
//!   device claims is stored with no device and the address is listed with
//!   the reason, and a line dropped under load is counted where the user can
//!   see it rather than quietly forgotten.
//! * **Rate is the risk.** A chattery campus sends tens of thousands of
//!   lines a minute. The sockets never block, the queue between them and the
//!   database is bounded, writes are batched, the live tail samples rather
//!   than flooding the event bus — and says that it sampled — and what the
//!   rules remember per device and instance is capped and counted.
//! * **Secrets are not stored.** Device output can carry a credential, so
//!   everything kept goes through `nocturne_ai`'s redactor first
//!   ([`guard`]). And a trap's community *is* a credential: it is compared
//!   with the one in the vault and dropped where the datagram is decoded
//!   ([`traps::admit`]), so that no row, event, status, audit entry, error
//!   or log line can contain one.
//!
//! The flow collector ([`flows`]) is the odd one out: it keeps counts, not
//! rows. NetFlow and IPFIX are added up in memory into one-minute buckets
//! of the biggest talkers, kept a day, and nothing of a flow is written to
//! the database, the tail, a forwarder or a rule.

pub mod attribute;
pub mod correlate;
pub mod db;
pub mod flows;
pub mod forward;
pub mod guard;
pub mod ingest;
pub mod model;
pub mod pull;
pub mod query;
pub mod receiver;
pub mod rules;
pub mod settings;
pub mod setup;
pub mod state;
pub mod status;
pub mod syslog;
pub mod tail;
pub mod traps;

use nocturne_app::{App, AppError, NoParams, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::LogRule;
use crate::query::Filter;
use crate::state::LogsState;

/// The stream `App::audit` writes. `logs.clear` refuses it.
pub const AUDIT_STREAM: &str = "audit";

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    nocturne_app::settings::declare(
        builder,
        settings::SECTION,
        &settings::LogsSettings::default(),
    );
    // Without its tables the feature cannot store anything; failing here
    // means a broken database, not a condition to carry on with.
    db::init(builder.store()).expect("the log feature's tables can be created");
    builder.state(LogsState::default());
    register(&mut builder.router);
    builder.on_start(|app| {
        if let Err(err) = rules::ensure_builtins(app) {
            tracing::error!(%err, "the built-in log rules could not be written");
        }
        receiver::spawn(app);
        app.spawn("logs.tail", tail::publisher(app.clone()));
        app.spawn("logs.retention", ingest::retention(app.clone()));
        app.spawn("logs.rules.sweep", rules::sweeper(app.clone()));
        app.spawn("logs.rules.courier", rules::courier(app.clone()));
        forward::start(app);
    });
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct QueryParams {
    #[serde(flatten)]
    filter: Filter,
    /// Page backwards: only entries older than this id.
    before_id: Option<i64>,
    after_id: Option<i64>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct HistogramParams {
    #[serde(flatten)]
    filter: Filter,
    since: Option<i64>,
    until: Option<i64>,
    buckets: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TopParams {
    #[serde(flatten)]
    filter: Filter,
    by: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct BurstParams {
    #[serde(flatten)]
    filter: Filter,
    min_count: Option<i64>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FollowParams {
    #[serde(flatten)]
    filter: Filter,
    /// Renew an existing follow instead of opening another.
    follow_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct DeviceParams {
    device_id: String,
}

#[derive(Debug, Deserialize)]
struct IngestParams {
    device_id: String,
    lines: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AroundParams {
    entry_id: Option<i64>,
    at: Option<i64>,
    device_id: Option<String>,
    window_s: Option<u32>,
    limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClearParams {
    stream: Option<String>,
    before: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RuleTestParams {
    rule: LogRule,
    line: String,
}

pub fn register(router: &mut Router) {
    router.add("logs.query", |app: App, params: QueryParams| async move {
        let (entries, has_more) = query::page(
            &app,
            &params.filter,
            params.before_id,
            params.after_id,
            params.limit.unwrap_or(200),
        )?;
        Ok(json!({"entries": entries, "has_more": has_more}))
    });
    router.add(
        "logs.histogram",
        |app: App, params: HistogramParams| async move {
            let until = params.until.unwrap_or_else(nocturne_app::util::now_ms);
            let since = params.since.unwrap_or(until - 3_600_000);
            query::histogram(
                &app,
                &params.filter,
                since,
                until,
                params.buckets.unwrap_or(60),
            )
        },
    );
    router.add("logs.top", |app: App, params: TopParams| async move {
        query::top(
            &app,
            &params.filter,
            params.by.as_deref().unwrap_or("device"),
            params.limit.unwrap_or(20),
        )
    });
    router.add("logs.bursts", |app: App, params: BurstParams| async move {
        query::bursts(
            &app,
            &params.filter,
            params.min_count.unwrap_or(5),
            params.limit.unwrap_or(50),
        )
    });
    router.add("logs.status", |app: App, _: NoParams| async move {
        status::snapshot(&app)
    });
    router.add("logs.sources", |app: App, _: NoParams| async move {
        query::sources(&app)
    });
    router.add("logs.follow", |app: App, params: FollowParams| async move {
        tail::follow(&app, params.filter, params.follow_id)
    });
    router.add("logs.unfollow", |app: App, params: IdParams| async move {
        Ok(json!({"closed": tail::unfollow(&app, &params.id)}))
    });
    router.add("logs.around", |app: App, params: AroundParams| async move {
        correlate::around(
            &app,
            params.entry_id,
            params.at,
            params.device_id,
            params.window_s.unwrap_or(300),
            params.limit.unwrap_or(100),
        )
        .await
    });
    router.add("logs.pull", |app: App, params: DeviceParams| async move {
        pull::pull(&app, &params.device_id).await
    });
    router.add("logs.ingest", |app: App, params: IngestParams| async move {
        let stored = ingest::from_lab(&app, &params.device_id, &params.lines)?;
        Ok(json!({"stored": stored}))
    });
    router.add(
        "logs.device_setup",
        |app: App, params: DeviceParams| async move { setup::device_setup(&app, &params.device_id) },
    );
    router.add("logs.rules.list", |app: App, _: NoParams| async move {
        rules::list(&app)
    });
    router.add("logs.rules.save", |app: App, rule: LogRule| async move {
        rules::save(&app, rule)
    });
    router.add(
        "logs.rules.delete",
        |app: App, params: IdParams| async move {
            rules::delete(&app, &params.id)?;
            Ok(json!({"deleted": params.id}))
        },
    );
    router.add(
        "logs.rules.test",
        |app: App, params: RuleTestParams| async move {
            rules::test(&app, &params.rule, &params.line)
        },
    );
    router.add("logs.clear", |app: App, params: ClearParams| async move {
        let stream = params.stream.unwrap_or_else(|| db::STREAM.to_string());
        if stream.is_empty() || stream == "*" {
            return Err(AppError::invalid(
                "`logs.clear` needs the name of one stream (syslog, trap, event)",
            ));
        }
        if stream.eq_ignore_ascii_case(AUDIT_STREAM) {
            // The audit trail is evidence, and this call is one of the
            // things it records: the API that is audited does not get to
            // erase the audit.
            return Err(AppError::forbidden(
                "the audit trail cannot be cleared: it is the record of what was done here, \
                 this call included. It ages out with the log retention setting and no sooner",
            ));
        }
        let removed = query::clear(&app, &stream, params.before)?;
        status::publish(&app);
        Ok::<Value, AppError>(json!({"removed": removed, "stream": stream}))
    });
    router.add("logs.restart", |app: App, _: NoParams| async move {
        receiver::apply(&app).await?;
        status::snapshot(&app)
    });
    flows::register(router);
    forward::register(router);
}
