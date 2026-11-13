//! The tool layer: what the model can do, and the gate every call goes
//! through.
//!
//! A tool is a name the model sees, a description written for a model, a JSON
//! Schema for its arguments, the RPC methods it may use (see [`boundary`]),
//! and a handler that shapes what comes back. Handlers never touch the `App`
//! directly: they are handed a [`ToolCtx`], and everything they can reach goes
//! through [`ToolCtx::rpc`], which
//!
//! * refuses any method the tool did not declare, and any method that is not
//!   on the allow-list (it cannot even be named: see [`boundary::Method`]);
//! * runs the call in its own task, so cancelling a turn stops the waiting,
//!   never the device read that is already in flight;
//! * redacts every string of the result — and of the error — before the
//!   handler sees it, so no cap, no formatting mistake and no forgetful
//!   handler can leak a secret.

pub mod boundary;
mod changes;
mod data;
mod design;
mod devices;
mod net;
mod watch;

use std::sync::Arc;

use futures::future::BoxFuture;
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::{App, profiles};
use nocturne_drivers::policy::Classification;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::redact::ChatRedactor;
use crate::settings::Caps;
use crate::view::Link;
use boundary::Method;

/// What a handler returns: the text the model reads, a line for the UI, and
/// optionally the thing the result is about.
pub struct ToolOk {
    pub content: String,
    pub summary: String,
    pub link: Option<Link>,
}

impl ToolOk {
    pub fn new(content: impl Into<String>, summary: impl Into<String>) -> ToolOk {
        ToolOk {
            content: content.into(),
            summary: summary.into(),
            link: None,
        }
    }

    pub fn with_link(mut self, link: Link) -> ToolOk {
        self.link = Some(link);
        self
    }
}

type Handler = for<'a> fn(&'a ToolCtx, Value) -> BoxFuture<'a, AppResult<ToolOk>>;

/// One tool, as the model is offered it.
pub struct Tool {
    pub name: &'static str,
    /// Short human title for the UI ("Read configuration").
    pub title: &'static str,
    /// Written for the model: when to use it and what comes back.
    pub description: &'static str,
    /// The same, shorter, for a model with little context to spare.
    pub brief: &'static str,
    /// Methods the tool needs; without them it is not offered at all.
    pub required: &'static [Method],
    /// Methods it uses when they exist.
    pub optional: &'static [Method],
    /// Part of the core set a small-context model gets.
    pub core: bool,
    /// Opens a session to a device (or starts a job that does), so it is what
    /// `confirm_reads` asks about and what is audited.
    pub touches_device: bool,
    pub parallel_safe: bool,
    pub schema: fn() -> Value,
    pub run: Handler,
}

impl Tool {
    /// Every method this tool can possibly reach.
    pub fn reachable(&self) -> impl Iterator<Item = Method> + '_ {
        self.required.iter().chain(self.optional.iter()).copied()
    }

    pub fn description_for(&self, small: bool) -> &'static str {
        if small { self.brief } else { self.description }
    }

    pub fn spec(&self, small: bool) -> nocturne_ai::ToolSpec {
        nocturne_ai::ToolSpec {
            name: self.name.to_string(),
            description: self.description_for(small).to_string(),
            input_schema: (self.schema)(),
            parallel_safe: self.parallel_safe,
        }
    }
}

/// Every tool that exists, in a fixed order.
pub fn all() -> &'static [Tool] {
    const TOOLS: &[Tool] = &[
        devices::LIST_DEVICES,
        devices::GET_DEVICE,
        devices::RUN_COMMANDS,
        devices::DEVICE_PING,
        devices::EXPLAIN_TERMINAL,
        data::COLLECT_DATA,
        data::GET_COLLECTED,
        data::GET_CONFIG,
        data::SEARCH_CONFIGS,
        data::DIFF_CONFIG,
        data::BACKUP_CONFIG,
        data::COMPLIANCE_RESULTS,
        watch::LIST_ALERTS,
        watch::GET_METRICS,
        watch::INTERFACE_STATUS,
        watch::QUERY_LOGS,
        watch::LIST_INSIGHTS,
        net::TOPOLOGY_NEIGHBORS,
        net::FIND_PATH,
        net::LOCATE_HOST,
        net::IPAM_SEARCH,
        net::NEXT_FREE_ADDRESS,
        net::SUBNET_CALC,
        net::VLSM_PLAN,
        net::PING,
        net::TRACEROUTE,
        net::PORT_CHECK,
        net::DNS_LOOKUP,
        net::TLS_INSPECT,
        changes::PROPOSE_CHANGE,
        changes::PREVIEW_CHANGE,
        changes::GET_CHANGE,
        changes::LIST_CHANGES,
        changes::APPLY_CHANGE,
        changes::LIST_TEMPLATES,
        changes::RENDER_TEMPLATE,
        changes::VALIDATE_PROFILE,
        changes::SEARCH_NOTES,
        design::DESIGN_NETWORK,
        design::CREATE_LAB,
        design::LIST_LABS,
        design::LAB_CONNECTIVITY,
        design::LAB_PROBE,
    ];
    TOOLS
}

