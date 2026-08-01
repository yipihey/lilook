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
    if let Some(d) = usable_default(param).filter(|d| sentinel_of(d, param).is_none()) {
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
        // The first palette in the table, which is lilaq's own -- written out as
        // the array `cycle` actually takes. It used to write the bare name
        // `petroff10`, which is a binding inside the package: "unknown variable"
        // in the user's document. `cycle` has no sentinel, so the gate that
        // covers `set` never tried it and the one for adding an argument did.
        Control::Cycle => CYCLES.first().map(|(_, expr, _)| cycle_source(expr)),
        Control::Enum | Control::Mark | Control::Scale => param
            .and_then(|p| p.choices.first().cloned())
            .map(|c| format!("\"{c}\"")),
        // An array, a dictionary, a structure: lilook knows the shape but not the
        // contents, and a wrong guess is a broken figure. The source editor takes
        // it, with the shape shown as a hint.
        Control::Source | Control::ReadOnly | Control::Unset => None,
    }
}
/// The documented default, where writing it into the user's document means what
/// it meant in lilaq's.
///
/// Two filters, and the second was paid for: the value has to parse, *and* it
/// has to name nothing that only lilaq can see. `diagram`'s cycle defaults to
/// `petroff10` -- a binding inside the package, syntactically a perfect
/// identifier -- and adding the argument wrote "unknown variable: petroff10"
/// into a figure. `check_expr` cannot see that; [`crate::resolves_anywhere`]
/// can.
fn usable_default(param: Option<&ParamSchema>) -> Option<&str> {
    param
        .and_then(|p| p.default.as_deref())
        .filter(|d| crate::check_expr(d).is_ok() && crate::resolves_anywhere(d))
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

/// One argument a call does not have yet, ready to be written in a single act.
///
/// Both places lilook offers to add an argument -- the source pane's completion
/// popup and the inspector's add field -- build their list from
/// [`argument_offers`], because two lists that disagree is worse than either one
/// alone: the same figure would grow a different `interpolation` depending on
/// which pane you were looking at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentOffer {
    /// The parameter this writes.
    pub param: String,
    /// What to show: the name alone, or `name: value` where a value is chosen.
    pub label: String,
    /// The value to write, where there is one lilook is sure of. `None` where it
    /// knows the shape but not the contents -- see [`seed`]. The source pane
    /// writes the name and leaves the caret after the colon; the inspector has
    /// no caret to leave, so it falls back to [`ArgumentOffer::written`].
    pub value: Option<String>,
    /// What to write instead when there is no value to be sure of: the
    /// documented default where it reparses, and the shape's placeholder
    /// otherwise. The default comes first even when it is `auto` or `none`,
    /// because for a `scale` or a `variant` that *is* the value -- and
    /// `xscale: none` is a figure lilaq refuses to draw.
    pub fallback: String,
    /// The types, or the parameter this value belongs to.
    pub note: String,
    /// One line of what the parameter is for, to be shown on hover. The old
    /// combo box had this and a list of bare names does not deserve to lose it:
    /// `bounds` and `margin` are not self-explanatory.
    pub doc: String,
}

impl ArgumentOffer {
    /// The value to write where an argument cannot be left half-finished.
    ///
    /// An inspector row is a control, not a caret: `xlim: ` is not something it
    /// can render, so the fallback stands in and the row's source editor shows
    /// the shape as a hint.
    pub fn written(&self) -> String {
        self.value.clone().unwrap_or_else(|| self.fallback.clone())
    }
}

/// Every argument this call is missing, each offered once with a safe value and
/// again per choice where it has a small fixed set.
///
/// The per-choice rows are what make adding an argument one act rather than
/// three: picking `interpolation: "smooth"` writes it, instead of writing
/// `interpolation: auto` and leaving the user to find the menu and the value.
///
/// Positional parameters are left out on purpose. typst refuses one passed by
/// name -- `lq.plot(x: xs)` is "unexpected argument: x" -- so offering it is
/// offering a broken figure.
pub fn argument_offers(params: &[ParamSchema], call: &crate::CallSite) -> Vec<ArgumentOffer> {
    let mut out = vec![];
    for p in params
        .iter()
        .filter(|p| p.kind != "positional" && !call.named.iter().any(|a| a.name == p.name))
    {
        let control = control_for(Some(p));
        let fallback = usable_default(Some(p))
            .map(str::to_string)
            .unwrap_or_else(|| placeholder(&p.widget));
        let doc = p
            .doc
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default()
            .trim()
            .to_string();
        out.push(ArgumentOffer {
            param: p.name.clone(),
            label: p.name.clone(),
            value: seed(Some(p), control),
            fallback: fallback.clone(),
            note: p.types.join("|"),
            doc: doc.clone(),
        });
        for (label, value) in values_for(p, control) {
            out.push(ArgumentOffer {
                param: p.name.clone(),
                label: format!("{}: {label}", p.name),
                value: Some(value),
                fallback: fallback.clone(),
                note: p.name.clone(),
                doc: doc.clone(),
            });
        }
    }
    out
}

