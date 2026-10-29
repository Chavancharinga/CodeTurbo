//! Hallmark's static gates, natively: the tells that make a page read as
//! machine-made and the floors no designed page breaks, checked in the CSS
//! and markup after every edit. Ported from the gate list of Hallmark
//! (github.com/Nutlope/hallmark, MIT); each finding names its gate number.

use crate::color::{hue_sat, parse_hex};
use crate::fiscal::{Finding, Severity};
use regex::Regex;
use std::sync::LazyLock;

fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

static RULE: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)([^{}]+)\{([^{}]*)\}"));
static DECL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(-?-?[a-zA-Z][a-zA-Z0-9-]*)\s*:\s*([^;{}]+)(?:;|$)"));
static HEX: LazyLock<Regex> = LazyLock::new(|| re(r"#[0-9a-fA-F]{6}\b|#[0-9a-fA-F]{3}\b"));
static BEZIER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"cubic-bezier\(\s*([-\d.]+)\s*,\s*([-\d.]+)\s*,\s*([-\d.]+)\s*,\s*([-\d.]+)\s*\)")
});
static BLUR: LazyLock<Regex> = LazyLock::new(|| re(r"blur\(\s*(\d+)px"));
static TAG: LazyLock<Regex> = LazyLock::new(|| re(r"<(/?)([a-zA-Z][a-zA-Z0-9-]*)([^>]*?)(/?)>"));
static CLASS_ATTR: LazyLock<Regex> =
    LazyLock::new(|| re(r#"\bclass(?:Name)?\s*=\s*["']([^"']*)["']"#));
static EYEBROW: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?is)<(p|span|div|small)\b[^>]*\bclass(?:Name)?\s*=\s*["'][^"']*\b(eyebrow|kicker|overline|pre-?title|pre-?heading|section-label|tagline|label|badge|pill|tag)\b[^"']*["'][^>]*>\s*([^<]{1,48}?)\s*</(?:p|span|div|small)>\s*<h[1-3]\b"#,
    )
});
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<h([1-3])\b[^>]*>(.*?)</h[1-3]>"));
static ITALIC_IN: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?is)<(em|i)\b[^>]*>[^<]{1,30}</(?:em|i)>"));
static ACCENT_IN: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?is)<span\b[^>]*\bclass(?:Name)?\s*=\s*["'][^"']*\b(accent|highlight|gradient|text-primary|text-accent|colored|brand|marker)[^"']*["']"#,
    )
});
static ARROW_LINK: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?s)>([^<>]{2,40}?)\s*(→|->|›|»)\s*</(a|button)>"));
static CTA: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?is)<(a|button)\b[^>]*>\s*([^<]{2,40}?)\s*</(?:a|button)>"));
/// Link and button texts that could sit on any page.
const GENERIC_CTA: &[&str] = &[
    "saiba mais",
    "clique aqui",
    "leia mais",
    "ver mais",
    "enviar",
    "learn more",
    "click here",
    "read more",
    "submit",
    "more info",
    "mais informações",
];
static MIDDOTS: LazyLock<Regex> = LazyLock::new(|| re(r">[^<>]*\S\s+·\s+[^<>]*\S\s+·\s+\S[^<>]*<"));
static PROOF: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?i)trusted by|loved by|★★★★★|\b4\.[89]\s*/\s*5\b|\b\d[\d,.]*\s*(k|m)?\+\s*(happy\s+)?(users|customers|teams|companies|developers|businesses|clientes|empresas|usuários)|fatur\w*\s+(de\s+|até\s+)?r\$|ganh\w*\s+(até\s+)?r\$|earn\s+(up\s+to\s+)?\$\d|\b\d[\d.,]*\s*(k|mil)?\+?\s*(alunas|alunos|students|clientes atendidas|procedimentos)\b|\b4[.,][5-9]\s*(de\s*5|/\s*5|★|estrelas|stars)|\$[\d,.]+k?\s*(/|per\s+)month",
    )
});
static CHROME: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?i)class(?:Name)?\s*=\s*["'][^"']*\b(traffic-lights?|window-dots|browser-(bar|chrome|header|top)|mac-?dots|window-controls|titlebar-dots)\b"#,
    )
});
static VIDEO: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<video\b([^>]*)>"));
static IMG: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<img\b([^>]*)>"));
static SVG: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<svg\b([^>]*)>(.*?)</svg>"));
static FORM: LazyLock<Regex> = LazyLock::new(|| re(r"(?is)<form\b[^>]*>(.*?)</form>"));
static THANKS: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r#"(?i)["'`>][^"'`<]{0,30}\b(thanks|thank you|you're on the list|you are on the list|sent!|subscribed|reserved|obrigad[oa]|enviado)\b"#,
    )
});

