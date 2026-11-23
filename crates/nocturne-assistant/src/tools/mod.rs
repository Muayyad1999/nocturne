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
//!   handler can leak a secret;
//! * lets a method that writes a record through only after the person said
//!   yes to this call's own question ([`ToolCtx::confirm`]), and a method of
//!   the ISP ledger only while the `isp_access` setting is on.

pub mod boundary;
mod changes;
mod data;
mod design;
mod devices;
mod ledger;
mod net;
mod ops;
mod records;
mod signals;
mod watch;
mod work;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::future::BoxFuture;
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::{App, profiles};
use nocturne_drivers::policy::Classification;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::redact::ChatRedactor;
use crate::settings::{Caps, Settings};
use crate::view::Link;
use boundary::Method;

/// What the person said when a tool asked before writing a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    /// The question went unanswered for as long as the settings allow.
    Nobody,
}

/// How a tool puts a yes-or-no question to the person: the
/// `approval_needed` event, answered with `ai.approve`. The turn runner
/// implements it; a handler only ever sees [`ToolCtx::confirm`].
pub trait Asker: Send + Sync {
    /// `Err` means the turn was stopped while the question was open.
    fn ask<'a>(
        &'a self,
        call_id: &'a str,
        tool: &'static Tool,
        detail: String,
    ) -> BoxFuture<'a, AppResult<Answer>>;
}

/// What a handler returns: the text the model reads, a line for the UI, and
/// optionally the thing the result is about.
pub struct ToolOk {
    pub content: String,
    pub summary: String,
    pub link: Option<Link>,
    /// The person was asked and the tool did nothing: a result, not a
    /// failure, and the UI shows it as declined.
    pub declined: bool,
}

impl ToolOk {
    pub fn new(content: impl Into<String>, summary: impl Into<String>) -> ToolOk {
        ToolOk {
            content: content.into(),
            summary: summary.into(),
            link: None,
            declined: false,
        }
    }

