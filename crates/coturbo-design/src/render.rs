//! The page as a user sees it: the installed Edge (or Chrome) opens the
//! HTML without a window, at desktop and phone width; a script reads the
//! computed layout (overlaps, cramped neighbors, broken images, sideways
//! scrolling, visible scrollbars, sameness) and screenshots are saved.

use crate::fiscal::{Finding, Severity};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Android, in Termux.
pub fn on_android() -> bool {
    cfg!(target_os = "android") || std::env::var("PREFIX").is_ok_and(|p| p.contains("com.termux"))
}

/// What to say when there is no browser, for this system.
pub fn no_browser_help() -> &'static str {
    if on_android() {
        "no browser found. Install Chromium yourself with bash: `pkg install -y x11-repo && pkg install -y chromium` \
         (about 100 MB; the user approves the command), then call this tool again"
    } else {
        "no browser found (Edge, Chrome or Chromium); install one or set CODETURBO_BROWSER"
    }
}

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
const W=innerWidth;const clipped=e=>{for(let a=e.parentElement;a;a=a.parentElement){const s=getComputedStyle(a);if(s.overflow!=='visible'||s.overflowY!=='visible'||s.overflowX!=='visible'){const r=a.getBoundingClientRect();if(r.height<2||r.width<2)return true;}}return false;};
const vis=e=>{const s=getComputedStyle(e);return s.visibility!=='hidden'&&s.display!=='none'&&parseFloat(s.opacity)>0.2&&!clipped(e);};
const R=e=>e.getBoundingClientRect();
const label=e=>(e.tagName.toLowerCase()+(e.className&&typeof e.className==='string'?'.'+e.className.trim().split(/\s+/)[0]:'')+' "'+(e.innerText||e.alt||'').trim().replace(/\s+/g,' ').slice(0,24)+'"');
const all=[...document.querySelectorAll('body *')].filter(e=>{const r=R(e);return r.width>=40&&r.height>=24&&vis(e);});
const radii=new Set();let rounded=0;const shadows=new Set();let shadowed=0;let centered=0,text=0,tiny=0;
for(const e of all){const s=getComputedStyle(e);
if(s.borderTopLeftRadius!=='0px'&&!s.borderTopLeftRadius.includes('%')&&parseFloat(s.borderTopLeftRadius)<200){rounded++;radii.add(s.borderTopLeftRadius);}
if(s.boxShadow!=='none'){shadowed++;shadows.add(s.boxShadow);}
if(/^(P|H1|H2|H3|LI)$/.test(e.tagName)&&e.textContent.trim().length>20){text++;if(s.textAlign==='center')centered++;if(parseFloat(s.fontSize)<14)tiny++;}}
let rows=0;for(const p of document.querySelectorAll('body *')){const k=[...p.children].filter(c=>R(c).width>120);
if(k.length<3)continue;const w=k.map(c=>Math.round(R(c).width)),h=k.map(c=>R(c).height);
const same=k.every(c=>c.tagName===k[0].tagName&&c.className===k[0].className);
if(same&&Math.max(...w)-Math.min(...w)<=2&&Math.max(...h)/Math.max(1,Math.min(...h))<1.15&&k[0].children.length>=2)rows++;}
const own=e=>[...e.childNodes].some(n=>n.nodeType===3&&n.textContent.trim().length>0);
const items=[...document.querySelectorAll('body *')].filter(e=>vis(e)&&(own(e)||/^(IMG|BUTTON|INPUT|SELECT|TEXTAREA|SVG|VIDEO)$/i.test(e.tagName))).filter(e=>{const r=R(e);return r.width>4&&r.height>4;}).slice(0,600);
const decor=e=>!!e.closest('[aria-hidden="true"]')||getComputedStyle(e).pointerEvents==='none';
const overlaps=[],tight=[],under=[];
for(let i=0;i<items.length;i++){for(let j=i+1;j<items.length;j++){const a=items[i],b=items[j];if(a.contains(b)||b.contains(a))continue;
const p=R(a),q=R(b);const ix=Math.min(p.right,q.right)-Math.max(p.left,q.left),iy=Math.min(p.bottom,q.bottom)-Math.max(p.top,q.top);
if(ix>2&&iy>2){const area=ix*iy,small=Math.min(p.width*p.height,q.width*q.height);if(area>small*0.15&&area>60){const l=label(a)+' × '+label(b);if(decor(a)||decor(b)){if(under.length<8)under.push(l);}else if(overlaps.length<8)overlaps.push(l);}continue;}
const sameLine=Math.min(p.bottom,q.bottom)-Math.max(p.top,q.top)>Math.min(p.height,q.height)*0.5;
const gap=Math.max(q.left-p.right,p.left-q.right);
// Adjacent links or buttons with no gap read as one blob (a squished nav).
const clickable=e=>/^(A|BUTTON)$/.test(e.tagName);
if(sameLine&&gap>=-2&&gap<4&&clickable(a)&&clickable(b)&&a.parentElement===b.parentElement&&a.textContent.trim()&&b.textContent.trim()&&tight.length<8)tight.push(label(a)+' | '+label(b));}}
const broken=[...document.images].filter(i=>vis(i)&&i.complete&&i.naturalWidth===0).map(label).slice(0,5);
const hscroll=document.documentElement.scrollWidth>W+1;
const edge=W<600?items.filter(e=>{const r=R(e);return own(e)&&r.width<W-4&&(r.left<10||r.right>W-10);}).map(label).slice(0,5):[];
const cut=items.filter(e=>{const r=R(e);return own(e)&&(r.right>W+1||r.left<-1||e.scrollWidth>e.clientWidth+2);}).map(label).slice(0,5);
const scrollers=[...document.querySelectorAll('body *')].filter(e=>{const s=getComputedStyle(e);return vis(e)&&((/(auto|scroll)/.test(s.overflowY)&&e.scrollHeight>e.clientHeight+1)||(/(auto|scroll)/.test(s.overflowX)&&e.scrollWidth>e.clientWidth+1))&&e.clientWidth>0;}).map(label).slice(0,5);
const small=W<600?[...document.querySelectorAll('a,button,input,select')].filter(e=>{const r=R(e);return vis(e)&&r.width>0&&(r.height<32||r.width<32);}).length:0;
const rgb=t=>{const m=(t||'').match(/[\d.]+/g);return m?m.map(Number):null;};const lin=v=>{v/=255;return v<=0.03928?v/12.92:Math.pow((v+0.055)/1.055,2.4);};const lum=c=>0.2126*lin(c[0])+0.7152*lin(c[1])+0.0722*lin(c[2]);const bgOf=e=>{for(let a=e;a;a=a.parentElement){const s=getComputedStyle(a);if(s.backgroundImage&&s.backgroundImage!=='none')return null;const c=rgb(s.backgroundColor);if(c&&(c.length<4||c[3]>0.9))return c;}return [255,255,255];};const contrast=[];for(const e of items){if(contrast.length>=8)break;if(!own(e))continue;const s=getComputedStyle(e);const f=rgb(s.color);const b=bgOf(e);if(!f||!b||(f.length>3&&f[3]<0.5))continue;const L1=lum(f),L2=lum(b);const r=(Math.max(L1,L2)+0.05)/(Math.min(L1,L2)+0.05);const px=parseFloat(s.fontSize);const big=px>=24||(px>=18.6&&parseInt(s.fontWeight)>=700);if(r<(big?3:4.5))contrast.push(label(e)+' '+r.toFixed(1)+':1');}const longlines=[];const sizes=new Set();const inks=new Set();const fills=new Set();for(const e of items){if(!own(e))continue;const s=getComputedStyle(e);const px=parseFloat(s.fontSize);sizes.add(Math.round(px));inks.add(s.color);if(/^(P|LI|DD|BLOCKQUOTE)$/.test(e.tagName)&&e.textContent.trim().length>160&&longlines.length<5){const chars=R(e).width/(px*0.5);if(chars>90)longlines.push(label(e)+' ~'+Math.round(chars)+' chars');}}for(const e of all){const c=getComputedStyle(e).backgroundColor;if(c&&c!=='rgba(0, 0, 0, 0)'&&c!=='transparent')fills.add(c);}const noalt=[...document.images].filter(i=>vis(i)&&!i.hasAttribute('alt')).length;const unlabeled=[...document.querySelectorAll('input,select,textarea')].filter(i=>vis(i)&&!/^(hidden|submit|button|reset|image)$/.test(i.type)&&!(i.labels&&i.labels.length)&&!i.getAttribute('aria-label')&&!i.getAttribute('aria-labelledby')&&!i.title).length;const nolang=!document.documentElement.getAttribute('lang');const noviewport=!document.querySelector('meta[name=viewport]');let host=document.documentElement;try{if(window.parent!==window)host=parent.document.documentElement;}catch(e){}
const out=JSON.stringify({rounded,radii:radii.size,shadowed,shadows:shadows.size,centered,text,rows,tiny,overlaps,tight,broken,hscroll,scrollers,small,cut,edge,under,contrast,noalt,unlabeled,nolang,noviewport,longlines,sizes:sizes.size,inks:inks.size,fills:fills.size});host.setAttribute('data-codeturbo',out);console.log('CT_PROBE:'+out);};
if(document.readyState==='complete')setTimeout(go,300);else addEventListener('load',()=>setTimeout(go,300));})();</script>"#;

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
    /// Body text blocks under 14px.
    pub tiny: usize,
    /// Pairs of elements drawn on top of each other.
    pub overlaps: Vec<String>,
    /// Pairs of neighbors with no space between them.
    pub tight: Vec<String>,
    pub broken: Vec<String>,
    /// The page scrolls sideways.
    pub hscroll: bool,
    /// Elements showing their own scrollbar.
    pub scrollers: Vec<String>,
    /// Touch targets under 32px (phone width only).
    pub small: usize,
    /// Text running past the edge of the screen (hidden, not scrollable).
    pub cut: Vec<String>,
    /// Text touching the screen edge, with no side margin (phone only).
    pub edge: Vec<String>,
    /// Decorative elements (aria-hidden) drawn over content.
    pub under: Vec<String>,
    /// Text below the WCAG AA contrast against what is really behind it.
    pub contrast: Vec<String>,
    /// Images with no `alt`.
    pub noalt: usize,
    /// Form fields no label names.
    pub unlabeled: usize,
    /// `<html>` with no `lang`.
    pub nolang: bool,
    /// No `<meta name="viewport">`: phones show a shrunken desktop page.
    pub noviewport: bool,
    /// Paragraphs whose lines run past ~90 characters.
    pub longlines: Vec<String>,
    /// Distinct font sizes, text colours and background colours drawn.
    pub sizes: usize,
    pub inks: usize,
    pub fills: usize,
}

