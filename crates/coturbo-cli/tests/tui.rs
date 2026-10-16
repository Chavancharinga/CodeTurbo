use coturbo_agent::Decision;
use coturbo_agent::plan::PlanChoice;
use coturbo_cli::models::parse_ids;
use coturbo_cli::settings::{self, Credentials};
use coturbo_cli::tui::app::{App, Transcript, filtered};
use coturbo_cli::tui::commands::{self, Command, Parsed};
use coturbo_cli::tui::input::Input;
use coturbo_cli::tui::text::{ansi_line, wrap};
use coturbo_cli::tui::{Choice, In, Out, Question, Status, StreamKind};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;

fn plain(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn commands_parse_in_portuguese_and_english() {
    assert_eq!(commands::parse("/modelo"), Parsed::Command(Command::Model, String::new()));
    assert_eq!(commands::parse("/models"), Parsed::Command(Command::Model, String::new()));
    assert_eq!(commands::parse("  /Modelo qwen2.5:7b "), Parsed::Command(Command::Model, "qwen2.5:7b".into()));
    assert_eq!(commands::parse("/provider"), Parsed::Command(Command::Provider, String::new()));
    assert_eq!(commands::parse("/aprovação yolo"), Parsed::Command(Command::Approval, "yolo".into()));
    assert_eq!(commands::parse("/xyz"), Parsed::Unknown("/xyz".into()));
    assert_eq!(commands::parse("/src/main.rs tem um erro"), Parsed::Task, "a path is a task");
    assert_eq!(commands::parse("crie um site"), Parsed::Task);
    assert_eq!(commands::parse("/"), Parsed::Task);
    let names: Vec<_> = commands::suggestions("/mo").iter().map(|c| c.name).collect();
    assert_eq!(names, ["/model"]);
    assert_eq!(commands::suggestions("/").len(), commands::COMMANDS.len());
    assert!(commands::suggestions("/modelo x").is_empty(), "no list once arguments start");
    assert!(commands::suggestions("texto").is_empty());
}

#[test]
fn ansi_colors_become_styles() {
    let line = ansi_line("\x1b[48;2;70;18;24m\x1b[38;2;255;95;95m   12 -\x1b[38;2;255;205;205m old\x1b[0m end\x1b[K");
    assert_eq!(plain(&line), "   12 - old end");
    assert_eq!(line.spans[0].style.bg, Some(Color::Rgb(70, 18, 24)));
    assert_eq!(line.spans[0].style.fg, Some(Color::Rgb(255, 95, 95)));
    assert_eq!(line.spans[1].style.fg, Some(Color::Rgb(255, 205, 205)));
    assert_eq!(line.spans[2].style.bg, None, "reset");
    let dim = ansi_line("\x1b[2;3m∴ hmm\x1b[0m");
    assert!(dim.spans[0].style.add_modifier.contains(Modifier::DIM | Modifier::ITALIC));
    assert_eq!(ansi_line("\x1b[32m+3\x1b[0m").spans[0].style.fg, Some(Color::Green));
}

#[test]
fn wrapping_breaks_at_spaces_and_keeps_styles() {
    let line = ansi_line("\x1b[31mvermelho\x1b[0m e mais algumas palavras");
    let rows = wrap(&line, 12);
    let text: Vec<String> = rows.iter().map(plain).collect();
    assert_eq!(text, ["vermelho e ", "mais ", "algumas ", "palavras"]);
    assert_eq!(rows[0].spans[0].style.fg, Some(Color::Red));
    // A word longer than the row is cut.
    let long: Vec<String> = wrap(&Line::raw("abcdefghij"), 4).iter().map(plain).collect();
    assert_eq!(long, ["abcd", "efgh", "ij"]);
    // Wide characters count as two cells.
    let wide: Vec<String> = wrap(&Line::raw("日本語です"), 4).iter().map(plain).collect();
    assert_eq!(wide, ["日本", "語で", "す"]);
    assert_eq!(wrap(&Line::default(), 10).len(), 1, "an empty line keeps its row");
}

#[test]
fn input_edits_and_lays_out_with_the_cursor() {
    let mut i = Input::default();
    for c in "olá mundo".chars() {
        i.insert(c);
    }
    i.word_left();
    i.insert_str("grande ");
    assert_eq!(i.text(), "olá grande mundo");
    i.end();
    i.delete_word();
    assert_eq!(i.text(), "olá grande ");
    i.backspace();
    i.home();
    i.delete();
    assert_eq!(i.text(), "lá grande");
    i.set("linha um\nlinha dois longa");
    assert!(i.up(), "moves to the first line");
    assert!(!i.up(), "already on the first line");
    assert!(i.down());
    assert!(!i.down());
    i.end();
    i.insert_str("\r\ncolado");
    assert_eq!(i.text(), "linha um\nlinha dois longa\ncolado");

    let mut i = Input::default();
    i.set("abcdef");
    let (rows, cursor) = i.layout(4, false);
    assert_eq!(rows, ["abcd", "ef"]);
    assert_eq!(cursor, (1, 2));
    i.set("abcd");
    assert_eq!(i.layout(4, false), (vec!["abcd".to_string(), String::new()], (1, 0)), "cursor wraps past a full row");
    i.set("segredo");
    assert_eq!(i.layout(20, true).0, ["•••••••"]);
    assert_eq!(i.take(), "segredo");
    assert!(i.is_empty());
}

#[test]
fn streamed_text_and_scrolling() {
    let mut t = Transcript::default();
    t.push(Line::raw("início"));
    t.stream(StreamKind::Thinking, "pensando\nmais");
    t.stream(StreamKind::Text, "resposta ");
    t.stream(StreamKind::Text, "final");
    let lines: Vec<String> = t.lines().iter().map(plain).collect();
    assert_eq!(lines, ["início", "", "∴ pensando", "  mais", "", "resposta final"]);
    assert!(t.lines()[2].spans[0].style.add_modifier.contains(Modifier::ITALIC));

    let mut t = Transcript::default();
    for n in 0..10 {
        t.push(Line::raw(format!("linha {n}")));
    }
    let (rows, scroll, _) = t.view(40, 3, 0);
    assert_eq!(rows.iter().map(plain).collect::<Vec<_>>(), ["linha 7", "linha 8", "linha 9"]);
    assert_eq!(scroll, 0);
    let (rows, scroll, _) = t.view(40, 3, 2);
    assert_eq!(rows.iter().map(plain).collect::<Vec<_>>(), ["linha 5", "linha 6", "linha 7"]);
    assert_eq!(scroll, 2);
    let (rows, scroll, _) = t.view(40, 3, 99);
    assert_eq!(plain(&rows[0]), "linha 0", "clamped at the top");
    assert_eq!(scroll, 7);
}

#[test]
fn settings_are_saved_without_losing_the_rest_of_the_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "# minha configuração\nprovider = \"anthropic\"\nbase_url = \"http://x\"\n\n[approval]\nbash = \"prompt\"\n",
    )
    .unwrap();
    settings::save_config(dir.path(), &[("provider", Some("ollama")), ("model", Some("qwen3:4b")), ("base_url", None)])
        .unwrap();
    let text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(text.contains("# minha configuração"), "{text}");
    let (cfg, _) = coturbo_cli::config::load(Some(dir.path())).unwrap();
    assert_eq!((cfg.provider.as_str(), cfg.model.as_str(), cfg.base_url), ("ollama", "qwen3:4b", None));
    assert_eq!(cfg.approval.get("bash").map(String::as_str), Some("prompt"));

    // A new directory is created on first save.
    let fresh = dir.path().join("novo");
    settings::save_config(&fresh, &[("model", Some("m"))]).unwrap();
    assert_eq!(coturbo_cli::config::load(Some(&fresh)).unwrap().0.model, "m");
}

#[test]
fn keys_are_stored_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(Credentials::load(Some(dir.path())), Credentials::default());
    let mut c = Credentials::default();
    c.set("ollama", "  local-key \n");
    c.set("openrouter", "or-key");
    c.save(dir.path()).unwrap();
    let back = Credentials::load(Some(dir.path()));
    assert_eq!(back.get("ollama"), Some("local-key"));
    assert_eq!(back.get("openrouter"), Some("or-key"));
    // No environment variable for Ollama: the stored key is used.
    assert_eq!(settings::api_key("ollama", &back).as_deref(), Some("local-key"));
    assert_eq!(settings::api_key("ollama", &Credentials::default()), None);
    assert!(settings::provider("anthropic").unwrap().key_var.is_some());
    assert!(settings::provider("ollama").unwrap().key_var.is_none());
}

