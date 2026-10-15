//! The page as a user sees it: the installed Edge (or Chrome) opens the
//! HTML without a window, at desktop and phone width; a script reads the
//! computed layout (overlaps, cramped neighbors, broken images, sideways
//! scrolling, visible scrollbars, sameness) and screenshots are saved.

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
let host=document.documentElement;try{if(window.parent!==window)host=parent.document.documentElement;}catch(e){}
host.setAttribute('data-codeturbo',JSON.stringify({rounded,radii:radii.size,shadowed,shadows:shadows.size,centered,text,rows,tiny,overlaps,tight,broken,hscroll,scrollers,small,cut,edge,under}));};
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
}

/// Measurements at every screen size checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    pub desktop: Metrics,
    pub mobile: Metrics,
    pub tablet: Metrics,
    pub laptop: Metrics,
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
    })
}

fn sample(v: &[String]) -> String {
    v.iter().take(3).map(|s| format!("[{s}]")).collect::<Vec<_>>().join(", ")
}

/// What the measurements say about the page.
pub fn judge(p: &Page) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |severity, rule, message: String| out.push(Finding { severity, rule, line: 1, message });
    for (m, at) in [(&p.desktop, "no computador"), (&p.laptop, "no notebook"), (&p.tablet, "no tablet"), (&p.mobile, "no celular")] {
        if !m.overlaps.is_empty() {
            add(Severity::P0, "render-sobreposicao", format!("elementos sobrepostos {at}: {}", sample(&m.overlaps)));
        }
        if !m.under.is_empty() {
            add(Severity::P2, "render-ornamento-sobre-texto", format!(
                "ornamento decorativo passando sobre o conteúdo {at}: {}; afaste-o do texto ou reduza a opacidade",
                sample(&m.under)
            ));
        }
        if !m.tight.is_empty() {
            add(Severity::P1, "render-colados", format!("elementos colados, sem espaço entre si, {at}: {}", sample(&m.tight)));
        }
        if !m.cut.is_empty() {
            add(Severity::P0, "render-conteudo-cortado", format!(
                "texto cortado pela borda da tela {at}: {}; use larguras fluidas (max-width: 100%, clamp() no tamanho de fonte) em vez de esconder o excesso",
                sample(&m.cut)
            ));
        }
        if !m.scrollers.is_empty() {
            add(Severity::P1, "render-barra-de-rolagem", format!("barra de rolagem visível {at}: {}", sample(&m.scrollers)));
        }
    }
    let broken = if p.desktop.broken.is_empty() { &p.mobile.broken } else { &p.desktop.broken };
    if !broken.is_empty() {
        add(Severity::P0, "render-imagem-quebrada", format!(
            "imagem que não carrega: {}; sem um arquivo real, troque por uma ilustração em SVG embutido ou por um bloco feito em HTML e CSS, ou remova",
            sample(broken)
        ));
    }
    if !p.mobile.edge.is_empty() {
        add(Severity::P1, "render-sem-margem", format!(
            "texto encostado na borda da tela no celular: {}; dê margem lateral (padding-inline) aos contêineres",
            sample(&p.mobile.edge)
        ));
    }
    if p.mobile.hscroll {
        add(Severity::P1, "render-sem-responsividade", "a página rola para o lado no celular (390px): não é responsiva".into());
    }
    for (m, at) in [(&p.desktop, "no computador"), (&p.laptop, "no notebook (1280px)"), (&p.tablet, "no tablet (768px)")] {
        if m.hscroll {
            add(Severity::P1, "render-rolagem-lateral", format!("a página rola para o lado {at}"));
        }
    }
    if p.mobile.small >= 3 {
        add(Severity::P1, "render-alvo-pequeno", format!("{} botões ou links menores que 32px no celular", p.mobile.small));
    }
    if p.mobile.tiny >= 3 {
        add(Severity::P2, "render-texto-pequeno", format!("{} blocos de texto abaixo de 14px no celular", p.mobile.tiny));
    }
    let m = &p.desktop;
    if m.rows >= 3 {
        add(Severity::P2, "render-grades-iguais", format!("{} fileiras de blocos idênticos na página renderizada", m.rows));
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

async fn run(browser: &Path, size: (u32, u32), args: &[String]) -> Result<String, String> {
    // A fresh profile per run: Edge hands a second launch on a busy
    // profile to the running instance, which then ignores its flags.
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let profile = std::env::temp_dir().join(format!("codeturbo-render-{}-{n}", std::process::id()));
    let mut cmd = tokio::process::Command::new(browser);
    cmd.args(["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--allow-file-access-from-files"])
        .arg(format!("--window-size={},{}", size.0, size.1))
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
        let f = html.with_file_name(format!(".codeturbo-render-{stem}-{name}.html"));
        let doc = format!(
            "<!DOCTYPE html><html><body style=\"margin:0;background:#1b1d22\"><iframe scrolling=\"no\" src=\"{src}\" style=\"border:0;width:{}px;height:{}px;background:#fff\"></iframe></body></html>",
            size.0, size.1
        );
        std::fs::write(&f, doc).map_err(|e| e.to_string())?;
        Ok(file_url(&f.canonicalize().unwrap_or(f.clone())))
    };
    let (measure_url, shot_url) = if framed { (frame(&url, "medir")?, frame(&page, "ver")?) } else { (url, page) };
    let window = if framed { (520, size.1) } else { size };
    let dom = run(browser, window, &["--virtual-time-budget=5000".into(), "--dump-dom".into(), measure_url]).await;
    let shot = std::path::absolute(shot).unwrap_or(shot.to_path_buf());
    if let Some(dir) = shot.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = run(browser, window, &["--virtual-time-budget=5000".into(), format!("--screenshot={}", shot.display()), shot_url]).await;
    let _ = std::fs::remove_file(&tmp);
    for name in ["medir", "ver"] {
        let _ = std::fs::remove_file(html.with_file_name(format!(".codeturbo-render-{stem}-{name}.html")));
    }
    parse_metrics(&dom?).ok_or_else(|| "a página não pôde ser medida".to_string())
}

/// Phone screenshot path for a desktop one: `index.png` → `index-celular.png`.
pub fn mobile_shot(shot: &Path) -> PathBuf {
    let stem = shot.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
    shot.with_file_name(format!("{stem}-celular.png"))
}

/// Screenshot path for another size: `index.png` → `index-tablet.png`.
pub fn sized_shot(shot: &Path, name: &str) -> PathBuf {
    let stem = shot.file_stem().map_or("page".into(), |s| s.to_string_lossy().into_owned());
    shot.with_file_name(format!("{stem}-{name}.png"))
}

/// Renders `html` at desktop, laptop, tablet and phone sizes, in parallel.
/// Screenshots go to `shot` and its `-notebook`, `-tablet` and `-celular`
/// siblings.
pub async fn measure(browser: &Path, html: &Path, shot: &Path) -> Result<Page, String> {
    let (laptop_shot, tablet_shot, phone_shot) = (sized_shot(shot, "notebook"), sized_shot(shot, "tablet"), mobile_shot(shot));
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
    run(browser, size, &["--virtual-time-budget=6000".into(), format!("--screenshot={}", shot.display()), url.to_string()]).await?;
    if shot.exists() { Ok(()) } else { Err("o navegador não salvou a captura".into()) }
}
