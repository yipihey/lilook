//! Reading and writing the small Typst literals the inspector edits.
//!
//! Everything here is text in, text out. lilook never holds a parsed model of
//! an argument -- the source is the model -- so a control's job is to recognise
//! the shape it can edit, and to decline when it cannot. Declining is the
//! common case and must be cheap: `stroke: my-style` and
//! `stroke: 1pt + red.darken(20%)` both fall through to the source editor, and
//! that is correct behaviour rather than a gap.

pub use lilook_core::{parse_text, split_numeric, TextShape};
// The name tables live beside the schema policy, so a completion in the source
// pane offers exactly what the inspector's menu does.
pub use lilook_core::policy::{DASH_NAMES, MARK_NAMES, SCALE_NAMES};

use egui::Color32;

/// Typst's named colours, with the values typst actually uses. `red` is not
/// `#ff0000`, and a swatch that says it is would be a lie the user only notices
/// after exporting.
pub const NAMED_COLORS: &[(&str, u32)] = &[
    ("black", 0x000000),
    ("gray", 0xaaaaaa),
    ("silver", 0xdddddd),
    ("white", 0xffffff),
    ("navy", 0x001f3f),
    ("blue", 0x0074d9),
    ("aqua", 0x7fdbff),
    ("teal", 0x39cccc),
    ("eastern", 0x239dad),
    ("purple", 0xb10dc9),
    ("fuchsia", 0xf012be),
    ("maroon", 0x85144b),
    ("red", 0xff4136),
    ("orange", 0xff851b),
    ("yellow", 0xffdc00),
    ("olive", 0x3d9970),
    ("green", 0x2ecc40),
    ("lime", 0x01ff70),
];

pub const H_ALIGN: &[&str] = &["left", "center", "right", "start", "end"];
pub const V_ALIGN: &[&str] = &["top", "horizon", "bottom"];

/// Format a number back into source: no trailing zeros, no exponent soup.
pub fn num(v: f64) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// A colour literal lilook can show as a swatch and write back.
///
/// lilaq's own defaults lean on `.transparentize(N%)` -- the legend's fill is
/// `white.transparentize(20%)` -- so recognising only flat colours would
/// leave the commonest translucent fill in the package unrecognised as a
/// colour at all, degrading straight to the raw source editor.
pub fn parse_color(s: &str) -> Option<Color32> {
    let s = s.trim();
    if let Some((base, pct)) = s
        .strip_suffix(')')
        .and_then(|r| r.rsplit_once(".transparentize("))
    {
        let pct: f32 = pct.strip_suffix('%')?.trim().parse().ok()?;
        let base = parse_color(base)?;
        let keep = (1.0 - pct / 100.0).clamp(0.0, 1.0);
        let [r, g, b, a] = base.to_srgba_unmultiplied();
        return Some(Color32::from_rgba_unmultiplied(
            r,
            g,
            b,
            (a as f32 * keep).round() as u8,
        ));
    }
    if let Some(hex) = s
        .strip_prefix("rgb(")
        .and_then(|r| r.strip_suffix(')'))
        .map(str::trim)
        .and_then(|q| q.strip_prefix('"'))
        .and_then(|q| q.strip_suffix('"'))
        .and_then(|q| q.strip_prefix('#'))
    {
        return from_hex(hex);
    }
    if let Some(inner) = s.strip_prefix("luma(").and_then(|r| r.strip_suffix(')')) {
        let (v, unit) = split_numeric(inner)?;
        let l = if unit == "%" { v * 2.55 } else { v };
        let l = l.clamp(0.0, 255.0) as u8;
        return Some(Color32::from_rgb(l, l, l));
    }
    NAMED_COLORS
        .iter()
        .find(|(n, _)| *n == s)
        .map(|(_, v)| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, *v as u8))
}

fn from_hex(hex: &str) -> Option<Color32> {
    let b = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        6 => Some(Color32::from_rgb(b(0)?, b(2)?, b(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(b(0)?, b(2)?, b(4)?, b(6)?)),
        3 => {
            let d = |i: usize| {
                let c = u8::from_str_radix(hex.get(i..i + 1)?, 16).ok()?;
                Some(c * 17)
            };
            Some(Color32::from_rgb(d(0)?, d(1)?, d(2)?))
        }
        _ => None,
    }
}

/// Write a colour back as source. Named colours survive round-tripping: an
/// untouched `red` must not silently become `rgb("#ff4136")` in the user's file.
pub fn color_source(c: Color32, was: &str) -> String {
    if parse_color(was) == Some(c) {
        return was.trim().to_string();
    }
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 {
        format!("rgb(\"#{r:02x}{g:02x}{b:02x}\")")
    } else {
        format!("rgb(\"#{r:02x}{g:02x}{b:02x}{a:02x}\")")
    }
}

/// The parts of a stroke lilook can edit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stroke {
    pub paint: Option<String>,
    pub thickness: Option<String>,
    pub dash: Option<String>,
}

