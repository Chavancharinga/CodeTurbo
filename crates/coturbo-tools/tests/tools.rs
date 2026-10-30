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
        // Sorted, with tool_search (the list of the tools not loaded yet) last.
        let (last, rest) = n.split_last().unwrap();
        assert_eq!(last, "tool_search");
        let mut sorted = rest.to_vec();
        sorted.sort();
        assert_eq!(rest, &sorted[..], "tool order must be deterministic for prompt caching");
    }
    // Tools for particular jobs stay out of every request until loaded.
    assert!(!ast.contains(&"browse".to_string()) && !ast.contains(&"wifi_sense".to_string()));
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
    assert!(out.content.contains("default-font") && out.content.contains("color-off-contract"), "{}", out.content);
    let out = t.call("write_file", &json!({"path": "notes.txt", "content": "#6366f1"})).await;
    assert!(!out.content.contains("design "), "only interface files are inspected");
}

#[tokio::test]
async fn brand_colours_the_user_never_gave_do_not_switch_the_theme_off() {
    let brief = json!({
        "subject": "infraestrutura de IA e chips para data centers",
        "audience": "times de engenharia",
        "tone": "preciso e técnico",
        "signature": "um painel de telemetria ao vivo",
        "brand_colors": ["#00f0ff", "#7928ca"]
    });
    let dir = tempfile::tempdir().unwrap();
    coturbo_design::remember_request(dir.path(), "Crie um site para uma empresa de tecnologia de ponta.");
    let t = Tools::new(dir.path(), Mode::Ast);
    assert!(!t.call("design_brief", &brief).await.is_error);
    let md = read(dir.path(), "DESIGN.md");
    assert!(md.contains("## Theme:") && !md.to_lowercase().contains("#00f0ff"), "the theme stays: {}", &md[..400.min(md.len())]);

    let dir = tempfile::tempdir().unwrap();
    coturbo_design::remember_request(dir.path(), "Site da empresa, cores da marca #00f0ff e #7928ca.");
    let t = Tools::new(dir.path(), Mode::Ast);
    assert!(!t.call("design_brief", &brief).await.is_error);
    let md = read(dir.path(), "DESIGN.md");
    assert!(!md.contains("## Theme:"), "the user's own colours win");
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
    assert!(out.content.contains("conflict"), "unread overwrite warns: {}", out.content);
    // Now it was just written (recorded): overwriting again is clean.
    let out = t.call("write_file", &json!({"path": "a.txt", "content": "novo2\n"})).await;
    assert!(!out.content.contains("conflict"), "known file, no warning: {}", out.content);
    // External change after our write: next overwrite warns again.
    fs::write(dir.path().join("a.txt"), "mexido por fora\n").unwrap();
    let out = t.call("write_file", &json!({"path": "a.txt", "content": "novo3\n"})).await;
    assert!(out.content.contains("conflict"), "external change warns: {}", out.content);
    // Reading refreshes the record: then overwriting is clean.
    t.call("read_file", &json!({"path": "a.txt"})).await;
    fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    // changed again after read -> warns
    let out = t.call("write_file", &json!({"path": "a.txt", "content": "y\n"})).await;
    assert!(out.content.contains("conflict"), "changed after read warns: {}", out.content);
    // A brand-new file never warns.
    let out = t.call("write_file", &json!({"path": "novo.txt", "content": "z"})).await;
    assert!(!out.content.contains("conflict"), "new file is fine: {}", out.content);
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

#[tokio::test]
async fn browse_drives_a_page_shows_every_step_and_stays_in_the_project() {
    use std::sync::{Arc, Mutex};
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        "<!doctype html><html><head><meta charset=utf-8><title>Teste</title></head><body><nav><a href=\"two.html\">Dois</a></nav>\
         <button onclick=\"document.getElementById('o').textContent='clicado'\">Apertar</button><p id=o></p></body></html>",
    )
    .unwrap();
    std::fs::write(dir.path().join("two.html"), "<!doctype html><title>Página dois</title><h1>Dois</h1>").unwrap();
    let frames: Arc<Mutex<Vec<coturbo_browse::Frame>>> = Arc::default();
    let sink = frames.clone();
    let t = Tools::new(dir.path(), Mode::Naive).with_browse_sink(Arc::new(move |f| sink.lock().unwrap().push(f)));
    assert!(!t.specs().iter().any(|s| s.name == "browse"), "only named until loaded");
    assert!(t.specs().iter().any(|s| s.name == "tool_search" && s.description.contains("browse (drive a real browser")));
    t.call("tool_search", &json!({"query": "select:browse"})).await;
    assert!(t.specs().iter().any(|s| s.name == "browse"));
    assert_eq!(coturbo_tools::tier("browse"), coturbo_tools::Tier::Exec, "it opens a program, so it asks like one");

    // Pages outside the project are refused before any browser opens.
    let outside = t.call("browse", &json!({"actions": [{"do": "open", "url": "../fora.html"}]})).await;
    assert!(outside.is_error && outside.content.contains("inside the project"), "{outside:?}");
    // Bad input is explained.
    let bad = t.call("browse", &json!({"actions": [{"do": "click"}]})).await;
    assert!(bad.is_error && bad.content.contains("action 1"), "{bad:?}");
    if coturbo_design::render::find_browser().is_none() {
        return;
    }
    let out = t
        .call("browse", &json!({"actions": [
            {"do": "open", "url": "index.html"},
            {"do": "click", "text": "Apertar"},
            {"do": "text", "selector": "#o"},
            {"do": "click", "text": "Dois"}
        ]}))
        .await;
    assert!(!out.is_error, "{out:?}");
    assert!(out.content.starts_with("browser session, 4 step(s):"), "{}", out.content);
    assert!(out.content.contains("clicado") && out.content.contains("two.html") && out.content.contains("Nothing wrong was noticed."), "{}", out.content);
    assert_eq!(out.images.len(), 1, "the last picture goes to the model");
    let seen = frames.lock().unwrap();
    assert!(seen.len() >= 5 && seen.iter().any(|f| f.clicked && f.cursor.is_some()), "{} frames for the screen", seen.len());
}

