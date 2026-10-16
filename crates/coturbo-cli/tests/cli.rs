use coturbo_agent::approval::ApprovalConfig;
use coturbo_agent::{AgentConfig, Decision, Stop};
use coturbo_cli::bench::{self, BenchRun, Task};
use coturbo_cli::config::{self, Config, Pricer, parse_mode};
use coturbo_cli::status::{color_diff, human, parse_answer, status_line, tool_line};
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
    AgentConfig { model: "m".into(), max_turns: 5, max_tokens: 1000, effort: None, approval: ApprovalConfig::yolo(), plan_model: None, context_window: 1000, gate: None, max_fix_rounds: 3, vision: false }
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
    assert!(config::make_provider(&c, None).err().unwrap().contains("provedor desconhecido"));
    let c = Config { provider: "ollama".into(), ..Config::default() };
    assert!(config::make_provider(&c, None).is_ok(), "Ollama needs no key");
    let c = Config { provider: "anthropic".into(), ..Config::default() };
    assert!(config::make_provider(&c, None).err().unwrap().contains("/chave"), "says how to give the key");
    assert!(config::make_provider(&c, Some("sk-test".into())).is_ok());
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

/// A check that passes when `done.txt` has "ok" in it, with a tool every
/// system has: `grep` is not on a plain Windows PATH, `findstr` is.
fn check_done_file() -> &'static str {
    if cfg!(windows) { "findstr /c:ok done.txt" } else { "grep -q ok done.txt" }
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
    let r = bench::run_one(&mock, &task(check_done_file()), fixture.path(), Mode::Naive, &agent_cfg()).await;
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
    let r = bench::run_one(&mock, &task(check_done_file()), fixture.path(), Mode::Ast, &agent_cfg()).await;
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
        verified: Some(passed),
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

#[test]
fn approval_answers_parse() {
    assert_eq!(parse_answer("o"), Some(Decision::Once));
    assert_eq!(parse_answer(""), Some(Decision::Once));
    assert_eq!(parse_answer("S\n"), Some(Decision::Session));
    assert_eq!(parse_answer("d"), Some(Decision::Deny(None)));
    assert_eq!(parse_answer("d  use cargo nextest  "), Some(Decision::Deny(Some("use cargo nextest".into()))));
    assert_eq!(parse_answer("maybe"), None);
}

#[test]
fn diff_coloring_keeps_headers_plain() {
    let c = color_diff("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new\n ctx\n");
    assert!(c.starts_with("--- a/x\n+++ b/x\n"));
    assert!(c.contains("\x1b[31m-old\x1b[0m") && c.contains("\x1b[32m+new\x1b[0m"));
}

#[test]
fn approval_config_comes_from_config_toml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "approval_mode = \"always-ask\"\n[approval]\nbash = \"deny\"\n\"bash:cargo test*\" = \"allow\"\n",
    )
    .unwrap();
    let (c, _) = config::load(Some(dir.path())).unwrap();
    let a = c.approval_config().unwrap();
    assert_eq!(a.mode, coturbo_agent::approval::ApprovalMode::AlwaysAsk);
    let bad = Config { approval_mode: "sometimes".into(), ..Config::default() };
    assert!(bad.approval_config().is_err());
}

#[test]
fn splash_frames_keep_their_shape_and_reveal_the_name() {
    use coturbo_cli::splash;
    let strip = |s: &str| {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, 'm') => esc = false,
                (true, _) => {}
                (false, c) => out.push(c),
            }
        }
        out
    };
    let width = 120;
    let last = splash::frame(splash::frames() - 1, width);
    assert_eq!(last.len(), splash::ROWS);
    let w = strip(&last[0]).chars().count();
    assert!(w <= width && w >= splash::logo_width(), "{w}");
    for i in 0..splash::frames() {
        let f = splash::frame(i, width);
        assert!(f[..splash::ROWS].iter().all(|l| strip(l).chars().count() == w), "frame {i} misaligned");
    }
    // Narrow terminal: the trail shrinks away, the logo stays intact.
    let narrow = splash::frame(0, splash::logo_width() + 2);
    assert_eq!(strip(&narrow[0]).chars().count(), splash::logo_width());

    let mut buf = Vec::new();
    splash::play(&mut buf, false, &std::sync::atomic::AtomicBool::new(true));
    assert_eq!(String::from_utf8(buf).unwrap(), "CodeTurbo\n");
    assert!(strip(&splash::spinner_line(3, "verifying")).ends_with("@ verifying"));
}

