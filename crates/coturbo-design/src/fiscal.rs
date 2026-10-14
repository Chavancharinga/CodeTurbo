//! The design inspector: deterministic rules run after every edit of an
//! interface file. It flags what makes a page look machine-made (default
//! fonts and palettes, emoji icons, identical card grids, the template
//! section order, one radius everywhere) and what breaks the contract or
//! accessibility. P0 and P1 block the task; P2 are advice.

use crate::color::{Rgb, contrast, hue_sat, parse_hex};
use crate::contract::Contract;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    P0,
    P1,
    P2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    pub rule: &'static str,
    pub line: usize,
    pub message: String,
}

impl Finding {
    pub fn blocking(&self) -> bool {
        self.severity <= Severity::P1
    }
}

/// Extensions the inspector reads.
pub fn is_ui_file(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".html", ".htm", ".css", ".scss", ".jsx", ".tsx", ".vue", ".svelte", ".astro"].iter().any(|e| p.ends_with(e))
}

const DEFAULT_FONTS: [&str; 14] = [
    "inter", "roboto", "arial", "helvetica", "helvetica neue", "segoe ui", "open sans", "poppins", "montserrat", "lato",
    "system-ui", "-apple-system", "sans-serif", "nunito",
];

/// Tailwind 400–700 shades of indigo, violet, purple, blue, emerald, green,
/// teal, sky and pink: the palette of almost every generated page.
const GENERIC_HEX: [&str; 26] = [
    "#6366f1", "#4f46e5", "#4338ca", "#818cf8", "#8b5cf6", "#7c3aed", "#a78bfa", "#a855f7", "#9333ea", "#3b82f6", "#2563eb",
    "#1d4ed8", "#60a5fa", "#10b981", "#059669", "#34d399", "#22c55e", "#16a34a", "#14b8a6", "#0d9488", "#0ea5e9", "#0284c7",
    "#ec4899", "#db2777", "#f43f5e", "#667eea",
];

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

static STYLE_BLOCK: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<style[^>]*>(.*?)</style>"));
static STYLE_ATTR: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)\bstyle\s*=\s*"([^"]*)""#));
static SCRIPT_BLOCK: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<script[^>]*>.*?</script>"));
static ROOT_BLOCK: LazyLock<Regex> = LazyLock::new(|| re(r"(?s):root\s*\{[^}]*\}"));
static RULE: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)([^{}]+)\{([^{}]*)\}"));
static DECL: LazyLock<Regex> = LazyLock::new(|| re(r"([a-zA-Z-]+)\s*:\s*([^;{}]+)(?:;|$)"));
static HEX: LazyLock<Regex> = LazyLock::new(|| re(r"#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b"));
static RGB_FN: LazyLock<Regex> = LazyLock::new(|| re(r"rgba?\(\s*(\d{1,3})[\s,]+(\d{1,3})[\s,]+(\d{1,3})"));
static VAR: LazyLock<Regex> = LazyLock::new(|| re(r"var\(\s*(--[a-z0-9-]+)"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)<h([1-6])[\s>]"));
static CLASS: LazyLock<Regex> = LazyLock::new(|| re(r#"(?i)<([a-z][a-z0-9-]*)\b[^>]*?\bclass(?:Name)?\s*=\s*["']([^"']+)["']"#));
static LANDMARK: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?is)<(?:section|header|footer|div|nav)\b[^>]*\b(?:id|class)\s*=\s*"([^"]*)"|<h2[^>]*>(.*?)</h2>"#));

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// A CSS region of the file and where it starts.
struct Region {
    start: usize,
    css: String,
}

