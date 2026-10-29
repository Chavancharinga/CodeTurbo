//! Mostra o navegador do agente num terminal de verdade (com imagem real onde
//! o terminal desenha imagens). Uso: cargo run -p coturbo-cli --example painel_vivo

use coturbo_browse::{Action, Frame, Target};
use coturbo_cli::tui::app::{App, run};
use coturbo_cli::tui::{Out, Status};
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

#[tokio::main]
async fn main() {
    let site = std::env::temp_dir().join("codeturbo-painel-vivo");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), SITE).unwrap();
    let browser = coturbo_design::render::find_browser().expect("Edge ou Chrome");
    let actions = vec![
        Action::Open("index.html".into()),
        Action::Click(Target::Text("Adicionar ao carrinho".into())),
        Action::Fill(Target::Selector("#mail".into()), "ana@exemplo.com".into()),
        Action::Click(Target::Text("Fazer pedido".into())),
    ];
    let mut frames: Vec<Frame> = Vec::new();
    coturbo_browse::run(&browser, &site, &actions, (880, 560), &mut |f| frames.push(f)).await.expect("navegação");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (cancel, _crx) = tokio::sync::mpsc::unbounded_channel();
    let status = Status { provider: "openrouter".into(), model: "nemotron".into(), approval: "edição livre".into(), folder: "loja".into(), ..Status::default() };
    let app = App::new(status, tx, cancel, Arc::new(AtomicBool::new(false)));
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        let _ = out_tx.send(Out::Lines(vec![Line::from(vec![Span::raw("› teste o fluxo de compra da loja")])]));
        let _ = out_tx.send(Out::Busy(Some("browse".into())));
        for f in frames {
            let _ = out_tx.send(Out::Browser(f));
            std::thread::sleep(Duration::from_millis(1100));
        }
        std::thread::sleep(Duration::from_secs(14));
    });
    run(app, out_rx).unwrap();
}