#[test]
fn model_lists_of_each_provider() {
    let anthropic = json!({"data": [{"id": "claude-opus-5-5"}, {"id": "claude-sonnet-5-5"}]});
    assert_eq!(parse_ids("anthropic", &anthropic), ["claude-opus-5-5", "claude-sonnet-5-5"], "API order kept");
    let openai = json!({"data": [{"id": "gpt-5"}, {"id": "text-embedding-3-small"}, {"id": "gpt-4.1"}, {"id": "whisper-1"}]});
    assert_eq!(parse_ids("openai", &openai), ["gpt-4.1", "gpt-5"], "chat models only, sorted");
    let ollama = json!({"models": [{"name": "qwen3:4b"}, {"name": "qwen2.5:7b"}]});
    assert_eq!(parse_ids("ollama", &ollama), ["qwen2.5:7b", "qwen3:4b"]);
    assert!(parse_ids("openrouter", &json!({"error": "x"})).is_empty());
    let or = json!({"data": [{"id": "x/free:free", "name": "X Free", "context_length": 128000}, {"id": "a/b", "name": "A B"}]});
    let named = coturbo_cli::models::parse_named("openrouter", &or);
    assert_eq!(named[0], ("a/b".into(), "a/b · A B".into()));
    assert_eq!(named[1].1, "x/free:free · X Free · 128k context · free");
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

struct Harness {
    app: App,
    rx: UnboundedReceiver<In>,
    cancel: UnboundedReceiver<()>,
}

fn harness() -> Harness {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let (ctx, cancel) = tokio::sync::mpsc::unbounded_channel();
    let status = Status {
        provider: "anthropic".into(),
        model: "claude-sonnet-5-5".into(),
        approval: "auto-edit".into(),
        ..Status::default()
    };
    let mut app = App::new(status, tx, ctx, Arc::new(AtomicBool::new(false)));
    app.skip_intro();
    Harness { app, rx, cancel }
}

impl Harness {
    fn typed(&mut self, s: &str) {
        for c in s.chars() {
            self.press(KeyCode::Char(c));
        }
    }

    fn press(&mut self, code: KeyCode) {
        self.app.on_key(key(code, KeyModifiers::NONE), false);
    }

    fn ctrl(&mut self, c: char) {
        self.app.on_key(key(KeyCode::Char(c), KeyModifiers::CONTROL), false);
    }
}

#[test]
fn typing_and_sending_a_task() {
    let mut h = harness();
    h.typed("crie um site");
    h.app.on_key(key(KeyCode::Enter, KeyModifiers::SHIFT), false);
    h.typed("com css");
    // An Enter inside a paste burst is a line break, not a send.
    h.app.on_key(key(KeyCode::Enter, KeyModifiers::NONE), true);
    h.typed("e js");
    assert_eq!(h.app.input().text(), "crie um site\ncom css\ne js");
    h.press(KeyCode::Enter);
    assert_eq!(h.rx.try_recv().unwrap(), In::Submit("crie um site\ncom css\ne js".into()));
    assert!(h.app.input().is_empty());
    let shown: Vec<String> = h.app.transcript().lines().iter().map(plain).collect();
    assert!(shown.contains(&"› crie um site".to_string()), "{shown:?}");
    // History brings it back.
    h.app.on_key(key(KeyCode::Up, KeyModifiers::CONTROL), false);
    assert_eq!(h.app.input().text(), "crie um site\ncom css\ne js");
    h.app.on_key(key(KeyCode::Down, KeyModifiers::CONTROL), false);
    assert!(h.app.input().is_empty());
    // AltGr characters (Ctrl+Alt on Windows) are text.
    h.app.on_key(key(KeyCode::Char('@'), KeyModifiers::CONTROL | KeyModifiers::ALT), false);
    assert_eq!(h.app.input().text(), "@");
    // Key releases (Windows) are ignored.
    let mut release = key(KeyCode::Char('x'), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    h.app.on_event(Event::Key(release), false);
    assert_eq!(h.app.input().text(), "@");
}

#[test]
fn a_partly_typed_command_runs_the_suggestion() {
    let mut h = harness();
    h.typed("/mod");
    h.press(KeyCode::Enter);
    assert_eq!(h.rx.try_recv().unwrap(), In::Submit("/model".into()));
    h.typed("/pro");
    h.press(KeyCode::Tab);
    assert_eq!(h.app.input().text(), "/provider ");
    h.ctrl('c');
    assert!(h.app.input().is_empty(), "Ctrl+C clears the box first");
    h.typed("/sair");
    h.press(KeyCode::Enter);
    assert_eq!(h.rx.try_recv().unwrap(), In::Quit);
    assert!(h.app.closing());
}

#[test]
fn esc_and_ctrl_c_interrupt_a_running_task() {
    let mut h = harness();
    h.app.apply(Out::Busy(Some("pensando".into())));
    h.press(KeyCode::Esc);
    assert!(h.cancel.try_recv().is_ok());
    h.ctrl('c');
    assert!(h.cancel.try_recv().is_ok());
    assert!(!h.app.closing(), "Ctrl+C while busy does not quit");
    h.typed("próxima");
    h.press(KeyCode::Enter);
    assert_eq!(h.rx.try_recv().unwrap(), In::Submit("próxima".into()));
    let shown: Vec<String> = h.app.transcript().lines().iter().map(plain).collect();
    assert!(shown.iter().any(|l| l.contains("queued")), "{shown:?}");
    h.app.apply(Out::Busy(None));
    h.ctrl('c');
    assert_eq!(h.rx.try_recv().unwrap(), In::Quit, "idle with an empty box: quit");
}

#[test]
fn approval_with_a_deny_reason() {
    let mut h = harness();
    h.typed("rascunho");
    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Approve { title: "Permitir bash rm -rf x?".into(), reply: tx }));
    assert!(h.app.input().is_empty(), "the draft is put aside");
    h.press(KeyCode::Char('3'));
    assert!(rx.try_recv().is_err(), "choosing No asks for a reason first");
    h.typed("use git clean");
    h.press(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap(), Decision::Deny(Some("use git clean".into())));
    assert!(!h.app.is_asking());
    assert_eq!(h.app.input().text(), "rascunho", "the draft comes back");

    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Approve { title: "Permitir?".into(), reply: tx }));
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap(), Decision::Session);

    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Approve { title: "Permitir?".into(), reply: tx }));
    h.press(KeyCode::Esc);
    assert_eq!(rx.try_recv().unwrap(), Decision::Deny(None));
}

#[test]
fn plan_review_skips_the_disabled_option() {
    let mut h = harness();
    let (tx, mut rx) = oneshot::channel();
    let path = std::path::PathBuf::from("plano.md");
    h.app.apply(Out::Ask(Question::Plan { title: "Plano".into(), path: path.clone(), keep_allowed: false, reply: tx }));
    h.press(KeyCode::Char('3'));
    assert!(rx.try_recv().is_err(), "keep context is unavailable");
    h.press(KeyCode::Down);
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    assert!(h.app.is_asking(), "the third Down lands on Refine, which asks what to change");
    h.typed("mais testes");
    h.press(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap(), PlanChoice::Refine("mais testes".into()));

    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Plan { title: "Plano".into(), path: path.clone(), keep_allowed: true, reply: tx }));
    h.press(KeyCode::Char('e'));
    assert_eq!(h.app.take_edit(), Some(path), "e opens the plan in the editor");
    h.press(KeyCode::Char('1'));
    assert_eq!(rx.try_recv().unwrap(), PlanChoice::Execute);
}

#[test]
fn picking_from_a_filtered_list_or_typing_a_new_entry() {
    let options = vec![Choice::new("qwen2.5:7b", "qwen2.5:7b (atual)"), Choice::new("qwen3:4b", "qwen3:4b"), Choice::new("llama3", "llama3")];
    assert_eq!(filtered(&options, "QWEN").len(), 2);
    assert_eq!(filtered(&options, "").len(), 3);

    let mut h = harness();
    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Select { title: "Modelo".into(), options: options.clone(), custom: true, reply: tx }));
    h.typed("qwen");
    h.press(KeyCode::Down);
    h.press(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap(), Some("qwen3:4b".into()));

    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Select { title: "Modelo".into(), options: options.clone(), custom: true, reply: tx }));
    h.typed("deepseek-r1:8b");
    h.press(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap(), Some("deepseek-r1:8b".into()), "no match: the typed name is used");

    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Select { title: "Provedor".into(), options, custom: false, reply: tx }));
    h.typed("nada");
    h.press(KeyCode::Enter);
    assert!(h.app.is_asking(), "without custom entries, nothing to pick keeps the list open");
    h.press(KeyCode::Esc);
    assert_eq!(rx.try_recv().unwrap(), None);
}

#[test]
fn secret_entry_is_masked_and_trimmed() {
    let mut h = harness();
    let (tx, mut rx) = oneshot::channel();
    h.app.apply(Out::Ask(Question::Secret { title: "Chave".into(), reply: tx }));
    h.press(KeyCode::Enter);
    assert!(h.app.is_asking(), "an empty key is not accepted");
    h.app.on_event(Event::Paste("  sk-ant-123\n".into()), false);
    h.press(KeyCode::Enter);
    assert_eq!(rx.try_recv().unwrap(), Some("sk-ant-123".into()));
}