fn css_regions(path: &str, text: &str) -> Vec<Region> {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".html") || p.ends_with(".htm") || p.ends_with(".vue") || p.ends_with(".svelte") || p.ends_with(".astro") {
        let mut out: Vec<Region> = STYLE_BLOCK
            .captures_iter(text)
            .filter_map(|c| c.get(1))
            .map(|m| Region { start: m.start(), css: m.as_str().to_string() })
            .collect();
        out.extend(STYLE_ATTR.captures_iter(text).filter_map(|c| c.get(1)).map(|m| Region { start: m.start(), css: format!("x{{{}}}", m.as_str()) }));
        out
    } else {
        vec![Region { start: 0, css: text.to_string() }]
    }
}

/// Markup with scripts and styles blanked out (offsets kept).
fn markup(path: &str, text: &str) -> Option<String> {
    let p = path.to_ascii_lowercase();
    if p.ends_with(".css") || p.ends_with(".scss") {
        return None;
    }
    let mut s = text.to_string();
    for r in [&*SCRIPT_BLOCK, &*STYLE_BLOCK] {
        for m in r.find_iter(text) {
            s.replace_range(m.range(), &" ".repeat(m.len()));
        }
    }
    Some(s)
}

fn literal_colors(v: &str) -> Vec<Rgb> {
    let mut out: Vec<Rgb> = HEX.find_iter(v).filter_map(|m| parse_hex(m.as_str())).collect();
    out.extend(RGB_FN.captures_iter(v).filter_map(|c| Some((c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?))));
    out
}

/// The color a declaration value names: a literal, or a `var(--x)` resolved
/// through `vars`.
fn resolve_color(v: &str, vars: &BTreeMap<String, String>) -> Option<Rgb> {
    if let Some(c) = literal_colors(v).first() {
        return Some(*c);
    }
    let name = VAR.captures(v)?.get(1)?.as_str().to_string();
    literal_colors(vars.get(&name)?).first().copied()
}

fn first_family(v: &str) -> String {
    v.split(',').next().unwrap_or("").trim().trim_matches(['"', '\'']).to_lowercase()
}

const TEMPLATE: [(&str, &[&str]); 7] = [
    ("herói", &["hero", "banner", "inicio", "início"]),
    ("recursos", &["feature", "recurso", "funcionalidade", "benef", "vantage"]),
    ("depoimentos", &["testimonial", "depoimento", "review", "avalia"]),
    ("preços", &["pricing", "price", "preço", "preco", "plano", "plans"]),
    ("perguntas", &["faq", "pergunta", "question", "dúvida", "duvida"]),
    ("contato", &["contact", "contato"]),
    ("rodapé", &["footer", "rodapé", "rodape"]),
];

/// Longest run of template sections found in template order.
fn template_order(markup: &str) -> Vec<&'static str> {
    let mut seq: Vec<usize> = Vec::new();
    for c in LANDMARK.captures_iter(markup) {
        let token = c.get(1).or(c.get(2)).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        if let Some(i) = TEMPLATE.iter().position(|(_, keys)| keys.iter().any(|k| token.contains(k)))
            && seq.last() != Some(&i)
            && !seq.contains(&i)
        {
            seq.push(i);
        }
    }
    // Longest increasing subsequence (n ≤ 7).
    let n = seq.len();
    let mut best: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        let mut b = Vec::new();
        for j in 0..i {
            if seq[j] < seq[i] && best[j].len() > b.len() {
                b = best[j].clone();
            }
        }
        b.push(seq[i]);
        best[i] = b;
    }
    best.into_iter().max_by_key(Vec::len).unwrap_or_default().into_iter().map(|i| TEMPLATE[i].0).collect()
}

const PLACEHOLDER_IMG: [&str; 6] = ["via.placeholder.com", "placehold.co", "placehold.it", "picsum.photos", "source.unsplash.com", "dummyimage.com"];

/// Marketing phrases every generated page repeats.
const CLICHES: [&str; 14] = [
    "leve seu", "próximo nível", "revolucione", "transforme sua", "transforme seu", "solução completa", "tudo o que você precisa",
    "take your", "next level", "unlock", "seamless", "supercharge", "elevate your", "all-in-one",
];

fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F000..=0x1F2FF | 0x2B50 | 0x2B06 | 0x2194..=0x21AA)
}

