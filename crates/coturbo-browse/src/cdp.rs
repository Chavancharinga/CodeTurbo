//! Conversa com o navegador pelo protocolo de depuração (DevTools): abre o
//! Edge ou o Chrome sem janela, descobre a porta em que ele escuta e troca
//! mensagens por WebSocket. Nada passa pela saída padrão do navegador, então
//! funciona de qualquer shell (PowerShell, Git Bash, serviços).

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// How long one command may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

pub struct Cdp {
    ws: Ws,
    next: u64,
    /// Events that arrived while a command was waiting for its answer.
    events: Vec<Value>,
}

/// A browser started for one session, and what it takes to put it away.
pub struct Launched {
    child: tokio::process::Child,
    profile: PathBuf,
    pub port: u16,
    closed: bool,
}

fn unique_profile() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    std::env::temp_dir().join(format!("codeturbo-browse-{}-{n}", std::process::id()))
}

/// Starts the browser listening on a free port; waits for it to say which.
/// With `visible` it is a real window the user can watch; otherwise it has no window.
pub async fn launch(browser: &Path, size: (u32, u32), visible: bool) -> Result<Launched, String> {
    let profile = unique_profile();
    let mut cmd = tokio::process::Command::new(browser);
    cmd.args(["--no-first-run", "--no-default-browser-check", "--allow-file-access-from-files", "--remote-debugging-port=0"]);
    if visible {
        // A small window of its own, away from the user's profile and tabs.
        cmd.args(["--new-window", "--disable-features=msEdgeWelcomePage,msEdgeSplitWindow", "--disable-session-crashed-bubble"]);
    } else {
        cmd.args(["--headless=new", "--disable-gpu"]);
    }
    // Hosted CI runners (and root) forbid the browser's own sandbox, and on
    // Android (Termux) Chromium only starts without it.
    let android = cfg!(target_os = "android") || std::env::var("PREFIX").is_ok_and(|p| p.contains("com.termux"));
    if (cfg!(target_os = "linux") && std::env::var_os("CI").is_some()) || android {
        cmd.arg("--no-sandbox");
    }
    cmd.arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--window-size={},{}", size.0, size.1))
        .arg("about:blank")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(profile.with_extension("log")).map_or_else(|_| std::process::Stdio::null(), Into::into))
        .kill_on_drop(true);
    let child = cmd.spawn().map_err(|e| format!("could not open the browser: {e}"))?;
    // The browser writes the port it picked to this file.
    let file = profile.join("DevToolsActivePort");
    for _ in 0..150 {
        if let Ok(text) = std::fs::read_to_string(&file)
            && let Some(port) = text.lines().next().and_then(|l| l.trim().parse::<u16>().ok())
        {
            return Ok(Launched { child, profile, port, closed: false });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let log = std::fs::read_to_string(profile.with_extension("log")).unwrap_or_default();
    let _ = std::fs::remove_file(profile.with_extension("log"));
    let tail: Vec<&str> = log.lines().rev().take(4).collect();
    Err(format!("the browser did not report its debugging port in 15 s ({})", tail.into_iter().rev().collect::<Vec<_>>().join(" | ")))
}

impl Launched {
    /// The page's WebSocket address, waiting for the page to exist.
    pub async fn page_socket(&self) -> Result<String, String> {
        let url = format!("http://127.0.0.1:{}/json/list", self.port);
        for _ in 0..100 {
            if let Ok(resp) = reqwest::get(&url).await
                && let Ok(list) = resp.json::<Value>().await
                && let Some(ws) = list.as_array().into_iter().flatten().find(|t| t["type"] == "page").and_then(|t| t["webSocketDebuggerUrl"].as_str())
            {
                return Ok(ws.to_string());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("the browser did not open a page".into())
    }

    /// Closes the browser, politely first, then for sure, and drops its profile.
    pub async fn close(mut self) {
        self.closed = true;
        ask_to_close(self.port).await;
        let _ = self.child.kill().await;
        // The files may stay locked for a moment after the browser exits.
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

/// Asks the browser on `port` to close itself.
async fn ask_to_close(port: u16) {
    let version = tokio::time::timeout(Duration::from_secs(2), reqwest::get(format!("http://127.0.0.1:{port}/json/version"))).await;
    if let Ok(Ok(resp)) = version
        && let Ok(v) = resp.json::<Value>().await
        && let Some(url) = v["webSocketDebuggerUrl"].as_str()
        && let Ok(Ok(mut c)) = tokio::time::timeout(Duration::from_secs(2), Cdp::connect(url)).await
    {
        let _ = tokio::time::timeout(Duration::from_secs(3), c.call("Browser.close", json!({}))).await;
    }
}

/// A browser dropped without being closed (a test that ended, a task that was
/// interrupted, a panic) is closed here, so none is ever left behind.
impl Drop for Launched {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        let (port, profile) = (self.port, self.profile.clone());
        let _ = self.child.start_kill();
        // Off the current runtime, which may be shutting down.
        let worker = std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                rt.block_on(ask_to_close(port));
            }
            for _ in 0..10 {
                if std::fs::remove_dir_all(&profile).is_ok() || !profile.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        let _ = worker.join();
    }
}

impl Cdp {
    pub async fn connect(url: &str) -> Result<Self, String> {
        let (ws, _) = connect_async(url).await.map_err(|e| format!("could not talk to the browser: {e}"))?;
        Ok(Self { ws, next: 0, events: Vec::new() })
    }

    /// Runs a command and waits for its answer; events that arrive meanwhile
    /// are kept for [`Cdp::take_events`].
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let msg = json!({"id": id, "method": method, "params": params}).to_string();
        self.ws.send(Message::Text(msg.into())).await.map_err(|e| e.to_string())?;
        let wait = async {
            while let Some(m) = self.ws.next().await {
                let Message::Text(t) = m.map_err(|e| e.to_string())? else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                if v["id"].as_u64() == Some(id) {
                    return match v.get("error") {
                        Some(e) => Err(format!("{method}: {}", e["message"].as_str().unwrap_or("error"))),
                        None => Ok(v["result"].clone()),
                    };
                }
                if v.get("method").is_some() && self.events.len() < 3000 {
                    self.events.push(v);
                }
            }
            Err("the browser closed the connection".to_string())
        };
        tokio::time::timeout(COMMAND_TIMEOUT, wait).await.map_err(|_| format!("{method}: no answer in {}s", COMMAND_TIMEOUT.as_secs()))?
    }

    /// Reads for up to `ms` milliseconds, keeping whatever events arrive.
    pub async fn listen(&mut self, ms: u64) {
        let _ = tokio::time::timeout(Duration::from_millis(ms), async {
            while let Some(Ok(m)) = self.ws.next().await {
                if let Message::Text(t) = m
                    && let Ok(v) = serde_json::from_str::<Value>(&t)
                    && v.get("method").is_some()
                    && self.events.len() < 3000
                {
                    self.events.push(v);
                }
            }
        })
        .await;
    }

    /// Waits until an event called `name` has arrived (or `secs` pass).
    pub async fn wait_for(&mut self, name: &str, secs: u64) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            if self.events.iter().any(|e| e["method"] == name) {
                return true;
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                return false;
            }
            let step = left.min(Duration::from_millis(200)).as_millis() as u64;
            self.listen(step).await;
        }
    }

    /// Whether an event called `name` that satisfies `pred` has arrived.
    pub fn has_event(&self, name: &str, pred: impl Fn(&Value) -> bool) -> bool {
        self.events.iter().any(|e| e["method"] == name && pred(e))
    }

    /// The events kept so far, emptied.
    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    /// Forgets the events of one kind (before waiting for a new one).
    pub fn forget(&mut self, name: &str) {
        self.events.retain(|e| e["method"] != name);
    }
}
