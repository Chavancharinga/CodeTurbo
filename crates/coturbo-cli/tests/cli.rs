use coturbo_agent::{AgentConfig, Stop};
use coturbo_cli::bench::{self, BenchRun, Task};
use coturbo_cli::config::{self, Config, Pricer, parse_mode};
use coturbo_cli::status::{human, status_line, tool_line};
use coturbo_providers::{
    BoxFuture, ContentBlock, PriceTable, Provider, ProviderError, Request, Response, TextSink, Usage,
};
use coturbo_tools::{Mode, ToolOutput};
use serde_json::json;
use std::sync::Mutex;

struct Mock(Mutex<Vec<Response>>);

impl Mock {
    fn new(mut script: Vec<Response>) -> Self {
        script.reverse();
        Self(Mutex::new(script))
    }
}

impl Provider for Mock {
    fn complete<'a>(
        &'a self,
        _req: &'a Request,
        _on_text: &'a mut TextSink<'_>,
    ) -> BoxFuture<'a, Result<Response, ProviderError>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().pop().expect("script exhausted")) })
    }
}

fn usage(input: u64, output: u64) -> Usage {
    Usage { input, cache_write: 0, cache_read: 0, output }
}

fn agent_cfg() -> AgentConfig {
    AgentConfig { model: "m".into(), max_turns: 5, max_tokens: 1000, effort: None }
}

#[test]
fn config_defaults_when_dir_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let (c, prices) = config::load(Some(dir.path())).unwrap();
    assert_eq!(c, Config::default());
    assert!(prices.cost("claude-sonnet-5-5", &usage(1, 1)).is_some());
}

#[test]
fn config_file_overrides_and_pricing_merges() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "provider = \"ollama\"\nmodel = \"qwen\"\ntools = \"naive\"\n").unwrap();
    std::fs::write(
        dir.path().join("pricing.toml"),
        "[models.qwen]\ninput = 1.0\ncache_write = 1.0\ncache_read = 1.0\noutput = 2.0\n",
    )
    .unwrap();
    let (c, prices) = config::load(Some(dir.path())).unwrap();
    assert_eq!((c.provider.as_str(), c.model.as_str(), c.tools.as_str()), ("ollama", "qwen", "naive"));
    assert_eq!(c.max_turns, Config::default().max_turns);
    assert_eq!(prices.cost("qwen", &usage(1_000_000, 1_000_000)), Some(3.0));
}

#[test]
fn config_rejects_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "modle = \"x\"\n").unwrap();
    let err = config::load(Some(dir.path())).unwrap_err();
    assert!(err.contains("config.toml"), "{err}");
}

#[test]
fn modes_and_providers_are_validated() {
    assert_eq!(parse_mode("ast"), Ok(Mode::Ast));
    assert_eq!(parse_mode("naive"), Ok(Mode::Naive));
    assert!(parse_mode("fast").is_err());
    let c = Config { provider: "nope".into(), ..Config::default() };
    assert!(config::make_provider(&c).err().unwrap().contains("unknown provider"));
    let c = Config { provider: "ollama".into(), ..Config::default() };
    assert!(config::make_provider(&c).is_ok());
}

#[test]
fn ollama_models_fall_back_to_free_price() {
    let p = Pricer { table: PriceTable::builtin(), model: "qwen3".into(), provider: "ollama".into() };
    assert_eq!(p.cost(&usage(10, 10)), Some(0.0));
    let p = Pricer { table: PriceTable::builtin(), model: "qwen3".into(), provider: "openai".into() };
    assert_eq!(p.cost(&usage(10, 10)), None);
}

#[test]
fn status_line_formats() {
    assert_eq!(human(950), "950");
    assert_eq!(human(12_345), "12.3k");
    assert_eq!(human(1_200_000), "1.20M");
    let u = Usage { input: 1_000, cache_write: 500, cache_read: 20_000, output: 300 };
    assert_eq!(status_line(&u, Some(0.01234)), "tokens ↑1.5k ⟳20.0k ↓300 · $0.0123");
    assert_eq!(status_line(&u, None), "tokens ↑1.5k ⟳20.0k ↓300");
}

#[test]
fn tool_line_shows_errors_only_when_failed() {
    let ok = tool_line("read_file", &json!({"path": "src/lib.rs"}), &ToolOutput::ok("fn a()\n"));
    assert_eq!(ok, "  ✓ read_file src/lib.rs");
    let bad = tool_line("bash", &json!({"command": "cargo x"}), &ToolOutput::err("exit 101\nmore"));
    assert_eq!(bad, "  ✗ bash cargo x — exit 101");
}