const PLACEHOLDER_NAMES: [&str; 6] = [
    "jane doe",
    "john doe",
    "john smith",
    "acme",
    "lorem corp",
    "example inc",
];
const BUZZWORDS: [&str; 14] = [
    "robust",
    "empower",
    "effortless",
    "unleash",
    "next-generation",
    "next generation",
    "world-class",
    "best-in-class",
    "cutting-edge",
    "game-changer",
    "game changer",
    "revolutionize",
    "harness the power",
    "in today's fast-paced",
];
const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr", "path",
];

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// `letter-spacing` in em, when it can be told (px over a 16px base).
fn tracking_em(v: &str) -> Option<f64> {
    let v = v.trim();
    if let Some(n) = v.strip_suffix("em") {
        return n.trim().parse().ok();
    }
    v.strip_suffix("px")
        .and_then(|n| n.trim().parse::<f64>().ok())
        .map(|px| px / 16.0)
}

fn px(v: &str) -> Option<f64> {
    v.trim()
        .strip_suffix("px")
        .and_then(|n| n.trim().parse().ok())
}

/// Hue family of a saturated colour, for the gradient ban.
fn family(hex: &str) -> Option<&'static str> {
    let (h, s) = hue_sat(parse_hex(hex)?);
    if s < 0.35 {
        return None;
    }
    Some(match h {
        h if (165.0..200.0).contains(&h) => "cyan",
        h if (200.0..255.0).contains(&h) => "blue",
        h if (255.0..300.0).contains(&h) => "purple",
        h if (300.0..345.0).contains(&h) => "pink",
        _ => "other",
    })
}

