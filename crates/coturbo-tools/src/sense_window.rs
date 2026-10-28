//! WiFi sensing in a window of its own on a desktop: a 3D view of the link
//! and what disturbs it, served from this process and opened in the
//! browser's app mode (maximized, but it can be minimized and dragged).
//! Phones and machines without a browser keep the terminal view only.

use coturbo_net::sense::{self, Event, Reading};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PAGE: &str = include_str!("sense_window.html");

#[derive(Default)]
struct State {
    reading: Option<serde_json::Value>,
    ended: bool,
    /// When the page last asked for a reading.
    polled: Option<Instant>,
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
    })
}

/// Answers one request: the page, the latest reading, or a stop.
async fn answer(mut sock: tokio::net::TcpStream, state: Arc<Mutex<State>>) {
    let mut buf = [0u8; 2048];
    let Ok(Ok(n)) = tokio::time::timeout(Duration::from_secs(2), sock.read(&mut buf)).await else { return };
    let head = String::from_utf8_lossy(&buf[..n]);
    let path = head.split_whitespace().nth(1).unwrap_or("/");
    let (kind, body) = match path {
        "/" => ("text/html; charset=utf-8", PAGE.to_string()),
        "/reading" => {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            s.polled = Some(Instant::now());
            ("application/json", json!({ "reading": s.reading, "ended": s.ended }).to_string())
        }
        "/stop" | "/closed" => {
            sense::stop();
            ("application/json", "{}".to_string())
        }
        _ => ("text/plain", "not found".to_string()),
    };
    let status = if body == "not found" { "404 Not Found" } else { "200 OK" };
    let msg = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len());
    let _ = sock.write_all(msg.as_bytes()).await;
}

/// Serves the view on a free local port; the task ends with the listener.
async fn serve(state: Arc<Mutex<State>>) -> std::io::Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
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
    if !desktop() {
        return sense::run(seconds, on).await;
    }
    let state = Arc::new(Mutex::new(State::default()));
    let Ok((port, server)) = serve(state.clone()).await else { return sense::run(seconds, on).await };
    let mut child = launch(port);
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
    let feed = |e: Event| {
        if let Ok(mut s) = state.lock() {
            match &e {
                Event::Reading(r) => s.reading = Some(reading_json(r)),
                Event::End => s.ended = true,
                Event::Start => {}
            }
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
        let state = Arc::new(Mutex::new(State::default()));
        let (port, server) = serve(state.clone()).await.unwrap();
        let page = get(port, "/").await;
        assert!(page.starts_with("HTTP/1.1 200") && page.contains("<canvas id=\"scene\""));
        assert!(get(port, "/reading").await.ends_with("{\"ended\":false,\"reading\":null}"));

        let r = Reading { source: "this computer's WiFi".into(), csi: false, rate_hz: 2.0, seconds: 9.0, level: Some(-45.0), motion: 0.4, presence: sense::Presence::Moving, breathing: None, motion_history: vec![0.1, 0.4], signal_history: vec![-45.0, -47.0] };
        state.lock().unwrap().reading = Some(reading_json(&r));
        let body = get(port, "/reading").await;
        let j: serde_json::Value = serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(j["reading"]["presence"], "moving");
        assert_eq!(j["reading"]["level"], -45.0);
        assert!(state.lock().unwrap().polled.is_some());
        assert!(get(port, "/nope").await.starts_with("HTTP/1.1 404"));
        server.abort();
    }
}
