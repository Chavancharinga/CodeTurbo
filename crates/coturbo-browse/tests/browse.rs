//! Navega de verdade numa página pequena, com o navegador instalado.

use coturbo_browse::{Action, Frame, Target, run};
use std::path::Path;

const INDEX: &str = r##"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><title>Loja da Ana</title>
<style>
body{font:16px sans-serif;margin:0}
nav{display:flex;gap:16px;padding:16px;background:#223}
nav a{color:#fff}
main{padding:24px}
button,input{font:inherit;padding:8px 14px}
#banner{position:fixed;left:0;right:0;top:290px;height:90px;background:#fc3;display:flex;align-items:center;justify-content:center}
#news{position:absolute;left:30px;top:520px}
</style></head><body>
<nav><a href="index.html">Início</a><a href="about.html">Sobre</a><a href="#contato">Contato</a></nav>
<main>
<h1>Bem-vinda</h1>
<button id="buy" onclick="document.getElementById('msg').textContent='Obrigado pela compra!'">Comprar</button>
<p id="msg"></p>
<form onsubmit="event.preventDefault();document.getElementById('out').textContent='Olá, '+document.getElementById('nome').value">
<input id="nome" placeholder="Seu nome"> <button type="submit">Enviar</button></form>
<p id="out"></p>
<img src="nao-existe.png" alt="foto" width="40" height="40">
<button id="news">Newsletter</button>
<div id="contato" style="margin-top:900px">Fale conosco</div>
</main>
<div id="banner">Usamos cookies</div>
<script>console.error("falhou ao carregar o carrinho")</script>
</body></html>"##;

const ABOUT: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Sobre nós</title></head>
<body><h1>Sobre nós</h1><p>Uma loja pequena.</p><a href="index.html">Voltar</a></body></html>"#;

fn site() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), INDEX).unwrap();
    std::fs::write(dir.path().join("about.html"), ABOUT).unwrap();
    dir
}

async fn browse(root: &Path, actions: Vec<Action>) -> Option<(coturbo_browse::Report, Vec<Frame>)> {
    let browser = coturbo_design::render::find_browser()?;
    let mut frames = Vec::new();
    let report = run(&browser, root, &actions, (1000, 650), &mut |f| frames.push(f)).await.expect("the session runs");
    Some((report, frames))
}

#[test]
fn actions_are_read_from_json() {
    use serde_json::json;
    let list = json!([
        {"do": "open", "url": "index.html"},
        {"do": "snapshot"},
        {"do": "click", "text": "Sobre"},
        {"do": "click", "ref": "@e7"},
        {"do": "fill", "selector": "#nome", "value": "Maria"},
        {"do": "fill", "label": "Seu nome", "value": "Ana"},
        {"do": "press", "key": "Enter"},
        {"do": "scroll", "y": 400},
        {"do": "text"},
        {"do": "text", "selector": "#out"},
        {"do": "viewport", "width": 390, "height": 844},
        {"do": "back"}, {"do": "wait", "ms": 20000}, {"do": "shot"}
    ]);
    let a = Action::parse_all(&list).unwrap();
    assert_eq!(a[2], Action::Click(Target::Text("Sobre".into())));
    assert_eq!(a[3], Action::Click(Target::Ref("@e7".into())));
    assert_eq!(a[4], Action::Fill(Target::Selector("#nome".into()), "Maria".into()));
    assert_eq!(a[5], Action::Fill(Target::Text("Seu nome".into()), "Ana".into()));
    assert_eq!(a[8], Action::Text(None));
    assert_eq!(a[9], Action::Text(Some(Target::Selector("#out".into()))));
    assert_eq!(a[10], Action::Viewport(390, 844));
    assert_eq!(a[12], Action::Wait(10_000), "a wait is capped");
    for (bad, why) in [(json!([]), "pelo menos uma"), (json!([{"do": "click"}]), "alvo"), (json!([{"do": "voar"}]), "desconhecida"), (json!([{"do": "fill", "selector": "#a"}]), "value"), (json!([{"x": 1}]), "do")] {
        let e = Action::parse_all(&bad).unwrap_err();
        assert!(e.contains(why), "{bad}: {e}");
    }
    assert!(Action::parse_all(&json!((0..41).map(|_| json!({"do": "snapshot"})).collect::<Vec<_>>())).is_err());
}

