//! Prints how far "at 23:00" is from now, by the local clock.
#[tokio::main]
async fn main() {
    let dir = std::env::temp_dir().join("codeturbo-sched-check");
    let _ = std::fs::create_dir_all(&dir);
    let tools = coturbo_tools::Tools::new(&dir, coturbo_tools::Mode::Naive);
    let out = tools.call("schedule", &serde_json::json!({"action": "add", "when": "at 23:00", "task": "check"})).await;
    println!("{}", out.content);
    let _ = std::fs::remove_dir_all(&dir);
}
