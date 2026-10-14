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
    assert_eq!(names, ["/modelo"]);
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
    let (rows, scroll) = t.view(40, 3, 0);
    assert_eq!(rows.iter().map(plain).collect::<Vec<_>>(), ["linha 7", "linha 8", "linha 9"]);
    assert_eq!(scroll, 0);
    let (rows, scroll) = t.view(40, 3, 2);
    assert_eq!(rows.iter().map(plain).collect::<Vec<_>>(), ["linha 5", "linha 6", "linha 7"]);
    assert_eq!(scroll, 2);
    let (rows, scroll) = t.view(40, 3, 99);
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
        approval: "edição livre".into(),
        ..Status::default()
    };
    Harness { app: App::new(status, tx, ctx, Arc::new(AtomicBool::new(false))), rx, cancel }
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
    assert_eq!(h.rx.try_recv().unwrap(), In::Submit("/modelo".into()));
    h.typed("/pro");
    h.press(KeyCode::Tab);
    assert_eq!(h.app.input().text(), "/provedor ");
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
    assert!(shown.iter().any(|l| l.contains("na fila")), "{shown:?}");
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
    assert!(all.contains("Descreva a tarefa"), "placeholder in the empty box");
    assert!(rows.last().unwrap().contains("anthropic · claude-sonnet-5-5 · edição livre"), "{}", rows.last().unwrap());
    assert!(rows.last().unwrap().contains("contexto 12%"));
    // The logo is drawn in half blocks, in the brand blue.
    let blue = Color::Rgb(47, 123, 255);
    let logo = (0..8).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| {
        let c = &buf[(x, y)];
        (c.symbol() == "▀" || c.symbol() == "▄") && (c.fg == blue || c.bg == blue)
    });
    assert!(logo.count() > 20, "logo pixels on screen");

    // Typing `/` lists the commands above the box.
    h.typed("/");
    term.draw(|f| h.app.draw(f)).unwrap();
    let buf = term.backend().buffer().clone();
    let all: String = (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n")
        .collect();
    assert!(all.contains("/provedor") && all.contains("escolher o modelo"), "{all}");

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
    assert!(plain(preview.last().unwrap()).contains("mais 30 linhas · Ctrl+B"));
    assert_eq!(full.len(), 41, "every line, nothing capped");
    let short = diff_fold("@@ -3,1 +3,1 @@\n-velho\n+novo\n", 80);
    assert!(matches!(short, Out::Lines(ref l) if l.len() == 2), "a small fix is shown whole");

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
