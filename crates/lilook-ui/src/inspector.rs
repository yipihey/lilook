//! Schema-driven inspector: one call site's arguments, with the control for
//! each parameter chosen by the generated schema.
//!
//! Every widget kind the schema emits has a control here, and a test fails if a
//! regenerated schema introduces one that does not. The long tail -- 32
//! `variant` and 29 `opaque` parameters whose types are too wide to map -- gets
//! a validating source editor rather than a blind text box, which is the honest
//! form of "not specialised yet": the value is still editable, and one that
//! would not reparse is refused before it reaches the buffer.

pub use lilook_core::{SlotSource, UiEvent};
// Which control a parameter wants is decided by the schema, and lives beside it
// in the core so that a Swift view and the MCP server get the same answer.
pub use lilook_core::policy::{
    control_for, first_sentinel, placeholder, quoted_choice, seed, seed_for_test, sentinel_of,
    shape_hint, takes_text, widget_control, Control,
};

use lilook_core::schema::{FunctionSchema, ParamSchema};
use lilook_core::{CallSite, Editability, NamedArg};

use crate::pick;
// The pictures and the popup are shared with the source pane, so a colormap
// reads the same wherever lilook offers one.
use crate::pick::{ramp, swatches};
use crate::value::{
    alignment_source, color_source, num, parse_alignment, parse_color, parse_stroke, parse_text,
    split_numeric, stroke_source, text_source, Stroke, TextShape, DASH_NAMES, H_ALIGN, MARK_NAMES,
    SCALE_NAMES, V_ALIGN,
};

/// What the shell knows and the inspector cannot work out for itself.
///
/// Borrowed rather than `Copy`: it carries one entry per data slot now, because
/// a series' x and y are linked independently and the inspector has to say so
/// for each of them separately.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// Points recovered for this call site's series, if it is one. Enables the
    /// materialise action on a computed data slot.
    pub recovered_points: Option<usize>,
    /// Where each data slot's numbers come from, indexed by slot. Per slot
    /// rather than per call because x and y link independently: x can come from
    /// one file's column and y from another's.
    pub slot_sources: &'a [SlotSource],
    /// The user's own palettes and colour maps, offered beside the built-in
    /// ones. Borrowed, and empty by default: an inspector with no library is the
    /// same inspector with a shorter menu.
    pub saved: &'a [lilook_core::Saved],
}

/// Adapt a control to the value actually written there.
///
/// Two jobs. The widest schema families admit several shapes, and which one the
/// user chose is visible in the source: `xlabel: [Time]` is content whatever
/// the type union says. And an expression the core calls opaque may still be a
/// literal a specialised control can round-trip -- `stroke: 1pt + red` parses
/// as a binary operation, but it is a stroke, and refusing to edit it would
/// make the commonest stroke in every lilaq document read-only.
///
/// The safety rule: reopen an opaque value only when a parser that writes the
/// same shape back recognises it. `stroke: my-style` and `color: accent` are
/// recognised by nothing here, so they stay read-only with a jump-to-definition.
pub fn refine(control: Control, editability: Editability, text: &str) -> Control {
    let opaque = matches!(editability, Editability::Binding | Editability::Opaque);
    if opaque {
        return match control {
            Control::Stroke if parse_stroke(text).is_some() => Control::Stroke,
            Control::Color if parse_color(text).is_some() => Control::Color,
            Control::Alignment if parse_alignment(text).is_some() => Control::Alignment,
            Control::Text if parse_text(text).is_some() => Control::Text,
            // A colormap and a cycle are always *chosen*, never edited in place:
            // the picker writes a whole expression over whatever was there. So an
            // opaque current value -- `color.map.viridis` is a field access, and
            // every one of them is -- says nothing about whether it can be
            // changed. Without this the most consequential control on a heatmap
            // rendered as a read-only label.
            Control::Colormap => Control::Colormap,
            Control::Cycle => Control::Cycle,
            _ => Control::ReadOnly,
        };
    }
    match control {
        Control::Source if parse_text(text).is_some() => Control::Text,
        Control::Source if parse_color(text).is_some() => Control::Color,
        Control::NumberOrArray if split_numeric(text).is_some() => Control::Number,
        Control::NumberOrArray => Control::Source,
        other => other,
    }
}
/// The control for a parameter, given what is currently written there.
///
/// Prefer this to `control_for` + `refine`: it is the one place that sees both
/// the schema and the value, which is what an unset parameter needs.
pub fn control_of(param: Option<&ParamSchema>, editability: Editability, text: &str) -> Control {
    let control = refine(control_for(param), editability, text);
    if sentinel_of(text, param).is_none() || control == Control::ReadOnly {
        return control;
    }
    // An unset value carries nothing to lose, so the *schema* decides rather than
    // the text. `title: none` where the schema admits words is a text field, not a
    // box to type `[..]` into.
    if takes_text(param) {
        return Control::Text;
    }
    match control {
        // These show the sentinel in their own list, so they can say "not set"
        // and go back to it without help.
        Control::Enum | Control::Mark | Control::Scale => control,
        _ => Control::Unset,
    }
}