#[test]
fn plan_menu_parses_and_respects_full_context() {
    use coturbo_agent::plan::PlanChoice;
    use coturbo_cli::status::{PlanMenu, parse_plan_choice};
    assert_eq!(parse_plan_choice("1", true), Some(PlanMenu::Choice(PlanChoice::Execute)));
    assert_eq!(parse_plan_choice("2\n", true), Some(PlanMenu::Choice(PlanChoice::Compact)));
    assert_eq!(parse_plan_choice("3", true), Some(PlanMenu::Choice(PlanChoice::KeepContext)));
    assert_eq!(parse_plan_choice("3", false), None);
    assert_eq!(parse_plan_choice("4", true), Some(PlanMenu::Refine));
    assert_eq!(parse_plan_choice("5", true), Some(PlanMenu::Choice(PlanChoice::SaveQuit)));
    assert_eq!(parse_plan_choice("e", true), Some(PlanMenu::Edit));
    assert_eq!(parse_plan_choice("9", true), None);
}

#[test]
fn lsp_servers_are_configurable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "[lsp.servers.rust-analyzer]\ncommand = \"/opt/ra\"\nroot_markers = [\"Cargo.toml\"]\nextensions = [\"rs\"]\n\
         [lsp.servers.pyright]\ncommand = \"pyright-langserver\"\nargs = [\"--stdio\"]\nroot_markers = [\"pyproject.toml\"]\nextensions = [\"py\"]\n",
    )
    .unwrap();
    let (c, _) = config::load(Some(dir.path())).unwrap();
    let lsp = c.lsp_config().unwrap();
    let names: Vec<(&str, &str)> = lsp.servers.iter().map(|s| (s.name.as_str(), s.command.as_str())).collect();
    assert_eq!(
        names,
        [("rust-analyzer", "/opt/ra"), ("typescript-language-server", "typescript-language-server"), ("pyright", "pyright-langserver")]
    );
    let off = Config { diagnostics_on_write: false, ..Config::default() };
    let cfg = off.lsp_config().unwrap();
    assert!(cfg.servers.is_empty() && cfg.format_on_write, "format-only");
    let none = Config { diagnostics_on_write: false, format_on_write: false, ..Config::default() };
    assert!(none.lsp_config().is_none());
    assert!(Config::default().gate_config().is_some());
    assert!(Config { final_gate: false, ..Config::default() }.gate_config().is_none());
}

