//! The layout grid: the screen's exact width, less the side margins, halved
//! and halved again until the modules are about a finger wide, so the
//! column count is even (4 on a phone, 8 on a tablet, 16 on a desktop) and
//! every size is a multiple of 8 px. Elements then take whole cells by what
//! they are and where they show, packed into the first free place, so no
//! two ever overlap: the CSS grid draws exactly that map.

use std::fmt::Write as _;

/// Everything snaps to this, in px.
pub const UNIT: u32 = 8;
/// Halving stops before a column gets narrower than this.
const MIN_MODULE: f64 = 64.0;

fn snap(v: f64) -> u32 {
    ((v / f64::from(UNIT)).round() as u32).max(1) * UNIT
}

/// One screen's grid, in px.
#[derive(Debug, Clone, PartialEq)]
pub struct Grid {
    pub width: u32,
    pub height: u32,
    /// Side margin, each side.
    pub margin: u32,
    pub gutter: u32,
    pub columns: u32,
    /// One column's width.
    pub column: f64,
    /// One row's height: the vertical module.
    pub row: u32,
}

impl Grid {
    /// The grid for a `width`×`height` screen.
    pub fn for_viewport(width: u32, height: u32) -> Grid {
        let margin = snap((f64::from(width) * 0.05).clamp(16.0, 96.0));
        let content = f64::from(width.saturating_sub(2 * margin).max(UNIT));
        // Halve the content while both halves stay at least a module wide.
        let mut columns = 1;
        while content / f64::from(columns * 2) >= MIN_MODULE && columns < 16 {
            columns *= 2;
        }
        let module = content / f64::from(columns);
        // A gutter wider than the margin makes the edge look cramped.
        let gutter = snap(module / 4.0).min(32).min(margin);
        let column = (content - f64::from((columns - 1) * gutter)) / f64::from(columns);
        // Rows: half a column, on the 8 px beat, never under 32 (a line of text).
        let row = snap(column / 2.0).max(32);
        Grid { width, height, margin, gutter, columns, column, row }
    }

    /// Text rows that fill the screen's height below the margins.
    pub fn screen_rows(&self) -> u32 {
        (self.height.saturating_sub(2 * self.margin) / self.row).max(1)
    }

    /// Rows needed for `px` of height, with the gaps between them.
    fn rows_for(&self, px: f64) -> u32 {
        ((px + f64::from(UNIT)) / f64::from(self.row + UNIT)).ceil().max(1.0) as u32
    }

    /// Width in px of `span` columns with their gutters.
    fn span_px(&self, span: u32) -> f64 {
        self.column * f64::from(span) + f64::from(span.saturating_sub(1) * self.gutter)
    }
}

/// What an element is: it decides how many cells it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Nav,
    Title,
    Text,
    Image,
    Button,
    Card,
    Label,
    Other,
}

impl Kind {
    pub fn parse(s: &str) -> Kind {
        match s.to_ascii_lowercase().as_str() {
            "nav" | "header" | "menu" | "navbar" => Kind::Nav,
            "title" | "heading" | "h1" | "h2" | "headline" => Kind::Title,
            "text" | "paragraph" | "p" | "body" | "copy" => Kind::Text,
            "image" | "img" | "photo" | "picture" | "video" | "media" | "chart" => Kind::Image,
            "button" | "btn" | "cta" | "link" => Kind::Button,
            "card" | "panel" | "tile" => Kind::Card,
            "label" | "caption" | "tag" | "badge" | "stat" => Kind::Label,
            _ => Kind::Other,
        }
    }
}

/// One piece to place.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The CSS id it gets.
    pub id: String,
    pub kind: Kind,
    /// Its share of the row, 0-1, instead of the kind's own.
    pub share: Option<f64>,
    /// Starts a new row instead of sitting beside the one before.
    pub break_before: bool,
}