pub struct Inspector<'a> {
    pub schema: Option<&'a FunctionSchema>,
    /// The call's parameters *including the ones it forwards*.
    ///
    /// `lq.colorbar` takes `..args` "to pass to @diagram", so `width` and
    /// `height` are as settable there as on the diagram -- and were invisible,
    /// because the schema lists the sink rather than what it forwards to. The
    /// editor supplies this because it holds the whole schema; the inspector
    /// holds one function.
    pub effective: Vec<ParamSchema>,
    pub events: Vec<UiEvent>,
    pub context: Context<'a>,
}

impl<'a> Inspector<'a> {
    pub fn new(schema: Option<&'a FunctionSchema>) -> Self {
        Inspector {
            schema,
            effective: vec![],
            events: vec![],
            context: Context::default(),
        }
    }

    /// Supply the forwarded parameters, which only a holder of the whole schema
    /// can work out.
    pub fn with_effective(mut self, params: Vec<ParamSchema>) -> Self {
        self.effective = params;
        self
    }

    pub fn with_context(mut self, context: Context<'a>) -> Self {
        self.context = context;
        self
    }

    fn param(&self, name: &str) -> Option<&ParamSchema> {
        self.schema?.params.iter().find(|p| p.name == name)
    }

    /// Render one call site's arguments. Returns the events produced.
    pub fn ui(&mut self, ui: &mut egui::Ui, call: &CallSite) {
        let heading = if call.generated {
            format!("{}  (generated)", call.callee)
        } else {
            call.callee.clone()
        };
        ui.heading(heading);
        if let Some(doc) = self.schema.map(|s| first_line(&s.doc)) {
            if !doc.is_empty() {
                ui.weak(doc);
            }
        }

        if call.generated {
            ui.label(
                "Produced by a loop or spread. Visible and selectable, but not \
                 structurally editable here.",
            );
            return;
        }

        if !call.positional.is_empty() {
            ui.separator();
            self.data_slots(ui, call);
        }

        ui.separator();
        for arg in &call.named {
            self.argument(ui, call, arg);
        }

        self.palette_editor(ui, call);
        self.add_argument(ui, call);
    }

    // ------------------------------------------------------------ data slots

