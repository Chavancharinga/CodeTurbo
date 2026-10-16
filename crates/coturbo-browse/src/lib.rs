//! Navegar numa página como uma pessoa: abrir, olhar o que dá para clicar,
//! clicar com o mouse, preencher, apertar teclas, rolar. Usa o Edge ou o
//! Chrome sem janela, pelo protocolo de depuração, e entrega a cada passo uma
//! foto da página com a posição do ponteiro, para a tela mostrar o agente
//! mexendo o mouse. Também aponta o que uma pessoa estranharia: um clique que
//! cairia em outro elemento, erros no console, imagens e arquivos que não
//! carregam.

pub mod action;
mod cdp;
mod script;

pub use action::{Action, Target};

use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

/// A piece of text of the page and where it sits, in page pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    /// The tag it came from (`h1`, `a`, `button`...).
    pub kind: String,
}

/// What the page looks like after a step, and where the pointer is.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// The page, as a PNG.
    pub png: Vec<u8>,
    pub url: String,
    pub title: String,
    /// The pointer, in page pixels, once the agent has moved it.
    pub cursor: Option<(f32, f32)>,
    /// The pointer has just clicked.
    pub clicked: bool,
    /// The window the page was drawn in, in page pixels.
    pub size: (u32, u32),
    /// What the agent is doing.
    pub caption: String,
    /// The texts of the page, to read it in a small picture.
    pub labels: Vec<Label>,
    /// Which of the windows shown together this page is (0 to 5).
    pub window: usize,
}

/// What the whole session found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// One line per step, saying what happened.
    pub log: Vec<String>,
    /// What a person would notice is wrong.
    pub problems: Vec<String>,
    /// Photos the agent asked for (`shot`) and the last state of the page.
    pub shots: Vec<Vec<u8>>,
}

/// An address as the browser needs it: a site as it is, `localhost:3000` with
/// its scheme, a file of the project as a `file://` address.
pub fn resolve_url(root: &Path, s: &str) -> String {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if ["http://", "https://", "file://", "about:", "data:"].iter().any(|p| lower.starts_with(p)) {
        return s.to_string();
    }
    let host_port = s.split(['/', '?', '#']).next().unwrap_or("");
    let looks_like_host = lower.starts_with("localhost") || lower.starts_with("127.0.0.1") || (host_port.contains(':') && host_port.rsplit(':').next().is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) && !host_port.contains('\\'));
    if looks_like_host {
        return format!("http://{s}");
    }
    let given = Path::new(s);
    let mut path = if given.is_absolute() { given.to_path_buf() } else { root.join(given) };
    if path.is_dir() {
        path = path.join("index.html");
    }
    let path = path.canonicalize().unwrap_or(path);
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_start_matches("//?/").replace(' ', "%20");
    if text.starts_with('/') { format!("file://{text}") } else { format!("file:///{text}") }
}

fn decode_png(b64: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(b64).unwrap_or_default()
}

/// Key name to (key, code, virtual key code, text).
fn key_info(k: &str) -> (String, String, u32, String) {
    let named = |key: &str, code: &str, vk: u32, text: &str| (key.to_string(), code.to_string(), vk, text.to_string());
    match k {
        "Enter" | "Return" => named("Enter", "Enter", 13, "\r"),
        "Tab" => named("Tab", "Tab", 9, ""),
        "Escape" | "Esc" => named("Escape", "Escape", 27, ""),
        "Backspace" => named("Backspace", "Backspace", 8, ""),
        "Delete" => named("Delete", "Delete", 46, ""),
        "ArrowDown" | "Down" => named("ArrowDown", "ArrowDown", 40, ""),
        "ArrowUp" | "Up" => named("ArrowUp", "ArrowUp", 38, ""),
        "ArrowLeft" | "Left" => named("ArrowLeft", "ArrowLeft", 37, ""),
        "ArrowRight" | "Right" => named("ArrowRight", "ArrowRight", 39, ""),
        "Home" => named("Home", "Home", 36, ""),
        "End" => named("End", "End", 35, ""),
        "PageUp" => named("PageUp", "PageUp", 33, ""),
        "PageDown" => named("PageDown", "PageDown", 34, ""),
        "Space" | " " => named(" ", "Space", 32, " "),
        other => {
            let c = other.chars().next().unwrap_or(' ');
            let up = c.to_ascii_uppercase();
            (c.to_string(), format!("Key{up}"), up as u32, c.to_string())
        }
    }
}

