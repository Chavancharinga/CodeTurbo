//! The CodeTurbo intro: a one-color snail mark races beside the pixel
//! wordmark, leaving a fading blue trail. Drawn with half blocks in 24-bit
//! color; frames are pure functions so they can be tested, `play` only
//! draws them.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

type Rgb = (u8, u8, u8);

/// Brand colors: one blue, one soft white.
const BLUE: Rgb = (47, 123, 255);
const SOFT: Rgb = (225, 232, 245);
const TRAIL_FAR: Rgb = (14, 32, 80);

/// One snail design: its pixels and the numbers that drive its animation.
/// The same animation code draws every design.
pub struct Snail {
    /// Ringed shell and body, without antennae, sole or tail (drawn per
    /// frame). The top rows are left for the antennae.
    body: &'static [&'static str],
    /// Columns from here on are the body in front of the shell.
    front_x: usize,
    /// First and last row of the head, which moves as one piece.
    head: (usize, usize),
    /// Front rows from here down are the chest, which stays against the shell.
    chest_row: usize,
    /// Rows below the head down to here are the throat: their front edge
    /// never sticks out past the row above, so the passing wave leaves no lump.
    throat_end: usize,
    /// Bottom row of the body, where the muscle wave starts.
    base_row: usize,
    /// The body leans into a diagonal `/`: the top of the head sits this far
    /// ahead of the base.
    lean: usize,
    /// The muscle wave, as seen at one row over time: it swells, peaks and
    /// settles, then rests while the wave is further up. Each row runs one
    /// frame behind the row below, so the swell climbs from the chest, up
    /// the neck, to the head, and starts again at the chest: a loop that only
    /// ever flows forward, never back.
    bump: &'static [usize],
    /// The shell's inner spiral: a square block (top row, left column, size).
    spiral: (usize, usize, usize),
    /// Antennae swept back by the wind (row, column); right after the head
    /// surges, the tips whip back.
    antennae: [&'static [(usize, usize)]; 2],
    /// Room behind the shell for the tail, and the tail's resting length.
    tail_room: usize,
    tail: usize,
    /// Where the sole ends in front, from the left of the shell.
    sole_end: usize,
    /// Rows of the trail streaks behind the shell, with a phase offset each.
    streaks: &'static [(usize, usize)],
    /// Sprite width: tail room plus the body at its furthest reach.
    width: usize,
}

/// The first full-size design, kept as approved. Its frames are pinned by
/// `tests/fixtures/snail_large_frames.txt`; switch [`SNAIL`] to it to go back.
pub const SNAIL_LARGE: Snail = Snail {
    body: &[
        "....................",
        "....#####...........",
        "..##.....##.........",
        ".#..#####..#....####",
        "#..#.....#..#..#####",
        "#.#..###..#.#..####.",
        "#.#.#..#..#.#..###..",
        "#.#.#.##..#.#..###..",
        "#.#..#...#..#.####..",
        ".#..#####..#.#####..",
        "..##.....##.######..",
    ],
    front_x: 13,
    head: (3, 5),
    chest_row: 8,
    throat_end: 8,
    base_row: 10,
    lean: 2,
    bump: &[0, 1, 2, 1, 0, 0, 0, 0],
    spiral: (3, 3, 7),
    antennae: [
        &[(2, 16), (1, 15), (0, 14), (2, 18), (1, 17), (0, 16)],
        &[(2, 16), (1, 15), (1, 14), (0, 13), (2, 18), (1, 17), (1, 16), (0, 15)],
    ],
    tail_room: 7,
    tail: 5,
    sole_end: 20,
    streaks: &[(3, 0), (5, 3), (7, 1), (9, 4)],
    width: 7 + 24,
};