    /// The person did not say yes, so `what` was not done. Their no is an
    /// answer the model reads and relays, not an error to work around.
    pub fn declined(answer: Answer, what: &str) -> ToolOk {
        let (content, summary) = match answer {
            Answer::Nobody => (
                format!(
                    "Nobody answered the question within the time allowed, so nothing was \
                     written: {what}. Tell the user what you wanted to record and why; they can \
                     ask you to try again."
                ),
                "nobody answered",
            ),
            _ => (
                format!(
                    "The user was asked and said no, so nothing was written: {what}. That is \
                     their decision — do not ask again in another form. Carry on with what they \
                     asked for, without it."
                ),
                "declined by the user",
            ),
        };
        ToolOk {
            content,
            summary: summary.to_string(),
            link: None,
            declined: true,
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

    /// Writes a record that is not a device's configuration, so it asks the
    /// person first, every time, whatever the settings say.
    pub fn asks_first(&self) -> bool {
        self.reachable().any(boundary::asks_first)
    }

    /// Reads the ISP ledger: offered only while `isp_access` is on.
    pub fn reads_ledger(&self) -> bool {
        self.reachable().any(boundary::is_ledger)
    }

    pub fn description_for(&self, brief: bool) -> &'static str {
        if brief { self.brief } else { self.description }
    }

    pub fn spec(&self, brief: bool) -> nocturne_ai::ToolSpec {
        nocturne_ai::ToolSpec {
            name: self.name.to_string(),
            description: self.description_for(brief).to_string(),
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
        signals::LOG_BURSTS,
        signals::LOG_CONTEXT,
        signals::LIST_DETECTIONS,
        signals::SERVICE_CHECKS,
        ops::LIST_INCIDENTS,
        ops::GET_INCIDENT,
        ops::HANDOVER_SUMMARY,
        ops::LIST_EVIDENCE,
        ops::GET_EVIDENCE,
        work::DISCOVERY_RESULTS,
        work::AUTOMATION_RUNS,
        records::START_DISCOVERY,
        records::ACKNOWLEDGE_ALERT,
        records::RESERVE_ADDRESS,
        ledger::ISP_OVERVIEW,
        ledger::ISP_CUSTOMERS,
        ledger::ISP_INVOICES,
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
/// set, for a model with little room. The ISP ledger's tools need the
/// `isp_access` setting as well as the feature: having the edition is not
/// the same as agreeing to show a model the customers.
pub fn available(app: &App, small: bool) -> Vec<&'static Tool> {
    let registered = app.methods();
    let ledger = Settings::read(app).isp_access;
    all()
        .iter()
        .filter(|tool| !small || tool.core)
        .filter(|tool| ledger || !tool.reads_ledger())
        .filter(|tool| {
            tool.required
                .iter()
                .all(|method| registered.contains(&method.name()))
        })
        .collect()
}

/// Whether a model with `context_window` tokens of room should be told about
/// `tools` in their short descriptions.
///
/// The long descriptions are worth their tokens to a model with a large
/// window. To one without, they are the conversation: the tool set is sent
/// with every request, and once it fills most of what the agent lets a
/// conversation grow to, every step ends in a summary of the step before. So
/// the short form is used as soon as the long one would take more than half
/// of that room — measured, not fixed, so that it still holds when tools are
/// added. A window that is not known (`0`) is not treated as a small one.
pub fn describe_briefly(tools: &[&'static Tool], context_window: u32) -> bool {
    if context_window == 0 {
        return false;
    }
    let specs: Vec<nocturne_ai::ToolSpec> = tools.iter().map(|tool| tool.spec(false)).collect();
    let described = nocturne_ai::agent::estimate_tokens(crate::prompt::SYSTEM, &[], &specs);
    described > compaction_limit(context_window) / 2
}

/// The size, in tokens, at which the agent summarises a conversation for a
/// model with this window. The agent's own figures, so the two cannot drift.
pub fn compaction_limit(context_window: u32) -> u64 {
    let agent = nocturne_ai::AgentConfig::default();
    agent
        .compact_at_tokens
        .min((f64::from(context_window) * agent.compact_at_fraction) as u64)
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
    asker: Arc<dyn Asker>,
    /// Set by a yes to [`ToolCtx::confirm`], spent by the write it allows.
    confirmed: AtomicBool,
}

impl ToolCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: App,
        tool: &'static Tool,
        chat_id: &str,
        call_id: &str,
        redactor: Arc<ChatRedactor>,
        caps: Caps,
        cancel: CancellationToken,
        asker: Arc<dyn Asker>,
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
            asker,
            confirmed: AtomicBool::new(false),
        }
    }

    /// Ask the person whether a record may be written, showing exactly what
    /// it is, and wait for their answer. A yes lets this call reach one
    /// method that writes; anything else leaves the gate shut.
    pub async fn confirm(&self, question: &str) -> AppResult<Answer> {
        let answer = self
            .asker
            .ask(&self.call_id, self.tool, question.to_string())
            .await?;
        if answer == Answer::Yes {
            self.confirmed.store(true, Ordering::Release);
        }
        Ok(answer)
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
        // Checked here and not only where tools are offered: the setting can
        // be switched off while a turn that was offered the tool is running.
        if boundary::is_ledger(method) && !Settings::read(&self.app).isp_access {
            return Err(AppError::forbidden(
                "The assistant's access to the customer ledger is switched off (`isp_access` in \
                 the assistant's settings), so nothing was read.",
            ));
        }
        // One yes, one write. A handler that reaches a writing method
        // without having asked is a bug in this crate; nothing is called.
        let writes = boundary::asks_first(method);
        if writes && !self.confirmed.swap(false, Ordering::AcqRel) {
            tracing::error!(tool = self.tool.name, %method, "tool tried to write a record without asking");
            return Err(AppError::internal(format!(
                "the tool `{}` tried to call `{method}` without the user's answer",
                self.tool.name
            )));
        }
        // Stopping the turn takes a yes back for as long as nothing has been
        // written: a person who says yes and then stops has stopped.
        if writes && self.cancel.is_cancelled() {
            return Err(AppError::new(
                "cancelled",
                "The user stopped this turn before anything was written, so nothing was.",
            ));
        }
        let app = self.app.clone();
        let name = method.name();
        // In its own task: cancelling a turn stops us waiting, it does not
        // abort a device read half-way through.
        let call = tokio::spawn(async move { app.rpc(name, params).await });
        let finished = |joined: Result<AppResult<Value>, tokio::task::JoinError>| match joined {
            Ok(result) => result,
            Err(err) => Err(AppError::internal(format!(
                "`{method}` did not finish: {err}"
            ))),
        };
        let outcome = if writes {
            // A write that has begun is waited for whatever happens to the
            // turn: it is a short local write, and walking away from it
            // would leave a record written and the assistant's own account
            // of having written it (the audit entry, the answer) missing.
            finished(call.await)
        } else {
            tokio::select! {
                biased;
                joined = call => finished(joined),
                () = self.cancel.cancelled() => {
                    return Err(AppError::new(
                        "cancelled",
                        format!("The user stopped this turn while `{method}` was running; anything already sent to a device finishes on its own."),
                    ));
                }
            }
        };
        match outcome {
            Ok(value) if boundary::already_redacted(method) => Ok(self
                .redactor
                .scrub_json(&crate::redact::without_foreign_placeholders(&value))),
            Ok(value) => Ok(self.redactor.scrub_json(&value)),
            // The ledger's own words about a failure are free text out of a
            // store of other people's details ("no invoice for <name>,
            // <phone>"), and nothing of the ledger reaches a model that is
            // not on the tools' allow-list of fields. The code says enough.
            Err(err) if boundary::is_ledger(method) => Err(ledger_failure(err.code)),
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

/// What a model is told when a read of the customer ledger fails: which kind
/// of failure it was, and none of the ledger's own wording.
fn ledger_failure(code: &'static str) -> AppError {
    let message = match code {
        "not_found" => {
            "the customer ledger has no record of what was asked for (an id it does not know). \
             Its own message is not passed on."
        }
        "invalid" => {
            "the customer ledger did not accept that filter. Its own message is not passed on."
        }
        "forbidden" => "the customer ledger refused the read. Its own message is not passed on.",
        _ => {
            "the customer ledger could not be read; this is a failure, not an empty answer. Its \
             own message is not passed on."
        }
    };
    AppError::new(code, message)
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

/// A part of an answer that is missing because its feature is not in this
/// build. "Unknown" and "none" are different answers, and the model has to
/// be able to tell which one it got.
pub fn not_in_build(what: &str) -> String {
    format!("{what}: unknown — that part of Nocturne is not in this build.")
}

/// A part of an answer that is missing because the read failed.
pub fn unread(what: &str, err: &AppError) -> String {
    format!(
        "{what}: unknown — it could not be read ({}).",
        crate::shape::one_line(&err.message, 200)
    )
}

/// The inventory's own id and name for a device the model named. The record
/// stores of the other features are keyed by id, so a name has to be looked
/// up before it can filter anything.
pub async fn device_ref(ctx: &ToolCtx, reference: &str) -> AppResult<(String, String)> {
    let row = ctx
        .rpc(boundary::DEVICES_GET, json!({"id": reference}))
        .await
        .map_err(|err| devices::unknown_device(reference, err))?;
    Ok((
        crate::shape::text_of(&row, "id"),
        crate::shape::text_of(&row, "name"),
    ))
}

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
    fn a_model_without_much_room_is_told_about_the_tools_in_fewer_words() {
        let every: Vec<&'static Tool> = all().iter().collect();
        // A local model whose window nobody configured is given 16,000
        // tokens by the engine; a mid-sized one has twice that.
        assert!(describe_briefly(&every, 16_000));
        assert!(describe_briefly(&every, 32_000));
        // The models the long descriptions were written for.
        assert!(!describe_briefly(&every, 128_000));
        assert!(!describe_briefly(&every, 200_000));
        assert!(!describe_briefly(&every, 1_000_000));
        // An unknown window is not a small one.
        assert!(!describe_briefly(&every, 0));

        // And in the short form the whole tool set still leaves that local
        // model room to hold a conversation. When this fails, a tool was
        // added that does not fit: shorten the short descriptions, or give
        // such a model fewer tools — do not raise the number.
        let brief: Vec<nocturne_ai::ToolSpec> = every.iter().map(|tool| tool.spec(true)).collect();
        let described = nocturne_ai::agent::estimate_tokens(crate::prompt::SYSTEM, &[], &brief);
        let limit = compaction_limit(16_000);
        assert_eq!(limit, 11_200);
        assert!(
            described < limit * 9 / 10,
            "the tool set takes {described} of the {limit} tokens a 16k model's conversation may reach"
        );
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
