//! Renders the CodeTurbo logo as a looping GIF that plays every mood of the
//! snail in turn, each with its name under it: calm, running, running hard,
//! turbo, alert and denied. Straight from the interface's own pixels.
//!
//!     cargo run -p coturbo-cli --example logo_gif -- docs/codeturbo.gif
//!
//! A second argument (0..5) renders only that mood, to check it by eye.

use coturbo_cli::splash::{Mood, frames, pixels_mood};
use std::fs::File;

const SCALE: usize = 5;
const PAD: usize = 4;
const BG: (u8, u8, u8) = (13, 17, 23);
const MUTED: (u8, u8, u8) = (130, 145, 170);
/// Frames each mood plays (50 ms each, as in the terminal).
const PER_MOOD: usize = 46;
/// Rows the label takes under the logo, glyph height plus a gap.
const LABEL_ROWS: usize = 5 + 3;

/// What each mood shows under the snail.
const MOODS: [(Mood, &str, (u8, u8, u8)); 6] = [
    (Mood::Calm, "CALMO", MUTED),
    (Mood::Run, "CORRE", (47, 123, 255)),
    (Mood::Hard, "CORRE MUITO", (47, 123, 255)),
    (Mood::Turbo, "TURBO", (255, 214, 80)),
    (Mood::Alert, "ALERTA", (255, 196, 38)),
    (Mood::Denied, "DESIGN NEGADO", (240, 70, 60)),
];

/// A 3x5 pixel letter; unknown characters are blank.
fn glyph(c: char) -> [&'static str; 5] {
    match c {
        'A' => [".#.", "#.#", "###", "#.#", "#.#"],
        'B' => ["##.", "#.#", "##.", "#.#", "##."],
        'C' => [".##", "#..", "#..", "#..", ".##"],
        'D' => ["##.", "#.#", "#.#", "#.#", "##."],
        'E' => ["###", "#..", "##.", "#..", "###"],
        'F' => ["###", "#..", "##.", "#..", "#.."],
        'G' => [".##", "#..", "#.#", "#.#", ".##"],
        'H' => ["#.#", "#.#", "###", "#.#", "#.#"],
        'I' => ["###", ".#.", ".#.", ".#.", "###"],
        'L' => ["#..", "#..", "#..", "#..", "###"],
        'M' => ["#.#", "###", "###", "#.#", "#.#"],
        'N' => ["##.", "#.#", "#.#", "#.#", "#.#"],
        'O' => [".#.", "#.#", "#.#", "#.#", ".#."],
        'R' => ["##.", "#.#", "##.", "#.#", "#.#"],
        'S' => [".##", "#..", ".#.", "..#", "##."],
        'T' => ["###", ".#.", ".#.", ".#.", ".#."],
        'U' => ["#.#", "#.#", "#.#", "#.#", "###"],
        _ => ["...", "...", "...", "...", "..."],
    }
}

/// The label as lit pixels `(x, y)`, with its width in pixels.
fn label(text: &str) -> (Vec<(usize, usize)>, usize) {
    let mut lit = Vec::new();
    let mut x = 0;
    for ch in text.chars() {
        for (gy, row) in glyph(ch).iter().enumerate() {
            for (gx, c) in row.bytes().enumerate() {
                if c == b'#' {
                    lit.push((x + gx, gy));
                }
            }
        }
        x += 4;
    }
    (lit, x.saturating_sub(1))
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "codeturbo.gif".into());
    let typed = frames() + 8;
    let sample = pixels_mood(0, typed, 120, Mood::Run);
    let (ph, pw) = (sample.len(), sample[0].len());
    let (cw, ch) = (pw + 2 * PAD, ph + LABEL_ROWS + 2 * PAD);
    let (w, h) = (cw * SCALE, ch * SCALE);
    let mut file = File::create(&out).expect("create gif");
    let mut enc = gif::Encoder::new(&mut file, w as u16, h as u16, &[]).expect("gif encoder");
    enc.set_repeat(gif::Repeat::Infinite).expect("loop forever");
    let only: Option<usize> = std::env::args().nth(2).and_then(|a| a.parse().ok());
    let mut total = 0;
    for (n, (mood, text, color)) in MOODS.into_iter().enumerate() {
        if only.is_some_and(|o| o != n) {
            continue;
        }
        let (lit, lw) = label(text);
        let left = (cw.saturating_sub(lw)) / 2;
        // The snail's frame is a running total, so its pace is the mood's.
        let mut phase = 0.0f32;
        for _ in 0..PER_MOOD {
            phase += mood.speed();
            let px = pixels_mood(phase as usize, typed, 120, mood);
            let mut cells = vec![BG; cw * ch];
            for (y, row) in px.iter().enumerate() {
                for (x, p) in row.iter().enumerate() {
                    if let Some(c) = p {
                        cells[(y + PAD) * cw + x + PAD] = *c;
                    }
                }
            }
            for &(x, y) in &lit {
                cells[(ph + PAD + 3 + y) * cw + left + x] = color;
            }
            let mut rgb = vec![0u8; w * h * 3];
            for y in 0..h {
                for x in 0..w {
                    let c = cells[(y / SCALE) * cw + x / SCALE];
                    rgb[(y * w + x) * 3..][..3].copy_from_slice(&[c.0, c.1, c.2]);
                }
            }
            let mut frame = gif::Frame::from_rgb_speed(w as u16, h as u16, &rgb, 10);
            frame.delay = 5;
            enc.write_frame(&frame).expect("write frame");
            total += 1;
        }
    }
    eprintln!("wrote {out} ({w}x{h}, {total} frames, {} moods)", MOODS.len());
}
