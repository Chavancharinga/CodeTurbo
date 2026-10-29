use coturbo_design::color::{contrast, hex, hue_sat, oklch, parse_hex};
use coturbo_design::compiler::{Brief, Tone, compile};
use coturbo_design::contract::{self, Contract};
use coturbo_design::fiscal::{self, Severity};
use coturbo_design::render::{Metrics, Page, dom_from_log, judge, parse_metrics};

fn brief(tone: &str) -> Brief {
    Brief {
        subject: "aplicativo de finanças pessoais Poupa+".into(),
        audience: "jovens adultos no primeiro emprego".into(),
        tone: tone.into(),
        signature: "um cofre desenhado com linhas finas que enche conforme a meta".into(),
        brand: Vec::new(),
        fonts: Vec::new(),
        radii: Vec::new(),
        style: String::new(),
    }
}

#[test]
fn colors_convert_and_measure() {
    assert_eq!(oklch(1.0, 0.0, 0.0), (255, 255, 255));
    assert_eq!(oklch(0.0, 0.0, 0.0), (0, 0, 0));
    assert_eq!(parse_hex("#FfF"), Some((255, 255, 255)));
    assert_eq!(parse_hex("#10b981"), Some((16, 185, 129)));
    assert_eq!(hex((16, 185, 129)), "#10b981");
    assert!((contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.01);
    assert!(contrast((255, 255, 255), (16, 185, 129)) < 3.0, "white on emerald is hard to read");
    let (h, s) = hue_sat((255, 0, 0));
    assert!(h.abs() < 0.5 && s > 0.99);
}

#[test]
fn tones_come_from_plain_words() {
    assert_eq!(Tone::from_words("editorial e calmo"), Tone::Editorial);
    assert_eq!(Tone::from_words("divertido e jovem"), Tone::Playful);
    assert_eq!(Tone::from_words("técnico, preciso"), Tone::Technical);
    assert_eq!(Tone::from_words("acolhedor"), Tone::Warm);
    assert_eq!(Tone::from_words("moderno"), Tone::Balanced);
}

#[test]
fn compiled_tokens_are_readable_and_not_the_defaults() {
    for tone in ["editorial e sóbrio", "divertido", "técnico", "acolhedor", "moderno", "confiável e humano"] {
        let t = compile(&brief(tone));
        let bg = t.color("bg");
        assert!(contrast(t.color("ink"), bg) >= 7.0, "{tone}: body text");
        assert!(contrast(t.color("muted"), bg) >= 4.5, "{tone}: secondary text");
        assert!(contrast(t.color("on-primary"), t.color("primary")) >= 4.5, "{tone}: button label");
        assert!(contrast(t.color("primary"), bg) >= 3.0, "{tone}: primary against the page");
        for f in [&t.font_display, &t.font_body] {
            assert!(!["Inter", "Roboto", "Poppins", "Open Sans", "Montserrat", "Lato"].contains(&f.as_str()), "{f}");
        }
        let radii: std::collections::BTreeSet<u32> = t.radius.iter().map(|(_, r)| *r).collect();
        // Square on purpose only in the brutalist style.
        assert!(radii.len() >= 2 || t.style == coturbo_design::style::Style::Brutalist, "{tone}: distinct radii");
        // Same brief, same tokens.
        assert_eq!(compile(&brief(tone)), t);
    }
    let a = compile(&brief("divertido"));
    let mut other = brief("divertido");
    other.subject = "estúdio de cerâmica".into();
    assert_ne!(a.colors, compile(&other).colors, "different subjects get different palettes");
}

#[test]
fn the_contract_round_trips() {
    let t = compile(&brief("editorial"));
    let md = contract::markdown(&t);
    assert!(md.contains("Signature element: um cofre"));
    assert!(md.contains("fonts.googleapis.com/css2?family="));
    let c = Contract::parse(&md);
    assert_eq!(c.colors().len(), 11);
    assert_eq!(c.space(), t.space);
    assert_eq!(c.radii(), t.radius.iter().map(|(_, r)| *r).collect::<Vec<_>>());
    assert!(c.vars["--font-display"].contains(&t.font_display));
}

const GENERIC: &str = r##"<!DOCTYPE html><html lang="pt-BR"><head><style>
body { font-family: 'Inter', sans-serif; color: #1e293b; }
.hero { background: linear-gradient(135deg, #667eea, #ec4899); text-align: center; }
.btn { background: #10b981; color: #ffffff; border-radius: 12px; }
.card { border-radius: 12px; box-shadow: 0 4px 6px rgba(0,0,0,.1); text-align: center; }
.card:hover { transform: translateY(-5px); transition: all .3s; }
.plan { border-radius: 12px; text-align: center; }
.faq { border-radius: 12px; text-align: center; }
</style></head><body>
<section id="hero"><h1>Domine</h1><a class="btn" href="#x">Comece</a></section>
<section id="features"><h3>Recursos</h3>
<div class="card">📊 Gráficos</div><div class="card">💰 Metas</div><div class="card">🔒 Segurança</div></section>
<section id="testimonials"><h2>Depoimentos</h2></section>
<section id="pricing"><h2>Planos</h2></section>
<section id="faq"><h2>Perguntas</h2></section>
<section id="contact"><h2>Contato</h2></section>
<footer>©</footer></body></html>"##;

#[test]
fn a_generic_page_is_refused_for_the_right_reasons() {
    let f = fiscal::check("index.html", GENERIC, None);
    let rules: Vec<&str> = f.iter().map(|x| x.rule).collect();
    for r in [
        "default-font",
        "generic-palette",
        "generic-gradient",
        "contrast",
        "single-radius",
        "emoji-icon",
        "identical-cards",
        "headings",
        "missing-focus-style",
        "no-contract",
        "hover-lift",
        "all-centered",
        "motion",
    ] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}
{}", fiscal::render("index.html", &f));
    }
    assert!(f.iter().any(|x| x.rule == "contrast" && x.severity == Severity::P0));
    let report = fiscal::render("index.html", &f);
    assert!(report.starts_with("design index.html: "), "{report}");
    assert!(report.contains("index.html:2 [P1 default-font]"), "{report}");
}

#[test]
fn a_page_built_on_the_contract_passes() {
    let t = compile(&brief("editorial e sóbrio"));
    let c = Contract::parse(&contract::markdown(&t));
    let page = format!(
        r#"<!DOCTYPE html><html lang="pt-BR"><head><style>
{}
body {{ font-family: var(--font-body); color: var(--color-ink); background: var(--color-bg); }}
h1 {{ font-family: var(--font-display); font-size: var(--text-display); }}
.btn {{ background: var(--color-primary); color: var(--color-on-primary); border-radius: var(--radius-sm); padding: 12px 24px; }}
.btn:focus-visible {{ outline: 2px solid var(--color-accent); }}
.ledger {{ border-top: 1px solid var(--color-line); margin: 48px 0; }}
</style></head><body>
<header><h1>Poupa+</h1><a class="btn" href="/conta">Abrir conta</a></header>
<main><h2>Quanto falta para a sua meta</h2><p class="ledger">Texto.</p></main>
</body></html>"#,
        contract::css(&t)
    );
    let f = fiscal::check("index.html", &page, Some(&c));
    assert!(f.iter().all(|x| !x.blocking()), "{f:?}");
    // A stray color is caught against the contract.
    let stray = page.replace("background: var(--color-primary)", "background: #123456");
    let f = fiscal::check("index.html", &stray, Some(&c));
    assert!(f.iter().any(|x| x.rule == "color-off-contract"), "{f:?}");
}

#[test]
fn only_interface_files_are_inspected() {
    assert!(fiscal::is_ui_file("src/App.tsx"));
    assert!(fiscal::is_ui_file("index.HTML"));
    assert!(!fiscal::is_ui_file("src/main.rs"));
    assert!(fiscal::check("style.css", "a:hover { color: red; }", None).iter().all(|f| f.rule != "missing-focus-style"));
}

#[test]
fn rendered_measurements_are_judged() {
    let dom = r#"<html data-codeturbo="{&quot;rounded&quot;:9,&quot;radii&quot;:1,&quot;shadowed&quot;:5,&quot;shadows&quot;:1,&quot;centered&quot;:9,&quot;text&quot;:10,&quot;rows&quot;:3,&quot;overlaps&quot;:[&quot;div.card × img&quot;],&quot;tight&quot;:[&quot;a.logo | a&quot;],&quot;broken&quot;:[&quot;img&quot;],&quot;hscroll&quot;:true,&quot;scrollers&quot;:[&quot;div.box&quot;],&quot;small&quot;:4,&quot;tiny&quot;:0,&quot;cut&quot;:[&quot;h1.title&quot;]}"><body></body></html>"#;
    let m = parse_metrics(dom).unwrap();
    assert_eq!((m.rounded, m.radii, m.rows, m.small), (9, 1, 3, 4));
    assert_eq!(m.overlaps, ["div.card × img"]);
    assert!(m.hscroll);
    let page = Page { desktop: m.clone(), mobile: m, ..Page::default() };
    let rules: Vec<&str> = judge(&page).iter().map(|f| f.rule).collect();
    for r in [
        "render-overlap", "render-cramped", "render-cut-off", "render-scrollbar", "render-broken-image",
        "render-not-responsive", "render-small-target", "render-identical-grids", "render-single-radius",
        "render-all-centered", "render-single-shadow",
    ] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}");
    }
    let clean = Metrics { rounded: 9, radii: 3, text: 10, centered: 2, ..Metrics::default() };
    assert!(judge(&Page { desktop: clean.clone(), mobile: clean, ..Page::default() }).is_empty());
    assert!(parse_metrics("<html>").is_none());
}

/// The rendered page is checked for accessibility the way a person meets it:
/// contrast against what is really behind the text, images with no alt,
/// fields with no label, no page language, no viewport.
#[tokio::test]
async fn the_rendered_page_is_checked_for_accessibility() {
    let Some(browser) = coturbo_design::render::find_browser() else { return };
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("a11y.html");
    std::fs::write(
        &page,
        r##"<!DOCTYPE html><html><head><meta charset="utf-8"><title>t</title></head>
<body style="background:#ffffff;font-family:Georgia;padding:24px">
<section style="background:#1f2a44;padding:24px"><p style="color:#2b3550">Dark text on a dark band, inherited background.</p></section>
<p style="color:#111111">Readable text on white.</p>
<img src="data:image/gif;base64,R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==" width="80" height="80">
<form><input type="email" placeholder="Your e-mail"><label>Name <input type="text"></label></form>
</body></html>"##,
    )
    .unwrap();
    let m = coturbo_design::render::measure(&browser, &page, &dir.path().join("a11y.png")).await.expect("rendered");
    assert_eq!(m.desktop.contrast.len(), 1, "only the dark-on-dark text: {:?}", m.desktop.contrast);
    assert!(m.desktop.contrast[0].contains("Dark text"), "{:?}", m.desktop.contrast);
    assert_eq!((m.desktop.noalt, m.desktop.unlabeled), (1, 1), "the image and the placeholder-only field");
    assert!(m.desktop.nolang && m.desktop.noviewport);
    let rules: Vec<&str> = judge(&m).iter().map(|f| f.rule).collect();
    for r in ["render-low-contrast", "render-missing-alt", "render-unlabeled-field", "render-no-viewport", "render-no-lang"] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}");
    }
}