/// Recognise `red`, `2pt`, `red + 2pt`, and the dictionary form. Anything else
/// -- a binding, an expression, a gradient -- returns None and gets the source
/// editor.
pub fn parse_stroke(s: &str) -> Option<Stroke> {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        let mut out = Stroke::default();
        for part in split_top_level(inner, ',') {
            let (k, v) = part.split_once(':')?;
            let v = v.trim().to_string();
            match k.trim() {
                "paint" => out.paint = Some(v),
                "thickness" => out.thickness = Some(v),
                "dash" => out.dash = Some(v.trim_matches('"').to_string()),
                // `cap`, `join`, `miter-limit`: recognised but not editable
                // here, so the whole thing stays in the source editor rather
                // than being silently dropped on write-back.
                _ => return None,
            }
        }
        return Some(out);
    }
    let mut out = Stroke::default();
    for part in split_top_level(s, '+') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        if split_numeric(part).is_some_and(|(_, u)| !u.is_empty()) {
            out.thickness = Some(part.to_string());
        } else if parse_color(part).is_some() {
            out.paint = Some(part.to_string());
        } else {
            return None;
        }
    }
    (out != Stroke::default()).then_some(out)
}

pub fn stroke_source(s: &Stroke) -> String {
    match (&s.paint, &s.thickness, &s.dash) {
        (p, t, None) => match (p, t) {
            (Some(p), Some(t)) => format!("{p} + {t}"),
            (Some(p), None) => p.clone(),
            (None, Some(t)) => t.clone(),
            (None, None) => "none".into(),
        },
        (p, t, Some(d)) => {
            let mut parts = vec![];
            if let Some(p) = p {
                parts.push(format!("paint: {p}"));
            }
            if let Some(t) = t {
                parts.push(format!("thickness: {t}"));
            }
            parts.push(format!("dash: \"{d}\""));
            format!("({})", parts.join(", "))
        }
    }
}

/// Split on a separator that is not inside brackets, quotes or a call.
pub fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out = vec![];
    let mut depth = 0i32;
    let mut quoted = false;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '(' | '[' | '{' if !quoted => depth += 1,
            ')' | ']' | '}' if !quoted => depth -= 1,
            _ if c == sep && depth == 0 && !quoted => {
                out.push(s[start..i].trim().to_string());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() || out.is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// Write words back in the shape they came in.
pub fn text_source(shape: TextShape, text: &str) -> String {
    match shape {
        TextShape::Str => {
            let mut out = String::with_capacity(text.len() + 2);
            out.push('"');
            for c in text.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    _ => out.push(c),
                }
            }
            out.push('"');
            out
        }
        // Content is markup, so `]` has to be escaped or it ends the block early.
        // Everything else the user types is markup on purpose -- `*bold*` should
        // stay bold, which is why anyone picks content over a string.
        TextShape::Content => format!("[{}]", text.replace('\\', "\\\\").replace(']', "\\]")),
    }
}

/// `left + top` -> ("left", "top"), in either order.
pub fn parse_alignment(s: &str) -> Option<(Option<String>, Option<String>)> {
    let parts = split_top_level(s.trim(), '+');
    let (mut h, mut v) = (None, None);
    for p in parts {
        if H_ALIGN.contains(&p.as_str()) {
            h = Some(p);
        } else if V_ALIGN.contains(&p.as_str()) {
            v = Some(p);
        } else {
            return None;
        }
    }
    (h.is_some() || v.is_some()).then_some((h, v))
}

pub fn alignment_source(h: &Option<String>, v: &Option<String>) -> String {
    match (h, v) {
        (Some(h), Some(v)) => format!("{h} + {v}"),
        (Some(h), None) => h.clone(),
        (None, Some(v)) => v.clone(),
        (None, None) => "center".into(),
    }
}

