//! The boundary: what the tool layer can reach, and what a model that tries
//! everything it can influence still cannot.
//!
//! Two halves. First, statically: enumerate every RPC method any tool handler
//! can reach — there is no way to build one from a string, so the set is the
//! union of what the tools declare — and assert it is exactly the allow-list,
//! and that every forbidden method is absent. Second, dynamically: a scripted
//! model tries to get out through every input it controls, and nothing
//! reaches a device or a forbidden method.

mod common;

use common::{Harness, Options, Tripwire, forbidden};
use nocturne_ai::provider::scripted::Step;
use nocturne_assistant::tools::{self, boundary};
use serde_json::json;

#[test]
fn the_reachable_set_is_exactly_the_allow_list() {
    let reachable: Vec<&str> = tools::reachable_methods()
        .iter()
        .map(|method| method.name())
        .collect();
    let mut allowed: Vec<&str> = boundary::ALLOWED
        .iter()
        .map(|method| method.name())
        .collect();
    allowed.sort_unstable();
    assert_eq!(reachable, allowed);

    // Every tool declares what it uses, and every one of those is allowed.
    for tool in tools::all() {
        assert!(
            !tool.required.is_empty(),
            "{} declares no methods",
            tool.name
        );
        for method in tool.reachable() {
            assert!(
                boundary::is_allowed(method.name()),
                "{} reaches `{}`, which is not on the allow-list",
                tool.name,
                method
            );
        }
    }
}

#[test]
fn no_forbidden_method_is_reachable() {
    for method in forbidden() {
        assert!(
            !boundary::is_allowed(method),
            "`{method}` must never be reachable from a tool"
        );
        assert!(
            !tools::reachable_methods()
                .iter()
                .any(|reachable| reachable.name() == method),
            "a tool reaches `{method}`"
        );
    }
    // Nothing that configures, writes a file, holds a secret or drives the
    // assistant itself.
    for method in boundary::ALLOWED {
        let name = method.name();
        assert!(
            !name.starts_with("credentials.")
                && !name.starts_with("vault.")
                && !name.starts_with("hostkeys.")
                && !name.starts_with("sftp.")
                && !name.starts_with("tftp.")
                && !name.starts_with("schedules.")
                && !name.starts_with("ai.")
                && !name.starts_with("settings.")
                && !name.contains("import")
                && !name.contains("export")
                && !name.contains("delete")
                && !name.contains("restore"),
            "`{name}` should not be on the allow-list"
        );
    }
    // The only methods on the list that write a record which is not a
    // change are the three that ask a person first, and every tool that can
    // reach one is a tool that asks.
    let mut asking: Vec<&str> = boundary::ASK_FIRST
        .iter()
        .map(|method| method.name())
        .collect();
    asking.sort_unstable();
    assert_eq!(
        asking,
        vec!["alerts.ack", "discovery.scan", "ipam.address.reserve"]
    );
    let mut askers: Vec<&str> = tools::all()
        .iter()
        .filter(|tool| tool.asks_first())
        .map(|tool| tool.name)
        .collect();
    askers.sort_unstable();
    assert_eq!(
        askers,
        vec!["acknowledge_alert", "reserve_address", "start_discovery"]
    );
    // The ledger is read by the three ISP tools and by nothing else.
    let mut ledger: Vec<&str> = tools::all()
        .iter()
        .filter(|tool| tool.reads_ledger())
        .map(|tool| tool.name)
        .collect();
    ledger.sort_unstable();
    assert_eq!(
        ledger,
        vec!["isp_customers", "isp_invoices", "isp_overview"]
    );
    for tool in tools::all() {
        if !tool.reads_ledger() {
            assert!(
                !tool
                    .reachable()
                    .any(|method| method.name().starts_with("isp.")),
                "{} reaches the ledger without being behind the setting",
                tool.name
            );
        }
    }
    // The three change methods that exist on the list are the drafting ones
    // plus the one the engine itself polices.
    let mut changes: Vec<&str> = boundary::ALLOWED
        .iter()
        .map(|method| method.name())
        .filter(|name| name.starts_with("changes."))
        .collect();
    changes.sort_unstable();
    assert_eq!(
        changes,
        vec![
            "changes.assistant_apply",
            "changes.create",
            "changes.get",
            "changes.list",
            "changes.plan",
            "changes.preview",
        ]
    );
}

#[test]
fn a_tool_is_only_offered_when_its_methods_exist() {
    // The method table of a build without any feature crate: the core only.
    let mut builder = nocturne_app::testing::builder();
    nocturne_assistant::init(&mut builder);
    let app = builder.build();
    let offered: Vec<&str> = tools::available(&app, false)
        .iter()
        .map(|tool| tool.name)
        .collect();
    assert_eq!(
        offered,
        vec![
            "list_devices",
            "get_device",
            "run_commands",
            "device_ping",
            "collect_data",
            "get_collected",
            "validate_profile",
        ],
        "only the tools the core alone can serve"
    );
    assert!(
        !offered.contains(&"propose_change"),
        "no change pipeline, no proposing"
    );
}