#[test]
fn the_screen_draws_logo_conversation_box_and_status() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut h = harness();
    h.app.apply(Out::Lines(vec![Line::raw("olá do CodeTurbo")]));
    h.app.apply(Out::Usage("tokens ↑1.0k ⟳0 ↓200 · $0.0100".into()));
    h.app.apply(Out::Context(12));
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| h.app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let rows: Vec<String> =
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect();
    let all = rows.join("\n");
    assert!(all.contains("olá do CodeTurbo"), "{all}");
    assert!(all.contains("Describe the task"), "placeholder in the empty box");
    assert!(rows.last().unwrap().contains("anthropic · claude-sonnet-5-5 · auto-edit"), "{}", rows.last().unwrap());
    assert!(rows.last().unwrap().contains("context 12%"));
    // The logo is drawn in half blocks, in the brand colours: the idle snail is white, the name blue and white.
    let (blue, white) = (Color::Rgb(47, 123, 255), Color::Rgb(225, 232, 245));
    let logo = (0..8).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| {
        let c = &buf[(x, y)];
        (c.symbol() == "▀" || c.symbol() == "▄") && [blue, white].iter().any(|&k| c.fg == k || c.bg == k)
    });
    assert!(logo.count() > 20, "logo pixels on screen");

    // Typing `/` lists the commands above the box.
    h.typed("/");
    term.draw(|f| h.app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let all: String = (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n")
        .collect();
    assert!(all.contains("/provider") && all.contains("choose the model"), "{all}");

    // A narrow terminal falls back to the plain name.
    h.ctrl('c');
    let mut small = Terminal::new(TestBackend::new(40, 12)).unwrap();
    small.draw(|f| h.app.draw(f)).unwrap();
    let top: String = (0..40).map(|x| small.backend().buffer()[(x, 0)].symbol().to_string()).collect();
    assert!(top.contains("CodeTurbo"), "{top}");
}

#[test]
fn model_markdown_is_rendered_without_markers() {
    use coturbo_cli::tui::text::markdown_line;
    use ratatui::style::Style;
    let base = Style::new();
    assert_eq!(plain(&markdown_line("## Plano de ação", false, base)), "Plano de ação");
    assert!(markdown_line("# Título", false, base).spans[0].style.add_modifier.contains(Modifier::BOLD));
    let l = markdown_line("Use **negrito**, *itálico* e `index.html`.", false, base);
    assert_eq!(plain(&l), "Use negrito, itálico e index.html.");
    assert!(l.spans.iter().any(|s| s.content == "negrito" && s.style.add_modifier.contains(Modifier::BOLD)));
    assert!(l.spans.iter().any(|s| s.content == "itálico" && s.style.add_modifier.contains(Modifier::ITALIC)));
    assert_eq!(plain(&markdown_line("- item", false, base)), "• item");
    assert_eq!(plain(&markdown_line("  * sub", false, base)), "  • sub");
    assert_eq!(plain(&markdown_line("snake_case_name e 2 * 3", false, base)), "snake_case_name e 2 * 3");
    assert_eq!(plain(&markdown_line("**x** = *y*", true, base)), "  **x** = *y*", "code is shown as is");

    let mut t = Transcript::default();
    t.stream(StreamKind::Text, "## Feito\nVeja:\n```html\n<p>*oi*</p>\n```\nfim **ok**");
    let shown: Vec<String> = t.lines().iter().map(plain).collect();
    assert_eq!(shown, ["Feito", "Veja:", "", "  <p>*oi*</p>", "", "fim ok"]);
}

#[test]
fn long_code_is_folded_until_ctrl_b() {
    use coturbo_cli::tui::host::{PREVIEW_ROWS, diff_fold};
    let long: String = (0..40).map(|i| format!("+linha {i}\n")).collect();
    let Out::Fold { preview, full } = diff_fold(&format!("@@ -0,0 +1,40 @@\n{long}"), 80) else { panic!("folded") };
    assert_eq!(preview.len(), PREVIEW_ROWS + 1);
    assert!(plain(preview.last().unwrap()).contains("30 more lines · click or Ctrl+B"));
    assert_eq!(full.len(), 41, "every line, nothing capped");
    let short = diff_fold("@@ -3,1 +3,1 @@\n-velho\n+novo\n", 80);
    assert!(matches!(short, Out::Clipped(ref l) if l.len() == 2), "a small fix is shown whole");

    let mut h = harness();
    h.app.apply(Out::Fold { preview: preview.clone(), full: full.clone() });
    assert_eq!(h.app.transcript().lines().len(), PREVIEW_ROWS + 1);
    h.ctrl('b');
    assert_eq!(h.app.transcript().lines().len(), 41);
    h.ctrl('b');
    assert_eq!(h.app.transcript().lines().len(), PREVIEW_ROWS + 1);
}

#[test]
fn task_time_reads_naturally() {
    use coturbo_cli::status::elapsed;
    use std::time::Duration;
    assert_eq!(elapsed(Duration::from_secs(42)), "42s");
    assert_eq!(elapsed(Duration::from_secs(125)), "2min 05s");
    assert_eq!(elapsed(Duration::from_secs(3780)), "1h 03min");
}

#[test]
fn clicking_code_opens_it_and_the_wheel_scrolls() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut h = harness();
    let full: Vec<Line<'static>> = (0..30).map(|i| Line::raw(format!("linha {i}"))).collect();
    let mut preview = full[..10].to_vec();
    preview.push(Line::raw("… mais 20"));
    h.app.apply(Out::Fold { preview, full });
    let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
    term.draw(|f| h.app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let row = (0..40).find(|&y| (0..100).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().contains("linha 3")).unwrap();
    let click = |row| Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row, modifiers: KeyModifiers::NONE });
    h.app.on_event(click(row), false);
    assert_eq!(h.app.transcript().lines().len(), 30, "clicked code opens");
    term.draw(|f| h.app.draw(f)).unwrap();
    h.app.on_event(Event::Mouse(MouseEvent { kind: MouseEventKind::ScrollUp, column: 5, row: 10, modifiers: KeyModifiers::NONE }), false);
    term.draw(|f| h.app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let all: String = (0..40).map(|y| (0..100).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect();
    assert!(!all.contains("linha 29"), "the wheel scrolled up");
}

#[test]
fn folders_are_trusted_once_and_recognized_after() {
    use coturbo_cli::settings::{Trust, known_project, too_broad};
    let config = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let inner = project.path().join("src");
    std::fs::create_dir_all(&inner).unwrap();
    let mut t = Trust::load(Some(config.path()));
    assert!(!t.covers(project.path()), "a new folder asks first");
    t.add(Some(config.path()), project.path()).unwrap();
    let again = Trust::load(Some(config.path()));
    assert!(again.covers(project.path()), "remembered after restart");
    assert!(again.covers(&inner), "folders inside a trusted one are trusted");
    assert!(!again.covers(config.path()), "others are not");
    assert!(!known_project(project.path()));
    std::fs::create_dir_all(project.path().join(".codeturbo")).unwrap();
    assert!(known_project(project.path()), "CodeTurbo's own files mark a known project");
    assert!(!too_broad(&inner));
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        assert!(too_broad(std::path::Path::new(&home)), "the home folder is too broad");
    }
}

fn screen_rows(h: &mut Harness, w: u16, hgt: u16) -> Vec<String> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut term = Terminal::new(TestBackend::new(w, hgt)).unwrap();
    term.draw(|f| h.app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect()
}

#[test]
fn the_footer_shows_the_folder_next_to_the_model() {
    let mut h = harness();
    h.app.apply(Out::Status(Status {
        provider: "claude-code".into(),
        model: "haiku".into(),
        approval: "auto-edit".into(),
        folder: r"C:\Users\danie\Music\CoTurbon".into(),
        ..Status::default()
    }));
    let rows = screen_rows(&mut h, 110, 30);
    let last = rows.last().unwrap();
    assert!(last.contains(r"claude-code · haiku · auto-edit · C:\Users\danie\Music\CoTurbon"), "{last}");
}

#[test]
fn a_long_folder_is_cut_to_its_last_three_folders() {
    let mut h = harness();
    h.app.apply(Out::Status(Status {
        provider: "claude-code".into(),
        model: "haiku".into(),
        approval: "auto-edit".into(),
        usage: "tokens ↑0 ⟳0 ↓0".into(),
        folder: r"C:\Users\danie\Music\CoTurbon\sandbox\paint\src".into(),
        ..Status::default()
    }));
    // Not enough room for the whole path: only the last three folders show.
    let rows = screen_rows(&mut h, 90, 30);
    let last = rows.last().unwrap();
    assert!(last.contains(r"…\sandbox\paint\src"), "{last}");
    assert!(!last.contains("Users"), "{last}");
    assert!(last.contains("tokens ↑0"), "the usage stays at the right: {last}");
}

#[test]
fn the_snail_goes_white_then_blue_then_green() {
    use coturbo_cli::splash::Mood;
    let mut h = harness();
    assert_eq!(h.app.mood(), Mood::Calm, "white before any prompt");
    h.app.apply(Out::Busy(None));
    assert_eq!(h.app.mood(), Mood::Calm, "nothing happened, so still white");
    // A task begins: blue, walking.
    h.app.apply(Out::Busy(Some("think".into())));
    assert_eq!(h.app.mood(), Mood::Start);
    // The work then sets the pace.
    h.app.apply(Out::Mood(Mood::Run));
    assert_eq!(h.app.mood(), Mood::Run);
    h.app.apply(Out::Mood(Mood::Hard));
    assert_eq!(h.app.mood(), Mood::Hard);
    h.app.apply(Out::Mood(Mood::Turbo));
    assert_eq!(h.app.mood(), Mood::Turbo);
    // Waiting on the user in the middle of the task does not restart it.
    h.app.apply(Out::Busy(Some("wait: your answer".into())));
    assert_eq!(h.app.mood(), Mood::Turbo);
    // Everything done: green, and it stays green until the next task.
    h.app.apply(Out::Busy(None));
    assert_eq!(h.app.mood(), Mood::Done);
    h.app.apply(Out::Busy(None));
    assert_eq!(h.app.mood(), Mood::Done);
    // The next task starts blue again.
    h.app.apply(Out::Busy(Some("think".into())));
    assert_eq!(h.app.mood(), Mood::Start);
}