    fn data_slots(&mut self, ui: &mut egui::Ui, call: &CallSite) {
        let names: Vec<String> = self
            .schema
            .map(|s| {
                s.params
                    .iter()
                    .filter(|p| p.kind == "positional")
                    .map(|p| p.name.clone())
                    .collect()
            })
            .unwrap_or_default();

        for (index, slot) in call.positional.iter().enumerate() {
            let name = names
                .get(index)
                .cloned()
                .unwrap_or_else(|| format!("#{index}"));
            let source = self.context.slot_sources.get(index);
            let linked = source.and_then(|s| s.file.as_deref());
            ui.horizontal(|ui| {
                ui.label(&name);
                if slot.elements.is_empty() {
                    ui.weak(ellipsis(&slot.text, 24)).on_hover_text(&slot.text);
                    // The data is an expression, so its points cannot be
                    // dragged -- but lilook has them from the compile, and
                    // writing them into the source is an explicit conversion
                    // rather than a silent regeneration.
                    if let Some(n) = self.context.recovered_points {
                        // Veusz calls this unlocking, and the word is better:
                        // for a linked slot it does not just make the points
                        // draggable, it *ends the link*.
                        let (label, hint) = match linked {
                            Some(f) => (
                                "unlock",
                                format!(
                                    "stop reading {f} and write the {n} values into \
                                     the document. The figure stops following the \
                                     file, and the points become draggable."
                                ),
                            ),
                            None => (
                                "materialise",
                                format!(
                                    "replace this expression with the {n} evaluated \
                                     values, so the points become draggable"
                                ),
                            ),
                        };
                        if ui.small_button(label).on_hover_text(hint).clicked() {
                            self.events.push(UiEvent::Materialize {
                                node: call.id,
                                index,
                            });
                        }
                    }
                } else {
                    ui.weak(format!("{} values", slot.elements.len()))
                        .on_hover_text(ellipsis(&slot.text, 200));
                }
            });
            // Where the numbers come from, said per slot: x and y link
            // independently, so one can be fresh while the other is stale.
            if let (Some(file), Some(s)) = (linked, source) {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    if s.missing {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("↥ {file} · missing"),
                        );
                    } else if s.stale {
                        ui.colored_label(ui.visuals().warn_fg_color, format!("↥ {file} · changed"));
                    } else {
                        ui.weak(format!("↥ {file}")).on_hover_text(
                            "Linked: this file is the source of truth, and the \
                             figure follows it. Nothing is stored in the document.",
                        );
                    }
                });
            }
        }
    }

    // --------------------------------------------------------------- one row

    fn argument(&mut self, ui: &mut egui::Ui, call: &CallSite, arg: &NamedArg) {
        let schema = self.param(&arg.name);
        let control = control_of(schema, arg.editability, &arg.text);
        let choices: Vec<String> = schema.map(|p| p.choices.clone()).unwrap_or_default();
        let sentinels: Vec<String> = schema.map(|p| p.sentinels.clone()).unwrap_or_default();
        let tooltip = schema.map(|p| {
            let mut t = p.doc.clone();
            if let Some(d) = &p.default {
                t.push_str(&format!("\n\ndefault: {d}"));
            }
            t
        });
        let node = call.id;
        let name = arg.name.clone();
        let mut ev: Vec<UiEvent> = Vec::new();

        ui.horizontal(|ui| {
            let label = ui.label(&name);
            if let Some(t) = &tooltip {
                if !t.trim().is_empty() {
                    label.on_hover_text(t);
                }
            }

            match control {
                Control::Unset => {
                    ui.weak(arg.text.trim())
                        .on_hover_text("Not set, so lilaq decides.");
                    // The control this *would* be if it held a value, so the seed
                    // is of the right type.
                    let typed = refine(control_for(schema), arg.editability, "");
                    match seed(schema, typed) {
                        Some(value) => {
                            if ui
                                .small_button("set")
                                .on_hover_text(format!("start from {value} and edit it here"))
                                .clicked()
                            {
                                ev.push(set(node, &name, value));
                            }
                        }
                        // lilook knows the shape but not the contents, so it shows
                        // the shape and lets the user supply them. Nothing invalid
                        // is written either way: `check_expr` still guards.
                        None => source_row_hinted(ui, node, &name, "", shape_hint(schema), &mut ev),
                    }
                }
                Control::Number | Control::Length | Control::NumberOrArray => {
                    match split_numeric(&arg.text) {
                        Some((mut v, unit)) => {
                            let r = ui.add(
                                egui::DragValue::new(&mut v)
                                    .speed(drag_speed(&unit))
                                    .suffix(&unit),
                            );
                            if r.drag_started() {
                                ev.push(UiEvent::Begin {
                                    node,
                                    param: name.clone(),
                                });
                            }
                            if r.changed() {
                                ev.push(set(node, &name, format!("{}{unit}", num(v))));
                            }
                            if r.drag_stopped() {
                                ev.push(UiEvent::Commit);
                            }
                        }
                        // An array in a number-or-array slot, or an expression.
                        None => source_row(ui, node, &name, &arg.text, &mut ev),
                    }
                }
                Control::Toggle => {
                    let mut b = arg.text.trim() == "true";
                    if ui.checkbox(&mut b, "").changed() {
                        ev.push(set(node, &name, b.to_string()));
                    }
                }
                Control::Enum => {
                    // Sentinels first, so "auto" is a menu entry rather than
                    // something to remember the spelling of.
                    let options: Vec<String> =
                        sentinels.iter().cloned().chain(choices.clone()).collect();
                    let current = arg.text.trim().trim_matches('"').to_string();
                    if let Some(c) = combo(ui, (node, &name), &current, &options) {
                        ev.push(set(node, &name, quoted_choice(&c, &sentinels)));
                    }
                }
                // A colour ramp, shown as one. A name in a list says nothing
                // about what the figure will look like; a gradient strip says all
                // of it, and choosing a map is the single biggest visual decision
                // on a heatmap.
                Control::Colormap => {
                    let current = arg.text.trim().to_string();
                    let mine: Vec<(&str, &str)> = self
                        .context
                        .saved
                        .iter()
                        .filter(|s| s.kind == lilook_core::Kind::Colormap)
                        .map(|s| (s.name.as_str(), s.value.as_str()))
                        .collect();
                    // A map of the user's own is matched by its value, a typst
                    // one by its name -- `color.map.viridis` is a name lilook
                    // never sees the colours of.
                    let short = mine
                        .iter()
                        .find(|(_, expr)| *expr == current)
                        .map(|(n, _)| n.to_string())
                        .unwrap_or_else(|| pick::map_name(&current).to_string());
                    let mut picked = None;
                    let mut edit = None;
                    egui::ComboBox::from_id_salt((node, &name))
                        .selected_text(&short)
                        .width(190.0)
                        .show_ui(ui, |ui| {
                            for (i, (n, expr)) in mine.iter().enumerate() {
                                let r = pick::click_row(
                                    ui,
                                    ui.id().with(("mine-map", i)),
                                    short == *n,
                                    |ui| {
                                        pick::ramp_of(ui, &pick::cycle_parts(expr));
                                        ui.label(*n);
                                        if pick::chip(ui, ui.id().with(("forget-map", i)), "×")
                                            .on_hover_text("remove from your library")
                                            .clicked()
                                        {
                                            edit = Some(Edit::Forget(n.to_string()));
                                        }
                                    },
                                );
                                if r.on_hover_text("yours").clicked() {
                                    picked = Some(expr.to_string());
                                }
                            }
                            for (map, note) in lilook_core::COLORMAPS {
                                // The whole row, ramp included: the gradient is
                                // what the choice *is*, and it used to be the one
                                // part of the row that could not be clicked.
                                let r = pick::click_row(
                                    ui,
                                    ui.id().with(("colormap", map)),
                                    short == *map,
                                    |ui| {
                                        ramp(ui, map);
                                        ui.label(*map);
                                    },
                                );
                                if r.on_hover_text(*note).clicked() {
                                    picked = Some(format!("color.map.{map}"));
                                }
                            }
                        });
                    // The strip beside the box, so the current choice reads
                    // without opening anything -- from the user's own colours
                    // where they are the user's, and from lilook's preview of a
                    // typst map where they are not.
                    match mine.iter().find(|(_, expr)| *expr == current) {
                        Some((_, expr)) => pick::ramp_of(ui, &pick::cycle_parts(expr)),
                        None => ramp(ui, &short),
                    }
                    if pick::chip(ui, ui.id().with((node, "new-map")), "new…")
                        .on_hover_text("build a colour map of your own, starting from this one")
                        .clicked()
                    {
                        edit = Some(Edit::Open(current.clone()));
                    }
                    if let Some(v) = picked {
                        ev.push(set(node, &name, v));
                    }
                    match edit {
                        Some(Edit::Open(from)) => {
                            let kind = lilook_core::Kind::Colormap;
                            let taken = lilook_core::Prefs {
                                version: lilook_core::Prefs::VERSION,
                                saved: self.context.saved.to_vec(),
                            };
                            let suggested = taken.free_name(kind, "my map");
                            open_cycle_editor(ui, kind, node, &name, &suggested, &from)
                        }
                        Some(Edit::Forget(n)) => ev.push(UiEvent::RemovePref {
                            kind: lilook_core::Kind::Colormap,
                            name: n,
                        }),
                        None => {}
                    }
                }
                // The palette every series in this diagram draws from: the
                // user's own first, then lilaq's.
                Control::Cycle => {
                    let current = arg.text.trim().to_string();
                    let mine: Vec<(&str, &str, &str)> = self
                        .context
                        .saved
                        .iter()
                        .filter(|s| s.kind == lilook_core::Kind::Cycle)
                        .map(|s| (s.name.as_str(), s.value.as_str(), "yours"))
                        .collect();
                    let label = mine
                        .iter()
                        .map(|(n, expr, _)| (*n, *expr))
                        .chain(lilook_core::CYCLES.iter().map(|(n, expr, _)| (*n, *expr)))
                        .find(|(_, expr)| *expr == current)
                        .map(|(n, _)| n.to_string())
                        .unwrap_or_else(|| match current.len() > 18 {
                            true => "custom".into(),
                            false => current.clone(),
                        });
                    let mut picked = None;
                    let mut edit = None;
                    egui::ComboBox::from_id_salt((node, &name))
                        .selected_text(&label)
                        .width(210.0)
                        .show_ui(ui, |ui| {
                            let builtin = lilook_core::CYCLES
                                .iter()
                                .map(|(n, expr, note)| (*n, *expr, *note));
                            for (i, (n, expr, note)) in
                                mine.iter().copied().chain(builtin).enumerate()
                            {
                                // Swatches included, for the same reason the
                                // colormap ramp is.
                                let r = pick::click_row(
                                    ui,
                                    ui.id().with(("cycle", i)),
                                    label == n,
                                    |ui| {
                                        swatches(ui, expr);
                                        ui.label(n);
                                        if note == "yours"
                                            && pick::chip(ui, ui.id().with(("forget", i)), "×")
                                                .on_hover_text("remove from your library")
                                                .clicked()
                                        {
                                            edit = Some(Edit::Forget(n.to_string()));
                                        }
                                    },
                                );
                                if r.on_hover_text(note).clicked() {
                                    picked = Some(expr.to_string());
                                }
                            }
                        });
                    // Beside the menu rather than in it: a combo box scrolls its
                    // own list, and an action at the bottom of one is an action
                    // nobody finds.
                    if pick::chip(ui, ui.id().with((node, "new-palette")), "new…")
                        .on_hover_text("build a palette of your own, from this one")
                        .clicked()
                    {
                        edit = Some(Edit::Open(current.clone()));
                    }
                    if let Some(v) = picked {
                        // A named cycle is a string; a list of colours is an
                        // array and must not be quoted.
                        ev.push(set(node, &name, lilook_core::cycle_source(&v)));
                    }
                    match edit {
                        Some(Edit::Open(from)) => {
                            let kind = lilook_core::Kind::Cycle;
                            // Named before it is opened, so `save` is never
                            // refused for a field the user has not reached yet.
                            // The rule for a free name lives with the library.
                            let taken = lilook_core::Prefs {
                                version: lilook_core::Prefs::VERSION,
                                saved: self.context.saved.to_vec(),
                            };
                            let suggested = taken.free_name(lilook_core::Kind::Cycle, "my palette");
                            open_cycle_editor(ui, kind, node, &name, &suggested, &from)
                        }
                        Some(Edit::Forget(n)) => ev.push(UiEvent::RemovePref {
                            kind: lilook_core::Kind::Cycle,
                            name: n,
                        }),
                        None => {}
                    }
                }
                Control::Mark | Control::Scale => {
                    let fallback: &[&str] = if control == Control::Mark {
                        MARK_NAMES
                    } else {
                        SCALE_NAMES
                    };
                    let options: Vec<String> = if choices.is_empty() {
                        strs(fallback)
                    } else {
                        choices.clone()
                    };
                    let options: Vec<String> = sentinels.iter().cloned().chain(options).collect();
                    let current = arg.text.trim().trim_matches('"').to_string();
                    if options.contains(&current) {
                        if let Some(c) = combo(ui, (node, &name), &current, &options) {
                            ev.push(set(node, &name, quoted_choice(&c, &sentinels)));
                        }
                    } else {
                        // `lq.marks.x`, a custom scale object: editable, but not
                        // from a list of names.
                        source_row(ui, node, &name, &arg.text, &mut ev);
                    }
                }
                Control::Color => match parse_color(&arg.text) {
                    Some(c) => {
                        let mut c = c;
                        if ui.color_edit_button_srgba(&mut c).changed() {
                            ev.push(set(node, &name, color_source(c, &arg.text)));
                        }
                        ui.weak(ellipsis(&arg.text, 16));
                    }
                    None => source_row(ui, node, &name, &arg.text, &mut ev),
                },
                Control::Stroke => match parse_stroke(&arg.text) {
                    Some(s) => {
                        if let Some(next) = stroke_row(ui, (node, &name), &s) {
                            ev.push(set(node, &name, stroke_source(&next)));
                        }
                    }
                    None => source_row(ui, node, &name, &arg.text, &mut ev),
                },
                Control::Alignment => match parse_alignment(&arg.text) {
                    Some((h, v)) => {
                        let (mut h, mut v) = (h, v);
                        let mut changed = false;
                        if let Some(c) = combo(
                            ui,
                            (node, &format!("{name}-h")),
                            h.as_deref().unwrap_or("—"),
                            &strs(H_ALIGN),
                        ) {
                            h = Some(c);
                            changed = true;
                        }
                        if let Some(c) = combo(
                            ui,
                            (node, &format!("{name}-v")),
                            v.as_deref().unwrap_or("—"),
                            &strs(V_ALIGN),
                        ) {
                            v = Some(c);
                            changed = true;
                        }
                        if changed {
                            ev.push(set(node, &name, alignment_source(&h, &v)));
                        }
                    }
                    None => source_row(ui, node, &name, &arg.text, &mut ev),
                },
                Control::Text => {
                    let sentinel = sentinel_of(&arg.text, schema);
                    // An unset parameter shows an empty field; anything else shows
                    // its words, without the `[..]` or `".."` around them.
                    let parsed = parse_text(&arg.text);
                    if sentinel.is_none() && parsed.is_none() {
                        source_row(ui, node, &name, &arg.text, &mut ev);
                    } else {
                        // Keep the shape the user wrote. Where there is none yet,
                        // prefer content: it is the typst idiom, and it is the only
                        // one of the two that can hold markup.
                        let shape = parsed.as_ref().map_or_else(
                            || {
                                if schema.is_some_and(|p| p.types.iter().any(|t| t == "content"))
                                    || schema.is_some_and(|p| p.widget == "content")
                                {
                                    TextShape::Content
                                } else {
                                    TextShape::Str
                                }
                            },
                            |(s, _)| *s,
                        );
                        let mut buf = parsed.map(|(_, t)| t).unwrap_or_default();
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut buf)
                                .hint_text(sentinel.unwrap_or(""))
                                .desired_width(150.0),
                        );
                        if r.changed() {
                            // Emptying the field means "unset" again where the
                            // parameter has a sentinel -- otherwise there would be
                            // no way back to `none` without the source editor.
                            let value = match (buf.trim().is_empty(), first_sentinel(schema)) {
                                (true, Some(s)) => s.to_string(),
                                _ => text_source(shape, &buf),
                            };
                            ev.push(set(node, &name, value));
                        }
                    }
                }
                Control::Source => source_row(ui, node, &name, &arg.text, &mut ev),
                Control::ReadOnly => {
                    ui.weak(ellipsis(&arg.text, 22)).on_hover_text(&arg.text);
                    if arg.editability == Editability::Binding
                        && ui.small_button("go to definition").clicked()
                    {
                        ev.push(UiEvent::GoToBinding {
                            node,
                            param: name.clone(),
                            name: arg.text.clone(),
                        });
                    }
                }
            }

            // Sentinels (auto / none) are first-class, not a nullable hack:
            // Phase 0 found they appear in ~44% of typed parameters.
            for s in &sentinels {
                if arg.text.trim() != s.as_str() && ui.small_button(s).clicked() {
                    ev.push(set(node, &name, s.clone()));
                }
            }
            if ui
                .small_button("×")
                .on_hover_text("remove this argument, returning it to its default")
                .clicked()
            {
                ev.push(UiEvent::Remove {
                    node,
                    param: name.clone(),
                });
            }
        });

        self.events.extend(ev);
    }

    // ----------------------------------------------------------- palettes

    /// Build a palette or a colour map of your own, from the one in force.
    ///
    /// Open only when the cycle menu asked for it, and it edits *source text*
    /// rather than colours: `rgb("#4477aa")` comes back out the way it went in,
    /// because a palette that silently reprints itself every time it is opened
    /// is a palette that has been rewritten rather than edited.
    ///
    /// Saving does two things on purpose -- keeps it in the library and applies
    /// it to the figure. Building a palette to look at is not a thing anyone
    /// wants, and having to pick it from the menu afterwards is a step with no
    /// question in it.
    fn palette_editor(&mut self, ui: &mut egui::Ui, call: &CallSite) {
        let id = cycle_editor_id(call.id);
        let Some(mut state) = ui.data(|d| d.get_temp::<PaletteEdit>(id)) else {
            return;
        };
        let mut close = false;
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("palette");
            ui.add(
                egui::TextEdit::singleline(&mut state.name)
                    .id(id.with("name"))
                    .desired_width(130.0),
            );
            if pick::chip(ui, id.with("save"), "save")
                .on_hover_text("keep this palette and use it here")
                .clicked()
            {
                let value = pick::cycle_array(&state.colors);
                self.events.push(UiEvent::SavePref {
                    kind: state.kind,
                    name: state.name.clone(),
                    value: value.clone(),
                });
                self.events.push(set(call.id, &state.param, value));
                close = true;
            }
            if pick::chip(ui, id.with("cancel"), "cancel").clicked() {
                close = true;
            }
        });

        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (i, part) in state.colors.iter_mut().enumerate() {
                let mut c = parse_color(part).unwrap_or(egui::Color32::GRAY);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    *part = color_source(c, part);
                }
                if pick::chip(ui, id.with(("drop", i)), "×")
                    .on_hover_text("drop this colour")
                    .clicked()
                {
                    remove = Some(i);
                }
            }
            if pick::chip(ui, id.with("add"), "+")
                .on_hover_text("one more, to edit")
                .clicked()
            {
                // From the last colour rather than from black: a palette is
                // built by variation, and an editor that starts every entry at
                // the same place makes the user do that work twice.
                let next = state.colors.last().cloned().unwrap_or_else(|| "red".into());
                state.colors.push(next);
            }
        });
        if let Some(i) = remove {
            state.colors.remove(i);
        }
        // What it will look like, from the same painter its menu uses: a
        // palette is distinct colours and a map is a ramp between them.
        match state.kind {
            lilook_core::Kind::Colormap => pick::ramp_of(ui, &state.colors),
            _ => swatches(ui, &pick::cycle_array(&state.colors)),
        }

        match close {
            true => ui.data_mut(|d| d.remove::<PaletteEdit>(id)),
            false => ui.data_mut(|d| {
                d.insert_temp(id, state);
            }),
        }
    }

    // ---------------------------------------------------------- add argument

    /// Add an argument: type to narrow, one click to write it.
    ///
    /// The same popup the source pane opens at the caret, over the same offers,
    /// accepted the same way -- see `pick`. It used to be a combo box and an
    /// `add` button, which made adding `interpolation: "smooth"` three acts
    /// (choose the name, press add, then find the value) where typing it was one.
    fn add_argument(&mut self, ui: &mut egui::Ui, call: &CallSite) {
        let Some(schema) = self.schema else { return };
        let all = match self.effective.is_empty() {
            true => &schema.params,
            false => &self.effective,
        };
        let offers = lilook_core::argument_offers(all, call);
        if offers.is_empty() {
            return;
        }

        // What has been typed lives in egui's own store, keyed by the call site,
        // *not* in a field on `Inspector`.
        //
        // It used to be a field, and the field was useless: the shell builds a
        // fresh `Inspector` every frame, so the state was dropped between the
        // click that made it and the frame that would have acted on it -- the
        // combo reset to "add argument…" and the "add" button never appeared. Two
        // frames of state cannot live on a one-frame object.
        let filter_id = add_argument_filter_id(call.id);
        let mut typed: String = ui
            .data(|d| d.get_temp::<String>(filter_id))
            .unwrap_or_default();

        ui.separator();
        let field = ui
            .horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut typed)
                        .id(filter_id.with("field"))
                        .hint_text("add argument…")
                        .desired_width(190.0),
                )
            })
            .inner;

        // Gated on focus, like the source pane's: a field nobody is in should not
        // cover the arguments below it. `pick::popup` keeps it up across the
        // press and release of a click, which is the part focus alone cannot do.
        let open = field.has_focus();
        // A row matches on its own name or on any value it names, so `smo` finds
        // the row that carries `smooth`.
        let matching: Vec<&lilook_core::ArgumentOffer> = offers
            .iter()
            .filter(|o| {
                pick::matches(
                    &pick::haystack(&o.label, o.choices.iter().map(|(l, _)| l.as_str())),
                    &typed,
                )
            })
            .collect();
        // These outlive the borrowed rows they are shown in.
        let values: Vec<String> = matching.iter().map(|o| o.written()).collect();
        let labels: Vec<Vec<String>> = matching
            .iter()
            .map(|o| o.choices.iter().map(|(l, _)| l.clone()).collect())
            .collect();
        let rows: Vec<pick::Offer> = matching
            .iter()
            .zip(&values)
            .zip(&labels)
            .map(|((o, v), c)| {
                pick::Offer::new(&o.label, &o.note, v)
                    .hint(&o.doc)
                    .choices(c)
            })
            .collect();

        let popup_id = filter_id.with("popup");
        let clicked = pick::popup(ui.ctx(), popup_id, field.rect.left_bottom(), &rows, open);
        // Enter takes the first match, which is what a field you type a name into
        // is for. The source pane has no equivalent: there, Enter is a newline.
        //
        // `lost_focus`, not `open`: a single-line field gives up focus on Enter,
        // so by the time the key is visible here the field is no longer focused.
        let entered = field.lost_focus()
            && !rows.is_empty()
            && !typed.trim().is_empty()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let taken = clicked.or(entered.then_some(pick::Picked {
            row: 0,
            choice: None,
        }));

        if let Some(p) = taken {
            let offer = matching[p.row];
            self.events.push(UiEvent::Insert {
                node: call.id,
                param: offer.param.clone(),
                // The value the pointer was on, where it was on one -- picking
                // `smooth` and getting `pixelated` is the failure this exists to
                // make impossible.
                value: match p.choice.and_then(|k| offer.choices.get(k)) {
                    Some((_, value)) => value.clone(),
                    None => values[p.row].clone(),
                },
            });
            // Done: the field empties and lets go, so the popup closes rather
            // than offering the argument that was just added.
            typed.clear();
            ui.memory_mut(|m| m.surrender_focus(filter_id.with("field")));
        }
        ui.data_mut(|d| d.insert_temp(filter_id, typed));
    }
}

