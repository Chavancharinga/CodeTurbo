//! Model Context Protocol client: the external tool servers Claude Code,
//! Cursor and others use (GitHub, Figma, databases, Slack...). Servers run
//! as a local program spoken to over stdin/stdout, or live at an HTTP
//! address (streamable HTTP). Their tools are offered to the model as
//! `mcp__<server>__<tool>`.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

pub const PROTOCOL: &str = "2025-06-18";

/// How to reach one server.
#[derive(Debug, Clone, PartialEq)]
pub enum Server {
    /// A program started here, spoken to over stdin/stdout.
    Stdio { command: String, args: Vec<String>, env: BTreeMap<String, String> },
    /// A server at an address.
    Http { url: String, headers: BTreeMap<String, String> },
}

/// One tool a server offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub server: String,
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl Tool {
    /// The name the model calls it by.
    pub fn qualified(&self) -> String {
        format!("mcp__{}__{}", clean(&self.server), clean(&self.name))
    }
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// What a tool call gave back: text, and images as `(media type, base64)`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    pub text: String,
    pub images: Vec<(String, String)>,
    pub is_error: bool,
}

/// The servers in a project's `.mcp.json` (Claude Code's format) or under
/// `mcpServers` of any JSON.
pub fn servers_from_json(v: &Value) -> BTreeMap<String, Server> {
    let list = v.get("mcpServers").unwrap_or(v);
    let mut out = BTreeMap::new();
    for (name, s) in list.as_object().into_iter().flatten() {
        let map = |k: &str| -> BTreeMap<String, String> {
            s[k].as_object().into_iter().flatten().filter_map(|(a, b)| Some((a.clone(), b.as_str()?.to_string()))).collect()
        };
        if let Some(url) = s["url"].as_str() {
            out.insert(name.clone(), Server::Http { url: url.to_string(), headers: map("headers") });
        } else if let Some(command) = s["command"].as_str() {
            let args = s["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
            out.insert(name.clone(), Server::Stdio { command: command.to_string(), args, env: map("env") });
        }
    }
    out
}

/// `.mcp.json` in the project, if there is one.
pub fn project_servers(root: &Path) -> BTreeMap<String, Server> {
    std::fs::read_to_string(root.join(".mcp.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).map(|v| servers_from_json(&v)).unwrap_or_default()
}

/// Expands `${VAR}` from the environment (keys stay out of config files).
fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let Some(end) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let var = &rest[i + 2..i + end];
        out.push_str(&std::env::var(var).unwrap_or_default());
        rest = &rest[i + end + 1..];
    }
    out.push_str(rest);
    out
}

struct StdioPipes {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

enum Transport {
    Stdio(Box<StdioPipes>),
    Http { client: reqwest::Client, url: String, headers: BTreeMap<String, String>, session: Option<String> },
}

/// A connection to one server.
pub struct Client {
    name: String,
    transport: Mutex<Transport>,
    next: std::sync::atomic::AtomicU64,
}

const TIMEOUT: Duration = Duration::from_secs(60);

impl Client {
    pub async fn connect(name: &str, server: &Server, root: &Path) -> Result<Client, String> {
        let transport = match server {
            Server::Stdio { command, args, env } => {
                let mut cmd = shell_command(&expand(command), &args.iter().map(|a| expand(a)).collect::<Vec<_>>());
                for (k, v) in env {
                    cmd.env(k, expand(v));
                }
                cmd.current_dir(root).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
                let mut child = cmd.spawn().map_err(|e| format!("could not start `{command}`: {e}"))?;
                let stdin = child.stdin.take().ok_or("no stdin")?;
                let stdout = child.stdout.take().ok_or("no stdout")?;
                Transport::Stdio(Box::new(StdioPipes { child, stdin, lines: BufReader::new(stdout).lines() }))
            }
            Server::Http { url, headers } => Transport::Http {
                client: reqwest::Client::builder().timeout(TIMEOUT).build().map_err(|e| e.to_string())?,
                url: expand(url),
                headers: headers.iter().map(|(k, v)| (k.clone(), expand(v))).collect(),
                session: None,
            },
        };
        let c = Client { name: name.to_string(), transport: Mutex::new(transport), next: std::sync::atomic::AtomicU64::new(1) };
        c.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "codeturbo", "version": env!("CARGO_PKG_VERSION")}}),
        )
        .await?;
        c.notify("notifications/initialized").await?;
        Ok(c)
    }

    async fn notify(&self, method: &str) -> Result<(), String> {
        let msg = json!({"jsonrpc": "2.0", "method": method});
        let mut t = self.transport.lock().await;
        match &mut *t {
            Transport::Stdio(p) => p.stdin.write_all(format!("{msg}\n").as_bytes()).await.map_err(|e| e.to_string()),
            Transport::Http { client, url, headers, session } => {
                let mut req = client.post(url.as_str()).header("Accept", "application/json, text/event-stream").json(&msg);
                for (k, v) in headers.iter() {
                    req = req.header(k, v);
                }
                if let Some(s) = session {
                    req = req.header("Mcp-Session-Id", s.as_str());
                }
                req.send().await.map(|_| ()).map_err(|e| e.to_string())
            }
        }
    }

    /// One JSON-RPC request; the result, or the server's error.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut t = self.transport.lock().await;
        let reply = match &mut *t {
            Transport::Stdio(p) => {
                let StdioPipes { child, stdin, lines } = &mut **p;
                stdin.write_all(format!("{msg}\n").as_bytes()).await.map_err(|e| format!("{}: {e}", self.name))?;
                let deadline = tokio::time::Instant::now() + TIMEOUT;
                loop {
                    let line = match tokio::time::timeout_at(deadline, lines.next_line()).await {
                        Err(_) => return Err(format!("{} did not answer {method}", self.name)),
                        Ok(Ok(Some(l))) => l,
                        Ok(_) => {
                            let code = child.try_wait().ok().flatten().map(|s| s.to_string()).unwrap_or_default();
                            return Err(format!("{} stopped {code}", self.name));
                        }
                    };
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    if v["id"].as_u64() == Some(id) {
                        break v;
                    }
                    // A request from the server (e.g. ping): answer politely.
                    if v.get("method").is_some() && v.get("id").is_some() {
                        let pong = json!({"jsonrpc": "2.0", "id": v["id"], "result": {}});
                        let _ = stdin.write_all(format!("{pong}\n").as_bytes()).await;
                    }
                }
            }
            Transport::Http { client, url, headers, session } => {
                let mut req = client.post(url.as_str()).header("Accept", "application/json, text/event-stream").json(&msg);
                for (k, v) in headers.iter() {
                    req = req.header(k, v);
                }
                if let Some(s) = session.as_deref() {
                    req = req.header("Mcp-Session-Id", s);
                }
                let resp = req.send().await.map_err(|e| format!("{}: {e}", self.name))?;
                if let Some(s) = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()) {
                    *session = Some(s.to_string());
                }
                let status = resp.status();
                let kind = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let body = resp.text().await.map_err(|e| e.to_string())?;
                if !status.is_success() {
                    return Err(format!("{} answered {status}: {}", self.name, body.chars().take(300).collect::<String>()));
                }
                if kind.contains("event-stream") { from_sse(&body, id).ok_or_else(|| format!("{} sent no answer", self.name))? } else { serde_json::from_str(&body).map_err(|e| e.to_string())? }
            }
        };
        if let Some(e) = reply.get("error") {
            return Err(format!("{}: {}", self.name, e["message"].as_str().unwrap_or("error")));
        }
        Ok(reply["result"].clone())
    }

    /// Every tool the server offers (all pages).
    pub async fn tools(&self) -> Result<Vec<Tool>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.request("tools/list", params).await?;
            for t in r["tools"].as_array().into_iter().flatten() {
                out.push(Tool {
                    server: self.name.clone(),
                    name: t["name"].as_str().unwrap_or("").to_string(),
                    description: t["description"].as_str().unwrap_or("").to_string(),
                    input_schema: if t["inputSchema"].is_object() { t["inputSchema"].clone() } else { json!({"type": "object"}) },
                });
            }
            cursor = r["nextCursor"].as_str().map(str::to_string);
            if cursor.is_none() {
                return Ok(out);
            }
        }
    }

    pub async fn call(&self, tool: &str, arguments: Value) -> Result<Output, String> {
        let r = self.request("tools/call", json!({"name": tool, "arguments": arguments})).await?;
        let mut out = Output { is_error: r["isError"].as_bool().unwrap_or(false), ..Output::default() };
        for c in r["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("text") => {
                    if !out.text.is_empty() {
                        out.text.push('\n');
                    }
                    out.text.push_str(c["text"].as_str().unwrap_or(""));
                }
                Some("image") => out.images.push((c["mimeType"].as_str().unwrap_or("image/png").to_string(), c["data"].as_str().unwrap_or("").to_string())),
                Some("resource") => {
                    if let Some(t) = c["resource"]["text"].as_str() {
                        out.text.push_str(t);
                    }
                }
                _ => {}
            }
        }
        if out.text.is_empty() && r.get("structuredContent").is_some() {
            out.text = r["structuredContent"].to_string();
        }
        Ok(out)
    }
}

