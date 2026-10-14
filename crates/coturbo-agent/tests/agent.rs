use coturbo_agent::approval::{ApprovalConfig, ApprovalMode};
use coturbo_agent::plan::{PlanChoice, PlanReview};
use coturbo_agent::{Agent, AgentConfig, ApprovalRequest, Decision, Host, NoopHost, Stop};
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
                    on_text(coturbo_providers::Delta::Text(text.clone()));
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
    AgentConfig { model: "mock".into(), max_turns: 10, max_tokens: 1000, effort: None, approval: ApprovalConfig::yolo(), plan_model: None, context_window: 1000, gate: None, max_fix_rounds: 3 }
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
    let report = agent.run_task("leia lib.rs", &mut NoopHost).await.unwrap();

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
    agent.run_task("explore", &mut NoopHost).await.unwrap();

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
    let report = agent.run_task("loop", &mut NoopHost).await.unwrap();
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
    agent.run_task("x", &mut NoopHost).await.unwrap();
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
    agent.run_task("x", &mut NoopHost).await.unwrap();
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
    let r = agent.run_task("mude add", &mut NoopHost).await.unwrap();
    assert!(r.edited.contains("src/lib.rs"));
    agent.compact();
    agent.run_task("próxima tarefa", &mut NoopHost).await.unwrap();

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
    let r = agent.run_task("x", &mut NoopHost).await.unwrap();
    assert_eq!(r.stop, Stop::Refused);
}

/// Answers approval prompts from a script and records what it was asked.
struct ScriptedHost {
    answers: Vec<Decision>,
    asked: Vec<ApprovalRequest>,
}

impl ScriptedHost {
    fn new(mut answers: Vec<Decision>) -> Self {
        answers.reverse();
        Self { answers, asked: Vec::new() }
    }
}

impl Host for ScriptedHost {
    fn approve<'a>(&'a mut self, req: &'a ApprovalRequest) -> BoxFuture<'a, Decision> {
        self.asked.push(req.clone());
        let d = self.answers.pop().expect("unexpected prompt");
        Box::pin(async move { d })
    }
}

fn ask_config(mode: ApprovalMode) -> AgentConfig {
    let approval = ApprovalConfig::new(mode, &Default::default()).unwrap();
    AgentConfig { approval, ..config() }
}

#[tokio::test]
async fn denied_edit_reports_reason_and_leaves_file_untouched() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "a + b", "new": "a - b"}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut host = ScriptedHost::new(vec![Decision::Deny(Some("keep addition".into()))]);
    let mut agent = Agent::new(&mock, &tools, ask_config(ApprovalMode::AlwaysAsk));
    let r = agent.run_task("x", &mut host).await.unwrap();

    assert_eq!(host.asked.len(), 1);
    let diff = host.asked[0].preview.as_deref().unwrap();
    assert!(diff.contains("-    a + b") && diff.contains("+    a - b"), "{diff}");
    let results = tool_results(&mock.requests()[1]);
    assert_eq!(results[0].1, "denied by user: keep addition");
    assert!(results[0].2);
    assert!(r.edited.is_empty());
    assert!(std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap().contains("a + b"));
}

#[tokio::test]
async fn session_grant_is_remembered_across_tasks() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "bash", json!({"command": "echo one"}))], "tool_use", usage(1, 1)),
        resp(vec![tool("t2", "bash", json!({"command": "echo two"}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
        resp(vec![tool("t3", "bash", json!({"command": "echo three"}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut host = ScriptedHost::new(vec![Decision::Session]);
    let mut agent = Agent::new(&mock, &tools, ask_config(ApprovalMode::Write));
    agent.run_task("a", &mut host).await.unwrap();
    agent.compact();
    agent.run_task("b", &mut host).await.unwrap();
    assert_eq!(host.asked.len(), 1, "one prompt for every `echo …`");
    let results = tool_results(&mock.requests()[4]);
    assert!(results[0].1.contains("three"));
}

#[tokio::test]
async fn write_mode_edits_without_asking_and_noop_host_denies_commands() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(
            vec![
                tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "a + b", "new": "a - b"})),
                tool("t2", "bash", json!({"command": "echo hi"})),
            ],
            "tool_use",
            usage(1, 1),
        ),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut agent = Agent::new(&mock, &tools, ask_config(ApprovalMode::Write));
    let r = agent.run_task("x", &mut NoopHost).await.unwrap();
    assert!(r.edited.contains("src/lib.rs"));
    let results = tool_results(&mock.requests()[1]);
    assert!(!results[0].2);
    assert!(results[1].2 && results[1].1.contains("--auto-approve"), "{:?}", results[1]);
}

#[tokio::test]
async fn failing_edit_is_not_prompted() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "nope", "new": "x"}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut host = ScriptedHost::new(vec![]);
    let mut agent = Agent::new(&mock, &tools, ask_config(ApprovalMode::AlwaysAsk));
    agent.run_task("x", &mut host).await.unwrap();
    let results = tool_results(&mock.requests()[1]);
    assert!(results[0].2 && results[0].1.contains("not found"));
}

/// Fails its first call, then replays the inner mock.
struct FlakyOnce {
    inner: Mock,
    failed: Mutex<bool>,
}

impl Provider for FlakyOnce {
    fn complete<'a>(
        &'a self,
        req: &'a Request,
        on_text: &'a mut TextSink<'_>,
    ) -> BoxFuture<'a, Result<Response, ProviderError>> {
        let first = !std::mem::replace(&mut *self.failed.lock().unwrap(), true);
        if first {
            return Box::pin(async { Err(ProviderError::Stream("boom".into())) });
        }
        self.inner.complete(req, on_text)
    }
}

