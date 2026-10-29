//! `cargo run -p coturbo-net --example scan [sweep]`: lists the devices on this network.
#[tokio::main]
async fn main() {
    let sweep = std::env::args().any(|a| a == "sweep");
    let started = std::time::Instant::now();
    let inv = coturbo_net::scan(coturbo_net::ScanOptions {
        sweep,
        ..Default::default()
    })
    .await;
    print!("{}", coturbo_net::render(&inv));
    println!("\n({:.1}s)", started.elapsed().as_secs_f32());
}