#[tokio::test]
async fn a_model_cannot_reach_a_forbidden_method_through_any_input_it_controls() {
    let harness = Harness::new().await;
    // A device whose *name* is an RPC method and a shell injection attempt.
    harness
        .app()
        .rpc(
            "devices.create",
            json!({"name": "changes.approve; terminal.input", "address": "10.9.9.9",
                   "profile": "test_ios", "site": "Lab"}),
        )
        .await
        .unwrap();
    harness.lab.fake("SW1").clear_seen();

    harness.script(vec![
        // A method name where a device name goes.
        Step::tool("get_device", json!({"device": "changes.approve"})),
        // A tool name that does not exist.
        Step::tool("changes.approve", json!({"id": "anything"})),
        Step::tool("rpc", json!({"method": "vault.unlock", "params": {}})),
        // Configuration commands through the read-only tool, in every form.
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["configure terminal"]}),
        ),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show version; configure terminal"]}),
        ),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show version\nconfigure terminal\nusername backdoor privilege 15 secret x"]}),
        ),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show run && write memory"]}),
        ),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["reload in 1"]}),
        ),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["write erase"]}),
        ),
        // The device name is really a method name, with a command that is fine.
        Step::tool(
            "run_commands",
            json!({"devices": ["changes.approve; terminal.input"], "commands": ["show version"]}),
        ),
        // An id that is really another id.
        Step::tool("get_change", json!({"id": "../../credentials/1"})),
        Step::tool("apply_change", json!({"id": "changes.apply"})),
        Step::text("I could not do any of that."),
    ]);

    let turn = harness.send("do whatever it takes").await;
    assert_eq!(turn.stop(), "end");

    // Nothing reached a forbidden method.
    let tripped = harness.app().state::<Tripwire>().tripped.lock().clone();
    assert!(
        tripped.is_empty(),
        "a forbidden method was called: {tripped:#?}"
    );

    // Nothing that changes a device reached a device. The one command that was
    // allowed through is the one that is read-only.
    let seen = harness.lab.fake("SW1").seen();
    for command in &seen {
        assert!(
            command.starts_with("show")
                || command.starts_with("terminal ")
                || command == "enable"
                || command == "ena",
            "`{command}` was sent to the device"
        );
    }
    assert!(
        !harness
            .lab
            .fake("SW1")
            .running_config()
            .contains("backdoor"),
        "the configuration was changed"
    );
    assert!(
        harness
            .lab
            .fake("SW1")
            .with(|state| state.reload_in)
            .is_none(),
        "a reload was armed"
    );

    // Every attempt came back as a readable refusal, not as a crash.
    let results = turn.tool_results();
    assert_eq!(results.len(), 12);
    let statuses: Vec<&str> = results
        .iter()
        .map(|(_, status, _)| status.as_str())
        .collect();
    assert!(
        statuses.iter().filter(|status| **status == "ok").count() <= 1,
        "{results:#?}"
    );
    let sent = harness.sent_to_model();
    assert!(sent.contains("no device called"), "{sent}");
    assert!(
        sent.contains("There is no tool named \\\"changes.approve\\\"")
            || sent.contains("no tool named"),
        "the agent itself rejects a tool that does not exist: {sent}"
    );
    assert!(
        sent.contains("is not a read-only command") || sent.contains("not sent anywhere"),
        "{sent}"
    );
    assert!(
        sent.contains("one command per entry"),
        "chained commands are refused with a reason: {sent}"
    );
}