/// Columns and rows an element takes on `g`.
fn size(e: &Element, g: &Grid) -> (u32, u32) {
    let c = g.columns;
    let share = e.share.unwrap_or(match (e.kind, c) {
        (Kind::Nav, _) => 1.0,
        (_, 1..=4) if !matches!(e.kind, Kind::Button | Kind::Label) => 1.0,
        (Kind::Title, _) => 0.75,
        (Kind::Text, 8) => 0.75,
        (Kind::Text | Kind::Image | Kind::Other, _) => 0.5,
        (Kind::Card, 8) => 0.5,
        (Kind::Card, _) => 0.25,
        (Kind::Button, 1..=4) => 0.5,
        (Kind::Button | Kind::Label, 8) => 0.25,
        (Kind::Button | Kind::Label, _) => 0.1875,
    });
    let span = ((f64::from(c) * share.clamp(0.0, 1.0)).round() as u32).clamp(1, c);
    let w = g.span_px(span);
    let rows = match e.kind {
        Kind::Nav => g.rows_for(64.0),
        Kind::Title => g.rows_for(if c <= 4 { 96.0 } else { 144.0 }),
        Kind::Text => g.rows_for(160.0),
        Kind::Image => g.rows_for(w * 3.0 / 4.0),
        // A finger: 48 px at least.
        Kind::Button => g.rows_for(48.0),
        Kind::Card => g.rows_for(w * 0.8),
        Kind::Label => 1,
        Kind::Other => g.rows_for(w / 2.0),
    };
    (span, rows)
}

/// Where an element went: 1-based column and row, and how many of each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub id: String,
    pub col: u32,
    pub row: u32,
    pub span: u32,
    pub rows: u32,
}

/// Packs the elements in order into the first free cells that fit, left to
/// right and top to bottom: none can overlap another. Each starts after the
/// one before in reading order, so a later element never fills a hole above
/// an earlier one.
pub fn place(elements: &[Element], g: &Grid) -> Vec<Placed> {
    let cols = g.columns as usize;
    let mut taken: Vec<Vec<bool>> = Vec::new();
    // Where the previous element started and ended: the search starts there.
    let mut cursor = (0usize, 0usize);
    let mut out = Vec::new();
    for e in elements {
        let (span, rows) = size(e, g);
        let (sw, sh) = (span as usize, rows as usize);
        if e.break_before {
            cursor = (taken.len(), 0);
        }
        let free = |taken: &Vec<Vec<bool>>, r: usize, c: usize| c + sw <= cols && (r..r + sh).all(|y| (c..c + sw).all(|x| taken.get(y).is_none_or(|row| !row[x])));
        let (mut r, mut c) = cursor;
        while !free(&taken, r, c) {
            c += 1;
            if c + sw > cols {
                c = 0;
                r += 1;
            }
        }
        while taken.len() < r + sh {
            taken.push(vec![false; cols]);
        }
        for row in &mut taken[r..r + sh] {
            row[c..c + sw].iter_mut().for_each(|x| *x = true);
        }
        cursor = (r, c + sw);
        out.push(Placed { id: e.id.clone(), col: c as u32 + 1, row: r as u32 + 1, span, rows });
    }
    out
}

/// The screens the layout is made for: phone, tablet, desktop, and the
/// width each starts at.
pub const SCREENS: [(&str, u32, u32, u32); 3] = [("phone", 390, 844, 0), ("tablet", 768, 1024, 768), ("desktop", 1440, 900, 1200)];

fn css_id(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect()
}

