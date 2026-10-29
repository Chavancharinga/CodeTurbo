//! `cargo run -p coturbo-tools --example web -- search <words>` or `fetch <url>`.
#[tokio::main]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let tools = coturbo_tools::Tools::new(&std::env::temp_dir(), coturbo_tools::Mode::Naive);
    let out = match a[0].as_str() {
        "search" => tools.call("web_search", &serde_json::json!({"query": a[1..].join(" ")})).await,
        _ => tools.call("web_fetch", &serde_json::json!({"url": a[1]})).await,
    };
    println!("error={}\n{}", out.is_error, out.content.chars().take(1500).collect::<String>());
}
