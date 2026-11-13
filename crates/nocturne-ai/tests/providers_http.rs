//! The providers over real HTTP, against a scripted local server: request
//! shape, streaming, retries, the prompted-tools fallback and cancellation.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nocturne_ai::{
    AiError, Block, CancellationToken, ChatRequest, Message, ProviderConfig, ProviderKind,
    StopReason, StreamEvent, ToolMode, ToolSpec, build_provider,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;

struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
    /// Keep the connection open after the body instead of closing it.
    stall: bool,
}

impl Reply {
    fn sse(events: &[Value]) -> Reply {
        let body = events
            .iter()
            .map(|event| {
                format!(
                    "event: {}\ndata: {event}\n\n",
                    event["type"].as_str().unwrap_or("message")
                )
            })
            .collect();
        Reply {
            status: 200,
            content_type: "text/event-stream",
            body,
            stall: false,
        }
    }

    fn sse_data(chunks: &[Value]) -> Reply {
        let mut body: String = chunks
            .iter()
            .map(|chunk| format!("data: {chunk}\n\n"))
            .collect();
        body.push_str("data: [DONE]\n\n");
        Reply {
            status: 200,
            content_type: "text/event-stream",
            body,
            stall: false,
        }
    }

    fn ndjson(lines: &[Value]) -> Reply {
        let body = lines.iter().map(|line| format!("{line}\n")).collect();
        Reply {
            status: 200,
            content_type: "application/x-ndjson",
            body,
            stall: false,
        }
    }

    fn json(status: u16, body: Value) -> Reply {
        Reply {
            status,
            content_type: "application/json",
            body: body.to_string(),
            stall: false,
        }
    }
}

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

struct Server {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    async fn start(replies: Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let queue = queue.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    log.lock().unwrap().push(request);
                    let reply = queue.lock().unwrap().pop_front();
                    let Some(reply) = reply else {
                        let _ = socket.write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                        return;
                    };
                    let head = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: {}\r\nconnection: close\r\n{}\r\n",
                        reply.status,
                        reply.content_type,
                        if reply.stall {
                            String::new()
                        } else {
                            format!("content-length: {}\r\n", reply.body.len())
                        },
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    // Two writes, split mid-body, to exercise chunk reassembly.
                    let (first, second) = reply.body.split_at(reply.body.len() / 2);
                    let _ = socket.write_all(first.as_bytes()).await;
                    let _ = socket.flush().await;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let _ = socket.write_all(second.as_bytes()).await;
                    let _ = socket.flush().await;
                    if reply.stall {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                });
            }
        });
        Server { url, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let path = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&buffer[header_end..]).unwrap_or(Value::Null);
    Some(Seen {
        method,
        path,
        headers,
        body,
    })
}

fn tools() -> Vec<ToolSpec> {
    vec![ToolSpec {
        name: "run_command".into(),
        description: "Run a read-only command on a device".into(),
        input_schema: json!({"type": "object", "properties": {"device": {"type": "string"}, "command": {"type": "string"}}, "required": ["device", "command"]}),
        parallel_safe: true,
    }]
}

fn config(kind: ProviderKind, url: &str, model: &str) -> ProviderConfig {
    ProviderConfig {
        kind,
        base_url: url.to_string(),
        model: model.to_string(),
        ..ProviderConfig::default()
    }
}

async fn chat(
    config: &ProviderConfig,
    key: Option<&str>,
    messages: &[Message],
) -> (Result<nocturne_ai::ChatResponse, AiError>, Vec<StreamEvent>) {
    let provider = build_provider(config, key.map(str::to_string)).unwrap();
    let (tx, mut rx) = unbounded_channel();
    let tools = tools();
    let request = ChatRequest {
        system: "You are a network assistant.",
        messages,
        tools: &tools,
        allow_tools: true,
    };
    let result = provider.chat(request, &tx, &CancellationToken::new()).await;
    drop(tx);
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (result, events)
}

fn anthropic_tool_turn() -> Vec<Value> {
    vec![
        json!({"type": "message_start", "message": {"id": "msg_01", "model": "claude-opus-5-5", "usage": {"input_tokens": 812, "cache_read_input_tokens": 4096, "output_tokens": 2}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "The user wants the VLAN list."}}),
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EuYBCkYIBxgC"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "I'll check "}}),
        json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "the switch — one moment."}}),
        json!({"type": "content_block_stop", "index": 1}),
        json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_01A", "name": "run_command", "input": {}}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"device\": \"sw1\", \"comm"}}),
        json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "and\": \"show vlan brief\"}"}}),
        json!({"type": "content_block_stop", "index": 2}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 88}}),
        json!({"type": "message_stop"}),
    ]
}

