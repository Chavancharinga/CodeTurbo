//! The design compiler: the model's short brief becomes concrete tokens.
//! The model only decides what needs judgment (subject, audience, tone,
//! signature); colors, type, spacing, radii and shadows are computed, with
//! contrast checked and the usual AI defaults ruled out.

use crate::color::{Rgb, contrast, hex, hue_sat, luminance, oklch, parse_hex, to_oklch};

/// What the model decides before any UI code.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brief {
    pub subject: String,
    pub audience: String,
    pub tone: String,
    /// The one element that makes this interface recognizable.
    pub signature: String,
    /// Colors the brand already uses (`#rrggbb`), most important first. When
    /// present, the palette is organized around them instead of replaced.
    pub brand: Vec<String>,
    /// Fonts the project already uses (display first); kept when present.
    pub fonts: Vec<String>,
    /// Corner radii the project already uses, in px; kept when present.
    pub radii: Vec<u32>,
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
            Tone::Technical => "technical",
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
        // Bordeaux and rose through to ochre.
        Tone::Editorial => (-25.0, 75.0, 0.12),
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

/// Brand colors sorted into roles: the light one becomes the page, the dark
/// one the text, the saturated ones primary and accent.
struct BrandRoles {
    light: Option<Rgb>,
    dark: Option<Rgb>,
    chromatic: Vec<Rgb>,
}

fn brand_roles(colors: &[String]) -> Option<BrandRoles> {
    let parsed: Vec<Rgb> = colors.iter().filter_map(|c| parse_hex(c)).collect();
    if parsed.is_empty() {
        return None;
    }
    let mut roles = BrandRoles { light: None, dark: None, chromatic: Vec::new() };
    for c in parsed {
        let (_, sat) = hue_sat(c);
        let lum = luminance(c);
        if sat > 0.25 && (0.03..0.9).contains(&lum) {
            roles.chromatic.push(c);
        } else if lum >= 0.75 {
            roles.light.get_or_insert(c);
        } else if lum <= 0.05 {
            roles.dark.get_or_insert(c);
        } else if sat > 0.25 {
            roles.chromatic.push(c);
        }
    }
    Some(roles)
}

/// A color with the same hue, made dark (or light) enough to read on `bg`.
fn readable(c: Rgb, bg: Rgb, ratio: f64) -> Rgb {
    if contrast(c, bg) >= ratio {
        return c;
    }
    let (l, ch, h) = to_oklch(c);
    reach(l, ch, h, bg, ratio, luminance(bg) > 0.4)
}

/// White or near-black, whichever reads better on `c`.
fn on(c: Rgb) -> Rgb {
    let (_, _, h) = to_oklch(c);
    let light = oklch(0.97, 0.012, h);
    let dark = oklch(0.16, 0.02, h);
    if contrast(light, c) >= contrast(dark, c) { light } else { dark }
}

