use coturbo_tools::{Mode, Tools};
use serde_json::json;
use std::fs;
use std::path::Path;

const LIB: &str = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";

fn crate_dir(lib: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), lib).unwrap();
    dir
}

fn big_lib() -> String {
    let mut s = String::new();
    for i in 0..30 {
        s.push_str(&format!(
            "pub fn f{i}(x: i32) -> i32 {{\n    let y = x * {i};\n    let z = y.saturating_add(x);\n    if z > 100 {{\n        return z - 100;\n    }}\n    z + 1\n}}\n\n"
        ));
    }
    s
}

fn read(dir: &Path, rel: &str) -> String {
    fs::read_to_string(dir.join(rel)).unwrap()
}

#[test]
fn tool_sets_differ_and_are_sorted() {
    let dir = crate_dir(LIB);
    let names = |m| Tools::new(dir.path(), m).specs().into_iter().map(|s| s.name).collect::<Vec<_>>();
    let naive = names(Mode::Naive);
    let ast = names(Mode::Ast);
    for n in [&naive, &ast] {
        let mut sorted = n.clone();
        sorted.sort();
        assert_eq!(n, &sorted, "tool order must be deterministic for prompt caching");
    }
    assert!(naive.contains(&"edit_file".to_string()));
    assert!(!naive.contains(&"read_symbol".to_string()));
    for t in ["read_symbol", "replace_symbol", "insert_symbol", "workspace_diagnostics", "edit_file"] {
        assert!(ast.contains(&t.to_string()), "ast mode missing {t}");
    }
}

#[tokio::test]
async fn paths_cannot_escape_the_workspace() {
    let dir = crate_dir(LIB);
    let t = Tools::new(dir.path(), Mode::Naive);
    for p in ["../secret.txt", "src/../../x", "C:/Windows/win.ini", "/etc/passwd"] {
        let out = t.call("read_file", &json!({"path": p})).await;
        assert!(out.is_error, "{p} should be rejected: {}", out.content);
    }
    let out = t.call("write_file", &json!({"path": "../evil.rs", "content": "x"})).await;
    assert!(out.is_error);
}

