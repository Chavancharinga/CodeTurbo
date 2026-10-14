//! Styled text for the interface: ANSI-colored strings (the diffs and lines
//! `view` renders for the plain terminal) turned into ratatui lines, and
//! lines wrapped to the screen width.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

fn basic(n: u16) -> Color {
    const COLORS: [Color; 8] =
        [Color::Black, Color::Red, Color::Green, Color::Yellow, Color::Blue, Color::Magenta, Color::Cyan, Color::Gray];
    const BRIGHT: [Color; 8] = [
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    match n {
        30..=37 | 40..=47 => COLORS[(n % 10) as usize],
        _ => BRIGHT[(n % 10) as usize],
    }
}

/// Applies one SGR sequence (`1;38;2;r;g;b`) to `style`.
fn apply_sgr(style: Style, params: &str) -> Style {
    let nums: Vec<u16> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    let mut s = style;
    let mut i = 0;
    while i < nums.len() {
        match nums[i] {
            0 => s = Style::default(),
            1 => s = s.add_modifier(Modifier::BOLD),
            2 => s = s.add_modifier(Modifier::DIM),
            3 => s = s.add_modifier(Modifier::ITALIC),
            4 => s = s.add_modifier(Modifier::UNDERLINED),
            22 => s = s.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => s = s.remove_modifier(Modifier::ITALIC),
            24 => s = s.remove_modifier(Modifier::UNDERLINED),
            n @ (30..=37 | 90..=97) => s = s.fg(basic(n)),
            n @ (40..=47 | 100..=107) => s = s.bg(basic(n)),
            39 => s.fg = None,
            49 => s.bg = None,
            n @ (38 | 48) => {
                let color = match nums.get(i + 1) {
                    Some(2) if i + 4 < nums.len() => {
                        let c = Color::Rgb(nums[i + 2] as u8, nums[i + 3] as u8, nums[i + 4] as u8);
                        i += 4;
                        Some(c)
                    }
                    Some(5) if i + 2 < nums.len() => {
                        let c = Color::Indexed(nums[i + 2] as u8);
                        i += 2;
                        Some(c)
                    }
                    _ => None,
                };
                if let Some(c) = color {
                    s = if n == 38 { s.fg(c) } else { s.bg(c) };
                }
            }
            _ => {}
        }
        i += 1;
    }
    s
}

/// Appends `text` to `line` in `style`, merging with the last span when
/// the style is the same.
pub fn push(line: &mut Line<'static>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(last) = line.spans.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(text);
        return;
    }
    line.spans.push(Span::styled(text.to_string(), style));
}

/// One line of ANSI-colored text. Color sequences become styles; other
/// escape sequences are dropped.
pub fn ansi_line(s: &str) -> Line<'static> {
    let mut line = Line::default();
    let mut style = Style::default();
    let mut text = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if c != '\n' && c != '\r' {
                text.push(c);
            }
            continue;
        }
        if chars.peek() != Some(&'[') {
            continue;
        }
        chars.next();
        let mut params = String::new();
        let mut fin = None;
        for c in chars.by_ref() {
            if ('\x40'..='\x7e').contains(&c) {
                fin = Some(c);
                break;
            }
            params.push(c);
        }
        if fin == Some('m') {
            push(&mut line, &text, style);
            text.clear();
            style = apply_sgr(style, &params);
        }
    }
    push(&mut line, &text, style);
    line
}

/// ANSI-colored text with line breaks, one line per row.
pub fn ansi_lines(s: &str) -> Vec<Line<'static>> {
    s.lines().map(ansi_line).collect()
}

/// Display width of a line in cells.
pub fn width(line: &Line) -> usize {
    line.spans.iter().flat_map(|s| s.content.chars()).map(|c| c.width().unwrap_or(0)).sum()
}

fn build(cells: &[(char, Style)]) -> Line<'static> {
    let mut line = Line::default();
    let mut buf = [0u8; 4];
    for &(c, style) in cells {
        push(&mut line, c.encode_utf8(&mut buf), style);
    }
    line
}

/// Splits `line` into rows at most `width` cells wide, breaking after the
/// last space that fits when there is one.
pub fn wrap(line: &Line, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let base = line.style;
    let cells: Vec<(char, Style)> = line
        .spans
        .iter()
        .flat_map(|s| {
            let style = base.patch(s.style);
            s.content.chars().map(move |c| (if c == '\t' { ' ' } else { c }, style))
        })
        .collect();
    let mut rows = Vec::new();
    let mut row: Vec<(char, Style)> = Vec::new();
    let mut row_w = 0;
    for cell in cells {
        let w = cell.0.width().unwrap_or(0);
        if row_w + w > width && !row.is_empty() {
            // Carry the unfinished word to the next row.
            let cut = row.iter().rposition(|(c, _)| *c == ' ').map(|i| i + 1).filter(|&i| i < row.len());
            let carry = match cut {
                Some(i) => row.split_off(i),
                None => Vec::new(),
            };
            rows.push(build(&row));
            row = carry;
            row_w = row.iter().map(|(c, _)| c.width().unwrap_or(0)).sum();
        }
        row.push(cell);
        row_w += w;
    }
    rows.push(build(&row));
    rows
}