#[test]
fn alert_and_denied_flash_over_the_mood_and_outlive_the_task() {
    use coturbo_cli::splash::Mood;
    let mut h = harness();
    h.app.apply(Out::Busy(Some("think".into())));
    h.app.apply(Out::Mood(Mood::Turbo));
    h.app.apply(Out::Mood(Mood::Denied));
    assert_eq!(h.app.mood(), Mood::Denied);
    h.app.apply(Out::Mood(Mood::Alert));
    assert_eq!(h.app.mood(), Mood::Alert);
    h.app.apply(Out::Busy(None));
    assert_eq!(h.app.mood(), Mood::Alert, "the flash outlives the end of the task for a moment");
}

#[test]
fn the_snail_is_yellow_while_a_question_waits() {
    use coturbo_cli::splash::Mood;
    let mut h = harness();
    h.app.apply(Out::Mood(Mood::Turbo));
    let (reply, _rx) = tokio::sync::oneshot::channel();
    h.app.apply(Out::Ask(Question::Confirm { title: "Confiar nesta pasta?".into(), reply }));
    assert_eq!(h.app.mood(), Mood::Alert);
    h.app.apply(Out::CloseQuestion);
    assert_eq!(h.app.mood(), Mood::Turbo, "the working mood returns once answered");
}

#[test]
fn the_snail_changes_color_on_screen() {
    use coturbo_cli::splash::Mood;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    // Colors drawn where the snail is: the right of the wordmark, which is itself white and blue.
    let snail = |mood: Mood| {
        let mut h = harness();
        h.app.apply(Out::Mood(mood));
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| h.app.draw(f)).unwrap();
        let buf = term.backend().buffer().clone();
        (0..8).flat_map(|y| (62..100).map(move |x| (x, y))).map(|(x, y)| (buf[(x, y)].fg, buf[(x, y)].bg)).collect::<Vec<_>>()
    };
    let has = |cs: &[(Color, Color)], c: Color| cs.iter().any(|&(f, b)| f == c || b == c);
    let (white, blue, green) = (Color::Rgb(225, 232, 245), Color::Rgb(47, 123, 255), Color::Rgb(30, 150, 78));
    let (yellow, red) = (Color::Rgb(255, 196, 38), Color::Rgb(240, 70, 60));
    let calm = snail(Mood::Calm);
    assert!(has(&calm, white) && !has(&calm, blue) && !has(&calm, green), "white before anything");
    let start = snail(Mood::Start);
    assert!(has(&start, blue) && !has(&start, white), "blue when a task starts");
    let done = snail(Mood::Done);
    assert!(has(&done, green) && !has(&done, blue) && !has(&done, white), "green when it is done");
    assert!(has(&snail(Mood::Alert), yellow));
    assert!(has(&snail(Mood::Denied), red));
    assert!(has(&snail(Mood::Run), blue) && has(&snail(Mood::Turbo), blue));
}

#[test]
fn state_labels_use_the_tool_names_in_english() {
    use coturbo_cli::tui::host::{action_label, busy_label, design_refused};
    use serde_json::json;
    assert_eq!(busy_label("thinking"), "think");
    assert_eq!(busy_label("verifying"), "verify: clippy, tests");
    assert_eq!(action_label("read_file", &json!({"path": "src/main.rs"})), "read_file src/main.rs");
    assert_eq!(action_label("write_file", &json!({"path": "a.html"})), "write_file a.html");
    assert_eq!(action_label("edit_file", &json!({"path": "b.rs"})), "edit_file b.rs");
    assert_eq!(action_label("grep", &json!({"pattern": "fn main"})), "grep \"fn main\"");
    assert_eq!(action_label("bash", &json!({"command": "cargo build"})), "bash cargo build");
    assert_eq!(action_label("investigate", &json!({"target": "steam.exe"})), "investigate steam.exe");
    assert_eq!(action_label("observe_app", &json!({"target": "mspaint.exe"})), "observe_app mspaint.exe");
    assert_eq!(action_label("list_files", &json!({})), "list_files");
    assert!(design_refused("design index.html: 2 blocking, 1 warning(s)\n- P0 ..."));
    assert!(!design_refused("design index.html: 0 blocking, 3 warning(s)"));
    assert!(!design_refused("check src/lib.rs: clean"));
}

#[test]
fn the_debugger_reads_compiler_and_test_lines() {
    use coturbo_cli::tui::debugger::{Finding, parse};
    let text = [
        "✗ verification failed at `clippy` (`cargo clippy`): 2 error(s), 0 warning(s).",
        "error[clippy::nonminimal_bool] src/app.rs:704:12: this boolean expression can be simplified",
        "    if !x {",
        r"- error[E0308] C:\proj\src\main.rs:38:9: mismatched types",
        "warning unused_variables src/lib.rs:7:5: unused variable: `y`",
        "error[clippy::x] src/app.rs:704:20: the same line again",
        "---- tests::soma stdout ----",
        "thread 'tests::soma' (4) panicked at tests/t.rs:12:5:",
        "assertion `left == right` failed",
    ]
    .join("\n");
    let f = parse(&text);
    assert_eq!(f[0], Finding { error: true, path: "src/app.rs".into(), line: 704, col: 12, message: "this boolean expression can be simplified".into() });
    assert_eq!((f[1].path.as_str(), f[1].line, f[1].col), (r"C:\proj\src\main.rs", 38, 9), "a drive letter does not confuse the path");
    assert!(!f[2].error && f[2].line == 7, "warnings are told apart");
    assert_eq!(f.iter().filter(|x| x.path == "src/app.rs" && x.line == 704).count(), 1, "one entry per line");
    let panic = f.last().unwrap();
    assert_eq!((panic.path.as_str(), panic.line, panic.col), ("tests/t.rs", 12, 5));
    assert!(panic.message.starts_with("panicked: assertion"), "{}", panic.message);
    assert!(parse("tudo certo, nada a ver aqui").is_empty());
}

