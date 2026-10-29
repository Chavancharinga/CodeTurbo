//! Renders the CodeTurbo logo as a looping GIF that plays every mood of the
//! snail in turn: white, blue, alert, denied, then running, running hard and
//! turbo, and green last.
//! Straight from the interface's own pixels, with no text added.
//!
//!     cargo run -p coturbo-cli --example logo_gif -- docs/codeturbo.gif
//!
//! A second argument (0..7) renders only that mood, to check it by eye.

#[path = "moods_common/mod.rs"]
mod common;

use common::{MOODS, Rgb, frame};
use std::fs::File;

const BG: Rgb = (13, 17, 23);
const SCALE: usize = 6;
const PAD: usize = 4;
/// Frames each mood plays (50 ms each, as in the terminal).
const PER_MOOD: usize = 40;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "codeturbo.gif".into());
    let only: Option<usize> = std::env::args().nth(2).and_then(|a| a.parse().ok());
    let first = frame(MOODS[0], 0, PAD);
    let (ch, cw) = (first.len(), first[0].len());
    let (w, h) = (cw * SCALE, ch * SCALE);
    let mut file = File::create(&out).expect("create gif");
    let mut enc = gif::Encoder::new(&mut file, w as u16, h as u16, &[]).expect("gif encoder");
    enc.set_repeat(gif::Repeat::Infinite).expect("loop forever");
    let mut total = 0;
    for (n, mood) in MOODS.into_iter().enumerate() {
        if only.is_some_and(|o| o != n) {
            continue;
        }
        for i in 0..PER_MOOD {
            let cells = frame(mood, i, PAD);
            let mut rgb = vec![0u8; w * h * 3];
            for y in 0..h {
                for x in 0..w {
                    let c = cells[y / SCALE][x / SCALE].unwrap_or(BG);
                    rgb[(y * w + x) * 3..][..3].copy_from_slice(&[c.0, c.1, c.2]);
                }
            }
            let mut f = gif::Frame::from_rgb_speed(w as u16, h as u16, &rgb, 10);
            f.delay = 5;
            enc.write_frame(&f).expect("write frame");
            total += 1;
        }
    }
    eprintln!(
        "wrote {out} ({w}x{h}, {total} frames, {} moods)",
        MOODS.len()
    );
}
