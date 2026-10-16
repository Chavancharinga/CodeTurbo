//! Captura a janela de um programa rodando.
//! Uso: cargo run -q -p coturbo-rea --example observar -- "<exe>" "<saida.png>" [titulo]

#[tokio::main]
async fn main() {
    let mut a = std::env::args().skip(1);
    let target = a.next().expect("uso: observar <exe> <saida.png> [titulo]");
    let out = std::path::PathBuf::from(a.next().expect("faltou a saída .png"));
    let title = a.next();
    match coturbo_rea::observe::observe(&target, title.as_deref(), 10, true, &out).await {
        Ok(s) => println!("ok: {} -> {}", s.info, out.display()),
        Err(e) => eprintln!("erro: {e}"),
    }
}
