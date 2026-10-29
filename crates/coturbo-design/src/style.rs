//! Named visual styles. A page commits to one, and the style fixes what
//! would otherwise drift section by section: type, corners, borders,
//! shadows, icon stroke and motion. One style, applied everywhere, is what
//! makes a page read as designed instead of assembled.

use crate::compiler::Tone;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Brutalist,
    NeoBrutalist,
    EditorialLuxury,
    Swiss,
    SoftOrganic,
    Cinematic,
}

/// How things move in a style, read by `codeturbo-motion.js`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    pub duration: f64,
    pub ease: &'static str,
    /// How far elements rise as they enter, in px.
    pub rise: u32,
    pub stagger: f64,
    /// Parallax strength of images, 0-0.35.
    pub parallax: f64,
}

pub const ALL: [Style; 6] = [
    Style::Brutalist,
    Style::NeoBrutalist,
    Style::EditorialLuxury,
    Style::Swiss,
    Style::SoftOrganic,
    Style::Cinematic,
];

impl Style {
    pub fn name(self) -> &'static str {
        match self {
            Style::Brutalist => "brutalist",
            Style::NeoBrutalist => "neo-brutalist",
            Style::EditorialLuxury => "editorial-luxury",
            Style::Swiss => "swiss",
            Style::SoftOrganic => "soft-organic",
            Style::Cinematic => "cinematic",
        }
    }

    /// From the name the model gave, in a few spellings.
    pub fn parse(s: &str) -> Option<Style> {
        let s = s.to_lowercase().replace(['_', ' '], "-");
        Some(match s.as_str() {
            x if x.contains("neo") => Style::NeoBrutalist,
            x if x.contains("brutal") => Style::Brutalist,
            x if x.contains("lux") || x.contains("editorial") => Style::EditorialLuxury,
            x if x.contains("swiss")
                || x.contains("suí")
                || x.contains("sui")
                || x.contains("international") =>
            {
                Style::Swiss
            }
            x if x.contains("organic")
                || x.contains("soft")
                || x.contains("orgân")
                || x.contains("suave") =>
            {
                Style::SoftOrganic
            }
            x if x.contains("cinema") || x.contains("dark") => Style::Cinematic,
            _ => return None,
        })
    }

    /// The style a tone falls into when the model names none.
    pub fn for_tone(t: Tone, seed: u64) -> Style {
        match t {
            Tone::Editorial => Style::EditorialLuxury,
            Tone::Playful => Style::NeoBrutalist,
            Tone::Technical => {
                if seed.is_multiple_of(2) {
                    Style::Swiss
                } else {
                    Style::Brutalist
                }
            }
            Tone::Warm => Style::SoftOrganic,
            Tone::Balanced => {
                if seed.is_multiple_of(2) {
                    Style::Swiss
                } else {
                    Style::Cinematic
                }
            }
        }
    }

    /// Display and body fonts (Google Fonts), the style's own voice.
    pub fn fonts(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Style::Brutalist => &[
                ("Archivo Black", "JetBrains Mono"),
                ("Anton", "IBM Plex Mono"),
                ("Space Grotesk", "JetBrains Mono"),
            ],
            Style::NeoBrutalist => &[
                ("Bricolage Grotesque", "Figtree"),
                ("Unbounded", "Onest"),
                ("Syne", "Hanken Grotesk"),
            ],
            Style::EditorialLuxury => &[
                ("Cormorant Garamond", "Karla"),
                ("Playfair Display", "Instrument Sans"),
                ("Fraunces", "Newsreader"),
            ],
            Style::Swiss => &[
                ("Archivo", "Archivo"),
                ("Hanken Grotesk", "Public Sans"),
                ("Schibsted Grotesk", "Source Sans 3"),
            ],
            Style::SoftOrganic => &[
                ("Fraunces", "Nunito Sans"),
                ("Gloock", "Mulish"),
                ("Young Serif", "Figtree"),
            ],
            Style::Cinematic => &[
                ("Syne", "Hanken Grotesk"),
                ("Bebas Neue", "Figtree"),
                ("Unbounded", "Onest"),
            ],
        }
    }

    /// Corner radii sm, md, lg in px.
    pub fn radii(self) -> [u32; 3] {
        match self {
            Style::Brutalist => [0, 0, 0],
            Style::NeoBrutalist => [8, 14, 24],
            Style::EditorialLuxury => [0, 2, 4],
            Style::Swiss => [0, 0, 2],
            Style::SoftOrganic => [12, 24, 40],
            Style::Cinematic => [4, 12, 20],
        }
    }

    /// Shadows 1 and 2; `{ink}` is replaced by the ink colour.
    pub fn shadows(self) -> [&'static str; 2] {
        match self {
            Style::Brutalist => ["4px 4px 0 {ink}", "8px 8px 0 {ink}"],
            Style::NeoBrutalist => ["3px 3px 0 {ink}", "6px 6px 0 {ink}"],
            Style::EditorialLuxury | Style::Swiss => ["none", "none"],
            Style::SoftOrganic => [
                "0 2px 6px rgb(0 0 0 / 0.06)",
                "0 24px 48px -24px rgb(0 0 0 / 0.25)",
            ],
            Style::Cinematic => [
                "0 1px 0 rgb(255 255 255 / 0.06)",
                "0 40px 80px -32px rgb(0 0 0 / 0.7)",
            ],
        }
    }

    /// Width of borders on panels, buttons and photos, in px (0: none).
    pub fn border(self) -> u32 {
        match self {
            Style::Brutalist => 3,
            Style::NeoBrutalist => 2,
            Style::Swiss => 0,
            Style::EditorialLuxury | Style::SoftOrganic | Style::Cinematic => 0,
        }
    }

    pub fn uppercase_display(self) -> bool {
        matches!(self, Style::Brutalist | Style::Cinematic)
    }

    pub fn dark(self) -> bool {
        self == Style::Cinematic
    }

    /// Stroke width and line ends of the icons.
    pub fn icon_stroke(self) -> (f64, &'static str, &'static str) {
        match self {
            Style::Brutalist => (2.5, "square", "miter"),
            Style::NeoBrutalist => (2.25, "round", "round"),
            Style::EditorialLuxury => (1.25, "round", "round"),
            Style::Swiss => (2.0, "square", "miter"),
            Style::SoftOrganic => (1.75, "round", "round"),
            Style::Cinematic => (1.5, "round", "round"),
        }
    }

    pub fn motion(self) -> Motion {
        match self {
            Style::Brutalist => Motion {
                duration: 0.45,
                ease: "power4.out",
                rise: 48,
                stagger: 0.05,
                parallax: 0.12,
            },
            Style::NeoBrutalist => Motion {
                duration: 0.6,
                ease: "back.out(1.4)",
                rise: 40,
                stagger: 0.08,
                parallax: 0.15,
            },
            Style::EditorialLuxury => Motion {
                duration: 1.2,
                ease: "expo.out",
                rise: 60,
                stagger: 0.12,
                parallax: 0.28,
            },
            Style::Swiss => Motion {
                duration: 0.6,
                ease: "power3.out",
                rise: 24,
                stagger: 0.06,
                parallax: 0.08,
            },
            Style::SoftOrganic => Motion {
                duration: 1.0,
                ease: "sine.out",
                rise: 32,
                stagger: 0.1,
                parallax: 0.18,
            },
            Style::Cinematic => Motion {
                duration: 1.1,
                ease: "power3.out",
                rise: 56,
                stagger: 0.1,
                parallax: 0.35,
            },
        }
    }

    /// What the style is, in rules the whole page follows.
    pub fn rules(self) -> &'static str {
        match self {
            Style::Brutalist => {
                "Raw and loud. Square corners everywhere; 3 px ink borders on panels, buttons and photos; hard offset shadows (no blur); \
                huge uppercase display type that may run edge to edge; monospace for body, labels and numbers; flat blocks of strong colour \
                set side by side; the grid shown, not hidden (rules between columns). No gradients, no soft shadows, no rounded anything. \
                Motion is fast and decisive."
            }
            Style::NeoBrutalist => {
                "Bold and friendly. Medium rounded corners; 2 px ink borders and offset shadows on every card, button and photo; \
                bright flat fills from the palette, one per block; chunky display type; stickers and tags slightly rotated; \
                no gradients and no blur. Motion has a little spring."
            }
            Style::EditorialLuxury => {
                "Quiet and expensive. Near-square corners; no shadows; hairline rules; a high-contrast serif set very large, \
                with small, wide-spaced body text; photos big, often full-bleed, with lots of empty space around few words; \
                numerals and quotes as display pieces. Motion is slow and smooth, images drift in parallax."
            }
            Style::Swiss => {
                "Precise and rational. A strict, visible grid; flush-left grotesk type in a few sizes with big jumps between them; \
                square corners; no shadows; thick rules above sections; one strong accent used sparingly; numbers large. \
                Motion is short and exact."
            }
            Style::SoftOrganic => {
                "Warm and tactile. Generous rounded corners and pill buttons; soft, diffuse shadows; tinted panels; \
                organic blob shapes behind photos; a soft serif for headings; relaxed spacing. Motion floats gently."
            }
            Style::Cinematic => {
                "Dark and immersive. A dark page; full-bleed photos with deep overlays and the text over them; \
                big uppercase display type; light text, one luminous accent; long sections that unfold as you scroll. \
                Motion is slow, with strong parallax and photos zooming as they enter."
            }
        }
    }
}
