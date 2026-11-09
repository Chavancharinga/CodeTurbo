---
name: frontend-design
description: Use automatically for any web interface work — a new site, page, landing, dashboard, app screen or component, and above all a REDESIGN of an existing generic site into a professional one. Sets the art direction, a palette derived from the brand identity, typography, the layout grid, UI/UX rules, GSAP motion with smooth scrolling and invisible scrollbars, and the verification loop.
---

# Frontend design: from generic to professional

A page looks generated when every decision was the default one. This skill replaces each default with a decision. Work through the phases in order; do not write markup before phase 3.

## 1. Read what exists (always, and first on a redesign)

- Collect the identity: logo, brand colours (from the logo, CSS variables, Tailwind config, existing pages), fonts, tone of the copy, photos the client owns.
- Collect the content: every heading, paragraph, price, contact, testimonial and link. A redesign keeps the content and the brand; it changes structure, hierarchy, rhythm, typography and motion. Never invent facts, numbers, reviews, clients or people: leave a visible slot such as `[real testimonial]` for the user to fill.
- Audit the current page and write the list of what makes it generic before changing anything (see the checklist in section 9). The redesign answers that list point by point.

## 2. Choose one direction

Name it in two or three words and write one sentence on why it fits the audience (for example "Editorial warmth: a family bakery selling to neighbours"). Pick one signature element the page is built around: an oversized numeral, a photographic crop, a typographic logo lockup, a cut-out shape, a ticker. Everything else stays quiet so the signature stands out.

Directions that work as starting points (adapt, never copy whole):

| Direction | Background | Ink | Lines | Accent idea |
| --- | --- | --- | --- | --- |
| Industrial precision | `#0A0A0B` / surface `#141417` | `#F4F4F5` | `#27272A` | international orange or acid green, used sparingly |
| Editorial paper | `#F7F5F0` | `#18181B` | `#E4E2DD` | terracotta or deep cobalt |
| Sober luxury | `#090D16` | `#E8E4DA` | `#1F2633` | champagne or cold slate |
| Organic calm | `#F3F1EA` | `#1F2A22` | `#D9D6CB` | moss, clay or ochre |

## 3. Palette from the identity

Derive the palette from the brand colour, never from a framework palette.

1. Convert the brand colour to OKLCH. Keep its hue (H) as the identity hue.
2. Neutrals: tint them with the identity hue at very low chroma (C 0.005–0.02): background L≈0.97 (light) or 0.14 (dark), surface one step away, lines L±0.08 from the background, muted text L≈0.50, ink L≈0.18 (or 0.95 on dark).
3. Primary: the brand colour itself, adjusted only for contrast. Text on it must reach 4.5:1; if it does not, darken or lighten L, not the hue.
4. Accent: one colour that makes the page memorable — the complement (H+180), a split complement (H±150) or a warm/cold counterpoint — at higher chroma, used on at most 10% of the surface: numerals, a rule, a highlight, the focus ring.
5. Deep: a very dark, slightly saturated version of the identity hue for full-bleed bands.
6. Proportions 60/30/10: background and neutrals, identity colour in whole sections (not only buttons), accent in details. At least a third of a long page sits on a colour other than the plain background.
7. Check every text pair: body text 4.5:1, large text and UI 3:1. Write the tokens as CSS custom properties (`--color-bg`, `--color-surface`, `--color-ink`, `--color-muted`, `--color-line`, `--color-primary`, `--color-on-primary`, `--color-accent`, `--color-deep`) with a dark-mode set under `prefers-color-scheme`.

Forbidden as defaults: purple/indigo-to-blue gradients, the stock Tailwind palette, pure `#000` on pure `#fff` for long text, more than one accent, gradients as decoration on every section.

## 4. Typography

- Two families at most, plus a mono for figures: a display face with character (Instrument Serif, Fraunces, Syne, Space Grotesk, Bricolage Grotesque, Playfair Display, Clash Display) and a quiet text face (Geist, DM Sans, Inter Tight, Source Serif 4). Inter, Roboto, Arial or Open Sans alone means no decision was made.
- A modular scale (1.25 or 1.333) with fluid sizes: `font-size: clamp(min, preferred-vw, max)`. The display heading is at least three steps above body text.
- Body 16–18 px, line-height 1.5–1.7, measure 60–75 characters (`max-width: 65ch`). Headings line-height 1.0–1.15 and slight negative tracking; `text-wrap: balance` on headings, `pretty` on paragraphs.
- Numbers, prices and data in the mono or with `font-variant-numeric: tabular-nums`.

## 5. Grid and layout (UI rules)

- Spacing comes from one scale on an 8 px base (4, 8, 12, 16, 24, 32, 48, 64, 96, 128). No arbitrary values.
- Columns: 12 on desktop (≥1024 px), 8 on tablet, 4 on phone; gutters 24/20/16 px; outer margins `clamp(16px, 5vw, 64px)`; content max-width about 1200–1320 px, text columns narrower. Every block starts and ends on a column line.
- Break the symmetry on purpose: 7/5 or 8/4 splits, a block that crosses into the margin, varied block sizes in a bento mosaic. Never three identical cards in a row as the main idea of a section.
- Vertical rhythm: section padding 96–160 px desktop, 64–96 px phone; related items close together, unrelated items far apart (proximity).
- Alignment: running text left-aligned; centre only short headings and single calls to action.
- Radii from a small set (for example 4 / 12 / 999) chosen for the direction; borders 1 px in the line colour before reaching for shadows; at most two shadow levels, soft and low.
- Layer order is planned: nothing overlaps by accident, nothing touches the viewport edge, no horizontal scroll at 390 px.