/// The same snail and animation, drawn at the height of the wordmark's `b`.
pub const SNAIL_SMALL: Snail = Snail {
    body: &[
        ".............",
        "..###........",
        ".#...#...###.",
        "#.##..#.####.",
        "#.#.#.#.###..",
        "#..##.#.###..",
        ".#...#.####..",
        "..###.#####..",
    ],
    front_x: 7,
    head: (2, 3),
    chest_row: 6,
    throat_end: 5,
    base_row: 7,
    lean: 1,
    bump: &[0, 1, 1, 1, 0, 0, 0, 0],
    spiral: (3, 2, 3),
    // Too small for a whip pose without the antennae lying flat on the head:
    // both poses are the same forked antennae.
    antennae: [&[(1, 9), (0, 8), (1, 11), (0, 10)], &[(1, 9), (0, 8), (1, 11), (0, 10)]],
    tail_room: 5,
    tail: 3,
    sole_end: 12,
    streaks: &[(2, 0), (4, 3), (6, 1)],
    width: 5 + 16,
};

/// The snail the intro draws.
pub const SNAIL: &Snail = &SNAIL_SMALL;

/// Canvas height for a design: its body plus the sole, rounded up to whole
/// text rows (two pixels per row).
const fn canvas_h(s: &Snail) -> usize {
    (s.body.len() + 2) / 2 * 2
}

/// Text rows of the logo.
pub const ROWS: usize = canvas_h(SNAIL) / 2;

/// 5×7 lowercase pixel font.
fn glyph(c: char) -> [&'static str; 7] {
    match c {
        'c' => [".....", ".....", ".####", "#....", "#....", "#....", ".####"],
        'o' => [".....", ".....", ".###.", "#...#", "#...#", "#...#", ".###."],
        'd' => ["....#", "....#", ".####", "#...#", "#...#", "#...#", ".####"],
        'e' => [".....", ".....", ".###.", "#...#", "#####", "#....", ".####"],
        't' => [".#...", ".#...", "####.", ".#...", ".#...", ".#...", "..##."],
        'u' => [".....", ".....", "#...#", "#...#", "#...#", "#...#", ".####"],
        'r' => [".....", ".....", "#.##.", "##..#", "#....", "#....", "#...."],
        'b' => ["#....", "#....", "####.", "#...#", "#...#", "#...#", "####."],
        _ => ["....."; 7],
    }
}

pub const NAME: &str = "codeturbo";
/// "code" in soft white, "turbo" in brand blue.
const SPLIT: usize = 4;
const GAP: usize = 3;
/// Trail length behind the shell.
const TRAIL_LEN: usize = 18;
const FRAMES: usize = 40;
const FRAME: Duration = Duration::from_millis(50);
const TAGLINE: &str = "fast · frugal · verified";

