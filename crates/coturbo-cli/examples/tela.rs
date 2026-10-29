//! Desenha uma tela do CodeTurbo com uma tarefa em andamento (etapas, checklist
//! e depurador) e grava como HTML colorido, para ver o resultado de verdade.
//!
//!     cargo run -p coturbo-cli --example tela -- tela.html [largura altura]

#[path = "screen_html/mod.rs"]
mod screen_html;

use coturbo_cli::splash::Mood;
use coturbo_cli::tui::app::App;
use coturbo_cli::tui::debugger::{Finding, render};
use coturbo_cli::tui::{Out, StageState, StageUpdate, Status, TaskItem, TaskState};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::text::Line;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "tela.html".into());
    let w: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(112);
    let h: u16 = args.next().and_then(|a| a.parse().ok()).unwrap_or(40);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (cancel, _crx) = tokio::sync::mpsc::unbounded_channel();
    let status = Status {
        provider: "claude-code".into(),
        model: "sonnet".into(),
        approval: "edição livre".into(),
        usage: "tokens ↑12.4k ⟳8.1k ↓2.2k".into(),
        folder: r"C:\Users\danie\Music\CoTurbon\sandbox\contador".into(),
        ..Status::default()
    };
    let mut app = App::new(status, tx, cancel, Arc::new(AtomicBool::new(false)));
    app.apply(Out::Lines(vec![
        Line::raw("› crie um contador de palavras em rust, com testes"),
        Line::default(),
        Line::raw("  Vou criar o projeto e os testes, depois verifico."),
    ]));
    // The debugger of a failed check, on a real file.
    let dir = std::env::temp_dir().join("codeturbo-tela");
    let _ = std::fs::create_dir_all(dir.join("src"));
    let _ = std::fs::write(
        dir.join("src/main.rs"),
        "use std::fs;\n\nfn contar(texto: &str) -> usize {\n    let palavras = texto.split_whitespace().count();\n    let total: usize = palavras +;\n    total\n}\n\nfn main() {\n    println!(\"{}\", contar(\"oi mundo\"));\n}\n",
    );
    let found = [Finding {
        error: true,
        path: "src/main.rs".into(),
        line: 5,
        col: 36,
        message: "expected expression, found `;`".into(),
    }];
    app.apply(Out::Lines(render(&found, &dir, w as usize - 4)));
    app.apply(Out::Busy(Some("think · round 3".into())));
    app.apply(Out::Mood(Mood::Hard));
    let now = |s: &str, st, note: &str, exp: Option<f32>| {
        let mut u = StageUpdate::new(s, st, note);
        u.expected = exp;
        Out::Stage(u)
    };
    app.apply(now("WRITE", StageState::Done, "src/main.rs", None));
    app.apply(now("CHECK", StageState::Failed, "1 error(s)", None));
    app.apply(now("FIX", StageState::Done, "tentativa 1/3", None));
    app.apply(now("CLIPPY", StageState::Done, "3.1s", Some(3.0)));
    app.apply(now("TEST", StageState::Running, "2/2", Some(6.0)));
    app.apply(Out::Tasks(vec![
        TaskItem {
            title: "criar o projeto cargo".into(),
            state: TaskState::Done,
        },
        TaskItem {
            title: "implementar contar() e o main".into(),
            state: TaskState::Done,
        },
        TaskItem {
            title: "escrever 3 testes unitários".into(),
            state: TaskState::Doing,
        },
        TaskItem {
            title: "rodar a verificação".into(),
            state: TaskState::Pending,
        },
    ]));
    std::thread::sleep(Duration::from_millis(1700));
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
    term.draw(|f| app.draw(f)).expect("draw");
    let html = screen_html::html(term.backend().buffer());
    std::fs::write(&out, html).expect("write html");
    eprintln!("wrote {out} ({w}x{h})");
}