// ------------------------------------------------------------------ helpers

/// What the cycle menu asked for. The menu runs inside egui's closure, which is
/// already borrowing the inspector, so the answer is carried out rather than
/// acted on where it is made.
enum Edit {
    /// Start a palette of your own from this expression.
    Open(String),
    /// Take this one out of the library.
    Forget(String),
}

/// The list of colours being built, between frames.
///
/// In egui's store keyed by the call site, for the same reason the
/// add-argument field's text is: the shell builds a fresh `Inspector` every
/// frame, so anything that outlives a frame cannot live on it.
///
/// One editor for two kinds. A palette and a colour map are both an array of
/// colours -- lilaq reads the first as the series to draw with and the second as
/// stops to interpolate between -- so they differ in what the preview draws and
/// in which argument the result goes to, and in nothing else.
#[derive(Clone)]
struct PaletteEdit {
    kind: lilook_core::Kind,
    /// The argument it will be applied to -- `cycle` on a diagram, `map` on a
    /// mesh -- named rather than assumed, because a forwarded parameter can
    /// carry either.
    param: String,
    name: String,
    /// Each colour as *source text*, not as pixels.
    colors: Vec<String>,
}

/// Where the list being built lives. Derived from the call site alone, like
/// every other piece of two-frame state here.
pub fn cycle_editor_id(node: usize) -> egui::Id {
    egui::Id::new((node, "palette-editor"))
}