/// The CSS of the file: `(offset in the file, css)` pieces.
pub(crate) fn check_css(text: &str, regions: &[(usize, String)], out: &mut Vec<Finding>) {
    let mut add = |severity, rule, line, message: String| {
        out.push(Finding {
            severity,
            rule,
            line,
            message,
        })
    };
    let starting_style = regions.iter().any(|(_, c)| c.contains("@starting-style"));
    let mut sticky_top = Vec::new();
    for (start, css) in regions {
        for rule in RULE.captures_iter(css) {
            let (sel, body) = (rule.get(1).expect("group"), rule.get(2).expect("group"));
            let selector = sel
                .as_str()
                .trim()
                .rsplit(['}', ';'])
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if selector.starts_with(":root") || selector.starts_with('@') {
                continue;
            }
            let line = line_of(text, start + body.start());
            let decls: Vec<(String, String)> = DECL
                .captures_iter(body.as_str())
                .map(|d| (d[1].to_ascii_lowercase(), d[2].trim().to_lowercase()))
                .collect();
            let get = |p: &str| decls.iter().find(|(k, _)| k == p).map(|(_, v)| v.as_str());
            for (prop, value) in &decls {
                match prop.as_str() {
                    "background" | "background-image" if value.contains("gradient(") => {
                        let fams: Vec<&str> = HEX
                            .find_iter(value)
                            .filter_map(|m| family(m.as_str()))
                            .collect();
                        let has = |f: &str| fams.contains(&f);
                        if has("purple") && (has("blue") || has("pink")) {
                            add(Severity::P1, "ai-gradient", line, "purple-to-blue or purple-to-pink gradient, the signature of a generated page (Hallmark gate 2a): one anchor hue, solid ink".into());
                        }
                    }
                    "background-clip" | "-webkit-background-clip" if value.contains("text") => {
                        add(Severity::P2, "gradient-text", line, "gradient text (background-clip: text) (Hallmark gate 2b): solid ink, with weight or a display face".into());
                    }
                    "height" | "min-height"
                        if selector.contains("hero")
                            && value.contains("vh")
                            && !value.contains("dvh")
                            && !value.contains("svh")
                            && !value.contains("lvh") =>
                    {
                        add(
                            Severity::P2,
                            "hero-vh",
                            line,
                            format!(
                                "hero sized in vh (`{value}`) jumps with the phone's toolbar (Hallmark gate 6a): use dvh or svh"
                            ),
                        );
                    }
                    "transition" | "transition-property" => {
                        if value.split([',', ' ']).any(|w| w == "all") || value == "all" {
                            add(Severity::P1, "transition-all", line, "`transition: all` animates layout by accident (Hallmark gate 10a): list the properties, prefer transform and opacity".into());
                        } else if value
                            .split([',', ' '])
                            .any(|w| matches!(w, "width" | "height" | "top" | "left"))
                        {
                            add(Severity::P2, "layout-animation", line, "animating width, height, top or left repaints the page (Hallmark gate 10a): animate transform".into());
                        }
                        if selector.contains(":focus") {
                            add(Severity::P1, "focus-fade", line, "a focus ring that fades in (Hallmark gate 15): show focus instantly".into());
                        }
                    }
                    "width" | "min-width" | "max-width" if value.contains("100vw") => {
                        add(Severity::P1, "width-100vw", line, "`100vw` includes the scrollbar and scrolls the page sideways (Hallmark gate 34): use width: 100%".into());
                    }
                    "filter" => {
                        if let Some(n) = BLUR.captures(value).and_then(|c| c[1].parse::<u32>().ok())
                            && n >= 40
                        {
                            add(
                                Severity::P2,
                                "blur-orb",
                                line,
                                format!(
                                    "blur({n}px) glow behind the content, the floating orb of generated pages (Hallmark gate 29)"
                                ),
                            );
                        }
                    }
                    _ => {}
                }
                if let Some(c) = BEZIER.captures(value) {
                    let y1: f64 = c[2].parse().unwrap_or(0.0);
                    let y2: f64 = c[4].parse().unwrap_or(0.0);
                    if !(0.0..=1.0).contains(&y1) || !(0.0..=1.0).contains(&y2) {
                        add(Severity::P2, "bounce-easing", line, "overshooting easing on interface state (Hallmark gate 12): exponential ease-out".into());
                    }
                }
            }
            let upper = get("text-transform") == Some("uppercase")
                || get("font-variant-caps").is_some_and(|v| v.contains("small-caps"));
            if upper {
                let small = get("font-size").and_then(px).is_none_or(|s| s <= 15.0);
                if small
                    && get("letter-spacing")
                        .and_then(tracking_em)
                        .is_some_and(|e| e > 0.03)
                    && !selector.contains("logo")
                    && !selector.contains("wordmark")
                {
                    add(
                        Severity::P2,
                        "tracked-caps",
                        line,
                        format!(
                            "tracked capitals on small text in `{selector}` (Hallmark gate 67): sentence case, no tracking"
                        ),
                    );
                }
                if let Some(lh) = get("line-height").and_then(|v| v.parse::<f64>().ok())
                    && lh < 1.0
                {
                    add(
                        Severity::P2,
                        "caps-leading",
                        line,
                        format!(
                            "uppercase head at line-height {lh}: wrapped lines collide (Hallmark gate 55a); 1.02 to 1.08"
                        ),
                    );
                }
            }
            if get("position") == Some("sticky")
                && get("top").is_some_and(|t| t == "0" || t == "0px")
            {
                sticky_top.push(line);
            }
            let prose = selector.split(',').any(|s| {
                let s = s.trim();
                s == "p" || s.ends_with(" p") || s.contains("prose") || s.contains("article")
            });
            if prose
                && let Some(ch) = get("max-width")
                    .and_then(|v| v.strip_suffix("ch"))
                    .and_then(|n| n.trim().parse::<f64>().ok())
                && !(45.0..=75.0).contains(&ch)
            {
                add(
                    Severity::P2,
                    "prose-measure",
                    line,
                    format!(
                        "running text {ch}ch wide (Hallmark gate 25): 45 to 75ch, 65 is a good default"
                    ),
                );
            }
            let reveal = [
                "reveal",
                "fade",
                "animate",
                "aos",
                "appear",
                "in-view",
                "inview",
                "on-scroll",
                "scroll-in",
            ]
            .iter()
            .any(|k| selector.contains(k));
            let hidden = get("opacity").is_some_and(|o| o == "0" || o == "0.0")
                || get("visibility") == Some("hidden");
            let scoped = selector.starts_with(".js")
                || selector.starts_with("html.js")
                || selector.contains("[data-js")
                || selector.contains(".is-")
                || selector.contains(".hidden");
            if reveal
                && hidden
                && !scoped
                && !starting_style
                && get("animation").is_none()
                && !selector.contains(":not(")
            {
                add(
                    Severity::P1,
                    "hidden-at-rest",
                    line,
                    format!(
                        "`{selector}` hides content until a script reveals it (Hallmark gate 62): without JavaScript the page is blank; scope it under `.js`, or use @starting-style"
                    ),
                );
            }
        }
    }
    if sticky_top.len() >= 2 {
        add(Severity::P2, "double-sticky", sticky_top[1], "two `position: sticky; top: 0` elements slide under each other (Hallmark gate 56): offset the second by the bar's height".into());
    }
}

