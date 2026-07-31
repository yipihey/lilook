//! Painting the source pane from `Document::spans()`.
//!
//! egui needs no new widget for this. `TextEdit::layouter` takes a closure
//! returning a `Galley`, so a `LayoutJob` built from the core's spans renders
//! inside the existing editable pane with selection, undo and typing intact.
//! Proving that was the one real unknown in `docs/plan-2.0.md`; it holds.
//!
//! The core says *what* each range is and this decides how it looks. That split
//! is what lets a SwiftUI pane show the same document without agreeing about a
//! palette -- and it is why `Token::Series(i)` carries an index rather than a
//! colour: only a frontend knows what the cycle resolved to on screen.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};
use lilook_core::Token;

/// The colours a series is drawn in, in cycle order.
///
/// lilaq's `petroff10`, which is its default. A figure that sets its own cycle
/// will disagree, and that is acceptable: the point of tinting a call is to tell
/// *which* series it is at a glance, not to be a colour reference.
const SERIES: [Color32; 10] = [
    Color32::from_rgb(0x3f, 0x90, 0xda),
    Color32::from_rgb(0xff, 0xa9, 0x0e),
    Color32::from_rgb(0xbd, 0x1f, 0x01),
    Color32::from_rgb(0x94, 0xa4, 0xa2),
    Color32::from_rgb(0x83, 0x2d, 0xb6),
    Color32::from_rgb(0xa9, 0x6b, 0x59),
    Color32::from_rgb(0xe7, 0x63, 0x00),
    Color32::from_rgb(0xb9, 0xac, 0x70),
    Color32::from_rgb(0x71, 0x75, 0x81),
    Color32::from_rgb(0x92, 0xda, 0xdd),
];

/// What colour a token gets, given the surrounding theme.
pub fn token_color(token: Token, visuals: &egui::Visuals) -> Color32 {
    let dark = visuals.dark_mode;
    match token {
        Token::Comment => visuals.weak_text_color(),
        Token::Str if dark => Color32::from_rgb(0x9e, 0xce, 0x6a),
        Token::Str => Color32::from_rgb(0x2a, 0x7a, 0x2a),
        Token::Number if dark => Color32::from_rgb(0xff, 0x9e, 0x64),
        Token::Number => Color32::from_rgb(0xa8, 0x54, 0x00),
        Token::Keyword if dark => Color32::from_rgb(0xbb, 0x9a, 0xf7),
        Token::Keyword => Color32::from_rgb(0x7a, 0x3e, 0xa8),
        Token::Binding if dark => Color32::from_rgb(0x7d, 0xcf, 0xff),
        Token::Binding => Color32::from_rgb(0x00, 0x5f, 0x87),
        Token::Call => visuals.strong_text_color(),
        // The delight: a series call wears the colour it draws, so the line in
        // the source and the curve on the canvas are visibly the same thing.
        Token::Series(i) => SERIES[i % SERIES.len()],
    }
}

/// Build the layout for one pass of the source pane.
///
/// `spans` must be ordered and non-overlapping, which is `Document::spans()`'s
/// contract. Anything between them is plain text.
pub fn layout_job(
    text: &str,
    spans: &[(std::ops::Range<usize>, Token)],
    font: FontId,
    visuals: &egui::Visuals,
    wrap_width: f32,
) -> LayoutJob {
    let mut job = LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: wrap_width,
            ..Default::default()
        },
        ..Default::default()
    };
    let plain = TextFormat {
        font_id: font.clone(),
        color: visuals.text_color(),
        ..Default::default()
    };
    let mut at = 0usize;
    for (range, token) in spans {
        // Defensive: the pane may lay out a buffer the user has typed into
        // before the document has caught up, so a stale span could be out of
        // bounds or overlap. Skipping is the right answer -- a frame of plain
        // text beats a panic.
        if range.start < at || range.end > text.len() {
            continue;
        }
        if !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
            continue;
        }
        if range.start > at {
            job.append(&text[at..range.start], 0.0, plain.clone());
        }
        job.append(
            &text[range.clone()],
            0.0,
            TextFormat {
                font_id: font.clone(),
                color: token_color(*token, visuals),
                ..Default::default()
            },
        );
        at = range.end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, plain);
    }
    job
}
