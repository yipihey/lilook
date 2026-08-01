//! Choosing one thing from a list -- the popup, the rows, and the little
//! pictures that go in them.
//!
//! There is one implementation because there were two. The source pane's
//! completion popup offered a whole argument -- `interpolation: "smooth"`, name
//! and value -- for one click; the inspector's "add argument" combo made the
//! user pick a name, press `add`, and *then* find the control and set the value.
//! Same job, same list, two behaviours. So the popup lives here and both panes
//! call it, and a divergence now has to be written on purpose.
//!
//! Which was worth doing for a second reason: the popup that offered the better
//! interaction could not actually be clicked (see [`popup`]), and one of the two
//! had to be right before either could be shared.
//!
//! The other half is [`click_row`]. `selectable_label` senses only the text it
//! painted, which is why the colour ramp beside a colormap's name was inert: the
//! biggest thing in the row, the one that actually says what the choice *is*, and
//! aiming at it did nothing. Every row here is one target, picture included.

use egui::Color32;

use crate::value::parse_color;

/// How tall a popup gets before it scrolls, in points -- about a dozen rows,
/// which fits under a caret without covering the code being typed.
///
/// A *height*, not a count. It was a count, capped at twelve, and the twelve
/// were taken off the front of a list that runs name, then values, then the next
/// name: on `lq.colormesh` the cut fell exactly between `interpolation` and its
/// `pixelated` and `smooth`. The row you could see wrote the first value
/// whatever you aimed at, and the rows that wrote the other one were not on
/// screen to be aimed at. Everything that matches is in the list now, and the
/// list scrolls.
pub const MAX_HEIGHT: f32 = 260.0;

/// How wide a popup is, in points. Fixed, so rows are a stable target and the
/// list does not resize under the pointer as the filter narrows it.
pub const WIDTH: f32 = 320.0;

/// One row of a popup: what to show, what to say beside it, and the value it
/// stands for -- the last only so a picture can be painted where one helps.
#[derive(Debug, Clone, Copy, Default)]
pub struct Offer<'a> {
    pub label: &'a str,
    pub note: &'a str,
    pub value: &'a str,
    /// A sentence for the hover, where the caller has one.
    pub hint: &'a str,
    /// Values this row can write instead of its own, each shown as its own
    /// target on the same line. Picking one comes back as [`Picked::choice`].
    pub choices: &'a [String],
}

impl<'a> Offer<'a> {
    pub fn new(label: &'a str, note: &'a str, value: &'a str) -> Self {
        Offer {
            label,
            note,
            value,
            hint: "",
            choices: &[],
        }
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }

    pub fn choices(mut self, choices: &'a [String]) -> Self {
        self.choices = choices;
        self
    }
}

/// What was taken from a popup: a row, and which of its named values -- if the
/// pointer was on one rather than on the row itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picked {
    pub row: usize,
    pub choice: Option<usize>,
}

/// Does what has been typed select this label?
///
/// A prefix of the label, or of any word in it, so `smo` reaches
/// `interpolation: "smooth"` -- the value is what the user is thinking of, and
/// requiring them to remember which parameter carries it is requiring them to
/// know the answer before they ask.
pub fn matches(label: &str, typed: &str) -> bool {
    let typed = typed.trim().to_ascii_lowercase();
    if typed.is_empty() {
        return true;
    }
    let label = label.to_ascii_lowercase();
    label.starts_with(&typed)
        || label
            .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .any(|w| !w.is_empty() && w.starts_with(&typed))
}

/// What a row is searched by: its label and every value it names, so typing
/// `smo` finds the `interpolation` row that carries `smooth`.
///
/// Uncapped on purpose, wherever this is used to filter: the popup bounds its
/// *height* and scrolls, so nothing is offered-but-unreachable.
pub fn haystack<'a>(label: &str, choices: impl Iterator<Item = &'a str>) -> String {
    let mut out = label.to_string();
    for c in choices {
        out.push(' ');
        out.push_str(c);
    }
    out
}