struct Session {
    /// A window the user watches: the agent's pointer is drawn on the page.
    visible: bool,
    cdp: cdp::Cdp,
    size: (u32, u32),
    cursor: Option<(f32, f32)>,
    url: String,
    title: String,
    seen: Vec<String>,
    requests: std::collections::HashMap<String, String>,
    report: Report,
}

impl Session {
    async fn eval(&mut self, body: &str) -> Result<Value, String> {
        let expression = script::wrap(body);
        let r = self.cdp.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true, "awaitPromise": true})).await?;
        if let Some(e) = r.get("exceptionDetails") {
            let text = e["exception"]["description"].as_str().or_else(|| e["text"].as_str()).unwrap_or("error in the page script");
            return Err(text.lines().next().unwrap_or(text).to_string());
        }
        Ok(r["result"]["value"].clone())
    }

    /// `eval` for a script that answers with JSON text.
    async fn eval_json(&mut self, body: &str) -> Result<Value, String> {
        let v = self.eval(body).await?;
        serde_json::from_str(v.as_str().unwrap_or("null")).map_err(|e| format!("unexpected answer from the page: {e}"))
    }

    async fn refresh_state(&mut self) {
        if let Ok(v) = self.eval_json("return JSON.stringify({url: location.href, title: document.title})").await {
            self.url = v["url"].as_str().unwrap_or(&self.url).to_string();
            self.title = v["title"].as_str().unwrap_or("").to_string();
        }
    }

    /// Reads what the browser reported since the last step and keeps what is wrong.
    fn collect_problems(&mut self) {
        let events = self.cdp.take_events();
        let add = |s: &mut Self, p: String| {
            if !s.seen.contains(&p) && s.report.problems.len() < 12 {
                s.seen.push(p.clone());
                s.report.problems.push(p);
            }
        };
        for e in events {
            let p = &e["params"];
            match e["method"].as_str().unwrap_or("") {
                "Network.requestWillBeSent" => {
                    if let (Some(id), Some(url)) = (p["requestId"].as_str(), p["request"]["url"].as_str()) {
                        self.requests.insert(id.to_string(), url.to_string());
                    }
                }
                "Runtime.exceptionThrown" => {
                    let d = &p["exceptionDetails"];
                    let text = d["exception"]["description"].as_str().or_else(|| d["text"].as_str()).unwrap_or("exception");
                    add(self, format!("JavaScript error: {}", text.lines().next().unwrap_or(text)));
                }
                "Runtime.consoleAPICalled" if p["type"] == "error" => {
                    let text: Vec<String> = p["args"].as_array().into_iter().flatten().map(|a| a["value"].as_str().map(str::to_string).or_else(|| a["description"].as_str().map(str::to_string)).unwrap_or_default()).collect();
                    add(self, format!("console.error: {}", text.join(" ")));
                }
                "Network.loadingFailed" => {
                    let url = p["requestId"].as_str().and_then(|i| self.requests.get(i)).cloned().unwrap_or_default();
                    let why = p["errorText"].as_str().unwrap_or("falhou");
                    if !url.ends_with("favicon.ico") && p["canceled"] != true && !why.contains("ABORTED") {
                        add(self, format!("did not load: {url} ({why})"));
                    }
                }
                "Network.responseReceived" => {
                    let status = p["response"]["status"].as_u64().unwrap_or(200);
                    let url = p["response"]["url"].as_str().unwrap_or("");
                    if status >= 400 && !url.ends_with("favicon.ico") {
                        add(self, format!("resposta {status}: {url}"));
                    }
                }
                _ => {}
            }
        }
    }

    async fn shoot(&mut self) -> Vec<u8> {
        match self.cdp.call("Page.captureScreenshot", json!({"format": "png"})).await {
            Ok(r) => decode_png(r["data"].as_str().unwrap_or("")),
            Err(_) => Vec::new(),
        }
    }

    async fn page_labels(&mut self) -> Vec<Label> {
        let Ok(v) = self.eval_json(&script::labels()).await else { return Vec::new() };
        let n = |l: &Value, k: &str| l[k].as_f64().unwrap_or(0.0) as f32;
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| Some(Label { x: n(l, "x"), y: n(l, "y"), w: n(l, "w"), h: n(l, "h"), text: l["t"].as_str()?.to_string(), kind: l["k"].as_str().unwrap_or("").to_string() }))
            .collect()
    }

    async fn frame(&mut self, caption: &str, clicked: bool, on_frame: &mut (dyn FnMut(Frame) + Send)) -> Vec<u8> {
        self.refresh_state().await;
        let png = self.shoot().await;
        let labels = self.page_labels().await;
        on_frame(Frame { png: png.clone(), url: self.url.clone(), title: self.title.clone(), cursor: self.cursor, clicked, size: self.size, caption: caption.to_string(), labels, window: 0 });
        png
    }

    /// Waits for a navigation the last step may have started.
    async fn settle(&mut self) {
        self.cdp.listen(350).await;
        let navigated = self.cdp.has_event("Page.frameNavigated", |e| e["params"]["frame"].get("parentId").is_none());
        if navigated {
            self.cdp.wait_for("Page.loadEventFired", 10).await;
            self.cdp.listen(250).await;
        }
    }

    /// In a window the user watches, glides the drawn pointer to `(x, y)`.
    async fn glide(&mut self, x: f64, y: f64) {
        if self.visible {
            let _ = self.eval(&format!("window.__ctMove&&window.__ctMove({x},{y});return 1")).await;
            self.cdp.listen(430).await;
        }
    }

    /// And opens the ring of a click there.
    async fn ring(&mut self, x: f64, y: f64) {
        if self.visible {
            let _ = self.eval(&format!("window.__ctClick&&window.__ctClick({x},{y});return 1")).await;
        }
    }

    async fn mouse(&mut self, kind: &str, x: f64, y: f64) -> Result<(), String> {
        let mut p = json!({"type": kind, "x": x, "y": y, "button": "none"});
        if kind != "mouseMoved" {
            p["button"] = json!("left");
            p["clickCount"] = json!(1);
        }
        self.cdp.call("Input.dispatchMouseEvent", p).await.map(|_| ())
    }

    async fn navigate(&mut self, url: &str) -> Result<(), String> {
        self.cdp.forget("Page.loadEventFired");
        self.cdp.forget("Page.frameNavigated");
        let r = self.cdp.call("Page.navigate", json!({"url": url})).await?;
        if let Some(e) = r["errorText"].as_str() {
            return Err(format!("could not open {url}: {e}"));
        }
        if !self.cdp.wait_for("Page.loadEventFired", 20).await {
            self.report.problems.push(format!("the page {url} took more than 20 s to load"));
        }
        self.cdp.listen(300).await;
        Ok(())
    }

    async fn step(&mut self, a: &Action, root: &Path, on_frame: &mut (dyn FnMut(Frame) + Send)) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        match a {
            Action::Open(u) => {
                let url = resolve_url(root, u);
                self.navigate(&url).await?;
                self.refresh_state().await;
                out.push(format!("opened {} — \"{}\"", self.url, self.title));
                self.frame(&a.caption(), false, on_frame).await;
            }
            Action::Snapshot => {
                let v = self.eval_json(&script::snapshot()).await?;
                out.push(script::describe_snapshot(&v));
                self.frame(&a.caption(), false, on_frame).await;
            }
            Action::Click(t) => {
                let g = self.eval_json(&script::prepare(&t.to_json(), false)).await?;
                if let Some(e) = g["error"].as_str() {
                    return Err(e.to_string());
                }
                let (x, y) = (g["x"].as_f64().unwrap_or(0.0), g["y"].as_f64().unwrap_or(0.0));
                let label = g["name"].as_str().unwrap_or("?").to_string();
                if g["disabled"] == true {
                    self.report.problems.push(format!("{} is disabled and does not respond to a click", t.describe()));
                }
                if let Some(b) = g["blocked"].as_str().filter(|b| !b.is_empty()) {
                    self.report.problems.push(format!("the click on \"{label}\" lands on <{b}>: something is covering it"));
                }
                // The pointer arrives first, then presses.
                self.cursor = Some((x as f32, y as f32));
                self.glide(x, y).await;
                self.mouse("mouseMoved", x, y).await?;
                self.cdp.forget("Page.frameNavigated");
                self.cdp.forget("Page.loadEventFired");
                self.frame(&a.caption(), false, on_frame).await;
                self.ring(x, y).await;
                self.mouse("mousePressed", x, y).await?;
                self.mouse("mouseReleased", x, y).await?;
                self.settle().await;
                let before = self.url.clone();
                self.refresh_state().await;
                out.push(if self.url != before { format!("clicked \"{label}\" → {} — \"{}\"", self.url, self.title) } else { format!("clicked \"{label}\" ({})", g["tag"].as_str().unwrap_or("")) });
                self.frame(&a.caption(), true, on_frame).await;
            }
            Action::Fill(t, text) => {
                let g = self.eval_json(&script::prepare(&t.to_json(), true)).await?;
                if let Some(e) = g["error"].as_str() {
                    return Err(e.to_string());
                }
                if g["editable"] != true {
                    return Err(format!("{} does not accept text", t.describe()));
                }
                let (x, y) = (g["x"].as_f64().unwrap_or(0.0), g["y"].as_f64().unwrap_or(0.0));
                self.cursor = Some((x as f32, y as f32));
                self.glide(x, y).await;
                self.mouse("mouseMoved", x, y).await?;
                self.ring(x, y).await;
                self.mouse("mousePressed", x, y).await?;
                self.mouse("mouseReleased", x, y).await?;
                self.eval(&script::select_all(&t.to_json())).await?;
                self.cdp.call("Input.insertText", json!({"text": text})).await?;
                self.cdp.listen(120).await;
                let value = self.eval(&script::value_of(&t.to_json())).await?;
                out.push(format!("filled {} with \"{}\"", t.describe(), value.as_str().unwrap_or(text)));
                self.frame(&a.caption(), true, on_frame).await;
            }
            Action::Press(k) => {
                let (key, code, vk, text) = key_info(k);
                self.cdp.forget("Page.frameNavigated");
                self.cdp.forget("Page.loadEventFired");
                let mut down = json!({"type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk});
                if !text.is_empty() {
                    down["text"] = json!(text);
                }
                self.cdp.call("Input.dispatchKeyEvent", down).await?;
                self.cdp.call("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk})).await?;
                self.settle().await;
                out.push(format!("pressed {k}"));
                self.frame(&a.caption(), false, on_frame).await;
            }
            Action::Scroll(y) => {
                let v = self.eval_json(&format!("scrollBy(0, {y}); return JSON.stringify({{y: Math.round(scrollY), h: document.documentElement.scrollHeight, vh: innerHeight}})")).await?;
                self.cdp.listen(150).await;
                out.push(format!("scrolled to {}px of {}px", v["y"], v["h"].as_u64().unwrap_or(0).saturating_sub(v["vh"].as_u64().unwrap_or(0))));
                self.frame(&a.caption(), false, on_frame).await;
            }
            Action::Wait(ms) => {
                self.cdp.listen(*ms).await;
                out.push(format!("waited {ms} ms"));
            }
            Action::Back => {
                self.cdp.forget("Page.frameNavigated");
                self.cdp.forget("Page.loadEventFired");
                self.eval("history.back(); return 'ok'").await?;
                self.settle().await;
                self.refresh_state().await;
                out.push(format!("went back to {} — \"{}\"", self.url, self.title));
                self.frame(&a.caption(), false, on_frame).await;
            }
            Action::Viewport(w, h) => {
                self.size = (*w, *h);
                self.cdp.call("Emulation.setDeviceMetricsOverride", json!({"width": w, "height": h, "deviceScaleFactor": 1, "mobile": *w < 500})).await?;
                self.cdp.listen(250).await;
                out.push(format!("window at {w}×{h}"));
                self.frame(&a.caption(), false, on_frame).await;
            }
            Action::Text(t) => {
                let v = self.eval(&script::read_text(t.as_ref().map(Target::to_json))).await?;
                out.push(format!("texto:\n{}", v.as_str().unwrap_or("")));
            }
            Action::Shot => {
                let png = self.frame(&a.caption(), false, on_frame).await;
                self.report.shots.push(png);
                out.push(format!("foto de {}", self.url));
            }
        }
        Ok(out)
    }
}

/// Evaluates an expression that answers with JSON text.
async fn page_json(cdp: &mut cdp::Cdp, expression: &str) -> Option<Value> {
    let r = cdp.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true})).await.ok()?;
    serde_json::from_str(r["result"]["value"].as_str()?).ok()
}