#[tokio::test]
async fn provider_error_restores_history() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let flaky = FlakyOnce { inner: Mock::new(vec![resp(vec![text("ok")], "end_turn", usage(1, 1))]), failed: Mutex::new(false) };
    let mut agent = Agent::new(&flaky, &tools, config());
    assert!(agent.run_task("lost", &mut NoopHost).await.is_err());
    agent.run_task("retry", &mut NoopHost).await.unwrap();
    let req = flaky.inner.requests().pop().unwrap();
    assert_eq!(req.messages.len(), 1, "failed task must not leave a dangling user message");
    let ContentBlock::Text { text } = &req.messages[0].content[0] else { panic!() };
    assert_eq!(text, "retry");
}

/// Answers plan reviews from a script.
struct PlanHost {
    choices: Vec<PlanChoice>,
    reviews: Vec<PlanReview>,
}

impl Host for PlanHost {
    fn review_plan<'a>(&'a mut self, plan: &'a PlanReview) -> BoxFuture<'a, PlanChoice> {
        self.reviews.push(plan.clone());
        let c = self.choices.remove(0);
        Box::pin(async move { c })
    }
}

fn plan_writes() -> Vec<Response> {
    vec![
        resp(
            vec![
                tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "a + b", "new": "a - b"})),
                tool("t2", "bash", json!({"command": "echo hi"})),
                tool("t3", "write_file", json!({"path": ".coturbo/plans/sub-plan.md", "content": "# Plan\n1. change add to sub\n"})),
            ],
            "tool_use",
            usage(10, 1),
        ),
        resp(vec![tool("t4", "propose_plan", json!({"slug": "sub"}))], "tool_use", usage(10, 1)),
    ]
}

fn plan_agent_config() -> AgentConfig {
    AgentConfig { plan_model: Some("planner".into()), ..config() }
}

#[tokio::test]
async fn plan_mode_blocks_changes_then_execute_starts_fresh_from_the_plan() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mut script = plan_writes();
    script.push(resp(vec![tool("t5", "edit_file", json!({"path": "src/lib.rs", "old": "a + b", "new": "a - b"}))], "tool_use", usage(1, 1)));
    script.push(resp(vec![text("implemented")], "end_turn", usage(1, 1)));
    let mock = Mock::new(script);
    let mut host = PlanHost { choices: vec![PlanChoice::Execute], reviews: vec![] };
    let mut agent = Agent::new(&mock, &tools, plan_agent_config());
    agent.set_plan_mode(true);
    let r = agent.run_task("make add subtract", &mut host).await.unwrap();

    let reqs = mock.requests();
    // Plan phase: read-only enforced, plan file allowed, planner model used.
    let ContentBlock::Text { text: first } = &reqs[0].messages[0].content[0] else { panic!() };
    assert!(first.starts_with("[Plan mode is active]") && first.ends_with("make add subtract"));
    assert_eq!(reqs[0].model, "planner");
    let results = tool_results(&reqs[1]);
    assert!(results[0].2 && results[0].1.contains("plan mode"), "{:?}", results[0]);
    assert!(results[1].2, "bash must be refused in plan mode");
    assert!(!results[2].2, "plan file write must succeed: {:?}", results[2]);
    assert_eq!(host.reviews.len(), 1);
    assert_eq!(host.reviews[0].slug, "sub");
    assert!(host.reviews[0].content.contains("change add to sub"));
    assert_eq!(host.reviews[0].context_pct, 1);

    // Execute phase: fresh history holding only the plan, normal model, writes allowed.
    let exec = &reqs[2];
    assert_eq!(exec.model, "mock");
    assert_eq!(exec.messages.len(), 1);
    let ContentBlock::Text { text: p } = &exec.messages[0].content[0] else { panic!() };
    assert!(p.contains("approved plan") && p.contains("change add to sub"));
    assert!(!tool_results(&reqs[3])[0].2);
    assert!(std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap().contains("a - b"));
    assert_eq!(r.stop, Stop::Done);
    assert_eq!(r.final_text, "implemented");
    assert_eq!(r.turns, 4);
    assert!(!agent.plan_mode());
    // The tool list never changes, so the cached prefix survives the mode switch.
    assert!(reqs.windows(2).all(|w| w[0].tools == w[1].tools));
}