// Recovered series data is emitted by `lilook_core::data_array_source`, not from
// here. `num()` below is for numbers a *gesture* produced, where six decimal
// places is right because the rest is mouse jitter; running data through it
// truncated small magnitudes to zero and turned every non-finite value into a
// real measurement of zero. Two emitters is how that stayed invisible, so there
// is now one.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_units() {
        assert_eq!(split_numeric("8cm"), Some((8.0, "cm".into())));
        assert_eq!(split_numeric("6%"), Some((6.0, "%".into())));
        assert_eq!(split_numeric("-2.5pt"), Some((-2.5, "pt".into())));
        assert_eq!(split_numeric("42"), Some((42.0, "".into())));
        assert_eq!(split_numeric("calc.pi"), None);
        // An exponent is part of the number, not a unit.
        assert_eq!(split_numeric("1e-4"), Some((1e-4, "".into())));
        assert_eq!(split_numeric("1em"), Some((1.0, "em".into())));
    }

    #[test]
    fn colours_round_trip_without_rewriting_names() {
        let red = parse_color("red").unwrap();
        assert_eq!(red, Color32::from_rgb(0xff, 0x41, 0x36));
        // Untouched, `red` stays `red`.
        assert_eq!(color_source(red, "red"), "red");
        // Changed, it becomes an explicit literal.
        assert_eq!(
            color_source(Color32::from_rgb(0x12, 0x34, 0x56), "red"),
            "rgb(\"#123456\")"
        );
        assert_eq!(
            parse_color("rgb(\"#4c72b0\")"),
            Some(Color32::from_rgb(0x4c, 0x72, 0xb0))
        );
        assert_eq!(parse_color("luma(50%)"), Some(Color32::from_gray(127)));
        assert_eq!(parse_color("color.map.viridis"), None);
        assert_eq!(parse_color("accent"), None);
    }

    #[test]
    fn transparentize_is_alpha_not_an_opaque_expression() {
        // lilaq's own legend default: white at 80% opacity.
        let fill = parse_color("white.transparentize(20%)").unwrap();
        assert_eq!(fill, Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 204));
        // A named colour composes with it too.
        assert_eq!(
            parse_color("red.transparentize(50%)"),
            Some(Color32::from_rgba_unmultiplied(0xff, 0x41, 0x36, 128))
        );
        // Unrecognised until now: this used to fall back to the raw source
        // editor, the same as any other opaque expression.
        assert_eq!(
            color_source(fill, "white.transparentize(20%)"),
            "white.transparentize(20%)",
            "unchanged, so opening the picker does not rewrite the source"
        );
    }

    #[test]
    fn strokes_recognised_and_written_back() {
        assert_eq!(
            parse_stroke("red + 2pt"),
            Some(Stroke {
                paint: Some("red".into()),
                thickness: Some("2pt".into()),
                dash: None
            })
        );
        assert_eq!(
            parse_stroke("2pt"),
            Some(Stroke {
                thickness: Some("2pt".into()),
                ..Default::default()
            })
        );
        let dict = parse_stroke("(paint: blue, thickness: 1pt, dash: \"dashed\")").unwrap();
        assert_eq!(dict.dash.as_deref(), Some("dashed"));
        assert_eq!(
            stroke_source(&dict),
            "(paint: blue, thickness: 1pt, dash: \"dashed\")"
        );
        assert_eq!(
            stroke_source(&Stroke {
                paint: Some("red".into()),
                thickness: Some("2pt".into()),
                dash: None
            }),
            "red + 2pt"
        );
        // Not recognised: a binding, an expression, or a form we would lose
        // information writing back.
        assert_eq!(parse_stroke("my-style"), None);
        assert_eq!(parse_stroke("red.darken(20%)"), None);
        assert_eq!(parse_stroke("(paint: red, cap: \"round\")"), None);
    }

    #[test]
    fn splitting_respects_nesting() {
        assert_eq!(
            split_top_level("rgb(\"#aabbcc\") + 2pt", '+'),
            vec!["rgb(\"#aabbcc\")", "2pt"]
        );
        assert_eq!(
            split_top_level("a: (1, 2), b: 3", ','),
            vec!["a: (1, 2)", "b: 3"]
        );
    }

    #[test]
    fn content_and_alignment() {
        assert_eq!(
            parse_alignment("left + top"),
            Some((Some("left".into()), Some("top".into())))
        );
        assert_eq!(
            parse_alignment("horizon"),
            Some((None, Some("horizon".into())))
        );
        assert_eq!(parse_alignment("wherever"), None);
        assert_eq!(
            alignment_source(&Some("center".into()), &Some("horizon".into())),
            "center + horizon"
        );
    }

    /// `num()` is for gesture geometry, and rounds hard on purpose. The tests
    /// that this is *not* good enough for data live next to the data emitter in
    /// `lilook-core`.
    #[test]
    fn gesture_numbers_round_to_something_a_person_would_type() {
        assert_eq!(num(6.000000001), "6");
        assert_eq!(num(2.5), "2.5");
        assert_eq!(num(-0.3333333333), "-0.333333");
    }
}
