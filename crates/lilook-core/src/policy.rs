//! How a parameter should be edited, decided from the schema.
//!
//! Not a widget in sight. Which control a parameter wants, what a safe starting
//! value is, whether `none` means "unset" here -- these are facts about lilaq's
//! schema, and every frontend needs the same answers. The egui inspector renders
//! them; a Swift view would render them differently; and the MCP server states
//! them in words so an agent knows that `xscale` wants one of a fixed set and
//! `width` wants a length before it writes anything.
//!
//! The line drawn here is *decided by the schema alone*. Narrowing a control by
//! looking at the value already written -- `refine` and `control_of` -- stays in
//! the egui crate, because it parses colours and strokes into toolkit types. What
//! is here needs nothing but the schema.
//!
//! Keeping this beside the schema rather than beside the widgets is what stops
//! the three from drifting. The hard-won rules here -- that `str` without
//! `content` is a named variant, that an empty array parses but does not compile,
//! that zero is not a neutral seed -- were each paid for once.

use crate::schema::ParamSchema;

/// Which control a parameter got, and why. Surfaced in the UI so the long tail
/// is visibly the long tail rather than silently broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Number,
    Length,
    Toggle,
    Enum,
    Color,
    Stroke,
    Mark,
    Scale,
    Alignment,
    /// Words: content `[..]` or a string `".."`, edited as plain text and
    /// written back in whichever shape it came in.
    Text,
    /// A number or an array of them; which one is decided by the current value.
    NumberOrArray,
    /// A colour ramp for a field: `color.map.viridis`, or an array of colours.
    Colormap,
    /// The palette a diagram draws its series from.
    Cycle,
    /// The value is a sentinel (`none`/`auto`) and this kind of control has no way
    /// to *show* that -- a slider at zero would claim a value the document does
    /// not have. So it says "not set" and offers one press to start from the
    /// documented default.
    Unset,
    /// Editable as raw Typst source, validated before it is applied.
    Source,
    /// Not editable here: bound, computed, or generated.
    ReadOnly,
}
/// The schema's `widget` string to a control. `None` means the schema has grown
/// a kind this frontend does not know about -- a test asserts that never
/// happens, so a lilaq release that adds a type family fails loudly instead of
/// quietly rendering text boxes.
pub fn widget_control(widget: &str) -> Option<Control> {
    Some(match widget {
        "number" | "integer" => Control::Number,
        "length" | "relative" | "ratio" | "angle" | "coordinate" => Control::Length,
        "toggle" => Control::Toggle,
        "enum" => Control::Enum,
        "color" | "paint" => Control::Color,
        "stroke" => Control::Stroke,
        "mark" => Control::Mark,
        "scale" => Control::Scale,
        "alignment" => Control::Alignment,
        // `content` writes `[..]`; `text` is lilaq's plain `str`, which writes
        // `".."`. One control, because `Control::Text` already keeps whichever
        // shape is written -- but the *seed* differs, and getting that wrong is
        // how `xscale` was once handed `[]`.
        "content" | "text" => Control::Text,
        "number-or-array" => Control::NumberOrArray,
        "colormap" => Control::Colormap,
        "cycle" => Control::Cycle,
        // Deliberately the source editor: unions too wide to map, or structures
        // with no small form.
        "array" | "data" | "dictionary" | "structured" | "variant" | "opaque" => Control::Source,
        _ => return None,
    })
}
/// Is this value one of typst's ways of saying "not set"?
///
/// `none` and `auto`, and only where the schema says the parameter accepts them
/// -- 140 of lilaq's 409 parameters do, and their documented default is usually
/// one of the two. Returns the sentinel as written, for showing in the UI.
///
/// This is what keeps an unset parameter out of the raw source editor. Every
/// typed control used to fall back to that when its parser did not recognise the
/// value, and no parser recognises `none` -- so adding any of those 140 dropped
/// the user into a text box to type syntax lilook already knew the shape of.
pub fn sentinel_of<'a>(text: &str, param: Option<&'a ParamSchema>) -> Option<&'a str> {
    let text = text.trim();
    param?
        .sentinels
        .iter()
        .find(|s| s.as_str() == text)
        .map(String::as_str)
}
/// What to write when the user presses `set` on an unset parameter, if lilook can
/// name a value it is sure of.
///
/// `None` means it cannot, and the row offers a source editor instead. That
/// distinction is the point: the value has to **compile**, not merely reparse. An
/// empty array `()` parses fine and lilaq rejects it -- "Limit arrays must contain
/// exactly two items" -- so `xlim` gets no seed, because nothing here knows which
/// two numbers the user wants. That one shipped to a browser before it was caught.
///
/// The documented default comes first where it is a real value, because then
/// pressing `set` changes nothing about how the figure looks and simply makes the
/// control editable.
pub fn seed(param: Option<&ParamSchema>, control: Control) -> Option<String> {
    let default = param
        .and_then(|p| p.default.as_deref())
        .filter(|d| sentinel_of(d, param).is_none() && crate::check_expr(d).is_ok());
    if let Some(d) = default {
        return Some(d.to_string());
    }
    match control {
        // `1`, not `0`: zero is only neutral for an offset, and it is invalid for
        // everything else a bare number means here. `aspect-ratio: 0` compiled to
        // "cannot divide by zero". One is valid wherever a positive number is
        // wanted and harmless where an offset is.
        Control::Number | Control::NumberOrArray => Some("1".into()),
        Control::Length => Some("1cm".into()),
        Control::Toggle => Some("true".into()),
        Control::Color => Some("red".into()),
        Control::Stroke => Some("1pt".into()),
        Control::Alignment => Some("center".into()),
        // `[]` only where content is actually a type. lilaq's `load-txt` takes
        // real free-text strings -- `delimiter: ","`, `comments: "#"` -- and
        // seeding those with content would be rejected the way `xscale: []` was.
        Control::Text if takes_text(param) => Some("[]".into()),
        Control::Text => Some("\"\"".into()),
        // The defaults lilaq itself uses, so pressing `set` changes nothing about
        // the figure and simply makes the control editable.
        Control::Colormap => Some("color.map.viridis".into()),
        Control::Cycle => Some("petroff10".into()),
        Control::Enum | Control::Mark | Control::Scale => param
            .and_then(|p| p.choices.first().cloned())
            .map(|c| format!("\"{c}\"")),
        // An array, a dictionary, a structure: lilook knows the shape but not the
        // contents, and a wrong guess is a broken figure. The source editor takes
        // it, with the shape shown as a hint.
        Control::Source | Control::ReadOnly | Control::Unset => None,
    }
}
/// The shape a parameter wants, as placeholder text, so an unset structure is a
/// prompt rather than a puzzle.
pub fn shape_hint(param: Option<&ParamSchema>) -> &'static str {
    match param.map(|p| p.widget.as_str()) {
        Some("array") | Some("data") | Some("number-or-array") => "(0, 10)",
        Some("dictionary") | Some("structured") => "(key: value)",
        _ => "",
    }
}
/// A name from a menu, written the way typst wants it: a sentinel is a keyword,
/// everything else is a string.
pub fn quoted_choice(choice: &str, sentinels: &[String]) -> String {
    if sentinels.iter().any(|s| s == choice) {
        choice.to_string()
    } else {
        format!("\"{choice}\"")
    }
}
/// `seed`, for the test that checks every parameter's seed value reparses.
pub fn seed_for_test(param: Option<&ParamSchema>, control: Control) -> Option<String> {
    seed(param, control)
}
/// The sentinel to write when a control is cleared, if the parameter has one.
pub fn first_sentinel(param: Option<&ParamSchema>) -> Option<&str> {
    param?.sentinels.first().map(String::as_str)
}
/// Does this parameter take words the user writes?
///
/// `content` is the test, deliberately -- **not** `str`. In lilaq's schema a `str`
/// without a `content` alongside it is always a *named variant*: `xscale: "log"`,
/// `mark: "o"`, which the scale and mark menus already offer by name. Treating
/// those as free text seeded `xscale` with `[]` and lilaq rejected it: "expected
/// auto, string or dictionary, found content". A string a user types is still
/// edited as words -- `refine` reaches `Text` for one that is already written --
/// but an unset parameter only gets a text field when content is a type it takes.
pub fn takes_text(param: Option<&ParamSchema>) -> bool {
    param.is_some_and(|p| p.widget == "content" || p.types.iter().any(|t| t == "content"))
}
/// The schema's answer, before the value is looked at. Pass it through
/// [`refine`] to get the control the inspector actually renders.
pub fn control_for(param: Option<&ParamSchema>) -> Control {
    param
        .and_then(|p| widget_control(&p.widget))
        .unwrap_or(Control::Source)
}
/// A value to offer when the schema has no usable default.
pub fn placeholder(widget: &str) -> String {
    match widget_control(widget) {
        Some(Control::Number) | Some(Control::NumberOrArray) => "0".into(),
        Some(Control::Length) => "0pt".into(),
        Some(Control::Toggle) => "false".into(),
        Some(Control::Color) | Some(Control::Stroke) => "black".into(),
        Some(Control::Text) if widget == "content" => "[]".into(),
        Some(Control::Text) => r#""""#.into(),
        _ => "none".into(),
    }
}