#[tokio::test]
async fn refine_keeps_plan_mode_and_save_quit_stops() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mut script = plan_writes();
    script.push(resp(vec![tool("t5", "propose_plan", json!({"slug": "sub"}))], "tool_use", usage(1, 1)));
    let mock = Mock::new(script);
    let mut host = PlanHost { choices: vec![PlanChoice::Refine("add a test".into()), PlanChoice::SaveQuit], reviews: vec![] };
    let mut agent = Agent::new(&mock, &tools, plan_agent_config());
    agent.set_plan_mode(true);
    let r = agent.run_task("x", &mut host).await.unwrap();

    let reqs = mock.requests();
    let refine = tool_results(&reqs[2]);
    assert!(!refine[0].2 && refine[0].1.contains("add a test"));
    assert_eq!(reqs[2].model, "planner", "still planning after refine");
    assert_eq!(r.stop, Stop::PlanSaved);
    assert!(!agent.plan_mode());
    assert!(dir.path().join(".coturbo/plans/sub-plan.md").exists());
    assert!(std::fs::read_to_string(dir.path().join("src/lib.rs")).unwrap().contains("a + b"));
}

#[tokio::test]
async fn keep_context_continues_in_the_same_conversation() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mut script = plan_writes();
    script.push(resp(vec![text("done")], "end_turn", usage(1, 1)));
    let mock = Mock::new(script);
    let mut host = PlanHost { choices: vec![PlanChoice::KeepContext], reviews: vec![] };
    let mut agent = Agent::new(&mock, &tools, plan_agent_config());
    agent.set_plan_mode(true);
    agent.run_task("x", &mut host).await.unwrap();
    let reqs = mock.requests();
    assert_eq!(&reqs[2].messages[..reqs[1].messages.len()], &reqs[1].messages[..], "history kept");
    assert!(tool_results(&reqs[2])[0].1.contains("Write access is restored"));
    assert_eq!(reqs[2].model, "mock");
}

#[tokio::test]
async fn keep_context_falls_back_to_compact_when_context_is_full() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mut script = plan_writes();
    script[1].usage = usage(990, 1); // 99% of the 1000-token window
    script.push(resp(vec![text("done")], "end_turn", usage(1, 1)));
    let mock = Mock::new(script);
    let mut host = PlanHost { choices: vec![PlanChoice::KeepContext], reviews: vec![] };
    let mut agent = Agent::new(&mock, &tools, plan_agent_config());
    agent.set_plan_mode(true);
    agent.run_task("make add subtract", &mut host).await.unwrap();
    assert!(!host.reviews[0].keep_context_allowed());
    let exec = mock.requests().pop().unwrap();
    assert_eq!(exec.messages.len(), 1, "compacted to a single summary + plan message");
    let ContentBlock::Text { text } = &exec.messages[0].content[0] else { panic!() };
    assert!(text.contains("[Previous task]") && text.contains("approved plan"));
}