/// Rhythm and coherence: lines too long to read, a pile of font sizes, a
/// patchwork of colours.
#[tokio::test]
async fn the_rendered_page_is_checked_for_rhythm_and_coherence() {
    let Some(browser) = coturbo_design::render::find_browser() else { return };
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("rhythm.html");
    let long = "Words that keep going across the whole width of a very wide desktop screen without any measure at all. ".repeat(4);
    let sizes: String = (0..11).map(|i| format!("<p style=\"font-size:{}px;color:#{:02x}2a3a\">Size {i}</p>", 12 + i * 3, 20 + i * 9)).collect();
    std::fs::write(
        &page,
        format!(r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"></head>
<body style="font-family:Georgia;margin:0;padding:16px"><p style="font-size:16px">{long}</p>{sizes}</body></html>"#),
    )
    .unwrap();
    let m = coturbo_design::render::measure(&browser, &page, &dir.path().join("r.png")).await.expect("rendered");
    let rules: Vec<&str> = judge(&m).iter().map(|f| f.rule).collect();
    for r in ["render-long-lines", "render-too-many-sizes", "render-too-many-colours"] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}");
    }
}

#[tokio::test]
async fn review_renders_in_the_installed_browser() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), GENERIC).unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    assert!(coturbo_design::review(dir.path(), &["main.rs".into()]).await.is_none());
    let r = coturbo_design::review(dir.path(), &["index.html".into()]).await.unwrap();
    assert!(!r.ok);
    assert!(r.summary.starts_with("✗ design refused"), "{}", r.summary);
    if coturbo_design::render::find_browser().is_some() {
        assert!(r.summary.contains("rendered page"), "{}", r.summary);
        assert!(dir.path().join(".codeturbo/design/index.png").exists(), "desktop screenshot saved");
        assert!(dir.path().join(".codeturbo/design/index-phone.png").exists(), "phone screenshot saved");
        assert!(dir.path().join(".codeturbo/design/index-tablet.png").exists(), "tablet screenshot saved");
        assert!(dir.path().join(".codeturbo/design/index-laptop.png").exists(), "laptop screenshot saved");
    }
}

