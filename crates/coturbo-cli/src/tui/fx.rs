//! Text effects for the header, after the `slide` and `highlight` effects of
//! TerminalTextEffects: the logo and the snail slide into view from outside
//! the terminal, and a specular glint runs over the name while the agent works.
//! Pure functions of the clock, so they are easy to test and never drift.

pub type Rgb = (u8, u8, u8);
pub type Grid = Vec<Vec<Option<Rgb>>>;

/// TerminalTextEffects' `slide` at its defaults: the picture is split into rows,
/// each row is added `GAP_FRAMES` after the one above, and every character of a
/// row sets off from just outside the left edge, at the same speed, with an
/// in-out-quad easing, to its place. The row forms as its characters spread.
const FRAME_MS: f32 = 1000.0 / 60.0;
const GAP_FRAMES: f32 = 2.0;
/// Cells per frame; a little faster than the original's 0.8 so a wide header
/// does not make the start of every session wait.
const SPEED: f32 = 1.1;
/// Everything has landed by this time, whatever the width of the header.
pub const INTRO_MS: u128 = 2800;
/// The glint comes back this often, and takes [`GLINT_MS`] to cross.
const GLINT_EVERY_MS: u128 = 6500;
const GLINT_MS: f32 = 1100.0;

fn in_out_quad(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
    }
}

/// The grid `ms` after the start, with every row sliding in from the left.
/// After [`INTRO_MS`] it is the grid itself.
pub fn slide_in(grid: &Grid, ms: u128) -> Grid {
    if ms >= INTRO_MS || grid.is_empty() {
        return grid.clone();
    }
    let width = grid[0].len();
    let mut out: Grid = vec![vec![None; width]; grid.len()];
    for (r, row) in grid.iter().enumerate() {
        // A row of text is two rows of half-block pixels.
        let delay = (r / 2) as f32 * GAP_FRAMES * FRAME_MS;
        for (c, px) in row.iter().enumerate() {
            let Some(px) = px else { continue };
            let distance = (c + 1) as f32;
            let duration = distance / SPEED * FRAME_MS;
            let progress = (ms as f32 - delay) / duration;
            if progress <= 0.0 {
                continue;
            }
            let x = (-1.0 + distance * in_out_quad(progress)).round() as i64;
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
            let Some((red, green, blue)) = px else {
                continue;
            };
            let d = ((c as f32 + r as f32 * 0.7) - at).abs();
            if d < 4.5 {
                let k = (1.0 - d / 4.5) * 0.75;
                let lift = |v: u8| (f32::from(v) + (255.0 - f32::from(v)) * k).round() as u8;
                *px = Some((lift(*red), lift(*green), lift(*blue)));
            }
        }
    }
}
