use coturbo_agent::{Agent, AgentConfig, NoopObserver, Stop};
use coturbo_providers::{
    BoxFuture, ContentBlock, INVALID_JSON_KEY, Provider, ProviderError, Request, Response, Role,
    TextSink, Usage,
};
use coturbo_tools::{Mode, Tools};
use serde_json::json;
use std::sync::Mutex;

/// Replays scripted responses and records every request it receives.
struct Mock {
    script: Mutex<Vec<Response>>,
    seen: Mutex<Vec<Request>>,
}

impl Mock {
    fn new(mut script: Vec<Response>) -> Self {
        script.reverse();
        Self { script: Mutex::new(script), seen: Mutex::new(Vec::new()) }
    }
    fn requests(&self) -> Vec<Request> {
        self.seen.lock().unwrap().clone()
    }
}

impl Provider for Mock {
    fn complete<'a>(
        &'a self,
        req: &'a Request,
        on_text: &'a mut TextSink<'_>,
    ) -> BoxFuture<'a, Result<Response, ProviderError>> {
        Box::pin(async move {
            self.seen.lock().unwrap().push(req.clone());
            let resp = self.script.lock().unwrap().pop().expect("script exhausted");
            for b in &resp.content {
                if let ContentBlock::Text { text } = b {
                    on_text(text);
                }
            }
            Ok(resp)
        })
    }
}

fn usage(input: u64, output: u64) -> Usage {
    Usage { input, cache_write: 0, cache_read: 0, output }
}

fn tool(id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
    ContentBlock::ToolUse { id: id.into(), name: name.into(), input }
}

fn text(t: &str) -> ContentBlock {
    ContentBlock::Text { text: t.into() }
}

fn resp(content: Vec<ContentBlock>, stop: &str, u: Usage) -> Response {
    Response { content, stop_reason: stop.into(), usage: u }
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n").unwrap();
    dir
}

fn config() -> AgentConfig {
    AgentConfig { model: "mock".into(), max_turns: 10, max_tokens: 1000, effort: None }
}

fn tool_results(req: &Request) -> Vec<(String, String, bool)> {
    req.messages
        .last()
        .unwrap()
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                Some((tool_use_id.clone(), content.clone(), *is_error))
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn runs_tools_until_end_turn_and_sums_usage() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![text("lendo"), tool("t1", "read_file", json!({"path": "src/lib.rs"}))], "tool_use", usage(100, 10)),
        resp(vec![text("pronto")], "end_turn", usage(150, 5)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    let report = agent.run_task("leia lib.rs", &mut NoopObserver).await.unwrap();

    assert_eq!(report.stop, Stop::Done);
    assert_eq!(report.turns, 2);
    assert_eq!(report.usage, usage(250, 15));
    assert_eq!(report.final_text, "pronto");

    let reqs = mock.requests();
    let results = tool_results(&reqs[1]);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "t1");
    assert!(results[0].1.contains("a + b"));
    assert!(!results[0].2);
}

#[tokio::test]
async fn history_is_append_only_and_prefix_is_stable() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Ast);
    let thinking = ContentBlock::Opaque(json!({"type": "thinking", "thinking": "", "signature": "sig"}));
    let mock = Mock::new(vec![
        resp(vec![thinking.clone(), tool("t1", "list_files", json!({}))], "tool_use", usage(1, 1)),
        resp(vec![tool("t2", "grep", json!({"pattern": "add"}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    agent.run_task("explore", &mut NoopObserver).await.unwrap();

    let reqs = mock.requests();
    for w in reqs.windows(2) {
        assert_eq!(w[0].system, w[1].system);
        assert_eq!(w[0].tools, w[1].tools);
        assert_eq!(&w[1].messages[..w[0].messages.len()], &w[0].messages[..], "history was rewritten");
    }
    assert_eq!(reqs[1].messages[1].role, Role::Assistant);
    assert_eq!(reqs[1].messages[1].content[0], thinking, "thinking must be echoed verbatim");
}

#[tokio::test]
async fn stops_at_max_turns() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let script = (0..3)
        .map(|i| resp(vec![tool(&format!("t{i}"), "list_files", json!({}))], "tool_use", usage(1, 1)))
        .collect();
    let mock = Mock::new(script);
    let mut agent = Agent::new(&mock, &tools, AgentConfig { max_turns: 3, ..config() });
    let report = agent.run_task("loop", &mut NoopObserver).await.unwrap();
    assert_eq!(report.stop, Stop::MaxTurns);
    assert_eq!(report.turns, 3);
}

#[tokio::test]
async fn invalid_tool_json_becomes_error_result() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "read_file", json!({INVALID_JSON_KEY: "{\"path\": "}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    agent.run_task("x", &mut NoopObserver).await.unwrap();
    let results = tool_results(&mock.requests()[1]);
    assert!(results[0].2);
    assert!(results[0].1.contains("invalid JSON"));
}

#[tokio::test]
async fn truncated_turn_still_answers_every_tool_use() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "write_file", json!({"path": "a.txt"}))], "max_tokens", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    agent.run_task("x", &mut NoopObserver).await.unwrap();
    let results = tool_results(&mock.requests()[1]);
    assert_eq!(results.len(), 1);
    assert!(results[0].2);
    assert!(!dir.path().join("a.txt").exists(), "truncated tool call must not run");
}

#[tokio::test]
async fn compact_resets_history_to_a_summary_of_edits() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "a + b", "new": "a - b"}))], "tool_use", usage(1, 1)),
        resp(vec![text("troquei para subtração")], "end_turn", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    let r = agent.run_task("mude add", &mut NoopObserver).await.unwrap();
    assert!(r.edited.contains("src/lib.rs"));
    agent.compact();
    agent.run_task("próxima tarefa", &mut NoopObserver).await.unwrap();

    let last = mock.requests().pop().unwrap();
    assert_eq!(last.messages.len(), 1);
    let ContentBlock::Text { text } = &last.messages[0].content[0] else { panic!() };
    assert!(text.contains("mude add"));
    assert!(text.contains("src/lib.rs"));
    assert!(text.contains("troquei para subtração"));
    assert!(text.ends_with("próxima tarefa"));
    assert_eq!(agent.session_usage(), usage(3, 3));
}

#[tokio::test]
async fn refusal_stops_the_task() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![resp(vec![], "refusal", usage(1, 0))]);
    let mut agent = Agent::new(&mock, &tools, config());
    let r = agent.run_task("x", &mut NoopObserver).await.unwrap();
    assert_eq!(r.stop, Stop::Refused);
}
