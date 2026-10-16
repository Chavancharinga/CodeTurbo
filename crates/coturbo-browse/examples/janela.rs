//! Abre a janela real do navegador e navega devagar numa página, para ver o
//! ponteiro do agente. Uso: cargo run -p coturbo-browse --example janela -- [url]

use coturbo_browse::{Action, Browser, Target};

const SITE: &str = r##"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8"><title>Casa do Café</title>
<style>
:root{--ink:#2a1f17;--paper:#f6efe6;--accent:#b4472b}
*{box-sizing:border-box}body{margin:0;font:17px Georgia,serif;color:var(--ink);background:var(--paper)}
header{display:flex;align-items:center;gap:22px;padding:16px 28px;border-bottom:2px solid var(--ink)}
header b{font-size:24px;letter-spacing:.04em}header a{color:var(--ink);text-decoration:none;font-size:16px}
main{padding:28px}
.foto{height:260px;background:linear-gradient(135deg,#d9b48f,#8a5a3a);border-radius:6px;margin-bottom:22px}
h1{margin:0 0 6px;font-size:32px}.preco{font-size:22px;color:var(--accent);margin:0 0 18px}
button{font:inherit;padding:11px 22px;border:2px solid var(--ink);background:#fff;cursor:pointer}
button.pri{background:var(--accent);color:#fff;border-color:var(--accent)}
label{display:block;margin:20px 0 6px;font-size:14px}input{font:inherit;padding:10px;width:100%;border:2px solid var(--ink);background:#fff}
#ok{margin-top:16px;color:#1f6b3a;min-height:24px}
</style></head><body>
<header><b>CASA DO CAFÉ</b><a href="#">Grãos</a><a href="#">Métodos</a><a href="#">Assinatura</a></header>
<main><div class="foto"></div>
<h1>Etiópia Yirgacheffe</h1><p class="preco">R$ 48,00 · 250 g</p>
<button onclick="document.getElementById('ok').textContent='1 item no carrinho'">Adicionar ao carrinho</button>
<label for="mail">Seu e-mail</label><input id="mail" placeholder="voce@exemplo.com">
<p><button class="pri" onclick="document.getElementById('ok').textContent='Pedido enviado para '+document.getElementById('mail').value">Fazer pedido</button></p>
<div id="ok"></div></main></body></html>"##;

#[tokio::main]
async fn main() {
    let site = std::env::temp_dir().join("codeturbo-janela-demo");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::write(site.join("index.html"), SITE).unwrap();
    let browser = coturbo_design::render::find_browser().expect("Edge ou Chrome");
    let mut b = Browser::open_with(&browser, (880, 560), true).await.expect("abre a janela");
    let url = std::env::args().nth(1).unwrap_or_else(|| "index.html".into());
    let steps: Vec<Vec<Action>> = vec![
        vec![Action::Open(url), Action::Wait(1200)],
        vec![Action::Click(Target::Text("Adicionar ao carrinho".into())), Action::Wait(700)],
        vec![Action::Fill(Target::Selector("#mail".into()), "ana@exemplo.com".into()), Action::Wait(500)],
        vec![Action::Click(Target::Text("Fazer pedido".into())), Action::Wait(9000)],
    ];
    for s in steps {
        let r = b.run(&site, &s, &mut |_| {}).await.expect("passo");
        eprintln!("{}", r.log.join(" | "));
    }
    b.close().await;
}
