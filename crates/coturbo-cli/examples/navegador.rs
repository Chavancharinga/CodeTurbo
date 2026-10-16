//! O agente navegando numa página de verdade, visto pela tela do CodeTurbo:
//! abre uma loja, clica, preenche e finaliza o pedido, e a mini janela mostra
//! o ponteiro a caminho e o clique. Grava duas imagens (HTML).
//!
//!     cargo run -p coturbo-cli --example navegador -- <pasta de saida>

#[path = "screen_html/mod.rs"]
mod screen_html;

use coturbo_browse::{Action, Frame, Target};
use coturbo_cli::tui::app::App;
use coturbo_cli::tui::{Out, Status};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const SITE: &str = r##"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><title>Casa do Café</title>
<style>
:root{--ink:#2a1f17;--paper:#f6efe6;--accent:#b4472b}
*{box-sizing:border-box}body{margin:0;font:17px Georgia,serif;color:var(--ink);background:var(--paper)}
header{display:flex;align-items:center;gap:28px;padding:18px 40px;border-bottom:2px solid var(--ink)}
header b{font-size:26px;letter-spacing:.04em}header a{color:var(--ink);text-decoration:none;font-size:16px}
main{display:grid;grid-template-columns:1.1fr 1fr;gap:40px;padding:40px}
.foto{height:320px;background:linear-gradient(135deg,#d9b48f,#8a5a3a);border-radius:6px}
h1{margin:0 0 6px;font-size:34px}.preco{font-size:22px;color:var(--accent);margin:0 0 18px}
button{font:inherit;padding:11px 22px;border:2px solid var(--ink);background:#fff;cursor:pointer}
button.pri{background:var(--accent);color:#fff;border-color:var(--accent)}
label{display:block;margin:22px 0 6px;font-size:14px}input{font:inherit;padding:10px;width:100%;border:2px solid var(--ink);background:#fff}
#ok{margin-top:16px;color:#1f6b3a;min-height:24px}
</style></head><body>
<header><b>CASA DO CAFÉ</b><a href="#">Grãos</a><a href="#">Métodos</a><a href="#">Assinatura</a></header>
<main><div class="foto"></div><div>
<h1>Etiópia Yirgacheffe</h1><p class="preco">R$ 48,00 · 250 g</p>
<button onclick="document.getElementById('ok').textContent='1 item no carrinho'">Adicionar ao carrinho</button>
<label for="mail">Seu e-mail</label><input id="mail" placeholder="voce@exemplo.com">
<p><button class="pri" onclick="document.getElementById('ok').textContent='Pedido enviado para '+document.getElementById('mail').value">Fazer pedido</button></p>
<div id="ok"></div></div></main></body></html>"##;

fn line(spans: Vec<Span<'static>>) -> Line<'static> {
    Line::from(spans)
}

fn tool(name: &str, rest: &str) -> Line<'static> {
    line(vec![
        Span::styled("  ● ", Style::new().fg(Color::Rgb(86, 204, 128))),
        Span::styled(name.to_string(), Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {rest}"), Style::new().fg(Color::Rgb(47, 123, 255))),
    ])
}

#[tokio::main]
async fn main() {
    let out = std::path::PathBuf::from(std::env::args().nth(1).expect("pasta de saída"));
    std::fs::create_dir_all(&out).unwrap();
    let site = std::env::temp_dir().join("codeturbo-navegador-demo");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), SITE).unwrap();
    let browser = coturbo_design::render::find_browser().expect("Edge ou Chrome");
    let actions = vec![
        Action::Open("index.html".into()),
        Action::Snapshot,
        Action::Click(Target::Text("Adicionar ao carrinho".into())),
        Action::Fill(Target::Selector("#mail".into()), "ana@exemplo.com".into()),
        Action::Click(Target::Text("Fazer pedido".into())),
    ];
    let mut frames: Vec<Frame> = Vec::new();
    let report = coturbo_browse::run(&browser, &site, &actions, (880, 560), &mut |f| frames.push(f)).await.expect("navegação");
    eprintln!("{}\nproblemas: {:?}\n{} quadros", report.log.join("\n"), report.problems, frames.len());

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (cancel, _crx) = tokio::sync::mpsc::unbounded_channel();
    let status = Status {
        provider: "claude-code".into(),
        model: "sonnet".into(),
        approval: "edição livre".into(),
        usage: "tokens ↑8.2k ⟳4.0k ↓1.1k".into(),
        folder: r"C:\Users\danie\Music\CoTurbon\sandbox\loja".into(),
        ..Status::default()
    };
    let mut app = App::new(status, tx, cancel, Arc::new(AtomicBool::new(false)));
    app.apply(Out::Lines(vec![
        line(vec![Span::raw("› teste o fluxo de compra da loja")]),
        Line::default(),
        tool("browse", "· 5 step(s)"),
        Line::default(),
        line(vec![Span::styled("    1. abriu index.html — \"Casa do Café\"", Style::new().fg(Color::Rgb(110, 125, 150)))]),
        line(vec![Span::styled("    2. @e1 link \"Grãos\" · @e4 button \"Adicionar ao carrinho\" · @e5 input", Style::new().fg(Color::Rgb(110, 125, 150)))]),
        line(vec![Span::styled("    3. clicou em \"Adicionar ao carrinho\"", Style::new().fg(Color::Rgb(110, 125, 150)))]),
    ]));
    app.apply(Out::Busy(Some("browse · 5 step(s)".into())));
    let click = |c: &Frame| c.caption.contains("Fazer pedido");
    // The last click: the pointer on its way, then the press.
    let arriving = frames.iter().position(|f| click(f) && !f.clicked).expect("o ponteiro chegando");
    let pressed = frames.iter().position(|f| click(f) && f.clicked).expect("o clique");
    app.apply(Out::Browser(frames[arriving - 1].clone()));
    std::thread::sleep(Duration::from_millis(500));
    app.apply(Out::Browser(frames[arriving].clone()));
    std::thread::sleep(Duration::from_millis(170));
    let mut term = Terminal::new(TestBackend::new(124, 34)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    std::fs::write(out.join("a_caminho.html"), screen_html::html(term.backend().buffer())).unwrap();
    app.apply(Out::Browser(frames[pressed].clone()));
    std::thread::sleep(Duration::from_millis(90));
    term.draw(|f| app.draw(f)).unwrap();
    std::fs::write(out.join("clique.html"), screen_html::html(term.backend().buffer())).unwrap();
    // The opening, in four instants after the first picture.
    let (tx2, _r2) = tokio::sync::mpsc::unbounded_channel();
    let (c2, _c2r) = tokio::sync::mpsc::unbounded_channel();
    let status2 = Status { provider: "claude-code".into(), model: "sonnet".into(), approval: "edição livre".into(), folder: r"C:\Users\danie\Music\CoTurbon\sandbox\loja".into(), ..Status::default() };
    let mut film = App::new(status2, tx2, c2, Arc::new(AtomicBool::new(false)));
    film.apply(Out::Lines(vec![
        line(vec![Span::raw("› teste o fluxo de compra da loja")]),
        Line::default(),
        tool("browse", "· 5 step(s)"),
        Line::default(),
        line(vec![Span::styled("    1. abriu index.html — \"Casa do Café\"", Style::new().fg(Color::Rgb(110, 125, 150)))]),
    ]));
    film.apply(Out::Busy(Some("browse · 5 step(s)".into())));
    std::thread::sleep(Duration::from_millis(1800));
    film.apply(Out::Browser(frames[0].clone()));
    let mut strip = String::from("<!doctype html><meta charset=utf-8><body style='margin:0;background:#0d1117;display:grid;grid-template-columns:1fr 1fr'>");
    let mut term2 = Terminal::new(TestBackend::new(110, 22)).unwrap();
    let mut last = 0u64;
    for at in [120u64, 260, 400, 640] {
        std::thread::sleep(Duration::from_millis(at - last));
        last = at;
        term2.draw(|f| film.draw(f)).unwrap();
        strip.push_str(&format!(
            "<div style='zoom:.62'><div style='color:#8a96ab;font:20px Consolas;padding:4px 10px'>+{at} ms</div>{}</div>",
            screen_html::fragment(term2.backend().buffer())
        ));
    }
    std::fs::write(out.join("abrindo.html"), strip).unwrap();
    eprintln!("gravado em {}", out.display());
}