#[tokio::test]
async fn browse_keeps_the_page_between_calls_until_the_session_ends() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        "<!doctype html><meta charset=utf-8><title>Viva</title><input id=n placeholder=Nome><button onclick=\"document.getElementById('o').textContent='Oi, '+document.getElementById('n').value\">Dizer</button><p id=o></p>",
    )
    .unwrap();
    let t = Tools::new(dir.path(), Mode::Naive);
    if coturbo_design::render::find_browser().is_none() {
        return;
    }
    let first = t.call("browse", &json!({"actions": [{"do": "open", "url": "index.html"}, {"do": "fill", "selector": "#n", "value": "Rui"}]})).await;
    assert!(!first.is_error, "{first:?}");
    // The next call does not open anything: the page is already there, with the name in it.
    let second = t.call("browse", &json!({"actions": [{"do": "click", "text": "Dizer"}, {"do": "text", "selector": "#o"}]})).await;
    assert!(!second.is_error && second.content.contains("Oi, Rui"), "{second:?}");
    // The session ends: the browser goes with it, and the next one starts blank.
    t.shutdown().await;
    let third = t.call("browse", &json!({"actions": [{"do": "click", "text": "Dizer"}]})).await;
    assert!(third.content.contains("ERROR: could not find"), "a new browser starts on a blank page: {third:?}");
    t.shutdown().await;
}

