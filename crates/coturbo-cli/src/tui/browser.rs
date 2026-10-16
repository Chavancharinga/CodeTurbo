//! A mini janela do navegador dentro do terminal: mostra a página que o agente
//! está usando, desenhada com meio-blocos (dois pixels por célula, como o
//! logo), com o ponteiro do mouse andando até o alvo e o clique pulsando.
//! Funciona em qualquer terminal. Onde o terminal desenha imagens (Kitty,
//! iTerm2, Sixel), a página aparece como imagem de verdade, nítida, com o
//! ponteiro desenhado nela; nos outros, em meios-blocos.

use coturbo_browse::{Frame, Label};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{FilterType, Resize, StatefulImage};
use std::time::{Duration, Instant};

const BLUE: Color = Color::Rgb(47, 123, 255);
const DIM: Color = Color::Rgb(110, 125, 150);
const BORDER: Color = Color::Rgb(36, 66, 130);
/// How long the window stays after the last picture: until the user closes it
/// (Esc) or a new task starts; ten minutes is only the limit for a forgotten one.
const LINGER: Duration = Duration::from_secs(600);
/// The window grows from the corner in this long, and shrinks back in [`CLOSE`].
const OPEN: Duration = Duration::from_millis(520);
const CLOSE: Duration = Duration::from_millis(340);
/// The page shows from this point of the opening, top to bottom.
const PAGE_FROM: f32 = 0.3;
/// How long the pointer takes to reach a new target.
const TRAVEL: Duration = Duration::from_millis(380);
const PRESS: Duration = Duration::from_millis(260);
const RIPPLE: Duration = Duration::from_millis(720);

/// A picture decoded to RGB, row by row.
#[derive(Debug, Clone)]
struct Pixels {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

/// Decodes a PNG (RGB or RGBA, 8 bits) to RGB.
fn decode(png_bytes: &[u8]) -> Option<Pixels> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Indexed => return None,
    };
    let mut rgb = Vec::with_capacity(w * h * 3);
    for px in buf[..info.buffer_size()].chunks_exact(channels) {
        match channels {
            1 | 2 => rgb.extend([px[0]; 3]),
            _ => rgb.extend(&px[..3]),
        }
    }
    Some(Pixels { w, h, rgb })
}

impl Pixels {
    /// Averages the picture down to `out_w` by `out_h` pixels.
    fn scaled(&self, out_w: usize, out_h: usize) -> Vec<(u8, u8, u8)> {
        let mut out = Vec::with_capacity(out_w * out_h);
        for oy in 0..out_h {
            let (y0, y1) = (oy * self.h / out_h, ((oy + 1) * self.h / out_h).max(oy * self.h / out_h + 1).min(self.h));
            for ox in 0..out_w {
                let (x0, x1) = (ox * self.w / out_w, ((ox + 1) * self.w / out_w).max(ox * self.w / out_w + 1).min(self.w));
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * self.w + x) * 3;
                        r += u32::from(self.rgb[i]);
                        g += u32::from(self.rgb[i + 1]);
                        b += u32::from(self.rgb[i + 2]);
                        n += 1;
                    }
                }
                let n = n.max(1);
                out.push(((r / n) as u8, (g / n) as u8, (b / n) as u8));
            }
        }
        out
    }
}

/// The terminal's way of showing real pictures, and the page in it.
struct Native {
    picker: Picker,
    page: Option<StatefulProtocol>,
}

impl std::fmt::Debug for Native {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Native")
    }
}