/// Measurements at every screen size checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub desktop: Metrics,
    pub mobile: Metrics,
    pub tablet: Metrics,
    pub laptop: Metrics,
}

/// The probe's result as the browser's log carries it (`CT_PROBE:{...}`),
/// shaped like the attribute [`parse_metrics`] reads. The log is a file, so it
/// reaches us from any shell, while the browser's standard output does not
/// from a terminal with no console (Git Bash, services).
pub fn dom_from_log(log: &str) -> Option<String> {
    let line = log.lines().rev().find(|l| l.contains("CT_PROBE:"))?;
    let from = line.find("CT_PROBE:")? + "CT_PROBE:".len();
    let end = line.rfind("\", source:").unwrap_or(line.len());
    let json = line[from..end].trim().trim_end_matches('"');
    let esc = json.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
    Some(format!("<html data-codeturbo=\"{esc}\"></html>"))
}

/// Waits up to `secs` for `file` to exist (the browser may finish after the
/// program it was started through has already returned).
async fn wait_for(file: &Path, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if file.metadata().is_ok_and(|m| m.len() > 0) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// Saves a screenshot of `url` to `shot`. Where the browser is started
/// through a launcher that returns early, a start now and then goes nowhere
/// (several at once, or a slow machine), so a missing file earns more tries.
async fn shoot(browser: &Path, size: (u32, u32), budget: &str, url: &str, shot: &Path) -> bool {
    for wait in [10, 20, 30] {
        let _ = std::fs::remove_file(shot);
        let _ = run(browser, size, &[format!("--virtual-time-budget={budget}"), format!("--screenshot={}", shot.display()), url.to_string()]).await;
        if wait_for(shot, wait).await {
            return true;
        }
    }
    false
}

pub fn parse_metrics(dom: &str) -> Option<Metrics> {
    let start = dom.find("data-codeturbo=\"")? + "data-codeturbo=\"".len();
    let raw = &dom[start..start + dom[start..].find('"')?];
    let json = raw.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
    let v: serde_json::Value = serde_json::from_str(&json).ok()?;
    let n = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    let list = |k: &str| v[k].as_array().into_iter().flatten().filter_map(|x| x.as_str().map(str::to_string)).collect();
    Some(Metrics {
        rounded: n("rounded"),
        radii: n("radii"),
        shadowed: n("shadowed"),
        shadows: n("shadows"),
        centered: n("centered"),
        text: n("text"),
        rows: n("rows"),
        tiny: n("tiny"),
        overlaps: list("overlaps"),
        tight: list("tight"),
        broken: list("broken"),
        hscroll: v["hscroll"].as_bool().unwrap_or(false),
        scrollers: list("scrollers"),
        small: n("small"),
        cut: list("cut"),
        edge: list("edge"),
        under: list("under"),
        contrast: list("contrast"),
        noalt: n("noalt"),
        unlabeled: n("unlabeled"),
        nolang: v["nolang"].as_bool().unwrap_or(false),
        noviewport: v["noviewport"].as_bool().unwrap_or(false),
        longlines: list("longlines"),
        sizes: n("sizes"),
        inks: n("inks"),
        fills: n("fills"),
    })
}

fn sample(v: &[String]) -> String {
    v.iter().take(3).map(|s| format!("[{s}]")).collect::<Vec<_>>().join(", ")
}

/// What the measurements say about the page.
pub fn judge(p: &Page) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |severity, rule, message: String| out.push(Finding { severity, rule, line: 1, message });
    for (m, at) in [(&p.desktop, "on desktop"), (&p.laptop, "on a laptop"), (&p.tablet, "on a tablet"), (&p.mobile, "on the phone")] {
        if !m.overlaps.is_empty() {
            add(Severity::P0, "render-overlap", format!("elements drawn on top of each other {at}: {}; give them their own space in the layout (grid/flex gap) instead of absolute positions", sample(&m.overlaps)));
        }
        if !m.under.is_empty() {
            add(Severity::P2, "render-ornament-over-text", format!(
                "decorative ornament over the content {at}: {}; move it away from the text or lower its opacity",
                sample(&m.under)
            ));
        }
        if !m.tight.is_empty() {
            add(Severity::P1, "render-cramped", format!("elements touching, with no space between them, {at}: {}; add a gap from the spacing scale", sample(&m.tight)));
        }
        if !m.cut.is_empty() {
            add(Severity::P0, "render-cut-off", format!(
                "text cut off by the screen edge {at}: {}; use fluid widths (max-width: 100%, clamp() for font sizes) instead of hiding the overflow",
                sample(&m.cut)
            ));
        }
        if !m.scrollers.is_empty() {
            add(Severity::P1, "render-scrollbar", format!("visible scrollbar {at}: {}; let the content grow or wrap instead of scrolling inside a box", sample(&m.scrollers)));
        }
    }
    let broken = if p.desktop.broken.is_empty() { &p.mobile.broken } else { &p.desktop.broken };
    if !broken.is_empty() {
        add(Severity::P0, "render-broken-image", format!(
            "image that does not load: {}; without a real file, replace it with an inline SVG illustration or a block made in HTML and CSS, or remove it",
            sample(broken)
        ));
    }
    if !p.mobile.edge.is_empty() {
        add(Severity::P1, "render-no-side-margin", format!(
            "text touching the screen edge on the phone: {}; give the containers side padding (padding-inline)",
            sample(&p.mobile.edge)
        ));
    }
    if p.mobile.hscroll {
        add(Severity::P1, "render-not-responsive", "the page scrolls sideways on the phone (390px): it is not responsive; find the fixed width (min-width, wide images, long words) and make it fluid".into());
    }
    for (m, at) in [(&p.desktop, "on desktop"), (&p.laptop, "on a laptop (1280px)"), (&p.tablet, "on a tablet (768px)")] {
        if m.hscroll {
            add(Severity::P1, "render-sideways-scroll", format!("the page scrolls sideways {at}"));
        }
    }
    if p.mobile.small >= 3 {
        add(Severity::P1, "render-small-target", format!("{} buttons or links smaller than 32px on the phone; touch targets need at least 44px (padding, min-height)", p.mobile.small));
    }
    if p.mobile.tiny >= 3 {
        add(Severity::P2, "render-small-text", format!("{} text blocks under 14px on the phone; body text needs 16px", p.mobile.tiny));
    }
    // Accessibility, as the page is drawn.
    let d = &p.desktop;
    if !d.contrast.is_empty() {
        add(Severity::P0, "render-low-contrast", format!(
            "text below the WCAG AA contrast (4.5:1, or 3:1 for large text): {}; darken the text or lighten what is behind it, using the contract colours",
            sample(&d.contrast)
        ));
    }
    if d.noalt > 0 {
        add(Severity::P1, "render-missing-alt", format!(
            "{} image(s) with no alt text; describe what the image shows, or alt=\"\" when it is only decoration",
            d.noalt
        ));
    }
    if d.unlabeled > 0 {
        add(Severity::P1, "render-unlabeled-field", format!(
            "{} form field(s) with no label (a placeholder is not a label); add <label for> or aria-label",
            d.unlabeled
        ));
    }
    if d.noviewport {
        add(Severity::P1, "render-no-viewport", "no <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">: phones show a shrunken desktop page".into());
    }
    if d.nolang {
        add(Severity::P2, "render-no-lang", "<html> has no lang attribute: screen readers and translators guess the language; add lang=\"en\" (or the page's language)".into());
    }
    // Rhythm and coherence.
    if !d.longlines.is_empty() {
        add(Severity::P2, "render-long-lines", format!(
            "lines too long to read comfortably: {}; keep running text near 60-75 characters (max-width: 65ch)",
            sample(&d.longlines)
        ));
    }
    if d.sizes > 8 {
        add(Severity::P2, "render-too-many-sizes", format!(
            "{} different font sizes on the page; use the contract's type scale (about 5-7 steps)",
            d.sizes
        ));
    }
    if d.inks > 6 || d.fills > 8 {
        add(Severity::P2, "render-too-many-colours", format!(
            "{} text colours and {} background colours drawn; keep to the contract roles (ink, muted, primary...) so the palette reads as one",
            d.inks, d.fills
        ));
    }
    let m = &p.desktop;
    if m.rows >= 3 {
        add(Severity::P2, "render-identical-grids", format!("{} rows of identical blocks in the rendered page; vary size, emphasis or layout so the important one leads", m.rows));
    }
    if m.rounded >= 6 && m.radii == 1 {
        add(Severity::P1, "render-single-radius", format!("{} elements rendered with a single radius; use the contract radii by role (small for inputs, larger for surfaces)", m.rounded));
    }
    if m.text >= 8 && m.centered * 10 >= m.text * 7 {
        add(Severity::P1, "render-all-centered", format!("{} of {} text blocks centred; align running text to the left and keep centring for short headings", m.centered, m.text));
    }
    if m.shadowed >= 4 && m.shadows == 1 {
        add(Severity::P2, "render-single-shadow", format!("{} elements with the same shadow; use elevation levels, or borders instead", m.shadowed));
    }
    out
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_start_matches("//?/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

async fn run(browser: &Path, size: (u32, u32), args: &[String]) -> Result<String, String> {
    // A few browsers at a time: many starting together on one machine make
    // some of them hand back nothing (no screenshot, no measurements).
    static SLOTS: std::sync::OnceLock<tokio::sync::Semaphore> = std::sync::OnceLock::new();
    let _slot = SLOTS.get_or_init(|| tokio::sync::Semaphore::new(3)).acquire().await.map_err(|e| e.to_string())?;
    // A fresh profile per run: Edge hands a second launch on a busy
    // profile to the running instance, which then ignores its flags.
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let profile = std::env::temp_dir().join(format!("codeturbo-render-{}-{n}", std::process::id()));
    let mut cmd = tokio::process::Command::new(browser);
    cmd.args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--allow-file-access-from-files"]);
    // Hosted CI runners on Linux forbid the browser's own sandbox; on Android
    // (Termux) Chromium only starts without it.
    if (cfg!(target_os = "linux") && std::env::var_os("CI").is_some()) || on_android() {
        cmd.arg("--no-sandbox");
    }
    cmd.arg(format!("--window-size={},{}", size.0, size.1))
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(40), cmd.output())
        .await
        .map_err(|_| "the browser did not answer in 40s".to_string())?
        .map_err(|e| e.to_string())?;
    // Not removed now: on Windows the program started hands the page to
    // another browser process and returns before the screenshot is written;
    // deleting its profile here kills that render. Old profiles go instead.
    sweep_profiles(&profile);
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Removes render profiles left from earlier runs (older than ten minutes),
/// never the one in use.
fn sweep_profiles(current: &Path) {
    let Some(dir) = current.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(600));
        if name.starts_with("codeturbo-render-") && e.path() != current && old && e.path().is_dir() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

pub const DESKTOP: (u32, u32) = (1440, 900);
pub const MOBILE: (u32, u32) = (390, 844);
pub const TABLET: (u32, u32) = (768, 1024);
pub const LAPTOP: (u32, u32) = (1280, 800);

/// Measures `html` at one size and saves a screenshot to `shot`.
async fn measure_at(browser: &Path, html: &Path, shot: &Path, size: (u32, u32)) -> Result<Metrics, String> {
    let text = std::fs::read_to_string(html).map_err(|e| format!("{}: {e}", html.display()))?;
    let probed = match text.rfind("</body>") {
        Some(i) => format!("{}{PROBE}{}", &text[..i], &text[i..]),
        None => format!("{text}{PROBE}"),
    };
    // Next to the original so relative assets still load.
    let stem = html.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
    let tmp = html.with_file_name(format!(".codeturbo-render-{stem}-{}.html", size.0));
    std::fs::write(&tmp, probed).map_err(|e| e.to_string())?;
    let url = file_url(&tmp.canonicalize().unwrap_or(tmp.clone()));
    let page = file_url(&html.canonicalize().unwrap_or(html.to_path_buf()));
    // The browser window can't be narrower than ~500px: phones are shown
    // in a frame of their real width instead.
    let framed = size.0 < 500;
    let frame = |src: &str, name: &str| -> Result<String, String> {
        let f = html.with_file_name(format!(".codeturbo-render-{stem}-{}-{name}.html", size.0));
        let doc = format!(
            "<!DOCTYPE html><html><body style=\"margin:0;background:#1b1d22\"><iframe scrolling=\"no\" src=\"{src}\" style=\"border:0;width:{}px;height:{}px;background:#fff\"></iframe></body></html>",
            size.0, size.1
        );
        std::fs::write(&f, doc).map_err(|e| e.to_string())?;
        Ok(file_url(&f.canonicalize().unwrap_or(f.clone())))
    };
    let (measure_url, shot_url) = if framed { (frame(&url, "measure")?, frame(&page, "view")?) } else { (url, page) };
    let window = if framed { (520, size.1) } else { size };
    // The result also goes to the browser's log, for shells where its
    // standard output is lost.
    static LOGS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = LOGS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let log = std::env::temp_dir().join(format!("codeturbo-render-{}-{n}.log", std::process::id()));
    let _ = std::fs::remove_file(&log);
    let mut dom = Err("the page could not be measured".to_string());
    for wait in [12, 20] {
        let _ = std::fs::remove_file(&log);
        let out = run(
            browser,
            window,
            &["--enable-logging".into(), "--v=0".into(), format!("--log-file={}", log.display()), "--virtual-time-budget=5000".into(), "--dump-dom".into(), measure_url.clone()],
        )
        .await;
        if out.as_deref().is_ok_and(|d| parse_metrics(d).is_some()) {
            dom = out;
            break;
        }
        // The probe line may land a moment after the log file appears.
        let mut found = None;
        for _ in 0..wait * 10 {
            if let Some(d) = std::fs::read_to_string(&log).ok().as_deref().and_then(dom_from_log) {
                found = Some(d);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(d) = found {
            dom = Ok(d);
            break;
        }
        dom = out;
    }
    let _ = std::fs::remove_file(&log);
    let shot = std::path::absolute(shot).unwrap_or(shot.to_path_buf());
    if let Some(dir) = shot.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    shoot(browser, window, "5000", &shot_url, &shot).await;
    let _ = std::fs::remove_file(&tmp);
    // Only this size's own frames: the sizes run at the same time, and one
    // finishing first must not delete the phone's frame before its screenshot.
    if framed {
        for name in ["measure", "view"] {
            let _ = std::fs::remove_file(html.with_file_name(format!(".codeturbo-render-{stem}-{}-{name}.html", size.0)));
        }
    }
    parse_metrics(&dom?).ok_or_else(|| "the page could not be measured".to_string())
}

/// Phone screenshot path for a desktop one: `index.png` → `index-phone.png`.
pub fn mobile_shot(shot: &Path) -> PathBuf {
    let stem = shot.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
    shot.with_file_name(format!("{stem}-phone.png"))
}

/// Screenshot path for another size: `index.png` → `index-tablet.png`.
pub fn sized_shot(shot: &Path, name: &str) -> PathBuf {
    let stem = shot.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
    shot.with_file_name(format!("{stem}-{name}.png"))
}

/// Renders `html` at desktop, laptop, tablet and phone sizes, in parallel.
/// Screenshots go to `shot` and its `-laptop`, `-tablet` and `-phone`
/// siblings.
pub async fn measure(browser: &Path, html: &Path, shot: &Path) -> Result<Page, String> {
    let (laptop_shot, tablet_shot, phone_shot) = (sized_shot(shot, "laptop"), sized_shot(shot, "tablet"), mobile_shot(shot));
    let (desktop, laptop, tablet, mobile) = tokio::join!(
        measure_at(browser, html, shot, DESKTOP),
        measure_at(browser, html, &laptop_shot, LAPTOP),
        measure_at(browser, html, &tablet_shot, TABLET),
        measure_at(browser, html, &phone_shot, MOBILE),
    );
    Ok(Page { desktop: desktop?, laptop: laptop?, tablet: tablet?, mobile: mobile? })
}

/// The page at `url` after its scripts ran, as HTML.
pub async fn dump_dom(browser: &Path, url: &str) -> Result<String, String> {
    run(browser, DESKTOP, &["--virtual-time-budget=6000".into(), "--dump-dom".into(), url.to_string()]).await
}

/// A screenshot of `url` at `size`, saved to `shot`.
pub async fn screenshot(browser: &Path, url: &str, shot: &Path, size: (u32, u32)) -> Result<(), String> {
    let shot = std::path::absolute(shot).unwrap_or(shot.to_path_buf());
    if let Some(dir) = shot.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if shoot(browser, size, "6000", url, &shot).await { Ok(()) } else { Err("the browser did not save the screenshot".into()) }
}