#[tokio::test]
async fn anthropic_streams_a_tool_call_and_sends_the_documented_request() {
    let server = Server::start(vec![Reply::sse(&anthropic_tool_turn())]).await;
    let config = config(ProviderKind::Anthropic, &server.url, "claude-opus-5-5");
    let (result, events) = chat(
        &config,
        Some("sk-ant-test"),
        &[Message::user("Which VLANs does sw1 have?")],
    )
    .await;
    let response = result.unwrap();

    assert_eq!(response.stop, StopReason::ToolUse);
    assert_eq!(
        response.message.text(),
        "I'll check the switch — one moment."
    );
    let (id, name, input) = response.message.tool_uses().next().unwrap();
    assert_eq!((id, name), ("toolu_01A", "run_command"));
    assert_eq!(
        input,
        &json!({"device": "sw1", "command": "show vlan brief"})
    );
    assert_eq!(response.usage.cache_read_tokens, 4096);
    assert!(events.contains(&StreamEvent::Text {
        text: "I'll check ".into()
    }));
    assert!(events.contains(&StreamEvent::Thinking {
        text: "The user wants the VLAN list.".into()
    }));

    let seen = &server.seen()[0];
    assert_eq!(
        (seen.method.as_str(), seen.path.as_str()),
        ("POST", "/v1/messages")
    );
    assert_eq!(seen.header("x-api-key"), Some("sk-ant-test"));
    assert_eq!(seen.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(seen.body["model"], "claude-opus-5-5");
    assert_eq!(seen.body["thinking"]["type"], "adaptive");
    assert_eq!(seen.body["output_config"]["effort"], "high");
    assert_eq!(seen.body["tools"][0]["name"], "run_command");
    assert_eq!(seen.body["messages"][0]["role"], "user");
    assert!(seen.body.get("temperature").is_none());

    // The next request must replay the assistant turn byte-for-byte, signature included.
    let mut history = vec![
        Message::user("Which VLANs does sw1 have?"),
        response.message,
    ];
    history.push(Message {
        role: nocturne_ai::Role::User,
        content: vec![Block::ToolResult {
            tool_use_id: "toolu_01A".into(),
            content: "10 Staff\n20 Guest".into(),
            is_error: false,
        }],
        raw: None,
    });
    let follow_up = Server::start(vec![Reply::sse(&[
        json!({"type": "message_start", "message": {"model": "claude-opus-5-5", "usage": {"input_tokens": 5}}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "VLANs 10 and 20."}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 9}}),
    ])])
    .await;
    let config = self::config(ProviderKind::Anthropic, &follow_up.url, "claude-opus-5-5");
    let (result, _) = chat(&config, Some("k"), &history).await;
    assert_eq!(result.unwrap().message.text(), "VLANs 10 and 20.");
    let sent = &follow_up.seen()[0].body["messages"];
    assert_eq!(
        sent[1]["content"][0],
        json!({"type": "thinking", "thinking": "The user wants the VLAN list.", "signature": "EuYBCkYIBxgC"})
    );
    assert_eq!(sent[1]["content"][2]["id"], "toolu_01A");
    assert_eq!(sent[2]["content"][0]["type"], "tool_result");
}

#[tokio::test]
async fn anthropic_retries_overload_then_succeeds_and_reports_auth_errors() {
    let server = Server::start(vec![
        Reply::json(529, json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}})),
        Reply::sse(&[
            json!({"type": "message_start", "message": {"model": "claude-opus-5-5", "usage": {"input_tokens": 3}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": "ok"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
        ]),
    ])
    .await;
    let config = config(ProviderKind::Anthropic, &server.url, "claude-opus-5-5");
    let (result, _) = chat(&config, Some("k"), &[Message::user("hi")]).await;
    assert_eq!(result.unwrap().message.text(), "ok");
    assert_eq!(server.seen().len(), 2);

    let denied = Server::start(vec![Reply::json(
        401,
        json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}, "request_id": "req_9"}),
    )])
    .await;
    let config = self::config(ProviderKind::Anthropic, &denied.url, "claude-opus-5-5");
    let (result, _) = chat(&config, Some("bad"), &[Message::user("hi")]).await;
    let error = result.unwrap_err();
    assert!(
        matches!(&error, AiError::Api { status: 401, request_id: Some(id), .. } if id == "req_9")
    );
    assert!(error.to_string().contains("API key was rejected"));
    assert_eq!(
        denied.seen().len(),
        1,
        "authentication failures must not be retried"
    );
}