#[test]
fn repeated_accordion_items_are_not_cards() {
    let page = r#"<div class="faq-item">a</div><div class="faq-item">b</div><div class="faq-item">c</div>"#;
    assert!(fiscal::check("p.html", page, None).iter().all(|f| f.rule != "identical-cards"));
    let cards = page.replace("faq-item", "price-card");
    assert!(fiscal::check("p.html", &cards, None).iter().any(|f| f.rule == "identical-cards"));
}

#[test]
fn more_generic_tells_are_caught() {
    let page = r##"<html><head><style>
.nav { backdrop-filter: blur(12px); display: flex; gap: 8px; padding: 8px; margin: 0 auto; max-width: 1200px; width: 100%; }
button { outline: none; }
p { font-size: 12px; line-height: 1.1; text-align: justify; }
.modal { z-index: 99999; }
.grid { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 24px; padding: 48px; margin: 0 auto; max-width: 1100px; }
.a { color: red !important; } .b { color: red !important; } .c { color: red !important; }
.d { color: red !important; } .e { color: red !important; } .f { color: red !important; }
</style></head><body><p>Lorem ipsum dolor. Leve seu negócio ao próximo nível.</p>
<a href="#">a</a><a href="#">b</a><a href="#">c</a><img src="https://picsum.photos/200"></body></html>"##;
    let rules: Vec<&str> = fiscal::check("p.html", page, None).iter().map(|f| f.rule).collect();
    for r in [
        "frosted-glass", "outline-removed", "small-text", "line-height", "justified-text", "z-index", "important",
        "not-responsive", "placeholder-text", "placeholder-image", "empty-links", "cliche-copy",
    ] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}");
    }
    let fixed = page.replace(".grid {", "@media (max-width: 700px) { .grid { grid-template-columns: 1fr; } }\n.grid {");
    assert!(!fiscal::check("p.html", &fixed, None).iter().any(|f| f.rule == "not-responsive"));
}