#[test]
fn diffs_show_line_numbers_signs_and_colors() {
    use coturbo_cli::view::{diff_stat, render_diff};
    let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -273,4 +273,4 @@\n     assert!(x);\n-    let d = first(\"a\");\n+    let d = first(\"b\");\n     ok();\n@@ -300,2 +300,3 @@\n a\n+b\n c\n";
    let plain = render_diff(diff, 60, false);
    assert_eq!(
        plain,
        [
            "  273       assert!(x);",
            "  274 -     let d = first(\"a\");",
            "  274 +     let d = first(\"b\");",
            "  275       ok();",
            "      ⋮",
            "  300   a",
            "  301 + b",
            "  302   c",
        ]
    );
    assert_eq!(diff_stat(diff), (2, 1));
    let color = render_diff(diff, 60, true);
    assert!(color[1].starts_with("\x1b[48;2;70;18;24m"), "removed line has a red background");
    assert!(color[2].starts_with("\x1b[48;2;16;58;32m"), "added line has a green background");
    assert!(color[0].starts_with("\x1b[2m"), "context is dimmed");

    let long: String = (0..100).map(|i| format!("+line {i}\n")).collect();
    let rows = render_diff(&format!("@@ -0,0 +1,100 @@\n{long}"), 60, false);
    assert_eq!(rows.len(), 81);
    assert_eq!(rows[80], "      … mais 20 linhas de diff");
}

#[test]
fn turn_line_shows_speed_context_cost_and_limits() {
    use coturbo_cli::view::{limits_summary, turn_line};
    let headers = vec![
        ("anthropic-ratelimit-requests-limit".to_string(), "50".to_string()),
        ("anthropic-ratelimit-requests-remaining".to_string(), "49".to_string()),
        ("anthropic-ratelimit-tokens-limit".to_string(), "80000".to_string()),
        ("anthropic-ratelimit-tokens-remaining".to_string(), "78000".to_string()),
        ("anthropic-ratelimit-tokens-reset".to_string(), "2026-10-14T12:00:00Z".to_string()),
    ];
    let limits = limits_summary(&headers).unwrap();
    assert_eq!(limits, "tokens 78.0k/80.0k · requisições 49/50");
    let openai = vec![
        ("x-ratelimit-limit-tokens".to_string(), "30000".to_string()),
        ("x-ratelimit-remaining-tokens".to_string(), "29000".to_string()),
    ];
    assert_eq!(limits_summary(&openai).unwrap(), "tokens 29.0k/30.0k");
    assert!(limits_summary(&[]).is_none());

    let u = Usage { input: 1_000, cache_write: 0, cache_read: 9_000, output: 400 };
    let line = turn_line(&u, 10.0, 200_000, Some(0.0123), Some(&limits));
    assert_eq!(line, "↑1.0k ⟳9.0k ↓400 · 40 tok/s · contexto 5% de 200.0k · $0.0123 · limites tokens 78.0k/80.0k · requisições 49/50");
}

/// The track (trail, dust and snail) of frames 40..48, one full loop after
/// the intro, at width 120: the snail animation alone, independent of the
/// wordmark beside it.
fn loop_frames(s: &coturbo_cli::splash::Snail) -> String {
    let mut out = String::new();
    for i in 40..48 {
        for line in coturbo_cli::splash::track_rows_with(s, i, 120) {
            out.push_str(&line);
            out.push('\n');
        }
        out.push_str("=====\n");
    }
    out
}

#[test]
fn approved_large_snail_animation_is_pinned() {
    // The approved full-size animation, saved before the small design. If
    // this fails, the large snail changed: restore it, don't re-pin it.
    let pinned = include_str!("fixtures/snail_large_frames.txt").replace("\r\n", "\n");
    assert!(loop_frames(&coturbo_cli::splash::SNAIL_LARGE) == pinned, "the approved large snail animation changed");
}

#[test]
fn small_snail_animation_is_pinned() {
    let pinned = include_str!("fixtures/snail_small_frames.txt").replace("\r\n", "\n");
    assert!(loop_frames(&coturbo_cli::splash::SNAIL_SMALL) == pinned, "the small snail animation changed");
}

#[test]
fn live_logo_redraws_only_the_track() {
    use coturbo_cli::splash;
    let strip = |s: &str| {
        let mut out = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, 'm') => esc = false,
                (true, _) => {}
                (false, c) => out.push(c),
            }
        }
        out
    };
    let full = splash::frame(60, 120);
    let track = splash::track_rows(60, 120);
    assert_eq!(track.len(), splash::ROWS);
    for (f, t) in full.iter().zip(&track) {
        // The track is exactly the tail of the full row: same pixels, no word.
        assert!(strip(f).ends_with(&strip(t)), "{f:?} vs {t:?}");
        assert!(strip(f).chars().count() > strip(t).chars().count());
    }
}