/// Seed the editor from whatever the figure is using now.
///
/// A value that is already an array of colours opens as its own colours --
/// exactly, from its source text. Anything else opens from what lilook can say
/// about it, and the two cases differ in an important way:
///
/// - a palette falls back to lilaq's own, which is what a diagram draws with
///   when nothing else is said, so the editor starts from what is on screen;
/// - a **colour map named by typst** -- `color.map.viridis` -- falls back to
///   lilook's *five-stop preview* of it, because the real map is 256 entries
///   inside typst and lilook has never had them. That is a fine place to start
///   a map of your own and a lie if it were presented as viridis, so it is
///   named as yours from the moment it opens and the hover says where the
///   colours came from.
pub fn open_cycle_editor(
    ui: &mut egui::Ui,
    kind: lilook_core::Kind,
    node: usize,
    param: &str,
    name: &str,
    from: &str,
) {
    let mut colors = pick::cycle_parts(from);
    if colors.is_empty() {
        colors = match kind {
            lilook_core::Kind::Colormap => pick::colormap_stops(pick::map_name(from))
                .iter()
                .map(pick::color_source_of)
                .collect(),
            _ => lilook_core::CYCLES
                .first()
                .map(|(_, expr, _)| pick::cycle_parts(expr))
                .unwrap_or_default(),
        };
    }
    let state = PaletteEdit {
        kind,
        param: param.to_string(),
        name: name.to_string(),
        colors,
    };
    ui.data_mut(|d| d.insert_temp(cycle_editor_id(node), state));
}