#[test]
fn tasks_load_sorted_and_strict() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.toml"), "id = \"b\"\nprompt = \"p\"\nverify = \"v\"\n").unwrap();
    std::fs::write(dir.path().join("a.toml"), "id = \"a\"\nprompt = \"p\"\nverify = \"v\"\n").unwrap();
    std::fs::write(dir.path().join("notes.md"), "ignored").unwrap();
    let tasks = bench::load_tasks(dir.path()).unwrap();
    assert_eq!(tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    std::fs::write(dir.path().join("c.toml"), "id = \"c\"\nprompt = \"p\"\n").unwrap();
    assert!(bench::load_tasks(dir.path()).is_err());
}

#[test]
fn copy_tree_skips_target_and_git() {
    let src = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(src.path().join("src")).unwrap();
    std::fs::create_dir_all(src.path().join("target/debug")).unwrap();
    std::fs::create_dir_all(src.path().join(".git")).unwrap();
    std::fs::write(src.path().join("src/lib.rs"), "x").unwrap();
    std::fs::write(src.path().join("target/debug/big"), "x").unwrap();
    let dst = tempfile::tempdir().unwrap();
    bench::copy_tree(src.path(), dst.path()).unwrap();
    assert!(dst.path().join("src/lib.rs").exists());
    assert!(!dst.path().join("target").exists());
    assert!(!dst.path().join(".git").exists());
}

fn task(verify: &str) -> Task {
    Task { id: "t1".into(), prompt: "create done.txt".into(), verify: verify.into() }
}

#[tokio::test]
async fn bench_run_edits_a_fixture_copy_and_verifies() {
    let fixture = tempfile::tempdir().unwrap();
    std::fs::write(fixture.path().join("README"), "fixture").unwrap();
    let mock = Mock::new(vec![
        Response {
            content: vec![ContentBlock::ToolUse {
                id: "1".into(),
                name: "write_file".into(),
                input: json!({"path": "done.txt", "content": "ok"}),
            }],
            stop_reason: "tool_use".into(),
            usage: usage(100, 10),
        },
        Response { content: vec![ContentBlock::Text { text: "done".into() }], stop_reason: "end_turn".into(), usage: usage(120, 5) },
    ]);
    let r = bench::run_one(&mock, &task("grep -q ok done.txt"), fixture.path(), Mode::Naive, &agent_cfg()).await;
    assert!(r.passed, "{:?}", r.error);
    assert_eq!((r.stop, r.turns, r.usage), (Some(Stop::Done), 2, usage(220, 15)));
    assert!(!fixture.path().join("done.txt").exists(), "fixture must stay pristine");
}

#[tokio::test]
async fn bench_run_fails_when_verify_fails() {
    let fixture = tempfile::tempdir().unwrap();
    let mock = Mock::new(vec![Response {
        content: vec![ContentBlock::Text { text: "nothing to do".into() }],
        stop_reason: "end_turn".into(),
        usage: usage(50, 5),
    }]);
    let r = bench::run_one(&mock, &task("grep -q ok done.txt"), fixture.path(), Mode::Ast, &agent_cfg()).await;
    assert!(!r.passed);
    assert!(r.error.is_some());
}

fn run(task: &str, mode: Mode, tokens: u64, cost: Option<f64>, passed: bool) -> BenchRun {
    BenchRun {
        task: task.into(),
        mode,
        stop: Some(Stop::Done),
        turns: 2,
        usage: usage(tokens, 0),
        cost,
        passed,
        error: None,
        seconds: 1.0,
    }
}

#[test]
fn markdown_reports_savings() {
    let runs = vec![
        run("t1", Mode::Naive, 1000, Some(0.10), true),
        run("t1", Mode::Ast, 600, Some(0.05), true),
        run("t2", Mode::Naive, 1000, Some(0.10), true),
        run("t2", Mode::Ast, 400, Some(0.05), false),
    ];
    let md = bench::markdown("m", &runs);
    assert!(md.contains("| naive | 2/2 | 2.0 | 2000 | $0.2000 |"), "{md}");
    assert!(md.contains("| ast | 1/2 | 2.0 | 1000 | $0.1000 |"), "{md}");
    assert!(md.contains("ast vs naive: +50.0% tokens saved, +50.0% cost saved"), "{md}");
    let j = bench::to_json("m", &runs);
    assert_eq!(j["runs"][3]["passed"], json!(false));
    assert_eq!(j["runs"][1]["mode"], json!("ast"));
}

#[test]
fn unpriced_runs_have_no_total_cost() {
    let runs = vec![run("t1", Mode::Ast, 10, Some(0.1), true), run("t2", Mode::Ast, 10, None, true)];
    assert_eq!(bench::totals(&runs, Mode::Ast).cost, None);
}
