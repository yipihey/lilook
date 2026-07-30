//! Schema-driven inspector: one call site's arguments, with the control for
//! each parameter chosen by the generated schema.
//!
//! Every widget kind the schema emits has a control here, and a test fails if a
//! regenerated schema introduces one that does not. The long tail -- 32
//! `variant` and 29 `opaque` parameters whose types are too wide to map -- gets
//! a validating source editor rather than a blind text box, which is the honest
//! form of "not specialised yet": the value is still editable, and one that
//! would not reparse is refused before it reaches the buffer.

use lilook_core::schema::{FunctionSchema, ParamSchema};
use lilook_core::{CallSite, Editability, NamedArg};

use crate::value::{
    alignment_source, color_source, num, parse_alignment, parse_color, parse_content, parse_stroke,
    split_numeric, stroke_source, Stroke, DASH_NAMES, H_ALIGN, MARK_NAMES, SCALE_NAMES, V_ALIGN,
};

#[derive(Debug, Clone, PartialEq)]
pub enum UiEvent {
    /// Pointer went down on a continuous control: open a coalescing transaction.
    Begin { node: usize, param: String },
    Set {
        node: usize,
        param: String,
        value: String,
    },
    /// Pointer released: commit, making the whole drag one undo step.
    Commit,
    /// Add an argument the call does not have yet.
    Insert {
        node: usize,
        param: String,
        value: String,
    },
    /// Remove an argument, returning the parameter to its default.
    Remove { node: usize, param: String },
    /// Write the evaluated data of a computed slot into the source, so its
    /// points become editable.
    Materialize { node: usize, index: usize },
    /// User asked to jump to the `#let` a value is bound to.
    GoToBinding {
        node: usize,
        param: String,
        name: String,
    },
}

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
    Content,
    /// A number or an array of them; which one is decided by the current value.
    NumberOrArray,
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
        "content" => Control::Content,
        "number-or-array" => Control::NumberOrArray,
        // Deliberately the source editor: unions too wide to map, or structures
        // with no small form.
        "array" | "data" | "dictionary" | "structured" | "variant" | "opaque" => Control::Source,
        _ => return None,
    })
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
            Control::Content if parse_content(text).is_some() => Control::Content,
            _ => Control::ReadOnly,
        };
    }
    match control {
        Control::Source if parse_content(text).is_some() => Control::Content,
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

/// What a data slot reads, as far as the document says.
///
/// Read out of the source rather than recorded anywhere: the slot expression
/// names a binding, and the binding says `csv("run.csv")`. So provenance cannot
/// go stale, cannot lie after a copy into another document, and needs no format
/// of lilook's own -- the compiler remains the only thing that decides what a
/// document means.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotSource {
    /// The file this slot's data is read from, if it is read from one.
    pub file: Option<String>,
    /// The file is named but was not there at the last compile.
    pub missing: bool,
    /// The file changed since the last compile and has not been reread.
    pub stale: bool,
}

pub struct Inspector<'a> {
    pub schema: Option<&'a FunctionSchema>,
    pub events: Vec<UiEvent>,
    pub context: Context<'a>,
    /// Parameter chosen in the "add argument" combo, kept across frames.
    adding: Option<String>,
}

impl<'a> Inspector<'a> {
    pub fn new(schema: Option<&'a FunctionSchema>) -> Self {
        Inspector {
            schema,
            events: vec![],
            context: Context::default(),
            adding: None,
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
        let control = refine(control_for(schema), arg.editability, &arg.text);
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
                    let current = arg.text.trim().trim_matches('"').to_string();
                    if let Some(c) = combo(ui, (node, &name), &current, &choices) {
                        ev.push(set(node, &name, format!("\"{c}\"")));
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
                    let current = arg.text.trim().trim_matches('"').to_string();
                    if options.contains(&current) {
                        if let Some(c) = combo(ui, (node, &name), &current, &options) {
                            ev.push(set(node, &name, format!("\"{c}\"")));
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
                Control::Content => match parse_content(&arg.text) {
                    Some(inner) => {
                        let mut buf = inner;
                        if ui.text_edit_singleline(&mut buf).changed() {
                            ev.push(set(node, &name, format!("[{buf}]")));
                        }
                    }
                    None => source_row(ui, node, &name, &arg.text, &mut ev),
                },
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

        ui.separator();
        ui.horizontal(|ui| {
            let selected = self
                .adding
                .clone()
                .unwrap_or_else(|| "add argument…".to_string());
            let mut picked = None;
            egui::ComboBox::from_id_salt((call.id, "add-argument"))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for p in &missing {
                        let is = self.adding.as_deref() == Some(p.name.as_str());
                        if ui
                            .selectable_label(is, &p.name)
                            .on_hover_text(first_line(&p.doc))
                            .clicked()
                        {
                            picked = Some(p.name.clone());
                        }
                    }
                });
            if picked.is_some() {
                self.adding = picked;
            }

            let ready = self
                .adding
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
                    self.adding = None;
                }
            }
        });
    }
}

// ------------------------------------------------------------------ helpers

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
        Some(Control::Content) => "[]".into(),
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
