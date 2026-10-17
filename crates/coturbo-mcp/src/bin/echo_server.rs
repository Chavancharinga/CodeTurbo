//! A tiny MCP server over stdio, for the tests: one tool, `echo`.
use std::io::{BufRead, Write};

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let Some(id) = v.get("id").cloned() else { continue };
        let result = match v["method"].as_str() {
            Some("initialize") => serde_json::json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "echo", "version": "1"}}),
            Some("tools/list") => serde_json::json!({"tools": [{"name": "echo", "description": "Repeats the text.", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}}]}),
            Some("tools/call") => serde_json::json!({"content": [{"type": "text", "text": format!("echo: {}", v["params"]["arguments"]["text"].as_str().unwrap_or(""))}]}),
            _ => serde_json::json!({}),
        };
        let reply = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result});
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
    }
}