/// The completion popup, anchored where the choice is being made. Returns what
/// was taken, if anything.
///
/// One click is the whole interaction: an offer carries its value, so accepting
/// it writes `interpolation: "smooth"` rather than leaving a name to be filled
/// in. Where a row names its values, each is its own target and the answer says
/// which one -- clicking the word `smooth` must not write `pixelated`.
///
/// `open` is the caller's gate -- a caret in the buffer, focus in the add field
/// -- and it is deliberately not the only one. A click is a press in one frame
/// and a release in the next, and egui surrenders a text field's focus in
/// between: the moment the pointer clicks anywhere that is not the field, the
/// popup included. Honouring `open` alone would take the popup down between the
/// press and the release, and *every row in it would be unclickable*. So a popup
/// the pointer is inside stays up, and only a stale one -- not drawn last pass --
/// is ignored.
pub fn popup(
    ctx: &egui::Context,
    id: egui::Id,
    anchor: egui::Pos2,
    offers: &[Offer<'_>],
    open: bool,
) -> Option<Picked> {
    let shown = id.with("shown");
    let pass = ctx.cumulative_pass_nr();
    let held = ctx
        .data(|d| d.get_temp::<(u64, egui::Rect)>(shown))
        .is_some_and(|(nr, rect)| {
            nr + 1 >= pass
                && ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| rect.contains(p)))
        });
    if offers.is_empty() || !(open || held) {
        ctx.data_mut(|d| d.remove::<(u64, egui::Rect)>(shown));
        return None;
    }
    let mut accepted = None;
    let area = egui::Area::new(id)
        .fixed_pos(anchor)
        .order(egui::Order::Foreground)
        // Kept on screen: anchored at the caret on the last line of the source
        // pane, an unconstrained popup opens below the window edge.
        .constrain(true)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                // One width, not a content-shaped one. A row is a click target as
                // wide as the popup looks, and an `Area` sized to its longest
                // label leaves the rest of every other row hanging outside the
                // frame -- painted as part of the row, hit-tested as background.
                ui.set_min_width(WIDTH);
                ui.set_max_width(WIDTH);
                egui::ScrollArea::vertical()
                    .max_height(MAX_HEIGHT)
                    .show(ui, |ui| {
                        for (i, o) in offers.iter().enumerate() {
                            // Derived from the popup's own id, so a test can find
                            // a row, or a value inside one, and click it.
                            let row_id = id.with(("row", i));
                            let mut chosen = None;
                            let r = click_row(ui, row_id, false, |ui| {
                                preview(ui, o.value);
                                ui.label(o.label);
                                if !o.note.is_empty() {
                                    ui.weak(o.note);
                                }
                                for (k, c) in o.choices.iter().enumerate() {
                                    if chip(ui, row_id.with(("choice", k)), c).clicked() {
                                        chosen = Some(k);
                                    }
                                }
                            });
                            let r = match o.hint.is_empty() {
                                true => r,
                                false => r.on_hover_text(o.hint),
                            };
                            // A value the pointer was actually on wins over the
                            // row that contains it.
                            if chosen.is_some() {
                                accepted = Some(Picked {
                                    row: i,
                                    choice: chosen,
                                });
                            } else if r.clicked() {
                                accepted = Some(Picked {
                                    row: i,
                                    choice: None,
                                });
                            }
                        }
                    });
            });
        });
    match accepted {
        // Taking an offer closes the popup: the next frame must not still be
        // offering the thing that was just written.
        Some(_) => ctx.data_mut(|d| d.remove::<(u64, egui::Rect)>(shown)),
        None => ctx.data_mut(|d| {
            d.insert_temp(shown, (pass, area.response.rect));
        }),
    }
    accepted
}

/// A whole row as one click target, hover highlight included -- and anything
/// inside it may be a target of its own.
///
/// Whatever `contents` lays out -- a colour ramp, a palette, a name, a note --
/// belongs to the row's response, so nothing in a row is scenery. But a widget
/// the contents create keeps its own click: the row is claimed **before** the
/// contents are laid out, so egui hit-tests the later, inner widget first. That
/// ordering is the whole reason this is not `ui.horizontal` followed by an
/// `interact` -- that way round, the row swallowed every value chip in it.
pub fn click_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    selected: bool,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    let height = ui.spacing().interact_size.y.max(18.0);
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), height));
    let r = ui.interact(rect, id, egui::Sense::click());
    if selected || r.hovered() {
        let visuals = ui.style().interact_selectable(&r, selected);
        ui.painter().rect_filled(
            rect.expand2(egui::vec2(2.0, 1.0)),
            visuals.corner_radius,
            visuals.weak_bg_fill,
        );
    }
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(3.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            // A label is draggable text by default, and text that eats the
            // pointer in the middle of a row is a row with a hole in it: the
            // click lands on the name, the name starts a selection, and nothing
            // is chosen.
            ui.style_mut().interaction.selectable_labels = false;
            contents(ui);
        },
    );
    r
}