#[tokio::test]
async fn browse_windows_are_limited_and_numbered_once() {
    let dir = tempfile::tempdir().unwrap();
    let t = Tools::new(dir.path(), Mode::Naive);
    let one = json!({"actions": [{"do": "wait", "ms": 1}]});
    let seven: Vec<_> = (0..7).map(|i| json!({"window": i, "actions": one["actions"]})).collect();
    let too_many = t.call("browse", &json!({"windows": seven})).await;
    assert!(too_many.is_error && too_many.content.contains("at most 6"), "{too_many:?}");
    let twice = t.call("browse", &json!({"windows": [{"window": 1, "actions": one["actions"]}, {"window": 1, "actions": one["actions"]}]})).await;
    assert!(twice.is_error && twice.content.contains("each is used once"), "{twice:?}");
    let far = t.call("browse", &json!({"window": 9, "actions": one["actions"]})).await;
    assert!(far.is_error && far.content.contains("0 to 5"), "{far:?}");
    // Each window's own mistakes are named by window.
    let bad = t.call("browse", &json!({"windows": [{"actions": [{"do": "click"}]}]})).await;
    assert!(bad.is_error && bad.content.contains("window 1"), "{bad:?}");
}

#[tokio::test]
async fn browse_shows_several_pages_at_once_each_in_its_own_window() {
    use std::sync::{Arc, Mutex};
    let dir = tempfile::tempdir().unwrap();
    for (name, title) in [("a.html", "Alpha"), ("b.html", "Beta"), ("c.html", "Gamma")] {
        std::fs::write(dir.path().join(name), format!("<!doctype html><title>{title}</title><h1>{title}</h1><button>Go</button>")).unwrap();
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let t = Tools::new(dir.path(), Mode::Naive).with_browse_sink(Arc::new(move |f| sink.lock().unwrap().push((f.window, f.url))));
    if coturbo_design::render::find_browser().is_none() {
        return;
    }
    let out = t
        .call("browse", &json!({"windows": [
            {"window": 0, "actions": [{"do": "open", "url": "a.html"}, {"do": "click", "text": "Go"}]},
            {"window": 1, "actions": [{"do": "open", "url": "b.html"}]},
            {"window": 2, "actions": [{"do": "open", "url": "c.html"}]}
        ]}))
        .await;
    assert!(!out.is_error, "{out:?}");
    assert!(out.content.contains("window 0:") && out.content.contains("window 1:") && out.content.contains("window 2:"), "{}", out.content);
    assert_eq!(out.images.len(), 3, "a picture of each page goes to the model");
    {
        let seen = seen.lock().unwrap();
        for (window, page) in [(0, "a.html"), (1, "b.html"), (2, "c.html")] {
            assert!(seen.iter().any(|(w, u)| *w == window && u.ends_with(page)), "window {window} showed {page}: {seen:?}");
        }
    }
    t.shutdown().await;
}

/// A command that never ends (a server) runs in the background: the call
/// comes back in seconds with its first output, and the session's end stops it.
#[tokio::test]
async fn a_server_runs_in_the_background_and_stops_with_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let started = std::time::Instant::now();
    // Long in any shell (cmd or sh).
    let long = if cfg!(windows) { "ping -n 60 127.0.0.1" } else { "sleep 60" };
    let out = tools.call("bash", &serde_json::json!({"command": long, "background": true})).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(started.elapsed() < std::time::Duration::from_secs(15), "came back without waiting for it");
    assert!(out.content.contains("running in the background"), "{}", out.content);
    let pid: u32 = out.content.split("pid ").nth(1).and_then(|s| s.split(')').next()).and_then(|p| p.parse().ok()).expect("a pid");
    tools.shutdown().await;
    // A quick command in background mode is just reported as finished.
    let quick = tools.call("bash", &serde_json::json!({"command": "echo done", "background": true})).await;
    assert!(quick.content.contains("exited") && quick.content.contains("done"), "{}", quick.content);
    assert!(pid > 0);
}