#[tokio::test]
async fn propose_plan_outside_plan_mode_or_without_file_is_an_error() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "propose_plan", json!({"slug": "x"}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    agent.run_task("a", &mut NoopHost).await.unwrap();
    assert!(tool_results(&mock.requests()[1])[0].1.contains("not active"));

    let mock = Mock::new(vec![
        resp(vec![tool("t2", "propose_plan", json!({"slug": "missing"}))], "tool_use", usage(1, 1)),
        resp(vec![text("ok")], "end_turn", usage(1, 1)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    agent.set_plan_mode(true);
    agent.run_task("b", &mut NoopHost).await.unwrap();
    let res = tool_results(&mock.requests()[1]);
    assert!(res[0].2 && res[0].1.contains("write the plan first"), "{:?}", res[0]);
}

fn cargo_project() -> tempfile::TempDir {
    let dir = project();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n#[test]\nfn adds() {\n    assert_eq!(add(2, 2), 4);\n}\n",
    )
    .unwrap();
    dir
}

fn gate_config() -> AgentConfig {
    AgentConfig { gate: Some(coturbo_verify::GateConfig::default()), max_fix_rounds: 2, ..config() }
}

/// Collects gate summaries.
#[derive(Default)]
struct GateWatch {
    gates: Vec<(bool, String)>,
    cheats: Vec<String>,
    accept_cheats: bool,
}

impl Host for GateWatch {
    fn on_event(&mut self, ev: coturbo_agent::Event<'_>) {
        if let coturbo_agent::Event::Gate { ok, summary } = ev {
            self.gates.push((ok, summary.to_string()));
        }
    }
    fn confirm_cheat<'a>(&'a mut self, f: &'a coturbo_verify::Finding) -> BoxFuture<'a, bool> {
        self.cheats.push(f.render());
        let ok = self.accept_cheats;
        Box::pin(async move { ok })
    }
}

#[tokio::test]
async fn gate_sends_warnings_back_until_the_code_is_clean() {
    let dir = cargo_project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "    a + b", "new": "    let unused = 0;\n    a + b"}))], "tool_use", usage(1, 1)),
        resp(vec![text("done")], "end_turn", usage(1, 1)),
        resp(vec![tool("t2", "edit_file", json!({"path": "src/lib.rs", "old": "    let unused = 0;\n", "new": ""}))], "tool_use", usage(1, 1)),
        resp(vec![text("fixed")], "end_turn", usage(1, 1)),
    ]);
    let mut host = GateWatch::default();
    let mut agent = Agent::new(&mock, &tools, gate_config());
    let r = agent.run_task("x", &mut host).await.unwrap();

    assert_eq!(r.verified, Some(true));
    assert_eq!(r.final_text, "fixed");
    assert_eq!(host.gates.len(), 2);
    assert!(!host.gates[0].0 && host.gates[0].1.contains("unused variable"), "{}", host.gates[0].1);
    assert!(host.gates[1].0 && host.gates[1].1.contains("0 errors, 0 warnings"));
    // The failure reached the model as a plain user message, appended.
    let reqs = mock.requests();
    let ContentBlock::Text { text } = &reqs[2].messages.last().unwrap().content[0] else { panic!() };
    assert!(text.starts_with("✗ verification failed at `build`"));
}

#[tokio::test]
async fn gate_gives_up_after_max_fix_rounds() {
    let dir = cargo_project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "add(2, 2), 4", "new": "add(2, 2), 5"}))], "tool_use", usage(1, 1)),
        resp(vec![text("done")], "end_turn", usage(1, 1)),
        resp(vec![text("still done")], "end_turn", usage(1, 1)),
        resp(vec![text("really done")], "end_turn", usage(1, 1)),
    ]);
    let mut host = GateWatch::default();
    let mut agent = Agent::new(&mock, &tools, gate_config());
    let r = agent.run_task("x", &mut host).await.unwrap();
    assert_eq!(r.verified, Some(false));
    assert_eq!(host.gates.len(), 3, "first try + 2 fix rounds");
    assert!(host.gates.iter().all(|(ok, s)| !ok && s.contains("failed at `test`") && s.contains("adds")));
}

#[tokio::test]
async fn ignoring_a_test_is_refused_unless_the_user_accepts_it() {
    let edit = || {
        vec![
            resp(vec![tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "#[test]\n", "new": "#[test]\n#[ignore]\n"}))], "tool_use", usage(1, 1)),
            resp(vec![text("done")], "end_turn", usage(1, 1)),
            resp(vec![text("ok")], "end_turn", usage(1, 1)),
        ]
    };
    // Rejected: the model is told why and the task ends unverified.
    let dir = cargo_project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(edit());
    let mut host = GateWatch::default();
    let cfg = AgentConfig { max_fix_rounds: 1, approval: ApprovalConfig::new(ApprovalMode::Write, &Default::default()).unwrap(), ..gate_config() };
    let mut agent = Agent::new(&mock, &tools, cfg.clone());
    let r = agent.run_task("x", &mut host).await.unwrap();
    assert_eq!(host.cheats, ["src/lib.rs: 1 new skipped/ignored test(s)"]);
    assert!(host.gates[0].1.contains("weaken the checks"));
    assert_eq!(r.verified, Some(false));

    // Accepted once: not asked again, and the real gate runs.
    let dir = cargo_project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(edit());
    let mut host = GateWatch { accept_cheats: true, ..GateWatch::default() };
    let mut agent = Agent::new(&mock, &tools, cfg);
    let r = agent.run_task("x", &mut host).await.unwrap();
    assert_eq!(host.cheats.len(), 1);
    assert_eq!(r.verified, Some(true), "{:?}", host.gates);
}