/// Paints the agent's pointer (and, on a click, a ring) into the page's pixels.
fn bake_pointer(px: &mut Pixels, at: (f32, f32), size: (u32, u32), clicked: bool) {
    let k = px.w as f32 / size.0.max(1) as f32;
    let (cx, cy) = (at.0 * k, at.1 * k);
    let blend = |px: &mut Pixels, x: i32, y: i32, c: [u8; 3], a: f32| {
        if x >= 0 && y >= 0 && (x as usize) < px.w && (y as usize) < px.h {
            let i = (y as usize * px.w + x as usize) * 3;
            for (n, v) in c.iter().enumerate() {
                px.rgb[i + n] = (f32::from(px.rgb[i + n]) * (1.0 - a) + f32::from(*v) * a) as u8;
            }
        }
    };
    let unit = (px.w as f32 / 520.0).max(1.0);
    if clicked {
        let r = 20.0 * unit;
        for y in (cy - r - 3.0) as i32..=(cy + r + 3.0) as i32 {
            for x in (cx - r - 3.0) as i32..=(cx + r + 3.0) as i32 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d <= r {
                    blend(px, x, y, [255, 214, 80], if d >= r - 3.0 * unit { 0.95 } else { 0.28 });
                }
            }
        }
    }
    // An arrow, in the page's own pixels: black edge, white body.
    let arrow = [(0.0, 0.0), (0.0, 17.0), (4.2, 13.2), (7.2, 20.0), (9.8, 18.8), (6.9, 12.4), (12.4, 12.0)];
    let inside = |x: f32, y: f32| {
        let mut c = false;
        let mut j = arrow.len() - 1;
        for i in 0..arrow.len() {
            let (a, b) = (arrow[i], arrow[j]);
            if (a.1 > y) != (b.1 > y) && x < (b.0 - a.0) * (y - a.1) / (b.1 - a.1) + a.0 {
                c = !c;
            }
            j = i;
        }
        c
    };
    let edge = 1.6;
    for y in (cy as i32 - 2)..(cy as i32 + (24.0 * unit) as i32) {
        for x in (cx as i32 - 2)..(cx as i32 + (16.0 * unit) as i32) {
            let (ax, ay) = ((x as f32 - cx) / unit, (y as f32 - cy) / unit);
            if inside(ax, ay) {
                blend(px, x, y, [255, 255, 255], 1.0);
            } else if [(edge, 0.0), (-edge, 0.0), (0.0, edge), (0.0, -edge), (edge, edge), (-edge, -edge), (edge, -edge), (-edge, edge)]
                .iter()
                .any(|(dx, dy)| inside(ax + dx, ay + dy))
            {
                blend(px, x, y, [17, 17, 17], 1.0);
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct BrowserView {
    /// The terminal draws real pictures: the page is one, not blocks.
    native: Option<Native>,
    pixels: Option<Pixels>,
    url: String,
    title: String,
    caption: String,
    /// The page's window in page pixels.
    size: (u32, u32),
    /// The pointer: where it was, where it goes, and since when.
    from: Option<(f32, f32)>,
    to: Option<(f32, f32)>,
    moved_at: Option<Instant>,
    clicked_at: Option<Instant>,
    touched: Option<Instant>,
    /// When the window began to open.
    opened_at: Option<Instant>,
    /// The texts of the page, drawn over the picture so it can be read.
    labels: Vec<Label>,
    /// Nothing is running: the caption says how to close the window.
    idle: bool,
    steps: usize,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Black or white, whichever reads better on `bg`.
fn readable_on(bg: (u8, u8, u8)) -> Color {
    let lum = 0.299 * f32::from(bg.0) + 0.587 * f32::from(bg.1) + 0.114 * f32::from(bg.2);
    if lum > 140.0 { Color::Rgb(24, 24, 28) } else { Color::Rgb(245, 245, 248) }
}

impl BrowserView {
    /// Writes the page's own texts over the picture, where they are on the
    /// page and in colours that read on what is behind them: a headline, a
    /// button, a menu. The picture alone is only a blur of colours.
    fn overlay(&self, buf: &mut Buffer, grid: &[(u8, u8, u8)], (out_w, out_h): (usize, usize), (ox, oy): (u16, u16), scale: f32, rows: usize) {
        let mut taken = vec![vec![false; out_w]; out_h / 2];
        let mut labels: Vec<&Label> = self.labels.iter().collect();
        labels.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap_or(std::cmp::Ordering::Equal));
        for l in labels {
            let col = (l.x * scale).round().max(0.0) as usize;
            let room = ((l.w * scale).round() as usize).max(1);
            // Wide texts show their start; narrow ones need a few cells to say anything.
            let want = room.max(if l.kind.starts_with('h') { room } else { 4 }).min(out_w.saturating_sub(col));
            if col >= out_w || want < 3 || room < 2 {
                continue;
            }
            let tall = l.h * scale / 2.0 > 3.0;
            let row0 = if tall { (l.y * scale / 2.0).round() } else { ((l.y + l.h / 2.0) * scale / 2.0).round() }.max(0.0) as usize;
            let text: Vec<char> = l.text.chars().filter(|c| !c.is_control()).take(want).collect();
            // Try the row, then the next one, when something is already written there.
            let Some(row) = [row0, row0 + 1].into_iter().find(|&r| r < rows && taken[r][col..(col + text.len()).min(out_w)].iter().all(|t| !t)) else { continue };
            let mut style = Modifier::empty();
            if l.kind.starts_with('h') || l.kind == "button" || l.kind == "strong" {
                style |= Modifier::BOLD;
            }
            if l.kind == "a" {
                style |= Modifier::UNDERLINED;
            }
            for (i, ch) in text.iter().enumerate() {
                let c = col + i;
                if c >= out_w {
                    break;
                }
                let (top, bottom) = (grid[row * 2 * out_w + c], grid[(row * 2 + 1).min(out_h - 1) * out_w + c]);
                let bg = ((top.0 / 2 + bottom.0 / 2), (top.1 / 2 + bottom.1 / 2), (top.2 / 2 + bottom.2 / 2));
                let cell = &mut buf[(ox + c as u16, oy + row as u16)];
                cell.set_symbol(&ch.to_string()).set_fg(readable_on(bg)).set_bg(Color::Rgb(bg.0, bg.1, bg.2)).set_style(Style::new().add_modifier(style));
                taken[row][c] = true;
            }
        }
    }

    /// The terminal can show real pictures: use them for the page.
    pub fn set_picker(&mut self, picker: Picker) {
        self.native = Some(Native { picker, page: None });
    }

    pub fn has_native(&self) -> bool {
        self.native.is_some()
    }

    pub fn apply(&mut self, f: Frame, now: Instant) {
        if let Some(p) = decode(&f.png) {
            if let Some(n) = self.native.as_mut() {
                let mut shot = p.clone();
                if let Some(c) = f.cursor {
                    bake_pointer(&mut shot, c, f.size, f.clicked);
                }
                if let Some(img) = image::RgbImage::from_raw(shot.w as u32, shot.h as u32, shot.rgb) {
                    n.page = Some(n.picker.new_resize_protocol(image::DynamicImage::ImageRgb8(img)));
                }
            }
            self.pixels = Some(p);
        }
        self.url = f.url;
        self.title = f.title;
        self.caption = f.caption;
        self.size = f.size;
        self.labels = f.labels;
        // The pointer sets off from where it is drawn now.
        let here = self.pointer(now);
        if f.cursor != self.to {
            self.from = here.or(f.cursor);
            self.to = f.cursor;
            self.moved_at = Some(now);
        }
        if f.clicked {
            self.clicked_at = Some(now);
        }
        // A window that is not on screen opens; one that is closing opens again from where it is.
        let p = self.progress(now);
        self.opened_at = Some(if p >= 1.0 { self.opened_at.unwrap_or(now) } else { now.checked_sub(OPEN.mul_f32(p)).unwrap_or(now) });
        self.touched = Some(now);
        self.steps += 1;
    }

    pub fn clear(&mut self) {
        // What the terminal can draw does not change with the task.
        let native = self.native.take().map(|n| Native { page: None, ..n });
        *self = Self { native, ..Self::default() };
    }

    pub fn set_idle(&mut self, idle: bool) {
        self.idle = idle;
    }

    /// The user closes the window: it shrinks away.
    pub fn dismiss(&mut self, now: Instant) {
        if self.visible(now) {
            self.touched = now.checked_sub(LINGER);
        }
    }

    /// Whether the window is on screen: a while after the last picture, and
    /// the moment it takes to close.
    pub fn visible(&self, now: Instant) -> bool {
        self.pixels.is_some() && self.touched.is_some_and(|t| now.saturating_duration_since(t) < LINGER + CLOSE)
    }

    /// How open the window is, 0 to 1: it opens from the corner, stays, and closes.
    pub fn progress(&self, now: Instant) -> f32 {
        let (Some(opened), Some(touched)) = (self.opened_at, self.touched) else { return 0.0 };
        if self.pixels.is_none() {
            return 0.0;
        }
        let idle = now.saturating_duration_since(touched);
        if idle >= LINGER {
            return 1.0 - ease((idle - LINGER).as_secs_f32() / CLOSE.as_secs_f32());
        }
        ease(now.saturating_duration_since(opened).as_secs_f32() / OPEN.as_secs_f32())
    }

    /// Where the pointer is drawn at `now`, in page pixels.
    pub fn pointer(&self, now: Instant) -> Option<(f32, f32)> {
        let to = self.to?;
        let (from, at) = (self.from.unwrap_or(to), self.moved_at?);
        let t = now.saturating_duration_since(at).as_secs_f32() / TRAVEL.as_secs_f32();
        let e = ease(t);
        Some((from.0 + (to.0 - from.0) * e, from.1 + (to.1 - from.1) * e))
    }

    /// Rows the window needs to show the page whole at `cols` columns.
    pub fn rows_for(&self, cols: u16) -> u16 {
        let (w, h) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
        let inner = cols.saturating_sub(2) as f32;
        // Two pixels per row, plus the address bar, the caption and the frame.
        (inner * h / w / 2.0).ceil() as u16 + 4
    }

    /// The area of the page inside the window drawn at `rect`.
    fn page_in(rect: Rect) -> Rect {
        let inner = Block::new().borders(Borders::ALL).inner(rect);
        Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: inner.height.saturating_sub(2) }
    }

    /// With real pictures: once the window is fully open, the page is drawn as
    /// one (the cells below it stay as the frame).
    pub fn draw_native(&mut self, f: &mut ratatui::Frame, rect: Rect, now: Instant) {
        if self.progress(now) < 1.0 || rect.width < 12 || rect.height < 6 {
            return;
        }
        if let Some(Native { page: Some(proto), .. }) = self.native.as_mut() {
            f.render_stateful_widget(StatefulImage::default().resize(Resize::Fit(Some(FilterType::Lanczos3))), Self::page_in(rect), proto);
        }
    }

    /// Draws the window into `rect`, as open as it is at `now`.
    pub fn draw(&self, buf: &mut Buffer, rect: Rect, now: Instant) {
        let p = self.progress(now);
        if p <= 0.0 || rect.width < 12 || rect.height < 6 {
            return;
        }
        // It grows from the top right corner; the page appears top to bottom once the frame is there.
        let (w, h) = (((rect.width as f32 * p).round() as u16).max(12).min(rect.width), ((rect.height as f32 * p).round() as u16).max(6).min(rect.height));
        let rect = Rect { x: rect.right() - w, y: rect.y, width: w, height: h };
        let reveal = if p >= 1.0 { 1.0 } else { ((p - PAGE_FROM) / (1.0 - PAGE_FROM)).clamp(0.0, 1.0) };
        let block = Block::new().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::new().fg(BORDER));
        let inner = block.inner(rect);
        block.render(rect, buf);
        // The title bar: three dots and the address.
        let shown = if self.url.is_empty() { "about:blank".to_string() } else { self.url.replace("file:///", "").replace("file://", "") };
        let room = inner.width.saturating_sub(7) as usize;
        let addr: String = if shown.chars().count() > room { format!("…{}", shown.chars().rev().take(room.saturating_sub(1)).collect::<Vec<_>>().into_iter().rev().collect::<String>()) } else { shown };
        Line::from(vec![
            Span::styled("● ", Style::new().fg(Color::Rgb(255, 95, 86))),
            Span::styled("● ", Style::new().fg(Color::Rgb(255, 189, 46))),
            Span::styled("● ", Style::new().fg(Color::Rgb(39, 201, 63))),
            Span::styled(addr, Style::new().fg(DIM)),
        ])
        .render(Rect { height: 1, ..inner }, buf);
        let page = Rect { x: inner.x, y: inner.y + 1, width: inner.width, height: inner.height.saturating_sub(2) };
        // The caption under the page.
        let caption_row = Rect { x: inner.x, y: inner.bottom().saturating_sub(1), width: inner.width, height: 1 };
        let caption = if self.idle { format!("{} · Esc fecha", self.caption) } else { self.caption.clone() };
        let cap: String = caption.chars().take(inner.width as usize).collect();
        Line::styled(cap, Style::new().fg(BLUE).add_modifier(Modifier::BOLD)).render(caption_row, buf);
        let Some(px) = &self.pixels else { return };
        // A real picture takes the page once the window is open.
        if p >= 1.0 && self.native.as_ref().is_some_and(|n| n.page.is_some()) {
            return;
        }
        // Fit the page in the space, keeping its shape: a cell is one pixel wide and two tall.
        let (vw, vh) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
        let scale = (page.width as f32 / vw).min(page.height as f32 * 2.0 / vh);
        let out_w = ((vw * scale).round() as usize).clamp(1, page.width as usize);
        let out_h = (((vh * scale).round() as usize).clamp(2, page.height as usize * 2)) & !1;
        let grid = px.scaled(out_w, out_h);
        let (ox, oy) = (page.x + ((page.width as usize - out_w) / 2) as u16, page.y);
        let shown_rows = ((out_h / 2) as f32 * reveal).ceil() as usize;
        for row in 0..shown_rows.min(out_h / 2) {
            for col in 0..out_w {
                let (t, b) = (grid[row * 2 * out_w + col], grid[(row * 2 + 1) * out_w + col]);
                let cell = &mut buf[(ox + col as u16, oy + row as u16)];
                cell.set_symbol("▀").set_fg(Color::Rgb(t.0, t.1, t.2)).set_bg(Color::Rgb(b.0, b.1, b.2));
            }
        }
        self.overlay(buf, &grid, (out_w, out_h), (ox, oy), scale, shown_rows.min(out_h / 2));
        // The pointer, and the click.
        let Some((cx, cy)) = self.pointer(now) else { return };
        let (mx, my) = ((cx * scale).round() as i32, (cy * scale / 2.0).round() as i32);
        let at = |dx: i32, dy: i32| -> Option<(u16, u16)> {
            let (x, y) = (mx + dx, my + dy);
            (x >= 0 && y >= 0 && (x as usize) < out_w && (y as usize) < shown_rows.min(out_h / 2)).then(|| (ox + x as u16, oy + y as u16))
        };
        let mut put = |p: Option<(u16, u16)>, sym: &str, fg: Color| {
            if let Some((x, y)) = p {
                let cell = &mut buf[(x, y)];
                cell.set_symbol(sym).set_fg(fg);
            }
        };
        if let Some(t) = self.clicked_at {
            let age = now.saturating_duration_since(t);
            if age < RIPPLE {
                let yellow = Color::Rgb(255, 214, 80);
                let r = if age < PRESS { 1 } else { 2 + (age.as_millis() as i32 - PRESS.as_millis() as i32) / 230 };
                for (dx, dy, sym) in [(-r, 0, "("), (r, 0, ")"), (0, -1, "·"), (0, 1, "·")] {
                    put(at(dx, dy), sym, yellow);
                }
                if age < PRESS {
                    put(at(0, 0), "◉", yellow);
                    return;
                }
            }
        }
        put(at(0, 0), "◤", Color::White);
        // The arrow's body, so it reads against any page.
        if let Some((x, y)) = at(0, 0) {
            buf[(x, y)].set_bg(Color::Black).set_style(Style::new().add_modifier(Modifier::BOLD));
        }
    }
}
