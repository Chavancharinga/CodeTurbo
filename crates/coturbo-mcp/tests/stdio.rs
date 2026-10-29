//! A real MCP server process, from handshake to a tool call.
use coturbo_mcp::{Manager, Server};
use std::collections::BTreeMap;

#[tokio::test]
async fn a_stdio_server_is_connected_and_its_tool_called() {
    let server = Server::Stdio { command: env!("CARGO_BIN_EXE_echo_server").to_string(), args: vec![], env: BTreeMap::new() };
    let servers = BTreeMap::from([("echo".to_string(), server), ("broken".to_string(), Server::Stdio { command: "no-such-program-here".into(), args: vec![], env: BTreeMap::new() })]);
    let dir = std::env::temp_dir();
    let m = Manager::start(&servers, &dir).await;
    assert_eq!(m.summary(), vec![("echo".to_string(), 1)]);
    assert_eq!(m.failed.len(), 1, "the missing program is reported, not fatal");
    assert_eq!(m.tools()[0].qualified(), "mcp__echo__echo");
    let out = m.call("mcp__echo__echo", serde_json::json!({"text": "olá"})).await.unwrap().unwrap();
    assert_eq!(out.text, "echo: olá");
    assert!(m.call("read_file", serde_json::json!({})).await.is_none(), "not an MCP tool");
}
