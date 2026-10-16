//! Converte uma tela do terminal (a grade de células) em HTML colorido, para
//! olhar o resultado de verdade numa imagem.

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use std::fmt::Write;

fn hex(c: Color, default: &str) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Red | Color::LightRed => "#ff5c5c".into(),
        Color::Green | Color::LightGreen => "#56cc80".into(),
        Color::Yellow | Color::LightYellow => "#f0be50".into(),
        Color::Blue | Color::LightBlue => "#2f7bff".into(),
        Color::Cyan | Color::LightCyan => "#4fd1e8".into(),
        Color::White => "#ffffff".into(),
        Color::Black => "#000000".into(),
        Color::Gray | Color::DarkGray => "#8a96ab".into(),
        _ => default.into(),
    }
}


/// The screen as an HTML page.
pub fn html(buf: &Buffer) -> String {
    let (bg0, fg0) = ("#0d1117", "#c9d1d9");
    let mut html = format!(
        "<!doctype html><meta charset=utf-8><body style='margin:0;background:{bg0}'><pre style=\"margin:0;padding:10px;font:14px/1 'Cascadia Mono',Consolas,monospace;color:{fg0};background:{bg0}\">"
    );
    for y in 0..buf.area.height {
        let mut x = 0;
        while x < buf.area.width {
            let c = &buf[(x, y)];
            let key = (c.fg, c.bg, c.modifier);
            let mut text = String::new();
            while x < buf.area.width && (buf[(x, y)].fg, buf[(x, y)].bg, buf[(x, y)].modifier) == key {
                text.push_str(buf[(x, y)].symbol());
                x += 1;
            }
            let esc = text.replace('&', "&amp;").replace('<', "&lt;");
            let mut style = format!("color:{};background:{};", hex(key.0, fg0), hex(key.1, bg0));
            if key.2.contains(Modifier::BOLD) {
                style.push_str("font-weight:700;");
            }
            if key.2.contains(Modifier::DIM) {
                style.push_str("opacity:.65;");
            }
            let _ = write!(html, "<span style=\"{style}\">{esc}</span>");
        }
        html.push('\n');
    }
    html.push_str("</pre>");
    html
}
