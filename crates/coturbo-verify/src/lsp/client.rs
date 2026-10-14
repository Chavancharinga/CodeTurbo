//! One language server process: initialize, sync documents, collect
//! `publishDiagnostics`, answer the server's own requests so it never stalls.

use super::rpc::{self, path_to_uri, uri_key};
use crate::diag::{Diag, Level};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::BufReader;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Notify, oneshot};

/// Unversioned publishes count once no newer one arrives for this long.
const SETTLE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
struct Published {
    version: Option<i64>,
    seq: u64,
    at: Instant,
    diags: Vec<Diag>,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<HashMap<i64, oneshot::Sender<Value>>>,
    published: Mutex<HashMap<String, Published>>,
    /// Latest text per document key, to show the offending source line.
    texts: Mutex<HashMap<String, String>>,
    notify: Notify,
    alive: AtomicBool,
    seq: AtomicU64,
}

pub struct Client {
    root: PathBuf,
    stdin: tokio::sync::Mutex<ChildStdin>,
    _child: Child,
    shared: Arc<Shared>,
    next_id: AtomicI64,
    /// Open documents → last version sent.
    docs: Mutex<HashMap<String, i64>>,
    capabilities: Value,
}

fn severity(v: &Value) -> Option<Level> {
    match v.as_i64() {
        Some(1) | None => Some(Level::Error),
        Some(2) => Some(Level::Warning),
        _ => None,
    }
}

