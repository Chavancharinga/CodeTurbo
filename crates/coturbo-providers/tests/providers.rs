use coturbo_providers::{
    ContentBlock, Message, PriceTable, Request, Role, ToolSpec, Usage, anthropic, openai,
    sse::SseParser,
};
use serde_json::json;

/// Feeds a recorded SSE stream in small, uneven chunks (splitting multi-byte
/// UTF-8 chars) to make sure framing does not depend on network chunking.
fn feed(raw: &str, chunk: usize) -> Vec<(Option<String>, String)> {
    let mut p = SseParser::default();
    let mut out = Vec::new();
    for c in raw.as_bytes().chunks(chunk) {
        out.extend(p.push(c));
    }
    out
}

fn request() -> Request {
    Request {
        model: "claude-opus-5-5".into(),
        system: "You are CoTurbo.".into(),
        tools: vec![ToolSpec {
            name: "read_symbol".into(),
            description: "Read one symbol".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        }],
        messages: vec![
            Message { role: Role::User, content: vec![ContentBlock::Text { text: "olá".into() }] },
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Opaque(json!({"type":"thinking","thinking":"","signature":"s1"})),
                    ContentBlock::ToolUse {
                        id: "t1".into(),
                        name: "read_symbol".into(),
                        input: json!({"path":"a.rs"}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "fn a() {}".into(),
                    is_error: false,
                }],
            },
        ],
        max_tokens: 64000,
        effort: Some("high".into()),
    }
}

#[test]
fn sse_parser_handles_arbitrary_chunking() {
    let raw = include_str!("fixtures/anthropic_tool.sse");
    let a = feed(raw, 7);
    assert_eq!(a, feed(raw, 4096));
    assert_eq!(a.len(), 15);
    assert_eq!(a[0].0.as_deref(), Some("message_start"));
    assert_eq!(feed(&raw.replace('\n', "\r\n"), 5), a);
}

#[test]
fn anthropic_stream_accumulates_text_thinking_tool_and_usage() {
    let mut acc = anthropic::StreamAcc::default();
    let mut streamed = String::new();
    for (_, data) in feed(include_str!("fixtures/anthropic_tool.sse"), 11) {
        if let Some(t) = acc.on_data(&data).unwrap() {
            streamed.push_str(&t);
        }
    }
    let resp = acc.finish();
    assert_eq!(streamed, "Vou ler o símbolo.");
    assert_eq!(resp.stop_reason, "tool_use");
    assert_eq!(resp.usage, Usage { input: 120, cache_write: 900, cache_read: 4000, output: 57 });
    assert_eq!(resp.content.len(), 3);
    assert_eq!(
        resp.content[0],
        ContentBlock::Opaque(json!({"type":"thinking","thinking":"","signature":"sig-abc"}))
    );
    assert_eq!(resp.content[1], ContentBlock::Text { text: "Vou ler o símbolo.".into() });
    assert_eq!(
        resp.content[2],
        ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "read_symbol".into(),
            input: json!({"path":"src/lib.rs","symbol":"add"}),
        }
    );
}

#[test]
fn anthropic_stream_error_event_is_an_error() {
    let mut acc = anthropic::StreamAcc::default();
    let err = acc
        .on_data(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)
        .unwrap_err();
    assert!(err.to_string().contains("Overloaded"));
}

#[test]
fn anthropic_body_caches_prefix_and_echoes_thinking() {
    let body = anthropic::build_body(&request(), true);
    assert_eq!(body["stream"], true);
    assert_eq!(body["cache_control"], json!({"type":"ephemeral"}));
    assert_eq!(body["system"][0]["cache_control"], json!({"type":"ephemeral"}));
    assert_eq!(body["tools"][0]["eager_input_streaming"], true);
    assert_eq!(body["output_config"]["effort"], "high");
    assert_eq!(body["fallbacks"], "default");
    let asst = &body["messages"][1]["content"];
    assert_eq!(asst[0], json!({"type":"thinking","thinking":"","signature":"s1"}));
    assert_eq!(asst[1]["type"], "tool_use");
    assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
    assert!(body.get("thinking").is_none());
}

#[test]
fn openai_stream_accumulates_tool_calls_and_cached_usage() {
    let mut acc = openai::StreamAcc::default();
    let mut streamed = String::new();
    for (_, data) in feed(include_str!("fixtures/openai_tool.sse"), 9) {
        if let Some(t) = acc.on_data(&data).unwrap() {
            streamed.push_str(&t);
        }
    }
    let resp = acc.finish();
    assert_eq!(streamed, "Lendo agora.");
    assert_eq!(resp.stop_reason, "tool_use");
    assert_eq!(resp.usage, Usage { input: 904, cache_write: 0, cache_read: 4096, output: 40 });
    assert_eq!(
        resp.content,
        vec![
            ContentBlock::Text { text: "Lendo agora.".into() },
            ContentBlock::ToolUse {
                id: "call_1".into(),
                name: "read_symbol".into(),
                input: json!({"path":"src/lib.rs","symbol":"add"}),
            },
        ]
    );
}

#[test]
fn openai_body_maps_roles_tools_and_drops_opaque_blocks() {
    let body = openai::build_body(&request());
    let msgs = body["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 4);
    assert_eq!(msgs[0], json!({"role":"system","content":"You are CoTurbo."}));
    assert_eq!(msgs[1], json!({"role":"user","content":"olá"}));
    assert_eq!(msgs[2]["role"], "assistant");
    assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "read_symbol");
    assert_eq!(msgs[2]["tool_calls"][0]["function"]["arguments"], r#"{"path":"a.rs"}"#);
    assert_eq!(msgs[3], json!({"role":"tool","tool_call_id":"t1","content":"fn a() {}"}));
    assert_eq!(body["tools"][0]["function"]["parameters"]["required"][0], "path");
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[test]
fn invalid_tool_json_is_flagged_not_dropped() {
    let mut acc = anthropic::StreamAcc::default();
    for d in [
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t","name":"x","input":{}}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\": "}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
    ] {
        acc.on_data(d).unwrap();
    }
    let resp = acc.finish();
    let ContentBlock::ToolUse { input, .. } = &resp.content[0] else { panic!("not a tool use") };
    assert!(input.get(coturbo_providers::INVALID_JSON_KEY).is_some());
}

#[test]
fn pricing_computes_cost_and_unknown_models() {
    let t = PriceTable::builtin();
    let u = Usage { input: 1_000_000, cache_write: 1_000_000, cache_read: 1_000_000, output: 1_000_000 };
    let c = t.cost("claude-opus-5-5", &u).unwrap();
    assert!((c - (4.0 + 5.0 + 0.20 + 20.0)).abs() < 1e-9, "{c}");
    assert!(t.cost("some/unknown-model", &u).is_none());
    let mut total = Usage::default();
    total += u;
    total += u;
    assert_eq!(total.output, 2_000_000);
}
