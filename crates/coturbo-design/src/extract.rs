//! The design system a project already has, read from its code and the
//! team's design files: CSS (and the styles inside HTML, JSX, Vue and
//! Svelte), CSS custom properties, Tailwind configuration and design token
//! JSON. A redesign starts from it instead of replacing it.

use crate::color::{hex, hue_sat, luminance, parse_hex};
use crate::fiscal;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Existing {
    /// Colors by use, most used first (near shades merged).
    pub colors: Vec<String>,
    /// Font families that are not generic defaults, most used first.
    pub fonts: Vec<String>,
    /// Corner radii in px, most used first.
    pub radii: Vec<u32>,
    /// Spacing values in px, most used first.
    pub spacing: Vec<u32>,
    /// Custom properties the project defines (`--name: value`).
    pub tokens: Vec<(String, String)>,
    /// UI frameworks in use (Tailwind, shadcn/ui, MUI, ...).
    pub frameworks: Vec<String>,
    /// Files the system was read from.
    pub sources: Vec<String>,
}

impl Existing {
    pub fn is_empty(&self) -> bool {
        self.colors.is_empty() && self.fonts.is_empty() && self.tokens.is_empty()
    }

    /// The main colors sorted for roles: chromatic first, then the light and
    /// dark ones (page and text).
    pub fn brand(&self) -> Vec<String> {
        let mut chroma = Vec::new();
        let mut rest = Vec::new();
        for c in &self.colors {
            let Some(rgb) = parse_hex(c) else { continue };
            if hue_sat(rgb).1 > 0.25 && (0.03..0.9).contains(&luminance(rgb)) { chroma.push(c.clone()) } else { rest.push(c.clone()) }
        }
        chroma.truncate(3);
        rest.truncate(2);
        chroma.extend(rest);
        chroma
    }
}

const GENERIC: [&str; 16] = [
    "sans-serif", "serif", "monospace", "system-ui", "-apple-system", "blinkmacsystemfont", "inherit", "initial", "cursive",
    "ui-sans-serif", "ui-serif", "ui-monospace", "arial", "helvetica", "segoe ui", "apple color emoji",
];

fn bump<K: Ord>(m: &mut BTreeMap<K, usize>, k: K) {
    *m.entry(k).or_insert(0) += 1;
}

fn ranked<K: Ord + Clone>(m: BTreeMap<K, usize>, take: usize) -> Vec<K> {
    let mut v: Vec<(K, usize)> = m.into_iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    v.into_iter().take(take).map(|(k, _)| k).collect()
}

fn px(v: &str) -> Option<u32> {
    v.trim().strip_suffix("px")?.parse::<f64>().ok().map(|f| f.round() as u32)
}