/// typst's named colour maps, in the order a chooser should list them.
///
/// Perceptually uniform first -- viridis and its family are the ones that do not
/// invent structure that is not in the data, which is why they are the default
/// here and in most of science. The diverging and categorical ones follow, and
/// `rainbow` is last with a warning attached rather than omitted: people ask for
/// it, and refusing to offer it just means they type it by hand.
pub const COLORMAPS: &[(&str, &str)] = &[
    ("viridis", "perceptually uniform — the safe default"),
    ("magma", "perceptually uniform, dark"),
    ("inferno", "perceptually uniform, warm"),
    ("plasma", "perceptually uniform, bright"),
    ("rocket", "perceptually uniform, red"),
    ("mako", "perceptually uniform, blue-green"),
    ("turbo", "high contrast, not uniform"),
    ("crest", "sequential, blue-green"),
    ("flare", "sequential, warm"),
    ("vlag", "diverging, blue to red"),
    ("icefire", "diverging, dark centre"),
    ("spectral", "diverging, full hue range"),
    ("rainbow", "not perceptually uniform — invents banding"),
];

/// Palettes for a diagram's series, as `(name, expression, note)`.
///
/// `cycle` decides what *every* series in a diagram looks like, so it is the
/// largest single lever on how a figure reads. lilaq's own default leads; the
/// rest are here because a scientific figure has two audiences it is easy to
/// forget -- readers who cannot distinguish red from green, and readers holding a
/// greyscale printout.
pub const CYCLES: &[(&str, &str, &str)] = &[
    (
        "petroff10",
        "petroff10",
        "lilaq's default — 10 distinct hues",
    ),
    ("petroff6", "petroff6", "6 hues, for fewer series"),
    ("petroff8", "petroff8", "8 hues"),
    (
        "Okabe–Ito",
        r##"(rgb("#e69f00"), rgb("#56b4e9"), rgb("#009e73"), rgb("#f0e442"), rgb("#0072b2"), rgb("#d55e00"), rgb("#cc79a7"), rgb("#000000"))"##,
        "colourblind-safe, the standard 8",
    ),
    (
        "Tol bright",
        r##"(rgb("#4477aa"), rgb("#ee6677"), rgb("#228833"), rgb("#ccbb44"), rgb("#66ccee"), rgb("#aa3377"), rgb("#bbbbbb"))"##,
        "colourblind-safe, lighter",
    ),
    (
        "greyscale",
        r##"(luma(0%), luma(35%), luma(60%), luma(80%))"##,
        "survives a black-and-white printout",
    ),
];