#[test]
fn a_redesign_keeps_the_brand_and_organizes_it() {
    use coturbo_design::color::luminance;
    let mut b = brief("moderno");
    b.brand = vec!["#ffd60a".into(), "#ffffff".into(), "#111111".into()];
    let t = compile(&b);
    assert_eq!(t.color("primary"), (255, 214, 10), "the brand yellow stays exactly");
    assert_eq!(t.color("bg"), (255, 255, 255), "white stays the page");
    assert_eq!(t.color("ink"), (17, 17, 17), "black stays the text");
    assert!(contrast(t.color("on-primary"), t.color("primary")) >= 4.5, "button labels readable on yellow");
    assert!(contrast(t.color("primary-ink"), t.color("bg")) >= 4.5, "yellow made readable as text");
    let grays: Vec<_> = ["gray-100", "gray-300", "gray-500", "gray-700"].iter().map(|n| luminance(t.color(n))).collect();
    assert!(grays.windows(2).all(|w| w[0] > w[1]), "a neutral scale from light to dark");
    assert!(contract::markdown(&t).contains("Brand colours kept: #ffd60a"));
}

#[test]
fn brand_colors_are_found_in_existing_files() {
    let files = vec![(
        "site.css".to_string(),
        ".a{color:#111111} .b{background:#ffd60a} .c{background:#FFD60B} .d{color:#ffd60a} body{background:#fff}".to_string(),
    )];
    let found = fiscal::brand_colors(&files);
    assert_eq!(found[0], "#ffd60a", "most used first, near shades merged: {found:?}");
    assert!(found.contains(&"#111111".to_string()) && found.contains(&"#ffffff".to_string()));
}

mod kit {
    use super::*;
    use coturbo_design::kit::{base_css, blueprint, divider_svg, ornament_svg};

    fn tones() -> [&'static str; 5] {
        ["editorial", "divertido", "técnico", "acolhedor", "moderno"]
    }

    #[test]
    fn the_base_kit_already_passes_the_inspector() {
        for tone in tones() {
            let t = compile(&brief(tone));
            let c = Contract::parse(&contract::markdown(&t));
            let css = format!("{}\n{}", contract::css(&t), base_css(&t));
            let f = fiscal::check("kit.css", &css, Some(&c));
            assert!(f.iter().all(|x| !x.blocking()), "{tone}: {f:?}");
        }
    }

    #[test]
    fn every_project_gets_its_own_plan_and_ornaments() {
        let a = compile(&brief("acolhedor"));
        let mut other = brief("acolhedor");
        other.subject = "estúdio de cerâmica artesanal".into();
        let b = compile(&other);
        assert_eq!(blueprint(&a), blueprint(&a), "same project, same plan");
        assert_ne!(blueprint(&a), blueprint(&b), "another project, another plan");
        assert_ne!(ornament_svg(&a), ornament_svg(&b));
        assert_ne!(divider_svg(&a), divider_svg(&b));
        let plan = blueprint(&a);
        assert_eq!(plan.len(), 6, "opening, four sections, closing");
        let unique: std::collections::HashSet<_> = plan.iter().collect();
        assert_eq!(unique.len(), 6, "no section twice");
        for t in tones() {
            let tk = compile(&brief(t));
            for svg in [ornament_svg(&tk), divider_svg(&tk)] {
                assert!(svg.starts_with("<svg") && svg.ends_with("</svg>") && svg.contains("aria-hidden"), "{svg}");
                assert!(svg.len() < 4000, "small enough to inline: {}", svg.len());
            }
        }
    }

