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

use lilook_core::schema::{FunctionSchema, ParamSchema};
use lilook_core::{CallSite, Editability, NamedArg};

use crate::value::{
    alignment_source, color_source, num, parse_alignment, parse_color, parse_stroke, parse_text,
    split_numeric, stroke_source, text_source, Stroke, TextShape, DASH_NAMES, H_ALIGN, MARK_NAMES,
    SCALE_NAMES, V_ALIGN,
};

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
fn seed(param: Option<&ParamSchema>, control: Control) -> Option<String> {
    let default = param
        .and_then(|p| p.default.as_deref())
        .filter(|d| sentinel_of(d, param).is_none() && lilook_core::check_expr(d).is_ok());
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
fn shape_hint(param: Option<&ParamSchema>) -> &'static str {
    match param.map(|p| p.widget.as_str()) {
        Some("array") | Some("data") | Some("number-or-array") => "(0, 10)",
        Some("dictionary") | Some("structured") => "(key: value)",
        _ => "",
    }
}

/// A name from a menu, written the way typst wants it: a sentinel is a keyword,
/// everything else is a string.
fn quoted_choice(choice: &str, sentinels: &[String]) -> String {
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
fn first_sentinel(param: Option<&ParamSchema>) -> Option<&str> {
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
fn takes_text(param: Option<&ParamSchema>) -> bool {
    param.is_some_and(|p| p.widget == "content" || p.types.iter().any(|t| t == "content"))
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

/// The schema's answer, before the value is looked at. Pass it through
/// [`refine`] to get the control the inspector actually renders.
pub fn control_for(param: Option<&ParamSchema>) -> Control {
    param
        .and_then(|p| widget_control(&p.widget))
        .unwrap_or(Control::Source)
}

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
}

pub struct Inspector<'a> {
    pub schema: Option<&'a FunctionSchema>,
    pub events: Vec<UiEvent>,
    pub context: Context<'a>,
}

impl<'a> Inspector<'a> {
    pub fn new(schema: Option<&'a FunctionSchema>) -> Self {
        Inspector {
            schema,
            events: vec![],
            context: Context::default(),
        }
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

    // ---------------------------------------------------------- add argument

    fn add_argument(&mut self, ui: &mut egui::Ui, call: &CallSite) {
        let Some(schema) = self.schema else { return };
        let missing: Vec<ParamSchema> = schema
            .params
            .iter()
            .filter(|p| p.kind != "positional" && !call.named.iter().any(|a| a.name == p.name))
            .cloned()
            .collect();
        if missing.is_empty() {
            return;
        }

        // The chosen parameter lives in egui's own per-widget store, keyed by the
        // combo's id, *not* in a field on `Inspector`.
        //
        // It used to be a field, and the field was useless: the shell builds a
        // fresh `Inspector` every frame, so the choice was dropped between the
        // click that made it and the frame that would have acted on it -- the
        // combo reset to "add argument…" and the "add" button never appeared. Two
        // frames of state cannot live on a one-frame object.
        let choice_id = add_argument_choice_id(call.id);
        let chosen: Option<String> = ui.data(|d| d.get_temp::<String>(choice_id));

        ui.separator();
        ui.horizontal(|ui| {
            let selected = chosen
                .clone()
                .unwrap_or_else(|| "add argument…".to_string());
            let mut picked = None;
            egui::ComboBox::from_id_salt((call.id, "add-argument"))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for p in &missing {
                        let is = chosen.as_deref() == Some(p.name.as_str());
                        if ui
                            .selectable_label(is, &p.name)
                            .on_hover_text(first_line(&p.doc))
                            .clicked()
                        {
                            picked = Some(p.name.clone());
                        }
                    }
                });
            if let Some(name) = picked.clone() {
                ui.data_mut(|d| d.insert_temp(choice_id, name));
            }

            // This frame's pick counts immediately, so the "add" button appears
            // as soon as something is chosen rather than a frame later.
            let current = picked.or(chosen);
            let ready = current
                .as_ref()
                .and_then(|n| missing.iter().find(|p| &p.name == n));
            if let Some(p) = ready {
                if ui.button("add").clicked() {
                    // Start from the documented default, so adding an argument
                    // changes nothing until the user edits it.
                    let value = p
                        .default
                        .clone()
                        .filter(|d| lilook_core::check_expr(d).is_ok())
                        .unwrap_or_else(|| placeholder(&p.widget));
                    self.events.push(UiEvent::Insert {
                        node: call.id,
                        param: p.name.clone(),
                        value,
                    });
                    ui.data_mut(|d| d.remove::<String>(choice_id));
                }
            }
        });
    }
}

// ------------------------------------------------------------------ helpers

/// Where the "add argument" combo keeps the parameter you picked.
///
/// Derived from the call site alone, deliberately. It must not depend on the
/// `Inspector` (the shell builds a new one every frame) and it must not depend on
/// the enclosing `Ui` either -- `make_persistent_id` mixes that in, and the panel
/// the inspector draws into is not guaranteed to hash the same across frames as
/// the tree above it grows and shrinks.
pub fn add_argument_choice_id(node: usize) -> egui::Id {
    egui::Id::new((node, "add-argument-choice"))
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

/// A value to offer when the schema has no usable default.
fn placeholder(widget: &str) -> String {
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
