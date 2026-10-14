//! The interface's session end to end, without a terminal: a fake Ollama
//! server answers model lists and chat requests, a fake screen collects
//! what would be shown and answers questions.

use coturbo_cli::config::Config;
use coturbo_cli::tui::session::{Channels, Launch, drive};
use coturbo_cli::tui::{In, Out, Question};
use coturbo_providers::PriceTable;
use coturbo_tools::Mode;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Answers `GET /api/tags` and streams a chat reply for every
/// `POST /v1/chat/completions`; a prompt containing `espere` never gets one.
async fn fake_ollama() -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let chats = Arc::new(AtomicUsize::new(0));
    let count = chats.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let count = count.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body_start) = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break (String::from_utf8_lossy(&buf[..i]).to_string(), i + 4);
                    }
                };
                let len = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                    .unwrap_or(0usize);
                while buf.len() < body_start + len {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body = String::from_utf8_lossy(&buf[body_start..]).to_string();
                let response = if head.starts_with("GET /api/tags") {
                    let json = r#"{"models":[{"name":"fake:1b"},{"name":"outro:2b"}]}"#;
                    format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}", json.len())
                } else if head.starts_with("POST /v1/chat/completions") {
                    count.fetch_add(1, Ordering::SeqCst);
                    if body.contains("espere") {
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    let events = [
                        r#"{"choices":[{"index":0,"delta":{"content":"Olá do "}}]}"#,
                        r#"{"choices":[{"index":0,"delta":{"content":"modelo."}}]}"#,
                        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
                        r#"{"choices":[],"usage":{"prompt_tokens":120,"completion_tokens":8}}"#,
                        "[DONE]",
                    ];
                    let sse: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
                    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse}")
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                };
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (url, chats)
}

/// What the fake screen saw.
#[derive(Default)]
struct Seen {
    lines: Vec<String>,
    usage: String,
    titles: Vec<String>,
}

impl Seen {
    fn has(&self, needle: &str) -> bool {
        self.lines.iter().any(|l| l.contains(needle))
    }
}

/// Collects the conversation as text and answers every list with
/// `pick` (or the first option) and every confirmation with yes.
fn fake_screen(out: std::sync::mpsc::Receiver<Out>, pick: Arc<Mutex<Option<String>>>) -> Arc<Mutex<Seen>> {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s = seen.clone();
    std::thread::spawn(move || {
        let mut stream = String::new();
        while let Ok(m) = out.recv() {
            let mut seen = s.lock().unwrap();
            match m {
                Out::Lines(lines) => {
                    if !stream.is_empty() {
                        seen.lines.push(std::mem::take(&mut stream));
                    }
                    seen.lines.extend(lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()));
                }
                Out::Stream(_, delta) => stream.push_str(&delta),
                Out::Usage(u) => seen.usage = u,
                Out::Status(st) => seen.usage = st.usage,
                Out::Ask(q) => match q {
                    Question::Select { title, options, reply, .. } => {
                        seen.titles.push(title);
                        let choice = pick.lock().unwrap().take().or_else(|| options.first().map(|c| c.value.clone()));
                        let _ = reply.send(choice);
                    }
                    Question::Confirm { reply, .. } => {
                        let _ = reply.send(true);
                    }
                    Question::Approve { reply, .. } => {
                        let _ = reply.send(coturbo_agent::Decision::Once);
                    }
                    Question::Secret { reply, .. } => {
                        let _ = reply.send(Some("chave-teste".into()));
                    }
                    Question::Plan { reply, .. } => {
                        let _ = reply.send(coturbo_agent::plan::PlanChoice::SaveQuit);
                    }
                },
                _ => {}
            }
        }
    });
    seen
}

async fn wait_for(seen: &Arc<Mutex<Seen>>, needle: &str) {
    for _ in 0..200 {
        if seen.lock().unwrap().has(needle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("never shown: {needle}\n{}", seen.lock().unwrap().lines.join("\n"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_runs_tasks_switches_models_and_quits() {
    let (url, chats) = fake_ollama().await;
    let project = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();
    let cfg = Config {
        provider: "ollama".into(),
        model: "fake:1b".into(),
        base_url: Some(url),
        final_gate: false,
        diagnostics_on_write: false,
        format_on_write: false,
        ..Config::default()
    };
    let launch = Launch {
        cfg,
        prices: PriceTable::builtin(),
        dir: Some(config_dir.path().to_path_buf()),
        root: project.path().to_path_buf(),
        mode: Mode::Ast,
        plan: false,
        verbose: false,
        prompt: Some("diga olá".into()),
        update: None,
    };
    let (out, out_rx) = std::sync::mpsc::channel();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let (cancel_tx, cancel) = tokio::sync::mpsc::unbounded_channel();
    let pick = Arc::new(Mutex::new(None));
    let seen = fake_screen(out_rx, pick.clone());
    let session = tokio::spawn(drive(launch, Channels { out, rx, cancel, closing: Arc::new(AtomicBool::new(false)) }));

    // The task given at launch runs first, shown as if typed.
    wait_for(&seen, "› diga olá").await;
    wait_for(&seen, "Olá do modelo.").await;
    wait_for(&seen, "concluído em 1 rodada").await;
    assert!(seen.lock().unwrap().usage.contains("↑120"), "{}", seen.lock().unwrap().usage);

    // A request that never answers is interrupted, and the next one works.
    tx.send(In::Submit("espere para sempre".into())).unwrap();
    for _ in 0..200 {
        if chats.load(Ordering::SeqCst) == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    cancel_tx.send(()).unwrap();
    wait_for(&seen, "tarefa interrompida").await;

    // `/modelo` lists the server's models; the pick is saved.
    *pick.lock().unwrap() = Some("outro:2b".into());
    tx.send(In::Submit("/modelo".into())).unwrap();
    wait_for(&seen, "Modelo: outro:2b").await;
    assert!(seen.lock().unwrap().titles.iter().any(|t| t.contains("Modelo · ") && t.contains("digite para pesquisar")));
    let saved = std::fs::read_to_string(config_dir.path().join("config.toml")).unwrap();
    assert!(saved.contains("model = \"outro:2b\""), "{saved}");

    tx.send(In::Submit("de novo".into())).unwrap();
    for _ in 0..200 {
        if seen.lock().unwrap().lines.iter().filter(|l| l.contains("concluído em 1 rodada")).count() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let done: Vec<String> = seen.lock().unwrap().lines.iter().filter(|l| l.contains("concluído")).cloned().collect();
    assert_eq!(done.len(), 2, "{done:?}");
    assert!(done[1].contains("s · tarefa tokens"), "shows how long it took: {}", done[1]);
    let before = chats.load(Ordering::SeqCst);
    tx.send(In::Submit("/repetir".into())).unwrap();
    for _ in 0..200 {
        if chats.load(Ordering::SeqCst) > before {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(chats.load(Ordering::SeqCst), before + 1, "/repetir runs the last task again");
    wait_for(&seen, "› de novo").await;
    tx.send(In::Submit("/uso".into())).unwrap();
    wait_for(&seen, "Sessão: tokens").await;
    tx.send(In::Submit("/inexistente".into())).unwrap();
    wait_for(&seen, "comando desconhecido: /inexistente").await;

    tx.send(In::Submit("/sair".into())).unwrap();
    let used = tokio::time::timeout(Duration::from_secs(5), session).await.unwrap().unwrap().unwrap();
    assert!(used.output >= 16, "both finished tasks are counted: {used:?}");
}
