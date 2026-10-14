use coturbo_design::color::{contrast, hex, hue_sat, oklch, parse_hex};
use coturbo_design::compiler::{Brief, Tone, compile};
use coturbo_design::contract::{self, Contract};
use coturbo_design::fiscal::{self, Severity};
use coturbo_design::render::{Metrics, Page, judge, parse_metrics};

fn brief(tone: &str) -> Brief {
    Brief {
        subject: "aplicativo de finanças pessoais Poupa+".into(),
        audience: "jovens adultos no primeiro emprego".into(),
        tone: tone.into(),
        signature: "um cofre desenhado com linhas finas que enche conforme a meta".into(),
        brand: Vec::new(),
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
        assert_eq!(radii.len(), 3, "{tone}: three distinct radii");
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
    assert!(md.contains("Elemento de assinatura: um cofre"));
    assert!(md.contains("fonts.googleapis.com/css2?family="));
    let c = Contract::parse(&md);
    assert_eq!(c.colors().len(), 8);
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
        "fonte-padrao",
        "paleta-generica",
        "gradiente-generico",
        "contraste",
        "raio-unico",
        "emoji-icone",
        "cartoes-identicos",
        "sequencia-template",
        "titulos",
        "foco-ausente",
        "sem-contrato",
        "hover-elevacao",
        "tudo-centralizado",
        "movimento",
    ] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}
{}", fiscal::render("index.html", &f));
    }
    assert!(f.iter().any(|x| x.rule == "contraste" && x.severity == Severity::P0));
    let report = fiscal::render("index.html", &f);
    assert!(report.starts_with("design index.html: "), "{report}");
    assert!(report.contains("index.html:2 [P1 fonte-padrao]"), "{report}");
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
    assert!(f.iter().any(|x| x.rule == "cor-fora-do-contrato"), "{f:?}");
}

#[test]
fn only_interface_files_are_inspected() {
    assert!(fiscal::is_ui_file("src/App.tsx"));
    assert!(fiscal::is_ui_file("index.HTML"));
    assert!(!fiscal::is_ui_file("src/main.rs"));
    assert!(fiscal::check("style.css", "a:hover { color: red; }", None).iter().all(|f| f.rule != "foco-ausente"));
}

#[test]
fn rendered_measurements_are_judged() {
    let dom = r#"<html data-codeturbo="{&quot;rounded&quot;:9,&quot;radii&quot;:1,&quot;shadowed&quot;:5,&quot;shadows&quot;:1,&quot;centered&quot;:9,&quot;text&quot;:10,&quot;rows&quot;:3,&quot;overlaps&quot;:[&quot;div.card × img&quot;],&quot;tight&quot;:[&quot;a.logo | a&quot;],&quot;broken&quot;:[&quot;img&quot;],&quot;hscroll&quot;:true,&quot;scrollers&quot;:[&quot;div.box&quot;],&quot;small&quot;:4,&quot;tiny&quot;:0}"><body></body></html>"#;
    let m = parse_metrics(dom).unwrap();
    assert_eq!((m.rounded, m.radii, m.rows, m.small), (9, 1, 3, 4));
    assert_eq!(m.overlaps, ["div.card × img"]);
    assert!(m.hscroll);
    let page = Page { desktop: m.clone(), mobile: m };
    let rules: Vec<&str> = judge(&page).iter().map(|f| f.rule).collect();
    for r in [
        "render-sobreposicao", "render-colados", "render-barra-de-rolagem", "render-imagem-quebrada",
        "render-sem-responsividade", "render-alvo-pequeno", "render-grades-iguais", "render-raio-unico",
        "render-centralizado", "render-sombra-unica",
    ] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}");
    }
    let clean = Metrics { rounded: 9, radii: 3, text: 10, centered: 2, ..Metrics::default() };
    assert!(judge(&Page { desktop: clean.clone(), mobile: clean }).is_empty());
    assert!(parse_metrics("<html>").is_none());
}

#[tokio::test]
async fn review_renders_in_the_installed_browser() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), GENERIC).unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();
    assert!(coturbo_design::review(dir.path(), &["main.rs".into()]).await.is_none());
    let r = coturbo_design::review(dir.path(), &["index.html".into()]).await.unwrap();
    assert!(!r.ok);
    assert!(r.summary.starts_with("✗ design recusado"), "{}", r.summary);
    if coturbo_design::render::find_browser().is_some() {
        assert!(r.summary.contains("página renderizada"), "{}", r.summary);
        assert!(dir.path().join(".codeturbo/design/index.png").exists(), "desktop screenshot saved");
        assert!(dir.path().join(".codeturbo/design/index-celular.png").exists(), "phone screenshot saved");
    }
}

#[test]
fn repeated_accordion_items_are_not_cards() {
    let page = r#"<div class="faq-item">a</div><div class="faq-item">b</div><div class="faq-item">c</div>"#;
    assert!(fiscal::check("p.html", page, None).iter().all(|f| f.rule != "cartoes-identicos"));
    let cards = page.replace("faq-item", "price-card");
    assert!(fiscal::check("p.html", &cards, None).iter().any(|f| f.rule == "cartoes-identicos"));
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
        "vidro-fosco", "outline-removido", "texto-pequeno", "entrelinha", "texto-justificado", "z-index", "important",
        "sem-responsividade", "texto-de-preenchimento", "imagem-de-preenchimento", "links-vazios", "texto-cliche",
    ] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}");
    }
    let fixed = page.replace(".grid {", "@media (max-width: 700px) { .grid { grid-template-columns: 1fr; } }\n.grid {");
    assert!(!fiscal::check("p.html", &fixed, None).iter().any(|f| f.rule == "sem-responsividade"));
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
    assert!(contract::markdown(&t).contains("Cores da marca preservadas: #ffd60a"));
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