/// Where the "add argument" field keeps what has been typed into it.
///
/// Derived from the call site alone, deliberately. It must not depend on the
/// `Inspector` (the shell builds a new one every frame) and it must not depend on
/// the enclosing `Ui` either -- `make_persistent_id` mixes that in, and the panel
/// the inspector draws into is not guaranteed to hash the same across frames as
/// the tree above it grows and shrinks.
pub fn add_argument_filter_id(node: usize) -> egui::Id {
    egui::Id::new((node, "add-argument-filter"))
}

fn set(node: usize, param: &str, value: String) -> UiEvent {
    UiEvent::Set {
        node,
        param: param.to_string(),
        value,
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().trim().to_string()
}

fn ellipsis(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let cut: String = s.chars().take(n.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// Points and centimetres want different drag rates: 0.1 pt is imperceptible,
/// 0.1 cm is a visible step.
fn drag_speed(unit: &str) -> f64 {
    match unit {
        "cm" | "in" => 0.05,
        "mm" | "em" => 0.1,
        "%" => 1.0,
        "" => 0.05,
        _ => 0.2,
    }
}

fn combo(
    ui: &mut egui::Ui,
    id: (usize, &str),
    current: &str,
    options: &[String],
) -> Option<String> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for c in options {
                if ui.selectable_label(c == current, c).clicked() {
                    picked = Some(c.clone());
                }
            }
        });
    picked
}

