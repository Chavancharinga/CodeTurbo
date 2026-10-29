//! `cargo run -p coturbo-design --example shots <page.html>`: renders a page at
//! the four sizes and lists the screenshots it left.
#[tokio::main]
async fn main() {
    let page = std::path::PathBuf::from(std::env::args().nth(1).expect("page.html"));
    let browser = coturbo_design::render::find_browser().expect("a browser");
    let shot = page.with_extension("png");
    let t = std::time::Instant::now();
    let r = coturbo_design::render::measure(&browser, &page, &shot).await;
    println!(
        "measure: {:?} in {:.1}s",
        r.as_ref().map(|_| "ok"),
        t.elapsed().as_secs_f32()
    );
    for e in std::fs::read_dir(page.parent().unwrap()).unwrap().flatten() {
        println!(
            "  {} {}",
            e.file_name().to_string_lossy(),
            e.metadata().map(|m| m.len()).unwrap_or(0)
        );
    }
}
