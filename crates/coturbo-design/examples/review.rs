//! `cargo run -p coturbo-design --example review <root> <page.html>`: the
//! design review a task ends with, printed.
#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().expect("project root"));
    let page = args.next().expect("page.html");
    match coturbo_design::review(&root, &[page]).await {
        Some(r) => println!("ok={}\n{}", r.ok, r.summary),
        None => println!("not an interface file"),
    }
}