/// Markup, with scripts and styles blanked (offsets kept).
pub(crate) fn check_markup(text: &str, markup: &str, out: &mut Vec<Finding>) {
    let mut add = |severity, rule, line, message: String| {
        out.push(Finding {
            severity,
            rule,
            line,
            message,
        })
    };
    let lower = markup.to_lowercase();

    if let Some(c) = EYEBROW.captures(markup)
        && c[3].split_whitespace().count() <= 5
    {
        let m = c.get(0).expect("match");
        add(
            Severity::P2,
            "eyebrow",
            line_of(text, m.start()),
            format!(
                "\"{}\" announces the heading below it, the eyebrow of every generated page (Hallmark gate 54): let the heading speak",
                c[3].trim()
            ),
        );
    }
    for h in HEADING.captures_iter(markup) {
        let inner = &h[2];
        let line = line_of(text, h.get(0).expect("match").start());
        let plain_len = inner
            .split('<')
            .map(|s| s.split_once('>').map_or(s, |(_, t)| t))
            .collect::<String>()
            .trim()
            .len();
        if let Some(i) = ITALIC_IN.find(inner)
            && plain_len > i.as_str().len() / 2 + 4
        {
            add(Severity::P2, "italic-word", line, "one italic word inside a roman heading (Hallmark gate 38i): set the whole head one way".into());
        }
        if &h[1] != "3" && ACCENT_IN.is_match(inner) {
            add(Severity::P2, "accent-word", line, "one word of the heading set apart in colour or a highlight (Hallmark gate 38iii): the whole head in one ink".into());
        }
    }
    if let Some(m) = ARROW_LINK.find(markup) {
        add(Severity::P2, "arrow-link", line_of(text, m.start()), "an arrow glued to link or button text, as in `Learn more →` (Hallmark gate 63): the words alone".into());
    }
    if let Some(c) = CTA
        .captures_iter(markup)
        .find(|c| GENERIC_CTA.contains(&c[2].trim().to_lowercase().as_str()))
    {
        add(
            Severity::P2,
            "generic-cta",
            line_of(text, c.get(0).expect("match").start()),
            format!(
                "\"{}\" says nothing about what the reader gets: name the result with a verb (DESIGN.md, copy)",
                c[2].trim()
            ),
        );
    }
    if let Some(m) = MIDDOTS.find(markup) {
        add(
            Severity::P2,
            "middot-meta",
            line_of(text, m.start()),
            "meta joined with middle dots (`A · B · C`), template punctuation (Hallmark gate 63)"
                .into(),
        );
    }
    if let Some(m) = PROOF.find(markup) {
        add(
            Severity::P1,
            "invented-proof",
            line_of(text, m.start()),
            format!(
                "\"{}\": a metric, rating, income or student count must be the user's real one (Hallmark gate 46a); the user gave none, so leave it out or write what the offer includes instead",
                m.as_str().trim()
            ),
        );
    }
    if let Some((i, name)) = PLACEHOLDER_NAMES
        .iter()
        .filter_map(|n| lower.find(n).map(|i| (i, *n)))
        .min()
    {
        add(
            Severity::P1,
            "placeholder-name",
            line_of(text, i),
            format!(
                "placeholder name \"{name}\" (Hallmark gate 19): use the user's real names or none"
            ),
        );
    }
    let words: Vec<&str> = BUZZWORDS
        .iter()
        .copied()
        .filter(|w| lower.contains(w))
        .collect();
    if !words.is_empty() {
        add(
            Severity::P2,
            "buzzwords",
            1,
            format!(
                "buzzwords: {} (Hallmark gate 19): say what it does",
                words.join(", ")
            ),
        );
    }
    if let Some(m) = CHROME.find(markup) {
        add(Severity::P2, "fake-browser", line_of(text, m.start()), "a hand-drawn browser or window bar (Hallmark gate 47a): a real screenshot in a <figure>, or no chrome".into());
    }
    for v in VIDEO.captures_iter(markup) {
        let attrs = v[1].to_lowercase();
        if attrs.contains("autoplay") && !attrs.contains("muted") {
            add(Severity::P1, "video-autoplay", line_of(text, v.get(0).expect("match").start()), "a video plays itself with sound (Hallmark gate 28a): autoplay muted loop playsinline".into());
        }
    }
    if let Some(first) = IMG.captures(markup)
        && first[1].to_lowercase().contains("loading=\"lazy\"")
    {
        add(Severity::P2, "lazy-hero", line_of(text, first.get(0).expect("match").start()), "the first image is lazy-loaded, so the hero paints late (Hallmark gate 28a): fetchpriority=\"high\" instead".into());
    }
    let unlabeled: Vec<usize> = SVG
        .captures_iter(markup)
        .filter(|s| {
            let a = s[1].to_lowercase();
            !a.contains("aria-hidden")
                && !a.contains("aria-label")
                && !a.contains("aria-labelledby")
                && !s[2].to_lowercase().contains("<title")
        })
        .map(|s| line_of(text, s.get(0).expect("match").start()))
        .collect();
    if let Some(l) = unlabeled.first() {
        add(
            Severity::P2,
            "svg-unlabeled",
            *l,
            format!(
                "{} <svg> with neither aria-hidden=\"true\" nor a label (Hallmark gate 33): hide decoration, name what informs",
                unlabeled.len()
            ),
        );
    }
    // A card inside a card.
    let mut stack: Vec<(String, bool)> = Vec::new();
    let mut nested = None;
    for t in TAG.captures_iter(markup) {
        let name = t[2].to_ascii_lowercase();
        if VOID.contains(&name.as_str()) || &t[4] == "/" {
            continue;
        }
        if &t[1] == "/" {
            if let Some(p) = stack.iter().rposition(|(n, _)| *n == name) {
                stack.truncate(p);
            }
            continue;
        }
        let card = CLASS_ATTR.captures(&t[3]).is_some_and(|c| {
            c[1].split_whitespace().any(|k| {
                k == "card" || k.ends_with("-card") || k.starts_with("card-") && !k.contains("__")
            })
        });
        if card && stack.iter().any(|(_, c)| *c) {
            nested.get_or_insert(line_of(text, t.get(0).expect("match").start()));
        }
        stack.push((name, card));
    }
    if let Some(l) = nested {
        add(
            Severity::P2,
            "nested-card",
            l,
            "a card inside a card (Hallmark gate 4): one layer of containment".into(),
        );
    }
    // The email sign-up closing the page.
    if let Some(f) = FORM.captures_iter(markup).last() {
        let body = f[1].to_lowercase();
        let after = &lower[f.get(0).expect("match").end()..];
        let inputs = body.matches("<input").count();
        let closes = !after.contains("<section") && !after.contains("<h2");
        if closes && body.contains("type=\"email\"") && inputs <= 2 && !lower.contains("newsletter")
        {
            add(Severity::P2, "email-close", line_of(text, f.get(0).expect("match").start()), "the page ends on an email sign-up, the stock close (Hallmark gate 85): end on the page's own action".into());
        }
    }
}

/// A form that says it worked without sending anything (Hallmark gate 70).
pub(crate) fn check_script(text: &str, out: &mut Vec<Finding>) {
    let lower = text.to_lowercase();
    if !lower.contains("<form") || !lower.contains("preventdefault") {
        return;
    }
    let sends = [
        "fetch(",
        "xmlhttprequest",
        "axios",
        "sendbeacon",
        "$.post",
        "$.ajax",
        "action=\"http",
        "action=\"/",
        "supabase",
        "firebase",
        "formspree",
        "netlify",
    ]
    .iter()
    .any(|k| lower.contains(k));
    if !sends && let Some(m) = THANKS.find(text) {
        out.push(Finding {
            severity: Severity::P1,
            rule: "fake-send",
            line: line_of(text, m.start()),
            message: "the form prints success but sends nothing (Hallmark gate 70): send it to a real endpoint, or say plainly it is a demo".into(),
        });
    }
}
