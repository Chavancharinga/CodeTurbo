//! The design contract: the compiled tokens written to `DESIGN.md`, with
//! the CSS custom properties the code must use, and read back for checks.

use crate::color::{Rgb, hex, parse_hex};
use crate::compiler::Tokens;
use std::collections::BTreeMap;
use std::path::Path;

pub const FILE: &str = "DESIGN.md";

fn font_stack(name: &str, display: bool, technical: bool) -> String {
    let generic = if name.contains("Mono") {
        "monospace"
    } else if display && !technical && (name.contains("Serif") || ["Fraunces", "Newsreader", "Cormorant Garamond", "Libre Caslon Display", "Gloock"].contains(&name)) {
        "serif"
    } else {
        "sans-serif"
    };
    format!("\"{name}\", {generic}")
}

/// The `:root { ... }` block with every token.
pub fn css(t: &Tokens) -> String {
    let technical = t.tone == crate::compiler::Tone::Technical;
    let mut s = String::from(":root {\n");
    for (n, c) in &t.colors {
        s.push_str(&format!("  --color-{n}: {};\n", hex(*c)));
    }
    s.push_str(&format!("  --font-display: {};\n", font_stack(&t.font_display, true, technical)));
    s.push_str(&format!("  --font-body: {};\n", font_stack(&t.font_body, false, technical)));
    for (n, rem) in &t.type_scale {
        s.push_str(&format!("  --text-{n}: {rem}rem;\n"));
    }
    for (i, px) in t.space.iter().enumerate() {
        s.push_str(&format!("  --space-{}: {px}px;\n", i + 1));
    }
    // Semantic aliases so the model's natural names (--space-md, --space-xl)
    // resolve too, instead of collapsing to an undefined value.
    let at = |i: usize| t.space.get(i).copied().unwrap_or(16);
    for (name, px) in [
        ("xs", at(1)),
        ("sm", at(2)),
        ("s", at(2)),
        ("md", at(3)),
        ("m", at(3)),
        ("lg", at(5)),
        ("l", at(5)),
        ("xl", at(6)),
        ("2xl", at(7)),
        ("3xl", at(8)),
    ] {
        s.push_str(&format!("  --space-{name}: {px}px;\n"));
    }
    for (n, px) in &t.radius {
        s.push_str(&format!("  --radius-{n}: {px}px;\n"));
    }
    for (n, v) in &t.shadows {
        s.push_str(&format!("  --shadow-{n}: {v};\n"));
    }
    s.push('}');
    s
}

/// `<link>` tags loading the two fonts.
pub fn font_link(t: &Tokens) -> String {
    let fam = |f: &str| format!("family={}:wght@400;600;700", f.replace(' ', "+"));
    format!(
        "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n<link href=\"https://fonts.googleapis.com/css2?{}&{}&display=swap\" rel=\"stylesheet\">",
        fam(&t.font_display),
        fam(&t.font_body)
    )
}

/// The contract file.
pub fn markdown(t: &Tokens) -> String {
    let b = &t.brief;
    format!(
        "# Contrato de design\n\n\
         Gerado pelo CodeTurbo. Todo código de interface usa somente estes tokens; \
         a verificação recusa cores, raios e espaçamentos fora deles e os padrões \
         genéricos de interface gerada por IA.\n\n\
         ## Resumo\n\n\
         - Assunto: {}\n- Público: {}\n- Tom: {} ({})\n- Elemento de assinatura: {}\n- Cores da marca preservadas: {}\n\n\
         ## Composição\n\n{}.\n\n\
         Evitar: sequência herói, recursos, depoimentos, preços, perguntas e contato; \
         grade de cartões idênticos com ícone, título e texto; ícones de emoji; \
         gradiente de duas cores; tudo centralizado; um único raio e uma única sombra em tudo.\n\n\
         ## Plano de composição\n\n{}\n\n\
         ## Fontes\n\n{}\n\n\
         ## Tokens\n\n```css\n{}\n```\n\n\
         ## Kit base\n\nJá cumpre a verificação: margens, tipografia fluida, foco, movimento reduzido e grade responsiva. \
         Use as classes .container, .section, .grid, .cluster, .btn, .panel e .tag e construa por cima.\n\n```css\n{}\n```\n\n\
         ## Ornamentos do projeto\n\nDesenhados para este projeto (cor via `color`). Motivo:\n\n```html\n{}\n```\n\nDivisor de seção:\n\n```html\n{}\n```\n",
        b.subject,
        b.audience,
        b.tone,
        t.tone.name(),
        b.signature,
        if b.brand.is_empty() { "nenhuma (paleta nova)".to_string() } else { b.brand.join(", ") }
            + &if b.fonts.is_empty() { String::new() } else { format!("\n- Fontes do projeto preservadas: {}", b.fonts.join(", ")) },
        t.layout,
        crate::kit::blueprint(t).iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("\n"),
        font_link(t),
        css(t),
        crate::kit::base_css(t),
        crate::kit::ornament_svg(t),
        crate::kit::divider_svg(t)
    )
}

/// The contract as read back from `DESIGN.md`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contract {
    pub vars: BTreeMap<String, String>,
}

impl Contract {
    pub fn parse(text: &str) -> Contract {
        let re = regex::Regex::new(r"(--[a-z0-9-]+)\s*:\s*([^;]+);").expect("valid regex");
        let vars = re.captures_iter(text).map(|c| (c[1].to_string(), c[2].trim().to_string())).collect();
        Contract { vars }
    }

    pub fn load(root: &Path) -> Option<Contract> {
        let text = std::fs::read_to_string(root.join(FILE)).ok()?;
        let c = Contract::parse(&text);
        (!c.vars.is_empty()).then_some(c)
    }

    pub fn colors(&self) -> Vec<Rgb> {
        self.vars.iter().filter(|(k, _)| k.starts_with("--color-")).filter_map(|(_, v)| parse_hex(v)).collect()
    }

    /// Pixel values of the tokens starting with `prefix`, smallest first.
    fn px(&self, prefix: &str) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .vars
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .filter_map(|(_, v)| v.trim_end_matches("px").parse().ok())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn space(&self) -> Vec<u32> {
        self.px("--space-")
    }

    pub fn radii(&self) -> Vec<u32> {
        self.px("--radius-")
    }
}
