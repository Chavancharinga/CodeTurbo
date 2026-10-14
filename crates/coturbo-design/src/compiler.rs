//! The design compiler: the model's short brief becomes concrete tokens.
//! The model only decides what needs judgment (subject, audience, tone,
//! signature); colors, type, spacing, radii and shadows are computed, with
//! contrast checked and the usual AI defaults ruled out.

use crate::color::{Rgb, contrast, hex, oklch};

/// What the model decides before any UI code.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brief {
    pub subject: String,
    pub audience: String,
    pub tone: String,
    /// The one element that makes this interface recognizable.
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Editorial,
    Playful,
    Technical,
    Warm,
    Balanced,
}

impl Tone {
    /// From the tone the model described, in Portuguese or English.
    pub fn from_words(words: &str) -> Tone {
        let w = words.to_lowercase();
        let any = |keys: &[&str]| keys.iter().any(|k| w.contains(k));
        if any(&["editorial", "elegan", "luxo", "luxury", "sofistic", "sério", "serio", "serious", "clássic", "classic", "premium", "institucional"]) {
            Tone::Editorial
        } else if any(&["divert", "playful", "jovem", "young", "vibrant", "energ", "ousad", "bold", "lúdic", "fun", "criativ"]) {
            Tone::Playful
        } else if any(&["técnic", "tecnic", "technical", "minimal", "precis", "dev", "engenh", "dados", "data", "brutal"]) {
            Tone::Technical
        } else if any(&["acolhed", "warm", "calm", "natural", "orgân", "organic", "human", "suave", "gentle", "confi", "trust", "familiar"]) {
            Tone::Warm
        } else {
            Tone::Balanced
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Tone::Editorial => "editorial",
            Tone::Playful => "expressivo",
            Tone::Technical => "técnico",
            Tone::Warm => "acolhedor",
            Tone::Balanced => "equilibrado",
        }
    }
}

/// Display and body font pairs per tone, from Google Fonts. None of the
/// fonts every generated page uses (Inter, Roboto, Poppins, ...).
fn font_pairs(t: Tone) -> &'static [(&'static str, &'static str)] {
    match t {
        Tone::Editorial => &[("Fraunces", "Newsreader"), ("Instrument Serif", "Instrument Sans"), ("Cormorant Garamond", "Karla"), ("Libre Caslon Display", "Libre Franklin")],
        Tone::Playful => &[("Bricolage Grotesque", "Figtree"), ("Syne", "Hanken Grotesk"), ("Unbounded", "Onest"), ("Darker Grotesque", "Be Vietnam Pro")],
        Tone::Technical => &[("JetBrains Mono", "IBM Plex Sans"), ("Space Mono", "Archivo"), ("Chivo Mono", "Chivo"), ("IBM Plex Mono", "Instrument Sans")],
        Tone::Warm => &[("Fraunces", "Nunito Sans"), ("DM Serif Display", "DM Sans"), ("Young Serif", "Atkinson Hyperlegible"), ("Gloock", "Mulish")],
        Tone::Balanced => &[("Sora", "Public Sans"), ("Epilogue", "Albert Sans"), ("Schibsted Grotesk", "Source Sans 3"), ("Familjen Grotesk", "Red Hat Text")],
    }
}

/// OKLCH hues of the colors generated pages reach for (Tailwind indigo,
/// violet, blue, emerald, green): the compiler steers away from them.
const DEFAULT_HUES: [f64; 5] = [264.0, 277.0, 293.0, 150.0, 163.0];

/// Hue window and chroma of the main color per tone.
fn hue_range(t: Tone) -> (f64, f64, f64) {
    match t {
        Tone::Editorial => (15.0, 75.0, 0.12),
        Tone::Playful => (0.0, 360.0, 0.19),
        Tone::Technical => (180.0, 240.0, 0.11),
        Tone::Warm => (25.0, 110.0, 0.13),
        Tone::Balanced => (0.0, 360.0, 0.14),
    }
}

fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub tone: Tone,
    pub brief: Brief,
    /// `(name, color)`: bg, surface, ink, muted, line, primary, on-primary, accent.
    pub colors: Vec<(&'static str, Rgb)>,
    pub font_display: String,
    pub font_body: String,
    pub type_ratio: f64,
    /// `(name, rem)`.
    pub type_scale: Vec<(&'static str, f64)>,
    pub space: Vec<u32>,
    /// `(name, px)`.
    pub radius: Vec<(&'static str, u32)>,
    pub shadows: Vec<(&'static str, String)>,
    /// Layout guidance for the tone.
    pub layout: &'static str,
}

impl Tokens {
    pub fn color(&self, name: &str) -> Rgb {
        self.colors.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).unwrap_or((0, 0, 0))
    }
}

/// Darkens (or lightens) `l` until `fg` reaches `ratio` against `bg`.
fn reach(mut l: f64, c: f64, h: f64, bg: Rgb, ratio: f64, darker: bool) -> Rgb {
    loop {
        let col = oklch(l, c, h);
        if contrast(col, bg) >= ratio || !(0.02..=0.98).contains(&l) {
            return col;
        }
        l += if darker { -0.01 } else { 0.01 };
    }
}

pub fn compile(b: &Brief) -> Tokens {
    let tone = Tone::from_words(&format!("{} {}", b.tone, b.signature));
    let seed = hash(&format!("{}|{}|{}|{}", b.subject, b.audience, b.tone, b.signature));
    let (lo, hi, chroma) = hue_range(tone);
    // A hue in the tone's window, nudged out of the default ones.
    let mut hue = lo + (seed % 1000) as f64 / 1000.0 * (hi - lo);
    for _ in 0..12 {
        match DEFAULT_HUES.iter().find(|d| ((hue - **d + 540.0) % 360.0 - 180.0).abs() < 14.0) {
            Some(d) => hue = (d + 28.0) % 360.0,
            None => break,
        }
    }
    let dark = tone == Tone::Technical && seed.is_multiple_of(2);
    let (bg_l, surface_l) = if dark { (0.18, 0.23) } else { (0.975, 0.945) };
    let bg = oklch(bg_l, 0.012, hue);
    let surface = oklch(surface_l, 0.016, hue);
    let ink = reach(if dark { 0.93 } else { 0.24 }, 0.025, hue, bg, 10.0, !dark);
    let muted = reach(if dark { 0.75 } else { 0.48 }, 0.03, hue, bg, 4.6, !dark);
    let line = oklch(if dark { 0.32 } else { 0.87 }, 0.02, hue);
    let primary = reach(if dark { 0.72 } else { 0.52 }, chroma, hue, bg, 3.2, !dark);
    let white = (255, 255, 255);
    let on_primary = if contrast(primary, white) >= 4.5 { white } else { reach(0.18, 0.02, hue, primary, 4.5, true) };
    let offset = match tone {
        Tone::Editorial | Tone::Warm => 35.0 + (seed % 30) as f64,
        _ => 150.0 + (seed % 60) as f64,
    };
    let accent = oklch(if dark { 0.8 } else { 0.72 }, chroma * 1.1, (hue + offset) % 360.0);
    let pairs = font_pairs(tone);
    let (display, body) = pairs[(seed / 7) as usize % pairs.len()];
    let ratio: f64 = match tone {
        Tone::Editorial => 1.414,
        Tone::Playful => 1.333,
        Tone::Technical => 1.2,
        Tone::Warm => 1.25,
        Tone::Balanced => 1.25,
    };
    let names = ["sm", "base", "lg", "xl", "2xl", "3xl", "display"];
    let type_scale = names.iter().enumerate().map(|(i, n)| (*n, (ratio.powi(i as i32 - 1) * 1000.0).round() / 1000.0)).collect();
    let radius = match tone {
        Tone::Editorial => vec![("sm", 0), ("md", 2), ("lg", 4)],
        Tone::Technical => vec![("sm", 0), ("md", 3), ("lg", 6)],
        Tone::Playful => vec![("sm", 6), ("md", 14), ("lg", 32)],
        Tone::Warm => vec![("sm", 4), ("md", 10), ("lg", 22)],
        Tone::Balanced => vec![("sm", 3), ("md", 8), ("lg", 16)],
    };
    let ink_hex = hex(ink);
    let shadows = match tone {
        Tone::Editorial => vec![("1", "none".to_string()), ("2", format!("0 0 0 1px {}", hex(line)))],
        Tone::Technical => vec![("1", format!("0 0 0 1px {}", hex(line))), ("2", format!("4px 4px 0 {}", hex(line)))],
        Tone::Playful => vec![("1", format!("3px 3px 0 {ink_hex}")), ("2", format!("6px 6px 0 {ink_hex}"))],
        Tone::Warm | Tone::Balanced => vec![
            ("1", "0 1px 2px rgb(0 0 0 / 0.06)".to_string()),
            ("2", "0 18px 40px -24px rgb(0 0 0 / 0.35)".to_string()),
        ],
    };
    let layout = match tone {
        Tone::Editorial => "coluna de leitura estreita (60–70 caracteres), títulos grandes alinhados à esquerda, divisórias finas, numerais e citações em destaque; nada de grade de cartões",
        Tone::Playful => "composição assimétrica, blocos de cor chapada, elementos sobrepostos e rotacionados de leve, tipografia de exibição muito grande",
        Tone::Technical => "grade rígida visível, monoespaçada para dados e rótulos, tabelas e listas densas em vez de cartões, contraste alto",
        Tone::Warm => "seções com respiro largo, imagens ou ilustrações com cantos suaves, texto à esquerda, ritmo variado entre seções",
        Tone::Balanced => "alinhamento à esquerda, uma seção de destaque fora da grade, alternância de larguras entre seções",
    };
    Tokens {
        tone,
        brief: b.clone(),
        colors: vec![
            ("bg", bg),
            ("surface", surface),
            ("ink", ink),
            ("muted", muted),
            ("line", line),
            ("primary", primary),
            ("on-primary", on_primary),
            ("accent", accent),
        ],
        font_display: display.to_string(),
        font_body: body.to_string(),
        type_ratio: ratio,
        type_scale,
        space: vec![4, 8, 12, 16, 24, 32, 48, 64, 96, 128],
        radius,
        shadows,
        layout,
    }
}