/// The stylesheet: `.layout` is the grid, each element placed by its id,
/// one block per screen.
pub fn css(elements: &[Element]) -> String {
    let mut out = String::from(
        "/* Generated by CodeTurbo: the layout grid. Put each element straight inside\n   <div class=\"layout\">, with its id. Sizes are multiples of 8 px. */\n",
    );
    for (name, w, h, from) in SCREENS {
        let g = Grid::for_viewport(w, h);
        let mut block = format!(
            ".layout {{ display: grid; grid-template-columns: repeat({}, minmax(0, 1fr)); column-gap: {}px; row-gap: {}px; grid-auto-rows: minmax({}px, auto); padding-inline: {}px; align-items: start; }}\n",
            g.columns, g.gutter, UNIT, g.row - UNIT, g.margin
        );
        for p in place(elements, &g) {
            let _ = writeln!(block, "#{} {{ grid-column: {} / span {}; grid-row: {} / span {}; min-width: 0; }}", css_id(&p.id), p.col, p.span, p.row, p.rows);
        }
        if from == 0 {
            let _ = write!(out, "/* {name}: {} columns of {:.0}px, gutter {}px, margin {}px, rows {}px */\n{block}", g.columns, g.column, g.gutter, g.margin, g.row);
        } else {
            let indented: String = block.lines().map(|l| format!("  {l}\n")).collect();
            let _ = write!(out, "/* {name}: {} columns of {:.0}px, gutter {}px, margin {}px, rows {}px */\n@media (min-width: {from}px) {{\n{indented}}}\n", g.columns, g.column, g.gutter, g.margin, g.row);
        }
    }
    out
}

/// The map of one screen in text: a letter per element, `.` for free cells.
pub fn ascii(elements: &[Element], g: &Grid) -> String {
    let placed = place(elements, g);
    let height = placed.iter().map(|p| p.row + p.rows - 1).max().unwrap_or(0) as usize;
    let mut cells = vec![vec!['.'; g.columns as usize]; height];
    let mut legend = Vec::new();
    for (n, p) in placed.iter().enumerate() {
        let mark = (b'A' + (n % 26) as u8) as char;
        legend.push(format!("{mark} {} ({}×{})", p.id, p.span, p.rows));
        for row in &mut cells[(p.row - 1) as usize..(p.row + p.rows - 1) as usize] {
            row[(p.col - 1) as usize..(p.col + p.span - 1) as usize].iter_mut().for_each(|c| *c = mark);
        }
    }
    let mut out: String = cells.iter().map(|r| r.iter().collect::<String>() + "\n").collect();
    out.push_str(&legend.join(", "));
    out
}

