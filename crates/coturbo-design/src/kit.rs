//! What the engine gives the model to build with, beyond the tokens: a
//! base stylesheet that already meets the inspector (fluid type, margins,
//! focus, motion, responsive grid, varied radii), a composition plan
//! picked for this project from a catalog of authored layouts, and SVG
//! ornaments drawn from the project's own seed. Every project gets its
//! own combination; none of it is the template.

use crate::compiler::{Tokens, Tone};

/// Deterministic numbers from the project's brief.
struct Rng(u64);

impl Rng {
    fn new(t: &Tokens) -> Self {
        let b = &t.brief;
        let seed = format!("{}|{}|{}|{}", b.subject, b.audience, b.tone, b.signature)
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ c as u64).wrapping_mul(0x0100_0000_01b3));
        Rng(seed | 1)
    }

    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

const HEROES: [&str; 6] = [
    "Asymmetric split opening (7/5): display-scale title on the left, the signature element on the right breaking the grid",
    "Typographic opening: the title takes the full width at display scale; supporting line and call to action left-aligned below, with the ornament behind",
    "Opening with the product in use: a product screen drawn in HTML and CSS (not an image), slightly tilted, with two real data labels",
    "Editorial opening: a large number or figure as the visual anchor, narrow text column beside it and a thin separating line",
    "Opening as a flat brand-colour band with a curved cut at the bottom (use the SVG divider), title at maximum contrast",
    "Two-layer opening: text on an offset surface panel, with the SVG ornament crossing both",
];

const SECTIONS: [&str; 12] = [
    "Benefits as a large numbered list (01, 02, 03) in two columns, without cards",
    "Mosaic of blocks of different sizes: one large with a demo, two medium and one narrow with a number",
    "How it works as a horizontal timeline that turns vertical on the phone, with thin connectors",
    "Before and after comparison in two columns with a colour contrast between them",
    "A single strong quote in large typographic emphasis, with name and context (not three testimonial cards)",
    "Proof in numbers: three or four large figures in a row, each with a short caption",
    "Plans in a typographic table: rows, columns and the recommended plan highlighted by background colour, not identical cards",
    "Frequently asked questions in two columns: question in display type on the left, answer on the right",
    "A highlight band in the brand colour with one short line and the signature element repeated",
    "Zoomed detail: a crop of the product at large scale with annotations pointing at real parts",
    "Manifesto: three short sentences at large scale, alternating alignment, separated by lines",
    "Use cases in tabs or a content switch, with the content changing beside it (no sideways scrolling)",
];

/// The composition plan for this project: an opening and four sections
/// drawn from the catalog in an order of its own, then the closing.
pub fn blueprint(t: &Tokens) -> Vec<String> {
    let mut rng = Rng::new(t);
    let mut plan = vec![HEROES[rng.below(HEROES.len())].to_string()];
    let mut pool: Vec<&str> = SECTIONS.to_vec();
    for _ in 0..4 {
        let i = rng.below(pool.len());
        plan.push(pool.remove(i).to_string());
    }
    plan.push("Closing: a short final call to action in a brand-colour block and a lean footer with the essentials".into());
    plan
}

