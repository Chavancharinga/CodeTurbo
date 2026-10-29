//! WiFi sensing in a window of its own on a desktop: a 3D view of the link
//! and what disturbs it, served from this process and opened in the
//! browser's app mode (maximized, but it can be minimized and dragged).
//! On a phone (Termux) the same view opens in the phone's browser, at a
//! fixed address, while the terminal view stays; elsewhere without a
//! browser, the terminal view only.

use coturbo_net::sense::{self, Event, Reading};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PAGE: &str = include_str!("sense_window.html");
/// three.js 0.160.0 (MIT), served with the page so the window works offline.
const THREE: &str = include_str!("three.module.min.js");

struct State {
    reading: Option<serde_json::Value>,
    ended: bool,
    /// When the page last asked for a reading.
    polled: Option<Instant>,
    map_file: std::path::PathBuf,
    /// Whether the page going away ends the run: a desktop window, not a
    /// phone's browser, which the user leaves and reloads freely.
    closing_stops: bool,
}

impl State {
    fn new(map_file: std::path::PathBuf) -> Self {
        State { reading: None, ended: false, polled: None, map_file, closing_stops: true }
    }
}

fn reading_json(r: &Reading) -> serde_json::Value {
    json!({
        "source": r.source,
        "csi": r.csi,
        "rate_hz": r.rate_hz,
        "seconds": r.seconds,
        "level": r.level,
        "motion": r.motion,
        "presence": format!("{:?}", r.presence).to_lowercase(),
        "breathing": r.breathing.map(|(bpm, c)| [bpm, c]),
        "motion_history": r.motion_history,
        "signal_history": r.signal_history,
        "fingerprint": r.fingerprint.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(),
    })
}

/// Where the room map lives: the spots the user stood at and what the WiFi
/// heard there, kept between runs.
fn map_file() -> std::path::PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("codeturbo").join("sense-map.json")
}

/// The whole request, the body read up to its Content-Length (64 KiB at most).
async fn request(sock: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(2), sock.read(&mut chunk)).await.ok()?.ok()?;
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some(end) = text.find("\r\n\r\n") {
            let len = text[..end].lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok())).unwrap_or(0);
            if buf.len() >= end + 4 + len || n == 0 || buf.len() > 65536 {
                return Some((text[..end].to_string(), text[end + 4..].to_string()));
            }
        } else if n == 0 || buf.len() > 65536 {
            return None;
        }
    }
}

/// Answers one request: the page, the latest reading, or a stop.
async fn answer(mut sock: tokio::net::TcpStream, state: Arc<Mutex<State>>) {
    let Some((head, sent)) = request(&mut sock).await else { return };
    let mut words = head.split_whitespace();
    let (method, path) = (words.next().unwrap_or("GET"), words.next().unwrap_or("/"));
    let map = state.lock().map(|s| s.map_file.clone()).unwrap_or_else(|_| map_file());
    let (kind, body) = match path {
        "/map" if method == "POST" => {
            let ok = serde_json::from_str::<serde_json::Value>(&sent).is_ok() && map.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&map, &sent).is_ok();
            ("application/json", json!({ "saved": ok }).to_string())
        }
        "/map" => ("application/json", std::fs::read_to_string(&map).ok().filter(|t| serde_json::from_str::<serde_json::Value>(t).is_ok()).unwrap_or_else(|| "{\"points\":[]}".to_string())),
        "/" => ("text/html; charset=utf-8", PAGE.to_string()),
        "/three.module.js" => ("text/javascript; charset=utf-8", THREE.to_string()),
        "/reading" => {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            s.polled = Some(Instant::now());
            ("application/json", json!({ "reading": s.reading, "ended": s.ended }).to_string())
        }
        "/stop" | "/closed" => {
            if path == "/stop" || state.lock().map(|s| s.closing_stops).unwrap_or(true) {
                sense::stop();
            }
            ("application/json", "{}".to_string())
        }
        _ => ("text/plain", "not found".to_string()),
    };
    let status = if body == "not found" { "404 Not Found" } else { "200 OK" };
    let msg = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = sock.write_all(msg.as_bytes()).await;
}

/// The address a phone's browser can keep: this port when it is free.
const PHONE_PORT: u16 = 5006;

/// Serves the view on `port` (0 for any free one); the task ends with the listener.
async fn serve(state: Arc<Mutex<State>>, port: u16) -> std::io::Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(_) => tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?,
    };
    let port = listener.local_addr()?.port();
    let task = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(answer(sock, state.clone()));
        }
    });
    Ok((port, task))
}

