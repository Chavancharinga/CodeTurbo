//! Renders the CodeTurbo logo (wordmark and running snail) as an animated
//! SVG, straight from the terminal animation's pixels. Unlike a GIF, it
//! plays by itself on GitHub, with no play button, and stays sharp at any
//! size.
//!
//!     cargo run -p coturbo-cli --example logo_svg -- docs/codeturbo.svg

use coturbo_cli::splash::{LOOP_FRAMES, frames, pixels};
use std::collections::BTreeMap;
use std::fmt::Write;

const PAD: usize = 4;
const BG: &str = "#0d1117";
const FRAME_MS: usize = 50;

type Grid = Vec<Vec<Option<(u8, u8, u8)>>>;

fn hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// One `<path>` per color: horizontal runs of pixels as rectangles.
fn paths(px: &Grid, keep: impl Fn(usize, usize) -> bool) -> String {
    let mut by_color: BTreeMap<String, String> = BTreeMap::new();
    for (y, row) in px.iter().enumerate() {
        let mut x = 0;
        while x < row.len() {
            let Some(c) = row[x].filter(|_| keep(x, y)) else {
                x += 1;
                continue;
            };
            let start = x;
            while x < row.len() && row[x] == Some(c) && keep(x, y) {
                x += 1;
            }
            let d = by_color.entry(hex(c)).or_default();
            let _ = write!(d, "M{} {}h{}v1h-{}z", start + PAD, y + PAD, x - start, x - start);
        }
    }
    by_color.into_iter().map(|(c, d)| format!("<path fill=\"{c}\" d=\"{d}\"/>")).collect()
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "codeturbo.svg".into());
    let first = frames();
    let all: Vec<Grid> = (first..first + LOOP_FRAMES).map(|i| pixels(i, 120)).collect();
    let (h, w) = (all[0].len(), all[0][0].len());
    // Pixels identical in every frame (the wordmark) are drawn once.
    let fixed = |x: usize, y: usize| all.iter().all(|f| f[y][x] == all[0][y][x]);
    let total = LOOP_FRAMES * FRAME_MS;
    let step = 100.0 / LOOP_FRAMES as f64;
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {vw} {vh}\" width=\"{pw}\" height=\"{ph}\" \
         shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"CodeTurbo\">\
         <style>g{{visibility:hidden;animation:f {total}ms step-end infinite}}\
         @keyframes f{{0%{{visibility:visible}}{step:.4}%{{visibility:hidden}}}}</style>\
         <rect width=\"100%\" height=\"100%\" rx=\"2\" fill=\"{BG}\"/>",
        vw = w + 2 * PAD,
        vh = h + 2 * PAD,
        pw = (w + 2 * PAD) * 6,
        ph = (h + 2 * PAD) * 6,
    );
    svg.push_str(&paths(&all[0], fixed));
    for (k, f) in all.iter().enumerate() {
        let _ = write!(svg, "<g style=\"animation-delay:{}ms\">{}</g>", k * FRAME_MS, paths(f, |x, y| !fixed(x, y)));
    }
    svg.push_str("</svg>\n");
    std::fs::write(&out, &svg).expect("write svg");
    eprintln!("wrote {out} ({} KB, {LOOP_FRAMES} frames)", svg.len() / 1024);
}
