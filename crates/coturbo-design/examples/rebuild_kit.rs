//! Regrava codeturbo-kit.css de um projeto com o motor atual, mantendo os
//! tokens (cores/fontes) que já estão no DESIGN.md.
//! `cargo run -p coturbo-design --example rebuild_kit -- <root>`

use coturbo_design::{compiler, contract, kit};

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let root = std::path::Path::new(&root);
    let md = std::fs::read_to_string(root.join("DESIGN.md")).expect("DESIGN.md");
    // Descobre o tom escrito no contrato para o base_css; o resto vem dos tokens.
    let tone_word = md.lines().find(|l| l.contains("Tom:")).unwrap_or("");
    let c = contract::Contract::parse(&md);
    // Reconstrói tokens a partir das variáveis do contrato.
    let mut t = compiler::compile(&compiler::Brief { tone: tone_word.into(), ..Default::default() });
    t.colors = ["bg", "surface", "ink", "muted", "line", "primary", "on-primary", "primary-ink", "accent"]
        .iter()
        .filter_map(|n| {
            let v = c.vars.get(&format!("--color-{n}"))?;
            Some((*n, coturbo_design::color::parse_hex(v)?))
        })
        .collect();
    let root_block = md.split("```css").nth(1).and_then(|s| s.split("```").next()).unwrap_or("").trim().to_string();
    // Inject the semantic space aliases from the --space-N already present.
    let val = |n: usize| {
        root_block
            .lines()
            .find(|l| l.trim_start().starts_with(&format!("--space-{n}:")))
            .and_then(|l| l.split(':').nth(1))
            .map(|v| v.trim().trim_end_matches(';').to_string())
            .unwrap_or_else(|| format!("{}px", [4, 8, 12, 16, 24, 32, 48, 64, 96, 128][n.min(9) - 1]))
    };
    let aliases: String = [("xs", 2), ("sm", 3), ("s", 3), ("md", 4), ("m", 4), ("lg", 6), ("l", 6), ("xl", 7), ("2xl", 8), ("3xl", 9)]
        .iter()
        .map(|(name, n)| format!("  --space-{name}: {};\n", val(*n)))
        .collect();
    let root_block = root_block.replacen('}', &format!("{aliases}}}"), 1);
    let css = format!("/* Regravado pelo CodeTurbo. */\n{root_block}\n\n{}\n", kit::base_css(&t));
    std::fs::write(root.join("codeturbo-kit.css"), css).expect("write");
    println!("codeturbo-kit.css regravado ({} bytes)", std::fs::metadata(root.join("codeturbo-kit.css")).unwrap().len());
}