#[test]
fn the_debugger_shows_the_exact_line_with_an_arrow_under_the_column() {
    use coturbo_cli::tui::debugger::{Finding, render};
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.rs"), "fn main() {\n    let a = 1;\n    let y = foo(;\n    println!(\"{a}\");\n}\n").unwrap();
    let found = [Finding { error: true, path: "src/a.rs".into(), line: 3, col: 17, message: "expected expression".into() }];
    let lines = render(&found, dir.path(), 80);
    let rows: Vec<String> = lines.iter().map(plain).collect();
    let all = rows.join("\n");
    assert!(all.contains("DEBUGGER") && all.contains("errors on lines 3"), "{all}");
    assert!(all.contains("src/a.rs:3:17"), "{all}");
    let marked = rows.iter().position(|r| r.contains('▶') && r.contains("let y = foo(;")).expect("the faulty line is marked");
    // The arrow sits under column 17, right on the `;`, and carries the message.
    let (code, arrow) = (&rows[marked], &rows[marked + 1]);
    let col_of = |s: &str, needle: char| s.chars().position(|c| c == needle);
    assert_eq!(col_of(arrow, '^'), col_of(code, ';'), "{code}\n{arrow}");
    assert!(arrow.contains("expected expression"));
    // Context: the line before and after, with their numbers.
    assert!(all.contains("2 │") && all.contains("4 │"), "{all}");
    // The faulty line is the only one with a red background.
    let red = lines.iter().filter(|l| l.spans.iter().any(|s| s.style.bg == Some(Color::Rgb(74, 18, 22)))).count();
    assert_eq!(red, 1);
    // A file that is not there still shows the message.
    let gone = render(&[Finding { error: true, path: "nope.rs".into(), line: 9, col: 1, message: "algo".into() }], dir.path(), 80);
    assert!(gone.iter().map(plain).any(|r| r.contains("algo")));
    assert!(render(&[], dir.path(), 80).is_empty());
}

#[test]
fn stages_fill_while_they_run_and_turn_green_or_red() {
    use coturbo_cli::tui::work::Work;
    use coturbo_cli::tui::{StageState, StageUpdate};
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let mut w = Work::default();
    assert!(!w.visible(true, t0), "nothing to show yet");
    let mut up = StageUpdate::new("CLIPPY", StageState::Running, "1/2");
    up.expected = Some(10.0);
    w.apply(up, t0);
    let half = w.stages[0].fraction(t0 + Duration::from_secs(5));
    assert!((0.45..0.55).contains(&half), "half of the time it took last time: {half}");
    assert!(w.stages[0].fraction(t0 + Duration::from_secs(60)) <= 0.96, "never full before it ends");
    // With no history it still moves, ever more slowly.
    w.apply(StageUpdate::new("TEST", StageState::Running, ""), t0);
    let (a, b) = (w.stages[1].fraction(t0 + Duration::from_secs(2)), w.stages[1].fraction(t0 + Duration::from_secs(20)));
    assert!(0.1 < a && a < b && b < 0.96, "{a} {b}");
    w.apply(StageUpdate::new("CLIPPY", StageState::Done, "4.2s"), t0 + Duration::from_secs(4));
    assert_eq!(w.stages[0].fraction(t0), 1.0);
    assert_eq!(w.stages[0].took, Some(Duration::from_secs(4)));
    assert_eq!(w.stages[0].note, "4.2s");
    w.apply(StageUpdate::new("TEST", StageState::Failed, "2 erros"), t0 + Duration::from_secs(6));
    let rows: Vec<String> = w.lines(100, t0 + Duration::from_secs(6), 0).iter().map(plain).collect();
    assert!(rows[0].contains('✓') && rows[0].contains("CLIPPY") && rows[0].contains("100%") && rows[0].contains("4.2s"), "{rows:?}");
    assert!(rows[1].contains('✗') && rows[1].contains("TEST") && rows[1].contains("FAILED") && rows[1].contains("2 erros"), "{rows:?}");
    assert_eq!(w.stages.len(), 2, "an update of a known stage does not add a row");
    // The panel stays a while after the work ends, then goes away.
    assert!(w.visible(false, t0 + Duration::from_secs(20)));
    assert!(!w.visible(false, t0 + Duration::from_secs(120)));
    assert!(w.visible(true, t0 + Duration::from_secs(120)), "while busy it is always there");
    w.clear();
    assert!(!w.visible(true, t0) && w.stages.is_empty());
}

#[test]
fn the_failed_bar_is_red_the_done_bar_is_green_and_the_running_bar_glints() {
    use coturbo_cli::tui::work::Work;
    use coturbo_cli::tui::{StageState, StageUpdate};
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let mut w = Work::default();
    w.apply(StageUpdate::new("WRITE", StageState::Done, "a.rs"), t0);
    w.apply(StageUpdate::new("CLIPPY", StageState::Failed, ""), t0);
    let mut run = StageUpdate::new("TEST", StageState::Running, "");
    run.expected = Some(10.0);
    w.apply(run, t0);
    let lines = w.lines(100, t0 + Duration::from_secs(8), 1_000);
    let bar_colors = |i: usize| -> Vec<Color> { lines[i].spans.iter().filter(|s| s.content == "█").filter_map(|s| s.style.fg).collect() };
    assert!(bar_colors(0).iter().all(|&c| c == Color::Rgb(86, 204, 128)), "done is green");
    assert!(bar_colors(1).iter().all(|c| matches!(c, Color::Rgb(r, g, _) if *r > 150 && *g < 100)), "failed is red");
    assert!(bar_colors(2).contains(&Color::Rgb(47, 123, 255)), "running is blue");
    // The glint moves: two instants give two different pictures.
    let at = |ms| w.lines(100, t0 + Duration::from_secs(8), ms)[2].spans.iter().map(|s| format!("{:?}", s.style.fg)).collect::<Vec<_>>();
    assert_ne!(at(0), at(330));
}

#[test]
fn the_checklist_follows_the_model_and_keeps_the_current_step_in_view() {
    use coturbo_cli::tui::host::task_items;
    use coturbo_cli::tui::work::Work;
    use coturbo_cli::tui::{TaskItem, TaskState};
    use serde_json::json;
    use std::time::Instant;
    let items = task_items(&json!({"items": [
        {"title": "criar o projeto", "status": "done"},
        {"title": "escrever os testes", "status": "doing"},
        {"title": "rodar a verificação", "status": "pending"},
        {"title": "", "status": "pending"},
        {"status": "done"}
    ]}));
    assert_eq!(items.len(), 3, "entries without a title are dropped");
    assert_eq!(items[1], TaskItem { title: "escrever os testes".into(), state: TaskState::Doing });
    let mut w = Work::default();
    let now = Instant::now();
    w.set_tasks(items, now);
    let rows: Vec<String> = w.lines(80, now, 0).iter().map(plain).collect();
    assert!(rows[0].contains("TASKS") && rows[0].contains("1/3"), "{rows:?}");
    assert!(rows[1].contains('✓') && rows[1].contains("criar o projeto"));
    assert!(rows[2].contains("escrever os testes") && !rows[2].contains('✓') && !rows[2].contains('○'), "the current one is animated: {}", rows[2]);
    assert!(rows[3].contains('○') && rows[3].contains("rodar a verificação"));
    // Many steps: the window follows the one in progress.
    let many: Vec<TaskItem> = (0..15)
        .map(|i| TaskItem { title: format!("passo {i}"), state: if i < 10 { TaskState::Done } else if i == 10 { TaskState::Doing } else { TaskState::Pending } })
        .collect();
    w.set_tasks(many, now);
    let all = w.lines(80, now, 0).iter().map(plain).collect::<Vec<_>>().join("\n");
    assert!(all.contains("passo 10") && all.contains("10/15") && all.contains("and 8 more"), "{all}");
}

#[test]
fn edit_checks_become_a_stage_result() {
    use coturbo_cli::tui::host::edit_check;
    assert_eq!(edit_check("wrote a.rs\ncheck a.rs: clean"), Some((true, "no problems".into())));
    let (ok, note) = edit_check("wrote a.rs\ncheck a.rs: 2 error(s), 1 warning(s)\nerror a.rs:1:1: x").unwrap();
    assert!(!ok && note == "2 error(s), 1 warning(s)", "{note}");
    assert_eq!(edit_check("wrote a.rs"), None);
}

#[test]
fn the_bars_and_the_checklist_are_drawn_above_the_text_box() {
    use coturbo_cli::tui::{StageState, StageUpdate, TaskItem, TaskState};
    let mut h = harness();
    h.app.apply(Out::Busy(Some("clippy".into())));
    h.app.apply(Out::Stage(StageUpdate::new("WRITE", StageState::Done, "src/main.rs")));
    h.app.apply(Out::Stage(StageUpdate::new("CLIPPY", StageState::Running, "1/2")));
    h.app.apply(Out::Tasks(vec![
        TaskItem { title: "criar o projeto".into(), state: TaskState::Done },
        TaskItem { title: "verificar".into(), state: TaskState::Doing },
    ]));
    let rows = screen_rows(&mut h, 100, 34);
    let at = |needle: &str| rows.iter().position(|r| r.contains(needle)).unwrap_or_else(|| panic!("{needle} not on screen:\n{}", rows.join("\n")));
    let (write, clippy, tasks, input) = (at("WRITE"), at("CLIPPY"), at("TASKS"), at("Describe the task"));
    assert!(write < clippy && clippy < tasks && tasks < input, "bars, then checklist, then the text box");
    assert!(rows[write].contains("100%") && rows[write].contains("src/main.rs"));
    // A new job clears them.
    h.app.apply(Out::ClearWork);
    let joined = screen_rows(&mut h, 100, 34).join("\n");
    assert!(!joined.contains("WRITE") && !joined.contains("TASKS"));
    // Small terminals do without the panel instead of squeezing the text.
    h.app.apply(Out::Stage(StageUpdate::new("WRITE", StageState::Running, "")));
    assert!(!screen_rows(&mut h, 100, 18).join("\n").contains("WRITE"));
}

#[test]
fn the_folder_never_touches_the_usage_in_the_footer() {
    for width in [70u16, 80, 90, 100, 112, 140] {
        let mut h = harness();
        h.app.apply(Out::Status(Status {
            provider: "claude-code".into(),
            model: "sonnet".into(),
            approval: "auto-edit".into(),
            usage: "tokens ↑12.4k ⟳8.1k ↓2.2k".into(),
            folder: r"C:\Users\danie\Music\CoTurbon\sandbox\contador".into(),
            ..Status::default()
        }));
        let last = screen_rows(&mut h, width, 30).pop().unwrap();
        let at = last.find("tokens").expect("usage shown");
        assert!(last[..at].ends_with("  "), "width {width}: at least two spaces before the usage: {last:?}");
    }
}

#[test]
fn green_goes_back_to_white_after_ten_idle_minutes() {
    use coturbo_cli::splash::Mood;
    use coturbo_cli::tui::app::GREEN_FOR;
    use std::time::{Duration, Instant};
    let mut h = harness();
    h.app.apply(Out::Busy(Some("think".into())));
    h.app.apply(Out::Mood(Mood::Hard));
    h.app.apply(Out::Busy(None));
    let now = Instant::now();
    assert_eq!(h.app.mood_at(now), Mood::Done, "green right after the work");
    assert_eq!(h.app.mood_at(now + GREEN_FOR - Duration::from_secs(5)), Mood::Done, "still green just before ten minutes");
    assert_eq!(h.app.mood_at(now + GREEN_FOR + Duration::from_secs(5)), Mood::Calm, "white after ten idle minutes");
    // Green only comes after a task: a session with no task stays white.
    let mut fresh = harness();
    fresh.app.apply(Out::Busy(None));
    assert_eq!(fresh.app.mood_at(Instant::now()), Mood::Calm);
    // Another end-of-work message does not restart the ten minutes.
    h.app.apply(Out::Busy(None));
    assert_eq!(h.app.mood_at(now + GREEN_FOR + Duration::from_secs(5)), Mood::Calm);
    // A new task is blue again, even from white.
    h.app.apply(Out::Busy(Some("think".into())));
    assert_eq!(h.app.mood_at(now + GREEN_FOR + Duration::from_secs(5)), Mood::Start);
}

/// A PNG with a blue header, a white page and a red button, for the mini browser.
fn page_png(w: u32, h: u32) -> Vec<u8> {
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let px = if y < h / 6 {
                [47, 123, 255]
            } else if (x > w * 2 / 3) && (y > h / 2) && (y < h * 2 / 3) {
                [220, 40, 50]
            } else {
                [250, 250, 250]
            };
            data.extend(px);
        }
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&data).unwrap();
    }
    out
}

fn browser_frame(cursor: Option<(f32, f32)>, clicked: bool) -> coturbo_browse::Frame {
    coturbo_browse::Frame {
        png: page_png(400, 260),
        url: "file:///C:/proj/index.html".into(),
        title: "Shop".into(),
        window: 0,
        cursor,
        clicked,
        size: (400, 260),
        caption: "clicando \"Sobre\"".into(),
        labels: vec![
            coturbo_browse::Label { x: 20.0, y: 10.0, w: 260.0, h: 30.0, text: "Casa do Cafe".into(), kind: "h1".into() },
            coturbo_browse::Label { x: 270.0, y: 150.0, w: 110.0, h: 36.0, text: "Fazer pedido".into(), kind: "button".into() },
            coturbo_browse::Label { x: 20.0, y: 200.0, w: 6.0, h: 14.0, text: "ZQJ".into(), kind: "span".into() },
        ],
    }
}

