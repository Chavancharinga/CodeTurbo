//! Text effects for the header, after the `slide` and `highlight` effects of
//! TerminalTextEffects: the logo and the snail slide into view from outside
//! the terminal, and a specular glint runs over the name while the agent works.
//! Pure functions of the clock, so they are easy to test and never drift.

pub type Rgb = (u8, u8, u8);
pub type Grid = Vec<Vec<Option<Rgb>>>;

/// How long every column takes to slide into place.
const SLIDE_MS: f32 = 800.0;
/// The columns start one after the other over this long.
const STAGGER_MS: f32 = 450.0;
/// Time from the first column to the last one landing.
pub const INTRO_MS: u128 = (SLIDE_MS + STAGGER_MS) as u128;
/// The glint comes back this often, and takes [`GLINT_MS`] to cross.
const GLINT_EVERY_MS: u128 = 6500;
const GLINT_MS: f32 = 1100.0;

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The grid `ms` after the start: each column slides in from outside the
/// terminal, rows alternating between the right and the left, the columns
/// setting off one after the other. After [`INTRO_MS`] it is the grid itself.
pub fn slide_in(grid: &Grid, ms: u128) -> Grid {
    if ms >= INTRO_MS || grid.is_empty() {
        return grid.clone();
    }
    let width = grid[0].len();
    let mut out: Grid = vec![vec![None; width]; grid.len()];
    for (r, row) in grid.iter().enumerate() {
        let from_right = (r / 2) % 2 == 0;
        for (c, px) in row.iter().enumerate() {
            let Some(px) = px else { continue };
            let delay = STAGGER_MS * c as f32 / width.max(1) as f32;
            let left = 1.0 - ease_out((ms as f32 - delay) / SLIDE_MS);
            let at = if from_right { c as f32 + left * (width - c + 2) as f32 } else { c as f32 - left * (c + 3) as f32 };
            let x = at.round() as i64;
            if (0..width as i64).contains(&x) {
                out[r][x as usize] = Some(*px);
            }
        }
    }
    out
}

/// A glint across the picture: a bright diagonal band that crosses it every
/// few seconds, lighting what it passes over.
pub fn highlight(grid: &mut Grid, ms: u128) {
    let width = grid.first().map_or(0, Vec::len) as f32;
    let phase = (ms % GLINT_EVERY_MS) as f32;
    if phase >= GLINT_MS {
        return;
    }
    let at = phase / GLINT_MS * (width + 24.0) - 12.0;
    for (r, row) in grid.iter_mut().enumerate() {
        for (c, px) in row.iter_mut().enumerate() {
            let Some((red, green, blue)) = px else { continue };
            let d = ((c as f32 + r as f32 * 0.7) - at).abs();
            if d < 4.5 {
                let k = (1.0 - d / 4.5) * 0.75;
                let lift = |v: u8| (f32::from(v) + (255.0 - f32::from(v)) * k).round() as u8;
                *px = Some((lift(*red), lift(*green), lift(*blue)));
            }
        }
    }
}
