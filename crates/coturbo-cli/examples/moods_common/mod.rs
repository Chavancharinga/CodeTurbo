//! Shared by the logo generators (`logo_gif`, `logo_svg`): the moods of the
//! snail, in the order they play, and the frames of each.

use coturbo_cli::splash::{Mood, frames, pixels_mood};

pub type Rgb = (u8, u8, u8);
pub type Grid = Vec<Vec<Option<Rgb>>>;

/// The moods in the order they play: white (before anything is asked), blue
/// (a task starts), alert (yellow) and denied (red), then running, running
/// hard and turbo, and green last: it only comes when everything is done.
pub const MOODS: [Mood; 8] = [Mood::Calm, Mood::Start, Mood::Alert, Mood::Denied, Mood::Run, Mood::Hard, Mood::Turbo, Mood::Done];

/// Frame after which the name is fully typed in.
fn typed() -> usize {
    frames() + 8
}

/// Frame `n` of a mood: the logo with its snail at that mood's pace, as a
/// grid `padding` cells larger on every side.
pub fn frame(mood: Mood, n: usize, padding: usize) -> Grid {
    let mut phase = 0.0f32;
    for _ in 0..=n {
        phase += mood.speed();
    }
    let px = pixels_mood(phase as usize, typed(), 120, mood);
    let (ph, pw) = (px.len(), px[0].len());
    let mut grid: Grid = vec![vec![None; pw + 2 * padding]; ph + 2 * padding];
    for (y, row) in px.iter().enumerate() {
        for (x, p) in row.iter().enumerate() {
            grid[y + padding][x + padding] = *p;
        }
    }
    grid
}