/// The palette built around the brand's own colors.
fn brand_palette(roles: &BrandRoles, chroma_hint: f64) -> Option<Vec<(&'static str, Rgb)>> {
    let primary = *roles.chromatic.first().or(roles.dark.as_ref())?;
    let (_, _, hue) = to_oklch(primary);
    let bg = roles.light.unwrap_or_else(|| oklch(0.985, 0.008, hue));
    let ink = match roles.dark {
        Some(d) if contrast(d, bg) >= 7.0 => d,
        _ => reach(0.22, 0.02, hue, bg, 10.0, true),
    };
    let white = (255, 255, 255);
    let on_primary = [ink, white, (0, 0, 0)].into_iter().max_by(|a, b| contrast(*a, primary).total_cmp(&contrast(*b, primary))).unwrap_or(ink);
    let accent = roles.chromatic.get(1).copied().unwrap_or_else(|| {
        let (l, c, h) = to_oklch(primary);
        oklch((l - 0.25).max(0.3), (c * 0.9).max(chroma_hint * 0.5), h)
    });
    let (pl, pc, _) = to_oklch(primary);
    let deep = oklch(pl.min(0.27), pc.max(chroma_hint * 0.6), hue);
    Some(vec![
        ("bg", bg),
        ("surface", oklch(0.955, 0.018, hue)),
        ("ink", ink),
        ("muted", reach(0.48, 0.015, hue, bg, 4.6, true)),
        ("line", oklch(0.86, 0.01, hue)),
        ("primary", primary),
        ("on-primary", on_primary),
        // The brand color as text on the page, darkened only if it must be.
        ("primary-ink", readable(primary, bg, 4.5)),
        ("accent", accent),
        // Whole sections in the brand: a dark one and a light band.
        ("deep", deep),
        ("on-deep", on(deep)),
        ("tint", oklch(0.93, (pc * 0.35).max(0.035), hue)),
        // A neutral scale for hierarchy, tinted toward the brand.
        ("gray-100", oklch(0.95, 0.008, hue)),
        ("gray-300", oklch(0.82, 0.01, hue)),
        ("gray-500", oklch(0.6, 0.012, hue)),
        ("gray-700", oklch(0.4, 0.014, hue)),
    ])
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
    let deep = oklch(if dark { 0.3 } else { 0.27 }, chroma * 0.8, hue);
    let tint = oklch(if dark { 0.27 } else { 0.92 }, chroma * 0.35, hue);
    let pairs = font_pairs(tone);
    let (picked_display, picked_body) = pairs[(seed / 7) as usize % pairs.len()];
    // The project's own fonts win over the catalog.
    let display = b.fonts.first().map(String::as_str).unwrap_or(picked_display);
    let body = b.fonts.get(1).or(b.fonts.first()).map(String::as_str).unwrap_or(picked_body);
    let ratio: f64 = match tone {
        Tone::Editorial => 1.414,
        Tone::Playful => 1.333,
        Tone::Technical => 1.2,
        Tone::Warm => 1.25,
        Tone::Balanced => 1.25,
    };
    let names = ["sm", "base", "lg", "xl", "2xl", "3xl", "display"];
    let type_scale = names.iter().enumerate().map(|(i, n)| (*n, (ratio.powi(i as i32 - 1) * 1000.0).round() / 1000.0)).collect();
    let mut own: Vec<u32> = b.radii.clone();
    own.sort_unstable();
    own.dedup();
    let radius = if own.len() >= 2 {
        let pick = |i: usize| own[(i * (own.len() - 1)) / 2];
        vec![("sm", pick(0)), ("md", pick(1)), ("lg", pick(2))]
    } else {
        match tone {
        Tone::Editorial => vec![("sm", 2), ("md", 8), ("lg", 20)],
        Tone::Technical => vec![("sm", 2), ("md", 6), ("lg", 12)],
        Tone::Playful => vec![("sm", 6), ("md", 14), ("lg", 32)],
        Tone::Warm => vec![("sm", 6), ("md", 14), ("lg", 28)],
        Tone::Balanced => vec![("sm", 6), ("md", 12), ("lg", 24)],
        }
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
        Tone::Editorial => "a narrow reading column (60–70 characters), large left-aligned headings, thin dividers, numerals and quotes in the spotlight; no card grid",
        Tone::Playful => "asymmetric composition, flat colour blocks, slightly overlapping and rotated elements, very large display type",
        Tone::Technical => "a visible rigid grid, monospace for data and labels, dense tables and lists instead of cards, high contrast",
        Tone::Warm => "sections with generous breathing room, images or illustrations with soft corners, left-aligned text, varied rhythm between sections",
        Tone::Balanced => "left alignment, one highlighted section breaking the grid, alternating widths between sections",
    };
    let brand = brand_roles(&b.brand).and_then(|r| brand_palette(&r, chroma));
    Tokens {
        tone,
        brief: b.clone(),
        colors: if let Some(p) = brand { p } else { vec![
            ("bg", bg),
            ("surface", surface),
            ("ink", ink),
            ("muted", muted),
            ("line", line),
            ("primary", primary),
            ("on-primary", on_primary),
            ("accent", accent),
            ("deep", deep),
            ("on-deep", on(deep)),
            ("tint", tint),
        ] },
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
