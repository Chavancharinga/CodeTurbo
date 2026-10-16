use coturbo_providers::{Delta, 
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
    let mut thinking = String::new();
    for (_, data) in feed(include_str!("fixtures/anthropic_tool.sse"), 11) {
        match acc.on_data(&data).unwrap() {
            Some(Delta::Text(t)) => streamed.push_str(&t),
            Some(Delta::Thinking(t)) => thinking.push_str(&t),
            _ => {}
        }
    }
    assert_eq!(thinking, "", "the fixture's thinking is omitted (empty)");
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
    assert_eq!(body["thinking"], json!({"type": "adaptive", "display": "summarized"}));
    let mut old = request();
    old.model = "claude-haiku-4-5".into();
    assert!(anthropic::build_body(&old, false).get("thinking").is_none(), "older models keep their defaults");
}

#[test]
fn openai_stream_accumulates_tool_calls_and_cached_usage() {
    let mut acc = openai::StreamAcc::default();
    let mut streamed = String::new();
    for (_, data) in feed(include_str!("fixtures/openai_tool.sse"), 9) {
        if let Some(Delta::Text(t)) = acc.on_data(&data).unwrap() {
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

#[test]
fn thinking_deltas_are_streamed_separately() {
    let mut acc = anthropic::StreamAcc::default();
    acc.on_data(r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#).unwrap();
    let d = acc.on_data(r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Checking the tests."}}"#).unwrap();
    assert_eq!(d, Some(Delta::Thinking("Checking the tests.".into())));
    let resp = acc.finish();
    assert_eq!(resp.content[0], ContentBlock::Opaque(json!({"type":"thinking","thinking":"Checking the tests.","signature":""})));

    let mut acc = openai::StreamAcc::default();
    let d = acc.on_data(r#"{"choices":[{"index":0,"delta":{"reasoning_content":"hmm"}}]}"#).unwrap();
    assert_eq!(d, Some(Delta::Thinking("hmm".into())));
    assert!(acc.finish().content.is_empty(), "reasoning is not part of the answer");
}

#[test]
fn screenshots_reach_both_apis() {
    let mut req = request();
    req.messages.push(Message {
        role: Role::User,
        content: vec![
            ContentBlock::ToolResult { tool_use_id: "t2".into(), content: "rendered".into(), is_error: false },
            ContentBlock::Image { media_type: "image/png".into(), data: "iVBORw0K".into() },
        ],
    });
    let a = anthropic::build_body(&req, false);
    let last = a["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["content"][0]["type"], "tool_result", "results first");
    assert_eq!(last["content"][1], json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0K"}}));
    let o = openai::build_body(&req);
    let msgs = o["messages"].as_array().unwrap();
    let n = msgs.len();
    assert_eq!(msgs[n - 2]["role"], "tool");
    assert_eq!(msgs[n - 1]["role"], "user");
    assert_eq!(msgs[n - 1]["content"][1]["image_url"]["url"], "data:image/png;base64,iVBORw0K");
}

#[test]
fn the_text_tool_protocol_round_trips() {
    use coturbo_providers::cli::{parse_reply, protocol_prompt, protocol_system};
    let reply = "Vou ler.\n<tool_call>{\"name\": \"read_file\", \"input\": {\"path\": \"a.rs\"}}</tool_call>\n<tool_call>{\"name\": \"grep\", \"input\": {\"pattern\": \"fn\"}}</tool_call>";
    let blocks = parse_reply(reply, 3);
    assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "Vou ler."));
    assert!(matches!(&blocks[1], ContentBlock::ToolUse { id, name, input } if id == "cli-3-1" && name == "read_file" && input["path"] == "a.rs"));
    assert!(matches!(&blocks[2], ContentBlock::ToolUse { name, .. } if name == "grep"));
    let bad = parse_reply("<tool_call>{nope</tool_call>", 0);
    assert!(matches!(&bad[0], ContentBlock::ToolUse { input, .. } if input.get(coturbo_providers::INVALID_JSON_KEY).is_some()), "{bad:?}");
    assert!(matches!(&parse_reply("pronto, terminei", 0)[0], ContentBlock::Text { .. }));

    let req = request();
    let sys = protocol_system(&req);
    assert!(sys.starts_with("You are CoTurbo.") && sys.contains("\"read_symbol\"") && sys.contains("<tool_call>"));
    let prompt = protocol_prompt(&req);
    assert!(prompt.contains("[user]\nolá") && prompt.contains("<tool_call>{\"input\":{\"path\":\"a.rs\"},\"name\":\"read_symbol\"}</tool_call>"), "{prompt}");
    assert!(prompt.ends_with("[assistant]\n"));
}


#[test]
fn claude_native_invoke_format_is_parsed() {
    use coturbo_providers::cli::parse_reply;
    // Claude sometimes emits its own <invoke>/<parameter> form inside <tool_call>.
    let reply = "Vou criar.
<tool_call>
<invoke name=\"write_file\">
<parameter name=\"path\">a.txt</parameter>
<parameter name=\"content\">oi</parameter>
</invoke>
</tool_call>";
    let b = parse_reply(reply, 1);
    assert!(matches!(&b[0], ContentBlock::Text { text } if text == "Vou criar."), "{b:?}");
    assert!(matches!(&b[1], ContentBlock::ToolUse { name, input, .. } if name == "write_file" && input["path"] == "a.txt" && input["content"] == "oi"), "{b:?}");
    // And as bare <invoke> with no wrapper.
    let bare = parse_reply("<invoke name=\"bash\"><parameter name=\"command\">ls</parameter></invoke>", 2);
    assert!(matches!(&bare[0], ContentBlock::ToolUse { name, input, .. } if name == "bash" && input["command"] == "ls"), "{bare:?}");
    // JSON form still works.
    let js = parse_reply("<tool_call>{\"name\": \"read_file\", \"input\": {\"path\": \"x\"}}</tool_call>", 3);
    assert!(matches!(&js[0], ContentBlock::ToolUse { name, .. } if name == "read_file"));
}

fn with_images(n: usize) -> Request {
    let mut req = request();
    for i in 0..n {
        req.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Image { media_type: "image/png".into(), data: format!("img{i}") }, ContentBlock::Text { text: format!("shot {i}") }],
        });
    }
    req
}

#[test]
fn the_subscription_provider_keeps_only_the_latest_images() {
    use coturbo_providers::cli::{prompt_images, protocol_prompt};
    let req = with_images(6);
    let prompt = protocol_prompt(&req);
    assert_eq!(prompt.matches("is attached to this message").count(), 4, "{prompt}");
    assert_eq!(prompt.matches("[an earlier image was omitted]").count(), 2, "{prompt}");
    assert!(prompt.contains("[image 1 of 4 is attached") && prompt.contains("[image 4 of 4 is attached"), "{prompt}");
    let kept = prompt_images(&req);
    assert_eq!(kept.iter().map(|(_, d)| d.as_str()).collect::<Vec<_>>(), ["img2", "img3", "img4", "img5"]);
    assert!(kept.iter().all(|(m, _)| m == "image/png"));
    // No Read tool and no files: the prompt must not mention either.
    assert!(!prompt.contains("Read tool") && !prompt.contains("vision-"), "{prompt}");
}

#[test]
fn images_travel_inside_the_streaming_message() {
    use coturbo_providers::cli::{prompt_images, stream_message};
    let req = with_images(2);
    let line = stream_message("olhe", &prompt_images(&req));
    assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "one JSON line");
    let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(v["type"], "user");
    let content = v["message"]["content"].as_array().unwrap();
    assert_eq!(content.len(), 3);
    assert_eq!(content[0], json!({"type": "text", "text": "olhe"}));
    assert_eq!(content[1], json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "img0"}}));
    assert_eq!(content[2]["source"]["data"], "img1");
}