#[tokio::test]
async fn gate_is_skipped_without_code_edits_or_build_system() {
    // Read-only task: no gate.
    let dir = cargo_project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![resp(vec![text("nothing to change")], "end_turn", usage(1, 1))]);
    let mut host = GateWatch::default();
    let mut agent = Agent::new(&mock, &tools, gate_config());
    let r = agent.run_task("x", &mut host).await.unwrap();
    assert_eq!(r.verified, None);
    assert!(host.gates.is_empty());

    // Edits in a project with no build system: nothing to run, not a failure.
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![tool("t1", "write_file", json!({"path": "notes.txt", "content": "hi"}))], "tool_use", usage(1, 1)),
        resp(vec![text("done")], "end_turn", usage(1, 1)),
    ]);
    let mut host = GateWatch::default();
    let mut agent = Agent::new(&mock, &tools, gate_config());
    let r = agent.run_task("x", &mut host).await.unwrap();
    assert_eq!(r.verified, None);
    assert!(host.gates[0].1.starts_with("verification skipped"));
}

/// Records what a terminal would show.
#[derive(Default)]
struct Recorder {
    texts: String,
    diffs: Vec<String>,
    turns: Vec<(f64, u32)>,
}

impl Host for Recorder {
    fn on_event(&mut self, ev: coturbo_agent::Event<'_>) {
        match ev {
            coturbo_agent::Event::Text(t) => self.texts.push_str(t),
            coturbo_agent::Event::ToolEnd { diff: Some(d), .. } => self.diffs.push(d.to_string()),
            coturbo_agent::Event::Turn { secs, window, .. } => self.turns.push((secs, window)),
            _ => {}
        }
    }
}

#[tokio::test]
async fn edits_report_their_diff_and_turns_their_timing() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(
            vec![
                tool("t1", "edit_file", json!({"path": "src/lib.rs", "old": "a + b", "new": "a - b"})),
                tool("t2", "edit_file", json!({"path": "src/lib.rs", "old": "nope", "new": "x"})),
            ],
            "tool_use",
            usage(1, 1),
        ),
        resp(vec![text("done")], "end_turn", usage(1, 1)),
    ]);
    let mut rec = Recorder::default();
    let mut agent = Agent::new(&mock, &tools, config());
    agent.run_task("x", &mut rec).await.unwrap();
    assert_eq!(rec.diffs.len(), 1, "failed edits show no diff");
    assert!(rec.diffs[0].contains("-    a + b") && rec.diffs[0].contains("+    a - b"));
    assert_eq!(rec.texts, "done");
    assert_eq!(rec.turns.len(), 2);
    assert!(rec.turns.iter().all(|(s, w)| *s >= 0.0 && *w == 1000));
}

#[tokio::test]
async fn empty_replies_are_retried_without_touching_history() {
    let dir = project();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let mock = Mock::new(vec![
        resp(vec![], "end_turn", usage(10, 50)),
        resp(vec![tool("t1", "read_file", json!({"path": "src/lib.rs"}))], "tool_use", usage(10, 5)),
        resp(vec![text("done")], "end_turn", usage(10, 2)),
    ]);
    let mut agent = Agent::new(&mock, &tools, config());
    let r = agent.run_task("x", &mut NoopHost).await.unwrap();
    assert_eq!(r.final_text, "done");
    assert_eq!(r.turns, 3);
    let reqs = mock.requests();
    assert_eq!(reqs[0].messages, reqs[1].messages, "the empty reply left no trace");

    // Gives up after the retries: the task ends instead of looping.
    let mock = Mock::new(vec![resp(vec![], "end_turn", usage(1, 1)), resp(vec![], "end_turn", usage(1, 1)), resp(vec![], "end_turn", usage(1, 1))]);
    let mut agent = Agent::new(&mock, &tools, config());
    let r = agent.run_task("x", &mut NoopHost).await.unwrap();
    assert_eq!((r.stop, r.turns), (Stop::Done, 3));
}
