//! `cargo run -p coturbo-tools --example ci`: the CI runs of this branch.
#[tokio::main]
async fn main() {
    let tools = coturbo_tools::Tools::new(&std::env::current_dir().unwrap(), coturbo_tools::Mode::Naive);
    let out = tools.call("ci", &serde_json::json!({"action": "status"})).await;
    println!("error={}\n{}", out.is_error, out.content);
}
