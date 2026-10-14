use coturbo_verify::cheat::{CheatKind, compare};
use coturbo_verify::diag::{self, Level};
use coturbo_verify::gate::{self, GateConfig, Step};
use coturbo_verify::{Diag, Snapshot};
use std::fs;
use std::path::Path;

fn msg(level: &str, code: Option<&str>, text: &str, label: &str) -> String {
    serde_json::json!({
        "reason": "compiler-message",
        "message": {
            "level": level,
            "message": text,
            "code": code.map(|c| serde_json::json!({"code": c})),
            "spans": [{
                "is_primary": true, "file_name": "src\\lib.rs", "line_start": 3, "column_start": 9,
                "label": label, "text": [{"text": "    let x = 1;"}]
            }]
        }
    })
    .to_string()
}

#[test]
fn cargo_json_keeps_errors_and_warnings_deduplicated() {
    let out = [
        msg("warning", Some("unused_variables"), "unused variable: `x`", "help: prefix it"),
        msg("warning", Some("unused_variables"), "unused variable: `x`", "help: prefix it"),
        msg("error", Some("E0308"), "mismatched types", "expected i32"),
        r#"{"reason":"compiler-message","message":{"level":"warning","message":"1 warning emitted","spans":[]}}"#.into(),
        "not json".into(),
    ]
    .join("\n");
    let d = diag::cargo_json(&out);
    assert_eq!(d.len(), 2);
    assert_eq!(diag::count(&d, Level::Warning), 1);
    assert_eq!(d[0].path, "src/lib.rs");
    let list = diag::render_list(&d, 20);
    assert!(list.starts_with("error[E0308] src/lib.rs:3:9: mismatched types — expected i32\n    let x = 1;"), "{list}");
    assert!(list.contains("warning[unused_variables]"));
    let one = diag::render_list(&d, 1);
    assert!(one.ends_with("[1 more]"));
}

#[test]
fn test_failures_and_generic_warnings_are_extracted() {
    let out = "running 2 tests\ntest a ... ok\ntest b ... FAILED\n\nfailures:\n\n---- b stdout ----\n\nthread 'b' panicked at src/lib.rs:9:5:\nassertion `left == right` failed\n  left: 1\n right: 2\n";
    let f = diag::cargo_test_failures(out);
    assert!(f.iter().any(|l| l.starts_with("b: thread 'b' panicked at src/lib.rs:9:5")), "{f:?}");
    assert!(f.iter().any(|l| l.contains("left: 1")));
    assert_eq!(diag::cargo_test_failures("test x ... FAILED\n"), vec!["test x ... FAILED"]);

    let w = diag::generic_warnings("npm warn deprecated foo\n  3:7  warning  'x' is unused  no-unused-vars\n✖ 1 problem (0 errors, 1 warning)\nfound 0 warnings\n");
    assert_eq!(w.len(), 2, "{w:?}");
}

#[test]
fn detection_covers_every_toolchain_and_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("Cargo.toml"), "").unwrap();
    fs::write(root.join("package.json"), r#"{"scripts":{"lint":"eslint .","build":"vite build","test":"echo \"Error: no test specified\" && exit 1"}}"#).unwrap();
    fs::write(root.join("tsconfig.json"), "{}").unwrap();
    fs::write(root.join("bun.lock"), "").unwrap();
    fs::write(root.join("go.mod"), "").unwrap();
    let names: Vec<String> = gate::detect(root, &GateConfig::default()).iter().map(|s| s.command()).collect();
    assert_eq!(
        names,
        [
            "cargo build --all-targets --message-format=json",
            "cargo clippy --all-targets --message-format=json -- -D warnings",
            "cargo test",
            "npx tsc --noEmit",
            "bun run lint",
            "bun run build",
            "go build ./...",
            "go vet ./...",
            "go test ./...",
        ]
    );
    let cfg = GateConfig { verify: Some(vec!["make check".into()]), run: Some("./app --help".into()), ..GateConfig::default() };
    let steps = gate::detect(root, &cfg);
    assert_eq!(steps, vec![
        Step::Shell { name: "make check".into(), command: "make check".into(), optional: false },
        Step::Run { command: "./app --help".into() },
    ]);
    assert!(gate::detect(tempfile::tempdir().unwrap().path(), &GateConfig::default()).is_empty());
}

fn crate_dir(lib: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), lib).unwrap();
    dir
}

async fn gate_on(root: &Path) -> gate::GateReport {
    let cfg = GateConfig::default();
    gate::run_gate(root, &gate::detect(root, &cfg), &cfg).await
}

#[tokio::test]
async fn a_warning_fails_the_gate_and_the_fix_passes_it() {
    let dir = crate_dir("pub fn f() -> i32 {\n    let unused = 1;\n    2\n}\n");
    let r = gate_on(dir.path()).await;
    assert!(!r.ok());
    assert_eq!(r.steps.len(), 1, "stops at the first failing step");
    assert_eq!((r.steps[0].name.as_str(), r.steps[0].errors, r.steps[0].warnings), ("build", 0, 1));
    let s = r.summary();
    assert!(s.contains("failed at `build`") && s.contains("unused variable") && s.contains("src/lib.rs:2"), "{s}");

    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn f() -> i32 {\n    2\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn two() {\n        assert_eq!(super::f(), 2);\n    }\n}\n",
    )
    .unwrap();
    let r = gate_on(dir.path()).await;
    assert!(r.ok(), "{}", r.summary());
    assert_eq!(r.steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["build", "clippy", "test"]);
    assert_eq!(r.summary(), "✓ verification passed (build, clippy, test): 0 errors, 0 warnings");
}