fn draw_browser(v: &coturbo_cli::tui::browser::BrowserView, now: std::time::Instant) -> ratatui::buffer::Buffer {
    let area = ratatui::layout::Rect::new(0, 0, 60, 20);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    v.draw(&mut buf, area, now);
    buf
}

fn find_symbol(buf: &ratatui::buffer::Buffer, sym: &str) -> Vec<(u16, u16)> {
    let a = buf.area;
    (0..a.height).flat_map(|y| (0..a.width).map(move |x| (x, y))).filter(|&(x, y)| buf[(x, y)].symbol() == sym).collect()
}

fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let a = buf.area;
    (0..a.height).map(|y| (0..a.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[test]
fn the_mini_browser_draws_the_page_the_address_and_what_the_agent_does() {
    use coturbo_cli::tui::browser::BrowserView;
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let mut v = BrowserView::default();
    assert!(!v.visible(t0), "nothing to show before the first picture");
    v.apply(browser_frame(None, false), t0);
    assert!(v.visible(t0) && v.visible(t0 + Duration::from_secs(300)), "it stays open: no timer closes it under a person's eyes");
    assert!(!v.visible(t0 + Duration::from_secs(700)), "only a window forgotten for ten minutes goes away");
    let buf = draw_browser(&v, t0 + Duration::from_secs(1));
    let text = buffer_text(&buf);
    assert!(text.contains("index.html") && !text.contains("file://") && !text.contains("C:/proj"), "the address, without the scheme:\n{text}");
    assert!(text.contains("clicando \"Sobre\""), "what the agent is doing:\n{text}");
    // The page is there in half blocks, in its own colours: the blue header on top, white below.
    use ratatui::style::Color;
    let blue = Color::Rgb(47, 123, 255);
    let tops: Vec<Color> = find_symbol(&buf, "▀").iter().map(|&(x, y)| buf[(x, y)].fg).collect();
    assert!(tops.contains(&blue), "the header of the page");
    assert!(tops.iter().any(|c| matches!(c, Color::Rgb(r, g, b) if *r > 240 && *g > 240 && *b > 240)), "the white page");
    assert!(tops.iter().any(|c| matches!(c, Color::Rgb(r, g, b) if *r > 200 && *g < 60 && *b < 70)), "the red button");
    assert!(find_symbol(&buf, "◤").is_empty(), "no pointer before the agent moves it");
    // It keeps the page's shape: 58 columns of a 400x260 page are 19 rows of half blocks, plus the bar, the caption and the frame.
    assert_eq!(v.rows_for(60), 23);
    assert!(v.rows_for(40) < v.rows_for(60), "a narrower window is shorter");
}

#[test]
fn the_pointer_walks_to_its_target_and_the_click_ripples() {
    use coturbo_cli::tui::browser::BrowserView;
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    let mut v = BrowserView::default();
    v.apply(browser_frame(Some((60.0, 40.0)), false), t0);
    assert_eq!(v.pointer(t0 + ms(2000)), Some((60.0, 40.0)));
    // A new target: the pointer sets off from where it is.
    let t1 = t0 + ms(3000);
    v.apply(browser_frame(Some((340.0, 200.0)), false), t1);
    assert_eq!(v.pointer(t1), Some((60.0, 40.0)), "starts where it was");
    let mid = v.pointer(t1 + ms(190)).unwrap();
    assert!(mid.0 > 150.0 && mid.0 < 250.0 && mid.1 > 90.0 && mid.1 < 150.0, "half way at half the time: {mid:?}");
    assert_eq!(v.pointer(t1 + ms(1000)), Some((340.0, 200.0)), "arrives and stays");
    // The pointer is drawn, once, and moves across the screen as time passes.
    let at = |t| find_symbol(&draw_browser(&v, t), "◤");
    let (start, mid_p, end) = (at(t1), at(t1 + ms(190)), at(t1 + ms(900)));
    assert_eq!((start.len(), mid_p.len(), end.len()), (1, 1, 1));
    assert!(start[0].0 < mid_p[0].0 && mid_p[0].0 < end[0].0, "{start:?} {mid_p:?} {end:?}");
    // The click: a filled dot, then rings, then gone.
    let t2 = t1 + ms(2000);
    v.apply(browser_frame(Some((340.0, 200.0)), true), t2);
    let press = draw_browser(&v, t2 + ms(80));
    assert_eq!(find_symbol(&press, "◉").len(), 1, "the press");
    assert!(find_symbol(&press, "◤").is_empty());
    let ring = draw_browser(&v, t2 + ms(400));
    assert!(!find_symbol(&ring, "(").is_empty() && !find_symbol(&ring, ")").is_empty(), "the ripple opens");
    let after = draw_browser(&v, t2 + ms(1200));
    assert!(find_symbol(&after, "◉").is_empty() && find_symbol(&after, ")").is_empty(), "and ends");
    assert_eq!(find_symbol(&after, "◤").len(), 1, "the arrow stays on the target");
    // A frame that is not a click keeps drawing the pointer as an arrow.
    v.clear();
    assert!(!v.visible(t2));
}

#[test]
fn the_conversation_makes_room_for_the_mini_browser() {
    let mut h = harness();
    h.app.apply(Out::Lines(vec![Line::raw("uma linha longa da conversa que precisa caber ao lado da janela do navegador")]));
    let wide = screen_rows(&mut h, 130, 36);
    h.app.apply(Out::Browser(browser_frame(Some((100.0, 80.0)), false)));
    // Let it finish opening.
    std::thread::sleep(std::time::Duration::from_millis(700));
    let rows = screen_rows(&mut h, 130, 36);
    let all = rows.join("\n");
    assert!(all.contains("index.html") && all.contains("clicando"), "the window is on screen:\n{all}");
    // It sits on the right, and the text stays on the left.
    let addr = rows.iter().find(|r| r.contains("index.html")).unwrap();
    assert!(addr.find("index.html").unwrap() > 60, "{addr}");
    assert!(rows.iter().any(|r| r.contains("uma linha longa") && r.find("uma linha").unwrap() < 5));
    assert!(!wide.join("\n").contains("index.html"));
    // A small terminal keeps the text instead.
    let mut small = harness();
    small.app.apply(Out::Browser(browser_frame(Some((100.0, 80.0)), false)));
    assert!(!screen_rows(&mut small, 80, 30).join("\n").contains("index.html"));
    // A new job closes it.
    h.app.apply(Out::ClearWork);
    assert!(!screen_rows(&mut h, 130, 36).join("\n").contains("index.html"));
}

#[test]
fn browse_reads_as_a_step_count_in_the_status_line() {
    use coturbo_cli::tui::host::action_label;
    use serde_json::json;
    assert_eq!(action_label("browse", &json!({"actions": [{"do": "open"}, {"do": "snapshot"}, {"do": "click"}]})), "browse · 3 step(s)");
}

#[test]
fn the_mini_browser_opens_from_the_corner_page_on_top_to_bottom_and_closes_back() {
    use coturbo_cli::tui::browser::BrowserView;
    use std::time::{Duration, Instant};
    let ms = Duration::from_millis;
    let t0 = Instant::now();
    let mut v = BrowserView::default();
    assert_eq!(v.progress(t0), 0.0, "nothing before the first picture");
    v.apply(browser_frame(Some((100.0, 80.0)), false), t0);
    // It opens: the progress rises smoothly and is complete by the end.
    let p = |t: u64| v.progress(t0 + ms(t));
    assert!(p(0) < 0.05 && p(100) < p(250) && p(250) < p(400) && p(400) < 1.0, "{} {} {}", p(100), p(250), p(400));
    assert_eq!(p(700), 1.0);
    assert_eq!(p(20_000), 1.0, "and stays open");
    assert_eq!(p(300_000), 1.0, "for minutes");
    // It grows from the top right corner: the left edge of the frame travels to the left.
    let left = |t: u64| find_symbol(&draw_browser(&v, t0 + ms(t)), "╭").first().map(|c| c.0);
    let (a, b, c) = (left(120).unwrap(), left(280).unwrap(), left(700).unwrap());
    assert!(a > b && b > c && c <= 6, "{a} {b} {c}");
    let top = |t: u64| find_symbol(&draw_browser(&v, t0 + ms(t)), "╭").first().map(|c| c.1);
    assert_eq!(top(120), Some(0), "anchored at the top");
    // The page is not there while the frame opens; it then appears from the top down.
    let rows_with_page = |t: u64| {
        let buf = draw_browser(&v, t0 + ms(t));
        let mut rows: Vec<u16> = find_symbol(&buf, "▀").iter().map(|c| c.1).collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    };
    assert!(rows_with_page(60).is_empty(), "frame first");
    let (early, later, full) = (rows_with_page(300), rows_with_page(420), rows_with_page(900));
    assert!(!early.is_empty() && early.len() < later.len() && later.len() < full.len(), "{} < {} < {}", early.len(), later.len(), full.len());
    assert_eq!(early.first(), full.first(), "it reveals downwards from the same top row");
    // It closes after the lingering, shrinking back, and is gone after that.
    let linger = ms(600_000);
    assert_eq!(v.progress(t0 + linger), 1.0);
    let half = v.progress(t0 + linger + ms(170));
    assert!(half > 0.1 && half < 0.9, "{half}");
    assert_eq!(v.progress(t0 + linger + ms(400)), 0.0);
    assert!(v.visible(t0 + linger + ms(200)) && !v.visible(t0 + linger + ms(400)), "visible while it closes, not after");
    assert!(draw_browser(&v, t0 + linger + ms(400)).content.iter().all(|c| c.symbol() == " "), "nothing left drawn");
    // A new picture while it closes opens it again from where it is.
    let mid = t0 + linger + ms(170);
    v.apply(browser_frame(Some((100.0, 80.0)), false), mid);
    assert!(v.progress(mid) > 0.1 && v.progress(mid) < 0.9, "it does not jump: {}", v.progress(mid));
    assert!(v.progress(mid + ms(100)) > v.progress(mid));
    assert_eq!(v.progress(mid + ms(900)), 1.0);
}

#[test]
fn the_conversation_gives_way_as_the_window_opens() {
    let mut h = harness();
    h.app.apply(Out::Lines(vec![Line::raw("uma linha longa da conversa que precisa caber ao lado da janela do navegador")]));
    h.app.apply(Out::Browser(browser_frame(Some((100.0, 80.0)), false)));
    // Just opened: the window is a small frame in the corner and the text still has most of the room.
    let start = screen_rows(&mut h, 130, 36).join("\n");
    assert!(start.contains("uma linha longa da conversa que precisa caber ao lado da janela do navegador"), "{start}");
    assert!(!start.contains("index.html"), "no address yet: the page has not appeared");
    std::thread::sleep(std::time::Duration::from_millis(700));
    let open = screen_rows(&mut h, 130, 36).join("\n");
    assert!(open.contains("index.html") && open.contains("clicando"), "{open}");
    assert!(!open.contains("uma linha longa da conversa que precisa caber ao lado da janela do navegador"), "the text wrapped to make room");
}

#[test]
fn the_page_texts_are_written_over_the_blur_so_it_can_be_read() {
    use coturbo_cli::tui::browser::BrowserView;
    use ratatui::style::{Color, Modifier};
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let mut v = BrowserView::default();
    v.apply(browser_frame(None, false), t0);
    let buf = draw_browser(&v, t0 + Duration::from_secs(1));
    let text = buffer_text(&buf);
    assert!(text.contains("Casa do Cafe"), "the heading of the page is readable:\n{text}");
    assert!(text.contains("Fazer pedido") || text.contains("Fazer ped"), "the button is readable:\n{text}");
    // The letters sit where the heading is (top left), on the blue header, in a colour that reads on it.
    let cells: Vec<(u16, u16)> = (0..buf.area.height).flat_map(|y| (0..buf.area.width).map(move |x| (x, y))).collect();
    let word_at = |x: u16, y: u16, w: &str| w.chars().enumerate().all(|(i, c)| x + (i as u16) < buf.area.width && buf[(x + i as u16, y)].symbol() == c.to_string());
    let (x, y) = *cells.iter().find(|&&(x, y)| word_at(x, y, "Casa do")).expect("the heading");
    let cell = &buf[(x, y)];
    let lum = |c: Color| match c {
        Color::Rgb(r, g, b) => 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b),
        _ => 0.0,
    };
    assert!(matches!(cell.bg, Color::Rgb(..)), "the letter sits on the colour of the page behind it: {:?}", cell.bg);
    assert!((lum(cell.fg) - lum(cell.bg)).abs() > 90.0, "readable: letters {:?} on {:?}", cell.fg, cell.bg);
    assert!(cell.modifier.contains(Modifier::BOLD), "a heading is bold");
    // The button sits low and to the right; its letters read on it.
    let (bx, by) = *cells.iter().find(|&&(x, y)| buf[(x, y)].symbol() == "F" && y > 8).expect("the F of the button");
    assert!(bx > x + 15 && by > y + 4, "right and below the heading: ({bx},{by}) vs ({x},{y})");
    assert_eq!(text.matches("Casa do Cafe").count(), 1, "written once");
    // A text too small to say anything in cells is left out.
    assert!(!text.contains("ZQJ"), "a 6 px wide text is lost in a cell");
}

#[test]
fn esc_closes_the_mini_browser_when_nothing_is_running() {
    use std::time::Duration;
    let mut h = harness();
    h.app.apply(Out::Browser(browser_frame(Some((100.0, 80.0)), false)));
    std::thread::sleep(Duration::from_millis(650));
    assert!(screen_rows(&mut h, 130, 36).join("\n").contains("Esc closes"), "the caption says how to close it when idle");
    // While a task runs, Esc interrupts the task and leaves the window alone.
    h.app.apply(Out::Busy(Some("think".into())));
    h.press(KeyCode::Esc);
    assert!(h.cancel.try_recv().is_ok(), "the task was interrupted");
    let working = screen_rows(&mut h, 130, 36).join("\n");
    assert!(!working.contains("Esc closes") && working.contains("index.html"), "no hint while it works, and the window stays");
    h.app.apply(Out::Busy(None));
    // Idle: Esc closes it, shrinking away.
    h.press(KeyCode::Esc);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!screen_rows(&mut h, 130, 36).join("\n").contains("index.html"), "closed");
    // A narrow terminal (80 columns) still shows the window.
    let mut narrow = harness();
    narrow.app.apply(Out::Browser(browser_frame(Some((100.0, 80.0)), false)));
    std::thread::sleep(Duration::from_millis(650));
    assert!(screen_rows(&mut narrow, 80, 30).join("\n").contains("index.html"));
}