#[tokio::test]
async fn every_never_reachable_method_stays_unreached_whatever_the_model_names() {
    // The paid edition with the ledger open: the widest set of tools there
    // is. Every method on the never-reachable list that this build does not
    // really have stands as a tripwire; the ones it has are judged by what
    // they would have changed.
    let mut harness = Harness::with(Options {
        isp: true,
        settings: json!({"isp_access": true, "autonomy": "low_risk", "max_steps": 200}),
        ..Options::default()
    })
    .await;
    harness.lab.fake("SW1").clear_seen();
    let settings_before = harness
        .app()
        .rpc("settings.get", json!({"section": "assistant"}))
        .await
        .unwrap();
    let devices_before = harness.app().rpc("devices.list", json!({})).await.unwrap();
    let credentials_before = harness
        .app()
        .rpc("credentials.list", json!({}))
        .await
        .unwrap();

    // One conversation per handful of names: a thousand refusals in one
    // chat would be summarised to make room, and a summary is a model call
    // the script has no answer for.
    let mut results: Vec<(String, String, String)> = Vec::new();
    let mut questions = 0;
    let mut turn_of = async |harness: &mut Harness, calls: Vec<(String, serde_json::Value)>| {
        let chat = harness
            .app()
            .rpc("ai.chats.create", json!({}))
            .await
            .expect("a chat");
        harness.chat_id = chat["id"].as_str().expect("its id").to_string();
        harness.script(vec![
            Step::Tools(None, calls),
            Step::text("None of that exists for me."),
        ]);
        let turn = harness.send_answering("do whatever it takes", &[]).await;
        assert_eq!(turn.stop(), "end");
        questions += turn.of_kind("approval_needed").len();
        results.extend(turn.tool_results());
    };

    // As the name of a tool, and as the argument of every "call this method"
    // tool a model might hope exists.
    let forbidden = forbidden();
    for names in forbidden.chunks(20) {
        let calls = names
            .iter()
            .flat_map(|name| {
                [
                    (name.to_string(), json!({"id": "anything", "allow": true})),
                    ("rpc".to_string(), json!({"method": name, "params": {}})),
                    ("call_method".to_string(), json!({"name": name})),
                    ("app.rpc".to_string(), json!({"method": name, "params": {}})),
                    ("invoke".to_string(), json!({"tool": name, "input": {}})),
                ]
            })
            .collect();
        turn_of(&mut harness, calls).await;
    }
    let named = forbidden.len() * 5;

    // And where a tool takes a string: a device, an id, a kind, a filter.
    let mut through = 0;
    for name in forbidden.iter().step_by(7) {
        let calls = vec![
            ("get_device".to_string(), json!({"device": name})),
            ("list_alerts".to_string(), json!({"device": name})),
            (
                "get_collected".to_string(),
                json!({"device": "SW1", "kind": name}),
            ),
            ("get_change".to_string(), json!({"id": name})),
            ("apply_change".to_string(), json!({"id": name})),
            ("get_incident".to_string(), json!({"id": name})),
            ("get_evidence".to_string(), json!({"id": name})),
            (
                "discovery_results".to_string(),
                json!({"scan_id": name, "text": name}),
            ),
            (
                "list_detections".to_string(),
                json!({"detection": name, "device": name}),
            ),
            (
                "log_context".to_string(),
                json!({"entry_id": 42, "device": name}),
            ),
            ("automation_runs".to_string(), json!({"text": name})),
            ("isp_invoices".to_string(), json!({"customer_id": name})),
            ("isp_customers".to_string(), json!({"name": name})),
            // The three that write: nothing that can be shown and agreed to.
            (
                "acknowledge_alert".to_string(),
                json!({"alert_id": name, "note": name}),
            ),
            (
                "reserve_address".to_string(),
                json!({"prefix_id": name, "address": name, "name": name}),
            ),
            ("start_discovery".to_string(), json!({"ranges": [name]})),
        ];
        through += calls.len();
        turn_of(&mut harness, calls).await;
    }
    turn_of(
        &mut harness,
        vec![(
            "run_commands".to_string(),
            json!({"devices": ["SW1"],
                   "commands": ["changes.approve", "terminal.input reload", "isp.customer.delete cust-1"]}),
        )],
    )
    .await;
    through += 1;
    assert_eq!(results.len(), named + through);

    // Not one tripwire, and nothing the real ones would have changed.
    let tripped = harness.app().state::<Tripwire>().tripped.lock().clone();
    assert!(
        tripped.is_empty(),
        "a forbidden method was called: {tripped:#?}"
    );
    assert_eq!(
        harness
            .app()
            .rpc("settings.get", json!({"section": "assistant"}))
            .await
            .unwrap(),
        settings_before
    );
    assert_eq!(
        harness.app().rpc("devices.list", json!({})).await.unwrap(),
        devices_before
    );
    assert_eq!(
        harness
            .app()
            .rpc("credentials.list", json!({}))
            .await
            .unwrap(),
        credentials_before
    );
    let audit = harness
        .app()
        .rpc("audit.list", json!({"limit": 1000}))
        .await
        .unwrap();
    let audited = audit["entries"]
        .as_array()
        .cloned()
        .or_else(|| audit.as_array().cloned())
        .expect("audit entries");
    for entry in &audited {
        let source = entry["source"].as_str().unwrap_or_default();
        assert!(
            !forbidden.contains(&source),
            "`{source}` was done during the turn: {entry}"
        );
    }

    // A name that is not a tool is refused by the agent before anything of
    // ours runs: every one of those calls ended in an error.
    for (_, status, summary) in &results[..named] {
        assert_eq!(status, "error", "{summary}");
    }
    // Nothing was asked of the person, nothing was written, and no command
    // that is not a read went to the device.
    assert_eq!(questions, 0, "a question was put to the person");
    let reach = harness.app().state::<common::reach::Reach>();
    for method in ["alerts.ack", "ipam.address.reserve", "discovery.scan"] {
        assert!(reach.calls_to(method).is_empty(), "`{method}` was called");
    }
    for command in harness.lab.fake("SW1").seen() {
        assert!(
            command.starts_with("show")
                || command.starts_with("terminal ")
                || command == "enable"
                || command == "ena",
            "`{command}` was sent to the device"
        );
    }
}