/// The fallback: raw source, refused if it would not reparse. The refusal is
/// shown rather than swallowed, so "why did nothing happen" never comes up.
fn source_row(ui: &mut egui::Ui, node: usize, name: &str, text: &str, ev: &mut Vec<UiEvent>) {
    source_row_hinted(ui, node, name, text, "", ev)
}

/// The source editor, with placeholder text saying what shape is expected. Used
/// where a parameter is unset and lilook knows the shape but not the contents.
fn source_row_hinted(
    ui: &mut egui::Ui,
    node: usize,
    name: &str,
    text: &str,
    hint: &str,
    ev: &mut Vec<UiEvent>,
) {
    let id = ui.id().with((node, name, "src"));
    let editing = ui.memory(|m| m.has_focus(id));
    // Adopt the document's text whenever this box is not being typed in, so
    // undo and edits made on the canvas show up here.
    let mut buf = match editing {
        true => ui
            .data(|d| d.get_temp::<String>(id))
            .unwrap_or_else(|| text.to_string()),
        false => text.to_string(),
    };

    let r = ui.add(
        egui::TextEdit::singleline(&mut buf)
            .id(id)
            .hint_text(hint)
            .desired_width(150.0),
    );
    let valid = lilook_core::check_expr(&buf);
    if r.changed() && valid.is_ok() {
        ev.push(set(node, name, buf.clone()));
    }
    if let Err(e) = valid {
        ui.colored_label(ui.visuals().error_fg_color, "!")
            .on_hover_text(e);
    }
    ui.data_mut(|d| d.insert_temp(id, buf));
}