/// Figure widths journals actually ask for, as `(label, width, note)`.
///
/// The number that matters is not "how big do I want this" but "how wide is the
/// column it goes in" -- get it wrong and the figure is rescaled on import, which
/// is how a paper ends up with 6 pt axis labels. Heights are left alone: the
/// aspect ratio is the author's.
pub const FIGURE_WIDTHS: &[(&str, &str, &str)] = &[
    (
        "one column",
        "88mm",
        "most journals — Nature, Science, MNRAS",
    ),
    ("1.5 column", "120mm", "where a wider figure is allowed"),
    ("two column", "180mm", "full width, across the page"),
    ("AAS one column", "3.5in", "AAS journals in inches"),
    ("AAS two column", "7.3in", "AAS full width"),
    ("slide", "160mm", "16:9 presentation, not print"),
];

/// Type sizes for a figure, as `(label, size, note)`.
///
/// A figure's text is set relative to the page it is drawn on, not the paper it
/// lands in, so it is the one thing that reliably comes out wrong. Journals ask
/// for 5-7 pt in the final figure; these are the sizes that survive.
pub const FIGURE_TEXT_SIZES: &[(&str, &str, &str)] = &[
    ("7pt", "7pt", "comfortable in a one-column figure"),
    ("8pt", "8pt", "the usual default"),
    ("9pt", "9pt", "a wide figure, or a slide"),
    ("6pt", "6pt", "dense figures — check it is still legible"),
    ("11pt", "11pt", "presentations"),
];

/// Marks lilaq registers by name, from `lilaq 0.6.0 src/model/mark.typ:157`.
/// Order is the source's, so the list reads the way the docs do.
pub const MARK_NAMES: &[&str] = &[
    "o", "s", "d", "x", "+", "*", ".", ",", "|", "-", "^", "v", "<", ">", "star", "moon",
    "polygon", "asterisk", "a3", "a4", "a5", "a6", "p5", "p6", "p7", "p8", "s3", "s4", "s5", "s6",
];
/// Built-in scale names, from the `axis.scale` documentation.
pub const SCALE_NAMES: &[&str] = &["linear", "log", "symlog", "datetime"];

/// Typst's named dash patterns.
pub const DASH_NAMES: &[&str] = &[
    "solid",
    "dotted",
    "densely-dotted",
    "loosely-dotted",
    "dashed",
    "densely-dashed",
    "loosely-dashed",
    "dash-dotted",
    "densely-dash-dotted",
    "loosely-dash-dotted",
];