/// Unique decorative motif for the project, drawn in `currentColor`.
pub fn ornament_svg(t: &Tokens) -> String {
    let mut rng = Rng::new(t);
    rng.next();
    let mut shapes = String::new();
    match t.tone {
        Tone::Technical => {
            let step = 20 + rng.below(3) * 4;
            for y in (step..240).step_by(step) {
                for x in (step..240).step_by(step) {
                    if rng.unit() < 0.55 {
                        let r = if rng.unit() < 0.15 { 2.5 } else { 1.2 };
                        shapes.push_str(&format!("<circle cx=\"{x}\" cy=\"{y}\" r=\"{r}\"/>"));
                    }
                }
            }
        }
        Tone::Editorial => {
            let (cx, cy) = (40.0 + rng.unit() * 160.0, 40.0 + rng.unit() * 160.0);
            for i in 1..=(5 + rng.below(4)) {
                let r = i as f64 * (14.0 + rng.unit() * 6.0);
                shapes.push_str(&format!("<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{r:.1}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1\"/>"));
            }
        }
        Tone::Playful => {
            for _ in 0..(9 + rng.below(6)) {
                let (x, y, s) = (rng.unit() * 220.0 + 10.0, rng.unit() * 220.0 + 10.0, 8.0 + rng.unit() * 18.0);
                let rot = rng.below(360);
                shapes.push_str(&match rng.below(3) {
                    0 => format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{s:.1}\" height=\"{s:.1}\" transform=\"rotate({rot} {x:.1} {y:.1})\"/>", x - s / 2.0, y - s / 2.0),
                    1 => format!("<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{:.1}\"/>", s / 2.0),
                    _ => format!("<path d=\"M{x:.1} {:.1} L{:.1} {:.1} L{:.1} {:.1}Z\" transform=\"rotate({rot} {x:.1} {y:.1})\"/>", y - s / 2.0, x + s / 2.0, y + s / 2.0, x - s / 2.0, y + s / 2.0),
                });
            }
        }
        Tone::Warm | Tone::Balanced => {
            for i in 0..(4 + rng.below(3)) {
                let y = 30.0 + i as f64 * 38.0;
                let amp = 8.0 + rng.unit() * 14.0;
                let len = 40.0 + rng.unit() * 30.0;
                let mut d = format!("M0 {y:.1}");
                let mut x = 0.0;
                let mut up = true;
                while x < 240.0 {
                    let nx = x + len;
                    let cy = if up { y - amp } else { y + amp };
                    d.push_str(&format!(" Q{:.1} {cy:.1} {nx:.1} {y:.1}", x + len / 2.0));
                    x = nx;
                    up = !up;
                }
                shapes.push_str(&format!("<path d=\"{d}\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\"/>"));
            }
        }
    }
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 240 240\" fill=\"currentColor\" aria-hidden=\"true\">{shapes}</svg>")
}

/// A section divider in the project's own shape, full width, `currentColor`.
pub fn divider_svg(t: &Tokens) -> String {
    let mut rng = Rng::new(t);
    for _ in 0..3 {
        rng.next();
    }
    let path = match t.tone {
        Tone::Technical => {
            let mut d = String::from("M0 80 L0 40");
            let mut x = 0;
            while x < 1440 {
                x += 60 + rng.below(4) * 30;
                d.push_str(&format!(" L{} {}", x.min(1440), if rng.unit() < 0.5 { 40 } else { 20 }));
            }
            d + " L1440 80Z"
        }
        Tone::Editorial => format!("M0 80 L0 {} L1440 {} L1440 80Z", 50 + rng.below(20), 30 + rng.below(20)),
        Tone::Playful => {
            let mut d = String::from("M0 80 L0 50");
            let mut x = 0;
            while x < 1440 {
                let w = 40 + rng.below(40);
                d.push_str(&format!(" L{} {} L{} 50", x + w / 2, 15 + rng.below(20), (x + w).min(1440)));
                x += w;
            }
            d + " L1440 80Z"
        }
        Tone::Warm | Tone::Balanced => {
            let a = 20.0 + rng.unit() * 30.0;
            let b = 20.0 + rng.unit() * 30.0;
            format!("M0 80 L0 {a:.0} C360 {:.0} 1080 {:.0} 1440 {b:.0} L1440 80Z", a - 30.0, b + 30.0)
        }
    };
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1440 80\" preserveAspectRatio=\"none\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"{path}\"/></svg>")
}