#[test]
fn every_hosted_provider_has_a_key_and_an_endpoint_or_is_local() {
    use coturbo_cli::settings::{PROVIDERS, provider};
    for id in ["gemini", "groq", "deepseek", "mistral", "xai", "together", "openrouter"] {
        let p = provider(id).unwrap_or_else(|| panic!("{id} missing"));
        assert!(p.key_var.is_some() && p.base_url.is_some_and(|u| u.starts_with("https://")), "{id}");
    }
    let lm = provider("lmstudio").expect("lmstudio");
    assert!(lm.key_var.is_none() && lm.base_url.is_some_and(|u| u.contains("localhost")));
    let mut ids: Vec<_> = PROVIDERS.iter().map(|p| p.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), PROVIDERS.len(), "duplicate provider ids");
}

#[test]
fn model_ids_lose_the_gemini_prefix() {
    let ids = parse_ids("gemini", &json!({"data": [{"id": "models/gemini-2.5-pro"}]}));
    assert_eq!(ids, ["gemini-2.5-pro"]);
}

#[test]
fn only_providers_with_what_they_need_are_detected() {
    let mut creds = Credentials::default();
    // A hosted provider shows up once it has a key (its env var may also provide one).
    creds.set("deepseek", "k");
    assert!(settings::detected("deepseek", &creds));
    assert!(!settings::detected("no-such-provider", &creds));
    // A plan or local server is detected from the machine, never from a key.
    assert_eq!(settings::detected("claude-code", &creds), coturbo_providers::cli::on_path("claude"));
    assert_eq!(settings::detected("codex", &creds), coturbo_providers::cli::on_path("codex"));
}

#[test]
fn with_a_picture_protocol_the_page_is_one_picture_with_the_pointer_in_it() {
    use coturbo_cli::tui::browser::BrowserView;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let mut v = BrowserView::default();
    assert!(!v.has_native());
    v.set_picker(ratatui_image::picker::Picker::halfblocks());
    assert!(v.has_native());
    v.apply(browser_frame(Some((60.0, 40.0)), true), t0);
    let later = t0 + Duration::from_secs(2);
    let area = ratatui::layout::Rect::new(0, 0, 60, 24);
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal
        .draw(|f| {
            v.draw(f.buffer_mut(), area, later);
            v.draw_native(f, area, later);
        })
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let text = buffer_text(&buf);
    assert!(text.contains("clicando \"Sobre\""), "the window frame is still drawn:\n{text}");
    // The page itself comes from the picture: it has the blue header of the fixture.
    let blue = ratatui::style::Color::Rgb(47, 123, 255);
    let any_blue = (0..24).flat_map(|y| (0..60).map(move |x| (x, y))).any(|(x, y)| buf[(x, y)].fg == blue || buf[(x, y)].bg == blue);
    assert!(any_blue, "the page's header is in the picture:\n{text}");
}

#[test]
fn a_new_task_keeps_the_terminals_picture_protocol() {
    use coturbo_cli::tui::browser::BrowserView;
    let mut v = BrowserView::default();
    v.set_picker(ratatui_image::picker::Picker::halfblocks());
    v.apply(browser_frame(None, false), std::time::Instant::now());
    v.clear();
    assert!(v.has_native(), "clearing the window for a new task must not forget how the terminal draws pictures");
    assert!(!v.visible(std::time::Instant::now()), "but the old page is gone");
}

#[test]
fn the_code_being_written_shows_in_a_box_while_it_is_written() {
    use ratatui::text::Line;
    let mut h = harness();
    h.app.apply(Out::Busy(Some("write_file app.js · 300 chars".into())));
    h.app.apply(Out::Live(Some(("writing app.js".into(), vec![Line::raw("  const total = items.length;"), Line::raw("  render(total);")]))));
    let rows = screen_rows(&mut h, 100, 34).join("\n");
    assert!(rows.contains("writing app.js") && rows.contains("const total = items.length;"), "{rows}");
    // A finished call clears it.
    h.app.apply(Out::Live(None));
    assert!(!screen_rows(&mut h, 100, 34).join("\n").contains("const total"));
}

