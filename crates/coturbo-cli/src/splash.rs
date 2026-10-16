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
pub const BLUE: Rgb = (47, 123, 255);
pub const SOFT: Rgb = (225, 232, 245);
const TRAIL_FAR: Rgb = (14, 32, 80);
/// Resting color of the spinner label.
const LABEL: Rgb = (110, 125, 150);

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

/// 5×7 pixel font: the letters of the name.
fn glyph(c: char) -> [&'static str; 7] {
    match c {
        'C' => [".###.", "#...#", "#....", "#....", "#....", "#...#", ".###."],
        'T' => ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#.."],
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

pub const NAME: &str = "CodeTurbo";
/// "code" in soft white, "turbo" in brand blue.
const SPLIT: usize = 4;
const GAP: usize = 3;
/// Trail length behind the shell.
const TRAIL_LEN: usize = 18;
const FRAMES: usize = 40;
/// Time between animation frames.
pub const FRAME: Duration = Duration::from_millis(50);

fn word_width() -> usize {
    NAME.len() * 6 - 1
}

fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

const YELLOW: Rgb = (255, 196, 38);
const SPARK: Rgb = (255, 232, 96);
const RED: Rgb = (240, 70, 60);
const GREEN: Rgb = (30, 150, 78);

/// How the snail feels: its colour, pace, trail and sparks follow what the
/// agent is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mood {
    /// Before anything is asked: white, hardly moving, no trail.
    #[default]
    Calm,
    /// A task has just begun: blue, walking, no trail yet.
    Start,
    /// The task is done and checked: green, walking.
    Done,
    /// Something needs attention: the snail turns yellow and walks.
    Alert,
    /// The design check refused the work: the snail turns red and walks.
    Denied,
    /// Working on an answer: the run of the intro.
    Run,
    /// Tools and engines at work: runs hard, with a dense trail.
    Hard,
    /// Flat out: a blue trail with yellow in it and sparks from the shell.
    Turbo,
}

impl Mood {
    /// Animation frames per tick; 1.0 is the pace of the intro.
    pub fn speed(self) -> f32 {
        match self {
            Mood::Calm => 0.3,
            Mood::Start | Mood::Done => 0.7,
            Mood::Alert => 0.8,
            Mood::Denied => 0.6,
            Mood::Run => 1.0,
            Mood::Hard => 1.8,
            Mood::Turbo => 2.8,
        }
    }

    /// Whether the mood is a short flash that gives way to the working mood.
    pub fn is_flash(self) -> bool {
        matches!(self, Mood::Alert | Mood::Denied)
    }

    /// The colour of the snail, and of "turbo" in the name.
    pub fn body(self) -> Rgb {
        match self {
            Mood::Calm => SOFT,
            Mood::Done => GREEN,
            Mood::Alert => YELLOW,
            Mood::Denied => RED,
            _ => BLUE,
        }
    }

    /// The far, dark end of the trail.
    fn trail_far(self) -> Rgb {
        match self {
            Mood::Calm => (58, 64, 78),
            Mood::Done => (14, 62, 38),
            Mood::Alert => (84, 62, 10),
            Mood::Denied => (84, 22, 18),
            _ => TRAIL_FAR,
        }
    }

    /// Streak density: how many of every 8 cells of a streak are lit.
    fn density(self) -> usize {
        match self {
            Mood::Calm | Mood::Start | Mood::Done => 0,
            Mood::Alert | Mood::Denied => 5,
            Mood::Run => 5,
            Mood::Hard => 7,
            Mood::Turbo => 8,
        }
    }

    /// How far the trail reaches behind the shell. Only the running moods
    /// leave a running trail; yellow and red leave a small one (neither
    /// walking nor running); the walking ones leave none.
    fn trail_len(self) -> usize {
        match self {
            Mood::Calm | Mood::Start | Mood::Done => 0,
            Mood::Alert | Mood::Denied => 6,
            Mood::Run | Mood::Hard | Mood::Turbo => TRAIL_LEN,
        }
    }

    /// Quarter turns the spiral of the shell makes per frame: barely moving
    /// while the snail is still or walking, fast when it runs, faster running
    /// hard, and faster still in turbo. (A frame lasts 50 ms divided by the
    /// mood's pace, so these are about 0.4, 0.9, 4, 8 and 14 quarter turns a second.)
    pub fn spin(self) -> f32 {
        match self {
            Mood::Calm => 0.06,
            Mood::Start | Mood::Done => 0.065,
            Mood::Alert => 0.0625,
            Mood::Denied => 0.083,
            Mood::Run => 0.2,
            Mood::Hard => 0.222,
            Mood::Turbo => 0.25,
        }
    }
}

/// Pixels of frame `i`: the wordmark (first `reveal` letters), then the
/// track with the snail running in place at its end, trail and dust behind.
fn canvas(s: &Snail, i: usize, track: usize, reveal: usize) -> Vec<Vec<Option<Rgb>>> {
    // The intro turns the shell a quarter turn at every frame.
    canvas_with(s, i, track, reveal, Mood::Run, 1.0)
}

/// [`canvas`] for a mood, with the shell turning at the mood's own pace.
fn canvas_mood(s: &Snail, i: usize, track: usize, reveal: usize, mood: Mood) -> Vec<Vec<Option<Rgb>>> {
    canvas_with(s, i, track, reveal, mood, mood.spin())
}

/// `spin` is how many quarter turns the shell makes per frame.
fn canvas_with(s: &Snail, i: usize, track: usize, reveal: usize, mood: Mood, spin: f32) -> Vec<Vec<Option<Rgb>>> {
    let h = canvas_h(s);
    // Rows above the sprite when its height is odd.
    let pad = h - (s.body.len() + 1);
    let t0 = word_width() + GAP;
    let mut px = vec![vec![None; t0 + track]; h];
    for (n, ch) in NAME.chars().enumerate().take(reveal) {
        // "turbo" wears the colour of the snail.
        let color = if n < SPLIT { SOFT } else { mood.body() };
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
    let (far, body) = (mood.trail_far(), mood.body());
    let trail = mood.trail_len();
    for &(row, phase) in s.streaks {
        for d in 1..=trail {
            if (d + 2 * i + 3 * phase) % 8 < mood.density() {
                let t = 1.0 - d as f32 / trail as f32;
                // Flat out, the trail is hot near the shell: some cells burn yellow.
                let color = if mood == Mood::Turbo && d <= 10 && (d + i + phase).is_multiple_of(5) {
                    lerp((150, 104, 16), SPARK, t)
                } else if mood == Mood::Turbo {
                    lerp(far, body, t.sqrt())
                } else {
                    lerp(far, body, t * t)
                };
                put(row, shell - d as isize, color);
            }
        }
    }
    // Dust kicked up behind the tail tip, drifting back and fading.
    let tip = shell + 1 - s.tail_len(i) as isize;
    if trail > 0 {
        // Eight cells of dust behind a runner, as ever; three behind the small trails.
        for d in 1..=(if trail >= TRAIL_LEN { 8 } else { 3 }) {
            if (d + 2 * i).is_multiple_of(5) {
                put(s.body.len(), tip - d as isize, lerp(far, body, 1.0 - d as f32 / 8.0));
            }
        }
    }
    let turns = (i as f32 * spin) as usize;
    for (y, x) in s.sprite(i, turns) {
        put(y, pos + x as isize, body);
    }
    if mood == Mood::Turbo {
        // Sparks leave the middle of the shell and stream back, cooling from
        // bright yellow to a dull ember.
        let (sy, sx, n) = s.spiral;
        let (cy, cx) = ((sy + n / 2) as isize, (s.tail_room + sx + n / 2) as isize);
        for k in 0..5usize {
            let age = (i + 2 * k) % 9;
            let lift = [0isize, -1, 1, 0, -1][k];
            let color = match age {
                0..=2 => SPARK,
                3..=5 => lerp(YELLOW, (200, 110, 20), (age - 3) as f32 / 3.0),
                _ => (120, 84, 14),
            };
            put((cy + lift) as usize, pos + cx - (age + age / 2) as isize, color);
        }
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
    fn sprite(&self, t: usize, turns: usize) -> Vec<(usize, usize)> {
        let o = self.tail_room;
        let mut out = Vec::new();
        // Front edge of the row above, once the body starts.
        let mut above: Option<usize> = None;
        for (y, row) in self.body.iter().enumerate() {
            let lit: Vec<usize> = (0..row.len()).filter(|&x| self.body_lit(y, x, turns)).collect();
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

/// Frame `i` for a terminal `width` cells wide: the logo rows.
pub fn frame(i: usize, width: usize) -> Vec<String> {
    frame_with(SNAIL, i, width)
}

/// [`frame`] for a given design.
pub fn frame_with(s: &Snail, i: usize, width: usize) -> Vec<String> {
    half_blocks(&canvas(s, i, track_for(s, width), (i / 2).min(NAME.len())))
}

/// Pixels of frame `i` (`None` is background), as drawn in a terminal
/// `width` cells wide: for exporting the logo as an image.
pub fn pixels(i: usize, width: usize) -> Vec<Vec<Option<(u8, u8, u8)>>> {
    canvas(SNAIL, i, track_for(SNAIL, width), (i / 2).min(NAME.len()))
}

/// Pixels for the interface: the snail at animation frame `snail` in `mood`,
/// with the name typed in as of frame `typed` (the two advance at different
/// paces, because the snail's pace follows its mood).
pub fn pixels_mood(snail: usize, typed: usize, width: usize, mood: Mood) -> Vec<Vec<Option<(u8, u8, u8)>>> {
    canvas_mood(SNAIL, snail, track_for(SNAIL, width), (typed / 2).min(NAME.len()), mood)
}

/// Only the snail and its trail, `cols` pixels wide at most, for screens too
/// narrow for the whole logo (a phone in portrait).
pub fn snail_only(snail: usize, cols: usize, mood: Mood) -> Vec<Vec<Option<(u8, u8, u8)>>> {
    let track = cols.clamp(SNAIL.width, TRAIL_LEN + SNAIL.width);
    let t0 = word_width() + GAP;
    canvas_mood(SNAIL, snail, track, 0, mood).into_iter().map(|row| row[t0..].to_vec()).collect()
}

/// Frames in one seamless loop of the running snail once the name is shown
/// (the wave, spiral, trail and dust all repeat after this many).
pub const LOOP_FRAMES: usize = 40;

/// Frames of the opening (name typing in) before the loop.
pub fn frames() -> usize {
    FRAMES
}

/// Without a terminal: one plain line, no escape codes.
pub fn plain() -> String {
    NAME.to_string()
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
    track_rows_with(SNAIL, i, width)
}

/// [`track_rows`] for a given design.
pub fn track_rows_with(s: &Snail, i: usize, width: usize) -> Vec<String> {
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

/// One-line indicator shown while the model works, frame `i`: a wave of
/// dots flowing into the snail, and a light sweeping across the label.
pub fn spinner_line(i: usize, label: &str) -> String {
    const WAVE: usize = 12;
    const LEVELS: [char; 4] = ['⣀', '⣤', '⣶', '⣿'];
    let mut s = String::from("  ");
    for x in 0..WAVE {
        let phase = (x as f32 - i as f32 * 0.9) * 0.55;
        let level = ((phase.sin() + 1.0) / 2.0 * 3.0).round() as usize;
        let t = (x as f32 + 1.0) / WAVE as f32;
        s.push_str(&format!("{}{}", fg(lerp(TRAIL_FAR, BLUE, t * t)), LEVELS[level.min(3)]));
    }
    s.push_str(&format!(" {}@\x1b[0m ", fg(BLUE)));
    let n = label.chars().count() as isize;
    let light = (i as isize % (n + 8)) - 4;
    for (k, c) in label.chars().enumerate() {
        let glow = (1.0 - (k as isize - light).unsigned_abs() as f32 / 4.0).max(0.0);
        s.push_str(&format!("{}{c}", fg(lerp(LABEL, SOFT, glow))));
    }
    s.push_str("\x1b[0m");
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

#[cfg(test)]
mod mood_tests {
    use super::*;

    fn lit(px: &[Vec<Option<Rgb>>]) -> Vec<Rgb> {
        px.iter().flatten().flatten().copied().collect()
    }

    /// Cells of the track only (the word is excluded).
    fn track(px: &[Vec<Option<Rgb>>]) -> Vec<Rgb> {
        let t0 = word_width() + GAP;
        px.iter().flat_map(|r| r[t0..].iter().flatten().copied()).collect()
    }

    #[test]
    fn the_intro_is_unchanged_by_the_moods() {
        // The intro keeps its own quarter turn per frame; Run as a mood has its own spin.
        for i in [0, 7, 19, 40] {
            let intro = canvas(SNAIL, i, track_for(SNAIL, 100), 9);
            assert_eq!(intro, canvas_with(SNAIL, i, track_for(SNAIL, 100), 9, Mood::Run, 1.0));
        }
    }

    #[test]
    fn calm_leaves_next_to_no_trail() {
        let calm = track(&canvas_mood(SNAIL, 11, track_for(SNAIL, 100), 9, Mood::Calm));
        let run = track(&canvas_mood(SNAIL, 11, track_for(SNAIL, 100), 9, Mood::Run));
        assert!(calm.len() + 8 < run.len(), "calm {} vs run {}", calm.len(), run.len());
    }

    #[test]
    fn the_trail_gets_denser_with_effort() {
        let n = |m| (0..8).map(|i| track(&canvas_mood(SNAIL, i, track_for(SNAIL, 100), 9, m)).len()).sum::<usize>();
        assert!(n(Mood::Calm) < n(Mood::Run) && n(Mood::Run) < n(Mood::Hard) && n(Mood::Hard) < n(Mood::Turbo));
    }

    #[test]
    fn each_mood_has_its_color() {
        let body = |m| {
            let px = canvas_mood(SNAIL, 3, track_for(SNAIL, 100), 0, m);
            lit(&px)
        };
        // White before anything is asked, blue once a task starts, green when it is done.
        assert!(body(Mood::Calm).contains(&SOFT) && !body(Mood::Calm).contains(&BLUE));
        assert!(body(Mood::Start).contains(&BLUE));
        assert!(body(Mood::Done).contains(&GREEN) && !body(Mood::Done).contains(&BLUE));
        assert!(body(Mood::Alert).contains(&YELLOW) && !body(Mood::Alert).contains(&BLUE));
        assert!(body(Mood::Denied).contains(&RED) && !body(Mood::Denied).contains(&BLUE));
        for m in [Mood::Run, Mood::Hard, Mood::Turbo] {
            assert!(body(m).contains(&BLUE), "{m:?}");
        }
    }

    #[test]
    fn the_walking_moods_leave_next_to_no_trail() {
        // Every mood draws the same snail; only the trail differs.
        let n = |m| (0..8).map(|i| track(&canvas_mood(SNAIL, i, track_for(SNAIL, 100), 9, m)).len()).sum::<usize>();
        let (calm, run) = (n(Mood::Calm), n(Mood::Run));
        assert_eq!(n(Mood::Start), calm, "blue walking: no trail");
        assert_eq!(n(Mood::Done), calm, "green walking: no trail");
        assert!(run > calm + 100, "running has a real trail: {run} vs {calm}");
        // Alert and denied walk too, with a light trail in their colour.
        for m in [Mood::Alert, Mood::Denied] {
            assert!(n(m) > calm && n(m) < run, "{m:?}: {} between {calm} and {run}", n(m));
        }
    }

    #[test]
    fn turbo_has_yellow_sparks_and_yellow_in_the_trail_and_others_do_not() {
        let warm = |c: &Rgb| c.0 > 200 && c.1 > 150 && c.2 < 130;
        let count = |m| (0..12).map(|i| track(&canvas_mood(SNAIL, i, track_for(SNAIL, 100), 9, m)).iter().filter(|c| warm(c)).count()).sum::<usize>();
        assert!(count(Mood::Turbo) > 12, "turbo yellow cells: {}", count(Mood::Turbo));
        assert_eq!(count(Mood::Run), 0);
        assert_eq!(count(Mood::Hard), 0);
        assert_eq!(count(Mood::Calm), 0);
    }

    #[test]
    fn sparks_start_inside_the_shell() {
        // At age 0 a spark sits on the middle of the shell: at least one
        // bright spark pixel falls where the body would be drawn.
        let px = canvas_mood(SNAIL, 0, track_for(SNAIL, 100), 9, Mood::Turbo);
        assert!(track(&px).contains(&SPARK));
    }

    #[test]
    fn the_faster_the_effort_the_faster_the_frames() {
        assert!(Mood::Calm.speed() < Mood::Start.speed(), "idle is almost still");
        assert!(Mood::Start.speed() < Mood::Run.speed());
        assert!(Mood::Run.speed() < Mood::Hard.speed());
        assert!(Mood::Hard.speed() < Mood::Turbo.speed());
        assert_eq!(Mood::Start.speed(), Mood::Done.speed());
        assert!(Mood::Alert.is_flash() && Mood::Denied.is_flash());
        assert!(![Mood::Calm, Mood::Start, Mood::Done, Mood::Run, Mood::Hard, Mood::Turbo].iter().any(|m| m.is_flash()));
    }

    #[test]
    fn pixels_mood_keeps_the_picture_the_same_size() {
        let a = pixels_mood(0, 40, 100, Mood::Calm);
        let b = pixels_mood(0, 40, 100, Mood::Turbo);
        assert_eq!((a.len(), a[0].len()), (b.len(), b[0].len()));
    }

    /// Leftmost lit column of the track over several frames.
    fn reach(m: Mood) -> usize {
        let t0 = word_width() + GAP;
        (0..16)
            .filter_map(|i| {
                let px = canvas_mood(SNAIL, i, track_for(SNAIL, 100), 9, m);
                px.iter().filter_map(|r| r[t0..].iter().position(Option::is_some)).min()
            })
            .min()
            .unwrap()
    }

    #[test]
    fn only_the_running_moods_leave_a_running_trail() {
        let (calm, start, done) = (reach(Mood::Calm), reach(Mood::Start), reach(Mood::Done));
        assert_eq!((calm, start), (start, done), "the walking snails leave nothing behind them");
        for m in [Mood::Alert, Mood::Denied] {
            let r = reach(m);
            assert!(r + 3 <= calm, "{m:?} leaves a small trail: reaches {r}, walking reaches {calm}");
            assert!(r >= reach(Mood::Run) + 6, "{m:?} is far from a running trail: {r} vs {}", reach(Mood::Run));
        }
        assert!(reach(Mood::Run) + 6 <= calm);
        assert!(reach(Mood::Hard) <= reach(Mood::Run) + 1 && reach(Mood::Turbo) <= reach(Mood::Run) + 1);
    }

    #[test]
    fn no_dust_behind_a_walking_snail() {
        // The only lit cells of a walking snail are the snail itself.
        let t0 = word_width() + GAP;
        let cols = |m| (0..16).map(|i| canvas_mood(SNAIL, i, track_for(SNAIL, 100), 9, m).iter().map(|r| r[t0..].iter().filter(|c| c.is_some()).count()).sum::<usize>()).collect::<Vec<_>>();
        assert_eq!(cols(Mood::Calm), cols(Mood::Done));
        let walking = cols(Mood::Start);
        let sprite_only: Vec<usize> = (0..16).map(|i| SNAIL.sprite(i, 0).len()).collect();
        assert_eq!(walking, sprite_only, "nothing but the snail");
    }

    #[test]
    fn turbo_the_word_wears_the_color_of_the_snail() {
        let word = |m: Mood| {
            let px = canvas_mood(SNAIL, 5, track_for(SNAIL, 100), 9, m);
            let t0 = word_width() + GAP;
            // Columns of the letters of "turbo": after the first four, 6 columns each.
            let (from, to) = (SPLIT * 6, word_width().min(t0));
            let colors: std::collections::BTreeSet<Rgb> = px.iter().flat_map(|r| r[from..to].iter().flatten().copied()).collect();
            colors
        };
        for (m, c) in [(Mood::Calm, SOFT), (Mood::Start, BLUE), (Mood::Done, GREEN), (Mood::Alert, YELLOW), (Mood::Denied, RED), (Mood::Run, BLUE), (Mood::Turbo, BLUE)] {
            assert_eq!(word(m), [c].into_iter().collect(), "{m:?}");
        }
        // "code" stays white in every mood.
        let px = canvas_mood(SNAIL, 5, track_for(SNAIL, 100), 9, Mood::Denied);
        assert!(px.iter().flat_map(|r| r[..SPLIT * 6].iter().flatten()).all(|&c| c == SOFT));
    }

    #[test]
    fn the_shell_turns_slowly_walking_fast_running_and_faster_running_hard() {
        // Quarter turns a second: a frame is 50 ms divided by the pace.
        let qps = |m: Mood| m.spin() * 20.0 * m.speed();
        assert!((0.2..0.6).contains(&qps(Mood::Calm)), "still: {}", qps(Mood::Calm));
        assert!((0.5..1.5).contains(&qps(Mood::Start)) && (0.5..1.5).contains(&qps(Mood::Done)));
        assert!((2.5..6.0).contains(&qps(Mood::Run)), "running: {}", qps(Mood::Run));
        assert!(qps(Mood::Hard) > qps(Mood::Run) * 1.6, "running hard: {}", qps(Mood::Hard));
        assert!(qps(Mood::Turbo) > qps(Mood::Hard) * 1.4, "turbo: {}", qps(Mood::Turbo));
        // It shows: the spiral of the shell changes much less often in a walk than in a run.
        let changes = |m: Mood| {
            let shell = |i: usize| SNAIL.sprite(0, (i as f32 * m.spin()) as usize);
            (1..200).filter(|&i| shell(i) != shell(i - 1)).count()
        };
        assert!(changes(Mood::Calm) * 2 < changes(Mood::Run) && changes(Mood::Run) < changes(Mood::Turbo) + 1);
    }
}
