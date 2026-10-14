//! `cargo run -p coturbo-rea --example investigate -- <alvo> [pergunta]`

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let target = args.next().unwrap_or_default();
    let question = args.collect::<Vec<_>>().join(" ");
    let root = std::env::current_dir().unwrap();
    match coturbo_rea::investigate(&root, &target, &question).await {
        Ok(r) => println!("{}\n(imagens: {:?})", r.text, r.images),
        Err(e) => println!("erro: {e}"),
    }
}
