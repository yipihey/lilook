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

/// How many offers a popup shows at once.
///
/// Twelve fits under a caret without covering the code being typed, and a longer
/// list is answered by typing another letter rather than by scrolling.
pub const LIMIT: usize = 12;

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
}

impl<'a> Offer<'a> {
    pub fn new(label: &'a str, note: &'a str, value: &'a str) -> Self {
        Offer {
            label,
            note,
            value,
            hint: "",
        }
    }

    pub fn hint(mut self, hint: &'a str) -> Self {
        self.hint = hint;
        self
    }
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

/// The offers worth showing for what has been typed, capped at [`LIMIT`].
pub fn matching<'a, T>(items: &'a [T], typed: &str, label: impl Fn(&T) -> &str) -> Vec<&'a T> {
    items
        .iter()
        .filter(|it| matches(label(it), typed))
        .take(LIMIT)
        .collect()
}

/// The completion popup, anchored where the choice is being made. Returns the
/// index accepted, if one was.
///
/// One click is the whole interaction: an offer carries its value, so accepting
/// it writes `interpolation: "smooth"` rather than leaving a name to be filled in.
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
) -> Option<usize> {
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
                for (i, o) in offers.iter().enumerate() {
                    // Derived from the popup's own id, so a test can find a row
                    // and click a chosen part of it -- the picture, say.
                    let row_id = id.with(("row", i));
                    let r = click_row(ui, row_id, false, |ui| {
                        preview(ui, o.value);
                        ui.label(o.label);
                        if !o.note.is_empty() {
                            ui.weak(o.note);
                        }
                    });
                    let r = match o.hint.is_empty() {
                        true => r,
                        false => r.on_hover_text(o.hint),
                    };
                    if r.clicked() {
                        accepted = Some(i);
                    }
                }
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

/// A whole row as one click target, hover highlight included.
///
/// Whatever `contents` lays out -- a colour ramp, a palette, a name, a note --
/// shares one response. Nothing in a row is scenery: if it is in the row, it
/// selects the row.
pub fn click_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    selected: bool,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    // Reserved before the contents are laid out, because the highlight has to be
    // painted *under* them and egui paints in call order.
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = ui.horizontal(|ui| {
        // A label is draggable text by default, and text that eats the pointer in
        // the middle of a row is a row with a hole in it: the click lands on the
        // name, the name starts a selection, and nothing is chosen.
        ui.style_mut().interaction.selectable_labels = false;
        contents(ui)
    });

    // Full width, so there is no dead strip to the right of a short name.
    let mut rect = inner.response.rect;
    rect.max.x = rect.max.x.max(ui.max_rect().right());
    let rect = rect.expand2(egui::vec2(2.0, 1.0));

    let r = ui.interact(rect, id, egui::Sense::click());
    if selected || r.hovered() {
        let visuals = ui.style().interact_selectable(&r, selected);
        ui.painter().set(
            bg,
            egui::Shape::rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill),
        );
    }
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
    let stops = colormap_stops(map);
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
    let Some(inner) = expr
        .trim()
        .strip_prefix('(')
        .and_then(|e| e.strip_suffix(')'))
    else {
        return vec![];
    };
    inner
        .split(',')
        .filter_map(|part| parse_color(part.trim()))
        .collect()
}

fn hex(s: &str) -> Color32 {
    let v = u32::from_str_radix(s, 16).unwrap_or(0);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}