/// Many external tools are only named until the model loads the ones it
/// needs with tool_search: the prompt stays small.
#[test]
fn external_tools_load_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let ext: Vec<coturbo_mcp::Tool> = (0..10)
        .map(|i| coturbo_mcp::Tool { server: "gh".into(), name: format!("tool_{i}"), description: format!("does thing {i}"), input_schema: serde_json::json!({"type": "object"}) })
        .collect();
    let _ = tools.mcp_slot().set(coturbo_mcp::Manager::with_tools(ext));
    let names = |t: &Tools| t.specs().into_iter().map(|s| s.name).collect::<Vec<_>>();
    let before = names(&tools);
    assert!(before.contains(&"tool_search".to_string()));
    assert!(!before.iter().any(|n| n.starts_with("mcp__")), "only named, inside tool_search");
    let search = tools.specs().into_iter().find(|s| s.name == "tool_search").unwrap();
    assert!(search.description.contains("mcp__gh__tool_7"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let out = rt.block_on(tools.call("tool_search", &serde_json::json!({"query": "select:mcp__gh__tool_7"})));
    assert!(!out.is_error, "{}", out.content);
    let after = names(&tools);
    assert!(after.contains(&"mcp__gh__tool_7".to_string()), "loaded now");
    assert!(!after.contains(&"mcp__gh__tool_3".to_string()), "the others still wait");
}

/// What is remembered in one session is in the next one's instructions,
/// until it is forgotten.
#[tokio::test]
async fn memory_lasts_between_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let save = serde_json::json!({"name": "Language", "type": "feedback", "description": "talk to the user in Portuguese", "content": "Always answer in Portuguese.\n**Why:** the user reads Portuguese."});
    assert!(!tools.call("remember", &save).await.is_error);
    let index = coturbo_tools::memory::index(tools.root());
    assert!(index.contains("[feedback] language") && index.contains("talk to the user in Portuguese"), "{index}");
    // A new session of the same project sees it.
    let again = Tools::new(dir.path(), Mode::Naive);
    let full = again.call("recall", &serde_json::json!({"name": "language"})).await;
    assert!(full.content.contains("Always answer in Portuguese."), "{}", full.content);
    assert!(!again.call("forget", &serde_json::json!({"name": "language"})).await.is_error);
    assert!(coturbo_tools::memory::index(tools.root()).is_empty());
    let _ = std::fs::remove_dir_all(coturbo_tools::memory::dir(tools.root()));
}

/// Notebooks come as text and pictures as pictures.
#[tokio::test]
async fn notebooks_and_images_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let nb = r##"{"cells":[{"cell_type":"markdown","source":["# Sales"]},{"cell_type":"code","source":["print(2+2)"],"outputs":[{"text":["4\n"]}]}]}"##;
    std::fs::write(dir.path().join("a.ipynb"), nb).unwrap();
    // A 1x1 PNG.
    let png = [137u8, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 15, 4, 0, 9, 251, 3, 253, 227, 85, 242, 156, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
    std::fs::write(dir.path().join("dot.png"), png).unwrap();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let text = tools.call("read_file", &serde_json::json!({"path": "a.ipynb"})).await.content;
    assert!(text.contains("[markdown]") && text.contains("print(2+2)") && text.contains("# output\n4"), "{text}");
    let img = tools.call("read_file", &serde_json::json!({"path": "dot.png"})).await;
    assert_eq!(img.images.len(), 1);
    assert_eq!(img.images[0].0, "image/png");
}

/// Background jobs can be listed, read and stopped.
#[tokio::test]
async fn background_jobs_are_listed_read_and_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let tools = Tools::new(dir.path(), Mode::Naive);
    let long = if cfg!(windows) { "ping -n 60 127.0.0.1" } else { "sleep 60" };
    let started = tools.call("bash", &serde_json::json!({"command": long, "background": true})).await;
    assert!(started.content.contains("as job 1"), "{}", started.content);
    let list = tools.call("jobs", &serde_json::json!({"action": "list"})).await.content;
    assert!(list.contains("job 1 · running"), "{list}");
    let out = tools.call("jobs", &serde_json::json!({"action": "output", "id": 1})).await.content;
    assert!(out.starts_with("job 1 · still running"), "{out}");
    let stop = tools.call("jobs", &serde_json::json!({"action": "stop", "id": 1})).await;
    assert!(!stop.is_error, "{}", stop.content);
    assert_eq!(tools.call("jobs", &serde_json::json!({"action": "list"})).await.content, "No background jobs.");
}