pub fn find(name: &str) -> Option<&'static Tool> {
    all().iter().find(|tool| tool.name == name)
}

/// The tools this build can actually offer: a feature whose methods are not
/// registered simply has no tool. `small` drops everything outside the core
/// set, for a model with little room.
pub fn available(app: &App, small: bool) -> Vec<&'static Tool> {
    let registered = app.methods();
    all()
        .iter()
        .filter(|tool| !small || tool.core)
        .filter(|tool| {
            tool.required
                .iter()
                .all(|method| registered.contains(&method.name()))
        })
        .collect()
}

/// Every RPC method the whole tool set can reach. The allow-list test is
/// this, compared with [`boundary::ALLOWED`].
pub fn reachable_methods() -> Vec<Method> {
    let mut methods: Vec<Method> = all().iter().flat_map(Tool::reachable).collect();
    methods.sort_unstable();
    methods.dedup();
    methods
}

/// Everything a handler is allowed to touch.
pub struct ToolCtx {
    app: App,
    tool: &'static Tool,
    chat_id: String,
    call_id: String,
    redactor: Arc<ChatRedactor>,
    caps: Caps,
    cancel: CancellationToken,
    link: Mutex<Option<Link>>,
}

impl ToolCtx {
    pub fn new(
        app: App,
        tool: &'static Tool,
        chat_id: &str,
        call_id: &str,
        redactor: Arc<ChatRedactor>,
        caps: Caps,
        cancel: CancellationToken,
    ) -> ToolCtx {
        ToolCtx {
            app,
            tool,
            chat_id: chat_id.to_string(),
            call_id: call_id.to_string(),
            redactor,
            caps,
            cancel,
            link: Mutex::new(None),
        }
    }

    pub fn caps(&self) -> Caps {
        self.caps
    }

    pub fn redactor(&self) -> &ChatRedactor {
        &self.redactor
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Note what this result is about, for the card the UI draws.
    pub fn link(&self, link: Link) {
        *self.link.lock() = Some(link);
    }

    pub fn take_link(&self) -> Option<Link> {
        self.link.lock().take()
    }

    /// Is an optional method available in this build?
    pub fn has(&self, method: Method) -> bool {
        self.tool.reachable().any(|allowed| allowed == method)
            && self.app.methods().contains(&method.name())
    }

    /// Call an allow-listed method. The only way out of a tool handler.
    pub async fn rpc(&self, method: Method, params: Value) -> AppResult<Value> {
        if !self.tool.reachable().any(|allowed| allowed == method) {
            // A tool reaching for a method it did not declare is a bug in this
            // crate, not something the model can cause; fail loudly and call
            // nothing.
            tracing::error!(tool = self.tool.name, %method, "tool called an undeclared method");
            return Err(AppError::internal(format!(
                "the tool `{}` is not allowed to call `{method}`",
                self.tool.name
            )));
        }
        if !self.app.methods().contains(&method.name()) {
            return Err(AppError::not_found(format!(
                "`{method}` (that part of Nocturne is not in this build)"
            )));
        }
        let app = self.app.clone();
        let name = method.name();
        // In its own task: cancelling a turn stops us waiting, it does not
        // abort a device read half-way through.
        let call = tokio::spawn(async move { app.rpc(name, params).await });
        let outcome = tokio::select! {
            biased;
            joined = call => match joined {
                Ok(result) => result,
                Err(err) => Err(AppError::internal(format!("`{method}` did not finish: {err}"))),
            },
            () = self.cancel.cancelled() => {
                return Err(AppError::new(
                    "cancelled",
                    format!("The user stopped this turn while `{method}` was running; anything already sent to a device finishes on its own."),
                ));
            }
        };
        match outcome {
            Ok(value) => Ok(self.redactor.scrub_json(&value)),
            Err(err) => Err(AppError {
                code: err.code,
                message: self.redactor.scrub(&err.message),
                details: err
                    .details
                    .map(|details| self.redactor.scrub_json(&details)),
            }),
        }
    }

    /// How the platform's own policy classifies a command — checked before a
    /// session is opened, so a command that changes a device never reaches
    /// one.
    pub fn classify(&self, profile_id: &str, command: &str) -> Option<Classification> {
        let platform = profiles::platform(&self.app, profile_id).ok()?;
        Some(platform.policy.classify(command))
    }

    /// Record what the assistant did, with the chat it did it in.
    pub fn audit(&self, action: &str, device_id: Option<&str>, message: String, detail: Value) {
        let mut detail = detail;
        if let Some(map) = detail.as_object_mut() {
            map.insert("chat_id".into(), json!(self.chat_id));
            map.insert("tool".into(), json!(self.tool.name));
        }
        self.app.audit(action, device_id, message, detail);
    }

    /// Wait for a job that a tool started, without blocking cancellation.
    pub async fn wait_for_job(&self, job_id: &str, what: &str) -> AppResult<Value> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
        loop {
            let job = self.rpc(boundary::JOBS_GET, json!({"id": job_id})).await?;
            match job["state"].as_str() {
                Some("running") | None => {}
                Some("cancelled") => {
                    return Err(AppError::new(
                        "cancelled",
                        format!("{what} was cancelled before it finished."),
                    ));
                }
                Some("failed") => {
                    let error = job["error"].as_str().unwrap_or("no reason given");
                    return Err(AppError::unavailable(format!("{what} failed: {error}")));
                }
                Some(_) => return Ok(job),
            }
            if std::time::Instant::now() > deadline {
                return Err(AppError::timeout(format!(
                    "{what} is still running after 10 minutes; it carries on in the background — check the Jobs list."
                )));
            }
            tokio::select! {
                () = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
                () = self.cancel.cancelled() => {
                    return Err(AppError::new("cancelled", format!("The user stopped this turn; {what} carries on in the background.")));
                }
            }
        }
    }
}