/// The colors a project already uses, most used first (near-identical
/// shades merged): the brand to keep when redesigning.
pub fn brand_colors(texts: &[(String, String)]) -> Vec<String> {
    let mut counts: Vec<(Rgb, usize)> = Vec::new();
    for (path, text) in texts {
        for r in css_regions(path, text) {
            for c in literal_colors(&r.css) {
                match counts.iter_mut().find(|(k, _)| (k.0 as i32 - c.0 as i32).abs() + (k.1 as i32 - c.1 as i32).abs() + (k.2 as i32 - c.2 as i32).abs() < 24) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((c, 1)),
                }
            }
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    counts.into_iter().take(5).map(|(c, _)| crate::color::hex(c)).collect()
}

/// Every finding for one file. `contract` is the project's `DESIGN.md`.
pub fn check(path: &str, text: &str, contract: Option<&Contract>) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |severity, rule, line, message: String| out.push(Finding { severity, rule, line, message });
    let regions = css_regions(path, text);

    // Custom properties: the contract's and the file's own `:root`.
    let mut vars: BTreeMap<String, String> = contract.map(|c| c.vars.clone()).unwrap_or_default();
    for r in &regions {
        for m in ROOT_BLOCK.find_iter(&r.css) {
            vars.extend(Contract::parse(m.as_str()).vars);
        }
    }
    let allowed: Vec<Rgb> = contract.map(Contract::colors).unwrap_or_default();

    let mut radii: Vec<(String, usize)> = Vec::new();
    let mut shadows: Vec<(String, usize)> = Vec::new();
    let mut centered = Vec::new();
    let mut literals = 0;
    let (mut has_focus, mut has_motion, mut reduced) = (false, None, false);
    let (mut off_space, mut off_radius) = (Vec::new(), Vec::new());
    let mut outline_none = None;
    let mut important = 0;
    let mut families: Vec<String> = Vec::new();
    let mut layout = false;
    let mut responsive = false;

    for r in &regions {
        let roots: Vec<_> = ROOT_BLOCK.find_iter(&r.css).map(|m| m.range()).collect();
        let in_root = |o: usize| roots.iter().any(|rg| rg.contains(&o));
        has_focus |= r.css.contains(":focus");
        reduced |= r.css.contains("prefers-reduced-motion");
        important += r.css.matches("!important").count();
        responsive |= r.css.contains("@media") || r.css.contains("clamp(") || r.css.contains("auto-fit") || r.css.contains("auto-fill") || r.css.contains("@container");
        for rule in RULE.captures_iter(&r.css) {
            let (sel, body) = (rule.get(1).expect("group"), rule.get(2).expect("group"));
            if in_root(body.start()) {
                continue;
            }
            let line = line_of(text, r.start + body.start());
            let selector = sel.as_str().trim();
            let mut fg = None;
            let mut bg = None;
            for d in DECL.captures_iter(body.as_str()) {
                let (prop, value) = (d[1].to_ascii_lowercase(), d[2].trim().to_string());
                let dline = line_of(text, r.start + body.start() + d.get(0).expect("match").start());
                let lits = literal_colors(&value);
                literals += lits.len();
                for c in &lits {
                    let h = crate::color::hex(*c);
                    if contract.is_some() {
                        if !allowed.contains(c) {
                            add(Severity::P1, "cor-fora-do-contrato", dline, format!("{h} não está no DESIGN.md; use var(--color-…)"));
                        }
                    } else if GENERIC_HEX.contains(&h.as_str()) {
                        add(Severity::P1, "paleta-generica", dline, format!("{h} é uma cor padrão do Tailwind, a paleta de toda página gerada"));
                    }
                }
                match prop.as_str() {
                    "font-family" | "font" => {
                        let resolved = VAR.captures(&value).and_then(|c| vars.get(&c[1]).cloned()).unwrap_or(value.clone());
                        let fam = first_family(&resolved);
                        if !fam.is_empty() && !families.contains(&fam) {
                            families.push(fam.clone());
                        }
                        if DEFAULT_FONTS.contains(&fam.as_str()) {
                            add(Severity::P1, "fonte-padrao", dline, format!("fonte `{fam}` é a padrão de interfaces genéricas; use a fonte do contrato"));
                        }
                    }
                    "color" => fg = resolve_color(&value, &vars),
                    "background" | "background-color" => {
                        bg = resolve_color(&value, &vars);
                        if value.contains("gradient(") {
                            // Resolve var(--x) so token colors count too.
                            let expanded = VAR.replace_all(&value, |c: &regex::Captures| vars.get(&c[1]).cloned().unwrap_or_default());
                            let cols = literal_colors(&expanded);
                            let chromatic: Vec<(f64, f64)> = cols.iter().map(|c| hue_sat(*c)).filter(|(_, s)| *s > 0.3).collect();
                            if chromatic.len() >= 2 {
                                let (a, b) = (chromatic[0].0, chromatic[chromatic.len() - 1].0);
                                if ((a - b + 540.0) % 360.0 - 180.0).abs() > 25.0 {
                                    add(Severity::P1, "gradiente-generico", dline, "gradiente entre duas cores saturadas: assinatura visual de página gerada por IA".into());
                                }
                            }
                        }
                    }
                    "border-radius" => {
                        if value != "0" && value != "50%" && value != "999px" && value != "9999px" {
                            radii.push((value.clone(), dline));
                        }
                        if let Some(c) = contract
                            && let Ok(px) = value.trim_end_matches("px").parse::<u32>()
                            && !c.radii().is_empty()
                            && !c.radii().contains(&px)
                            && value.ends_with("px")
                        {
                            off_radius.push(dline);
                        }
                    }
                    "box-shadow" if value != "none" => shadows.push((value.clone(), dline)),
                    "backdrop-filter" | "-webkit-backdrop-filter" if value.contains("blur") => {
                        add(Severity::P1, "vidro-fosco", dline, "vidro fosco (backdrop-filter: blur): clichê de interface gerada por IA".into());
                    }
                    "outline" if (value == "none" || value == "0") && !selector.contains("focus-visible") => {
                        outline_none.get_or_insert(dline);
                    }
                    "text-align" if value == "justify" => {
                        add(Severity::P2, "texto-justificado", dline, "texto justificado cria buracos entre palavras na web".into());
                    }
                    "font-size" => {
                        if let Some(px) = value.strip_suffix("px").and_then(|n| n.trim().parse::<f64>().ok())
                            && px < 14.0
                        {
                            add(Severity::P2, "texto-pequeno", dline, format!("fonte de {px}px: abaixo de 14px fica difícil de ler"));
                        }
                    }
                    "line-height" => {
                        if let Ok(lh) = value.parse::<f64>()
                            && lh < 1.3
                            && ["body", "p", "li"].iter().any(|t| selector.split([',', ' ']).any(|s| s == *t))
                        {
                            add(Severity::P2, "entrelinha", dline, format!("entrelinha {lh} no texto corrido; use pelo menos 1.4"));
                        }
                    }
                    "z-index" => {
                        if value.parse::<i64>().is_ok_and(|z| z >= 9999) {
                            add(Severity::P2, "z-index", dline, format!("z-index {value}: use uma escala pequena de camadas"));
                        }
                    }
                    "display" if value.contains("grid") || value.contains("flex") => layout = true,
                    "text-align" if value == "center" => centered.push(dline),
                    "transition" | "animation" => {
                        has_motion.get_or_insert(dline);
                    }
                    "transform" if selector.contains(":hover") && value.contains("translateY(-") => {
                        add(Severity::P2, "hover-elevacao", dline, "cartão que sobe no hover: efeito padrão de template".into());
                    }
                    p if p.starts_with("padding") || p.starts_with("margin") || p == "gap" => {
                        if let Some(c) = contract {
                            let scale = c.space();
                            for tok in value.split_whitespace() {
                                if let Some(px) = tok.strip_suffix("px").and_then(|n| n.parse::<u32>().ok())
                                    && px > 1
                                    && !scale.is_empty()
                                    && !scale.contains(&px)
                                {
                                    off_space.push(dline);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let (Some(f), Some(b)) = (fg, bg) {
                let ratio = contrast(f, b);
                if ratio < 4.5 {
                    add(Severity::P0, "contraste", line, format!("contraste {ratio:.1}:1 em `{selector}` (mínimo 4.5:1)"));
                }
            }
        }
    }

    if let Some(l) = outline_none
        && !regions.iter().any(|r| r.css.contains(":focus-visible"))
    {
        add(Severity::P1, "outline-removido", l, "outline removido sem estilo de :focus-visible: quem usa teclado se perde".into());
    }
    if important > 5 {
        add(Severity::P2, "important", 1, format!("{important} usos de !important: sinal de cascata desorganizada"));
    }
    let real: Vec<&String> = families.iter().filter(|f| !DEFAULT_FONTS.contains(&f.as_str()) && *f != "monospace" && *f != "serif").collect();
    if real.len() > 3 {
        add(Severity::P2, "fontes-demais", 1, format!("{} famílias de fonte; o contrato usa duas", real.len()));
    }
    if layout && !responsive && regions.iter().map(|r| r.css.len()).sum::<usize>() > 400 {
        add(Severity::P1, "sem-responsividade", 1, "layout em grid/flex sem @media, clamp() ou auto-fit: quebra no celular".into());
    }
    let uniform = |v: &[(String, usize)]| v.iter().map(|(s, _)| s.as_str()).collect::<std::collections::HashSet<_>>().len() == 1;
    if radii.len() >= 4 && uniform(&radii) {
        add(Severity::P1, "raio-unico", radii[0].1, format!("{} elementos com o mesmo raio `{}`: varie entre os raios do contrato", radii.len(), radii[0].0));
    }
    if shadows.len() >= 3 && uniform(&shadows) {
        add(Severity::P2, "sombra-unica", shadows[0].1, format!("{} elementos com a mesma sombra", shadows.len()));
    }
    if centered.len() >= 4 {
        add(Severity::P2, "tudo-centralizado", centered[0], format!("{} blocos centralizados: alinhe o texto corrido à esquerda", centered.len()));
    }
    if !off_space.is_empty() {
        add(Severity::P2, "espaco-fora-da-escala", off_space[0], format!("{} espaçamento(s) fora da escala do contrato; use var(--space-…)", off_space.len()));
    }
    if !off_radius.is_empty() {
        add(Severity::P2, "raio-fora-do-contrato", off_radius[0], format!("{} raio(s) fora do contrato; use var(--radius-…)", off_radius.len()));
    }
    if contract.is_none() && literals >= 4 {
        add(Severity::P1, "sem-contrato", 1, "interface sem DESIGN.md: chame design_brief antes de escrever estilos".into());
    }
    if let Some(l) = has_motion
        && !reduced
    {
        add(Severity::P2, "movimento", l, "animação sem `@media (prefers-reduced-motion: reduce)`".into());
    }

    if let Some(m) = markup(path, text) {
        if !has_focus && (m.contains("<button") || m.contains("<a ") || m.contains("<input")) && !regions.is_empty() {
            add(Severity::P1, "foco-ausente", 1, "elementos interativos sem estilo de `:focus-visible`".into());
        }
        let mut last = 0;
        let mut h1 = 0;
        for c in HEADING.captures_iter(&m) {
            let level: usize = c[1].parse().unwrap_or(1);
            let line = line_of(text, c.get(0).expect("match").start());
            if level == 1 {
                h1 += 1;
                if h1 == 2 {
                    add(Severity::P1, "titulos", line, "mais de um <h1> na página".into());
                }
            }
            if last > 0 && level > last + 1 {
                add(Severity::P1, "titulos", line, format!("<h{level}> logo após <h{last}>: nível de título pulado"));
            }
            last = level;
        }
        if let Some((i, c)) = m.char_indices().find(|(_, c)| is_emoji(*c)) {
            let n = m.chars().filter(|c| is_emoji(*c)).count();
            add(Severity::P1, "emoji-icone", line_of(text, i), format!("{n} emoji(s) usados como ícone (`{c}`): use ícones SVG ou elementos tipográficos"));
        }
        let mut classes: HashMap<String, (usize, usize)> = HashMap::new();
        for c in CLASS.captures_iter(&m) {
            // List items, links and inline text repeat by nature.
            if ["li", "ul", "ol", "dl", "span", "a", "p", "i", "b", "strong", "em", "svg", "img", "button", "td", "tr", "th", "option", "label", "input"]
                .contains(&c[1].to_ascii_lowercase().as_str())
            {
                continue;
            }
            let v = c[2].trim().to_string();
            let lower = v.to_lowercase();
            // Accordions and lists repeat by nature; cards in a grid are the cliché.
            let list = ["faq", "accordion", "acordeao", "pergunta", "question", "nav", "menu", "link"].iter().any(|k| lower.contains(k));
            if !list && ["card", "feature", "box", "tile", "plan", "testimonial", "depoimento", "recurso"].iter().any(|k| lower.contains(k)) {
                let e = classes.entry(v).or_insert((0, line_of(text, c.get(0).expect("match").start())));
                e.0 += 1;
            }
        }
        let mut repeated: Vec<_> = classes.into_iter().filter(|(_, (n, _))| *n >= 3).collect();
        repeated.sort_by_key(|(_, (_, l))| *l);
        for (class, (n, line)) in repeated.into_iter().take(2) {
            add(Severity::P1, "cartoes-identicos", line, format!("{n} blocos `{class}` idênticos: grade de cartões genérica; varie tamanho, conteúdo ou forma"));
        }
        let lower = m.to_lowercase();
        if let Some(i) = lower.find("lorem ipsum") {
            add(Severity::P1, "texto-de-preenchimento", line_of(text, i), "lorem ipsum: escreva o texto real".into());
        }
        if let Some(i) = PLACEHOLDER_IMG.iter().filter_map(|h| lower.find(h)).min() {
            add(Severity::P2, "imagem-de-preenchimento", line_of(text, i), "imagem de serviço de preenchimento: use imagem real ou ilustração própria".into());
        }
        let empty_links = lower.matches("href=\"#\"").count();
        if empty_links >= 3 {
            add(Severity::P2, "links-vazios", 1, format!("{empty_links} links com href=\"#\" sem destino"));
        }
        let cliches: Vec<&str> = CLICHES.iter().copied().filter(|c| lower.contains(c)).collect();
        if !cliches.is_empty() {
            add(Severity::P2, "texto-cliche", 1, format!("frases de efeito genéricas: {}", cliches.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ")));
        }
        let order = template_order(&m);
        if order.len() >= 5 {
            add(Severity::P2, "sequencia-template", 1, format!("seções na ordem de template ({}): reorganize em torno do elemento de assinatura", order.join(" → ")));
        }
    }
    out.sort_by_key(|f| (f.severity, f.line));
    out
}

/// Compact report, like a compiler's: one line per finding.
pub fn render(path: &str, findings: &[Finding]) -> String {
    if findings.is_empty() {
        return format!("design {path}: sem problemas");
    }
    let blocking = findings.iter().filter(|f| f.blocking()).count();
    let mut s = format!("design {path}: {blocking} bloqueante(s), {} aviso(s)", findings.len() - blocking);
    for f in findings.iter().take(25) {
        s.push_str(&format!("\n  {path}:{} [{:?} {}] {}", f.line, f.severity, f.rule, f.message));
    }
    s
}
