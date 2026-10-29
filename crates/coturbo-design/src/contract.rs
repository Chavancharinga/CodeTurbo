//! The design contract: the compiled tokens written to `DESIGN.md`, with
//! the CSS custom properties the code must use, and read back for checks.

use crate::color::{Rgb, hex, parse_hex};
use crate::compiler::Tokens;
use std::collections::BTreeMap;
use std::path::Path;

pub const FILE: &str = "DESIGN.md";

fn font_stack(name: &str, display: bool, technical: bool) -> String {
    let generic = if name.contains("Mono") {
        "monospace"
    } else if display && !technical && (name.contains("Serif") || ["Fraunces", "Newsreader", "Cormorant Garamond", "Libre Caslon Display", "Gloock", "Playfair Display", "Young Serif"].contains(&name)) {
        "serif"
    } else {
        "sans-serif"
    };
    format!("\"{name}\", {generic}")
}

/// The `:root { ... }` block with every token.
pub fn css(t: &Tokens) -> String {
    let technical = t.tone == crate::compiler::Tone::Technical;
    let mut s = String::from(":root {\n");
    for (n, c) in &t.colors {
        s.push_str(&format!("  --color-{n}: {};\n", hex(*c)));
    }
    s.push_str(&format!("  --font-display: {};\n", font_stack(&t.font_display, true, technical)));
    s.push_str(&format!("  --font-body: {};\n", font_stack(&t.font_body, false, technical)));
    for (n, rem) in &t.type_scale {
        s.push_str(&format!("  --text-{n}: {rem}rem;\n"));
    }
    for (i, px) in t.space.iter().enumerate() {
        s.push_str(&format!("  --space-{}: {px}px;\n", i + 1));
    }
    // Semantic aliases so the model's natural names (--space-md, --space-xl)
    // resolve too, instead of collapsing to an undefined value.
    let at = |i: usize| t.space.get(i).copied().unwrap_or(16);
    for (name, px) in [
        ("xs", at(1)),
        ("sm", at(2)),
        ("s", at(2)),
        ("md", at(3)),
        ("m", at(3)),
        ("lg", at(5)),
        ("l", at(5)),
        ("xl", at(6)),
        ("2xl", at(7)),
        ("3xl", at(8)),
    ] {
        s.push_str(&format!("  --space-{name}: {px}px;\n"));
    }
    for (n, px) in &t.radius {
        s.push_str(&format!("  --radius-{n}: {px}px;\n"));
    }
    for (n, v) in &t.shadows {
        s.push_str(&format!("  --shadow-{n}: {v};\n"));
    }
    s.push('}');
    s
}

/// `<link>` tags loading the two fonts.
pub fn font_link(t: &Tokens) -> String {
    // Fonts cut in a single weight: asking Google Fonts for others fails the whole request.
    const SINGLE: [&str; 8] = ["Archivo Black", "Anton", "Bebas Neue", "DM Serif Display", "Gloock", "Young Serif", "Instrument Serif", "Libre Caslon Display"];
    let fam = |f: &str| {
        let name = f.replace(' ', "+");
        if SINGLE.contains(&f) { format!("family={name}") } else { format!("family={name}:wght@400;600;700") }
    };
    format!(
        "<link rel=\"preconnect\" href=\"https://fonts.googleapis.com\">\n<link href=\"https://fonts.googleapis.com/css2?{}&{}&display=swap\" rel=\"stylesheet\">",
        fam(&t.font_display),
        fam(&t.font_body)
    )
}

