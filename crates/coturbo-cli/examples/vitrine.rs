//! Scenes of the real screen, for screenshots: the same renderer the agent
//! uses, fed a fixed script, in a real terminal.
//!
//!     cargo run -p coturbo-cli --example vitrine -- <scene 1-5>
//!
//! 1 the browser inside the terminal · 2 code appearing as it is written ·
//! 3 the debugger · 4 the verification gate · 5 done and verified.

use coturbo_browse::{Action, Frame, Target};
use coturbo_cli::splash::Mood;
use coturbo_cli::tui::app::{App, run};
use coturbo_cli::tui::debugger::{Finding, render};
use coturbo_cli::tui::{Out, StageState, StageUpdate, Status, TaskItem, TaskState};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const SITE: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Roast & Co</title>
<style>
:root{--ink:#2a1f17;--paper:#f6efe6;--accent:#b4472b}
*{box-sizing:border-box}body{margin:0;font:17px Georgia,serif;color:var(--ink);background:var(--paper)}
header{display:flex;align-items:center;gap:28px;padding:18px 40px;border-bottom:2px solid var(--ink)}
header b{font-size:26px;letter-spacing:.04em}header a{color:var(--ink);text-decoration:none;font-size:16px}
main{display:grid;grid-template-columns:1.1fr 1fr;gap:40px;padding:40px}
.photo{height:320px;background:linear-gradient(135deg,#d9b48f,#8a5a3a);border-radius:0}
h1{margin:0 0 6px;font-size:34px}.price{font-size:22px;color:var(--accent);margin:0 0 18px}
button{font:inherit;padding:11px 22px;border:2px solid var(--ink);background:#fff;cursor:pointer}
button.main{background:var(--accent);color:#fff;border-color:var(--accent)}
label{display:block;margin:22px 0 6px;font-size:14px}input{font:inherit;padding:10px;width:100%;border:2px solid var(--ink);background:#fff}
#ok{margin-top:16px;color:#1f6b3a;min-height:24px}
</style></head><body>
<header><b>ROAST &amp; CO</b><a href="#">Beans</a><a href="#">Brewing</a><a href="#">Subscribe</a></header>
<main><div class="photo"></div><div>
<h1>Ethiopia Yirgacheffe</h1><p class="price">$ 18.00 · 250 g</p>
<button onclick="document.getElementById('ok').textContent='1 item in the cart'">Add to cart</button>
<label for="mail">Your e-mail</label><input id="mail" placeholder="you@example.com">
<p><button class="main" onclick="document.getElementById('ok').textContent='Order sent to '+document.getElementById('mail').value">Place order</button></p>
<div id="ok"></div></div></main></body></html>"##;

const CODE: [&str; 13] = [
    "function render(items) {",
    "  const list = document.querySelector('.todo-list');",
    "  list.innerHTML = '';",
    "  for (const item of items) {",
    "    const li = document.createElement('li');",
    "    li.className = item.done ? 'completed' : '';",
    "    li.innerHTML = `<input class=\"toggle\" type=\"checkbox\" ${item.done ? 'checked' : ''}>",
    "      <label>${escape(item.title)}</label><button class=\"destroy\"></button>`;",
    "    li.querySelector('.toggle').onchange = () => toggle(item.id);",
    "    li.querySelector('label').ondblclick = () => edit(li, item);",
    "    list.append(li);",
    "  }",
    "  counter.textContent = `${items.filter(i => !i.done).length} items left`;",
];

fn line(spans: Vec<Span<'static>>) -> Line<'static> {
    Line::from(spans)
}

fn tool(name: &str, rest: &str) -> Line<'static> {
    line(vec![
        Span::styled("  ✓ ", Style::new().fg(Color::Rgb(86, 204, 128))),
        Span::styled(name.to_string(), Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {rest}"), Style::new().fg(Color::Rgb(47, 123, 255))),
    ])
}

fn stage(name: &str, state: StageState, note: &str, expected: Option<f32>) -> Out {
    let mut u = StageUpdate::new(name, state, note);
    u.expected = expected;
    Out::Stage(u)
}

fn tasks(states: [TaskState; 5]) -> Out {
    let titles = ["Study the original", "Write index.html and style.css", "Write the app logic", "Run the checks", "Test it in the browser"];
    Out::Tasks(titles.iter().zip(states).map(|(t, s)| TaskItem { title: (*t).into(), state: s }).collect())
}

#[tokio::main]
async fn main() {
    let scene: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(1);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (cancel, _crx) = tokio::sync::mpsc::unbounded_channel();
    let status = Status {
        provider: "claude-code".into(),
        model: "sonnet".into(),
        approval: "auto-edit".into(),
        usage: "tokens ↑48.2k ⟳31.0k ↓6.4k".into(),
        folder: r"C:\dev\todo-app".into(),
        ..Status::default()
    };
    let mut app = App::new(status, tx, cancel, Arc::new(AtomicBool::new(false)));
    let (out_tx, out_rx) = std::sync::mpsc::channel::<Out>();
    let ask = "› recreate this todo app from its public page, then test your version in the browser";
    app.apply(Out::Lines(vec![line(vec![Span::raw(ask)]), Line::default()]));

    let mut frames: Vec<Frame> = Vec::new();
    let mut windows: Vec<Vec<Frame>> = Vec::new();
    if scene == 6 {
        let site = std::env::temp_dir().join("codeturbo-vitrine-site");
        std::fs::create_dir_all(&site).unwrap();
        let pages = [("index", "Roast & Co", "Small-batch coffee, roasted every Monday."), ("beans", "Beans", "Ethiopia · Colombia · Kenya · Brazil"), ("brewing", "Brewing", "V60, AeroPress, French press and moka."), ("contact", "Contact", "hello@roast.example · Mon–Sat 8–18")];
        for (name, title, text) in pages {
            let html = format!("<!doctype html><title>{title}</title><body style=\"margin:0;font:18px Georgia,serif;background:#f6efe6;color:#2a1f17\"><nav style=\"padding:16px 28px;border-bottom:2px solid #2a1f17;display:flex;gap:24px\"><b>ROAST &amp; CO</b><a href=\"index.html\">Home</a><a href=\"beans.html\">Beans</a><a href=\"brewing.html\">Brewing</a><a href=\"contact.html\">Contact</a></nav><main style=\"padding:40px 28px\"><h1 style=\"font-size:40px;margin:0 0 12px\">{title}</h1><p>{text}</p><div style=\"height:200px;margin-top:24px;background:linear-gradient(135deg,#d9b48f,#8a5a3a)\"></div><p><button style=\"font:inherit;padding:10px 22px;border:2px solid #2a1f17;background:#b4472b;color:#fff\">Order</button></p></main>");
            std::fs::write(site.join(format!("{name}.html")), html).unwrap();
        }
        let browser = coturbo_design::render::find_browser().expect("Edge or Chrome");
        for (n, (name, _, _)) in pages.iter().enumerate() {
            let mut got: Vec<Frame> = Vec::new();
            let actions = vec![Action::Open(format!("{name}.html")), Action::Click(Target::Text("Order".into()))];
            coturbo_browse::run(&browser, &site, &actions, (880, 560), &mut |mut f| {
                f.window = n;
                got.push(f)
            })
            .await
            .expect("browse");
            windows.push(got);
        }
    }
    if scene == 1 {
        let site = std::env::temp_dir().join("codeturbo-vitrine");
        std::fs::create_dir_all(&site).unwrap();
        std::fs::write(site.join("index.html"), SITE).unwrap();
        let browser = coturbo_design::render::find_browser().expect("Edge or Chrome");
        let actions = vec![
            Action::Open("index.html".into()),
            Action::Click(Target::Text("Add to cart".into())),
            Action::Fill(Target::Selector("#mail".into()), "ana@example.com".into()),
            Action::Click(Target::Text("Place order".into())),
        ];
        coturbo_browse::run(&browser, &site, &actions, (880, 560), &mut |f| frames.push(f)).await.expect("browse");
    }

    let mut order = vec![
        stage("WRITE", StageState::Done, "app.js", None),
        stage("CHECK", StageState::Done, "no problems", None),
    ];
    match scene {
        1 => {
            app.apply(Out::Lines(vec![
                line(vec![Span::raw("  I will open the page like a person: add to cart, fill the e-mail and place the order.")]),
                Line::default(),
                tool("browse", "· 4 step(s)"),
                Line::default(),
                line(vec![Span::styled("    1. opened index.html — \"Roast & Co\"", Style::new().fg(Color::Rgb(110, 125, 150)))]),
                line(vec![Span::styled("    2. clicked \"Add to cart\"", Style::new().fg(Color::Rgb(110, 125, 150)))]),
                line(vec![Span::styled("    3. filled #mail with \"ana@example.com\"", Style::new().fg(Color::Rgb(110, 125, 150)))]),
            ]));
            app.apply(Out::Busy(Some("browse · 4 step(s)".into())));
            app.apply(Out::Mood(Mood::Hard));
            order.push(stage("DESIGN", StageState::Done, "approved", None));
            for o in order {
                app.apply(o);
            }
            app.apply(tasks([TaskState::Done, TaskState::Done, TaskState::Done, TaskState::Done, TaskState::Doing]));
            // The frames arrive once the screen runs, as they do in a real session.
            let click = frames.iter().position(|f| f.caption.contains("Place order") && !f.clicked).unwrap_or(frames.len() - 1);
            let (a, b) = (frames[click].clone(), frames[(click + 1).min(frames.len() - 1)].clone());
            let late = out_tx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                let _ = late.send(Out::Browser(a));
                std::thread::sleep(Duration::from_millis(1400));
                let _ = late.send(Out::Browser(b));
            });
        }
        2 => {
            app.apply(Out::Lines(vec![line(vec![Span::raw("  Writing the app logic now: render, toggle, edit and filters.")])]));
            app.apply(Out::Busy(Some("write_file app.js · 2.1k chars".into())));
            app.apply(Out::Mood(Mood::Run));
            app.apply(stage("WRITE", StageState::Running, "app.js", Some(8.0)));
            app.apply(tasks([TaskState::Done, TaskState::Done, TaskState::Doing, TaskState::Pending, TaskState::Pending]));
            let lines = CODE.iter().map(|l| Line::styled(format!("  {l}"), Style::new().fg(Color::Rgb(190, 200, 215)))).collect();
            app.apply(Out::Live(Some(("writing app.js".into(), lines))));
        }
        3 => {
            let dir = std::env::temp_dir().join("codeturbo-vitrine-src");
            let _ = std::fs::create_dir_all(dir.join("src"));
            std::fs::write(
                dir.join("src/lib.rs"),
                "pub fn remaining(items: &[Item]) -> usize {\n    let open = items.iter().filter(|i| !i.done).count();\n    let label: String = open;\n    open\n}\n\npub struct Item {\n    pub done: bool,\n}\n",
            )
            .unwrap();
            let found = [Finding { error: true, path: "src/lib.rs".into(), line: 3, col: 25, message: "mismatched types: expected `String`, found `usize`".into() }];
            app.apply(Out::Lines(render(&found, &dir, 100)));
            app.apply(Out::Busy(Some("think · round 2".into())));
            app.apply(Out::Mood(Mood::Hard));
            app.apply(stage("WRITE", StageState::Done, "src/lib.rs", None));
            app.apply(stage("CHECK", StageState::Failed, "1 error(s)", None));
            app.apply(stage("FIX", StageState::Running, "attempt 1/3", None));
            app.apply(tasks([TaskState::Done, TaskState::Done, TaskState::Done, TaskState::Doing, TaskState::Pending]));
        }
        4 => {
            app.apply(Out::Lines(vec![line(vec![Span::raw("  Code written. Running the final checks.")])]));
            app.apply(Out::Busy(Some("verify: clippy, tests".into())));
            app.apply(Out::Mood(Mood::Turbo));
            app.apply(stage("WRITE", StageState::Done, "src/lib.rs", None));
            app.apply(stage("CHECK", StageState::Done, "no problems", None));
            app.apply(stage("FIX", StageState::Done, "", None));
            app.apply(stage("CLIPPY", StageState::Done, "3.1s", Some(3.0)));
            app.apply(stage("TEST", StageState::Running, "14/22", Some(8.0)));
            app.apply(tasks([TaskState::Done, TaskState::Done, TaskState::Done, TaskState::Doing, TaskState::Pending]));
            std::thread::sleep(Duration::from_millis(2500));
        }
        6 => {
            app.apply(Out::Lines(vec![
                line(vec![Span::raw("  The site has four pages. I will open all of them at the same time and check each one.")]),
                Line::default(),
                tool("browse", "· 4 windows"),
            ]));
            app.apply(Out::Busy(Some("browse · 4 windows".into())));
            app.apply(Out::Mood(Mood::Hard));
            for o in order {
                app.apply(o);
            }
            app.apply(tasks([TaskState::Done, TaskState::Done, TaskState::Done, TaskState::Doing, TaskState::Pending]));
            let late = out_tx.clone();
            let last: Vec<Frame> = windows.iter().filter_map(|w| w.last().cloned()).collect();
            let before: Vec<Frame> = windows.iter().filter_map(|w| w.iter().rev().nth(1).cloned()).collect();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1200));
                for f in before.into_iter().chain(last) {
                    let _ = late.send(Out::Browser(f));
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
        }
        _ => {
            app.apply(Out::Lines(vec![
                line(vec![Span::raw("  Done. The app matches the original on every behavior I tested:")]),
                line(vec![Span::raw("  add, complete, edit by double-click, filters and the item counter.")]),
                Line::default(),
                line(vec![Span::styled("  done in 4 rounds · ", Style::new().fg(Color::Rgb(110, 125, 150))), Span::styled("✓ verified", Style::new().fg(Color::Rgb(86, 204, 128)))]),
            ]));
            app.apply(Out::Busy(Some("verify".into())));
            for (n, st, note) in [("WRITE", StageState::Done, "app.js"), ("CHECK", StageState::Done, "no problems"), ("CLIPPY", StageState::Done, "3.1s"), ("TEST", StageState::Done, "22/22"), ("DESIGN", StageState::Done, "approved")] {
                app.apply(stage(n, st, note, None));
            }
            app.apply(tasks([TaskState::Done; 5]));
            app.apply(Out::Busy(None));
        }
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(90));
        drop(out_tx);
    });
    run(app, out_rx).unwrap();
}