#[test]
fn versions_compare_numerically() {
    use coturbo_cli::update::{is_newer, parse_version};
    assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
    assert_eq!(parse_version("0.10.0-beta.1"), Some((0, 10, 0)));
    assert_eq!(parse_version("garbage"), None);
    assert!(is_newer("v0.10.0", "0.9.9"), "numeric, not lexical");
    assert!(is_newer("v1.0.0", "0.99.0"));
    assert!(!is_newer("v0.1.0", "0.1.0"));
    assert!(!is_newer("v0.0.9", "0.1.0"));
    assert!(!is_newer("nightly", "0.1.0"));
}

#[test]
fn every_supported_platform_has_a_release_asset() {
    use coturbo_cli::update::asset_name_for;
    assert_eq!(asset_name_for("windows", "x86_64").as_deref(), Some("codeturbo-x86_64-pc-windows-msvc.tar.gz"));
    assert_eq!(asset_name_for("windows", "aarch64").as_deref(), Some("codeturbo-x86_64-pc-windows-msvc.tar.gz"));
    assert_eq!(asset_name_for("linux", "x86_64").as_deref(), Some("codeturbo-x86_64-unknown-linux-musl.tar.gz"));
    assert_eq!(asset_name_for("linux", "aarch64").as_deref(), Some("codeturbo-aarch64-unknown-linux-musl.tar.gz"));
    assert_eq!(asset_name_for("macos", "aarch64").as_deref(), Some("codeturbo-aarch64-apple-darwin.tar.gz"));
    assert_eq!(asset_name_for("macos", "x86_64").as_deref(), Some("codeturbo-x86_64-apple-darwin.tar.gz"));
    assert_eq!(asset_name_for("freebsd", "x86_64"), None);
    assert!(coturbo_cli::update::asset_name().is_some(), "this machine is a supported platform");
}

fn release_archive(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    tar.append_data(&mut header, name, bytes).unwrap();
    tar.into_inner().unwrap().finish().unwrap()
}

#[test]
fn the_binary_is_extracted_and_swapped_in() {
    use coturbo_cli::update::{extract_binary, replace_binary};
    let exe_name = if cfg!(windows) { "codeturbo.exe" } else { "codeturbo" };
    let archive = release_archive(exe_name, b"new build");
    assert_eq!(extract_binary(&archive).unwrap(), b"new build");
    assert!(extract_binary(&release_archive("README.md", b"x")).is_err());

    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join(exe_name);
    std::fs::write(&exe, b"old build").unwrap();
    replace_binary(&exe, b"new build").unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"new build");
    let parked = dir.path().join(format!("{exe_name}.old"));
    assert_eq!(std::fs::read(parked).unwrap(), b"old build", "old binary parked for cleanup");
}

#[tokio::test]
async fn a_fresh_cache_answers_without_asking_github() {
    use coturbo_cli::update::{VERSION, check, notice};
    let dir = tempfile::tempdir().unwrap();
    let now = 1_000_000;
    std::fs::write(dir.path().join("update-check.txt"), format!("{} v999.0.0\n", now - 60)).unwrap();
    assert_eq!(check(Some(dir.path().to_path_buf()), now).await.as_deref(), Some("v999.0.0"));
    std::fs::write(dir.path().join("update-check.txt"), format!("{} v{VERSION}\n", now - 60)).unwrap();
    assert_eq!(check(Some(dir.path().to_path_buf()), now).await, None, "same version: no notice");
    let msg = notice("v999.0.0");
    assert!(msg.contains("v999.0.0") && msg.contains("codeturbo update"), "{msg}");
}

#[test]
fn image_capable_models_are_recognized() {
    use coturbo_cli::config::sees_images;
    assert!(sees_images("anthropic", "claude-sonnet-5-5"));
    assert!(sees_images("openai", "gpt-5"));
    assert!(!sees_images("openai", "gpt-3.5-turbo"));
    assert!(sees_images("ollama", "gemma4:31b-cloud"));
    assert!(sees_images("ollama", "qwen2.5vl:7b"));
    assert!(!sees_images("ollama", "qwen3:4b"));
    let cfg = Config { provider: "ollama".into(), model: "qwen3:4b".into(), vision: Some(true), ..Config::default() };
    assert!(config::agent_config(&cfg).unwrap().vision, "the config can force it");
}