/// Reads the design system from `files` (`(relative path, text)`).
pub fn from_files(files: &[(String, String)]) -> Existing {
    let mut fonts = BTreeMap::new();
    let mut radii = BTreeMap::new();
    let mut spacing = BTreeMap::new();
    let mut tokens = BTreeMap::new();
    let mut frameworks = Vec::new();
    let mut sources = Vec::new();
    let font_link = regex::Regex::new(r"family=([A-Za-z0-9+]+)").expect("valid regex");
    let quoted_font = regex::Regex::new(r#"(?i)font(?:Family)?\s*[:=]\s*\{?[^}\n]*?["']([A-Z][A-Za-z0-9 ]+)["']"#).expect("valid regex");
    let json_color = regex::Regex::new(r#"["']?([a-zA-Z0-9_-]+)["']?\s*:\s*["'](#[0-9a-fA-F]{3,6})["']"#).expect("valid regex");
    let mut style_texts: Vec<(String, String)> = Vec::new();
    for (path, text) in files {
        let lower = path.to_ascii_lowercase();
        let name = lower.rsplit(['/', '\\']).next().unwrap_or(&lower).to_string();
        let mut used = false;
        if name.starts_with("tailwind.config") {
            frameworks.push("Tailwind CSS".to_string());
            for c in quoted_font.captures_iter(text) {
                bump(&mut fonts, c[1].trim().to_string());
            }
            for c in json_color.captures_iter(text) {
                tokens.insert(format!("--{}", &c[1]), c[2].to_lowercase());
            }
            used = true;
        } else if name == "package.json" {
            for (dep, label) in [
                ("tailwindcss", "Tailwind CSS"),
                ("@radix-ui", "Radix UI / shadcn/ui"),
                ("@mui/", "Material UI"),
                ("@chakra-ui", "Chakra UI"),
                ("styled-components", "styled-components"),
                ("bootstrap", "Bootstrap"),
            ] {
                if text.contains(dep) && !frameworks.iter().any(|f| f == label) {
                    frameworks.push(label.to_string());
                }
            }
        } else if lower.ends_with(".json") && (name.contains("token") || name.contains("theme") || name.contains("design")) {
            for c in json_color.captures_iter(text) {
                tokens.insert(format!("--{}", &c[1]), c[2].to_lowercase());
            }
            used = !tokens.is_empty();
        } else if fiscal::is_ui_file(path) {
            for c in font_link.captures_iter(text) {
                bump(&mut fonts, c[1].replace('+', " "));
            }
            style_texts.push((path.clone(), text.clone()));
            used = true;
        }
        if used {
            sources.push(path.clone());
        }
    }
    for (path, text) in &style_texts {
        for (prop, value) in fiscal::declarations(path, text) {
            match prop.as_str() {
                p if p.starts_with("--") => {
                    tokens.insert(p.to_string(), value.clone());
                }
                "font-family" => {
                    for fam in value.split(',') {
                        let f = fam.trim().trim_matches(['"', '\'']).to_string();
                        if !f.is_empty() && !f.starts_with("var(") && !GENERIC.contains(&f.to_lowercase().as_str()) {
                            bump(&mut fonts, f);
                            break;
                        }
                    }
                }
                "border-radius" => {
                    if let Some(v) = px(&value).filter(|v| *v > 0 && *v < 200) {
                        bump(&mut radii, v);
                    }
                }
                p if p.starts_with("padding") || p.starts_with("margin") || p == "gap" || p.ends_with("-gap") => {
                    for tok in value.split_whitespace() {
                        if let Some(v) = px(tok).filter(|v| *v >= 2 && *v <= 160) {
                            bump(&mut spacing, v);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // Colors: everything the styles use, plus colors defined as tokens.
    let mut colors = fiscal::brand_colors(&style_texts);
    for v in tokens.values() {
        if let Some(c) = parse_hex(v).map(hex)
            && !colors.contains(&c)
        {
            colors.push(c);
        }
    }
    colors.truncate(8);
    Existing {
        colors,
        fonts: ranked(fonts, 3),
        radii: ranked(radii, 4),
        spacing: ranked(spacing, 8),
        tokens: tokens.into_iter().take(40).collect(),
        frameworks,
        sources,
    }
}

/// Walks the project (respecting .gitignore) and reads its design system.
pub fn scan(root: &Path) -> Existing {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root).build();
    for entry in walker.filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(root).unwrap_or(entry.path()).to_string_lossy().replace('\\', "/");
        if rel.starts_with(".codeturbo") || rel.contains("node_modules/") {
            continue;
        }
        let lower = rel.to_ascii_lowercase();
        let wanted = fiscal::is_ui_file(&rel)
            || lower.ends_with("package.json")
            || lower.rsplit('/').next().is_some_and(|n| n.starts_with("tailwind.config"))
            || (lower.ends_with(".json") && ["token", "theme", "design"].iter().any(|k| lower.contains(k)));
        if !wanted || files.len() >= 80 {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path()) {
            files.push((rel, text));
        }
    }
    from_files(&files)
}

/// What was found, for the model and the contract.
pub fn summary(e: &Existing) -> String {
    let mut lines = Vec::new();
    if !e.colors.is_empty() {
        lines.push(format!("colors: {}", e.colors.join(", ")));
    }
    if !e.fonts.is_empty() {
        lines.push(format!("fonts: {}", e.fonts.join(", ")));
    }
    if !e.radii.is_empty() {
        lines.push(format!("radii: {}", e.radii.iter().map(|r| format!("{r}px")).collect::<Vec<_>>().join(", ")));
    }
    if !e.spacing.is_empty() {
        lines.push(format!("most used spacings: {}", e.spacing.iter().map(|r| format!("{r}px")).collect::<Vec<_>>().join(", ")));
    }
    if !e.tokens.is_empty() {
        lines.push(format!("{} design variables already defined", e.tokens.len()));
    }
    if !e.frameworks.is_empty() {
        lines.push(format!("libraries: {}", e.frameworks.join(", ")));
    }
    if !e.sources.is_empty() {
        lines.push(format!("read from: {}", e.sources.iter().take(6).cloned().collect::<Vec<_>>().join(", ")));
    }
    lines.join("\n")
}
