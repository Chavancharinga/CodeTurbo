//! Roda o REA num alvo real e imprime o relatório.
//! Uso: cargo run -q -p coturbo-rea --example investigar -- "<alvo>" "<pergunta>"

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let target = args.next().expect("uso: investigar <alvo> [pergunta]");
    let question = args
        .next()
        .unwrap_or_else(|| "o que este programa faz e como funciona".to_string());
    let root = std::env::current_dir().unwrap();
    match coturbo_rea::investigate(&root, &target, &question).await {
        Ok(report) => {
            println!("{}", report.text);
            if !report.images.is_empty() {
                println!("\n[imagens: {}]", report.images.len());
            }
        }
        Err(e) => eprintln!("erro: {e}"),
    }
}
