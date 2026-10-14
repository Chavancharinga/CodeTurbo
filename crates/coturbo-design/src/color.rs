//! Colors: OKLCH (perceptual lightness, chroma, hue) to sRGB, hex, and the
//! WCAG contrast ratio.

pub type Rgb = (u8, u8, u8);

fn to_gamma(x: f64) -> f64 {
    if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 }
}

fn to_linear(x: f64) -> f64 {
    if x <= 0.040_45 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

/// Linear sRGB of an OKLCH color, possibly out of gamut.
fn oklch_linear(l: f64, c: f64, h: f64) -> [f64; 3] {
    let (a, b) = (c * h.to_radians().cos(), c * h.to_radians().sin());
    let l_ = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let m_ = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let s_ = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    [
        4.076_741_662_1 * l_ - 3.307_711_591_3 * m_ + 0.230_969_929_2 * s_,
        -1.268_438_004_6 * l_ + 2.609_757_401_1 * m_ - 0.341_319_396_5 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_614_7 * m_ + 1.707_614_701 * s_,
    ]
}

/// sRGB of an OKLCH color (`l` 0–1, `c` chroma, `h` degrees). Out-of-gamut
/// colors keep their lightness and hue and lose chroma until they fit.
pub fn oklch(l: f64, c: f64, h: f64) -> Rgb {
    let mut c = c;
    loop {
        let rgb = oklch_linear(l, c, h);
        if rgb.iter().all(|v| (-1e-4..=1.0001).contains(v)) || c <= 0.0 {
            let ch = |v: f64| (to_gamma(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
            return (ch(rgb[0]), ch(rgb[1]), ch(rgb[2]));
        }
        c = (c - 0.005).max(0.0);
    }
}

pub fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// `#abc` or `#aabbcc` (case-insensitive).
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let h = s.trim().strip_prefix('#')?;
    let full: String = match h.len() {
        3 => h.chars().flat_map(|c| [c, c]).collect(),
        6 => h.to_string(),
        _ => return None,
    };
    let n = u32::from_str_radix(&full, 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

/// OKLCH `(l, c, h)` of an sRGB color.
pub fn to_oklch((r, g, b): Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (to_linear(r as f64 / 255.0), to_linear(g as f64 / 255.0), to_linear(b as f64 / 255.0));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let ll = 0.210_454_255_3 * l + 0.793_617_785 * m - 0.004_072_046_8 * s;
    let a = 1.977_998_495_1 * l - 2.428_592_205 * m + 0.450_593_709_9 * s;
    let bb = 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766 * s;
    let c = (a * a + bb * bb).sqrt();
    let h = (bb.atan2(a).to_degrees() + 360.0) % 360.0;
    (ll, c, h)
}

/// WCAG relative luminance.
pub fn luminance((r, g, b): Rgb) -> f64 {
    let f = |v: u8| to_linear(v as f64 / 255.0);
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// WCAG contrast ratio, 1–21.
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Hue in degrees and saturation (0–1) in HSL terms, for spotting hues.
pub fn hue_sat((r, g, b): Rgb) -> (f64, f64) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    if d < 1e-9 {
        return (0.0, 0.0);
    }
    let l = (max + min) / 2.0;
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    ((h + 360.0) % 360.0, s)
}
