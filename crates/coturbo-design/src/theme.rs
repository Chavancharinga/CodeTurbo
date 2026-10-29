//! Hallmark's themes, built into the engine: fifty materials (paper, ink,
//! faces, rules, radius, depth, controls, motion, voice), each a whole
//! visual language. A brief is dealt one by its subject, so a page never
//! falls back to the generic rounded look, and the theme's tokens become
//! the contract. Themes and layout come from Hallmark
//! (github.com/Nutlope/hallmark, MIT).

use crate::color::{Rgb, oklch, parse_hex};
use std::sync::LazyLock;

const FILES: [(&str, &str); 50] = [
    ("almanac", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/almanac.md")),
    ("arcade", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/arcade.md")),
    ("atelier", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/atelier.md")),
    ("bloom", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/bloom.md")),
    ("blueprint", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/blueprint.md")),
    ("broadside", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/broadside.md")),
    ("brutal", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/brutal.md")),
    ("carnival", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/carnival.md")),
    ("cobalt", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/cobalt.md")),
    ("console", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/console.md")),
    ("coral", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/coral.md")),
    ("darkroom", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/darkroom.md")),
    ("dose", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/dose.md")),
    ("dusk", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/dusk.md")),
    ("editorial", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/editorial.md")),
    ("formwork", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/formwork.md")),
    ("furrow", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/furrow.md")),
    ("garden", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/garden.md")),
    ("grid", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/grid.md")),
    ("halftone", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/halftone.md")),
    ("hedgerow", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/hedgerow.md")),
    ("hum", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/hum.md")),
    ("ink", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/ink.md")),
    ("kiln", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/kiln.md")),
    ("loom", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/loom.md")),
    ("lumen", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/lumen.md")),
    ("manifesto", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/manifesto.md")),
    ("midnight", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/midnight.md")),
    ("newsprint", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/newsprint.md")),
    ("overprint", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/overprint.md")),
    ("pasteup", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/pasteup.md")),
    ("piste", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/piste.md")),
    ("refract", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/refract.md")),
    ("ribbon", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/ribbon.md")),
    ("rim", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/rim.md")),
    ("riso", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/riso.md")),
    ("solstice", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/solstice.md")),
    ("specimen", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/specimen.md")),
    ("spore", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/spore.md")),
    ("sport", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/sport.md")),
    ("squeeze", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/squeeze.md")),
    ("sticker", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/sticker.md")),
    ("studio", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/studio.md")),
    ("terminal", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/terminal.md")),
    ("terrazzo", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/terrazzo.md")),
    ("ultraviolet", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/ultraviolet.md")),
    ("vantage", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/vantage.md")),
    ("vellum", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/vellum.md")),
    ("wayfind", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/wayfind.md")),
    ("werkstatt", include_str!("../../coturbo-tools/builtin/hallmark/references/themes/werkstatt.md")),
];

/// Hallmark's layout rules: sections, compositions, the hero, the page.
const LAYOUT: &str = include_str!("../../coturbo-tools/builtin/hallmark/references/layout.md");

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub name: String,
    pub slug: &'static str,
    /// editorial, modern-minimal, playful or atmospheric.
    pub genre: String,
    /// light, mid or dark.
    pub paper: String,
    pub display: String,
    pub posture: String,
    pub keywords: Vec<String>,
    pub material: String,
    pub photo: String,
    /// The file after its header: feel, tokens, type, surfaces, range,
    /// controls, motion, voice, do-nots, build hint.
    pub body: String,
    /// The custom properties of its tokens block, in order.
    pub vars: Vec<(String, String)>,
    /// The font `<link>` tags of its build hint.
    pub font_links: String,
    /// The CSS of its build hint.
    pub css: String,
}

fn header<'a>(head: &'a str, key: &str) -> &'a str {
    head.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':'))).map_or("", str::trim)
}

/// The first fenced block of `lang` after the heading `from`.
fn fenced<'a>(text: &'a str, from: &str, lang: &str) -> &'a str {
    let Some(at) = text.find(from) else { return "" };
    let rest = &text[at..];
    let open = format!("```{lang}\n");
    let Some(s) = rest.find(&open) else { return "" };
    let body = &rest[s + open.len()..];
    body.find("```").map_or("", |e| body[..e].trim_end())
}

fn parse(slug: &'static str, text: &str) -> Theme {
    // Checked out with Windows line ends or not, the same text.
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let (head, body) = match text.strip_prefix("---").and_then(|r| r.split_once("\n---")) {
        Some((h, b)) => (h, b.trim_start_matches('-').trim_start()),
        None => ("", text.as_str()),
    };
    let tokens = fenced(body, "## Tokens", "css");
    let decl = regex::Regex::new(r"(--[a-zA-Z0-9-]+)\s*:\s*([^;]+);").expect("valid regex");
    let vars = decl.captures_iter(tokens).map(|c| (c[1].to_string(), c[2].trim().to_string())).collect();
    Theme {
        name: header(head, "name").to_string(),
        slug,
        genre: header(head, "genre").to_string(),
        paper: header(head, "paper").split_whitespace().next().unwrap_or("light").to_string(),
        display: header(head, "display").to_string(),
        posture: header(head, "posture").to_string(),
        keywords: header(head, "keywords").split(',').map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect(),
        material: header(head, "material").to_string(),
        photo: header(head, "photo").to_string(),
        body: body.to_string(),
        vars,
        font_links: fenced(body, "## Build hint", "html").to_string(),
        css: fenced(body, "## Build hint", "css").to_string(),
    }
}

pub static THEMES: LazyLock<Vec<Theme>> = LazyLock::new(|| FILES.iter().map(|(s, t)| parse(s, t)).collect());

pub fn by_slug(slug: &str) -> Option<&'static Theme> {
    let s = slug.trim().to_lowercase();
    THEMES.iter().find(|t| t.slug == s || t.name.to_lowercase() == s)
}

/// Words of a Portuguese brief that route like the themes' English keywords.
const ALIASES: [(&str, &str); 34] = [
    ("manicure", "nails beauty salon craft workshop"),
    ("unha", "nails beauty salon craft"),
    ("beleza", "beauty salon wellness skincare"),
    ("estética", "beauty skincare wellness"),
    ("cabel", "salon beauty"),
    ("maquiag", "beauty cosmetics"),
    ("curso", "course workshop school"),
    ("aula", "course workshop class"),
    ("escola", "school education"),
    ("restaurante", "restaurant food menu"),
    ("comida", "food"),
    ("café", "cafe coffee"),
    ("padaria", "bakery food"),
    ("loja", "shop store"),
    ("moda", "fashion"),
    ("roupa", "fashion clothing"),
    ("joia", "jewellery luxury"),
    ("viagem", "travel"),
    ("hotel", "hotel hospitality"),
    ("pousada", "hotel hospitality stay"),
    ("música", "music"),
    ("banda", "music band record label"),
    ("festa", "festival events"),
    ("evento", "events"),
    ("academia", "fitness gym sport"),
    ("esporte", "sport"),
    ("saúde", "health wellness"),
    ("clínica", "health clinic"),
    ("imóve", "property real estate interiors"),
    ("arquitet", "architecture interiors"),
    ("artesan", "craft made by hand small-batch"),
    ("fazenda", "farm harvest"),
    ("tecnologia", "technology platform"),
    ("financ", "fintech payments"),
];

/// Hallmark's genre for a brief.
fn genre_of(text: &str) -> &'static str {
    let any = |keys: &[&str]| keys.iter().any(|k| text.contains(k));
    if any(&[" ai ", "generative", "music", "video", "voice", "atmospher", "cinema", "night", "noite"]) {
        "atmospheric"
    } else if any(&["saas", "enterprise", " api", "platform", "developer", "infra", "b2b", "dashboard", "technical", "técnic"]) {
        "modern-minimal"
    } else if any(&["playful", " fun", "friendly", "kids", "games", "community", "divert", "jovem", "lúdic", "infantil"]) {
        "playful"
    } else {
        "editorial"
    }
}

/// Deals the theme for a brief: the one whose keywords the brief names most,
/// or a close runner-up by `seed` so neighbouring prompts differ; a theme of
/// the brief's genre when no keyword fits.
pub fn deal(brief: &str, seed: u64) -> &'static Theme {
    let mut text = format!(" {} ", brief.to_lowercase());
    for (pt, en) in ALIASES {
        if text.contains(pt) {
            text.push_str(en);
            text.push(' ');
        }
    }
    let genre = genre_of(&text);
    let score = |t: &Theme| -> u32 {
        let named: u32 = t.keywords.iter().filter(|k| text.contains(&format!(" {k} ")) || text.contains(&format!(" {k}s "))).map(|k| if k.contains(' ') { 3 } else { 2 }).sum();
        named + u32::from(t.genre == genre)
    };
    let scored: Vec<(u32, &Theme)> = THEMES.iter().map(|t| (score(t), t)).collect();
    let best = scored.iter().map(|(s, _)| *s).max().unwrap_or(0);
    let pool: Vec<&Theme> = scored.iter().filter(|(s, _)| *s >= best.saturating_sub(1).max(1)).map(|(_, t)| *t).collect();
    pool[(seed % pool.len() as u64) as usize]
}

impl Theme {
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// A colour token as RGB.
    pub fn color(&self, name: &str) -> Option<Rgb> {
        color_value(self.var(name)?)
    }

    /// A length token in px (`0`, `3px`, `999px`).
    pub fn px(&self, name: &str) -> Option<u32> {
        let v = self.var(name)?.trim();
        if v == "0" {
            return Some(0);
        }
        v.strip_suffix("px")?.trim().parse::<f64>().ok().map(|p| p.round() as u32)
    }

    /// The first family of a font token, through `var(--font-display)`.
    pub fn font(&self, name: &str) -> Option<String> {
        let v = self.var(name)?.trim();
        if let Some(other) = v.strip_prefix("var(").and_then(|r| r.split([',', ')']).next()) {
            return (other.trim() != name).then(|| self.font(other.trim())).flatten();
        }
        let first = v.split(',').next()?.trim().trim_matches('"').trim_matches('\'');
        (!first.is_empty()).then(|| first.to_string())
    }

    pub fn dark(&self) -> bool {
        self.paper == "dark"
    }

    /// The theme's `:root` block, verbatim.
    pub fn root_css(&self) -> String {
        let mut s = String::from(":root {\n");
        for (n, v) in &self.vars {
            s.push_str(&format!("  {n}: {v};\n"));
        }
        s.push('}');
        s
    }

    /// The theme's photo treatment as a CSS filter.
    pub fn photo_filter(&self) -> String {
        let mut f: Vec<String> = Vec::new();
        let mut words = self.photo.split_whitespace();
        match words.next() {
            Some("warm") => f.push("sepia(0.12) saturate(1.05)".into()),
            Some("muted") => f.push("saturate(0.72)".into()),
            Some("mono" | "duotone") => f.push("grayscale(1)".into()),
            Some("tint") => f.push("grayscale(0.6)".into()),
            _ => {}
        }
        for w in words {
            let Some((k, v)) = w.split_once('=') else { continue };
            let Ok(n) = v.parse::<f64>() else { continue };
            let amount = 1.0 + n / 100.0;
            match k {
                "con" => f.push(format!("contrast({amount:.2})")),
                "sat" | "vib" => f.push(format!("saturate({amount:.2})")),
                "exp" => f.push(format!("brightness({amount:.2})")),
                "sepia" => f.push(format!("sepia({:.2})", n / 100.0)),
                _ => {}
            }
        }
        f.join(" ")
    }
}

/// `oklch(94.5% 0.010 76)`, `oklch(0.945 0.01 76 / 0.9)` or `#rrggbb`.
pub fn color_value(v: &str) -> Option<Rgb> {
    let v = v.trim();
    if let Some(inner) = v.strip_prefix("oklch(").and_then(|r| r.strip_suffix(')')) {
        let mut parts = inner.split('/').next()?.split_whitespace();
        let l = parts.next()?;
        let l = match l.strip_suffix('%') {
            Some(p) => p.parse::<f64>().ok()? / 100.0,
            None => l.parse::<f64>().ok()?,
        };
        let c = parts.next()?.parse::<f64>().ok()?;
        let h = parts.next()?.trim_end_matches("deg").parse::<f64>().ok()?;
        return Some(oklch(l, c, h));
    }
    parse_hex(v)
}

const SKILL: &str = include_str!("../../coturbo-tools/builtin/hallmark/SKILL.md");

/// Hallmark's Floor: the rules no page breaks, without the two that name
/// Hallmark's own stamp and lint script.
pub fn floor() -> String {
    let skill = SKILL.replace("\r\n", "\n");
    let from = skill.find("1. Every colour").unwrap_or(0);
    let to = skill.find("15. The first").unwrap_or(skill.len());
    skill[from..to].trim().to_string()
}

/// Hallmark's layout, sections 4 to 7: a section, the compositions, the
/// hero and the page (nav and footer included).
pub fn layout_rules() -> &'static str {
    let from = LAYOUT.find("## 4. A section").unwrap_or(0);
    let to = LAYOUT.find("## 8.").unwrap_or(LAYOUT.len());
    LAYOUT[from..to].trim()
}

const HEROES: [&str; 8] = ["Settled", "Stage", "Banner", "Ledge", "Corridor", "Poster", "Counter", "Index"];
const NAVS: [&str; 5] = [
    "edge-aligned: the wordmark and one or two links or the action at opposite edges",
    "a floating pill bar",
    "a masthead: a large centred wordmark over a rule",
    "a slab: a heavy full-width bar",
    "a three-part bar: the wordmark, four or more real destinations, the action",
];
const FOOTERS: [&str; 4] = [
    "a mast: the wordmark large over the real links and the legal line",
    "a statement: one closing sentence set large, the links beneath",
    "a colophon block: credits, photo credits and contact as one paragraph under a hairline",
    "a letter close: a sign-off line and the date, the links small beneath",
];
const COMPOSITIONS: [&str; 17] = [
    "Split", "Showcase", "Grid", "Bento", "Rows", "Table", "Figures", "Steps", "Statement", "Band", "Form", "Rail", "Pinned",
    "Switcher", "Index", "Schedule", "Offer",
];

/// What the page takes, dealt by `seed` as Hallmark's draw does: the hero
/// posture, the nav, the footer and two compositions to leave out.
pub fn picks(seed: u64, photos: bool) -> Vec<(&'static str, String)> {
    let at = |n: u64, len: usize| ((seed / n) % len as u64) as usize;
    // A page with photographs leads with them.
    let heroes: Vec<&str> = if photos { vec!["Stage", "Poster", "Settled", "Ledge"] } else { HEROES.to_vec() };
    let hero = heroes[at(3, heroes.len())];
    let a = at(11, COMPOSITIONS.len());
    let mut b = at(13, COMPOSITIONS.len());
    if b == a {
        b = (b + 5) % COMPOSITIONS.len();
    }
    vec![
        ("Hero posture", hero.to_string()),
        ("Nav", NAVS[at(5, NAVS.len())].to_string()),
        ("Footer", FOOTERS[at(7, FOOTERS.len())].to_string()),
        ("Leave out", format!("{} and {}", COMPOSITIONS[a], COMPOSITIONS[b])),
    ]
}