/// A browser that stays open between calls: the page, where it is scrolled
/// and the cookies are still there for the next steps.
pub struct Browser {
    launched: cdp::Launched,
    s: Session,
}

impl Browser {
    /// Opens a browser window of `size`.
    pub async fn open(browser: &Path, size: (u32, u32)) -> Result<Browser, String> {
        Self::open_with(browser, size, false).await
    }

    /// Opens a browser; with `visible`, a real small window the user can
    /// watch, at the right edge of the screen, with the agent's pointer on it.
    pub async fn open_with(browser: &Path, size: (u32, u32), visible: bool) -> Result<Browser, String> {
        let launched = cdp::launch(browser, size, visible).await?;
        match Self::connect(&launched, size, visible).await {
            Ok(s) => Ok(Browser { launched, s }),
            Err(e) => {
                launched.close().await;
                Err(e)
            }
        }
    }

    async fn connect(launched: &cdp::Launched, size: (u32, u32), visible: bool) -> Result<Session, String> {
        let ws = launched.page_socket().await?;
        let mut cdp = cdp::Cdp::connect(&ws).await?;
        for domain in ["Page.enable", "Runtime.enable", "Network.enable"] {
            cdp.call(domain, json!({})).await?;
        }
        // The pictures show the page, not its scrollbars.
        let _ = cdp.call("Emulation.setScrollbarsHidden", json!({"hidden": true})).await;
        let mut size = size;
        if visible {
            // The pointer, on every page that loads.
            cdp.call("Page.addScriptToEvaluateOnNewDocument", json!({"source": script::POINTER})).await?;
            size = Self::place_window(&mut cdp, &ws, size).await;
        } else {
            cdp.call("Emulation.setDeviceMetricsOverride", json!({"width": size.0, "height": size.1, "deviceScaleFactor": 1, "mobile": false})).await?;
        }
        Ok(Session { visible, cdp, size, cursor: None, url: String::new(), title: String::new(), seen: Vec::new(), requests: Default::default(), report: Report::default() })
    }