/// A desktop that can show a window: not a phone, and on Linux a display.
fn desktop() -> bool {
    if cfg!(target_os = "android") || std::env::var_os("TERMUX_VERSION").is_some() || std::env::var_os("CODETURBO_NO_WINDOW").is_some() {
        return false;
    }
    cfg!(any(windows, target_os = "macos")) || std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// Termux on Android: the view opens in the phone's browser.
fn phone() -> bool {
    std::env::var_os("CODETURBO_NO_WINDOW").is_none() && (cfg!(target_os = "android") || std::env::var_os("TERMUX_VERSION").is_some())
}

/// Opens the view in the phone's browser. Termux's own `am` first: it runs
/// in the Termux app, which is on screen, so Android lets it open another
/// app; Termux:API's opener runs in the background, where Android 10 and
/// later may drop the request without a word.
fn open_on_phone(port: u16) {
    let url = format!("http://127.0.0.1:{port}/");
    let said = |c: &mut std::process::Command| {
        c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    };
    // `am` says "Error" on its output when the activity did not start.
    let opened = said(std::process::Command::new("am").args(["start", "-a", "android.intent.action.VIEW", "-d", &url])).is_some_and(|out| !out.contains("Error"));
    if !opened {
        said(std::process::Command::new("termux-open-url").arg(&url));
    }
}

fn launch(port: u16) -> Option<std::process::Child> {
    let browser = coturbo_design::render::find_browser()?;
    // A profile of its own, so the window is a process of its own that ends when it closes.
    let profile = std::env::temp_dir().join("codeturbo-sense-window");
    std::process::Command::new(browser)
        .arg(format!("--app=http://127.0.0.1:{port}/"))
        .arg("--start-maximized")
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(["--no-first-run", "--no-default-browser-check", "--new-window"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

/// Runs a sensing measurement like [`sense::run`], also showing it in a 3D
/// window on a desktop. Closing the window stops the measurement; the end
/// of the measurement closes the window.
pub async fn run(seconds: Option<f64>, on: &(dyn Fn(Event) + Send + Sync)) -> Result<Reading, String> {
    let phone = phone();
    if !desktop() && !phone {
        return sense::run(seconds, on).await;
    }
    let state = Arc::new(Mutex::new(State { closing_stops: !phone, ..State::new(map_file()) }));
    let Ok((port, server)) = serve(state.clone(), if phone { PHONE_PORT } else { 0 }).await else { return sense::run(seconds, on).await };
    // On a phone the browser is another app the user switches to and from,
    // so only the terminal view ends the run; a desktop window ends it too.
    let mut child = if phone {
        tokio::task::spawn_blocking(move || open_on_phone(port));
        None
    } else {
        launch(port)
    };
    let watch = {
        let state = state.clone();
        tokio::spawn(async move {
            let mut open = child.is_some();
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                // The window closed: its process is gone and the page went quiet.
                let exited = child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))));
                let quiet = state.lock().map(|s| s.polled.is_none_or(|t| t.elapsed() > Duration::from_secs(3))).unwrap_or(true);
                if open && exited && quiet {
                    open = false;
                    sense::stop();
                }
                if state.lock().map(|s| s.ended).unwrap_or(true) {
                    if let Some(c) = child.as_mut() {
                        let _ = c.kill();
                        let _ = c.wait();
                    }
                    return;
                }
            }
        })
    };
    let feed = |mut e: Event| {
        if let Ok(mut s) = state.lock() {
            match &e {
                Event::Reading(r) => s.reading = Some(reading_json(r)),
                Event::End => s.ended = true,
                Event::Start => {}
            }
        }
        // The terminal says where the view is, for a browser that did not open.
        if phone && let Event::Reading(r) = &mut e {
            r.source = format!("3D view: 127.0.0.1:{port} · {}", r.source);
        }
        on(e);
    };
    let out = sense::run(seconds, &feed).await;
    if let Ok(mut s) = state.lock() {
        s.ended = true;
    }
    let _ = watch.await;
    server.abort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(port: u16, path: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn the_window_gets_the_page_and_the_latest_reading() {
        let map = std::env::temp_dir().join(format!("codeturbo-sense-map-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&map);
        let state = Arc::new(Mutex::new(State::new(map.clone())));
        let (port, server) = serve(state.clone(), 0).await.unwrap();
        let page = get(port, "/").await;
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("<canvas id=\"scene\""));
        assert!(get(port, "/reading").await.ends_with("{\"ended\":false,\"reading\":null}"));

        let r = Reading { source: "this computer's WiFi".into(), csi: false, rate_hz: 2.0, seconds: 9.0, level: Some(-45.0), motion: 0.4, presence: sense::Presence::Moving, breathing: None, motion_history: vec![0.1, 0.4], signal_history: vec![-45.0, -47.0], fingerprint: vec![("link".into(), -45.0), ("02:02:02:02:02:02".into(), -57.0)] };
        state.lock().unwrap().reading = Some(reading_json(&r));
        let body = get(port, "/reading").await;
        let j: serde_json::Value = serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(j["reading"]["presence"], "moving");
        assert_eq!(j["reading"]["level"], -45.0);
        assert_eq!(j["reading"]["fingerprint"]["02:02:02:02:02:02"], -57.0);
        assert!(state.lock().unwrap().polled.is_some());
        assert!(get(port, "/three.module.js").await.contains("text/javascript"));
        assert!(get(port, "/nope").await.starts_with("HTTP/1.1 404"));

        // The room map: empty at first, then what the page saved.
        assert!(get(port, "/map").await.ends_with("{\"points\":[]}"));
        let saved = r#"{"points":[{"x":1.5,"z":-2,"fp":{"link":-44}}]}"#;
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("POST /map HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{saved}", saved.len()).as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        assert!(out.ends_with("{\"saved\":true}"), "{out}");
        assert!(get(port, "/map").await.ends_with(saved));
        let _ = std::fs::remove_file(&map);
        server.abort();
    }
}