#[tokio::test]
async fn naive_read_write_and_edit() {
    let dir = crate_dir(LIB);
    let t = Tools::new(dir.path(), Mode::Naive);
    let out = t.call("read_file", &json!({"path": "src/lib.rs"})).await;
    assert!(!out.is_error);
    assert!(out.content.contains("a + b"));

    let out = t.call("edit_file", &json!({"path": "src/lib.rs", "old": "a + b", "new": "a - b"})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(read(dir.path(), "src/lib.rs").contains("a - b"));

    let out = t.call("edit_file", &json!({"path": "src/lib.rs", "old": "nope", "new": "x"})).await;
    assert!(out.is_error);

    let out = t.call("write_file", &json!({"path": "src/new/mod.rs", "content": "// hi\n"})).await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(read(dir.path(), "src/new/mod.rs"), "// hi\n");
}

#[tokio::test]
async fn edit_rejects_ambiguous_match() {
    let dir = crate_dir("fn a() { 1 }\nfn b() { 1 }\n");
    let t = Tools::new(dir.path(), Mode::Naive);
    let out = t.call("edit_file", &json!({"path": "src/lib.rs", "old": "{ 1 }", "new": "{ 2 }"})).await;
    assert!(out.is_error);
    assert!(out.content.contains("2"), "should say how many matches: {}", out.content);
}

#[tokio::test]
async fn ast_read_file_returns_skeleton_for_large_rust_files() {
    let dir = crate_dir(&big_lib());
    let naive = Tools::new(dir.path(), Mode::Naive);
    let ast = Tools::new(dir.path(), Mode::Ast);
    let full = naive.call("read_file", &json!({"path": "src/lib.rs"})).await.content;
    let skel = ast.call("read_file", &json!({"path": "src/lib.rs"})).await.content;
    assert!(full.contains("let y = x * 7;"));
    assert!(!skel.contains("let y = x * 7;"));
    assert!(skel.contains("pub fn f7(x: i32) -> i32 { … }"));
    assert!(skel.len() < full.len() / 2);
    let forced = ast.call("read_file", &json!({"path": "src/lib.rs", "full": true})).await.content;
    assert!(forced.contains("let y = x * 7;"));
}

#[tokio::test]
async fn ast_symbol_tools_edit_in_place_and_format() {
    let dir = crate_dir(LIB);
    let t = Tools::new(dir.path(), Mode::Ast);
    let out = t.call("read_symbol", &json!({"path": "src/lib.rs", "symbol": "add"})).await;
    assert!(out.content.contains("a + b"));

    let out = t
        .call("replace_symbol", &json!({"path": "src/lib.rs", "symbol": "add", "code": "pub fn add(a:i32,b:i32)->i32{a.wrapping_add(b)}"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    let src = read(dir.path(), "src/lib.rs");
    assert!(src.contains("a.wrapping_add(b)"));
    assert!(src.contains("pub fn add(a: i32, b: i32) -> i32 {"), "rustfmt should run: {src}");

    let out = t
        .call("insert_symbol", &json!({"path": "src/lib.rs", "after": "add", "code": "pub fn sub(a: i32, b: i32) -> i32 { a - b }"}))
        .await;
    assert!(!out.is_error, "{}", out.content);
    assert!(read(dir.path(), "src/lib.rs").contains("pub fn sub"));

    let out = t.call("replace_symbol", &json!({"path": "src/lib.rs", "symbol": "add", "code": "pub fn add( {"})).await;
    assert!(out.is_error);
    assert!(read(dir.path(), "src/lib.rs").contains("a.wrapping_add(b)"), "file untouched on bad edit");
}

#[tokio::test]
async fn grep_and_list_respect_gitignore() {
    let dir = crate_dir(LIB);
    fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    fs::create_dir_all(dir.path().join("target")).unwrap();
    fs::write(dir.path().join("target/junk.rs"), "pub fn add() {}").unwrap();
    fs::create_dir(dir.path().join(".git")).unwrap();
    let t = Tools::new(dir.path(), Mode::Naive);
    let out = t.call("grep", &json!({"pattern": "fn add"})).await;
    assert!(out.content.contains("src/lib.rs:1:"), "{}", out.content);
    assert!(!out.content.contains("junk"));
    let out = t.call("list_files", &json!({})).await;
    assert!(out.content.contains("src/lib.rs"));
    assert!(!out.content.contains("target"));
}

#[tokio::test]
async fn workspace_diagnostics_is_compact() {
    let dir = crate_dir("pub fn add(a: i32, b: i32) -> i32 {\n    let s: String = a + b;\n    s\n}\n");
    let t = Tools::new(dir.path(), Mode::Ast);
    let out = t.call("workspace_diagnostics", &json!({})).await;
    assert!(out.is_error);
    assert!(out.content.contains("src/lib.rs:2:"), "{}", out.content);
    assert!(out.content.contains("E0308"), "{}", out.content);
    assert!(out.content.len() < 1500, "too long: {}", out.content.len());

    fs::write(dir.path().join("src/lib.rs"), LIB).unwrap();
    let out = t.call("workspace_diagnostics", &json!({})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.starts_with("ok"));
}

#[tokio::test]
async fn bash_runs_in_workspace() {
    let dir = crate_dir(LIB);
    let t = Tools::new(dir.path(), Mode::Naive);
    let out = t.call("bash", &json!({"command": "echo hello-coturbo"})).await;
    assert!(out.content.contains("hello-coturbo"), "{}", out.content);
    let out = t.call("bash", &json!({"command": "exit 3"})).await;
    assert!(out.is_error);
}

#[tokio::test]
async fn unknown_tool_and_bad_input_are_errors() {
    let dir = crate_dir(LIB);
    let t = Tools::new(dir.path(), Mode::Naive);
    assert!(t.call("nope", &json!({})).await.is_error);
    assert!(t.call("read_file", &json!({})).await.is_error);
    assert!(t.call("read_symbol", &json!({"path": "src/lib.rs", "symbol": "add"})).await.is_error, "ast tool not in naive mode");
}

#[test]
fn tiers_are_fixed_per_tool() {
    use coturbo_tools::{Tier, tier};
    assert_eq!(tier("read_file"), Tier::Read);
    assert_eq!(tier("workspace_diagnostics"), Tier::Read);
    assert_eq!(tier("edit_file"), Tier::Write);
    assert_eq!(tier("replace_symbol"), Tier::Write);
    assert_eq!(tier("tasks"), Tier::Read, "the checklist never asks for approval");
    assert_eq!(tier("bash"), Tier::Exec);
    assert_eq!(tier("mcp__anything"), Tier::Exec);
}

#[test]
fn preview_shows_diff_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let diff = tools
        .preview("edit_file", &serde_json::json!({"path": "a.txt", "old": "two", "new": "TWO"}))
        .unwrap()
        .unwrap();
    assert!(diff.contains("-two") && diff.contains("+TWO"), "{diff}");
    assert!(diff.contains("a/a.txt"));
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one\ntwo\nthree\n");

    let new = tools.preview("write_file", &serde_json::json!({"path": "b.txt", "content": "hi\n"})).unwrap().unwrap();
    assert!(new.contains("+hi"));
    assert!(!dir.path().join("b.txt").exists());

    let bad = tools.preview("edit_file", &serde_json::json!({"path": "a.txt", "old": "zzz", "new": "x"})).unwrap();
    assert!(bad.is_err());
    assert!(tools.preview("bash", &serde_json::json!({"command": "ls"})).is_none());
}

fn lsp(root: &Path, servers: Vec<coturbo_verify::lsp::ServerConfig>) -> std::sync::Arc<coturbo_verify::lsp::LspManager> {
    let cfg = coturbo_verify::lsp::LspConfig { servers, ..coturbo_verify::lsp::LspConfig::default() };
    std::sync::Arc::new(coturbo_verify::lsp::LspManager::new(root, cfg))
}

#[tokio::test]
async fn rustfmt_on_write_only_keeps_clean_files_clean() {
    let dir = crate_dir(LIB);
    let t = Tools::new(dir.path(), Mode::Naive).with_lsp(lsp(dir.path(), vec![]));
    let out = t.call("edit_file", &json!({"path": "src/lib.rs", "old": "    a + b", "new": "    a+b"})).await;
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(), LIB, "formatted back");

    // A file that was never rustfmt-clean is not reformatted.
    let messy = "pub fn add(a:i32,b:i32)->i32{a+b}\n";
    fs::write(dir.path().join("src/lib.rs"), messy).unwrap();
    t.call("edit_file", &json!({"path": "src/lib.rs", "old": "a+b", "new": "b+a"})).await;
    assert_eq!(fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(), "pub fn add(a:i32,b:i32)->i32{b+a}\n");

    // Without LSP settings nothing is formatted.
    let plain = Tools::new(dir.path(), Mode::Naive);
    plain.call("write_file", &json!({"path": "src/new.rs", "content": "fn x(){}\n"})).await;
    assert_eq!(fs::read_to_string(dir.path().join("src/new.rs")).unwrap(), "fn x(){}\n");
}

#[tokio::test]
async fn missing_language_server_is_reported_once_in_edit_results() {
    let dir = crate_dir(LIB);
    let server = coturbo_verify::lsp::ServerConfig {
        name: "ra".into(),
        command: "definitely-not-rust-analyzer".into(),
        args: vec![],
        root_markers: vec!["Cargo.toml".into()],
        extensions: vec!["rs".into()],
    };
    let t = Tools::new(dir.path(), Mode::Naive).with_lsp(lsp(dir.path(), vec![server]));
    let first = t.call("write_file", &json!({"path": "src/a.rs", "content": "pub fn a() {}\n"})).await;
    assert!(!first.is_error);
    assert!(first.content.starts_with("wrote src/a.rs"), "{}", first.content);
    assert!(first.content.contains("[lsp] ra not available"), "{}", first.content);
    let second = t.call("write_file", &json!({"path": "src/b.rs", "content": "pub fn b() {}\n"})).await;
    assert!(!second.content.contains("[lsp]"), "{}", second.content);
}

#[tokio::test]
async fn html_written_on_one_line_is_formatted_and_checked() {
    let dir = tempfile::tempdir().unwrap();
    let t = Tools::new(dir.path(), Mode::Naive).with_native_checks(true).with_lsp(lsp(dir.path(), vec![]));
    let page = "<html><body><h1>Hi</h1><p>text</p></body></html>";
    let out = t.call("write_file", &json!({"path": "index.html", "content": page})).await;
    assert!(!out.is_error, "{}", out.content);
    let written = fs::read_to_string(dir.path().join("index.html")).unwrap();
    assert!(written.lines().count() >= 5, "formatted: {written}");
    assert!(out.content.contains("check index.html:") && out.content.contains("doctype"), "{}", out.content);
}

#[tokio::test]
async fn design_brief_locks_a_contract_and_edits_are_inspected() {
    let dir = tempfile::tempdir().unwrap();
    let t = Tools::new(dir.path(), Mode::Ast).with_native_checks(true);
    assert_eq!(coturbo_tools::tier("design_brief"), coturbo_tools::Tier::Write);
    let brief = json!({
        "subject": "padaria de bairro",
        "audience": "vizinhos que encomendam pão",
        "tone": "acolhedor e simples",
        "signature": "o horário da fornada em destaque, como um quadro de giz"
    });
    let out = t.call("design_brief", &brief).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("codeturbo-kit.css") && out.content.contains("Link it in every page"), "{}", out.content);
    assert!(read(dir.path(), "DESIGN.md").contains("quadro de giz"));
    let kit = read(dir.path(), "codeturbo-kit.css");
    assert!(kit.contains(":root {") && kit.contains("--color-primary") && kit.contains(":focus-visible"), "kit file written: {kit}");

    let page = "<html><head><style>body { font-family: Inter; color: #6366f1; }</style></head><body><p>oi</p></body></html>";
    let out = t.call("write_file", &json!({"path": "index.html", "content": page})).await;
    assert!(out.content.contains("design index.html:"), "{}", out.content);
    assert!(out.content.contains("fonte-padrao") && out.content.contains("cor-fora-do-contrato"), "{}", out.content);
    let out = t.call("write_file", &json!({"path": "notes.txt", "content": "#6366f1"})).await;
    assert!(!out.content.contains("design "), "only interface files are inspected");
}

#[tokio::test]
async fn large_typescript_files_are_read_as_skeletons() {
    let dir = tempfile::tempdir().unwrap();
    let mut src = String::from("import { x } from './x';\nexport interface User { id: number; name: string }\n");
    for i in 0..15 {
        src.push_str(&format!("export function f{i}(a: number): number {{\n  const b = a * {i};\n  const c = b + 1;\n  return c;\n}}\n"));
    }
    fs::write(dir.path().join("app.ts"), &src).unwrap();
    let t = Tools::new(dir.path(), Mode::Ast);
    let out = t.call("read_file", &json!({"path": "app.ts"})).await;
    assert!(out.content.starts_with("// skeleton of app.ts"), "{}", out.content);
    assert!(out.content.contains("export function f3(a: number): number { … }"), "{}", out.content);
    assert!(out.content.contains("export interface User"), "types stay");
    assert!(!out.content.contains("const b"), "bodies elided");
    assert!(out.content.len() < src.len() * 2 / 3, "{} vs {}", out.content.len(), src.len());
    let full = t.call("read_file", &json!({"path": "app.ts", "full": true})).await;
    assert!(full.content.contains("const b = a * 3"));
}

#[tokio::test]
async fn overwriting_an_unread_or_changed_file_warns() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "original\n").unwrap();
    let t = Tools::new(dir.path(), Mode::Naive);
    // Never read: overwriting warns of a possible clobber.
    let out = t.call("write_file", &json!({"path": "a.txt", "content": "novo\n"})).await;
    assert!(out.content.contains("conflito"), "unread overwrite warns: {}", out.content);
    // Now it was just written (recorded): overwriting again is clean.
    let out = t.call("write_file", &json!({"path": "a.txt", "content": "novo2\n"})).await;
    assert!(!out.content.contains("conflito"), "known file, no warning: {}", out.content);
    // External change after our write: next overwrite warns again.
    fs::write(dir.path().join("a.txt"), "mexido por fora\n").unwrap();
    let out = t.call("write_file", &json!({"path": "a.txt", "content": "novo3\n"})).await;
    assert!(out.content.contains("conflito"), "external change warns: {}", out.content);
    // Reading refreshes the record: then overwriting is clean.
    t.call("read_file", &json!({"path": "a.txt"})).await;
    fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    // changed again after read -> warns
    let out = t.call("write_file", &json!({"path": "a.txt", "content": "y\n"})).await;
    assert!(out.content.contains("conflito"), "changed after read warns: {}", out.content);
    // A brand-new file never warns.
    let out = t.call("write_file", &json!({"path": "novo.txt", "content": "z"})).await;
    assert!(!out.content.contains("conflito"), "new file is fine: {}", out.content);
}

#[tokio::test]
async fn the_checklist_is_checked_and_acknowledged() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let t = Tools::new(dir.path(), Mode::Naive);
    assert!(t.specs().iter().any(|s| s.name == "tasks"), "offered to the model");
    let ok = t
        .call("tasks", &json!({"items": [
            {"title": "criar o projeto", "status": "done"},
            {"title": "escrever os testes", "status": "doing"},
            {"title": "rodar a verificação", "status": "pending"}
        ]}))
        .await;
    assert!(!ok.is_error && ok.content == "checklist updated: 1/3 done", "{ok:?}");
    for bad in [
        json!({}),
        json!({"items": []}),
        json!({"items": [{"title": "x", "status": "finished"}]}),
        json!({"items": [{"title": "  ", "status": "done"}]}),
        json!({"items": (0..13).map(|i| json!({"title": format!("passo {i}"), "status": "pending"})).collect::<Vec<_>>()}),
    ] {
        assert!(t.call("tasks", &bad).await.is_error, "{bad}");
    }
    // It never touches the disk.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
