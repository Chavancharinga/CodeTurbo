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
    let dom = r#"<html data-codeturbo="{&quot;rounded&quot;:9,&quot;radii&quot;:1,&quot;shadowed&quot;:5,&quot;shadows&quot;:1,&quot;centered&quot;:9,&quot;text&quot;:10,&quot;rows&quot;:3,&quot;overlaps&quot;:[&quot;div.card × img&quot;],&quot;tight&quot;:[&quot;a.logo | a&quot;],&quot;broken&quot;:[&quot;img&quot;],&quot;hscroll&quot;:true,&quot;scrollers&quot;:[&quot;div.box&quot;],&quot;small&quot;:4,&quot;tiny&quot;:0,&quot;cut&quot;:[&quot;h1.title&quot;]}"><body></body></html>"#;
    let m = parse_metrics(dom).unwrap();
    assert_eq!((m.rounded, m.radii, m.rows, m.small), (9, 1, 3, 4));
    assert_eq!(m.overlaps, ["div.card × img"]);
    assert!(m.hscroll);
    let page = Page { desktop: m.clone(), mobile: m, ..Page::default() };
    let rules: Vec<&str> = judge(&page).iter().map(|f| f.rule).collect();
    for r in [
        "render-sobreposicao", "render-colados", "render-conteudo-cortado", "render-barra-de-rolagem", "render-imagem-quebrada",
        "render-sem-responsividade", "render-alvo-pequeno", "render-grades-iguais", "render-raio-unico",
        "render-centralizado", "render-sombra-unica",
    ] {
        assert!(rules.contains(&r), "missing {r}: {rules:?}");
    }
    let clean = Metrics { rounded: 9, radii: 3, text: 10, centered: 2, ..Metrics::default() };
    assert!(judge(&Page { desktop: clean.clone(), mobile: clean, ..Page::default() }).is_empty());
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
    assert!(r.summary.starts_with("✗ design refused"), "{}", r.summary);
    if coturbo_design::render::find_browser().is_some() {
        assert!(r.summary.contains("rendered page"), "{}", r.summary);
        assert!(dir.path().join(".codeturbo/design/index.png").exists(), "desktop screenshot saved");
        assert!(dir.path().join(".codeturbo/design/index-celular.png").exists(), "phone screenshot saved");
        assert!(dir.path().join(".codeturbo/design/index-tablet.png").exists(), "tablet screenshot saved");
        assert!(dir.path().join(".codeturbo/design/index-notebook.png").exists(), "laptop screenshot saved");
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
    assert!(fiscal::check("p.html", page, None).iter().all(|f| f.rule != "cartoes-identicos"));
    let order = r#"<section id="hero"></section><section id="features"></section><section id="testimonials"></section><section id="pricing"></section><section id="faq"></section><section id="contact"></section>"#;
    let f = fiscal::check("p.html", order, None);
    assert!(f.iter().any(|x| x.rule == "sequencia-template" && !x.blocking()), "the order is advice: {f:?}");
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
    assert!(s.contains("fontes: ") && s.contains("Tailwind CSS"), "{s}");

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
    for section in ["## Tema visual e atmosfera", "## Paleta e papéis", "## Faça e não faça", "| Token | Cor | Papel |"] {
        assert!(md.contains(section), "falta secao {section}");
    }
    assert!(md.contains("| `--color-primary` |"), "tabela de paleta: {md}");
    let roles = contract::color_roles(&t);
    assert!(roles.iter().any(|(n, _, r)| *n == "ink" && r.contains("Texto principal")));
    let html = contract::preview_html(&t);
    assert!(html.starts_with("<!DOCTYPE html>") && html.contains("Cores e papéis") && html.contains("btn--primary"), "preview");
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
