fn main() {
    let path = std::env::args().nth(1).expect("usage: skel <file.rs>");
    let src = std::fs::read_to_string(path).unwrap();
    let s = coturbo_ast::skeleton(&src);
    eprintln!("original: {} bytes, skeleton: {} bytes ({:.0}% smaller)",
        src.len(), s.len(), 100.0 * (1.0 - s.len() as f64 / src.len() as f64));
}