/// The contract file.
/// Semantic colour roles for the Stitch-format palette table.
pub fn color_roles(t: &Tokens) -> Vec<(&'static str, String, &'static str)> {
    let role = |n: &str| match n {
        "bg" => "Page background",
        "surface" => "Raised surfaces (cards, panels)",
        "ink" => "Main text",
        "muted" => "Secondary text",
        "line" => "Borders and dividers",
        "primary" => "Brand colour; main actions and fills",
        "on-primary" => "Text/icon on the primary colour",
        "primary-ink" => "The brand colour used as readable text",
        "accent" => "Secondary highlight",
        "deep" => "Bold sections in the brand (opening, highlight band, closing call)",
        "on-deep" => "Text on the deep colour",
        "tint" => "Light brand bands that alternate with the page",
        n if n.starts_with("gray") => "Neutral scale for hierarchy",
        _ => "",
    };
    t.colors.iter().map(|(n, c)| (*n, hex(*c), role(n))).collect()
}

/// A visual catalogue of the design system (swatches, type scale, buttons,
/// cards) the user can open in a browser, like the preview.html of a Stitch
/// DESIGN.md.
pub fn preview_html(t: &Tokens) -> String {
    let swatches: String = color_roles(t)
        .iter()
        .map(|(n, hx, role)| {
            format!(
                "<div class=\"panel\" style=\"display:flex;gap:var(--space-4);align-items:center\">\
                 <span style=\"width:48px;height:48px;border-radius:var(--radius-md);background:{hx};border:1px solid var(--color-line);flex:none\"></span>\
                 <span><strong>--color-{n}</strong> <code>{hx}</code><br><span class=\"muted\">{role}</span></span></div>"
            )
        })
        .collect();
    let type_rows: String = t
        .type_scale
        .iter()
        .rev()
        .map(|(n, rem)| format!("<p style=\"font-size:var(--text-{n});margin:0\">{n} — {rem}rem · Grumpy wizards make a toxic brew</p>"))
        .collect();
    format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Preview — {subject}</title>{fonts}<style>{tokens}\n{kit}\n\
         body{{padding:var(--space-6)}} section{{margin-bottom:var(--space-9)}} \
         .swatches{{display:grid;gap:var(--space-4);grid-template-columns:repeat(auto-fit,minmax(240px,1fr))}}</style></head>\
         <body><header style=\"margin-bottom:var(--space-8)\"><h1>{subject}</h1>\
         <p class=\"muted\">Design system catalogue — {tone} tone. Generated by CodeTurbo.</p></header>\
         <section><h2>Colours and roles</h2><div class=\"swatches\">{swatches}</div></section>\
         <section><h2>Typography</h2><div class=\"stack\">{type_rows}</div></section>\
         <section><h2>Buttons</h2><div class=\"cluster\"><a class=\"btn btn--primary\" href=\"#\">Main action</a>\
         <a class=\"btn btn--secondary\" href=\"#\">Secondary</a></div></section>\
         <section><h2>Cards</h2><div class=\"grid\">\
         <div class=\"panel card\"><h3>Panel</h3><p class=\"muted\">A raised surface with the contract border and shadow.</p></div>\
         <div class=\"panel card\"><h3>Another</h3><p class=\"muted\">The grid is responsive by default.</p></div></div></section>\
         </body></html>",
        subject = t.brief.subject,
        tone = t.tone.name(),
        fonts = font_link(t),
        tokens = css(t),
        kit = crate::kit::base_css(t),
    )
}