#[tokio::test]
async fn it_navigates_the_menu_fills_the_form_and_sees_what_is_wrong() {
    let dir = site();
    let actions = vec![
        Action::Open("index.html".into()),
        Action::Snapshot,
        Action::Click(Target::Text("Sobre".into())),
        Action::Text(None),
        Action::Back,
        Action::Click(Target::Text("Comprar".into())),
        Action::Fill(Target::Selector("#nome".into()), "Maria".into()),
        Action::Click(Target::Text("Enviar".into())),
        Action::Text(Some(Target::Selector("#out".into()))),
        Action::Click(Target::Text("Newsletter".into())),
        Action::Click(Target::Text("não existe esse botão".into())),
        Action::Snapshot,
    ];
    let Some((r, frames)) = browse(dir.path(), actions).await else { return };
    let log = r.log.join("\n");
    // It opened the page and listed the menu with references.
    assert!(log.contains("abriu file:///") && log.contains("Loja da Ana"), "{log}");
    assert!(log.contains("link \"Sobre\" → about.html") && log.contains("link \"Contato\" → #contato"), "{log}");
    assert!(log.contains("input \"Seu nome\"") || log.contains("Seu nome"), "{log}");
    // The menu works: it went to the other page, read it, and came back.
    assert!(log.contains("clicou em \"Sobre\" → file:///") && log.contains("about.html") && log.contains("Sobre nós"), "{log}");
    assert!(log.contains("Uma loja pequena."), "{log}");
    assert!(log.contains("voltou para file:///") && log.contains("index.html"), "{log}");
    // A button and a form behave.
    assert!(log.contains("clicou em \"Comprar\""), "{log}");
    assert!(log.contains("preencheu #nome com \"Maria\""), "{log}");
    assert!(log.contains("Olá, Maria"), "{log}");
    // What a person would notice.
    let problems = r.problems.join("\n");
    assert!(problems.contains("console.error: falhou ao carregar o carrinho"), "{problems}");
    assert!(problems.contains("nao-existe.png"), "{problems}");
    assert!(problems.contains("\"Newsletter\" cai em <div#banner>"), "the cookie banner sits on the button: {problems}");
    // A button that is not there ends the session with the reason.
    assert!(log.contains("ERRO: não achei {\"text\":\"não existe esse botão\"} na página"), "{log}");
    assert!(!log.contains("\n12."), "nothing runs after the failure: {log}");
    // The mini window gets a picture after every step, with the pointer.
    assert!(frames.len() >= 10, "{} frames", frames.len());
    assert!(frames.iter().all(|f| f.png.starts_with(&[0x89, b'P', b'N', b'G']) && f.size == (1000, 650)));
    let clicks: Vec<&Frame> = frames.iter().filter(|f| f.clicked).collect();
    assert!(clicks.len() >= 3 && clicks.iter().all(|f| f.cursor.is_some()), "pointer on every click");
    let moving: Vec<&Frame> = frames.iter().filter(|f| !f.clicked && f.cursor.is_some() && f.caption.starts_with("clicando")).collect();
    assert!(!moving.is_empty(), "the pointer is shown arriving before it clicks");
    let sobre = frames.iter().find(|f| f.caption.contains("Sobre") && f.clicked).unwrap();
    assert!(sobre.url.ends_with("about.html") && sobre.title == "Sobre nós", "{}", sobre.url);
    // The last state is handed back as a photo.
    assert!(r.shots.last().is_some_and(|p| p.starts_with(&[0x89, b'P', b'N', b'G'])));
}

#[tokio::test]
async fn it_uses_references_scrolls_and_resizes_for_a_phone() {
    let dir = site();
    let Some((r, _)) = browse(dir.path(), vec![Action::Open("index.html".into()), Action::Snapshot]).await else { return };
    let log = r.log.join("\n");
    let line = log.lines().find(|l| l.contains("link \"Contato\"")).expect("the menu is listed");
    let reference = line.split_whitespace().next().unwrap().to_string();
    assert!(reference.starts_with("@e"), "{line}");
    let actions = vec![
        Action::Open("index.html".into()),
        Action::Snapshot,
        Action::Viewport(390, 844),
        Action::Click(Target::Ref(reference)),
        Action::Scroll(300),
        Action::Press("Tab".into()),
        Action::Shot,
    ];
    let (r, frames) = browse(dir.path(), actions).await.unwrap();
    let log = r.log.join("\n");
    assert!(log.contains("janela em 390×844"), "{log}");
    assert!(log.contains("clicou em \"Contato\""), "{log}");
    assert!(log.contains("rolou até"), "{log}");
    assert!(log.contains("apertou Tab"), "{log}");
    assert!(frames.iter().any(|f| f.size == (390, 844)), "the phone frames carry their size");
    assert_eq!(r.shots.len(), 1, "the photo asked for is the last state, so the same picture is not kept twice");
}

#[tokio::test]
async fn the_browser_stays_open_between_calls_with_the_page_as_it_was() {
    let dir = site();
    let Some(path) = coturbo_design::render::find_browser() else { return };
    let mut browser = coturbo_browse::Browser::open(&path, (1000, 650)).await.expect("opens");
    let mut frames = Vec::new();
    // First call: open the page and fill the form, without sending it.
    let first = browser.run(dir.path(), &[Action::Open("index.html".into()), Action::Fill(Target::Selector("#nome".into()), "Joana".into())], &mut |f| frames.push(f)).await.unwrap();
    assert!(first.log.join("\n").contains("preencheu #nome com \"Joana\""), "{:?}", first.log);
    // Second call, no open: the page is still there with what was typed, so sending works.
    let second = browser.run(dir.path(), &[Action::Click(Target::Text("Enviar".into())), Action::Text(Some(Target::Selector("#out".into())))], &mut |f| frames.push(f)).await.unwrap();
    let log = second.log.join("\n");
    assert!(log.contains("clicou em \"Enviar\"") && log.contains("Olá, Joana"), "the second call continued the first: {log}");
    // And a third call sees the scroll and the page, not a blank one.
    let third = browser.run(dir.path(), &[Action::Scroll(300), Action::Snapshot], &mut |f| frames.push(f)).await.unwrap();
    let log = third.log.join("\n");
    assert!(log.contains("rolou até") && log.contains("Loja da Ana") && log.contains("link \"Sobre\""), "{log}");
    // Findings are per call: the console error of the page load is reported once, not again.
    assert!(first.problems.iter().any(|p| p.contains("falhou ao carregar o carrinho")), "{:?}", first.problems);
    assert!(!second.problems.iter().any(|p| p.contains("falhou ao carregar o carrinho")), "{:?}", second.problems);
    // The pictures of every call carry the texts of the page.
    assert!(frames.iter().any(|f| f.labels.iter().any(|l| l.text == "Bem-vinda" && l.kind == "h1")), "the headline is among the labels");
    assert!(frames.iter().any(|f| f.labels.iter().any(|l| l.text == "Comprar" && l.kind == "button" && l.w > 20.0 && l.y > 0.0)));
    browser.close().await;
}
