//! Renders the CodeTurbo logo (wordmark and running snail) as a looping GIF,
//! straight from the terminal animation's pixels.
//!
//!     cargo run -p coturbo-cli --example logo_gif -- docs/codeturbo.gif

use coturbo_cli::splash::{LOOP_FRAMES, frames, pixels};
use std::fs::File;

const SCALE: usize = 6;
const PAD: usize = 4;
const BG: (u8, u8, u8) = (13, 17, 23);

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "codeturbo.gif".into());
    // Start after the name has typed in, so every frame of the loop shows it.
    let first = frames();
    let sample = pixels(first, 120);
    let (ph, pw) = (sample.len(), sample[0].len());
    let (w, h) = ((pw + 2 * PAD) * SCALE, (ph + 2 * PAD) * SCALE);
    let mut file = File::create(&out).expect("create gif");
    let mut enc = gif::Encoder::new(&mut file, w as u16, h as u16, &[]).expect("gif encoder");
    enc.set_repeat(gif::Repeat::Infinite).expect("loop forever");
    for i in first..first + LOOP_FRAMES {
        let px = pixels(i, 120);
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let (gx, gy) = ((x / SCALE).checked_sub(PAD), (y / SCALE).checked_sub(PAD));
                let c = match (gx, gy) {
                    (Some(gx), Some(gy)) if gy < ph && gx < pw => px[gy][gx].unwrap_or(BG),
                    _ => BG,
                };
                rgb[(y * w + x) * 3..][..3].copy_from_slice(&[c.0, c.1, c.2]);
            }
        }
        let mut frame = gif::Frame::from_rgb_speed(w as u16, h as u16, &rgb, 10);
        frame.delay = 5; // 50 ms, as in the terminal
        enc.write_frame(&frame).expect("write frame");
    }
    eprintln!("wrote {out} ({w}x{h}, {LOOP_FRAMES} frames)");
}