#[tokio::test]
async fn failing_test_and_clippy_lint_are_reported() {
    let dir = crate_dir("pub fn f() -> i32 {\n    return 2;\n}\n");
    let r = gate_on(dir.path()).await;
    let last = r.steps.last().unwrap();
    assert_eq!(last.name, "clippy");
    assert!(last.details.contains("needless_return") || last.details.contains("unneeded `return`"), "{}", last.details);

    let dir = crate_dir("pub fn f() -> i32 {\n    3\n}\n\n#[test]\nfn is_two() {\n    assert_eq!(f(), 2);\n}\n");
    let r = gate_on(dir.path()).await;
    let last = r.steps.last().unwrap();
    assert_eq!(last.name, "test");
    assert!(!last.ok && last.details.contains("is_two"), "{}", last.details);
}

#[tokio::test]
async fn optional_missing_tools_are_skipped_and_run_detects_crashes() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = GateConfig::default();
    let steps = [Step::Shell { name: "x".into(), command: "definitely-not-installed-xyz".into(), optional: true }];
    let r = gate::run_gate(dir.path(), &steps, &cfg).await;
    assert!(r.ok() && r.nothing_ran());
    assert!(r.summary().starts_with("verification skipped"));

    let steps = [Step::Shell { name: "x".into(), command: "definitely-not-installed-xyz".into(), optional: false }];
    assert!(!gate::run_gate(dir.path(), &steps, &cfg).await.ok());

    let steps = [Step::Run { command: "echo \"thread 'main' panicked at src/main.rs:1:1\"".into() }];
    let r = gate::run_gate(dir.path(), &steps, &cfg).await;
    assert!(!r.ok(), "a panic message fails the run even with exit 0");

    let steps = [Step::Shell { name: "lint".into(), command: "echo '  3:7  warning  x is unused'".into(), optional: false }];
    let r = gate::run_gate(dir.path(), &steps, &cfg).await;
    assert!(!r.ok() && r.steps[0].warnings == 1);
}

const TESTS: &str = "pub fn f() -> i32 { 2 }\n#[cfg(test)]\nmod t {\n    #[test]\n    fn a() { assert_eq!(super::f(), 2); }\n    #[tokio::test]\n    async fn b() { assert!(super::f() > 0); }\n}\n";

fn kinds(path: &str, before: &str, after: &str) -> Vec<CheatKind> {
    compare(path, before, after).into_iter().map(|f| f.kind).collect()
}

#[test]
fn rust_cheats_are_detected_on_the_syntax_tree() {
    assert!(kinds("src/lib.rs", TESTS, TESTS).is_empty());
    let removed = TESTS.replace("    #[tokio::test]\n    async fn b() { assert!(super::f() > 0); }\n", "");
    assert_eq!(kinds("src/lib.rs", TESTS, &removed), [CheatKind::FewerTests]);
    let ignored = TESTS.replace("    #[test]\n", "    #[test]\n    #[ignore]\n");
    assert_eq!(kinds("src/lib.rs", TESTS, &ignored), [CheatKind::SkippedTest]);
    let allowed = format!("#![allow(dead_code)]\n{TESTS}");
    assert_eq!(kinds("src/lib.rs", TESTS, &allowed), [CheatKind::LintSuppressed]);
    let todo = TESTS.replace("{ 2 }", "{ todo!() }");
    assert_eq!(kinds("src/lib.rs", TESTS, &todo), [CheatKind::Placeholder]);
    let emptied = TESTS.replace("{ assert_eq!(super::f(), 2); }", "{ assert!(true); }");
    assert_eq!(kinds("src/lib.rs", TESTS, &emptied), [CheatKind::EmptiedTest]);
    // Mentions inside strings and comments are not code.
    let quoted = format!("{TESTS}// #[ignore] todo!()\npub const S: &str = \"#[allow(x)] todo!()\";\n");
    assert!(kinds("src/lib.rs", TESTS, &quoted).is_empty());
    // A brand-new file only counts what it adds.
    assert!(kinds("src/new.rs", "", TESTS).is_empty());
    assert!(kinds("README.md", "a", "").is_empty());
}

#[test]
fn js_and_python_cheats_are_detected() {
    let js = "test('a', () => { expect(f()).toBe(2) })\nit('b', () => {})\n";
    assert_eq!(kinds("a.test.ts", js, "test('a', () => { expect(f()).toBe(2) })\n"), [CheatKind::FewerTests]);
    assert_eq!(kinds("a.test.ts", js, &js.replace("it(", "it.skip(")), [CheatKind::SkippedTest]);
    assert_eq!(kinds("a.ts", "let x = 1\n", "// @ts-ignore\nlet x = 1\n"), [CheatKind::LintSuppressed]);
    let py = "def test_a():\n    assert f() == 2\n";
    assert_eq!(kinds("t.py", py, "@pytest.mark.skip\ndef test_a():\n    assert f() == 2\n"), [CheatKind::SkippedTest]);
    assert_eq!(kinds("t.py", py, "def test_a():\n    assert True\n"), [CheatKind::Placeholder]);
}

