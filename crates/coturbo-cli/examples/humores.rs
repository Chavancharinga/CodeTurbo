//! Gera uma folha SVG com o caracol em cada humor, em vários quadros.
//! Uso: cargo run -q -p coturbo-cli --example humores > humores.svg

use coturbo_cli::splash::{Mood, pixels_mood};

fn main() {
    let moods = [
        (Mood::Calm, "Calm - abre / ocioso: anda, quase sem rastro"),
        (Mood::Run, "Run - pediu codigo: corre"),
        (Mood::Hard, "Hard - ferramentas / REA: corre muito"),
        (Mood::Turbo, "Turbo - o mais rapido: rastro azul com amarelo + faiscas"),
        (Mood::Alert, "Alert - alerta: amarelo"),
        (Mood::Denied, "Denied - design negado: vermelho"),
    ];
    let cell = 5;
    let frames = [0usize, 3, 6, 9];
    let sample = pixels_mood(0, 40, 100, Mood::Run);
    let (w, h) = (sample[0].len() * cell, sample.len() * cell);
    let row_h = h + 34;
    let col_w = w + 16;
    let total_w = col_w * frames.len() + 20;
    let mut svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{total_w}' height='{}' style='background:#0b0f17'><style>text{{font:13px Consolas,monospace;fill:#9fb0c8}}</style>",
        row_h * moods.len() + 14
    );
    for (r, (mood, label)) in moods.iter().enumerate() {
        let y0 = 14 + r * row_h;
        svg.push_str(&format!("<text x='10' y='{}'>{label}</text>", y0 + 12));
        for (c, &f) in frames.iter().enumerate() {
            let px = pixels_mood(f, 40, 100, *mood);
            for (y, row) in px.iter().enumerate() {
                for (x, p) in row.iter().enumerate() {
                    if let Some((rr, g, b)) = p {
                        svg.push_str(&format!(
                            "<rect x='{}' y='{}' width='{cell}' height='{cell}' fill='rgb({rr},{g},{b})'/>",
                            10 + c * col_w + x * cell,
                            y0 + 22 + y * cell
                        ));
                    }
                }
            }
        }
    }
    svg.push_str("</svg>");
    println!("{svg}");
}
