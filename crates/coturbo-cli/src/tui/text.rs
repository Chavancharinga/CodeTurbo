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
fn width_of(line: &Line) -> usize {
    width(line)
}

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
/// `line` cut to `width` columns, with an ellipsis, instead of wrapped: code
/// and diffs stay one row per line however narrow the room.
pub fn clip(line: &Line, width: usize) -> Line<'static> {
    if width_of(line) <= width {
        return Line { spans: line.spans.iter().map(|s| Span::styled(s.content.to_string(), s.style)).collect(), ..Line::default() };
    }
    let mut left = width.saturating_sub(1);
    let mut spans: Vec<Span<'static>> = Vec::new();
    for s in &line.spans {
        if left == 0 {
            break;
        }
        let taken: String = s.content.chars().take(left).collect();
        left -= taken.chars().count();
        spans.push(Span::styled(taken, s.style));
    }
    spans.push(Span::raw("…"));
    Line::from(spans)
}

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

const HEADING: Color = Color::Rgb(47, 123, 255);
const CODE: Color = Color::Rgb(130, 200, 255);

/// Whether a line opens or closes a fenced code block.
pub fn is_fence(s: &str) -> bool {
    s.trim_start().starts_with("```")
}

/// Inline markdown of one line: `**bold**`, `*italic*`/`_italic_` and
/// `` `code` ``, with the markers removed.
fn inline(line: &mut Line<'static>, s: &str, base: Style) {
    let chars: Vec<char> = s.chars().collect();
    let (mut bold, mut italic, mut code) = (false, false, false);
    let mut buf = String::new();
    let style = |b: bool, i: bool, c: bool| {
        let mut st = base;
        if b {
            st = st.add_modifier(Modifier::BOLD);
        }
        if i {
            st = st.add_modifier(Modifier::ITALIC);
        }
        if c {
            st = st.fg(CODE);
        }
        st
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let toggle = if c == '`' {
            Some(2)
        } else if code {
            None
        } else if (c == '*' && next == Some('*')) || (c == '_' && next == Some('_')) {
            Some(0)
        } else if (c == '*' || c == '_')
            && (italic || next.is_some_and(|n| !n.is_whitespace()))
            && (c == '*' || italic || i == 0 || !chars[i - 1].is_alphanumeric())
        {
            Some(1)
        } else {
            None
        };
        match toggle {
            Some(kind) => {
                push(line, &buf, style(bold, italic, code));
                buf.clear();
                match kind {
                    0 => {
                        bold = !bold;
                        i += 1;
                    }
                    1 => italic = !italic,
                    _ => code = !code,
                }
            }
            None => buf.push(c),
        }
        i += 1;
    }
    push(line, &buf, style(bold, italic, code));
}

/// One line of model text rendered from markdown: headings without `#`,
/// bullets as `•`, emphasis and inline code styled; inside a code block
/// (`code`) the text is shown as is.
pub fn markdown_line(s: &str, code: bool, base: Style) -> Line<'static> {
    let mut line = Line::default();
    if code {
        push(&mut line, &format!("  {s}"), base.fg(CODE));
        return line;
    }
    if is_fence(s) {
        return line;
    }
    let trimmed = s.trim_start();
    let indent = &s[..s.len() - trimmed.len()];
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
        inline(&mut line, trimmed[hashes..].trim(), base.fg(HEADING).add_modifier(Modifier::BOLD));
        return line;
    }
    if let Some(rest) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).or_else(|| trimmed.strip_prefix("+ ")) {
        push(&mut line, &format!("{indent}• "), base);
        inline(&mut line, rest, base);
        return line;
    }
    if trimmed.chars().all(|c| c == '-' || c == '*' || c == '_') && trimmed.len() >= 3 {
        push(&mut line, &"─".repeat(24), base.add_modifier(Modifier::DIM));
        return line;
    }
    inline(&mut line, s, base);
    line
}
