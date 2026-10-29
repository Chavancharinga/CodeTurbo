//! Prints how many bytes the tool definitions take in every request.
fn main() {
    let t = coturbo_tools::Tools::new(&std::env::current_dir().unwrap(), coturbo_tools::Mode::Ast);
    let specs = t.specs();
    let mut rows: Vec<(usize, String)> = specs
        .iter()
        .map(|s| {
            (
                s.description.len() + s.input_schema.to_string().len() + s.name.len(),
                s.name.clone(),
            )
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let total: usize = rows.iter().map(|r| r.0).sum();
    println!(
        "{} tools, {total} bytes (~{} tokens)",
        rows.len(),
        total / 4
    );
    for (n, name) in rows.iter().take(45) {
        println!("{n:6} {name}");
    }
}