#[tokio::test]
async fn layout_grid_places_the_elements_on_every_screen() {
    let dir = tempfile::tempdir().unwrap();
    let t = Tools::new(dir.path(), Mode::Ast);
    assert_eq!(coturbo_tools::tier("layout_grid"), coturbo_tools::Tier::Write);
    let page = json!({"elements": [
        {"id": "title", "kind": "title"},
        {"id": "intro", "kind": "text"},
        {"id": "photo", "kind": "image"},
        {"id": "buy", "kind": "button", "new_row": true}
    ]});
    let out = t.call("layout_grid", &page).await;
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("phone 390px: 4 columns") && out.content.contains("desktop 1440px: 16 columns"), "{}", out.content);
    let css = read(dir.path(), "codeturbo-grid.css");
    assert!(css.contains(".layout { display: grid;") && css.contains("#photo {") && css.contains("@media (min-width: 1200px)"), "{css}");
    assert!(read(dir.path(), "layout-preview.html").contains("Layout grid"));
    assert!(t.call("layout_grid", &json!({"elements": []})).await.is_error);
}

#[test]
fn commons_answers_become_free_large_photos() {
    let answer = serde_json::json!({"query": {"pages": {
        "1": {"index": 2, "title": "File:Gel_nails_red.jpg", "imageinfo": [{"mime": "image/jpeg", "width": 3000, "height": 2000,
            "thumburl": "https://upload.wikimedia.org/a/1600px-Gel_nails_red.jpg", "thumbwidth": 1600, "thumbheight": 1067,
            "descriptionurl": "https://commons.wikimedia.org/wiki/File:Gel_nails_red.jpg",
            "extmetadata": {"LicenseShortName": {"value": "CC BY-SA 4.0"}, "Artist": {"value": "<a href=\"x\">Ana</a>"}, "ImageDescription": {"value": "Red <b>gel</b> nails"}}}]},
        "2": {"index": 1, "title": "File:Icon.png", "imageinfo": [{"mime": "image/png", "width": 64, "height": 64, "url": "https://x/icon.png", "extmetadata": {"LicenseShortName": {"value": "CC0"}}}]},
        "3": {"index": 3, "title": "File:Nc.jpg", "imageinfo": [{"mime": "image/jpeg", "width": 2000, "height": 2000, "url": "https://x/nc.jpg", "extmetadata": {"LicenseShortName": {"value": "CC BY-NC 2.0"}}}]}
    }}});
    let found = coturbo_tools::images::parse(&answer);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].alt, "Red gel nails");
    assert_eq!(found[0].author, "Ana");
    assert_eq!((found[0].width, found[0].height), (1600, 1067));
}

#[tokio::test]
#[ignore = "reaches Wikimedia Commons"]
async fn live_find_images() {
    let dir = std::path::PathBuf::from(std::env::var("CT_IMAGES_DIR").unwrap_or_else(|_| "target/live-images".into()));
    std::fs::create_dir_all(&dir).unwrap();
    let t = Tools::new(&dir, Mode::Ast);
    let out = t.call("find_images", &json!({"query": "gel nails", "count": 4})).await;
    println!("{}", out.content);
    assert!(!out.is_error && out.content.contains("assets/images/gel-nails-1"), "{}", out.content);
    assert!(dir.join("assets/images/CREDITS.md").exists());
}

#[test]
fn a_page_that_passes_is_told_to_stop_reviewing() {
    use coturbo_tools::review_advice;
    assert!(review_advice(0).contains("call design_review again"));
    assert!(review_advice(1).contains("finish the task now"));
    assert!(review_advice(2).contains("Do not review"));
    assert!(!review_advice(3).contains("call design_review again"));
}
