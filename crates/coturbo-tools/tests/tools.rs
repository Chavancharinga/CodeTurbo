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
    for t in ["read_symbol", "replace_symbol", "insert_symbol", "cargo_check", "edit_file"] {
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
async fn cargo_check_is_compact() {
    let dir = crate_dir("pub fn add(a: i32, b: i32) -> i32 {\n    let s: String = a + b;\n    s\n}\n");
    let t = Tools::new(dir.path(), Mode::Ast);
    let out = t.call("cargo_check", &json!({})).await;
    assert!(out.is_error);
    assert!(out.content.contains("src/lib.rs:2:"), "{}", out.content);
    assert!(out.content.contains("E0308"), "{}", out.content);
    assert!(out.content.len() < 1500, "too long: {}", out.content.len());

    fs::write(dir.path().join("src/lib.rs"), LIB).unwrap();
    let out = t.call("cargo_check", &json!({})).await;
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