/// paint · thickness · dash, returning the new stroke when something changed.
fn stroke_row(ui: &mut egui::Ui, id: (usize, &str), s: &Stroke) -> Option<Stroke> {
    let mut next = s.clone();
    let mut changed = false;

    let paint = s.paint.clone().unwrap_or_else(|| "black".into());
    match parse_color(&paint) {
        Some(c) => {
            let mut c = c;
            if ui.color_edit_button_srgba(&mut c).changed() {
                next.paint = Some(color_source(c, &paint));
                changed = true;
            }
        }
        None => {
            ui.weak(ellipsis(&paint, 12));
        }
    }

    let (mut v, unit) = s
        .thickness
        .as_deref()
        .and_then(split_numeric)
        .unwrap_or((1.0, "pt".into()));
    if ui
        .add(egui::DragValue::new(&mut v).speed(0.1).suffix(&unit))
        .changed()
    {
        next.thickness = Some(format!("{}{unit}", num(v)));
        changed = true;
    }

    let dash = s.dash.clone().unwrap_or_else(|| "solid".into());
    let mut options = vec!["solid".to_string()];
    options.extend(
        DASH_NAMES
            .iter()
            .filter(|d| **d != "solid")
            .map(|s| s.to_string()),
    );
    if let Some(c) = combo(ui, (id.0, id.1), &dash, &options) {
        next.dash = (c != "solid").then_some(c);
        changed = true;
    }

    changed.then_some(next)
}