/// On Windows `npx` and friends are `.cmd` scripts: run them through cmd.
fn shell_command(command: &str, args: &[String]) -> tokio::process::Command {
    if cfg!(windows) && !command.contains(['\\', '/']) && !command.ends_with(".exe") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(command).args(args);
        c
    } else {
        let mut c = tokio::process::Command::new(command);
        c.args(args);
        c
    }
}

/// The JSON-RPC answer with `id` inside a server-sent-events body.
pub fn from_sse(body: &str, id: u64) -> Option<Value> {
    body.lines().filter_map(|l| l.strip_prefix("data:")).filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok()).find(|v| v["id"].as_u64() == Some(id))
}

/// All the configured servers, connected in parallel.
#[derive(Default)]
pub struct Manager {
    clients: BTreeMap<String, Arc<Client>>,
    tools: Vec<Tool>,
    /// Servers that did not connect, with why.
    pub failed: Vec<(String, String)>,
}

impl Manager {
    pub async fn start(servers: &BTreeMap<String, Server>, root: &Path) -> Manager {
        let started = futures_util::future::join_all(servers.iter().map(|(name, s)| async move {
            let c = tokio::time::timeout(TIMEOUT, Client::connect(name, s, root)).await.map_err(|_| "did not start in 60 s".to_string()).and_then(|r| r);
            let c = match c {
                Ok(c) => c,
                Err(e) => return Err((name.clone(), e)),
            };
            match c.tools().await {
                Ok(t) => Ok((name.clone(), Arc::new(c), t)),
                Err(e) => Err((name.clone(), e)),
            }
        }))
        .await;
        let mut m = Manager::default();
        for r in started {
            match r {
                Ok((name, c, tools)) => {
                    m.clients.insert(name, c);
                    m.tools.extend(tools);
                }
                Err(f) => m.failed.push(f),
            }
        }
        m
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// A manager that only knows tools, with no server behind them (tests).
    #[doc(hidden)]
    pub fn with_tools(tools: Vec<Tool>) -> Manager {
        Manager { tools, ..Manager::default() }
    }

    /// `(server, tool count)` of the connected servers.
    pub fn summary(&self) -> Vec<(String, usize)> {
        self.clients.keys().map(|s| (s.clone(), self.tools.iter().filter(|t| &t.server == s).count())).collect()
    }

    /// Calls a tool by its qualified name; `None` when it is not an MCP tool.
    pub async fn call(&self, qualified: &str, arguments: Value) -> Option<Result<Output, String>> {
        let tool = self.tools.iter().find(|t| t.qualified() == qualified)?;
        let client = self.clients.get(&tool.server)?;
        Some(client.call(&tool.name, arguments).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_claude_codes_mcp_json() {
        let v = json!({"mcpServers": {
            "github": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"], "env": {"GITHUB_TOKEN": "${GH}"}},
            "figma": {"type": "http", "url": "https://mcp.figma.com/mcp", "headers": {"Authorization": "Bearer x"}}
        }});
        let s = servers_from_json(&v);
        assert!(matches!(&s["github"], Server::Stdio { command, args, .. } if command == "npx" && args.len() == 2));
        assert!(matches!(&s["figma"], Server::Http { url, .. } if url.ends_with("/mcp")));
    }

    #[test]
    fn names_and_answers() {
        let t = Tool { server: "my server".into(), name: "create.issue".into(), description: String::new(), input_schema: json!({}) };
        assert_eq!(t.qualified(), "mcp__my_server__create_issue");
        let sse = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"ok\":true}}\n\n";
        assert_eq!(from_sse(sse, 3).unwrap()["result"]["ok"], true);
        // SAFETY: test-only, single thread for this variable.
        unsafe { std::env::set_var("CT_MCP_TEST", "abc") };
        assert_eq!(expand("Bearer ${CT_MCP_TEST}!"), "Bearer abc!");
    }
}