    /// Puts the window at the right edge of the screen, tall and narrow, so it
    /// sits beside the terminal; returns the size of the page inside it. A
    /// browser that will not be moved stays where it opened.
    async fn place_window(cdp: &mut cdp::Cdp, ws: &str, fallback: (u32, u32)) -> (u32, u32) {
        let read = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
        let screen = page_json(cdp, "JSON.stringify({w: screen.availWidth, h: screen.availHeight})").await;
        if let (Some(screen), Some(target)) = (screen, ws.rsplit('/').next()) {
            let (sw, sh) = (read(&screen, "w"), read(&screen, "h"));
            if sw > 400.0 && sh > 300.0 {
                let width = (sw * 0.40).clamp(520.0, 980.0);
                let height = (sh - 40.0).max(400.0);
                if let Ok(win) = cdp.call("Browser.getWindowForTarget", json!({"targetId": target})).await
                    && let Some(id) = win["windowId"].as_i64()
                {
                    let bounds = json!({"left": (sw - width - 6.0).max(0.0), "top": 10, "width": width, "height": height, "windowState": "normal"});
                    let _ = cdp.call("Browser.setWindowBounds", json!({"windowId": id, "bounds": bounds})).await;
                    cdp.listen(500).await;
                }
            }
        }
        match page_json(cdp, "JSON.stringify({w: innerWidth, h: innerHeight})").await {
            Some(v) if read(&v, "w") > 100.0 && read(&v, "h") > 100.0 => (read(&v, "w") as u32, read(&v, "h") as u32),
            _ => fallback,
        }
    }