/// A page showing the grid and the boxes on each screen, to look at.
pub fn preview_html(elements: &[Element]) -> String {
    let mut body = String::new();
    for (name, w, h, _) in SCREENS {
        let g = Grid::for_viewport(w, h);
        let scale = 360.0 / f64::from(w);
        let s = |v: f64| v * scale;
        let placed = place(elements, &g);
        let rows = placed.iter().map(|p| p.row + p.rows - 1).max().unwrap_or(1);
        let top = |r: u32| f64::from(g.margin) + f64::from((r - 1) * (g.row + UNIT));
        let total = top(rows + 1) + f64::from(g.margin);
        let x = |c: u32| f64::from(g.margin) + f64::from(c - 1) * (g.column + f64::from(g.gutter));
        let mut boxes = String::new();
        for c in 1..=g.columns {
            let _ = write!(boxes, "<i style=\"left:{:.1}px;width:{:.1}px\"></i>", s(x(c)), s(g.column));
        }
        for p in &placed {
            let (l, t) = (x(p.col), top(p.row));
            let wpx = g.span_px(p.span);
            let hpx = f64::from(p.rows * g.row + (p.rows - 1) * UNIT);
            let _ = write!(boxes, "<b style=\"left:{:.1}px;top:{:.1}px;width:{:.1}px;height:{:.1}px\">{}</b>", s(l), s(t), s(wpx), s(hpx), p.id);
        }
        let _ = write!(
            body,
            "<figure><figcaption>{name} {w}×{h} · {} columns · gutter {}px · margin {}px</figcaption><div class=\"screen\" style=\"height:{:.0}px\">{boxes}</div></figure>",
            g.columns,
            g.gutter,
            g.margin,
            s(total)
        );
    }
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Layout grid</title>\
         <style>body{{margin:0;padding:24px;background:#0d1117;color:#c9d1d9;font:14px system-ui,sans-serif}}main{{display:flex;flex-wrap:wrap;gap:32px;align-items:flex-start}}\
         figure{{margin:0}}figcaption{{margin-bottom:8px;color:#8b949e}}.screen{{position:relative;width:360px;background:#161b22;border:1px solid #30363d;border-radius:8px;overflow:hidden}}\
         .screen i{{position:absolute;top:0;bottom:0;background:rgba(88,166,255,.08)}}.screen b{{position:absolute;box-sizing:border-box;border:1px solid #58a6ff;background:rgba(88,166,255,.18);\
         border-radius:4px;font-weight:500;font-size:11px;padding:3px 5px;overflow:hidden;color:#e6edf3}}</style></head>\
         <body><h1>Layout grid</h1><main>{body}</main></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: &str, kind: &str) -> Element {
        Element { id: id.into(), kind: Kind::parse(kind), share: None, break_before: false }
    }

    #[test]
    fn halving_gives_even_columns_on_the_8px_beat() {
        let cols: Vec<u32> = SCREENS.iter().map(|s| Grid::for_viewport(s.1, s.2).columns).collect();
        assert_eq!(cols, [4, 8, 16]);
        for (_, w, h, _) in SCREENS {
            let g = Grid::for_viewport(w, h);
            assert!(g.margin.is_multiple_of(UNIT) && g.gutter.is_multiple_of(UNIT) && g.row.is_multiple_of(UNIT), "{g:?}");
            assert!(g.column >= MIN_MODULE - f64::from(g.gutter), "{g:?}");
            assert!(g.gutter <= g.margin, "{g:?}");
            // The columns, gutters and margins fill the width exactly.
            let sum = g.column * f64::from(g.columns) + f64::from((g.columns - 1) * g.gutter + 2 * g.margin);
            assert!((sum - f64::from(w)).abs() < 0.01, "{sum} vs {w}");
        }
    }

    #[test]
    fn nothing_overlaps_and_everything_fits_on_every_screen() {
        let page: Vec<Element> = [("nav", "nav"), ("title", "title"), ("intro", "text"), ("hero", "image"), ("buy", "button"), ("more", "button"), ("a", "card"), ("b", "card"), ("c", "card"), ("note", "caption")]
            .iter()
            .map(|(i, k)| el(i, k))
            .collect();
        for (_, w, h, _) in SCREENS {
            let g = Grid::for_viewport(w, h);
            let placed = place(&page, &g);
            assert_eq!(placed.len(), page.len());
            let mut seen = std::collections::HashSet::new();
            for p in &placed {
                assert!(p.col + p.span - 1 <= g.columns, "{p:?} off the grid");
                for r in p.row..p.row + p.rows {
                    for c in p.col..p.col + p.span {
                        assert!(seen.insert((r, c)), "{} overlaps at {r},{c} on {w}px", p.id);
                    }
                }
            }
        }
        let phone = place(&page, &Grid::for_viewport(390, 844));
        assert_eq!((phone[1].col, phone[1].span), (1, 4), "the title takes the phone's width");
        let desk = place(&page, &Grid::for_viewport(1440, 900));
        assert_eq!(desk[2].span, 8, "text takes half the desktop");
        assert_eq!(desk[3].row, desk[2].row, "the image sits beside the text");
    }

    #[test]
    fn the_css_places_each_element_on_each_screen() {
        let page = vec![el("title", "h1"), Element { break_before: true, ..el("go", "button") }];
        let css = css(&page);
        assert!(css.contains("repeat(4, minmax(0, 1fr))") && css.contains("@media (min-width: 768px)") && css.contains("@media (min-width: 1200px)"));
        assert!(css.contains("#title { grid-column: 1 / span 4;"));
        assert_eq!(css.matches("#go {").count(), 3);
        let map = ascii(&page, &Grid::for_viewport(390, 844));
        assert!(map.starts_with("AAAA\n") && map.contains("A title (4×") && map.contains("B go"));
        assert!(preview_html(&page).contains("16 columns"));
    }
}

