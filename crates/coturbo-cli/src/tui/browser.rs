//! A mini janela do navegador dentro do terminal: mostra a página que o agente
//! está usando, desenhada com meio-blocos (dois pixels por célula, como o
//! logo), com o ponteiro do mouse andando até o alvo e o clique pulsando.
//! Funciona em qualquer terminal: não depende de protocolo gráfico.

use coturbo_browse::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};
use std::time::{Duration, Instant};

const BLUE: Color = Color::Rgb(47, 123, 255);
const DIM: Color = Color::Rgb(110, 125, 150);
const BORDER: Color = Color::Rgb(36, 66, 130);
/// How long the window stays after the last picture.
const LINGER: Duration = Duration::from_secs(25);
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

#[derive(Debug, Default)]
pub struct BrowserView {
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
    steps: usize,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl BrowserView {
    pub fn apply(&mut self, f: Frame, now: Instant) {
        if let Some(p) = decode(&f.png) {
            self.pixels = Some(p);
        }
        self.url = f.url;
        self.title = f.title;
        self.caption = f.caption;
        self.size = f.size;
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
        self.touched = Some(now);
        self.steps += 1;
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Whether the window is on screen: a while after the last picture.
    pub fn visible(&self, now: Instant) -> bool {
        self.pixels.is_some() && self.touched.is_some_and(|t| now.saturating_duration_since(t) < LINGER)
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

    /// Draws the window into `rect`.
    pub fn draw(&self, buf: &mut Buffer, rect: Rect, now: Instant) {
        if rect.width < 12 || rect.height < 6 {
            return;
        }
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
        let cap: String = self.caption.chars().take(inner.width as usize).collect();
        Line::styled(cap, Style::new().fg(BLUE).add_modifier(Modifier::BOLD)).render(caption_row, buf);
        let Some(px) = &self.pixels else { return };
        // Fit the page in the space, keeping its shape: a cell is one pixel wide and two tall.
        let (vw, vh) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
        let scale = (page.width as f32 / vw).min(page.height as f32 * 2.0 / vh);
        let out_w = ((vw * scale).round() as usize).clamp(1, page.width as usize);
        let out_h = (((vh * scale).round() as usize).clamp(2, page.height as usize * 2)) & !1;
        let grid = px.scaled(out_w, out_h);
        let (ox, oy) = (page.x + ((page.width as usize - out_w) / 2) as u16, page.y);
        for row in 0..out_h / 2 {
            for col in 0..out_w {
                let (t, b) = (grid[row * 2 * out_w + col], grid[(row * 2 + 1) * out_w + col]);
                let cell = &mut buf[(ox + col as u16, oy + row as u16)];
                cell.set_symbol("▀").set_fg(Color::Rgb(t.0, t.1, t.2)).set_bg(Color::Rgb(b.0, b.1, b.2));
            }
        }
        // The pointer, and the click.
        let Some((cx, cy)) = self.pointer(now) else { return };
        let (mx, my) = ((cx * scale).round() as i32, (cy * scale / 2.0).round() as i32);
        let at = |dx: i32, dy: i32| -> Option<(u16, u16)> {
            let (x, y) = (mx + dx, my + dy);
            (x >= 0 && y >= 0 && (x as usize) < out_w && (y as usize) < out_h / 2).then(|| (ox + x as u16, oy + y as u16))
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