fn to_diags(root: &Path, uri: &str, items: &[Value], text: Option<&str>) -> Vec<Diag> {
    let rel = {
        let key = uri_key(uri);
        let root_key = uri_key(&root.to_string_lossy());
        key.strip_prefix(&root_key).map(|r| r.trim_start_matches('/').to_string()).unwrap_or(key)
    };
    let mut out: Vec<Diag> = items
        .iter()
        .filter_map(|d| {
            let level = severity(&d["severity"])?;
            let line = d["range"]["start"]["line"].as_u64().unwrap_or(0) as u32;
            let col = d["range"]["start"]["character"].as_u64().unwrap_or(0) as u32;
            let code = match &d["code"] {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            };
            let source = text.and_then(|t| t.lines().nth(line as usize)).map(str::to_string);
            Some(Diag {
                level,
                path: rel.clone(),
                line: line + 1,
                col: col + 1,
                message: d["message"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string(),
                code,
                source,
            })
        })
        .collect();
    out.dedup();
    out
}

impl Client {
    pub async fn start(command: &str, args: &[String], root: &Path, timeout: Duration) -> Result<Arc<Self>, String> {
        let mut child = Command::new(command)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start `{command}`: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let shared = Arc::new(Shared { alive: AtomicBool::new(true), ..Shared::default() });
        let mut client = Client {
            root: root.to_path_buf(),
            stdin: tokio::sync::Mutex::new(stdin),
            _child: child,
            shared: shared.clone(),
            next_id: AtomicI64::new(1),
            docs: Mutex::new(HashMap::new()),
            capabilities: Value::Null,
        };
        let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let reader_root = root.to_path_buf();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            while let Ok(Some(msg)) = rpc::read_msg(&mut reader).await {
                let method = msg["method"].as_str().map(str::to_string);
                match (method.as_deref(), msg.get("id")) {
                    (None, Some(id)) => {
                        let tx = id.as_i64().and_then(|id| shared.pending.lock().unwrap().remove(&id));
                        if let Some(tx) = tx {
                            let result = if msg.get("error").is_some() { json!({"__error": msg["error"]}) } else { msg["result"].clone() };
                            let _ = tx.send(result);
                        }
                    }
                    (Some(m), Some(id)) => {
                        // Server → client request: answer so the server does not wait.
                        let result = if m == "workspace/configuration" {
                            let n = msg["params"]["items"].as_array().map_or(1, Vec::len);
                            Value::Array(vec![Value::Null; n])
                        } else {
                            Value::Null
                        };
                        let _ = req_tx.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
                    }
                    (Some("textDocument/publishDiagnostics"), None) => {
                        let uri = msg["params"]["uri"].as_str().unwrap_or("");
                        let key = uri_key(uri);
                        let text = shared.texts.lock().unwrap().get(&key).cloned();
                        let items = msg["params"]["diagnostics"].as_array().cloned().unwrap_or_default();
                        let diags = to_diags(&reader_root, uri, &items, text.as_deref());
                        let seq = shared.seq.fetch_add(1, Ordering::SeqCst) + 1;
                        let p = Published { version: msg["params"]["version"].as_i64(), seq, at: Instant::now(), diags };
                        shared.published.lock().unwrap().insert(key, p);
                        shared.notify.notify_waiters();
                    }
                    _ => {}
                }
            }
            shared.alive.store(false, Ordering::SeqCst);
            shared.notify.notify_waiters();
            for (_, tx) in shared.pending.lock().unwrap().drain() {
                let _ = tx.send(json!({"__error": "server exited"}));
            }
        });
        // Replies to server requests share stdin with our own messages: during
        // initialize they are sent from here, afterwards by a forwarding task.
        let init = json!({
            "processId": std::process::id(),
            "rootUri": path_to_uri(root),
            "workspaceFolders": [{"uri": path_to_uri(root), "name": "root"}],
            "clientInfo": {"name": "coturbo"},
            "capabilities": {
                "textDocument": {
                    "publishDiagnostics": {"versionSupport": true},
                    "synchronization": {"didSave": true},
                    "formatting": {}
                },
                "workspace": {"configuration": true, "workspaceFolders": true}
            }
        });
        let caps = {
            let pending = client.request_raw("initialize", init);
            tokio::pin!(pending);
            loop {
                tokio::select! {
                    r = &mut pending => break r,
                    Some(reply) = req_rx.recv() => { let _ = client.send(&reply).await; }
                    _ = tokio::time::sleep(timeout) => return Err(format!("`{command}` did not answer initialize in {}s", timeout.as_secs())),
                }
            }?
        };
        client.capabilities = caps["capabilities"].clone();
        client.notify("initialized", json!({})).await?;
        let client = Arc::new(client);
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            while let Some(reply) = req_rx.recv().await {
                let Some(c) = weak.upgrade() else { break };
                let _ = c.send(&reply).await;
            }
        });
        Ok(client)
    }

    async fn send(&self, msg: &Value) -> Result<(), String> {
        let mut stdin = self.stdin.lock().await;
        rpc::write_msg(&mut *stdin, msg).await.map_err(|e| format!("lsp write failed: {e}"))
    }

    pub fn alive(&self) -> bool {
        self.shared.alive.load(Ordering::SeqCst)
    }

    pub fn capabilities(&self) -> &Value {
        &self.capabilities
    }

    async fn request_raw(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().unwrap().insert(id, tx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).await?;
        let v = rx.await.map_err(|_| "server exited".to_string())?;
        match v.get("__error") {
            Some(e) => Err(format!("{method} failed: {e}")),
            None => Ok(v),
        }
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        tokio::time::timeout(timeout, self.request_raw(method, params))
            .await
            .map_err(|_| format!("{method} timed out"))?
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params})).await
    }

    /// Opens or updates the document with its full text; returns the version
    /// sent and the publish sequence number at that moment.
    pub async fn sync(&self, path: &Path, language_id: &str, text: &str) -> Result<(i64, u64), String> {
        let uri = path_to_uri(path);
        let key = uri_key(&uri);
        self.shared.texts.lock().unwrap().insert(key.clone(), text.to_string());
        let seq = self.shared.seq.load(Ordering::SeqCst);
        let prev = self.docs.lock().unwrap().get(&key).copied();
        let version = prev.map_or(1, |v| v + 1);
        self.docs.lock().unwrap().insert(key, version);
        match prev {
            None => {
                self.notify(
                    "textDocument/didOpen",
                    json!({"textDocument": {"uri": uri, "languageId": language_id, "version": version, "text": text}}),
                )
                .await?
            }
            Some(_) => {
                self.notify(
                    "textDocument/didChange",
                    json!({"textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}),
                )
                .await?
            }
        }
        self.notify("textDocument/didSave", json!({"textDocument": {"uri": uri}, "text": text})).await?;
        Ok((version, seq))
    }

    /// Diagnostics published for `path` at or after `version` (or, for
    /// servers that do not send versions, the first publish after `seq` that
    /// settles). `None` if nothing fresh arrives within `timeout`.
    pub async fn diagnostics(&self, path: &Path, version: i64, seq: u64, timeout: Duration) -> Option<Vec<Diag>> {
        let key = uri_key(&path_to_uri(path));
        let deadline = Instant::now() + timeout;
        loop {
            let notified = self.shared.notify.notified();
            let fresh = self.shared.published.lock().unwrap().get(&key).cloned().filter(|p| match p.version {
                Some(v) => v >= version,
                None => p.seq > seq,
            });
            if let Some(p) = fresh {
                if p.version.is_some() || p.at.elapsed() >= SETTLE {
                    return Some(p.diags);
                }
                // Unversioned: wait for it to settle, unless a newer one lands.
                let wait = SETTLE.saturating_sub(p.at.elapsed());
                let _ = tokio::time::timeout(wait.min(deadline.saturating_duration_since(Instant::now())), notified).await;
                if Instant::now() >= deadline {
                    return Some(p.diags);
                }
                continue;
            }
            if !self.alive() {
                return None;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || tokio::time::timeout(left, notified).await.is_err() {
                return None;
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
