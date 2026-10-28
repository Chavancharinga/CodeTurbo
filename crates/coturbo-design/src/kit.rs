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