## 6. UX rules

- One primary action per view, visually dominant; secondary actions as outline or text links. A call to action names the result with a verb ("Book a tasting", not "Learn more").
- Hierarchy reads in three levels at a glance: what this is, why it matters, what to do.
- Navigation: at most 5–7 items, one word or two each, the current page marked; on phones a menu that opens without covering content awkwardly.
- Touch targets at least 44×44 px; links distinguishable without colour alone.
- Every interactive element has hover, focus-visible, active and disabled states; async actions have loading, success, empty and error states with plain-language messages.
- Forms: visible labels (not placeholders as labels), inline validation after the field is left, errors next to the field, the right input types and autocomplete.
- Accessibility: semantic landmarks, one h1, ordered headings, alt text that says what an image shows, visible focus ring, `prefers-reduced-motion` honoured, colour contrast as in section 3.
- Performance: the hero image sized and preloaded, other images `loading="lazy"` with width/height or `aspect-ratio`, fonts with `font-display: swap`, no layout shift.

## 7. Motion: GSAP, smooth scroll, invisible scrollbars

- Animate only `transform` and `opacity`. Durations 0.2–0.4 s for UI feedback, 0.6–1.2 s for entrances; eases `power3.out` / `expo.out`; small staggers (0.06–0.12 s).
- Use GSAP with ScrollTrigger for scroll reveals, parallax on large photos (subtle, 5–15%), pinned sequences only when they tell something.
- Smooth scrolling with Lenis driven by the GSAP ticker; anchor links scroll smoothly too.
- Scrollbars are invisible everywhere; the page and inner strips still scroll by wheel, touch and keyboard.
- Content is visible without JavaScript; with `prefers-reduced-motion: reduce` nothing moves and native scrolling is used.

```html
<script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/gsap.min.js" defer></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/gsap/3.12.5/ScrollTrigger.min.js" defer></script>
<script src="https://cdn.jsdelivr.net/npm/lenis@1.1.20/dist/lenis.min.js" defer></script>
```

```css
html { scrollbar-width: none; -ms-overflow-style: none; }
html::-webkit-scrollbar, *::-webkit-scrollbar { display: none; width: 0; height: 0; }
* { scrollbar-width: none; }
@media (prefers-reduced-motion: no-preference) { html { scroll-behavior: smooth; } }
html.lenis, html.lenis body { height: auto; }
.lenis.lenis-smooth { scroll-behavior: auto !important; }
.lenis.lenis-stopped { overflow: hidden; }
```

```js
addEventListener("DOMContentLoaded", () => {
  const { gsap, ScrollTrigger, Lenis } = window;
  if (!gsap || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  gsap.registerPlugin(ScrollTrigger);
  if (Lenis) {
    const lenis = new Lenis({ duration: 1.1, easing: t => 1 - Math.pow(1 - t, 4) });
    lenis.on("scroll", ScrollTrigger.update);
    gsap.ticker.add(t => lenis.raf(t * 1000));
    gsap.ticker.lagSmoothing(0);
    document.querySelectorAll('a[href^="#"]').forEach(a => a.addEventListener("click", e => {
      const target = document.querySelector(a.getAttribute("href"));
      if (target) { e.preventDefault(); lenis.scrollTo(target, { offset: -80 }); }
    }));
  }
  gsap.utils.toArray("[data-reveal]").forEach(el => gsap.from(el, {
    y: 32, opacity: 0, duration: 0.9, ease: "power3.out",
    scrollTrigger: { trigger: el, start: "top 85%", once: true } }));
});
```

## 8. Images, icons and copy

- Photos for anything people judge with their eyes (food, places, products, people). Use the client's own, real stock photos (Unsplash, Pexels) whose URLs you have actually opened, or generated images when an image tool is available. Never invent an image URL; check each one loads.
- Icons from one vector set with one stroke width (Lucide, Phosphor, Tabler) or drawn with the `svg-craft` skill. Never emoji as icons or bullets.
- Copy: no emojis anywhere, no filler ("In today's digital world", "Unlock", "Elevate", "Seamless"), no lorem ipsum. Headline states the outcome for the reader; sections answer real objections; short paragraphs.

## 9. Generic-design checklist (audit before, verify after)

Each of these must be "no" when you finish:

- Centered hero with gradient blob, badge pill above the title and two buttons side by side, as the whole idea?
- Purple/blue gradient, glassmorphism or glow used as decoration?
- Inter/Roboto only, every heading the same weight and size class?
- Three identical feature cards with an icon on top?
- The stock order hero → features → testimonials → pricing → FAQ → footer with nothing changed?
- Uniform section padding and the same card radius everywhere?
- Emoji, stock 3D illustrations or invented testimonials?
- A visible scrollbar, horizontal scroll on the phone, text touching the screen edge, overlapping elements?
- Motion that is missing, or everything fading in at once in the same way?

## 10. Verify

Render the page at 1440, 1024, 768 and 390 px wide, look at the screenshots, fix what is off, and repeat. Click every link and button, tab through the page, test reduced motion. On a redesign, compare before and after side by side and state what changed and why.