#[test]
fn several_pages_share_the_space_of_one_window_with_only_the_address_on_top() {
    use coturbo_cli::tui::browser::BrowserView;
    use std::time::{Duration, Instant};
    let t0 = Instant::now();
    let frame = |window: usize, url: &str| {
        let mut f = browser_frame(None, false);
        f.window = window;
        f.url = url.into();
        f
    };
    let mut one = BrowserView::default();
    one.apply(frame(0, "file:///site/index.html"), t0);
    let mut four = BrowserView::default();
    for (i, page) in ["index", "about", "menu", "contact"].iter().enumerate() {
        four.apply(frame(i, &format!("file:///site/{page}.html")), t0);
    }
    assert_eq!((one.windows(), four.windows()), (1, 4));
    // Several pages share the height they are given.
    assert!(four.rows_wide(60, 24) <= 24, "{}", four.rows_wide(60, 24));
    let later = t0 + Duration::from_secs(2);
    let area = ratatui::layout::Rect::new(0, 0, 60, 24);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    four.draw(&mut buf, area, later);
    let text = buffer_text(&buf);
    for page in ["index.html", "about.html", "menu.html", "contact.html"] {
        assert!(text.contains(page), "{page} on screen:\n{text}");
    }
    // Like a browser: the address, no coloured dots, and no page title.
    assert!(!text.contains('●') && !text.contains("Shop"), "{text}");
    // Each caption says which window it is.
    assert!(text.contains("1 · ") && text.contains("4 · "), "{text}");
    // Six at most, and a seventh frame lands in the last one.
    let mut six = BrowserView::default();
    for i in 0..9 {
        six.apply(frame(i, &format!("file:///p{i}.html")), t0);
    }
    assert_eq!(six.windows(), 6);
}

#[test]
fn the_frame_hugs_the_page_and_the_caption_sits_below_it_outside() {
    use coturbo_cli::tui::browser::BrowserView;
    let t0 = std::time::Instant::now();
    let mut v = BrowserView::default();
    v.apply(browser_frame(None, false), t0);
    let buf = draw_browser(&v, t0 + std::time::Duration::from_secs(2));
    let text = buffer_text(&buf);
    let rows: Vec<&str> = text.lines().collect();
    let bottom = rows.iter().position(|r| r.contains('╰')).expect("the frame's bottom edge");
    let caption = rows.iter().position(|r| r.contains("clicando")).expect("the caption");
    assert!(caption > bottom, "the caption is under the frame, not inside it:\n{text}");
    // The frame is as wide as the picture: no empty bars at its sides.
    let top = rows.iter().find(|r| r.contains('╭')).unwrap();
    let page_row = rows.iter().find(|r| r.contains('▀')).unwrap();
    assert_eq!(top.find('╭'), page_row.find('│'), "frame and page share their left edge:\n{text}");
    assert!(!rows[..bottom].iter().any(|r| r.contains("clicando")));
}

#[test]
fn four_pages_all_sit_at_the_right_of_the_conversation() {
    let mut h = harness();
    h.app.apply(Out::Lines(vec![ratatui::text::Line::raw("a long line of the conversation on the left")]));
    h.app.apply(Out::Busy(Some("browse".into())));
    for (i, name) in ["one", "two", "three", "four"].iter().enumerate() {
        let mut f = browser_frame(None, false);
        f.window = i;
        f.url = format!("https://example.com/{name}");
        h.app.apply(Out::Browser(f));
    }
    std::thread::sleep(std::time::Duration::from_millis(700));
    let rows = screen_rows(&mut h, 150, 40);
    let all = rows.join("\n");
    for name in ["one", "two", "three", "four"] {
        let row = rows.iter().find(|r| r.contains(&format!("example.com/{name}"))).unwrap_or_else(|| panic!("{name} on screen:\n{all}"));
        assert!(row.find("example.com").unwrap() > 60, "{name} at the right: {row}");
    }
    assert!(rows.iter().any(|r| r.contains("a long line") && r.find("a long").unwrap() < 5), "the conversation stays at the left");
}

#[test]
fn the_code_box_sits_beside_the_pages_not_under_them() {
    use ratatui::text::Line;
    let mut h = harness();
    h.app.apply(Out::Busy(Some("write_file app.js · 1k chars".into())));
    for (i, name) in ["one", "two"].iter().enumerate() {
        let mut f = browser_frame(None, false);
        f.window = i;
        f.url = format!("https://example.com/{name}");
        h.app.apply(Out::Browser(f));
    }
    h.app.apply(Out::Live(Some(("writing app.js".into(), vec![Line::raw("  const total = 1;"), Line::raw("  render(total);")]))));
    std::thread::sleep(std::time::Duration::from_millis(700));
    let rows = screen_rows(&mut h, 150, 40);
    let all = rows.join("\n");
    let code = rows.iter().position(|r| r.contains("const total")).unwrap_or_else(|| panic!("code on screen:\n{all}"));
    let page = rows.iter().rposition(|r| r.contains("example.com")).expect("page address");
    // The box shares its rows with the pages, and stays at the left of them.
    let (code_col, page_col) = (rows[code].find("const total").unwrap(), rows[page].find("example.com").unwrap());
    assert!(code_col < page_col, "{code_col} vs {page_col}:\n{all}");
    assert!(code > page || code + 12 > page, "the box is in line with the pages:\n{all}");
}

#[test]
fn the_logo_slides_in_from_outside_and_lands_exactly() {
    use coturbo_cli::tui::fx::{INTRO_MS, slide_in};
    let w = 150;
    let mut grid = vec![vec![None; w]; 4];
    for (r, c) in [(0usize, 3usize), (1, 20), (2, 120), (3, 140)] {
        grid[r][c] = Some((200, 100, 50));
    }
    let count = |g: &coturbo_cli::tui::fx::Grid| g.iter().flatten().filter(|p| p.is_some()).count();
    // Before anything lands, nothing of it is on the canvas (it is outside the terminal).
    assert_eq!(count(&slide_in(&grid, 0)), 0);
    // Half way, the columns are somewhere between outside and home, none beyond the edge.
    let mid = slide_in(&grid, INTRO_MS / 2);
    assert!(count(&mid) > 0 && mid != grid, "it is moving");
    // At the end it is the picture itself, and it stays so.
    assert_eq!(slide_in(&grid, INTRO_MS), grid);
    assert_eq!(slide_in(&grid, INTRO_MS * 10), grid);
}

#[test]
fn a_glint_lights_part_of_the_picture_now_and_then() {
    use coturbo_cli::tui::fx::highlight;
    let base = vec![vec![Some((40u8, 90u8, 200u8)); 60]; 6];
    let lit = |ms: u128| {
        let mut g = base.clone();
        highlight(&mut g, ms);
        g.iter().flatten().filter(|p| **p != Some((40, 90, 200))).count()
    };
    assert!(lit(500) > 0 && lit(500) < 60 * 6, "a band, not the whole picture");
    assert_eq!(lit(3000), 0, "and most of the time nothing is lit");
}

#[test]
fn clicking_folded_code_still_works_with_pages_and_the_code_box_open() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut h = harness();
    h.app.apply(Out::Busy(Some("write_file x".into())));
    let full: Vec<Line<'static>> = (0..30).map(|i| Line::raw(format!("linha {i}"))).collect();
    let mut preview = full[..10].to_vec();
    preview.push(Line::raw("… mais 20"));
    h.app.apply(Out::Fold { preview, full });
    for i in 0..2 {
        let mut f = browser_frame(None, false);
        f.window = i;
        h.app.apply(Out::Browser(f));
    }
    h.app.apply(Out::Live(Some(("writing x".into(), vec![Line::raw("let a = 1;")]))));
    std::thread::sleep(std::time::Duration::from_millis(700));
    let mut term = Terminal::new(TestBackend::new(150, 44)).unwrap();
    term.draw(|f| h.app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let row = (0..44).find(|&y| (0..150).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().contains("linha 3")).expect("the code is on screen");
    let click = |row| Event::Mouse(MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 5, row, modifiers: KeyModifiers::NONE });
    h.app.on_event(click(row), false);
    assert_eq!(h.app.transcript().lines().len(), 30, "a click opens the code");
}

#[test]
fn the_sign_in_screen_says_where_each_provider_stands() {
    let mut creds = Credentials::default();
    assert_eq!(settings::standing("deepseek", &creds), "");
    creds.set("deepseek", "k");
    assert!(settings::standing("deepseek", &creds).contains("connected (api key)"));
    assert!(settings::standing("antigravity", &creds).contains("shares the IDE's login"));
}

#[test]
fn a_report_in_a_box_is_wrapped_and_closed_on_a_straight_edge() {
    use coturbo_cli::tui::text::{boxed, width};
    use ratatui::style::Color;
    let body = vec![Line::raw("a long line of the report that must wrap inside the box without breaking its right edge at all")];
    let rows = boxed(&body, 40, Color::White, vec![]);
    assert!(rows.len() >= 4, "wrapped to several rows");
    let widths: Vec<usize> = rows[1..rows.len() - 1].iter().map(width).collect();
    assert!(widths.iter().all(|w| *w == widths[0]), "every row the same width: {widths:?}");
    assert!(rows[1].spans.iter().any(|s| s.content.contains('┆')));
}