/// One named value, sitting on its parameter's line and answering for itself.
///
/// Small and quiet until the pointer is on it, because a row of these is meant
/// to read as *one* line -- `interpolation  pixelated smooth` -- rather than as
/// a toolbar.
pub fn chip(ui: &mut egui::Ui, id: egui::Id, text: &str) -> egui::Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, egui::Color32::PLACEHOLDER);
    let pad = egui::vec2(5.0, 1.0);
    let (_, rect) = ui.allocate_space(galley.size() + pad * 2.0);
    let r = ui.interact(rect, id, egui::Sense::click());
    let visuals = ui.style().interact(&r);
    if r.hovered() {
        ui.painter()
            .rect_filled(rect, visuals.corner_radius, visuals.bg_fill);
    }
    ui.painter()
        .galley(rect.min + pad, galley, visuals.text_color());
    r
}

/// The picture a value is worth, where lilook can paint one.
///
/// Driven by the value rather than by the parameter, so the same rule serves the
/// colormap menu, the cycle menu and both popups: anything that reads as a named
/// colour map gets its ramp, anything that reads as a palette gets its swatches,
/// and everything else takes no space at all.
pub fn preview(ui: &mut egui::Ui, value: &str) {
    // A completion carries `name: value`; a picker carries the value alone.
    let value = value.split_once(':').map_or(value, |(_, v)| v).trim();
    if let Some(map) = value.rsplit_once("color.map.").map(|(_, m)| m) {
        let map: String = map
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '-')
            .collect();
        if !colormap_stops(&map).is_empty() {
            ramp(ui, &map);
            return;
        }
    }
    if !cycle_colors(value).is_empty() {
        swatches(ui, value);
    }
}

/// A colour-map preview strip.
///
/// The stops are typst's own, sampled coarsely: enough to tell viridis from
/// magma at a glance, which is all a chooser needs. Painted rather than
/// described, because "perceptually uniform, warm" is not a thing anyone can
/// picture and a two-centimetre gradient is.
pub fn ramp(ui: &mut egui::Ui, map: &str) {
    let mut stops = colormap_stops(map_name(map));
    if is_reversed(map) {
        stops.reverse();
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(48.0, 12.0), egui::Sense::hover());
    if stops.is_empty() {
        return;
    }
    let n = stops.len();
    let w = rect.width() / n as f32;
    for (i, c) in stops.iter().enumerate() {
        let x = rect.left() + i as f32 * w;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(w + 0.5, rect.height()),
            ),
            0.0,
            *c,
        );
    }
}

/// A palette preview: one square per colour.
pub fn swatches(ui: &mut egui::Ui, expr: &str) {
    let colors = cycle_colors(expr);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(56.0, 12.0), egui::Sense::hover());
    if colors.is_empty() {
        return;
    }
    let w = rect.width() / colors.len() as f32;
    for (i, c) in colors.iter().enumerate() {
        let x = rect.left() + i as f32 * w;
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(x, rect.top()),
                egui::vec2(w - 1.0, rect.height()),
            ),
            1.0,
            *c,
        );
    }
}

/// Five stops per map, eyeballed from typst's own gradients.
///
/// Approximate on purpose: this is a preview, and carrying the real 256-entry
/// tables would be kilobytes of data to answer a question the eye settles in a
/// glance. The figure itself is drawn by typst from the true map.
pub fn colormap_stops(map: &str) -> Vec<Color32> {
    let stops: &[&str] = match map {
        "viridis" => &["440154", "3b528b", "21918c", "5ec962", "fde725"],
        "magma" => &["000004", "3b0f70", "8c2981", "de4968", "fcfdbf"],
        "inferno" => &["000004", "420a68", "932667", "dd513a", "fcffa4"],
        "plasma" => &["0d0887", "6a00a8", "b12a90", "e16462", "f0f921"],
        "rocket" => &["03051a", "541f3f", "a41e50", "e05c3a", "faebdd"],
        "mako" => &["0b0405", "382a54", "3e6d8a", "3ebcaa", "def5e5"],
        "turbo" => &["30123b", "1fa8d8", "a5fd3d", "fb8022", "7a0403"],
        "crest" => &["a5cd90", "5aa96f", "24837b", "1f5f8b", "39366a"],
        "flare" => &["edb081", "e7876f", "d75c68", "b13e64", "7d1d67"],
        "vlag" => &["2369bd", "8fb9d8", "f2f2f2", "d99c92", "a11a2b"],
        "icefire" => &["bde7f0", "4a86b8", "191a1a", "b8452e", "f0d9a8"],
        "spectral" => &["9e0142", "f98e52", "ffffbf", "88cfa4", "5e4fa2"],
        "rainbow" => &["6e40aa", "1ab0d0", "8fea52", "ff8c38", "d9335a"],
        _ => &[],
    };
    stops.iter().map(|s| hex(s)).collect()
}

