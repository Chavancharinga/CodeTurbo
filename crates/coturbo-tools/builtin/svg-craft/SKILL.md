---
name: svg-craft
description: Use automatically whenever you draw or edit an SVG — an icon, logo, illustration, ornament, pattern, divider, diagram, chart or SVG animation. Plans the shape on a grid, computes the geometry with code instead of guessing coordinates, keeps strokes and proportions consistent, and renders the result to an image to check it before delivering.
---

# SVG craft

Hand-typed coordinates are why generated SVGs look lopsided. Treat an SVG as geometry you compute and then look at, not as text you improvise.

## 1. Decide whether an SVG is the right medium

- Good for: icons, logos and wordmarks, ornaments and patterns, dividers, diagrams, charts, simple flat illustrations, UI mockups.
- Not for: realistic people, food, products, places, textures. Use a photograph or a generated image instead; a drawn imitation always looks worse.
- For standard icons (arrow, check, user, cart, mail) prefer an established set — Lucide, Phosphor, Tabler — copied as inline SVG. Draw only what does not exist there, and draw it in the same stroke and grid as the set used on the page.

## 2. Plan before drawing

Write the plan as a short comment or note first:

- Canvas: `viewBox` sized to the job — `0 0 24 24` for icons (live area 2–22, 2 px padding), `0 0 48 48` for detailed icons, a real aspect ratio for illustrations (`0 0 1200 800`), `0 0 1440 80` with `preserveAspectRatio="none"` for full-width dividers.
- Grid: the unit every coordinate snaps to (1 px for icons, 8 or 10 for illustrations).
- Construction: which primitives make the shape (circles, rects with rx, lines, arcs) and how they combine. Prefer primitives and simple paths over one long freehand path.
- Style: one stroke width (icons 1.5 or 2 at 24 px), one linecap and linejoin (`round` for friendly, `square`/`miter` for technical), fill or stroke but not a random mix, at most three colours from the page palette.

## 3. Compute the geometry with code

For anything with symmetry, repetition, curves or measured proportions, generate the SVG with a short script (Python, Node or the project's language) instead of typing numbers:

- Points on circles and polygons from `cos`/`sin`; radial repeats by rotating one element (`transform="rotate(a cx cy)"`) or computing each copy.
- Symmetric shapes: draw one half and mirror it (`scale(-1 1)` around the axis) or compute mirrored points.
- Curves: cubic Béziers with handles at about 0.55 × radius for circular arcs; smooth joins keep the handles of neighbouring segments collinear.
- Patterns and textures: a seeded random generator so the result is reproducible; `<pattern>` for repeats.
- Round to at most 2 decimals; snap 1 px strokes to half pixels (x.5) so they render crisp.
- Combine shapes with `<clipPath>`, `<mask>` or `fill-rule="evenodd"` for cut-outs instead of hand-tracing outlines.

## 4. Proportion and optical rules

- Optical centring: a triangle (play icon) moves about 1/12 of its width toward the point; round shapes are drawn slightly larger (about 4–6%) than square ones to look the same size.
- Consistent weight: equal stroke widths across all icons on a page; interior gaps at least equal to the stroke width.
- Corners: one radius family; inner radius = outer radius − stroke width when shapes nest.
- Balance: the visual centre sits slightly above the geometric centre; keep at least the padding of the grid clear on every side so nothing touches the viewBox edge (clipping is the most common defect).
- Logos: build the mark from a few strong primitives, test it at 16 px and in one colour; keep the wordmark as live text in the page's font or outline it — never rely on a font the SVG file cannot load.

## 5. Markup that behaves

- Colour through `currentColor` and CSS variables so the SVG follows the theme and dark mode; no hard-coded black.
- Decorative: `aria-hidden="true"`. Meaningful: `role="img"` with a `<title>` (and `<desc>` for diagrams).
- Inline in HTML for icons and anything animated; `width`/`height` attributes or CSS size so it never flashes at 300×150.
- No editor junk (metadata, ids nobody uses, `xml:space`, empty groups); group with `<g>` only when a group shares a transform or style.
- Text inside diagrams and charts: real `<text>` with the page font, `text-anchor` and `dominant-baseline` for alignment, at least 12 px rendered size.

## 6. Animation

- Prefer CSS or GSAP over SMIL. Line drawing: `stroke-dasharray` and `stroke-dashoffset` equal to `getTotalLength()`, animated to 0. Animate `transform` and `opacity`; set `transform-box: fill-box; transform-origin: center` when rotating parts.
- Honour `prefers-reduced-motion`: show the final state without motion.

## 7. Check it by looking at it

Never deliver an SVG you have not seen rendered:

1. Render it to PNG at its real size and at 4× (headless Edge/Chrome `--screenshot`, `resvg`, `rsvg-convert`, Inkscape, or the browser tool), on light and dark backgrounds.
2. Look at the image. Check: nothing clipped at the edges, strokes equal, symmetry true, gaps even, curves smooth with no kinks, the shape readable at 16–24 px, colours from the palette.
3. Fix the generating script, not the output by hand, and render again. Two or three rounds are normal.
