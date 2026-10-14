//! Reviews interface files with the design inspector and the rendered
//! measurements: `cargo run -p coturbo-design --example inspect -- <root> <file>...`.

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let files: Vec<String> = args.collect();
    println!("navegador: {:?}", coturbo_design::render::find_browser());
    match coturbo_design::review(&root, &files).await {
        Some(r) => println!("{}", r.summary),
        None => println!("nenhum arquivo de interface"),
    }
}
