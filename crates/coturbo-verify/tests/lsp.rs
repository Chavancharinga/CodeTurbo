use coturbo_verify::lsp::rpc::{path_to_uri, uri_key};
use coturbo_verify::lsp::{LspConfig, LspManager, ServerConfig, apply_edits};
use serde_json::json;
use std::fs;
use std::time::Duration;

const FAKE: &str = env!("CARGO_BIN_EXE_coturbo-fake-lsp");

fn cfg(command: &str, args: &[&str]) -> LspConfig {
    LspConfig {
        servers: vec![ServerConfig {
            name: "fake".into(),
            command: command.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            root_markers: vec!["package.json".into()],
            extensions: vec!["ts".into()],
        }],
        format_on_write: true,
        inline_wait: Duration::from_millis(1500),
        late_wait: Duration::from_secs(5),
        request_timeout: Duration::from_secs(5),
    }
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("package.json"), "{}").unwrap();
    fs::create_dir(dir.path().join("src")).unwrap();
    dir
}

#[tokio::test]
async fn diagnostics_ride_along_with_each_edit() {
    let dir = project();
    let lsp = LspManager::new(dir.path(), cfg(FAKE, &["fast"]));
    fs::write(dir.path().join("src/a.ts"), "let x = 1 // BAD\nWARN here\n").unwrap();
    let n = lsp.after_write("src/a.ts").await;
    assert!(!n.formatted);
    assert!(
        n.text.starts_with("lsp src/a.ts: 1 error(s), 1 warning(s)"),
        "{}",
        n.text
    );
    assert!(
        n.text
            .contains("error[fake-bad] src/a.ts:1:14: found BAD\n    let x = 1 // BAD"),
        "{}",
        n.text
    );
    assert!(
        !n.text.contains("second line"),
        "only the first message line is kept"
    );

    // Same document, new version: the stale publish must not be reused.
    fs::write(dir.path().join("src/a.ts"), "let x = 1\n").unwrap();
    assert_eq!(
        lsp.after_write("src/a.ts").await.text,
        "lsp src/a.ts: clean"
    );
    // Files the server does not handle are left alone.
    fs::write(dir.path().join("notes.md"), "BAD").unwrap();
    assert_eq!(lsp.after_write("notes.md").await.text, "");
}

#[tokio::test]
async fn format_on_write_rewrites_the_file() {
    let dir = project();
    let lsp = LspManager::new(dir.path(), cfg(FAKE, &["fast"]));
    fs::write(dir.path().join("src/a.ts"), "if (x) {\n\treturn 1\n}\n").unwrap();
    let n = lsp.after_write("src/a.ts").await;
    assert!(n.formatted);
    assert_eq!(
        fs::read_to_string(dir.path().join("src/a.ts")).unwrap(),
        "if (x) {\n    return 1\n}\n"
    );
    assert!(n.text.ends_with("clean"), "{}", n.text);
}

#[tokio::test]
async fn slow_servers_deliver_late_and_unversioned_ones_settle() {
    let dir = project();
    let mut c = cfg(FAKE, &["slow", "700"]);
    c.inline_wait = Duration::from_millis(150);
    c.format_on_write = false;
    let lsp = LspManager::new(dir.path(), c);
    fs::write(dir.path().join("src/a.ts"), "BAD\n").unwrap();
    let n = lsp.after_write("src/a.ts").await;
    assert!(n.text.contains("pending"), "{}", n.text);
    assert!(lsp.drain_late().is_empty());
    let mut late = Vec::new();
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        late = lsp.drain_late();
        if !late.is_empty() {
            break;
        }
    }
    assert_eq!(late.len(), 1);
    assert!(
        late[0].starts_with("[late] lsp src/a.ts: 1 error(s)"),
        "{late:?}"
    );
    assert!(lsp.drain_late().is_empty(), "drained once");

    let dir = project();
    let lsp = LspManager::new(dir.path(), cfg(FAKE, &["unversioned"]));
    fs::write(dir.path().join("src/a.ts"), "WARN\n").unwrap();
    assert!(
        lsp.after_write("src/a.ts")
            .await
            .text
            .contains("0 error(s), 1 warning(s)")
    );
}

#[tokio::test]
async fn crashing_or_missing_servers_never_block_edits() {
    let dir = project();
    let lsp = LspManager::new(dir.path(), cfg(FAKE, &["crash"]));
    fs::write(dir.path().join("src/a.ts"), "BAD\n").unwrap();
    let t = std::time::Instant::now();
    let first = lsp.after_write("src/a.ts").await;
    let second = lsp.after_write("src/a.ts").await;
    assert!(
        t.elapsed() < Duration::from_secs(5),
        "took {:?}",
        t.elapsed()
    );
    assert!(
        first.text.contains("pending") || first.text.is_empty(),
        "{}",
        first.text
    );
    assert!(
        second.text.contains("pending") || second.text.is_empty(),
        "{}",
        second.text
    );

    let lsp = LspManager::new(dir.path(), cfg("definitely-not-a-language-server", &[]));
    let n = lsp.after_write("src/a.ts").await;
    assert!(n.text.starts_with("[lsp] fake not available"), "{}", n.text);
    assert_eq!(
        lsp.after_write("src/a.ts").await.text,
        "",
        "reported only once"
    );
}

#[test]
fn text_edits_use_utf16_columns() {
    let text = "aé😀b\nline2\n";
    let edit = json!({"range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}}, "newText": "X"});
    assert_eq!(apply_edits(text, &[edit]), "aéXb\nline2\n");
    let two = [
        json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 4}}, "newText": "LINE"}),
        json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "A"}),
    ];
    assert_eq!(apply_edits(text, &two), "Aé😀b\nLINE2\n");
}

#[test]
fn uris_round_trip_to_comparable_keys() {
    let p = std::env::temp_dir().join("dir with space").join("a.ts");
    let uri = path_to_uri(&p);
    assert!(uri.starts_with("file:///") && uri.contains("%20"), "{uri}");
    assert_eq!(uri_key(&uri), uri_key(&p.to_string_lossy()));
    if cfg!(windows) {
        assert_eq!(
            uri_key("file:///c%3A/Users/X/a.ts"),
            uri_key("C:\\Users\\X\\a.ts")
        );
    }
}