#[test]
fn snapshot_compares_first_seen_content() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), TESTS).unwrap();
    let mut snap = Snapshot::default();
    snap.record(dir.path(), "src\\lib.rs");
    fs::write(dir.path().join("src/lib.rs"), TESTS.replace("{ 2 }", "{ todo!() }")).unwrap();
    snap.record(dir.path(), "src/lib.rs"); // second record keeps the original
    let f = snap.findings(dir.path());
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].render(), "src/lib.rs: 1 new todo!/unimplemented!");
    let _ = Diag { level: Level::Error, path: String::new(), line: 0, col: 0, message: String::new(), code: None, source: None };
}

#[test]
fn workspace_detects_every_language_with_priorities() {
    use coturbo_verify::workspace;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for f in ["Cargo.toml", "tsconfig.json", "go.mod", "pyproject.toml", "pyrightconfig.json"] {
        fs::write(root.join(f), "").unwrap();
    }
    fs::write(root.join("go.work"), "go 1.22\n\nuse (\n\t./api // service\n\t./lib/\n)\nuse ./tools\n").unwrap();
    let found: Vec<(String, String)> =
        workspace::detect(root).into_iter().map(|c| (c.description, format!("{} {}", c.program, c.args.join(" ")))).collect();
    assert_eq!(
        found,
        [
            ("Rust (cargo check)".into(), "cargo check --all-targets --message-format=json".into()),
            ("TypeScript (tsc)".into(), "npx tsc --noEmit --pretty false".into()),
            ("Go (go build)".into(), "go build ./api/... ./lib/... ./tools/...".into()),
            ("Python (pyright)".into(), "pyright ".into()),
        ]
    );
}

#[test]
fn workspace_render_labels_sections_and_flags_crashes() {
    use coturbo_verify::workspace::{CheckResult, render};
    let ok = CheckResult { description: "Rust (cargo check)".into(), errors: 0, warnings: 0, body: String::new(), failed: false };
    assert_eq!(render(std::slice::from_ref(&ok)), ("ok: 0 errors, 0 warnings".into(), true));
    let crashed = CheckResult {
        description: "TypeScript (tsc)".into(),
        errors: 0,
        warnings: 0,
        body: "checker exited with 1 and printed nothing".into(),
        failed: true,
    };
    let (text, clean) = render(&[ok, crashed]);
    assert!(!clean, "a crashed checker is never clean");
    assert!(text.starts_with("## Rust (cargo check)\nok") && text.contains("## TypeScript (tsc)\nchecker exited"), "{text}");
    assert!(render(&[]).1);
}

#[tokio::test]
async fn workspace_check_runs_cargo_for_real() {
    use coturbo_verify::workspace;
    let dir = crate_dir("pub fn f() -> i32 {\n    let x = 1;\n    2\n}\n");
    let r = workspace::check(dir.path()).await;
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].errors, r[0].warnings), (0, 1));
    let (text, clean) = workspace::render(&r);
    assert!(!clean && text.contains("src/lib.rs:2"), "{text}");
}

#[test]
fn native_checks_cover_every_language_in_process() {
    use coturbo_verify::diag::Level;
    use coturbo_verify::native::check;
    let first = |rel: &str, text: &str| check(rel, text).unwrap().into_iter().next();

    assert_eq!(check("src/lib.rs", "pub fn f() -> i32 { 2 }\n").unwrap(), vec![]);
    let d = first("src/lib.rs", "pub fn f( -> i32 {\n    2\n}\n").expect("rust syntax error");
    assert_eq!((d.level, d.line, d.path.as_str()), (Level::Error, 1, "src/lib.rs"));

    assert!(check("a.tsx", "export const A = () => <div className=\"x\">hi</div>;\n").unwrap().is_empty());
    let d = first("a.ts", "const x: number = 1;\nconst y = ;\n").expect("ts syntax error");
    assert_eq!(d.line, 2);
    assert_eq!(d.source.as_deref(), Some("const y = ;"));
    let d = first("a.js", "let a = 1;\nlet a = 2;\n").expect("semantic: redeclaration");
    assert_eq!(d.line, 2, "{d:?}");

    assert!(check("a.css", ".a { color: red; }\n").unwrap().is_empty());
    let d = first("a.css", ".a {{ }\n").expect("css error (recovered, reported as a warning)");
    assert_eq!((d.level, d.line), (Level::Warning, 1));

    let d = first("package.json", "{\n  \"a\": 1,\n}\n").expect("json trailing comma");
    assert_eq!(d.line, 3);
    let d = first("Cargo.toml", "[package]\nname = \n").expect("toml error");
    assert_eq!(d.line, 2);

    assert!(check("README.md", "# hi").is_none());
}
