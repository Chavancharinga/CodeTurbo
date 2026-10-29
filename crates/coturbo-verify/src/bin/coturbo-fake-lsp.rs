//! Minimal language server used by CoTurbo's tests. Reports an error for
//! every line containing `BAD` and a warning for `WARN`; formatting turns
//! tabs into four spaces. Arguments: `[mode] [delay_ms]`, mode one of
//! `fast` (default), `slow` (waits `delay_ms` before publishing),
//! `unversioned`, `crash` (exits on the first document).

use coturbo_verify::lsp::rpc::{read_msg, write_msg};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::BufReader;

fn diagnostics(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for (word, severity) in [("BAD", 1), ("WARN", 2)] {
            if let Some(col) = line.find(word) {
                out.push(json!({
                    "range": {"start": {"line": i, "character": col}, "end": {"line": i, "character": col + word.len()}},
                    "severity": severity,
                    "code": format!("fake-{}", word.to_lowercase()),
                    "message": format!("found {word}\nsecond line is dropped"),
                }));
            }
        }
    }
    out
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Arguments: [mode] [delay_ms]
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_else(|| "fast".into());
    let delay: u64 = args.get(1).and_then(|d| d.parse().ok()).unwrap_or(0);
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut texts = std::collections::HashMap::<String, String>::new();
    while let Ok(Some(msg)) = read_msg(&mut stdin).await {
        let method = msg["method"].as_str().unwrap_or("");
        match method {
            "initialize" => {
                // A server-side request the client must answer.
                let ask = json!({"jsonrpc": "2.0", "id": "cfg-1", "method": "workspace/configuration", "params": {"items": [{}]}});
                let _ = write_msg(&mut stdout, &ask).await;
                let caps = json!({"capabilities": {"textDocumentSync": 1, "documentFormattingProvider": true}});
                let _ = write_msg(
                    &mut stdout,
                    &json!({"jsonrpc": "2.0", "id": msg["id"], "result": caps}),
                )
                .await;
            }
            "textDocument/didOpen" | "textDocument/didChange" => {
                if mode == "crash" {
                    std::process::exit(1);
                }
                let doc = &msg["params"]["textDocument"];
                let text = if method == "textDocument/didOpen" {
                    doc["text"].as_str()
                } else {
                    msg["params"]["contentChanges"][0]["text"].as_str()
                }
                .unwrap_or("")
                .to_string();
                let uri = doc["uri"].as_str().unwrap_or("").to_string();
                texts.insert(uri.clone(), text.clone());
                if mode == "slow" {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                let mut params = json!({"uri": uri, "diagnostics": diagnostics(&text)});
                if mode != "unversioned" {
                    params["version"] = doc["version"].clone();
                }
                let publish = json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": params});
                let _ = write_msg(&mut stdout, &publish).await;
            }
            "textDocument/formatting" => {
                let uri = msg["params"]["textDocument"]["uri"].as_str().unwrap_or("");
                let text = texts.get(uri).cloned().unwrap_or_default();
                let result = if text.contains('\t') {
                    let lines = text.lines().count() + 1;
                    json!([{
                        "range": {"start": {"line": 0, "character": 0}, "end": {"line": lines, "character": 0}},
                        "newText": text.replace('\t', "    "),
                    }])
                } else {
                    json!([])
                };
                let _ = write_msg(
                    &mut stdout,
                    &json!({"jsonrpc": "2.0", "id": msg["id"], "result": result}),
                )
                .await;
            }
            "shutdown" => {
                let _ = write_msg(
                    &mut stdout,
                    &json!({"jsonrpc": "2.0", "id": msg["id"], "result": null}),
                )
                .await;
            }
            "exit" => break,
            _ => {}
        }
    }
}
