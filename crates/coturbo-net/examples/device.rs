//! `cargo run -p coturbo-net --example device <name or address> [status]`:
//! scans, then shows what the device is and its status (read only).
#[tokio::main]
async fn main() {
    let target = std::env::args().nth(1).expect("a device name or address");
    let _ = coturbo_net::scan(coturbo_net::ScanOptions::default()).await;
    let (ip, mac) = coturbo_net::registry::resolve(&target).expect("a known device");
    let kind = coturbo_net::home::detect(ip).await;
    println!("{ip}: {} (actions: {})", kind.label(), kind.actions().join(", "));
    println!("{:?}", coturbo_net::home::control(ip, mac, coturbo_net::home::Act::Status, None).await);
}