    /// Runs `actions` on the page as it is now, calling `on_frame` after every step.
    pub async fn run(&mut self, root: &Path, actions: &[Action], on_frame: &mut (dyn FnMut(Frame) + Send)) -> Result<Report, String> {
        tokio::time::timeout(Duration::from_secs(150), self.run_steps(root, actions, on_frame))
            .await
            .unwrap_or_else(|_| Err("the navigation ran past 150 s and was stopped".into()))
    }

    async fn run_steps(&mut self, root: &Path, actions: &[Action], on_frame: &mut (dyn FnMut(Frame) + Send)) -> Result<Report, String> {
        let s = &mut self.s;
        // Each call reports its own findings; what the page said before is not repeated.
        s.report = Report::default();
        s.seen.clear();
        let _ = s.cdp.take_events();
        // Is the browser still there? A cheap question that fails when it is not.
        s.eval("return 1").await.map_err(|e| format!("the browser does not respond: {e}"))?;
        for (i, a) in actions.iter().enumerate() {
            let line = match s.step(a, root, on_frame).await {
                Ok(lines) => lines.join("\n"),
                Err(e) => {
                    s.report.log.push(format!("{}. {} → ERROR: {e}", i + 1, a.caption()));
                    s.collect_problems();
                    // A step that fails ends the call: the next ones depend on it.
                    break;
                }
            };
            s.collect_problems();
            s.report.log.push(format!("{}. {}", i + 1, line));
        }
        // The state the page was left in.
        let png = s.frame("done", false, on_frame).await;
        if s.report.shots.last() != Some(&png) {
            s.report.shots.push(png);
        }
        s.collect_problems();
        Ok(std::mem::take(&mut s.report))
    }