fn word_width() -> usize {
    NAME.len() * 6 - 1
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

/// Pixels of frame `i`: the wordmark (first `reveal` letters), then the
/// track with the snail running in place at its end, trail and dust behind.
fn canvas(s: &Snail, i: usize, track: usize, reveal: usize) -> Vec<Vec<Option<Rgb>>> {
    let h = canvas_h(s);
    // Rows above the sprite when its height is odd.
    let pad = h - (s.body.len() + 1);
    let t0 = word_width() + GAP;
    let mut px = vec![vec![None; t0 + track]; h];
    for (n, ch) in NAME.chars().enumerate().take(reveal) {
        let color = if n < SPLIT { SOFT } else { BLUE };
        for (gy, row) in glyph(ch).iter().enumerate() {
            for (gx, c) in row.bytes().enumerate() {
                if c == b'#' {
                    // The word sits on the snail's foot line.
                    px[h - 2 - 6 + gy][n * 6 + gx] = Some(color);
                }
            }
        }
    }
    // The snail runs in place at the end of the track; the trail and dust
    // stream past behind it.
    let pos = (track - s.width) as isize;
    let mut put = |y: usize, x: isize, c: Rgb| {
        if (0..track as isize).contains(&x) {
            px[pad + y][t0 + x as usize] = Some(c);
        }
    };
    let shell = pos + s.tail_room as isize;
    for &(row, phase) in s.streaks {
        for d in 1..=TRAIL_LEN {
            if (d + 2 * i + 3 * phase) % 8 < 5 {
                let t = 1.0 - d as f32 / TRAIL_LEN as f32;
                put(row, shell - d as isize, lerp(TRAIL_FAR, BLUE, t * t));
            }
        }
    }
    // Dust kicked up behind the tail tip, drifting back and fading.
    let tip = shell + 1 - s.tail_len(i) as isize;
    for d in 1..=8usize {
        if (d + 2 * i).is_multiple_of(5) {
            put(s.body.len(), tip - d as isize, lerp(TRAIL_FAR, BLUE, 1.0 - d as f32 / 8.0));
        }
    }
    for (y, x) in s.sprite(i) {
        put(y, pos + x as isize, BLUE);
    }
    px
}

impl Snail {
    fn head_rows(&self) -> std::ops::RangeInclusive<usize> {
        self.head.0..=self.head.1
    }

    /// How far row `y` leans ahead of the base.
    fn lean(&self, y: usize) -> usize {
        let (top, base) = (self.head.0, self.base_row);
        if (top..=base).contains(&y) {
            (self.lean * (base - y) + (base - top) / 2) / (base - top)
        } else {
            0
        }
    }

    /// How far the wave swells row `y` at frame `t`. The head rides the wave
    /// as one piece, at the timing of its lowest row.
    fn swell(&self, y: usize, t: usize) -> usize {
        let row = if self.head_rows().contains(&y) { self.head.1 } else { y };
        let n = self.bump.len();
        self.bump[(t + 4 * n - (self.base_row - row)) % n]
    }

    /// Tail length at frame `t`: it reaches back as the wave leaves the chest.
    fn tail_len(&self, t: usize) -> usize {
        self.tail + usize::from(self.swell(self.base_row, t) > 0)
    }

    /// Whether body pixel `(y, x)` is lit with the spiral turned `turns`
    /// quarter turns clockwise, rolling forward.
    fn body_lit(&self, y: usize, x: usize, turns: usize) -> bool {
        let (sy, sx, n) = self.spiral;
        if (sy..sy + n).contains(&y) && (sx..sx + n).contains(&x) {
            let (mut by, mut bx) = (y - sy, x - sx);
            for _ in 0..turns % 4 {
                (by, bx) = (n - 1 - bx, by);
            }
            return self.body[sy + by].as_bytes()[sx + bx] == b'#';
        }
        self.body[y].as_bytes()[x] == b'#'
    }

    /// Lit pixels of the snail at frame `t`.
    fn sprite(&self, t: usize) -> Vec<(usize, usize)> {
        let o = self.tail_room;
        let mut out = Vec::new();
        // Front edge of the row above, once the body starts.
        let mut above: Option<usize> = None;
        for (y, row) in self.body.iter().enumerate() {
            let lit: Vec<usize> = (0..row.len()).filter(|&x| self.body_lit(y, x, t)).collect();
            out.extend(lit.iter().filter(|&&x| x < self.front_x).map(|&x| (y, o + x)));
            let front: Vec<usize> = lit.into_iter().filter(|&x| x >= self.front_x).collect();
            let (Some(&lo), Some(&hi)) = (front.first(), front.last()) else { continue };
            let (l, b) = (self.lean(y), self.swell(y, t));
            if self.head_rows().contains(&y) {
                // The head surges forward whole when the wave reaches it.
                out.extend(front.iter().map(|&x| (y, o + x + l + b)));
                above = Some(hi + l + b);
            } else {
                // Neck and chest: the front edge swells as the wave passes;
                // the throat never pokes out past the row above; the chest
                // stays against the shell behind.
                let mut edge = hi + l + b;
                if y <= self.throat_end
                    && let Some(a) = above
                {
                    edge = edge.min(a).max(hi + l);
                }
                above = Some(edge);
                out.extend((lo + l..=edge).map(|x| (y, o + x)));
                if y >= self.chest_row {
                    out.extend((lo..lo + l).map(|x| (y, o + x)));
                }
            }
        }
        // Antennae follow the head one frame late, whipping back right after
        // the surge.
        let n = self.bump.len();
        let peak = self.bump.iter().copied().max().unwrap_or(0);
        let before = self.swell(self.head.1, t + n - 1);
        let whip = usize::from(before == peak && self.swell(self.head.1, t) < before);
        let shift = self.lean(self.head.0) + before;
        out.extend(self.antennae[whip].iter().map(|&(y, x)| (y, o + x + shift)));
        // Sole from the tail tip to under the chest; the tail tapers behind.
        let (tail, sole) = (self.tail_len(t), self.body.len());
        out.extend((o + 1 - tail..o + self.sole_end + self.swell(self.base_row, t)).map(|x| (sole, x)));
        out.extend((o + 2 - tail / 2..o + 3).map(|x| (sole - 1, x)));
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn fg((r, g, b): Rgb) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

fn bg((r, g, b): Rgb) -> String {
    format!("\x1b[48;2;{r};{g};{b}m")
}

/// Two pixel rows per text row with `▀`/`▄`.
fn half_blocks(px: &[Vec<Option<Rgb>>]) -> Vec<String> {
    px.chunks(2)
        .map(|pair| {
            let mut s = String::new();
            for (top, bottom) in pair[0].iter().zip(&pair[1]) {
                match (*top, *bottom) {
                    (None, None) => s.push(' '),
                    (Some(t), None) => s.push_str(&format!("{}▀\x1b[0m", fg(t))),
                    (None, Some(b)) => s.push_str(&format!("{}▄\x1b[0m", fg(b))),
                    (Some(t), Some(b)) => s.push_str(&format!("{}{}▀\x1b[0m", fg(t), bg(b))),
                }
            }
            s
        })
        .collect()
}

fn track_for(s: &Snail, width: usize) -> usize {
    width.saturating_sub(2 + word_width() + GAP).clamp(s.width, TRAIL_LEN + s.width)
}

/// Narrowest frame: the word plus room for one snail.
pub fn logo_width() -> usize {
    word_width() + GAP + SNAIL.width
}

/// Frame `i` for a terminal `width` cells wide: the logo rows, then the
/// tagline under the word once it is fully revealed.
pub fn frame(i: usize, width: usize) -> Vec<String> {
    frame_with(SNAIL, i, width)
}

/// [`frame`] for a given design.
pub fn frame_with(s: &Snail, i: usize, width: usize) -> Vec<String> {
    let reveal = (i / 2).min(NAME.len());
    let mut out = half_blocks(&canvas(s, i, track_for(s, width), reveal));
    out.push(if reveal == NAME.len() { format!("\x1b[2m{TAGLINE}\x1b[0m") } else { String::new() });
    out
}

/// Frames of the opening (name typing in) before the loop.
pub fn frames() -> usize {
    FRAMES
}

/// Without a terminal: one plain line, no escape codes.
pub fn plain() -> String {
    format!("CodeTurbo — {TAGLINE}")
}

fn term_width() -> usize {
    terminal_size::terminal_size().map_or(100, |(w, _)| w.0 as usize)
}

/// Whether the animated logo can be drawn on this terminal.
pub fn can_animate(is_terminal: bool) -> bool {
    is_terminal && std::env::var_os("NO_COLOR").is_none() && term_width() >= logo_width() + 2
}

/// Draws the intro: the name types itself in while the snail runs, then it
/// keeps running until `stop` is set (pass a set flag to play it once).
/// Without animation, prints the plain line instead.
pub fn play(out: &mut dyn Write, animate: bool, stop: &AtomicBool) {
    if !can_animate(animate) {
        let _ = writeln!(out, "{}", plain());
        return;
    }
    let width = term_width();
    let _ = write!(out, "\x1b[?25l");
    let mut drawn = 0;
    let mut i = 0;
    while i < FRAMES || !stop.load(Ordering::SeqCst) {
        redraw(out, &frame(i, width), &mut drawn);
        std::thread::sleep(FRAME);
        i += 1;
    }
    let _ = write!(out, "\x1b[?25h");
    let _ = out.flush();
}

/// Column (in cells, from 0) where the track starts, right after the word.
fn track_col() -> usize {
    word_width() + GAP
}

/// Only the track (trail, dust and snail) of frame `i`: once the name is on
/// screen, this is all that changes.
pub fn track_rows(i: usize, width: usize) -> Vec<String> {
    let s = SNAIL;
    let t0 = track_col();
    let px = canvas(s, i, track_for(s, width), NAME.len());
    let track: Vec<Vec<Option<Rgb>>> = px.into_iter().map(|row| row[t0..].to_vec()).collect();
    half_blocks(&track)
}

/// Keeps the logo (already on screen, `up` lines above the cursor) running
/// in place while the user types below it. Stops on [`LiveLogo::stop`].
/// Only the track is redrawn: the name is never touched again, so it can't
/// flicker while the prompt is active.
pub struct LiveLogo {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LiveLogo {
    pub fn start(up: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let width = term_width();
        let thread = std::thread::spawn(move || {
            let mut i = FRAMES;
            let col = track_col() + 1;
            while !flag.load(Ordering::SeqCst) {
                // Save cursor, jump to the logo, redraw the track of each
                // row from its column, restore: one write.
                let mut buf = format!("\x1b7\x1b[{up}A");
                for row in track_rows(i, width) {
                    buf.push_str(&format!("\x1b[{col}G{row}\x1b[K\x1b[1E"));
                }
                buf.push_str("\x1b8");
                let mut err = std::io::stderr().lock();
                let _ = err.write_all(buf.as_bytes());
                let _ = err.flush();
                drop(err);
                std::thread::sleep(FRAME);
                i += 1;
            }
        });
        Self { stop, thread: Some(thread) }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for LiveLogo {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Moves back over the previous frame and overwrites it line by line.
fn redraw(out: &mut dyn Write, lines: &[String], drawn: &mut usize) {
    if *drawn > 0 {
        let _ = write!(out, "\x1b[{}A", *drawn);
    }
    for line in lines {
        let _ = write!(out, "\r\x1b[2K{line}\n");
    }
    *drawn = lines.len();
    let _ = out.flush();
}

/// One-line running snail shown while the model is thinking.
pub fn spinner_line(i: usize, label: &str) -> String {
    const LEN: usize = 10;
    let mut s = String::from("  ");
    for x in 0..LEN {
        let lit = (x + LEN - i % LEN) % LEN < 2;
        let t = (x as f32 + 1.0) / LEN as f32;
        let c = if lit { SOFT } else { lerp(TRAIL_FAR, BLUE, t * t) };
        s.push_str(&format!("{}─", fg(c)));
    }
    s.push_str(&format!("{}@\x1b[0m \x1b[2m{label}\x1b[0m", fg(BLUE)));
    s
}

/// Running snail on stderr until stopped. Does nothing off a terminal.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(label: &str) -> Self {
        use std::io::IsTerminal;
        let stop = Arc::new(AtomicBool::new(false));
        let enabled = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        let thread = enabled.then(|| {
            let stop = stop.clone();
            let label = label.to_string();
            std::thread::spawn(move || {
                let mut i = 0;
                while !stop.load(Ordering::SeqCst) {
                    eprint!("\r{}", spinner_line(i, &label));
                    let _ = std::io::stderr().flush();
                    std::thread::sleep(Duration::from_millis(80));
                    i += 1;
                }
                eprint!("\r\x1b[2K");
                let _ = std::io::stderr().flush();
            })
        });
        Self { stop, thread }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}