/// The colours of a cycle, for its swatch row.
///
/// Read out of the expression itself, which for every palette lilook offers is a
/// literal array -- lilaq's own included, because `cycle` takes an array and the
/// package exports no name for one. There is no second table of colours here to
/// drift from the first.
///
/// The array's own parentheses come off before the split, which they did not
/// before: `(rgb("#e69f00"), .., rgb("#000000"))` split on commas leaves the
/// first and last entries carrying a bracket, neither parses, and Okabe-Ito
/// showed six of its eight colours.
pub fn cycle_colors(expr: &str) -> Vec<Color32> {
    cycle_parts(expr)
        .iter()
        .filter_map(|p| parse_color(p))
        .collect()
}

/// The elements of a palette, as the source text of each -- which is what an
/// editor needs: `rgb("#4477aa")` must come back out the way it went in, not as
/// whatever a colour picker would print for the same pixels.
pub fn cycle_parts(expr: &str) -> Vec<String> {
    let Some(inner) = expr
        .trim()
        .strip_prefix('(')
        .and_then(|e| e.strip_suffix(')'))
    else {
        return vec![];
    };
    inner
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// The map a value names, for looking its preview up: `color.map.viridis` is
/// `viridis`, and anything else is itself. A reversed map is still that map --
/// `.rev()` changes the direction, not the colours.
pub fn map_name(value: &str) -> &str {
    let value = value.trim();
    let value = value.strip_suffix(REVERSE).unwrap_or(value);
    value
        .rsplit_once("color.map.")
        .map(|(_, n)| n)
        .unwrap_or(value)
}

/// How typst reverses an array, which is what a colour map is.
///
/// Written as a suffix rather than as stops of lilook's own: `color.map.viridis`
/// reversed this way is *exactly* viridis backwards, and lilook only ever had a
/// five-stop sketch of it. The same suffix works on a map of the user's own,
/// where the colours are theirs to begin with.
pub const REVERSE: &str = ".rev()";

/// Is this value a map read backwards?
pub fn is_reversed(value: &str) -> bool {
    value.trim().ends_with(REVERSE)
}

/// The same map, the other way round. Toggles, so it is its own undo.
pub fn reverse(value: &str) -> String {
    let value = value.trim();
    match value.strip_suffix(REVERSE) {
        Some(plain) => plain.to_string(),
        None => format!("{value}{REVERSE}"),
    }
}

/// A colour as typst source, for an editor that starts from painted stops
/// rather than from text.
pub fn color_source_of(c: &Color32) -> String {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    format!("rgb(\"#{r:02x}{g:02x}{b:02x}\")")
}

/// A ramp painted from stops given as source text, for a map being edited.
///
/// Interpolated, unlike [`swatches`]: a colour map is read as a gradient between
/// its entries, and drawing it as blocks would show something the figure will
/// not do.
pub fn ramp_of(ui: &mut egui::Ui, parts: &[String]) {
    let stops: Vec<Color32> = parts.iter().filter_map(|p| parse_color(p)).collect();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 12.0), egui::Sense::hover());
    if stops.len() < 2 {
        if let Some(c) = stops.first() {
            ui.painter().rect_filled(rect, 0.0, *c);
        }
        return;
    }
    // One thin column per pixel of width, so the eye sees a gradient rather than
    // the handful of stops it was built from.
    let steps = rect.width().round().max(1.0) as usize;
    let w = rect.width() / steps as f32;
    for i in 0..steps {
        let t = i as f32 / (steps - 1).max(1) as f32 * (stops.len() - 1) as f32;
        let lo = t.floor() as usize;
        let hi = (lo + 1).min(stops.len() - 1);
        let c = stops[lo].lerp_to_gamma(stops[hi], t - lo as f32);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(
                egui::pos2(rect.left() + i as f32 * w, rect.top()),
                egui::vec2(w + 0.5, rect.height()),
            ),
            0.0,
            c,
        );
    }
}

/// A palette written back as typst: an array, and a one-element array in typst
/// keeps its comma -- `(red)` is a colour in brackets, `(red,)` is a list of one.
pub fn cycle_array(parts: &[String]) -> String {
    match parts.len() {
        1 => format!("({},)", parts[0]),
        _ => format!("({})", parts.join(", ")),
    }
}

fn hex(s: &str) -> Color32 {
    let v = u32::from_str_radix(s, 16).unwrap_or(0);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}