#[test]
fn the_result_is_found_among_the_streamed_events() {
    use coturbo_providers::cli::result_message;
    let out = "{\"type\":\"system\",\"subtype\":\"init\"}\n\n{\"type\":\"assistant\",\"message\":{}}\nnot json\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"pronto\",\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}\n";
    let r = result_message(out).unwrap();
    assert_eq!(r["result"], "pronto");
    assert_eq!(r["usage"]["input_tokens"], 5);
    assert!(result_message("{\"type\":\"system\"}\n").is_none());
}

#[test]
fn antigravity_json_gives_the_reply_and_tokens() {
    use coturbo_providers::cli::parse_antigravity;
    let ok = r#"{"conversation_id":"c","status":"SUCCESS","response":"oi\n","usage":{"input_tokens":100,"output_tokens":7,"cache_read_tokens":60}}"#;
    let (text, usage) = parse_antigravity(ok).unwrap();
    assert_eq!(text, "oi\n");
    assert_eq!((usage.input, usage.cache_read, usage.output), (40, 60, 7));
    assert!(parse_antigravity(r#"{"status":"ERROR","error":"authentication required"}"#).unwrap_err().contains("authentication"));
    assert!(parse_antigravity("nada").is_err());
}

#[test]
fn the_code_being_written_is_read_from_a_call_that_is_still_streaming() {
    use coturbo_providers::partial_code;
    // JSON, cut in the middle of the content: what has arrived is shown.
    let json = r#"{"path": "app.js", "content": "const a = 1;\nconst b = \"two\";\nfunc"#;
    assert_eq!(partial_code(json).as_deref(), Some("const a = 1;\nconst b = \"two\";\nfunc"));
    // The native XML form.
    let xml = "<invoke name=\"write_file\"><parameter name=\"path\">a.js</parameter><parameter name=\"content\">let x;\nlet y;";
    assert_eq!(partial_code(xml).as_deref(), Some("let x;\nlet y;"));
    // Nothing to show before the content starts, and only the end of a long file.
    assert_eq!(partial_code(r#"{"path": "app.js"}"#), None);
    let long = format!(r#"{{"content": "{}"}}"#, (1..=40).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n"));
    let tail = partial_code(&long).unwrap();
    assert_eq!(tail.lines().count(), 14);
    assert!(tail.ends_with("line40") && tail.starts_with("line27"), "{tail}");
}
