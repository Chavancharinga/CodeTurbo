//! Sameness measured on the rendered page: the installed Edge (or Chrome)
//! opens the HTML without a window, a script reads the computed layout and
//! styles, and a screenshot is saved for the user.

use crate::fiscal::{Finding, Severity};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The browser used for rendering: `CODETURBO_BROWSER`, then Edge, then Chrome.
pub fn find_browser() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CODETURBO_BROWSER").map(PathBuf::from).filter(|p| p.exists()) {
        return Some(p);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var).map(PathBuf::from) {
                candidates.push(base.join(r"Microsoft\Edge\Application\msedge.exe"));
                candidates.push(base.join(r"Google\Chrome\Application\chrome.exe"));
            }
        }
    } else if cfg!(target_os = "macos") {
        candidates.push("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into());
        candidates.push("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into());
    } else {
        let path = std::env::var_os("PATH").unwrap_or_default();
        for name in ["microsoft-edge", "microsoft-edge-stable", "google-chrome", "chromium", "chromium-browser"] {
            candidates.extend(std::env::split_paths(&path).map(|d| d.join(name)));
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

/// Runs in the page and leaves its measurements on `<html data-codeturbo>`.
const PROBE: &str = r#"<script>(()=>{const go=()=>{
const els=[...document.querySelectorAll('body *')].filter(e=>{const r=e.getBoundingClientRect();return r.width>=40&&r.height>=24;});
const radii=new Set();let rounded=0;const shadows=new Set();let shadowed=0;let centered=0,text=0;
for(const e of els){const s=getComputedStyle(e);
if(s.borderTopLeftRadius!=='0px'&&!s.borderTopLeftRadius.includes('%')&&parseFloat(s.borderTopLeftRadius)<200){rounded++;radii.add(s.borderTopLeftRadius);}
if(s.boxShadow!=='none'){shadowed++;shadows.add(s.boxShadow);}
if(/^(P|H1|H2|H3|LI)$/.test(e.tagName)&&e.textContent.trim().length>20){text++;if(s.textAlign==='center')centered++;}}
let rows=0;for(const p of document.querySelectorAll('body *')){const k=[...p.children].filter(c=>c.getBoundingClientRect().width>120);
if(k.length<3)continue;const w=k.map(c=>Math.round(c.getBoundingClientRect().width)),h=k.map(c=>c.getBoundingClientRect().height);
const sameTag=k.every(c=>c.tagName===k[0].tagName&&c.className===k[0].className);
if(sameTag&&Math.max(...w)-Math.min(...w)<=2&&Math.max(...h)/Math.max(1,Math.min(...h))<1.15&&k[0].children.length>=2)rows++;}
document.documentElement.setAttribute('data-codeturbo',JSON.stringify({rounded,radii:radii.size,shadowed,shadows:shadows.size,centered,text,rows}));};
if(document.readyState==='complete')go();else addEventListener('load',go);})();</script>"#;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metrics {
    pub rounded: usize,
    pub radii: usize,
    pub shadowed: usize,
    pub shadows: usize,
    pub centered: usize,
    pub text: usize,
    /// Groups of three or more same-size, same-class blocks side by side.
    pub rows: usize,
}

pub fn parse_metrics(dom: &str) -> Option<Metrics> {
    let start = dom.find("data-codeturbo=\"")? + "data-codeturbo=\"".len();
    let raw = &dom[start..start + dom[start..].find('"')?];
    let json = raw.replace("&quot;", "\"").replace("&amp;", "&");
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let n = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    Some(Metrics {
        rounded: n("rounded"),
        radii: n("radii"),
        shadowed: n("shadowed"),
        shadows: n("shadows"),
        centered: n("centered"),
        text: n("text"),
        rows: n("rows"),
    })
}

/// What the measurements say about the page.
pub fn judge(m: &Metrics) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |severity, rule, message: String| out.push(Finding { severity, rule, line: 1, message });
    if m.rows >= 2 {
        add(Severity::P1, "render-grades-iguais", format!("{} fileiras de blocos idênticos na página renderizada", m.rows));
    }
    if m.rounded >= 6 && m.radii == 1 {
        add(Severity::P1, "render-raio-unico", format!("{} elementos renderizados com um único raio", m.rounded));
    }
    if m.text >= 8 && m.centered * 10 >= m.text * 7 {
        add(Severity::P1, "render-centralizado", format!("{} de {} blocos de texto centralizados", m.centered, m.text));
    }
    if m.shadowed >= 4 && m.shadows == 1 {
        add(Severity::P2, "render-sombra-unica", format!("{} elementos com a mesma sombra", m.shadowed));
    }
    out
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_start_matches("//?/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

async fn run(browser: &Path, args: &[String]) -> Result<String, String> {
    // A fresh profile per run: Edge hands a second launch on a busy
    // profile to the running instance, which then ignores its flags.
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let profile = std::env::temp_dir().join(format!("codeturbo-render-{}-{n}", std::process::id()));
    let mut cmd = tokio::process::Command::new(browser);
    cmd.args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--hide-scrollbars", "--window-size=1440,900"])
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(40), cmd.output())
        .await
        .map_err(|_| "o navegador não respondeu em 40s".to_string())?
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&profile);
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Renders `html` with `browser`: its measurements, and a screenshot saved
/// to `shot`.
pub async fn measure(browser: &Path, html: &Path, shot: &Path) -> Result<Metrics, String> {
    let text = std::fs::read_to_string(html).map_err(|e| format!("{}: {e}", html.display()))?;
    let probed = match text.rfind("</body>") {
        Some(i) => format!("{}{PROBE}{}", &text[..i], &text[i..]),
        None => format!("{text}{PROBE}"),
    };
    // Next to the original so relative assets still load.
    let stem = html.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
    let tmp = html.with_file_name(format!(".codeturbo-render-{stem}.html"));
    std::fs::write(&tmp, probed).map_err(|e| e.to_string())?;
    let url = file_url(&tmp.canonicalize().unwrap_or(tmp.clone()));
    let dom = run(browser, &["--virtual-time-budget=4000".into(), "--dump-dom".into(), url]).await;
    let shot = std::path::absolute(shot).unwrap_or(shot.to_path_buf());
    if let Some(dir) = shot.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let page = file_url(&html.canonicalize().unwrap_or(html.to_path_buf()));
    let _ = run(browser, &["--virtual-time-budget=4000".into(), format!("--screenshot={}", shot.display()), page]).await;
    let _ = std::fs::remove_file(&tmp);
    parse_metrics(&dom?).ok_or_else(|| "a página não pôde ser medida".to_string())
}