/// Parse a handler's arguments. The agent has already validated them against
/// the schema, so anything that fails here is a schema that does not match its
/// handler.
pub fn params<T: for<'de> Deserialize<'de>>(input: Value) -> AppResult<T> {
    serde_json::from_value(input)
        .map_err(|err| AppError::invalid(format!("those arguments do not fit this tool: {err}")))
}

/// `{"type": "object", …}` with `additionalProperties: false`, which keeps a
/// model from inventing arguments that are silently ignored.
pub fn schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

/// The `device` argument every device-facing tool takes.
pub const DEVICE_ARG: &str =
    "Device name, address or id, as `list_devices` returned it. A name is enough.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reachable_method_is_on_the_allow_list_and_the_list_is_all_of_them() {
        let reachable = reachable_methods();
        let mut allowed: Vec<Method> = boundary::ALLOWED.to_vec();
        allowed.sort_unstable();
        assert_eq!(
            reachable, allowed,
            "the allow-list and what the tools can reach must be the same set"
        );
    }

    #[test]
    fn tool_names_and_schemas_are_well_formed() {
        let mut names: Vec<&str> = all().iter().map(|tool| tool.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "tool names are unique");
        for tool in all() {
            let schema = (tool.schema)();
            assert_eq!(schema["type"], "object", "{}", tool.name);
            assert_eq!(schema["additionalProperties"], false, "{}", tool.name);
            assert!(
                schema["properties"].is_object(),
                "{} has no properties",
                tool.name
            );
            for required in schema["required"].as_array().unwrap_or(&Vec::new()) {
                let name = required.as_str().unwrap_or_default();
                assert!(
                    schema["properties"].get(name).is_some(),
                    "{} requires `{name}`, which it does not define",
                    tool.name
                );
            }
            assert!(
                !tool.description.is_empty() && !tool.brief.is_empty(),
                "{} needs a description",
                tool.name
            );
            assert!(
                !tool.required.is_empty(),
                "{} declares no methods",
                tool.name
            );
        }
    }

    #[test]
    fn the_core_set_is_the_one_a_small_model_gets() {
        let mut core: Vec<&str> = all()
            .iter()
            .filter(|tool| tool.core)
            .map(|tool| tool.name)
            .collect();
        core.sort_unstable();
        assert_eq!(
            core,
            vec![
                "get_change",
                "get_config",
                "get_device",
                "list_alerts",
                "list_devices",
                "ping",
                "propose_change",
                "run_commands",
                "search_configs",
                "subnet_calc",
            ]
        );
    }
}