pub fn markdown(t: &Tokens) -> String {
    let b = &t.brief;
    format!(
        "# Design contract\n\n\
         Generated by CodeTurbo. All interface code uses only these tokens; \
         the check refuses colours, radii and spacing outside them, and the \
         generic patterns of AI-generated interfaces.\n\n\
         ## Visual theme and atmosphere\n\n\
         - Subject: {}\n- Audience: {}\n- Tone: {} ({})\n- Signature element: {}\n- Brand colours kept: {}\n\n\
         ## Style: {style_name}\n\n\
         The whole page is in this one style, every section and component; mixing styles reads as visual confusion.\n\n\
         {style_rules}\n\n\
         ## Palette and roles\n\n| Token | Colour | Role |\n| --- | --- | --- |\n{roles}\n\n\
         ## Colour plan\n\n\
         Colour is the identity, not a button fill. Give whole sections their colour: the opening or the highlight band \
         on `--color-deep` (text `--color-on-deep`), at least one band on `--color-primary`, others alternating between \
         `--color-tint` and `--color-bg`; the accent marks numerals, ornaments and small details. Keep at least a third \
         of the page off the plain background: the check measures it on the rendered page.\n\n\
         ## Imagery\n\n\
         A subject people buy with their eyes (beauty, food, fashion, places, products, people) has to be shown in \
         photographs: the user's own, or free ones from `find_images` (call it once per motif before writing the page). \
         Use them in `<img>` with `aspect-ratio`, `object-fit: cover` and an alt saying what each shows. Line drawings \
         and icons do not replace a photo; the check refuses a long page with none.\n\n\
         ## Art direction\n\n\
         The page must look current, not like a 2010 template of stacked boxes:\n\n\
         - Opening: a photo that fills at least half the width (or a collage of two or three overlapping photos of \
         different sizes) beside a very large headline; the headline is the biggest thing on the screen.\n\
         - Scale contrast: one huge element per section (a photo, a numeral, a quote) against small, quiet text.\n\
         - At least one bento grid: tiles of different sizes in one block (a large photo tile, a number tile, a quote tile, \
         a short list tile), not a row of equal cards.\n\
         - A gallery band of the work (photos edge to edge, or a horizontal strip that scrolls with snap points).\n\
         - Alternate the layout from section to section: photo left, then right, then full width with the text over a \
         dark overlay; never the same title-then-boxes block twice in a row.\n\
         - Photos and tiles get the large radius; buttons are generous (48 px tall or more); the header is solid and \
         sticks to the top.\n\
         - Scrollbars are invisible (the kit hides them on the page and on every strip, which still scrolls by \
         swipe, wheel and drag); never style or bring them back.\n\n\
         ## Motion\n\n\
         `codeturbo-motion.js` animates the page with GSAP and ScrollTrigger in this style's rhythm ({motion}). \
         Load it at the end of `<body>`:\n\n```html\n\
         <script src=\"https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js\" defer></script>\n\
         <script src=\"https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/ScrollTrigger.min.js\" defer></script>\n\
         <script src=\"codeturbo-motion.js\" defer></script>\n```\n\n\
         Then mark the elements: `data-intro` on the opening; `data-reveal` on each section (`data-reveal=\"stagger\"` on a \
         list or grid brings its items in one by one); photos in `<figure class=\"frame\" data-parallax><img …></figure>` drift \
         as the page scrolls (use it on the opening photo and every large photo); `data-zoom` on a photo that grows as it enters; \
         a gallery strip of photos (not text tags) as `<div class=\"marquee\"><div data-marquee>…photos…</div></div>`. Content stays visible without \
         the script, and it does nothing under `prefers-reduced-motion`.\n\n\
         ## Icons\n\n\
         Drawn for this style; colour through `color`. Copy the `<svg>` you need into the HTML; never emoji or icon fonts.\n\n\
         ```html\n{icons}\n```\n\n\
         ## Copy\n\n\
         Write like a conversion copywriter, in the page's language:\n\n\
         - Headline: the outcome the audience wants, concrete (what they will be able to do or earn, and how soon), \
         not the name of the product's category. Subheading: for whom and how, in one sentence.\n\
         - Each section heading states a benefit or answers an objection; plain labels (\"Modules\", \"About\") only as small navigation.\n\
         - Specifics over adjectives: what is included, how long it takes, what they have at the end. Never invent \
         results, ratings, student counts, prices, testimonials or people (students, the teacher's name or photo) the user \
         did not give: leave a marked slot ([preço], [depoimento real], [foto da professora]) for the user to fill.\n\
         - Menu items are one or two short words (\"Módulos\", \"Professora\", \"Preço\") and, like buttons and the logo, \
         stay on one line; short titles fit on one line and longer ones break into even lines, never one lonely word.\n\
         - Calls to action name the result with a verb (\"Quero minha vaga\", \"Start the first lesson\"), never \
         \"Learn more\" or \"Click here\"; repeat the main one after the opening, after the proof and at the close.\n\
         - The FAQ answers the real objections: time, price, no experience, materials, certificate, guarantee.\n\
         - Short paragraphs (two or three lines), speak to the reader as \"you\", no buzzwords.\n\n\
         ## Composition\n\n{}.\n\n\
         ## Do and don't\n\n\
         Do: build around the signature element; vary the size and shape of blocks; \
         align running text to the left; give side margins and room between sections; keep visible focus, readable contrast and responsiveness.\n\n\
         Don't: the stock hero→features→testimonials→pricing→FAQ→contact order; grids of identical cards; \
         emoji icons; default fonts (Inter/Roboto); the Tailwind palette; colours outside this table.\n\n\
         ## Composition plan\n\n{}\n\n\
         ## Fonts\n\n{}\n\n\
         ## Tokens\n\n```css\n{}\n```\n\n\
         ## Base kit\n\nAlready passes the check: margins, fluid type, focus, reduced motion and a responsive grid. \
         Use the classes .container, .section, .grid, .cluster, .btn, .panel and .tag and build on top.\n\n```css\n{}\n```\n\n\
         ## Project ornaments\n\nDrawn for this project (colour through `color`). Motif:\n\n```html\n{}\n```\n\nSection divider:\n\n```html\n{}\n```\n",
        b.subject,
        b.audience,
        b.tone,
        t.tone.name(),
        b.signature,
        if b.brand.is_empty() { "none (new palette)".to_string() } else { b.brand.join(", ") }
            + &if b.fonts.is_empty() { String::new() } else { format!("\n- Project fonts kept: {}", b.fonts.join(", ")) },
        t.layout,
        crate::kit::blueprint(t).iter().enumerate().map(|(i, s)| format!("{}. {s}", i + 1)).collect::<Vec<_>>().join("\n"),
        font_link(t),
        css(t),
        crate::kit::base_css(t),
        crate::kit::ornament_svg(t),
        crate::kit::divider_svg(t),
        style_name = t.style.name(),
        style_rules = t.style.rules(),
        motion = {
            let m = t.style.motion();
            format!("{}s, {}, rising {}px, parallax {}", m.duration, m.ease, m.rise, m.parallax)
        },
        icons = crate::kit::icons(t).iter().map(|(n, svg)| format!("<!-- {n} -->\n{svg}")).collect::<Vec<_>>().join("\n"),
        roles = color_roles(t).iter().map(|(n, hx, role)| format!("| `--color-{n}` | {hx} | {role} |")).collect::<Vec<_>>().join("\n")
    )
}