#[tokio::test]
async fn anthropic_lists_models() {
    let server = Server::start(vec![Reply::json(
        200,
        json!({"data": [{"id": "claude-opus-5-5", "display_name": "Claude Opus 5.5", "max_input_tokens": 1000000, "max_tokens": 128000}], "has_more": false}),
    )])
    .await;
    let provider = build_provider(
        &config(ProviderKind::Anthropic, &server.url, "claude-opus-5-5"),
        Some("k".into()),
    )
    .unwrap();
    let models = provider.list_models().await.unwrap();
    assert_eq!(models[0].id, "claude-opus-5-5");
    assert_eq!(models[0].context_window, Some(1_000_000));
    assert!(server.seen()[0].path.starts_with("/v1/models"));
}

#[tokio::test]
async fn openai_compatible_streams_tool_calls_with_bearer_auth() {
    let server = Server::start(vec![Reply::sse_data(&[
        json!({"id": "c1", "model": "gpt-test", "choices": [{"index": 0, "delta": {"role": "assistant", "content": ""}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call_abc", "type": "function", "function": {"name": "run_command", "arguments": "{\"device\":\"r1\","}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"command\":\"show ip route\"}"}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
        json!({"choices": [], "usage": {"prompt_tokens": 400, "completion_tokens": 30}}),
    ])])
    .await;
    let config = config(
        ProviderKind::Openai,
        &format!("{}/v1", server.url),
        "gpt-test",
    );
    let (result, _) = chat(&config, Some("sk-test"), &[Message::user("routes on r1?")]).await;
    let response = result.unwrap();
    assert_eq!(response.stop, StopReason::ToolUse);
    let (id, _, input) = response.message.tool_uses().next().unwrap();
    assert_eq!(
        (id, input),
        (
            "call_abc",
            &json!({"device": "r1", "command": "show ip route"})
        )
    );
    assert_eq!(response.usage.input_tokens, 400);

    let seen = &server.seen()[0];
    assert_eq!(seen.path, "/v1/chat/completions");
    assert_eq!(seen.header("authorization"), Some("Bearer sk-test"));
    assert_eq!(seen.body["messages"][0]["role"], "system");
    assert_eq!(seen.body["tools"][0]["type"], "function");
}

#[tokio::test]
async fn falls_back_to_prompted_tools_when_the_model_has_no_tool_support() {
    let server = Server::start(vec![
        Reply::json(400, json!({"error": {"message": "registry.ollama.ai/library/tinymodel does not support tools", "type": "invalid_request_error"}})),
        Reply::sse_data(&[
            json!({"choices": [{"delta": {"content": "Let me look.\n```tool\n{\"name\": \"run_command\", \"arguments\": {\"device\": \"sw1\", \"command\": \"show clock\"}}\n```"}, "finish_reason": "stop"}]}),
        ]),
        Reply::sse_data(&[json!({"choices": [{"delta": {"content": "It is noon."}, "finish_reason": "stop"}]})]),
    ])
    .await;
    let config = ProviderConfig {
        tool_mode: ToolMode::Auto,
        ..config(ProviderKind::Openai, &server.url, "tinymodel")
    };
    let provider = build_provider(&config, None).unwrap();
    let tools = tools();
    let (tx, _rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    let mut history = vec![Message::user("time on sw1?")];
    let request = ChatRequest {
        system: "sys",
        messages: &history,
        tools: &tools,
        allow_tools: true,
    };
    let first = provider.chat(request, &tx, &cancel).await.unwrap();
    assert_eq!(first.stop, StopReason::ToolUse);
    assert_eq!(first.message.text(), "Let me look.");
    let (id, name, _) = first.message.tool_uses().next().unwrap();
    assert_eq!(name, "run_command");
    let id = id.to_string();

    let seen = server.seen();
    assert!(seen[0].body.get("tools").is_some());
    assert!(
        seen[1].body.get("tools").is_none(),
        "the retry must not send native tools"
    );
    assert!(
        seen[1].body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains("## run_command")
    );

    // The provider stays in prompted mode and shows the result as text.
    history.push(first.message);
    history.push(Message {
        role: nocturne_ai::Role::User,
        content: vec![Block::ToolResult {
            tool_use_id: id,
            content: "12:00:00 UTC".into(),
            is_error: false,
        }],
        raw: None,
    });
    let request = ChatRequest {
        system: "sys",
        messages: &history,
        tools: &tools,
        allow_tools: true,
    };
    let second = provider.chat(request, &tx, &cancel).await.unwrap();
    assert_eq!(second.message.text(), "It is noon.");
    let third_request = &server.seen()[2].body["messages"];
    assert!(
        third_request[2]["content"]
            .as_str()
            .unwrap()
            .contains("```tool")
    );
    assert!(
        third_request[3]["content"]
            .as_str()
            .unwrap()
            .contains("[Tool result from run_command]\n12:00:00 UTC")
    );
}

#[tokio::test]
async fn ollama_native_stream_and_model_listing() {
    let server = Server::start(vec![
        Reply::ndjson(&[
            json!({"model": "qwen3:8b", "message": {"role": "assistant", "content": "Checking "}, "done": false}),
            json!({"model": "qwen3:8b", "message": {"role": "assistant", "content": "now."}, "done": false}),
            json!({"model": "qwen3:8b", "message": {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "run_command", "arguments": {"device": "sw1", "command": "show version"}}}]}, "done": false}),
            json!({"model": "qwen3:8b", "message": {"role": "assistant", "content": ""}, "done": true, "done_reason": "stop", "prompt_eval_count": 950, "eval_count": 40}),
        ]),
        Reply::json(200, json!({"models": [{"name": "qwen3:8b", "details": {"parameter_size": "8.2B", "quantization_level": "Q4_K_M"}}]})),
    ])
    .await;
    let config = ProviderConfig {
        context_window: Some(32_768),
        ..config(ProviderKind::Ollama, &server.url, "qwen3:8b")
    };
    let (result, events) = chat(&config, None, &[Message::user("version of sw1?")]).await;
    let response = result.unwrap();
    assert_eq!(response.stop, StopReason::ToolUse);
    assert_eq!(response.message.text(), "Checking now.");
    assert_eq!(response.usage.input_tokens, 950);
    assert!(events.iter().any(
        |event| matches!(event, StreamEvent::ToolStart { name, .. } if name == "run_command")
    ));

    let seen = &server.seen()[0];
    assert_eq!(seen.path, "/api/chat");
    assert_eq!(seen.body["options"]["num_ctx"], 32_768);
    assert_eq!(seen.body["stream"], true);

    let provider = build_provider(&config, None).unwrap();
    let models = provider.list_models().await.unwrap();
    assert_eq!(models[0].name, "qwen3:8b (8.2B Q4_K_M)");
}

#[tokio::test]
async fn cancellation_interrupts_a_stalled_stream() {
    let stalled = Reply {
        status: 200,
        content_type: "text/event-stream",
        body: format!(
            "event: message_start\ndata: {}\n\n",
            json!({"type": "message_start", "message": {"model": "m", "usage": {}}})
        ),
        stall: true,
    };
    let server = Server::start(vec![stalled]).await;
    let provider = build_provider(
        &config(ProviderKind::Anthropic, &server.url, "claude-opus-5-5"),
        Some("k".into()),
    )
    .unwrap();
    let (tx, _rx) = unbounded_channel();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        trigger.cancel();
    });
    let messages = [Message::user("hi")];
    let request = ChatRequest {
        system: "s",
        messages: &messages,
        tools: &[],
        allow_tools: true,
    };
    let started = std::time::Instant::now();
    let result = provider.chat(request, &tx, &cancel).await;
    assert!(matches!(result, Err(AiError::Cancelled)));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn unreachable_endpoint_is_a_network_error() {
    // Bind then drop to get a port nothing is listening on.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let (result, _) = chat(
        &config(ProviderKind::Openai, &url, "m"),
        None,
        &[Message::user("hi")],
    )
    .await;
    assert!(matches!(result, Err(AiError::Network(_))), "{result:?}");
}