    /// Closes the window.
    pub async fn close(self) {
        self.launched.close().await;
    }
}

/// Runs the actions in a fresh browser, calling `on_frame` after every step,
/// and closes it.
pub async fn run(browser: &Path, root: &Path, actions: &[Action], size: (u32, u32), on_frame: &mut (dyn FnMut(Frame) + Send)) -> Result<Report, String> {
    let mut b = Browser::open(browser, size).await?;
    let result = b.run(root, actions, on_frame).await;
    b.close().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_made_ready_for_the_browser() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), "x").unwrap();
        std::fs::write(dir.path().join("sobre uma.html"), "x").unwrap();
        let root = dir.path();
        assert_eq!(resolve_url(root, "https://exemplo.com/a"), "https://exemplo.com/a");
        assert_eq!(resolve_url(root, "localhost:3000/menu"), "http://localhost:3000/menu");
        assert_eq!(resolve_url(root, "127.0.0.1:8080"), "http://127.0.0.1:8080");
        let index = resolve_url(root, "index.html");
        assert!(index.starts_with("file:///") && index.ends_with("/index.html") && !index.contains('\\') && !index.contains("//?/"), "{index}");
        assert_eq!(resolve_url(root, "."), index, "a folder opens its index.html");
        assert!(resolve_url(root, "sobre uma.html").ends_with("/sobre%20uma.html"));
    }

    #[test]
    fn keys_have_codes() {
        assert_eq!(key_info("Enter"), ("Enter".into(), "Enter".into(), 13, "\r".into()));
        assert_eq!(key_info("ArrowDown").2, 40);
        assert_eq!(key_info("a"), ("a".into(), "KeyA".into(), 65, "a".into()));
        assert_eq!(key_info("Escape").3, "");
    }
}