/// The contract as read back from `DESIGN.md`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Contract {
    pub vars: BTreeMap<String, String>,
}

impl Contract {
    pub fn parse(text: &str) -> Contract {
        let re = regex::Regex::new(r"(--[a-z0-9-]+)\s*:\s*([^;]+);").expect("valid regex");
        let vars = re.captures_iter(text).map(|c| (c[1].to_string(), c[2].trim().to_string())).collect();
        Contract { vars }
    }

    pub fn load(root: &Path) -> Option<Contract> {
        let text = std::fs::read_to_string(root.join(FILE)).ok()?;
        let c = Contract::parse(&text);
        (!c.vars.is_empty()).then_some(c)
    }

    pub fn colors(&self) -> Vec<Rgb> {
        self.vars.iter().filter(|(k, _)| k.starts_with("--color-")).filter_map(|(_, v)| parse_hex(v)).collect()
    }

    /// Pixel values of the tokens starting with `prefix`, smallest first.
    fn px(&self, prefix: &str) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .vars
            .iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .filter_map(|(_, v)| v.trim_end_matches("px").parse().ok())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    pub fn space(&self) -> Vec<u32> {
        self.px("--space-")
    }

    pub fn radii(&self) -> Vec<u32> {
        self.px("--radius-")
    }
}