    #[tokio::test]
    async fn a_page_built_from_the_kit_renders_clean() {
        let t = compile(&brief("editorial e sóbrio"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("DESIGN.md"), contract::markdown(&t)).unwrap();
        let page = format!(
            r#"<!DOCTYPE html><html lang="pt-BR"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Poupa+</title>
<style>{}
{}
.hero {{ display: grid; gap: var(--space-8); grid-template-columns: repeat(auto-fit, minmax(min(100%, 320px), 1fr)); align-items: center; }}
</style></head><body>
<header class="container cluster" style="justify-content: space-between; padding-block: var(--space-4)"><strong>Poupa+</strong><a class="btn btn--secondary" href="/entrar">Entrar</a></header>
<main>
<section class="section"><div class="container hero"><div><h1>Quanto falta para a sua meta</h1><p class="muted">Organize gastos e veja o progresso de cada objetivo.</p>
<div class="cluster"><a class="btn btn--primary" href="/conta">Abrir conta</a><a class="btn btn--secondary" href="/como">Como funciona</a></div></div>
<div class="panel"><p>Viagem: 64% guardado</p><p class="muted">Faltam R$ 1.200</p></div></div></section>
<section class="section section--surface"><div class="container"><h2>Três passos</h2><div class="grid"><div class="stack"><h3>01 · Conecte</h3><p>Importe o extrato.</p></div><div class="stack"><h3>02 · Separe</h3><p>Defina metas por mês.</p></div><div class="stack"><h3>03 · Acompanhe</h3><p>Receba o resumo semanal.</p></div></div></div></section>
</main><footer class="container section"><p class="muted">Poupa+ 2026</p></footer></body></html>"#,
            contract::css(&t),
            base_css(&t)
        );
        std::fs::write(dir.path().join("index.html"), page).unwrap();
        let r = coturbo_design::review(dir.path(), &["index.html".into()]).await.unwrap();
        assert!(r.ok, "{}", r.summary);
    }
}

#[test]
fn feature_lists_inside_plans_are_not_cards() {
    let page = r#"<ul><li class="price-feature">a</li><li class="price-feature">b</li><li class="price-feature">c</li></ul>"#;
    assert!(fiscal::check("p.html", page, None).iter().all(|f| f.rule != "identical-cards"));
    let order = r#"<section id="hero"></section><section id="features"></section><section id="testimonials"></section><section id="pricing"></section><section id="faq"></section><section id="contact"></section>"#;
    let f = fiscal::check("p.html", order, None);
    assert!(f.iter().any(|x| x.rule == "template-sequence" && !x.blocking()), "the order is advice: {f:?}");
}

#[test]
fn the_design_system_is_read_from_the_project() {
    use coturbo_design::extract;
    let dir = tempfile::tempdir().unwrap();
    let w = |p: &str, t: &str| {
        let f = dir.path().join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    w("src/site.css", ":root { --brand: #ffd60a; --ink: #111111; }\nbody { font-family: 'Space Grotesk', sans-serif; color: #111111; background: #ffffff; }\n.btn { background: #ffd60a; border-radius: 6px; padding: 12px 24px; }\n.card { border-radius: 14px; padding: 24px; gap: 16px; }\n.chip { border-radius: 6px; padding: 4px 12px; }\nh1 { font-family: \"Fraunces\", serif; }\n.btn2 { background: #ffd60a; }");
    w("tailwind.config.js", "module.exports = { theme: { extend: { colors: { brand: '#ffd60a', night: '#0b0b0f' }, fontFamily: { display: ['Fraunces', 'serif'] } } } }");
    w("design/tokens.json", r##"{ "color": { "accent": "#ff5a36" } }"##);
    w("package.json", r#"{ "dependencies": { "tailwindcss": "3", "@radix-ui/react-dialog": "1" } }"#);
    w("node_modules/x/ignored.css", "a { color: #00ff00; }");
    let e = extract::scan(dir.path());
    assert_eq!(e.colors[0], "#ffd60a", "most used first: {:?}", e.colors);
    assert!(e.colors.contains(&"#ff5a36".to_string()), "token files count: {:?}", e.colors);
    assert!(!e.colors.contains(&"#00ff00".to_string()), "dependencies are not the project");
    assert!(e.fonts.contains(&"Space Grotesk".to_string()) && e.fonts.contains(&"Fraunces".to_string()), "{:?}", e.fonts);
    assert_eq!(e.radii[0], 6);
    assert!(e.radii.contains(&14));
    assert!(e.spacing.contains(&24) && e.spacing.contains(&12));
    assert!(e.tokens.iter().any(|(k, v)| k == "--brand" && v == "#ffd60a"));
    assert!(e.frameworks.contains(&"Tailwind CSS".to_string()) && e.frameworks.iter().any(|f| f.contains("Radix")));
    let s = extract::summary(&e);
    assert!(s.contains("fonts: ") && s.contains("Tailwind CSS"), "{s}");

    // A redesign keeps it: same fonts, same radii, the brand yellow.
    let mut b = brief("moderno");
    b.brand = e.brand();
    b.fonts = e.fonts.iter().take(2).cloned().collect();
    b.radii = e.radii.clone();
    let t = compile(&b);
    assert_eq!(t.color("primary"), (255, 214, 10));
    assert!(b.fonts.contains(&t.font_display));
    let radii: Vec<u32> = t.radius.iter().map(|(_, r)| *r).collect();
    assert!(radii.contains(&6) && radii.contains(&14), "{radii:?}");
}

#[test]
fn a_page_linking_the_kit_file_satisfies_the_static_rules() {
    use coturbo_design::{kit, with_linked_css};
    let t = compile(&brief("técnico"));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("DESIGN.md"), contract::markdown(&t)).unwrap();
    std::fs::write(dir.path().join("codeturbo-kit.css"), format!("{}\n{}", contract::css(&t), kit::base_css(&t))).unwrap();
    // A page with NO inline styles, only the external kit and tokens.
    let page = r#"<!DOCTYPE html><html lang="pt-BR"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><link rel="stylesheet" href="codeturbo-kit.css"><title>X</title></head>
<body><header class="container cluster"><strong>Nordlync</strong><a class="btn btn--secondary" href="/x">Entrar</a></header>
<main><section class="section reveal"><div class="container"><h1>Automação industrial</h1><p class="muted">Texto.</p>
<div class="grid"><div class="panel card"><h2>Um</h2><p>a</p></div><div class="panel card"><h2>Dois</h2><p>b</p></div></div></div></section></main></body></html>"#;
    std::fs::write(dir.path().join("index.html"), page).unwrap();
    let c = Contract::parse(&contract::markdown(&t));
    let merged = with_linked_css(dir.path(), "index.html", page);
    let f = fiscal::check("index.html", &merged, Some(&c));
    assert!(f.iter().all(|x| !x.blocking()), "the external kit satisfies focus/responsiveness: {f:?}");
    assert!(merged.contains(":focus-visible"), "the kit css was inlined for checking");
}

#[test]
fn design_md_follows_the_stitch_format_and_preview_renders() {
    let t = compile(&brief("editorial"));
    let md = contract::markdown(&t);
    for section in ["## Visual theme and atmosphere", "## Palette and roles", "## Do and don't", "| Token | Colour | Role |"] {
        assert!(md.contains(section), "falta secao {section}");
    }
    assert!(md.contains("| `--color-primary` |"), "tabela de paleta: {md}");
    let roles = contract::color_roles(&t);
    assert!(roles.iter().any(|(n, _, r)| *n == "ink" && r.contains("Main text")));
    let html = contract::preview_html(&t);
    assert!(html.starts_with("<!DOCTYPE html>") && html.contains("Colours and roles") && html.contains("btn--primary"), "preview");
    assert!(html.contains(":root") && html.contains("--color-primary"), "tokens inline no preview");
}

#[test]
fn the_probe_result_can_be_read_from_the_browsers_log() {
    // A real line of the browser's log, with the page's console output in it.
    let log = concat!(
        "[56564:47560:1016/100338.100:INFO:CONSOLE:1] \"outra mensagem da pagina\", source: file:///x.html (1)\n",
        "[56564:47560:1016/100338.245:INFO:CONSOLE:1] \"CT_PROBE:{\"rounded\":4,\"radii\":2,\"shadowed\":1,\"shadows\":1,",
        "\"centered\":3,\"text\":9,\"rows\":0,\"tiny\":0,\"overlaps\":[\"a.btn × b.btn\"],\"tight\":[],\"broken\":[],",
        "\"hscroll\":true,\"scrollers\":[],\"small\":0,\"cut\":[],\"edge\":[],\"under\":[]}\", source: file:///C:/x/index.html (1)\n",
    );
    let dom = dom_from_log(log).expect("the probe line is found among other console output");
    let m = parse_metrics(&dom).expect("and it reads like the attribute does");
    assert_eq!((m.rounded, m.radii, m.centered, m.text), (4, 2, 3, 9));
    assert!(m.hscroll);
    assert_eq!(m.overlaps, ["a.btn × b.btn"]);
    assert!(dom_from_log("nada a ver\n[INFO:CONSOLE:1] \"so uma mensagem\"").is_none());
    assert!(dom_from_log("").is_none());
}

#[test]
fn hallmark_gates_catch_the_machine_made_tells() {
    let page = r#"<!DOCTYPE html><html><head><style>
.hero { min-height: 100vh; background: linear-gradient(135deg, #7c3aed, #2563eb); }
.title { background-clip: text; -webkit-background-clip: text; }
.btn { transition: all 0.3s cubic-bezier(0.68, -0.55, 0.27, 1.55); }
.btn:focus-visible { outline: 2px solid #222; transition: outline-color 0.2s; }
.label { text-transform: uppercase; letter-spacing: 0.12em; font-size: 12px; }
.wide { width: 100vw; }
.orb { filter: blur(80px); }
.reveal { opacity: 0; }
article p { max-width: 95ch; }
.nav { position: sticky; top: 0; }
.section-head { position: sticky; top: 0; }
</style></head><body>
<section class="hero"><p class="eyebrow">New release</p><h1>Build <em>faster</em> than ever before today</h1>
<h2>The <span class="text-accent">only</span> platform you need</h2>
<p>Trusted by 10,000+ teams · Fast · Secure</p><a href="/x">Learn more →</a></section>
<video autoplay loop src="a.mp4"></video>
<div class="card"><div class="card">inner</div></div>
<div class="browser-bar"><span></span></div>
<svg viewBox="0 0 10 10"><circle r="4"/></svg>
<p>Jane Doe, a robust and effortless solution.</p>
<form id="f"><input type="email"><button>Join</button></form>
<script>f.addEventListener('submit', e => { e.preventDefault(); f.innerHTML = "Thanks! You're on the list"; });</script>
</body></html>"#;
    let f = fiscal::check("index.html", page, None);
    let rules: Vec<&str> = f.iter().map(|x| x.rule).collect();
    for r in [
        "ai-gradient", "gradient-text", "hero-vh", "transition-all", "bounce-easing", "focus-fade", "tracked-caps", "width-100vw", "blur-orb",
        "hidden-at-rest", "prose-measure", "double-sticky", "eyebrow", "italic-word", "accent-word", "arrow-link", "middot-meta", "invented-proof",
        "placeholder-name", "buzzwords", "fake-browser", "video-autoplay", "svg-unlabeled", "nested-card", "email-close", "fake-send",
    ] {
        assert!(rules.contains(&r), "{r} not caught: {rules:?}");
    }
    assert!(f.iter().any(|x| x.message.contains("Hallmark gate")));
    // Scoped under .js, the reveal is fine; a form that really sends is fine.
    let ok = page.replace(".reveal { opacity: 0; }", ".js .reveal { opacity: 0; }").replace("e.preventDefault();", "e.preventDefault(); fetch('/join', {method: 'POST'});");
    let rules: Vec<&str> = fiscal::check("index.html", &ok, None).iter().map(|x| x.rule).collect();
    assert!(!rules.contains(&"hidden-at-rest") && !rules.contains(&"fake-send"), "{rules:?}");
}

#[test]
fn the_palette_has_room_for_the_brand_and_the_page_must_use_it() {
    let mut with_brand = brief("editorial");
    with_brand.brand = vec!["#8b3a52".into(), "#c98b67".into()];
    for t in [compile(&brief("editorial")), compile(&brief("divertido")), compile(&with_brand)] {
        let (deep, on_deep, tint, bg) = (t.color("deep"), t.color("on-deep"), t.color("tint"), t.color("bg"));
        assert!(contrast(on_deep, deep) >= 4.5, "text on deep reads: {} on {}", hex(on_deep), hex(deep));
        assert!(contrast(deep, bg) >= 7.0, "deep is a real dark band: {}", hex(deep));
        assert!(contrast(t.color("ink"), tint) >= 4.5, "text on tint reads: {}", hex(tint));
        assert_ne!(tint, bg);
    }
    let md = contract::markdown(&compile(&brief("editorial")));
    assert!(md.contains("## Colour plan") && md.contains("## Copy") && md.contains("## Imagery"));

    let plain = Metrics { screens: 60, colour: 4, ..Default::default() };
    let page = Page { desktop: plain.clone(), mobile: plain.clone(), tablet: plain.clone(), laptop: plain };
    assert!(judge(&page).iter().any(|f| f.rule == "render-no-identity" && f.blocking()));
    let banded = Metrics { screens: 60, colour: 40, ..Default::default() };
    let page = Page { desktop: banded.clone(), mobile: banded.clone(), tablet: banded.clone(), laptop: banded };
    assert!(!judge(&page).iter().any(|f| f.rule == "render-no-identity"));
}

#[test]
fn a_call_to_action_that_fits_any_page_is_flagged() {
    let page = "<!doctype html><html lang=\"pt\"><body><a class=\"btn\" href=\"#x\">Saiba mais</a><button>Quero minha vaga</button></body></html>";
    let f = fiscal::check("index.html", page, None);
    let cta: Vec<_> = f.iter().filter(|f| f.rule == "generic-cta").collect();
    assert_eq!(cta.len(), 1, "{f:?}");
    assert!(cta[0].message.contains("Saiba mais"));
}

#[test]
fn an_invented_income_promise_counts_as_invented_proof() {
    let page = "<!doctype html><html lang=\"pt\"><body><p>Aprenda a faturar de R$ 3.000 a R$ 7.000 por mês.</p></body></html>";
    assert!(fiscal::check("index.html", page, None).iter().any(|f| f.rule == "invented-proof" && f.blocking()));
    for claim in ["Mais de 2.400 alunas formadas", "+2.400 alunas", "4.9 de 5 estrelas", "6.000 procedimentos"] {
        let page = format!("<!doctype html><html lang=\"pt\"><body><p>{claim}</p></body></html>");
        assert!(fiscal::check("index.html", &page, None).iter().any(|f| f.rule == "invented-proof"), "{claim}");
    }
    let honest = "<!doctype html><html lang=\"pt\"><body><p>35 aulas em vídeo e 8 módulos</p></body></html>";
    assert!(!fiscal::check("index.html", honest, None).iter().any(|f| f.rule == "invented-proof"));
}

#[test]
fn a_full_row_of_the_grid_runs_to_the_screen_edges() {
    use coturbo_design::grid::{Element, Kind, css};
    let els = vec![
        Element { id: "hero".into(), kind: Kind::Title, share: Some(1.0), break_before: false },
    ];
    let out = css(&els);
    assert!(out.contains(":where(#hero) { margin-inline: -16px; padding-inline: 16px; }"), "{out}");
}

#[test]
fn a_named_style_fixes_the_look_of_the_whole_page() {
    use coturbo_design::style::{ALL, Style};
    for st in ALL {
        let mut b = brief("editorial");
        b.style = st.name().into();
        let t = compile(&b);
        assert_eq!(t.style, st);
        let radii: Vec<u32> = t.radius.iter().map(|(_, r)| *r).collect();
        assert_eq!(radii, st.radii().to_vec(), "{}", st.name());
        assert!(st.fonts().iter().any(|(d, _)| *d == t.font_display), "{}: {}", st.name(), t.font_display);
        assert!(contrast(t.color("ink"), t.color("bg")) >= 7.0, "{}", st.name());
        let md = contract::markdown(&t);
        assert!(md.contains(&format!("## Style: {}", st.name())) && md.contains("## Motion") && md.contains("## Icons"), "{}", st.name());
        assert!(md.contains("gsap/3.12.5/ScrollTrigger.min.js") && md.contains("<svg class=\"icon\""));
        assert!(!md.to_lowercase().contains("emoji icons") || md.contains("never emoji"));
        let js = coturbo_design::kit::motion_js(&t);
        assert!(js.contains("registerPlugin") && js.contains("prefers-reduced-motion") && js.contains("Headless"));
        assert!(js.contains(st.motion().ease));
    }
    let mut b = brief("editorial");
    b.style = "cinematic".into();
    assert!(coturbo_design::color::luminance(compile(&b).color("bg")) < 0.1, "cinematic is dark");
    b.style = "brutalist".into();
    let t = compile(&b);
    assert!(coturbo_design::kit::base_css(&t).contains("border: 3px solid var(--color-ink)"));
    assert_eq!(Style::parse("Neo Brutalism"), Some(Style::NeoBrutalist));
    assert_eq!(Style::parse("suíço"), Some(Style::Swiss));
}

#[test]
fn single_weight_fonts_are_linked_without_weights() {
    let mut b = brief("editorial");
    b.fonts = vec!["Archivo Black".into(), "JetBrains Mono".into()];
    let link = contract::font_link(&compile(&b));
    assert!(link.contains("family=Archivo+Black&") && link.contains("family=JetBrains+Mono:wght@400;600;700"), "{link}");
}

#[test]
fn scrollbars_are_invisible() {
    let t = coturbo_design::compiler::compile(&coturbo_design::compiler::Brief::default());
    let css = coturbo_design::kit::base_css(&t);
    assert!(css.contains("scrollbar-width: none"));
    assert!(css.contains("::-webkit-scrollbar { display: none"));
}