/// Base stylesheet built on the tokens. It already follows every rule the
/// inspector enforces, so pages start professional instead of fixing later.
pub fn base_css(t: &Tokens) -> String {
    format!("{}
{}", core_css(t), style_css(t))
}

fn core_css(t: &Tokens) -> String {
    let display_tracking = match t.tone {
        Tone::Editorial => "-0.02em",
        Tone::Playful => "-0.03em",
        Tone::Technical => "0",
        _ => "-0.015em",
    };
    let button_case = if t.tone == Tone::Technical { "uppercase" } else { "none" };
    format!(
        r#"*, *::before, *::after {{ box-sizing: border-box; }}
html {{ -webkit-text-size-adjust: 100%; }}
/* Scrollbars stay invisible; the page and every strip still scroll. */
* {{ scrollbar-width: none; -ms-overflow-style: none; }}
*::-webkit-scrollbar {{ display: none; width: 0; height: 0; }}
body {{ margin: 0; font-family: var(--font-body); font-size: var(--text-base); line-height: 1.6; color: var(--color-ink); background: var(--color-bg); }}
img, svg, video {{ display: block; max-width: 100%; height: auto; }}
h1, h2, h3 {{ font-family: var(--font-display); line-height: 1.1; letter-spacing: {display_tracking}; margin: 0 0 var(--space-4); text-wrap: balance; }}
h1 {{ font-size: clamp(var(--text-2xl), 6vw, var(--text-display)); }}
h2 {{ font-size: clamp(var(--text-xl), 4vw, var(--text-3xl)); }}
h3 {{ font-size: var(--text-lg); }}
p {{ margin: 0 0 var(--space-4); max-width: 65ch; text-wrap: pretty; }}
a {{ color: var(--color-primary-ink, var(--color-primary)); text-underline-offset: 0.2em; }}
body {{ overflow-x: clip; }}
.container {{ width: 100%; max-width: 1200px; margin-inline: auto; padding-inline: clamp(var(--space-4), 5vw, var(--space-8)); }}
/* Safety net: any top-level block or section that forgot .container still
   gets side padding, so content never hugs the edge. Low specificity. */
:where(body > header:not(.container), body > footer:not(.container), body > nav:not(.container), .section:not(.container)) {{ padding-inline: clamp(var(--space-4), 5vw, var(--space-8)); }}
.section {{ padding-block: clamp(var(--space-8), 10vw, var(--space-10)); }}
:where(main > * + *) {{ margin-top: clamp(var(--space-8), 8vw, var(--space-9)); }}
.section--surface {{ background: var(--color-surface); }}
.grid {{ display: grid; gap: var(--space-6); grid-template-columns: repeat(auto-fit, minmax(min(100%, 280px), 1fr)); }}
.stack > * + * {{ margin-top: var(--space-4); }}
.cluster {{ display: flex; flex-wrap: wrap; gap: var(--space-4); align-items: center; }}
/* Menu items, buttons and the logo stay on one line. */
:where(header a, nav a, .btn, button, .logo, .brand) {{ white-space: nowrap; }}
h4, h5, h6 {{ text-wrap: balance; }}
.btn {{ display: inline-flex; align-items: center; justify-content: center; gap: var(--space-2); min-height: 48px; padding: var(--space-3) var(--space-6); border: 2px solid transparent; border-radius: var(--radius-sm); font: 600 var(--text-base)/1 var(--font-body); text-transform: {button_case}; text-decoration: none; cursor: pointer; transition: transform 0.15s ease, background-color 0.15s ease; }}
.btn--primary {{ background: var(--color-primary); color: var(--color-on-primary); box-shadow: var(--shadow-1); }}
.btn--secondary {{ background: transparent; color: var(--color-ink); border-color: var(--color-ink); }}
.btn:active {{ transform: translateY(1px); }}
.panel {{ background: var(--color-surface); border: 1px solid var(--color-line); border-radius: var(--radius-lg); padding: var(--space-6); box-shadow: var(--shadow-2); }}
.tag {{ display: inline-block; padding: var(--space-1) var(--space-3); border-radius: var(--radius-md); background: var(--color-surface); color: var(--color-muted); font-size: var(--text-sm); }}
.muted {{ color: var(--color-muted); }}
.ornament {{ color: var(--color-accent); opacity: 0.35; pointer-events: none; }}
.divider {{ color: var(--color-surface); display: block; width: 100%; height: clamp(var(--space-8), 6vw, var(--space-9)); }}
.sr-only {{ position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; border: 0; }}
:focus-visible {{ outline: 3px solid var(--color-accent); outline-offset: 3px; }}
.card {{ transition: box-shadow 0.2s ease; }}
.link-underline {{ background-image: linear-gradient(currentColor, currentColor); background-size: 0% 2px; background-position: 0 100%; background-repeat: no-repeat; transition: background-size 0.25s ease; }}
.link-underline:hover {{ background-size: 100% 2px; }}
@media (prefers-reduced-motion: no-preference) {{
  .js .reveal {{ opacity: 0; transform: translateY(20px); transition: opacity 0.6s ease, transform 0.6s ease; }}
  .js .reveal.in {{ opacity: 1; transform: none; }}
  .card:hover {{ box-shadow: var(--shadow-2); }}
  .float {{ animation: ct-float 6s ease-in-out infinite; }}
  @keyframes ct-float {{ 0%, 100% {{ transform: translateY(0); }} 50% {{ transform: translateY(-10px); }} }}
  @keyframes ct-fade-rise {{ from {{ opacity: 0; transform: translateY(16px); }} to {{ opacity: 1; transform: none; }} }}
  [data-animate="rise"] {{ animation: ct-fade-rise 0.7s ease both; }}
}}
@media (max-width: 640px) {{ .cluster > .btn {{ flex: 1 1 100%; }} }}
@media (prefers-reduced-motion: reduce) {{ *, *::before, *::after {{ animation: none !important; transition: none !important; scroll-behavior: auto !important; }} }}"#
    )
}

/// The style's own rules on top of the base kit, plus the frames the motion
/// script drives (parallax photos and marquee strips).
pub fn style_css(t: &Tokens) -> String {
    let st = t.style;
    let mut s = format!("/* Style: {} */\n", st.name());
    let b = st.border();
    if b > 0 {
        s.push_str(&format!(
            ".panel, .card, .btn, .frame {{ border: {b}px solid var(--color-ink); }}\n\
             .btn--primary, .panel, .card {{ box-shadow: var(--shadow-1); }}\n\
             .btn:hover {{ box-shadow: var(--shadow-2); }}\n"
        ));
    }
    if st.uppercase_display() {
        s.push_str("h1, h2 { text-transform: uppercase; letter-spacing: -0.01em; line-height: 0.95; }\n");
    }
    match st {
        crate::style::Style::Swiss => s.push_str(".section > h2, .section > .container > h2 { border-top: 4px solid var(--color-ink); padding-top: var(--space-4); }\n"),
        crate::style::Style::SoftOrganic => s.push_str(".btn { border-radius: 999px; }\n"),
        crate::style::Style::EditorialLuxury => s.push_str("h1 { font-weight: 400; }\n.panel { border: 1px solid var(--color-line); }\n"),
        _ => {}
    }
    s.push_str(
        ".frame { position: relative; overflow: hidden; border-radius: var(--radius-lg); }\n\
         .frame > img, .frame > video { width: 100%; height: 100%; object-fit: cover; }\n\
         .motion [data-parallax] > img, .motion [data-parallax] > video { transform: scale(1.15); will-change: transform; }\n\
         .marquee { overflow: hidden; }\n\
         .marquee > [data-marquee] { display: flex; gap: var(--space-4); width: max-content; }\n\
         .marquee > [data-marquee] > * { flex: none; }\n",
    );
    s
}

/// `codeturbo-motion.js`: scroll animation with GSAP and ScrollTrigger in the
/// style's rhythm, driven by data attributes. Without GSAP, with reduced
/// motion or in a headless browser it does nothing, and the page stays whole.
pub fn motion_js(t: &Tokens) -> String {
    let m = t.style.motion();
    format!(
        r#"/* Generated by CodeTurbo: the motion of the {name} style. Load after GSAP:
   <script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js" defer></script>
   <script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/ScrollTrigger.min.js" defer></script>
   <script src="codeturbo-motion.js" defer></script>
   data-intro      the opening: its heading, text, buttons and photo come in in order
   data-reveal     rises in when scrolled to; data-reveal="stagger" does it child by child
   data-parallax   on a .frame holding an <img>: the photo drifts as the page scrolls (value 0-0.35)
   data-zoom       a photo that grows from 85% to full size as it enters
   data-marquee    inside a .marquee: the strip loops sideways (value: seconds per loop) */
(() => {{
  const M = {{ duration: {dur}, ease: "{ease}", rise: {rise}, stagger: {stagger}, parallax: {parallax} }};
  const g = window.gsap, ST = window.ScrollTrigger;
  if (!g || !ST || matchMedia("(prefers-reduced-motion: reduce)").matches || /Headless/i.test(navigator.userAgent)) return;
  g.registerPlugin(ST);
  document.documentElement.classList.add("motion");
  const enter = (targets, trigger, extra = {{}}) =>
    g.from(targets, {{ y: M.rise, opacity: 0, duration: M.duration, ease: M.ease, stagger: M.stagger, ...extra,
      scrollTrigger: trigger ? {{ trigger, start: "top 85%", once: true }} : undefined }});
  document.querySelectorAll("[data-intro]").forEach(el =>
    enter(el.querySelectorAll("h1, h2, p, .btn, .frame, img"), null, {{ delay: 0.1 }}));
  g.utils.toArray("[data-reveal]").forEach(el =>
    enter(el.dataset.reveal === "stagger" ? el.children : el, el));
  g.utils.toArray("[data-parallax]").forEach(frame => {{
    const media = frame.querySelector("img, video") || frame;
    const amp = Math.min(parseFloat(frame.dataset.parallax) || M.parallax, 0.35) * 18;
    g.fromTo(media, {{ yPercent: -amp }}, {{ yPercent: amp, ease: "none",
      scrollTrigger: {{ trigger: frame, start: "top bottom", end: "bottom top", scrub: true }} }});
  }});
  g.utils.toArray("[data-zoom]").forEach(el =>
    g.fromTo(el, {{ scale: 0.85 }}, {{ scale: 1, ease: "none", scrollTrigger: {{ trigger: el, start: "top bottom", end: "center center", scrub: true }} }}));
  g.utils.toArray("[data-marquee]").forEach(track => {{
    [...track.children].forEach(n => {{ const c = n.cloneNode(true); c.setAttribute("aria-hidden", "true"); track.appendChild(c); }});
    g.to(track, {{ xPercent: -50, ease: "none", duration: parseFloat(track.dataset.marquee) || 40, repeat: -1 }});
  }});
}})();
"#,
        name = t.style.name(),
        dur = m.duration,
        ease = m.ease,
        rise = m.rise,
        stagger = m.stagger,
        parallax = m.parallax,
    )
}

/// The icon set, drawn on a 24 px grid in the style's stroke: `(name, svg)`.
pub fn icons(t: &Tokens) -> Vec<(&'static str, String)> {
    let (w, cap, join) = t.style.icon_stroke();
    let r = t.radius.first().map_or(2, |(_, r)| (*r).min(4));
    let shapes: [(&str, String); 18] = [
        ("check", "<path d=\"M5 12.5l4.5 4.5L19 7\"/>".into()),
        ("arrow-right", "<path d=\"M4 12h15M13 6l6 6-6 6\"/>".into()),
        ("star", "<path d=\"M12 3l2.8 6 6.2.6-4.7 4.2 1.5 6.2L12 16.8 6.2 20l1.5-6.2L3 9.6 9.2 9z\"/>".into()),
        ("clock", "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M12 7v5l3 2\"/>".into()),
        ("shield", "<path d=\"M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z\"/><path d=\"M9 12l2 2 4-4\"/>".into()),
        ("award", "<circle cx=\"12\" cy=\"9\" r=\"5\"/><path d=\"M8.5 13.5L7 21l5-3 5 3-1.5-7.5\"/>".into()),
        ("play", "<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M10 8.5v7l6-3.5z\"/>".into()),
        ("chat", "<path d=\"M4 5h16v11H9l-5 4z\"/>".into()),
        ("calendar", format!("<rect x=\"3.5\" y=\"5\" width=\"17\" height=\"15.5\" rx=\"{r}\"/><path d=\"M3.5 10h17M8 3v4M16 3v4\"/>")),
        ("heart", "<path d=\"M12 20s-7-4.4-7-10a4 4 0 0 1 7-2.6A4 4 0 0 1 19 10c0 5.6-7 10-7 10z\"/>".into()),
        ("sparkle", "<path d=\"M12 3l1.8 5.4L19 10l-5.2 1.6L12 17l-1.8-5.4L5 10l5.2-1.6z\"/><path d=\"M19 16l.7 2 2 .7-2 .7-.7 2-.7-2-2-.7 2-.7z\"/>".into()),
        ("whatsapp", "<path d=\"M4 20l1.3-4A8 8 0 1 1 8 18.7z\"/><path d=\"M9 9.5c.5 2.5 2.5 4.5 5 5\"/>".into()),
        ("mail", format!("<rect x=\"3\" y=\"5.5\" width=\"18\" height=\"13\" rx=\"{r}\"/><path d=\"M3.5 6.5l8.5 6.5 8.5-6.5\"/>")),
        ("instagram", "<rect x=\"3.5\" y=\"3.5\" width=\"17\" height=\"17\" rx=\"5\"/><circle cx=\"12\" cy=\"12\" r=\"4\"/><path d=\"M17.2 6.8h.01\"/>".into()),
        ("map-pin", "<path d=\"M12 21s-6.5-6-6.5-11a6.5 6.5 0 0 1 13 0c0 5-6.5 11-6.5 11z\"/><circle cx=\"12\" cy=\"10\" r=\"2.5\"/>".into()),
        ("users", "<circle cx=\"9\" cy=\"8\" r=\"3.5\"/><path d=\"M2.5 20a6.5 6.5 0 0 1 13 0M16 4.5a3.5 3.5 0 0 1 0 7M18 14a5.5 5.5 0 0 1 3.5 6\"/>".into()),
        ("bottle", format!("<rect x=\"7\" y=\"10\" width=\"10\" height=\"11\" rx=\"{r}\"/><path d=\"M10 10V6h4v4M11 6V3h2v3\"/>")),
        ("plus", "<path d=\"M12 5v14M5 12h14\"/>".into()),
    ];
    shapes
        .into_iter()
        .map(|(name, body)| {
            (name, format!(
                "<svg class=\"icon\" viewBox=\"0 0 24 24\" width=\"24\" height=\"24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"{w}\" stroke-linecap=\"{cap}\" stroke-linejoin=\"{join}\" aria-hidden=\"true\">{body}</svg>"
            ))
        })
        .collect()
}