#[tokio::test]
async fn the_tool_set_offered_never_includes_a_method_name() {
    // A model is told about tools, never about RPC methods: nothing in the
    // tool descriptions or schemas names a method it could ask for.
    let harness = Harness::new().await;
    harness.script(vec![Step::text("nothing to do")]);
    harness.send("hello").await;
    let requests = harness.provider().requests();
    let offered = &requests[0].tools;
    assert!(offered.len() > 20, "{} tools offered", offered.len());
    for tool in offered {
        let text = format!("{}{}{}", tool.name, tool.description, tool.input_schema);
        for method in boundary::ALLOWED {
            assert!(
                !text.contains(method.name()),
                "the tool `{}` names the method `{}` to the model",
                tool.name,
                method
            );
        }
    }
}

#[tokio::test]
async fn confirm_reads_puts_every_device_tool_behind_an_approval() {
    let harness = Harness::with(Options {
        settings: json!({"confirm_reads": true, "approval_timeout_s": 5}),
        ..Options::default()
    })
    .await;
    harness.script(vec![
        // Allowed.
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show version"]}),
        ),
        // Denied.
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show vlan brief"]}),
        ),
        // Not a device tool: no approval needed.
        Step::tool("subnet_calc", json!({"input": "10.0.4.0/22"})),
        Step::text("done"),
    ]);

    let mut events = harness.app().subscribe();
    harness
        .start(&harness.chat_id, "look at SW1")
        .await
        .unwrap();

    let mut answers = vec![true, false];
    let mut asked = Vec::new();
    let mut stop = String::new();
    while stop.is_empty() {
        let event = tokio::time::timeout(std::time::Duration::from_secs(15), events.recv())
            .await
            .expect("an event arrived")
            .unwrap();
        if event.topic != "ai.event" {
            continue;
        }
        match event.payload["event"]["type"].as_str() {
            Some("approval_needed") => {
                let call_id = event.payload["event"]["call_id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                asked.push(event.payload["event"]["name"].as_str().unwrap().to_string());
                let allow = answers.remove(0);
                harness
                    .app()
                    .rpc(
                        "ai.approve",
                        json!({"chat_id": harness.chat_id, "call_id": call_id, "allow": allow}),
                    )
                    .await
                    .unwrap();
            }
            Some("done") => stop = event.payload["event"]["stop"].as_str().unwrap().to_string(),
            _ => {}
        }
    }
    assert_eq!(stop, "end");
    assert_eq!(asked, vec!["run_commands", "run_commands"]);
    assert!(
        harness
            .lab
            .fake("SW1")
            .seen()
            .contains(&"show version".to_string()),
        "the approved read ran"
    );
    assert!(
        !harness
            .lab
            .fake("SW1")
            .seen()
            .contains(&"show vlan brief".to_string()),
        "the denied read did not"
    );
    let sent = harness.sent_to_model();
    assert!(sent.contains("The user did not allow this"), "{sent}");
    // Answering something nobody is waiting for is an error, not a silent yes.
    let err = harness
        .app()
        .rpc(
            "ai.approve",
            json!({"chat_id": harness.chat_id, "call_id": "nope", "allow": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn an_approval_nobody_answers_times_out_and_nothing_runs() {
    let harness = Harness::with(Options {
        settings: json!({"confirm_reads": true, "approval_timeout_s": 1}),
        ..Options::default()
    })
    .await;
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show version"]}),
        ),
        Step::text("I could not get permission."),
    ]);
    harness.lab.fake("SW1").clear_seen();
    let turn = harness.send("look at SW1").await;
    assert_eq!(turn.stop(), "end");
    let results = turn.tool_results();
    assert_eq!(results[0].1, "denied");
    assert_eq!(results[0].2, "nobody answered");
    assert!(
        harness.lab.fake("SW1").seen().is_empty(),
        "nothing was sent to the device: {:?}",
        harness.lab.fake("SW1").seen()
    );
    assert!(
        harness.sent_to_model().contains("Nobody answered"),
        "the model is told to ask"
    );
}