/// The concrete values worth offering beside a parameter's name.
///
/// Only where the set is small and fixed: a scale, a colour map, a palette. A
/// length or a number has no useful list, and a menu of guesses is worse than a
/// field to type in.
fn values_for(p: &ParamSchema, control: Control) -> Vec<(String, String)> {
    let quoted = |names: &[&str]| -> Vec<(String, String)> {
        names
            .iter()
            .map(|n| ((*n).to_string(), format!("\"{n}\"")))
            .collect()
    };
    if !p.choices.is_empty() && p.choices.len() <= 12 {
        return quoted(&p.choices.iter().map(String::as_str).collect::<Vec<_>>());
    }
    match control {
        Control::Scale => quoted(SCALE_NAMES),
        Control::Colormap => COLORMAPS
            .iter()
            .map(|(m, _)| ((*m).to_string(), format!("color.map.{m}")))
            .collect(),
        Control::Cycle => CYCLES
            .iter()
            .map(|(n, expr, _)| ((*n).to_string(), cycle_source(expr)))
            .collect(),
        Control::Toggle => vec![
            ("true".into(), "true".into()),
            ("false".into(), "false".into()),
        ],
        _ => vec![],
    }
}

/// A cycle as typst wants it: an array of colours goes in as it is, anything
/// else is a name and a name is a string.
///
/// Every palette in [`CYCLES`] is an array, so this quotes nothing today. It
/// stays because the *value* it guards is the user's: a cycle written by hand
/// can be either, and the quoting rule is the one thing both panes must agree
/// on when they write one.
pub fn cycle_source(expr: &str) -> String {
    match expr.trim_start().starts_with('(') {
        true => expr.to_string(),
        false => format!("\"{expr}\""),
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
///
/// **Every expression is an array, including lilaq's own palettes.** `cycle`
/// takes a list of colours -- "expected array, found string" for anything else
/// -- and `petroff10` is a binding inside the package that its entry point does
/// not export, so there is no name a user's document could write. Offering one
/// wrote three palettes that no figure would compile with. The colours are
/// lilaq's, from `src/style/map.typ` in 0.6.0, so picking `petroff10` still
/// means exactly what lilaq means by it.
pub const CYCLES: &[(&str, &str, &str)] = &[
    (
        "petroff10",
        r##"(rgb("#3f90da"), rgb("#ffa90e"), rgb("#bd1f01"), rgb("#94a4a2"), rgb("#832db6"), rgb("#a96b59"), rgb("#e76300"), rgb("#b9ac70"), rgb("#717581"), rgb("#92dadd"))"##,
        "lilaq's default — 10 distinct hues",
    ),
    (
        "petroff6",
        r##"(rgb("#5790fc"), rgb("#f89c20"), rgb("#e42536"), rgb("#964a8b"), rgb("#9c9ca1"), rgb("#7a21dd"))"##,
        "6 hues, for fewer series",
    ),
    (
        "petroff8",
        r##"(rgb("#1845fb"), rgb("#ff5e02"), rgb("#c91f16"), rgb("#c849a9"), rgb("#adad7d"), rgb("#86c8dd"), rgb("#578dff"), rgb("#656364"))"##,
        "8 hues",
    ),
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

/// Typst expressions worth offering where a figure takes a value.
///
/// Not the standard library -- a menu of six hundred names is a worse answer
/// than none. These are what people actually write *inside a diagram*: arithmetic
/// on data, a range, a colour built rather than named.
///
/// A hand-kept table, deliberately. The alternative was `tinymist-query`, which
/// turned out to require a patched typst (see `docs/findings.md`), and thirty
/// names that never go stale beat a dependency that dictates the compiler.
pub const TYPST_HELPERS: &[(&str, &str, &str)] = &[
    ("calc.sqrt", "calc.sqrt(", "square root"),
    ("calc.pow", "calc.pow(", "x to the power y"),
    ("calc.exp", "calc.exp(", "e to the x"),
    ("calc.ln", "calc.ln(", "natural log"),
    ("calc.log", "calc.log(", "log base 10"),
    ("calc.sin", "calc.sin(", "sine, in radians"),
    ("calc.cos", "calc.cos(", "cosine, in radians"),
    ("calc.tan", "calc.tan(", "tangent"),
    ("calc.atan2", "calc.atan2(", "angle of a vector"),
    ("calc.abs", "calc.abs(", "absolute value"),
    ("calc.min", "calc.min(", "smallest of its arguments"),
    ("calc.max", "calc.max(", "largest of its arguments"),
    ("calc.round", "calc.round(", "to the nearest whole number"),
    ("calc.floor", "calc.floor(", "down"),
    ("calc.ceil", "calc.ceil(", "up"),
    ("calc.rem", "calc.rem(", "remainder"),
    ("calc.pi", "calc.pi", "π"),
    ("calc.e", "calc.e", "e"),
    ("calc.inf", "calc.inf", "infinity"),
    ("range", "range(", "0, 1, 2, … — an index axis"),
    ("float", "float(", "text to a number"),
    ("int", "int(", "to a whole number"),
    ("rgb", "rgb(\"#\")", "a colour by hex"),
    ("luma", "luma(50%)", "a grey"),
    ("cmyk", "cmyk(0%, 0%, 0%, 0%)", "a print colour"),
    (
        "gradient.linear",
        "gradient.linear(",
        "a colour ramp of your own",
    ),
];
