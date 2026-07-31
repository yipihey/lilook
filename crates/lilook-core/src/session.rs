//! One editing session: the document, what is selected, and every operation a
//! frontend can ask for -- with no toolkit anywhere in it.
//!
//! This is the layer that was measured and found to be in the wrong place. Of
//! the editor's methods, 44 never mentioned `egui`: linking a data file,
//! unlocking a dataset, the four theme operations, paste, duplicate, delete and
//! the whole gesture vocabulary. All of it was pure document logic behind a GUI
//! dependency, which meant a Swift view or an MCP tool could not call any of it
//! without reimplementing it.
//!
//! So the rule is now the same shape as "the editor never touches the compiler":
//! **the panels never touch the document.** A frontend owns a `Session`, feeds it
//! [`CanvasEvent`]s and [`UiEvent`]s, reads `requests` for the work it must do on
//! the session's behalf, and hands results back. Everything it can express, every
//! other frontend can express too.

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

use crate::render::{Diagnostic, Severity};
use crate::scene::Scene;
use crate::{CanvasEvent, DataFile, Document, Intent, Schema, UiEvent};

/// Seconds of quiet before a burst of small edits becomes one undo step.
pub const IDLE_COMMIT_SECONDS: f64 = 0.4;

/// A copied fragment and the definitions it depends on.
#[derive(Debug, Clone)]
pub struct Clip {
    pub source: String,
    /// `name` -> the whole `#let name = ..` that defines it, where the source
    /// document had one.
    pub bindings: Vec<(String, String)>,
    /// Names the source document did not define either, carried so a paste can
    /// say what will be missing rather than failing silently.
    pub unresolved: Vec<String>,
}

/// What the editor wants from its shell this frame. Everything platform-shaped
/// -- writing a file, closing a window -- is a request rather than an action,
/// because the browser cannot honour most of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Requests {
    pub save: bool,
    /// Source to compile, and at what resolution. Latest-wins: the shell may
    /// drop it if it is already busy.
    pub compile: Option<(String, f32)>,
    /// An expression for the shell to evaluate through the compiler, answered
    /// with `Editor::accept_answer`. The editor cannot ask directly: it does not
    /// depend on the compiler, and in the desktop shell the compiler is on
    /// another thread anyway.
    pub query: Option<String>,
    /// Files dropped onto the window that are not already under the project
    /// root. Typst cannot read a path that escapes the root, so the shell has to
    /// bring the file in -- copying it on the desktop, or into the in-memory
    /// file system in the browser -- and then call `Editor::file_adopted` with
    /// the project-relative path it ended up at.
    pub adopt: Vec<Dropped>,
    /// The user asked for the figure as a file. `"pdf"`, `"svg"` or `"png"`,
    /// with the resolution the PNG path needs.
    ///
    /// A request rather than an action for the same reason `save` is: the
    /// session has no compiler and no file system, and in the browser there is
    /// no file system to have. The shell exports from the document it already
    /// compiled and puts the bytes wherever it can put them.
    pub export: Option<(String, f32)>,
    /// Messages whose cause the shell should locate by recompiling variants.
    ///
    /// A request rather than an action because it needs a compiler, and because
    /// it costs ~4 ms a candidate -- worth doing when the user asks about an
    /// error, not on every frame that has one.
    pub blame: Vec<String>,
    /// A figure to write out beside the manuscript, and the import that now
    /// refers to it.
    pub write_figure: Option<Extraction>,
}

/// Linking a file to a series, one step at a time.
///
/// The middle state exists because lilook has to *ask the compiler* what is in
/// the file -- it has no CSV parser of its own, and does not need one -- and the
/// answer arrives through the shell a frame or more later.
#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    /// Waiting for the answer to `expr`.
    Asking { path: String, expr: String },
    /// The file described itself; the user picks which column goes where.
    Ready {
        path: String,
        kind: crate::SourceKind,
        columns: crate::Columns,
        x: usize,
        y: usize,
        /// Which entry supplies a mesh's field, when the target is a mesh and
        /// the file holds a 2-D one. `None` means link x and y only, which is
        /// every other series.
        z: Option<usize>,
    },
    /// The file could not be read, with the compiler's reason.
    Failed { path: String, why: String },
}

/// One thing that may be written at the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    /// What to show in the list.
    pub label: String,
    /// What to put in the buffer.
    pub insert: String,
    /// Type or explanation, shown beside it.
    pub note: String,
}

/// The call the caret is inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    pub doc: String,
    pub params: Vec<String>,
    /// Which parameter the caret is in, if any, so it can be emphasised.
    pub active: Option<String>,
}

/// A figure moved out to its own file: what to write, and where.
///
/// Returned rather than written, because the session has no file system -- the
/// shell puts it wherever it can, which in a browser is a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    pub path: String,
    pub contents: String,
}

/// Something lilook can offer to do about a diagnostic.
///
/// A label, a reason, and the edit. Never applied on its own.
#[derive(Debug, Clone)]
pub struct Action {
    pub label: String,
    pub note: String,
    /// The edits, applied as one transaction.
    ///
    /// A list rather than one intent because some fixes are two edits -- a
    /// rename is a removal and an insertion -- and inventing a `RenameNamedArg`
    /// intent for it would mean a new variant to teach the undo generator, for
    /// something the existing pair already expresses.
    pub intents: Vec<Intent>,
}

/// The concrete values worth offering beside a parameter's name.
///
/// Only where the set is small and fixed: a scale, a mark, a colour map, a
/// palette. A length or a number has no useful list, and a menu of guesses is
/// worse than a field to type in.
fn values_for(
    p: &crate::schema::ParamSchema,
    control: Option<crate::Control>,
) -> Vec<(String, String)> {
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
        Some(crate::Control::Scale) => quoted(crate::policy::SCALE_NAMES),
        Some(crate::Control::Colormap) => crate::COLORMAPS
            .iter()
            .map(|(m, _)| ((*m).to_string(), format!("color.map.{m}")))
            .collect(),
        Some(crate::Control::Cycle) => crate::CYCLES
            .iter()
            .map(|(n, expr, _)| {
                let v = match expr.starts_with('(') {
                    true => (*expr).to_string(),
                    false => format!("\"{expr}\""),
                };
                ((*n).to_string(), v)
            })
            .collect(),
        Some(crate::Control::Toggle) => vec![
            ("true".into(), "true".into()),
            ("false".into(), "false".into()),
        ],
        _ => vec![],
    }
}

/// Levenshtein distance, for "did you mean".
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        row[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

/// A value the compiler resolved, to be shown at a byte offset.
///
/// Data, not a widget: a frontend places it inline, in the margin, or in a
/// tooltip, and the core does not care which.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hint {
    /// Where it belongs, in the user's buffer.
    pub at: usize,
    /// What to show.
    pub text: String,
    /// Why, for a tooltip.
    pub note: String,
}

/// The document being edited and everything an operation needs to reach.
pub struct Session {
    pub doc: Document,
    pub schema: Schema,
    pub selected: usize,
    pub status: String,
    pub scenes: Vec<Scene>,
    pub data_files: Vec<DataFile>,
    pub diagnostics: Vec<Diagnostic>,
    pub dirty: bool,
    pub explicit_tx: bool,
    pub idle_tx: Option<f64>,
    pub busy: bool,
    pub timing: String,
    pub clipboard: Option<Clip>,
    pub requests: Requests,
    pub link: Option<Link>,
    pub queued_query: Option<String>,
    /// Blame asked for from inside a frame, emitted on the next one.
    pub queued_blame: Vec<String>,
    /// A figure extracted from inside a frame, handed over on the next one.
    pub queued_write: Option<Extraction>,
    pub changed_files: Vec<String>,
    pub follow_files: bool,
    pub link_path: String,
    /// What the shell last found to be responsible for an error.
    pub blames: Vec<crate::Blame>,
    /// Where a dragged decoration started, for the length of one gesture.
    drag_origin: Option<(f64, f64)>,
}

impl Session {
    pub fn new(text: impl Into<String>, schema: Schema) -> Session {
        let doc = Document::new(text);
        let selected = doc
            .calls()
            .iter()
            .find(|c| c.callee.ends_with("diagram"))
            .or_else(|| doc.calls().first())
            .map(|c| c.id)
            .unwrap_or(0);
        Session {
            doc,
            schema,
            selected,
            status: String::new(),
            scenes: vec![],
            data_files: vec![],
            diagnostics: vec![],
            dirty: true,
            explicit_tx: false,
            idle_tx: None,
            busy: false,
            timing: String::new(),
            clipboard: None,
            requests: Requests::default(),
            link: None,
            queued_query: None,
            queued_blame: vec![],
            queued_write: None,
            changed_files: vec![],
            follow_files: false,
            link_path: String::new(),
            blames: vec![],
            drag_origin: None,
        }
    }

    /// Replace the buffer, as an open-file does.
    pub fn open(&mut self, text: impl Into<String>) {
        self.doc = Document::new(text);
        self.selected = self
            .doc
            .calls()
            .iter()
            .find(|c| c.callee.ends_with("diagram"))
            .or_else(|| self.doc.calls().first())
            .map(|c| c.id)
            .unwrap_or(0);
        self.scenes.clear();
        self.link = None;
        self.dirty = true;
    }

    /// Tell the editor its document changed underneath it -- after an intent
    /// applied by something other than the editor itself.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn text(&self) -> &str {
        self.doc.text()
    }

    pub fn scenes(&self) -> &[Scene] {
        &self.scenes
    }

    /// Files the last compile read, so a shell can watch the ones that matter.
    pub fn data_files(&self) -> &[DataFile] {
        &self.data_files
    }

    /// Linked files that changed and have not been reread yet.
    pub fn changed_files(&self) -> &[String] {
        &self.changed_files
    }

    /// Reread linked files as soon as they change, rather than offering to.
    pub fn set_follow_files(&mut self, follow: bool) {
        self.follow_files = follow;
    }

    /// A shell has noticed these linked files change on disk.
    ///
    /// Reported rather than acted on: someone is editing that file, and a figure
    /// that redraws itself from a half-written file is worse than one that waits
    /// to be told. `check_disk` already decided that question for the manuscript;
    /// this is the same answer for its data. Following changes automatically is
    /// opt-in, and even then never lands mid-gesture.
    pub fn files_changed(&mut self, paths: &[String]) {
        for p in paths {
            if !self.changed_files.contains(p) {
                self.changed_files.push(p.clone());
            }
        }
        if self.follow_files && !self.explicit_tx && self.idle_tx.is_none() {
            self.reload_data();
        }
    }

    /// Recompile so linked files are read again.
    ///
    /// Not an edit: the document does not change, so the undo history stays
    /// valid and there is nothing to coalesce. That is the whole reason a live
    /// link is preferable to embedded values.
    pub fn reload_data(&mut self) {
        if self.changed_files.is_empty() {
            return;
        }
        self.status = match self.changed_files.len() {
            1 => format!("reread {}", self.changed_files[0]),
            n => format!("reread {n} files"),
        };
        self.changed_files.clear();
        self.dirty = true;
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Tell the editor a compile is running, so it does not ask again.
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    /// The single place UI events become transactions.
    pub fn handle(&mut self, events: Vec<UiEvent>, now: f64) {
        for e in events {
            match e {
                UiEvent::Begin { .. } => {
                    self.doc.begin("drag");
                    self.explicit_tx = true;
                    self.idle_tx = None;
                }
                UiEvent::Commit => {
                    self.doc.commit();
                    self.explicit_tx = false;
                }
                UiEvent::Set { node, param, value } => {
                    self.open_idle(now);
                    self.apply(Intent::SetNamedArg { node, param, value });
                }
                UiEvent::Insert { node, param, value } => {
                    self.open_idle(now);
                    self.apply(Intent::InsertNamedArg { node, param, value });
                }
                UiEvent::Remove { node, param } => {
                    self.open_idle(now);
                    self.apply(Intent::RemoveNamedArg { node, param });
                }
                UiEvent::Materialize { node, index } => self.unlock(node, index),
                UiEvent::GoToBinding { name, .. } => {
                    self.status = match self.doc.text().find(&format!("#let {name}")) {
                        Some(at) => format!("`{name}` bound at byte {at}"),
                        None => format!("`{name}` is not bound in this file"),
                    };
                }
            }
        }
    }

    /// Apply one intent through the editor, so its bookkeeping stays in step.
    ///
    /// Public for the frontends that edit a theme's body directly -- a `set-*`
    /// rule inside a `#let` is not reachable through any named-argument path.
    pub fn apply_intent(&mut self, intent: Intent) {
        self.apply(intent);
    }

    pub fn apply(&mut self, intent: Intent) {
        match self.doc.apply(intent) {
            Ok(()) => {
                self.dirty = true;
                self.status.clear();
            }
            Err(err) => self.status = err,
        }
    }

    /// Start (or extend) the transaction that closes itself once the user stops.
    pub fn open_idle(&mut self, now: f64) {
        if self.explicit_tx {
            return;
        }
        if self.idle_tx.is_none() {
            self.doc.begin("edit");
        }
        self.idle_tx = Some(now);
    }

    /// Write a computed data slot's evaluated values into the source.
    ///
    /// This is a large replacement rather than a surgical one, and it is
    /// deliberately an explicit user action: the values come from the compile,
    /// so nothing is regenerated from a model, and the expression the user
    /// wrote is what gets replaced.
    pub fn unlock(&mut self, node: usize, index: usize) {
        let Some(points) = self
            .scenes
            .iter()
            .flat_map(|s| &s.series)
            .find(|s| s.node == node)
            .map(|s| s.points.clone())
        else {
            self.status = "no evaluated data for that series yet".into();
            return;
        };
        let values: Vec<f64> = match index {
            0 => points.iter().map(|p| p.0).collect(),
            1 => points.iter().map(|p| p.1).collect(),
            _ => {
                self.status = format!("slot {index} has no recovered data");
                return;
            }
        };
        // The data emitter, not the geometry one: these values came from a file
        // or from an evaluated series, so six decimal places would silently
        // flatten anything smaller than a microunit and turn a masked sample
        // into a real zero. It also refuses rather than making the buffer
        // unusable, which is why this can fail.
        let value = match crate::data_array_source(&values) {
            Ok(v) => v,
            Err(e) => {
                self.status = e.to_string();
                return;
            }
        };

        // If this slot was linked, unlocking it may leave the file's binding with
        // nothing reading it. Leaving that behind would be worse than untidy: the
        // document would go on reading the file, so the Data panel would keep
        // listing it and the figure would look linked when it is not.
        let linked = self
            .doc
            .call(node)
            .and_then(|c| c.positional.get(index).cloned())
            .and_then(|slot| self.binding_behind(&slot));

        self.doc.begin(if linked.is_some() {
            "unlock data"
        } else {
            "materialise"
        });
        self.apply(Intent::SetPositionalArg { node, index, value });
        if let Some(name) = linked {
            self.drop_binding_if_unused(&name);
        }
        self.doc.commit();
    }

    /// Remove a `#let` nothing refers to any more, taking its line with it.
    ///
    /// Part of the same transaction as whatever orphaned it, so one undo puts
    /// both back.
    pub fn drop_binding_if_unused(&mut self, name: &str) {
        let Some(range) = self.doc.binding_of(name) else {
            return;
        };
        // Count mentions outside the binding itself. A plain substring scan would
        // match `run` inside `running`, so this walks identifiers instead.
        let text = self.doc.text().to_string();
        let used = self
            .doc
            .free_identifiers(0..text.len())
            .iter()
            .any(|n| n == name);
        if used {
            return;
        }
        // Take the newline before the binding too, so removing it does not leave
        // a blank line where the data used to come from.
        let start = text[..range.start].rfind('\n').map_or(range.start, |i| i);
        self.apply(Intent::ReplaceRange {
            range: start..range.end,
            value: String::new(),
        });
    }

    /// The name of the binding a slot reads its data through.
    pub fn binding_behind(&self, slot: &crate::PositionalArg) -> Option<String> {
        if !slot.elements.is_empty() {
            return None;
        }
        self.doc
            .free_identifiers(slot.range.clone())
            .into_iter()
            .find(|name| {
                self.doc
                    .binding_of(name)
                    .is_some_and(|r| read_path(&self.doc.text()[r]).is_some())
            })
    }

    /// Write a length back in the unit the user was already using.
    ///
    /// A figure written in centimetres should stay in centimetres: rewriting
    /// `width: 8cm` as `width: 226.77pt` is technically the same figure and a
    /// visible loss to whoever has to read the source afterwards.
    pub fn set_length(&mut self, node: usize, param: &str, points: f64) {
        let unit = self
            .doc
            .call(node)
            .and_then(|c| {
                let named = |name: &str| {
                    c.named
                        .iter()
                        .find(|a| a.name == name)
                        .and_then(|a| crate::split_numeric(&a.text))
                        .map(|(_, u)| u)
                };
                // This argument's unit, or the other dimension's -- a figure
                // with `height: 5cm` and no width should gain `width: 8cm`.
                named(param).or_else(|| named(if param == "width" { "height" } else { "width" }))
            })
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "cm".to_string());

        // 1pt is 1/72in, and typst's `pt` is the same.
        let value = match unit.as_str() {
            "cm" => points / 28.346_456_7,
            "mm" => points / 2.834_645_67,
            "in" => points / 72.0,
            "pt" => points,
            // `em` depends on the font size, and `%` on a container lilook
            // cannot see. Both would be a guess, so write points instead.
            _ => points,
        };
        let unit = match unit.as_str() {
            "cm" | "mm" | "in" | "pt" => unit,
            _ => "pt".to_string(),
        };
        self.set_or_insert(node, param, format!("{}{unit}", num(value)));
    }

    /// Rewrite a named argument, adding it when the user never wrote one. The
    /// first frame of a pan on a figure with automatic limits has to insert.
    pub fn set_or_insert(&mut self, node: usize, param: &str, value: String) {
        let present = self
            .doc
            .call(node)
            .is_some_and(|c| c.named.iter().any(|a| a.name == param));
        let intent = if present {
            Intent::SetNamedArg {
                node,
                param: param.into(),
                value,
            }
        } else {
            Intent::InsertNamedArg {
                node,
                param: param.into(),
                value,
            }
        };
        self.apply(intent);
    }

    pub fn delete_selection(&mut self) {
        let Some(call) = self.doc.call(self.selected) else {
            return;
        };
        if call.generated {
            self.status = "that call is produced by a loop or spread; edit the loop".into();
            return;
        }
        let what = call.callee.clone();
        self.doc.begin("delete");
        match self.doc.apply(Intent::RemoveNode {
            node: self.selected,
        }) {
            Ok(()) => {
                self.doc.commit();
                self.dirty = true;
                self.selected = 0;
                self.status = format!("deleted {what}");
            }
            Err(err) => self.status = err,
        }
    }

    /// Paste into the selected figure (or the figure the selected series is in).
    ///
    /// Free-variable capture is the whole problem here: a copied series usually
    /// reads a `#let` that may not exist at the destination. lilook carries
    /// those definitions rather than inlining values, because inlining would
    /// turn a two-line figure into a wall of numbers and would silently drop
    /// the relationship the user wrote. Anything it cannot resolve is named in
    /// the status line instead of being discovered as a compile error.
    pub fn paste(&mut self, text: String) {
        let Some(figure) = self.paste_target() else {
            self.status = "select a figure to paste into".into();
            return;
        };
        let clip = self
            .clipboard
            .clone()
            .filter(|c| c.source == text)
            .unwrap_or(Clip {
                source: text,
                bindings: vec![],
                unresolved: vec![],
            });

        if let Err(e) = crate::check_expr(&clip.source) {
            self.status = format!("clipboard is not a lilaq call: {e}");
            return;
        }

        self.doc.begin("paste");
        // The call goes in first. Call-site ids are indices into a
        // document-order walk, so inserting a binding above the figure
        // renumbers it -- `figure` would name something else by the time the
        // insertion ran.
        self.apply(Intent::InsertPositionalArg {
            node: figure,
            value: clip.source.clone(),
        });
        // Then the definitions it needs, each after the import so it is in
        // scope wherever the figure is.
        let mut carried = 0;
        for (name, definition) in &clip.bindings {
            if self.doc.binding_of(name).is_some() {
                continue;
            }
            let Some(at) = self.import_end() else {
                continue;
            };
            self.apply(Intent::ReplaceRange {
                range: at..at,
                value: format!("\n{definition}"),
            });
            carried += 1;
        }
        self.doc.commit();

        if self.status.is_empty() {
            self.status = match (carried, clip.unresolved.len()) {
                (0, 0) => "pasted".into(),
                (c, 0) => format!("pasted, carrying {c} binding(s)"),
                (c, _) => format!(
                    "pasted, carrying {c} binding(s); unresolved: {}",
                    clip.unresolved.join(", ")
                ),
            };
        }
        // Select what was just pasted: it is the last call inside the figure.
        if let Some(f) = self.doc.figures().into_iter().find(|f| f.node == figure) {
            if let Some(last) = f.series.last() {
                self.selected = *last;
            }
        }
    }

    /// Where a paste goes: the selected diagram, or the one the selection is in.
    pub fn paste_target(&self) -> Option<usize> {
        let call = self.doc.call(self.selected)?;
        if call.short_name() == "diagram" {
            return Some(call.id);
        }
        self.doc
            .figure_of(self.selected)
            .or_else(|| self.doc.figures().first().map(|f| f.node))
    }

    /// Duplicate in place: the same machinery, without the clipboard.
    pub fn duplicate_selection(&mut self) {
        let Some(call) = self.doc.call(self.selected) else {
            return;
        };
        if call.short_name() == "diagram" {
            self.status = "duplicating a whole figure is not supported yet".into();
            return;
        }
        let Some(figure) = self.doc.figure_of(self.selected) else {
            self.status = "that call is not inside a figure".into();
            return;
        };
        let source = self.doc.text()[call.range.clone()].to_string();
        self.doc.begin("duplicate");
        self.apply(Intent::InsertPositionalArg {
            node: figure,
            value: source,
        });
        self.doc.commit();
        if let Some(f) = self.doc.figures().into_iter().find(|f| f.node == figure) {
            if let Some(last) = f.series.last() {
                self.selected = *last;
            }
        }
    }

    /// Series the user could actually drag: their data is a literal array they
    /// wrote, not an expression lilook would have to rewrite.
    pub fn editable_series(&self) -> Vec<usize> {
        self.doc
            .calls()
            .iter()
            .filter(|c| {
                if c.generated {
                    return false;
                }
                // A series hanging off a secondary axis is drawn against *that*
                // axis, and lilook recovers one transform per diagram -- the
                // primary's. Its data is read correctly, so the tree and the
                // inspector are right; dragging it would move it by the wrong
                // scale, so the canvas is not offered the handle.
                if self.doc.on_secondary_axis(c.id) {
                    return false;
                }
                // A rules series is movable when *every* coordinate is a literal
                // number, because the canvas gets one flag per call and a partly
                // computed `hlines(1, threshold)` would offer a drag it cannot
                // honour for one of the two lines.
                let rules = c.literal_rules();
                if !rules.is_empty() {
                    return rules.len() == c.positional.len();
                }
                // An annotation is movable when its two coordinates are literal;
                // a line or path when every vertex is a literal pair. Same rule
                // throughout: what lilook cannot rewrite, it does not offer.
                if c.has_literal_anchor() {
                    return true;
                }
                let vertices = c.literal_vertices();
                if !vertices.is_empty() {
                    return vertices.len() == c.positional.len();
                }
                c.has_literal_points()
            })
            .map(|c| c.id)
            .collect()
    }

    /// Start linking `path`, which must already be readable from the project
    /// root. Asks the compiler what columns it has.
    pub fn begin_link(&mut self, path: impl Into<String>) {
        let path = path.into();
        // A delimited file describes itself with its first row; a keyed one --
        // CBOR, JSON -- with its keys. A transcoded HDF5, npz or FITS file is the
        // second, because that is what lilook wrote it as.
        let expr = crate::SourceKind::of(&path).columns_expr(&path);
        self.queued_query = Some(expr.clone());
        self.link = Some(Link::Asking { path, expr });
    }

    /// Take the answer to the query in `Requests::query`.
    ///
    /// `expr` is echoed back so a stale answer -- one for a link the user has
    /// since abandoned -- is discarded rather than applied to the wrong file.
    pub fn accept_answer(
        &mut self,
        expr: &str,
        answer: Option<crate::Answer>,
        diagnostics: &[Diagnostic],
    ) {
        let Some(Link::Asking { path, expr: asked }) = &self.link else {
            return;
        };
        if asked != expr {
            return;
        }
        let path = path.clone();
        match answer {
            // A delimited file answers with its first row.
            Some(crate::Answer::Strings(row)) if !row.is_empty() => {
                self.link_ready(path, crate::columns_of(&row));
            }
            // A keyed file answers entry by entry, with each one's shape.
            Some(crate::Answer::Fields(entries)) if !entries.is_empty() => {
                // Entries that are not arrays are not columns. A JSON file
                // commonly carries scalar metadata beside its data --
                // `{"title": "run 4", "age": [..]}` -- and offering `title` as
                // something to plot would produce a series of nothing.
                let entries: Vec<_> = entries.into_iter().filter(|(_, n, _)| *n > 0).collect();
                if entries.is_empty() {
                    self.link = Some(Link::Failed {
                        path,
                        why: "no arrays in it -- every entry is a single value".into(),
                    });
                    return;
                }
                let columns = crate::Columns {
                    names: entries.iter().map(|(k, _, _)| k.clone()).collect(),
                    // A keyed file's answer *is* its names; there is no header
                    // row to tell from data.
                    has_header: true,
                    grids: entries
                        .iter()
                        // Outer length is rows, inner is columns -- the same
                        // row-major reading a mesh's field gets everywhere else.
                        .map(|(_, n, m)| (*m > 0).then_some((*m, *n)))
                        .collect(),
                };
                self.link_ready(path, columns);
            }
            _ => {
                let why = diagnostics
                    .iter()
                    .find(|d| d.severity == Severity::Error)
                    .map(|d| d.message.clone())
                    .unwrap_or_else(|| "no rows, or not a delimited file".into());
                self.link = Some(Link::Failed { path, why });
            }
        }
    }

    /// The columns of the file an open link is waiting on, if it is waiting.
    pub fn link_columns(&self) -> Option<&[String]> {
        match &self.link {
            Some(Link::Ready { columns, .. }) => Some(&columns.names),
            _ => None,
        }
    }

    /// Why an open link failed, if it did.
    pub fn link_error(&self) -> Option<&str> {
        match &self.link {
            Some(Link::Failed { why, .. }) => Some(why),
            _ => None,
        }
    }

    /// Finish an open link, taking column `x` against column `y`.
    ///
    /// Returns false when no link is waiting for columns, so a caller that has
    /// lost track of the flow finds out rather than silently doing nothing.
    pub fn confirm_link(&mut self, x: usize, y: usize) -> bool {
        let z = match &self.link {
            Some(Link::Ready { z, .. }) => *z,
            _ => None,
        };
        self.confirm_field_link(x, y, z)
    }

    /// As [`confirm_link`](Self::confirm_link), also giving a mesh its field.
    pub fn confirm_field_link(&mut self, x: usize, y: usize, z: Option<usize>) -> bool {
        let Some(Link::Ready {
            path,
            kind,
            columns,
            ..
        }) = self.link.clone()
        else {
            return false;
        };
        self.commit_link(&path, kind, &columns, x, y, z);
        true
    }

    /// Abandon an open link.
    pub fn cancel_link(&mut self) {
        self.link = None;
    }

    /// A dropped file now lives at `path`, relative to the project root.
    pub fn file_adopted(&mut self, path: impl Into<String>) {
        let path = path.into();
        self.status = format!("{path} is in the project now");
        self.begin_link(path);
    }

    /// Report that a dropped file could not be brought in.
    pub fn adoption_failed(&mut self, name: &str, why: &str) {
        self.status = format!("could not add {name}: {why}");
    }

    /// Offer the file's entries, with sensible slots already chosen.
    ///
    /// A mesh is the only shape that can take a 2-D entry, so a field is offered
    /// as `z` only when the selected series is one; otherwise a file that holds
    /// nothing but a field has nothing to link and says so.
    pub fn link_ready(&mut self, path: String, columns: crate::Columns) {
        let kind = crate::SourceKind::of(&path);
        let (fields, plain) = columns.split_fields();
        let z = self
            .link_target()
            .filter(|_| self.target_is_mesh())
            .and(fields.first().copied());
        // x from the first column and y from the next: the overwhelmingly common
        // shape. For a mesh the field is `z`, so the axes come from what is left,
        // and when nothing is left `commit_link` uses the grid indices.
        let (x, y) = match z {
            Some(_) => (
                plain.first().copied().unwrap_or(0),
                plain
                    .get(1)
                    .copied()
                    .or(plain.first().copied())
                    .unwrap_or(0),
            ),
            None => (0, usize::from(columns.names.len() > 1)),
        };
        if z.is_none() && plain.is_empty() {
            self.link = Some(Link::Failed {
                path,
                why: "holds only two-dimensional data, which needs a colormesh, \
                      contour or mesh to link it to"
                    .into(),
            });
            return;
        }
        self.link = Some(Link::Ready {
            path,
            kind,
            columns,
            x,
            y,
            z,
        });
    }

    /// Is the series a link would land on a mesh?
    pub fn target_is_mesh(&self) -> bool {
        self.link_target()
            .and_then(|n| self.doc.calls().iter().find(|c| c.id == n))
            .is_some_and(|c| c.series_shape() == crate::SeriesShape::Mesh)
    }

    /// Write the link: a `#let` for the file, and the two slots that read it.
    ///
    /// One transaction, so undo takes the whole thing back. The order matters for
    /// the same reason paste's does -- call-site ids are indices into a
    /// document-order walk, so the *slots* are set before the binding is inserted
    /// above them, or the node they name would have moved.
    pub fn commit_link(
        &mut self,
        path: &str,
        kind: crate::SourceKind,
        columns: &crate::Columns,
        x: usize,
        y: usize,
        z: Option<usize>,
    ) {
        let Some(node) = self.link_target() else {
            self.status = "select a series to link the file to".into();
            return;
        };
        let name = crate::binding_name_for(path, |n| self.doc.binding_of(n).is_some());
        // A field's axes may not be in the file at all -- a FITS image is pixels
        // and nothing else -- so an axis that would otherwise name the field
        // becomes the grid's own indices, which is what the pixels are numbered
        // by anyway.
        let axis = |i: usize, n: usize| match columns.grid(i) {
            Some(_) => Some(format!("range({n})")),
            None => crate::column_source(&name, kind, columns, i),
        };
        let (cols, rows) = z.and_then(|i| columns.grid(i)).unwrap_or((0, 0));
        let (Some(xs), Some(ys)) = (axis(x, cols), axis(y, rows)) else {
            self.status = "that column is not in the file".into();
            return;
        };
        let Some(at) = self.import_end() else {
            self.status = "no `#import` to put the data binding after".into();
            return;
        };

        self.doc.begin("link data file");
        self.apply(Intent::SetPositionalArg {
            node,
            index: 0,
            value: xs,
        });
        self.apply(Intent::SetPositionalArg {
            node,
            index: 1,
            value: ys,
        });
        if let Some(zs) = z.and_then(|i| crate::column_source(&name, kind, columns, i)) {
            self.apply(Intent::SetPositionalArg {
                node,
                index: 2,
                value: zs,
            });
        }
        self.apply(Intent::ReplaceRange {
            range: at..at,
            value: format!(
                "\n#let {name} = {}",
                crate::binding_source(path, kind, columns.has_header)
            ),
        });
        self.doc.commit();

        self.link = None;
        self.link_path.clear();
        if self.status.is_empty() {
            let label = |i: usize| {
                columns
                    .names
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| i.to_string())
            };
            self.status = match z {
                Some(i) => format!("linked {path}: {} over {}×{}", label(i), cols, rows),
                None => format!("linked {path}: {} against {}", label(y), label(x)),
            };
        }
    }

    /// Where each of a call's data slots reads its numbers from.
    ///
    /// Worked out from the source, not from anything lilook stored: the slot
    /// expression mentions a binding, and the binding's own text says
    /// `csv("run.csv")`. So provenance cannot drift out of step with the
    /// document, cannot survive a copy into another project as a lie, and needs
    /// no lilook-only format for the compiler to be blind to.
    pub fn slot_sources(&self, call: &crate::CallSite) -> Vec<SlotSource> {
        call.positional
            .iter()
            .map(|slot| {
                let file = self.file_behind(slot);
                let (missing, stale) = match &file {
                    Some(f) => (
                        self.data_files.iter().any(|d| &d.path == f && !d.loaded),
                        self.changed_files.contains(f),
                    ),
                    None => (false, false),
                };
                SlotSource {
                    file,
                    missing,
                    stale,
                }
            })
            .collect()
    }

    /// The file a slot's data comes from, if the source says so.
    ///
    /// One hop: the slot names a binding, and that binding reads a file. Deeper
    /// chains are not followed -- a wrong answer about where data came from is
    /// worse than no answer, and one hop is the shape lilook itself writes.
    pub fn file_behind(&self, slot: &crate::PositionalArg) -> Option<String> {
        if !slot.elements.is_empty() {
            return None; // Literal values: nothing behind them.
        }
        for name in self.doc.free_identifiers(slot.range.clone()) {
            let Some(range) = self.doc.binding_of(&name) else {
                continue;
            };
            if let Some(path) = read_path(&self.doc.text()[range]) {
                return Some(path);
            }
        }
        None
    }

    /// Which series a link writes into: the selected one, if it is a series.
    pub fn link_target(&self) -> Option<usize> {
        let call = self.doc.call(self.selected)?;
        // A generated call is not the user's to edit -- the rule that keeps
        // lilook from writing into a `for` loop's output.
        (call.is_xy_series() && !call.generated).then_some(call.id)
    }

    /// The theme in force, and where it is written.
    ///
    /// The *last* one, because show rules stack and the later transform wraps
    /// the earlier: what the reader sees on top is what the panel should name.
    pub fn active_theme(&self) -> Option<crate::Theme> {
        self.doc.themes().into_iter().rfind(|t| t.document_level)
    }

    /// Apply a theme, replace the one in force, or remove it.
    ///
    /// One transaction and one show rule: `#show: lq.theme.ocean` is the entire
    /// representation, so switching is a byte-range replacement and removing is
    /// a deletion. Nothing is stored outside the document, which is why a themed
    /// figure pasted into another manuscript stays themed.
    pub fn set_theme(&mut self, name: Option<&str>) {
        let current = self.active_theme();
        let lq = self.doc.lilaq_alias();
        let rule = name.map(|n| match self.doc.binding_of(n) {
            // A theme of the user's own is named directly; lilaq's live under
            // the module.
            Some(_) => format!("#show: {n}"),
            None => format!("#show: {lq}.theme.{n}"),
        });
        self.doc.begin("set theme");
        match (current, rule) {
            (Some(t), Some(rule)) => self.apply(Intent::ReplaceRange {
                range: t.range,
                value: rule,
            }),
            // Removing takes the newline with it, or an empty line is left.
            (Some(t), None) => {
                let mut range = t.range;
                if self.doc.text()[..range.start].ends_with('\n') {
                    range.start -= 1;
                }
                self.apply(Intent::ReplaceRange {
                    range,
                    value: String::new(),
                })
            }
            (None, Some(rule)) => {
                let Some(at) = self.import_end() else {
                    self.status = "no lilaq import to place a theme after".into();
                    self.doc.commit();
                    return;
                };
                self.apply(Intent::ReplaceRange {
                    range: at..at,
                    value: format!("\n{rule}"),
                })
            }
            (None, None) => {}
        }
        self.doc.commit();
        self.status = match name {
            Some(n) => format!("theme: {n}"),
            None => "theme removed".into(),
        };
    }

    /// Derive a theme of the user's own from the one in force, under `name`.
    ///
    /// The new theme *composes* rather than copies:
    ///
    /// ```typst
    /// #let mine = it => { show: lq.theme.ocean; it }
    /// #show: mine
    /// ```
    ///
    /// Copying lilaq's body would mean chasing its imports -- `schoolbook` pulls
    /// in `@preview/tiptoe` -- and would silently go stale when lilaq revised a
    /// theme. Composing keeps the base authoritative, and every override added
    /// afterwards is an ordinary `set-*` rule the styles panel already edits.
    pub fn fork_theme(&mut self, name: &str) -> bool {
        let name = crate::binding_name_for(name, |n| self.doc.binding_of(n).is_some());
        let lq = self.doc.lilaq_alias();
        let base = match self.active_theme() {
            Some(t) if t.local => format!("  show: {},\n", t.name),
            Some(t) => format!("  show: {lq}.theme.{},\n", t.name),
            // Deriving from nothing is still a theme -- an empty one to fill in.
            None => String::new(),
        };
        let base = base.replace(",\n", "\n");
        let Some(at) = self.import_end() else {
            self.status = "no lilaq import to place a theme after".into();
            return false;
        };
        self.doc.begin("fork theme");
        // The show rule first: inserting the binding above it would move the
        // range the replacement is about to name.
        match self.active_theme() {
            Some(t) => self.apply(Intent::ReplaceRange {
                range: t.range,
                value: format!("#show: {name}"),
            }),
            None => self.apply(Intent::ReplaceRange {
                range: at..at,
                value: format!("\n#show: {name}"),
            }),
        }
        self.apply(Intent::ReplaceRange {
            range: at..at,
            value: format!("\n#let {name} = it => {{\n{base}  it\n}}"),
        });
        self.doc.commit();
        self.status = format!("theme {name} is yours to edit");
        true
    }

    /// Rename a theme of the user's own, binding and show rule together.
    pub fn rename_theme(&mut self, to: &str) -> bool {
        let Some(theme) = self.active_theme().filter(|t| t.local) else {
            self.status = "only a theme of your own can be renamed".into();
            return false;
        };
        let to =
            crate::binding_name_for(to, |n| n != theme.name && self.doc.binding_of(n).is_some());
        let Some(binding) = self.doc.binding_of(&theme.name) else {
            return false;
        };
        // The name inside the `#let`, found without touching anything else that
        // happens to spell it the same way.
        let text = self.doc.text();
        let Some(off) = text[binding.clone()].find(&theme.name) else {
            return false;
        };
        let at = binding.start + off;
        self.doc.begin("rename theme");
        // Later range first, so the earlier edit does not move it.
        self.apply(Intent::ReplaceRange {
            range: theme.transform.clone(),
            value: to.clone(),
        });
        self.apply(Intent::ReplaceRange {
            range: at..at + theme.name.len(),
            value: to.clone(),
        });
        self.doc.commit();
        self.status = format!("theme renamed to {to}");
        true
    }

    /// Move a series onto a secondary axis, or bring it back to the primary.
    ///
    /// lilaq's twin axis is a nesting: `lq.yaxis(position: right, <series>)`
    /// inside the diagram. So this is a wrap and an unwrap of the call's own
    /// bytes -- no new syntax, and undo takes it back like anything else.
    ///
    /// The series keeps working in the tree and the inspector afterwards, because
    /// the probe reads it through the nesting. What it loses is dragging: lilook
    /// recovers one transform per diagram, and a series on its own axis is not
    /// drawn against that one.
    pub fn set_secondary_axis(&mut self, node: usize, secondary: bool) -> bool {
        let Some(call) = self.doc.call(node).cloned() else {
            return false;
        };
        let on = self.doc.on_secondary_axis(node);
        if on == secondary {
            return false;
        }
        let lq = self.doc.lilaq_alias();
        if secondary {
            let text = self.doc.text()[call.range.clone()].to_string();
            self.doc.begin("second axis");
            self.apply(Intent::ReplaceRange {
                range: call.range,
                value: format!("{lq}.yaxis(position: right, {text})"),
            });
            self.doc.commit();
            self.status =
                "moved to a right-hand axis — it reads there, but cannot be dragged".into();
        } else {
            // Replace the wrapping axis call with the series alone.
            let Some(axis) = self.axis_around(node) else {
                return false;
            };
            let text = self.doc.text()[call.range.clone()].to_string();
            self.doc.begin("one axis");
            self.apply(Intent::ReplaceRange {
                range: axis,
                value: text,
            });
            self.doc.commit();
            self.status = "back on the diagram's own axis".into();
        }
        true
    }

    /// The byte range of the axis call wrapping this series, if there is one.
    fn axis_around(&self, node: usize) -> Option<std::ops::Range<usize>> {
        let mut at = self.doc.call(node)?.parent;
        while let Some(p) = at {
            let call = self.doc.call(p)?;
            if matches!(call.short_name(), "axis" | "xaxis" | "yaxis") {
                return Some(call.range.clone());
            }
            at = call.parent;
        }
        None
    }

    /// What may be written at this offset.
    ///
    /// Schema and parse only -- never a compile. A completion that waits on the
    /// compiler is a bug: the answer is already known before the figure is drawn,
    /// and the whole point is that it arrives while the user is still typing.
    pub fn completions(&self, offset: usize) -> Vec<Completion> {
        let cursor = self.doc.at(offset);
        let Some(call) = cursor.call.and_then(|id| self.doc.call(id)) else {
            return vec![];
        };
        let element = self.schema.element_as_function(&call.callee);
        let Some(f) = element
            .as_ref()
            .or_else(|| self.schema.function_for_callee(&call.callee))
        else {
            return vec![];
        };

        // On a value: what this parameter accepts.
        if let Some(param) = cursor.argument.as_deref() {
            let Some(p) = f.params.iter().find(|p| p.name == param) else {
                return vec![];
            };
            // Inside a string, offer only what belongs in one. `yscale: "l|og"`
            // wants the scale names -- the quotes are how a named variant is
            // written -- but a title is prose, and a parameter list dropped into
            // the middle of someone's words is noise.
            if cursor.in_string && crate::policy::takes_text(Some(p)) {
                return vec![];
            }
            // The schema does not always carry choices -- a named variant like
            // `yscale` has its values in lilaq's own code -- so the same
            // fallbacks the inspector's menus use apply here. One table, so the
            // text pane and the menu cannot offer different things.
            let fallback: &[&str] = match crate::widget_control(&p.widget) {
                Some(crate::Control::Scale) => crate::policy::SCALE_NAMES,
                Some(crate::Control::Mark) => crate::policy::MARK_NAMES,
                _ => &[],
            };
            let choices: Vec<String> = match p.choices.is_empty() {
                true => fallback.iter().map(|s| (*s).to_string()).collect(),
                false => p.choices.clone(),
            };
            let mut out: Vec<Completion> = choices
                .iter()
                .map(|c| Completion {
                    label: c.clone(),
                    insert: format!("\"{c}\""),
                    note: format!("a {} this takes", p.widget),
                })
                .collect();
            let _ = &fallback;
            // The pickers' own tables, so the text pane offers what the
            // inspector does rather than a different, poorer list.
            match crate::widget_control(&p.widget) {
                Some(crate::Control::Colormap) => {
                    out.extend(crate::COLORMAPS.iter().map(|(m, note)| Completion {
                        label: (*m).into(),
                        insert: format!("color.map.{m}"),
                        note: (*note).into(),
                    }))
                }
                Some(crate::Control::Cycle) => {
                    out.extend(crate::CYCLES.iter().map(|(n, expr, note)| Completion {
                        label: (*n).into(),
                        insert: match expr.starts_with('(') {
                            true => (*expr).into(),
                            false => format!("\"{expr}\""),
                        },
                        note: (*note).into(),
                    }))
                }
                _ => {}
            }
            out.extend(p.sentinels.iter().map(|sn| Completion {
                label: sn.clone(),
                insert: sn.clone(),
                note: "leave it to lilaq".into(),
            }));
            return out;
        }

        // A name goes here, and a string is never a name.
        if cursor.in_string {
            return vec![];
        }
        // Otherwise a parameter name. Each is offered once with its safe value,
        // and again per choice where it has a small fixed set -- so picking
        // `smooth` writes `interpolation: "smooth"` in one click instead of
        // completing the name and then having to know the values.
        let mut out = vec![];
        for p in f
            .params
            .iter()
            .filter(|p| !call.named.iter().any(|a| a.name == p.name))
        {
            let control = crate::widget_control(&p.widget);
            let seed = control
                .and_then(|c| crate::policy::seed(Some(p), c))
                .unwrap_or_default();
            out.push(Completion {
                label: p.name.clone(),
                insert: match seed.is_empty() {
                    true => format!("{}: ", p.name),
                    // The policy's safe value, so accepting a completion leaves
                    // a figure that still compiles.
                    false => format!("{}: {seed}", p.name),
                },
                note: p.types.join("|"),
            });
            for (label, value) in values_for(p, control) {
                out.push(Completion {
                    label: format!("{}: {label}", p.name),
                    insert: format!("{}: {value}", p.name),
                    note: p.name.clone(),
                });
            }
        }
        out
    }

    /// Everything worth knowing about what is at an offset, for a hover.
    ///
    /// The whole signature with its parameters, their types and defaults --
    /// which is the thing someone otherwise opens a browser tab to read. Hovering
    /// `lq.colormesh` should not require leaving the figure.
    pub fn describe_at(&self, offset: usize) -> Option<String> {
        let cursor = self.doc.at(offset);
        let call = cursor.call.and_then(|id| self.doc.call(id))?;
        let element = self.schema.element_as_function(&call.callee);
        let f = element
            .as_ref()
            .or_else(|| self.schema.function_for_callee(&call.callee))?;
        let mut out = format!("{}\n", call.short_name());
        let summary = f.doc.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        if !summary.trim().is_empty() {
            out.push_str(&format!("{}\n", summary.trim()));
        }
        out.push('\n');
        for p in &f.params {
            // The one the caret is in, marked, so a long list still answers the
            // question that was asked.
            let here = cursor.argument.as_deref() == Some(p.name.as_str());
            out.push_str(&format!(
                "{} {}: {}{}\n",
                if here { "▸" } else { " " },
                p.name,
                p.types.join("|"),
                p.default
                    .as_deref()
                    .map(|d| format!(" = {d}"))
                    .unwrap_or_default(),
            ));
        }
        Some(out)
    }

    /// The call the caret is inside, described in one line.
    pub fn signature(&self, offset: usize) -> Option<Signature> {
        let cursor = self.doc.at(offset);
        let call = cursor.call.and_then(|id| self.doc.call(id))?;
        let element = self.schema.element_as_function(&call.callee);
        let f = element
            .as_ref()
            .or_else(|| self.schema.function_for_callee(&call.callee))?;
        Some(Signature {
            name: call.short_name().to_string(),
            doc: f
                .doc
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string(),
            params: f.params.iter().map(|p| p.name.clone()).collect(),
            active: cursor.argument.clone(),
        })
    }

    /// Where an unknown name is written, so a fix can replace exactly it.
    ///
    /// The first standalone occurrence: a diagnostic for `ys-stacke` names the
    /// identifier, not a position, and replacing every match would rewrite a
    /// longer name that merely contains it.
    fn find_name(&self, name: &str) -> Option<std::ops::Range<usize>> {
        self.find_name_from(name, 0)
    }

    /// As [`find_name`](Self::find_name), searching from an offset -- so a name
    /// can be found where it is *used* rather than where it is imported.
    fn find_name_from(&self, name: &str, from: usize) -> Option<std::ops::Range<usize>> {
        let text = self.doc.text();
        let boundary = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        let mut from = from;
        while let Some(i) = text[from..].find(name) {
            let at = from + i;
            let before = text[..at].chars().next_back();
            let after = text[at + name.len()..].chars().next();
            if boundary(before) && boundary(after) {
                return Some(at..at + name.len());
            }
            from = at + name.len();
        }
        None
    }

    /// Apply an action, as one undoable step.
    pub fn apply_action(&mut self, action: &Action) {
        self.doc.begin(&action.label);
        for intent in &action.intents {
            self.apply(intent.clone());
        }
        self.doc.commit();
        self.status = action.label.clone();
        self.dirty = true;
    }

    /// What lilook can offer to do about a broken figure.
    ///
    /// Driven by `(message, document)` rather than by a diagnostic's span,
    /// because most of lilaq's errors have no span -- it validates inside its own
    /// package. See `docs/findings.md`. `blame` narrows *where*; this decides
    /// *what to offer*, and the two are independent: an action that knows which
    /// argument is at fault is better, and one that does not is still useful.
    ///
    /// Advisory, always. An action is a label and an `Intent`; applying it is an
    /// ordinary undoable edit that the user asked for.
    pub fn actions(&self, blames: &[crate::Blame]) -> Vec<Action> {
        let mut out = vec![];
        for d in self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
        {
            let msg = d.message.as_str();
            let blamed = |param: &str| {
                blames
                    .iter()
                    .find(|b| b.argument.as_deref() == Some(param))
                    .map(|b| b.node)
            };

            // A log axis given a limit at or below zero. lilaq raises this from
            // inside itself, so without blame there is nothing to point at.
            if msg.contains("strictly positive") {
                for axis in ["x", "y"] {
                    let scale = format!("{axis}scale");
                    let lim = format!("{axis}lim");
                    if let Some(node) = blamed(&scale) {
                        out.push(Action {
                            label: format!("use a linear {axis} axis"),
                            note: "a log axis cannot show zero or negative values".into(),
                            intents: vec![Intent::RemoveNamedArg {
                                node,
                                param: scale.clone(),
                            }],
                        });
                    }
                    if let Some(node) = blamed(&lim) {
                        out.push(Action {
                            label: format!("let lilaq choose the {axis} limits"),
                            note: "the data is positive even where the limit is not".into(),
                            intents: vec![Intent::SetNamedArg {
                                node,
                                param: lim.clone(),
                                value: "auto".into(),
                            }],
                        });
                    }
                }
            }

            // An empty or wrong-length limit array. The data range is known, so
            // the offer can be a real pair of numbers rather than a placeholder.
            if msg.contains("Limit arrays") {
                for b in blames
                    .iter()
                    .filter(|b| matches!(b.argument.as_deref(), Some("xlim") | Some("ylim")))
                {
                    let param = b.argument.clone().unwrap_or_default();
                    let from_data = self.scenes.iter().find(|s| s.figure == b.node).map(|s| {
                        match param.starts_with('x') {
                            true => (s.transform.x.min, s.transform.x.max),
                            false => (s.transform.y.min, s.transform.y.max),
                        }
                    });
                    let value = match from_data {
                        Some((lo, hi)) if lo < hi => {
                            format!("({}, {})", crate::gesture_num(lo), crate::gesture_num(hi))
                        }
                        _ => "auto".into(),
                    };
                    out.push(Action {
                        label: format!("set {param} to {value}"),
                        note: "a limit array holds exactly two numbers".into(),
                        intents: vec![Intent::SetNamedArg {
                            node: b.node,
                            param,
                            value,
                        }],
                    });
                }
            }

            // An unknown name. typst's own hint guesses at subtraction --
            // "if you meant to use subtraction, try `ys - stacke`" -- which is
            // rarely what happened and never what was meant. lilook knows every
            // name actually in scope: the document's own bindings and lilaq's
            // functions. Comparing against those turns a puzzle into a click.
            if let Some(wrong) = msg.strip_prefix("unknown variable: ") {
                let wrong = wrong.trim();
                let mut candidates: Vec<String> = self.doc.binding_names();
                candidates.extend(self.schema.functions.keys().cloned());
                let lq = self.doc.lilaq_alias();
                let near: Vec<(usize, String)> = {
                    let mut v: Vec<(usize, String)> = candidates
                        .iter()
                        .map(|c| (edit_distance(wrong, c), c.clone()))
                        .filter(|(d, c)| *d <= 3 && *d < c.len().max(1))
                        .collect();
                    v.sort();
                    v.dedup_by(|a, b| a.1 == b.1);
                    v.truncate(3);
                    v
                };
                for (_, right) in near {
                    // A lilaq function needs its module; a binding does not.
                    let text = match self.schema.functions.contains_key(&right)
                        && !self.doc.binding_names().contains(&right)
                    {
                        true => format!("{lq}.{right}"),
                        false => right.clone(),
                    };
                    let Some(range) = self.find_name(wrong) else {
                        continue;
                    };
                    out.push(Action {
                        label: format!("did you mean `{text}`?"),
                        note: "the nearest name in scope".into(),
                        intents: vec![Intent::ReplaceRange { range, value: text }],
                    });
                }
            }

            // A parameter that does not exist. The schema knows every name, so
            // the offer is the nearest one rather than a list to read.
            if msg.contains("unknown named") {
                for b in blames.iter().filter(|b| b.argument.is_some()) {
                    let wrong = b.argument.clone().unwrap_or_default();
                    let Some(call) = self.doc.call(b.node) else {
                        continue;
                    };
                    let element = self.schema.element_as_function(&call.callee);
                    let f = element
                        .as_ref()
                        .or_else(|| self.schema.function_for_callee(&call.callee));
                    let near = f.and_then(|f| {
                        f.params
                            .iter()
                            // Not onto a name the call already has: `widht: 8cm`
                            // beside an existing `width: 9cm` cannot be renamed,
                            // only removed. Offering the rename anyway produced
                            // an edit that silently did nothing.
                            .filter(|p| !call.named.iter().any(|a| a.name == p.name))
                            .map(|p| (edit_distance(&wrong, &p.name), p.name.clone()))
                            .filter(|(d, _)| *d <= 3)
                            .min()
                    });
                    match near {
                        Some((_, right)) => {
                            let value = call
                                .named
                                .iter()
                                .find(|a| a.name == wrong)
                                .map(|a| a.text.clone())
                                .unwrap_or_default();
                            out.push(Action {
                                label: format!("rename `{wrong}` to `{right}`"),
                                note: "the nearest parameter this call takes".into(),
                                intents: vec![
                                    Intent::RemoveNamedArg {
                                        node: b.node,
                                        param: wrong.clone(),
                                    },
                                    Intent::InsertNamedArg {
                                        node: b.node,
                                        param: right,
                                        value,
                                    },
                                ],
                            })
                        }
                        None => out.push(Action {
                            label: format!("remove `{wrong}`"),
                            note: match f.is_some_and(|f| f.params.iter().any(|p| {
                                edit_distance(&wrong, &p.name) <= 3
                                    && call.named.iter().any(|a| a.name == p.name)
                            })) {
                                true => "this call does not take it, and the name                                          it resembles is already set"
                                    .into(),
                                false => "this call does not take it".to_string(),
                            },
                            intents: vec![Intent::RemoveNamedArg {
                                node: b.node,
                                param: wrong.clone(),
                            }],
                        }),
                    }
                }
            }
        }
        out
    }

    /// What the compiler resolved, shown where it was left unsaid.
    ///
    /// The capability that exists *only* because of the probe. A language server
    /// can say what a name refers to; it cannot say what `xlim: auto` became,
    /// because that is not in the text -- it is in the rendered figure. lilook
    /// has the recovered transform sitting beside the source, so it can:
    ///
    /// ```typst
    /// #lq.diagram(xlim: auto,   ⟨0.82 … 4.18⟩
    /// ```
    ///
    /// Advisory, like every capability: a hint is a readout and never an edit.
    /// Empty when nothing has compiled, rather than stale from the last thing
    /// that did -- a number that quietly describes a different figure is worse
    /// than no number.
    pub fn hints(&self) -> Vec<Hint> {
        let mut out = vec![];
        for scene in &self.scenes {
            let Some(call) = self.doc.call(scene.figure) else {
                continue;
            };
            for (param, axis, numeric) in [
                ("xlim", scene.transform.x, scene.numeric.0),
                ("ylim", scene.transform.y, scene.numeric.1),
            ] {
                // Only where the user left it to lilaq. A limit they wrote needs
                // no echo, and an axis lilook could not model has nothing true
                // to say.
                if !numeric {
                    continue;
                }
                let Some(arg) = call.named.iter().find(|a| a.name == param) else {
                    continue;
                };
                if crate::policy::sentinel_of(&arg.text, None).is_some()
                    || arg.text.trim() != "auto"
                {
                    continue;
                }
                out.push(Hint {
                    at: arg.value.end,
                    text: format!(
                        "{} … {}",
                        crate::gesture_num(axis.min),
                        crate::gesture_num(axis.max)
                    ),
                    note: format!("what lilaq chose for {param}"),
                });
            }
            // What a slot's data actually amounted to, beside the expression
            // that produced it. `run.map(r => float(r.t))` says nothing about
            // how many rows arrived.
            for geom in &scene.series {
                let Some(series) = self.doc.call(geom.node) else {
                    continue;
                };
                let Some(slot) = series.positional.first() else {
                    continue;
                };
                if slot.elements.len() > 1 {
                    continue; // a literal array already shows its own length
                }
                out.push(Hint {
                    at: slot.range.end,
                    text: geom.summary(),
                    note: "what this slot evaluated to".into(),
                });
            }
        }
        out.sort_by_key(|h| h.at);
        out
    }

    /// Tidy the source: typst's own pretty-printer, as one undoable edit.
    ///
    /// A figure written by hand drifts -- and one written by *lilook* drifts
    /// faster, because a gesture appends an argument wherever the call happens to
    /// end. `typstyle-core` is the formatter tinymist uses, linked as a library
    /// rather than run as a process, so this works in a browser tab too.
    ///
    /// A no-op when the formatter declines. It parses what it is given, and a
    /// buffer mid-edit is often not valid typst; refusing to touch it is right.
    pub fn tidy(&mut self) {
        let text = self.doc.text().to_string();
        let styler = typstyle_core::Typstyle::default();
        let formatted = styler.format_text(&text).render();
        let Ok(out) = formatted else {
            self.status = "cannot tidy this yet -- it does not parse".into();
            return;
        };
        if out == text {
            self.status = "already tidy".into();
            return;
        }
        // As a minimal replacement, so an anchor into the document survives and
        // the undo step is the difference rather than the whole file.
        let Some((range, value)) = crate::minimal_replacement(&text, &out) else {
            return;
        };
        self.doc.begin("tidy");
        self.apply(Intent::ReplaceRange { range, value });
        self.doc.commit();
        self.status = "tidied".into();
        self.dirty = true;
    }

    /// Offset a title or an axis label, in points.
    ///
    /// These have no named places the way a legend does, so the drag becomes
    /// `dx`/`dy`. The value they carry -- `[Time]` -- has to be preserved, so an
    /// argument that is bare content is wrapped in the element that takes the
    /// offsets, and one already wrapped has its offsets rewritten.
    pub fn nudge_decoration(
        &mut self,
        figure: usize,
        kind: crate::scene::Decoration,
        dx: f64,
        dy: f64,
    ) {
        // Where it was when the drag began. Captured once, because the canvas
        // sends the offset *since* the drag started and only the session can read
        // what the title already carried -- without this the first pixel of a
        // drag throws away whatever was already set.
        let base = *self
            .drag_origin
            .get_or_insert_with(|| Session::offsets_of(&self.doc, figure, kind));
        let (dx, dy) = (base.0 + dx, base.1 + dy);
        let Some(call) = self.doc.call(figure).cloned() else {
            return;
        };
        let param = kind.param();
        let Some(arg) = call.named.iter().find(|a| a.name == param) else {
            self.status = format!("this diagram has no {param} to move");
            return;
        };
        let lq = self.doc.lilaq_alias();
        let element = kind.element();
        let head = format!("{lq}.{element}(");
        let text = arg.text.trim().to_string();

        let value = match text.strip_prefix(&head).and_then(|r| r.strip_suffix(')')) {
            // Already wrapped: keep its body and whatever else was set on it,
            // replacing only the offsets.
            Some(inner) => {
                let kept: Vec<&str> = inner
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.starts_with("dx:") && !p.starts_with("dy:") && !p.is_empty())
                    .collect();
                format!(
                    "{head}{}, dx: {}pt, dy: {}pt)",
                    kept.join(", "),
                    crate::gesture_num(dx),
                    crate::gesture_num(dy)
                )
            }
            None => format!(
                "{head}{text}, dx: {}pt, dy: {}pt)",
                crate::gesture_num(dx),
                crate::gesture_num(dy)
            ),
        };
        self.apply(Intent::SetNamedArg {
            node: figure,
            param: param.to_string(),
            value,
        });
    }

    /// The `dx`/`dy` a decoration already carries.
    fn offsets_of(doc: &Document, figure: usize, kind: crate::scene::Decoration) -> (f64, f64) {
        let Some(call) = doc.call(figure) else {
            return (0.0, 0.0);
        };
        let Some(arg) = call.named.iter().find(|a| a.name == kind.param()) else {
            return (0.0, 0.0);
        };
        let read = |key: &str| -> f64 {
            arg.text
                .split(',')
                .map(str::trim)
                .find_map(|p| p.strip_prefix(key))
                .and_then(|v| crate::split_numeric(v.trim()))
                .map(|(n, _)| n)
                .unwrap_or(0.0)
        };
        (read("dx:"), read("dy:"))
    }

    /// Move a figure into its own file, leaving an import behind.
    ///
    /// A `.lil` is **a typst file**. The extension exists so an operating system
    /// knows which application opens it -- lilook cannot claim `.typ` without
    /// taking every typst file from the editor the user already has -- and for
    /// nothing else. Nothing lilook-only is ever written into one: no header, no
    /// version marker, no metadata. The moment it needed one it would have become
    /// a format, and not being a format is the reason lilook is worth using.
    ///
    /// `#import` rather than `#include`, deliberately: it names the figure, it
    /// cannot splice stray content into the host, and it lets the file keep its
    /// own `#set page` for standalone preview without that reaching the paper.
    /// Verified -- an imported page rule does not leak.
    ///
    /// Reversible by [`inline_figure`](Self::inline_figure), so this is a
    /// preference rather than a commitment.
    pub fn extract_figure(&mut self, node: usize, path: &str) -> Option<Extraction> {
        let call = self.doc.call(node)?.clone();
        if call.short_name() != "diagram" {
            self.status = "only a whole diagram can move to its own file".into();
            return None;
        }
        let stem = path
            .rsplit('/')
            .next()?
            .rsplit_once('.')
            .map(|(s, _)| s)
            .unwrap_or(path);
        let name = crate::binding_name_for(stem, |n| self.doc.binding_of(n).is_some());
        let lq = self.doc.lilaq_alias();

        // Everything the figure needs to stand up on its own: the lilaq import,
        // a page that fits it, and whatever bindings its arguments mention.
        let body = self.doc.text()[call.range.clone()].to_string();
        let mut carried = String::new();
        for binding in self.figure_bindings(&call) {
            carried.push_str(self.doc.text()[binding].trim_end());
            carried.push('\n');
        }
        let file = format!(
            "#import \"@preview/lilaq:0.6.0\" as {lq}\n\
             // Previews at its own size; the page rule does not reach a document\n\
             // that imports this file.\n\
             #set page(width: auto, height: auto, margin: 4pt)\n\
             {carried}\n\
             #let {name} = {body}\n\
             \n\
             // Shown when this file is opened on its own.\n\
             #{name}\n"
        );

        self.doc.begin("extract figure");
        self.apply(Intent::ReplaceRange {
            range: call.range.clone(),
            value: name.clone(),
        });
        let at = self.import_end()?;
        self.apply(Intent::ReplaceRange {
            range: at..at,
            value: format!("\n#import \"{path}\": {name}"),
        });
        self.doc.commit();
        self.status = format!("moved to {path}");
        let out = Extraction {
            path: path.to_string(),
            contents: file,
        };
        self.queued_write = Some(out.clone());
        Some(out)
    }

    /// Bring an imported figure back into this document.
    pub fn inline_figure(&mut self, path: &str, contents: &str) -> bool {
        let text = self.doc.text().to_string();
        let Some(line_at) = text.find(&format!("\"{path}\"")) else {
            self.status = format!("nothing here imports {path}");
            return false;
        };
        let start = text[..line_at].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = text[line_at..]
            .find('\n')
            .map(|i| line_at + i)
            .unwrap_or(text.len());
        // What the file bound, so the reference in the host can be replaced by it.
        let other = Document::new(contents);
        let Some(figure) = other
            .figures()
            .first()
            .and_then(|f| other.call(f.node))
            .map(|c| contents[c.range.clone()].to_string())
        else {
            self.status = format!("{path} holds no figure to inline");
            return false;
        };
        let name = text[start..end]
            .rsplit(": ")
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        let Some(used) = self.find_name_from(&name, end) else {
            self.status = format!("nothing in this document uses {name}");
            return false;
        };
        self.doc.begin("inline figure");
        // Later range first, so the earlier edit does not move it.
        self.apply(Intent::ReplaceRange {
            range: used,
            value: figure,
        });
        self.apply(Intent::ReplaceRange {
            range: start..end + 1,
            value: String::new(),
        });
        self.doc.commit();
        self.status = format!("{path} is part of this document now");
        true
    }

    /// The `#let` bindings a figure's arguments mention, so an extracted file
    /// carries what it needs rather than importing a name that is not there.
    fn figure_bindings(&self, call: &crate::CallSite) -> Vec<std::ops::Range<usize>> {
        let mut out = vec![];
        for name in self.doc.free_identifiers(call.range.clone()) {
            if let Some(r) = self.doc.binding_of(&name) {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        out.sort_by_key(|r| r.start);
        out
    }

    /// Ask the shell to locate what causes the errors currently reported.
    pub fn request_blame(&mut self) {
        // Queued, not set directly: a frontend clears `requests` at the top of
        // every frame, so anything asked for from inside one is discarded before
        // the shell sees it. `queued_query` exists for exactly this reason, and
        // this is the second time the trap has been walked into.
        self.queued_blame = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error && d.range.is_none())
            .map(|d| d.message.clone())
            .collect();
    }

    /// Take what the shell found.
    pub fn accept_blame(&mut self, blames: Vec<crate::Blame>) {
        self.blames = blames;
        self.status = match self.blames.len() {
            0 => "nothing in the document accounts for that".into(),
            n => format!("{n} cause(s) found"),
        };
    }

    /// Ask the shell for the figure as a file.
    pub fn request_export(&mut self, format: &str, ppi: f32) {
        self.requests.export = Some((format.to_string(), ppi));
    }

    /// End of the line that imports lilaq: a set rule has to come after it, or
    /// the alias is not in scope.
    pub fn import_end(&self) -> Option<usize> {
        let text = self.doc.text();
        let at = text.find("#import")?;
        let end = text[at..].find('\n').map(|i| at + i)?;
        Some(end)
    }

    /// Apply what the canvas reported: a gesture, in the editor's own vocabulary.
    ///
    /// Public because a gesture is the editor's interface and not an internal
    /// step -- the same events reach it from the desktop window, the browser, and
    /// a test driving a random walk over a corpus of figures without an `egui`
    /// context in sight.
    pub fn handle_canvas(&mut self, events: Vec<CanvasEvent>) {
        for e in events {
            match e {
                CanvasEvent::Select(node) => self.selected = node,
                CanvasEvent::Begin => {
                    self.doc.begin("canvas");
                    self.explicit_tx = true;
                    self.idle_tx = None;
                }
                CanvasEvent::Commit => {
                    self.doc.commit();
                    self.explicit_tx = false;
                    self.drag_origin = None;
                }
                CanvasEvent::SetLimits { figure, x, y } => {
                    // `gesture_num`, not `num`: a limit is a value on a data axis,
                    // and six decimal places writes `3e-9` as `0`. On a log axis
                    // that is a limit a pan reaches legitimately, and lilaq then
                    // refuses the figure -- "value must be strictly positive".
                    //
                    // And the axis's own scale decides what is writable at all.
                    // The canvas pans through `AxisMap::shifted`, which cannot
                    // leave a log axis, but `SetLimits` is the editor's public
                    // vocabulary and reaches it from three shells -- so the
                    // check belongs here, where the document is written, not in
                    // one of the callers. A log axis simply keeps the limits it
                    // had rather than being given a made-up positive number: the
                    // gesture overshot, and inventing a bound the user did not
                    // ask for is worse than declining the one they did.
                    let scales = self
                        .scenes
                        .iter()
                        .find(|s| s.figure == figure)
                        .map(|s| (s.transform.x.kind, s.transform.y.kind));
                    for (param, (lo, hi), scale) in [
                        ("xlim", x, scales.map(|s| s.0)),
                        ("ylim", y, scales.map(|s| s.1)),
                    ] {
                        let logarithmic = scale == Some(crate::AxisScale::Log);
                        if logarithmic && (lo <= 0.0 || hi <= 0.0) {
                            continue;
                        }
                        let value =
                            format!("({}, {})", crate::gesture_num(lo), crate::gesture_num(hi));
                        self.set_or_insert(figure, param, value);
                    }
                }
                CanvasEvent::MoveLegend { figure, to } => {
                    let Some(scene) = self.scenes.iter().find(|s| s.figure == figure) else {
                        continue;
                    };
                    let position = scene.nearest_legend_position(to);
                    // The whole dictionary, because `legend:` takes one and
                    // rewriting a field inside it would mean parsing what the
                    // user wrote there.
                    self.set_or_insert(figure, "legend", format!("(position: {position})"));
                }
                CanvasEvent::MoveDecoration {
                    figure,
                    kind,
                    dx,
                    dy,
                } => {
                    self.nudge_decoration(figure, kind, dx, dy);
                }
                CanvasEvent::SetSize {
                    figure,
                    width_pt,
                    height_pt,
                } => {
                    for (param, value) in [("width", width_pt), ("height", height_pt)] {
                        let Some(pt) = value else { continue };
                        self.set_length(figure, param, pt);
                    }
                }
                CanvasEvent::MoveRule { node, slot, to } => {
                    // One whole positional argument, not an array element: a rule
                    // *is* its coordinate.
                    self.apply(Intent::SetPositionalArg {
                        node,
                        index: slot,
                        value: crate::gesture_num(to),
                    });
                }
                CanvasEvent::MovePoint { node, index, to } => {
                    // Where the coordinates *live* depends on the shape, and the
                    // edit has to match: a plot keeps parallel arrays, an
                    // annotation keeps two scalar arguments, a line keeps an
                    // `(x, y)` array per vertex. All three are two intents with
                    // two coalesce keys, and so one undo step.
                    let shape = self
                        .doc
                        .call(node)
                        .map(|c| c.series_shape())
                        .unwrap_or(crate::SeriesShape::Points);
                    for (which, v) in [(0usize, to.0), (1, to.1)] {
                        let value = crate::gesture_num(v);
                        let intent = match shape {
                            // `place(x, y, ..)`: the coordinates are the arguments.
                            crate::SeriesShape::Anchor => Intent::SetPositionalArg {
                                node,
                                index: which,
                                value,
                            },
                            // `line(start, end)`: vertex `index` is a slot, and the
                            // coordinate is an element inside it.
                            crate::SeriesShape::Vertices => Intent::SetArrayElement {
                                node,
                                arg: index,
                                element: which,
                                value,
                            },
                            // Parallel arrays: slot `which`, element `index`.
                            _ => Intent::SetArrayElement {
                                node,
                                arg: which,
                                element: index,
                                value,
                            },
                        };
                        self.apply(intent);
                    }
                }
            }
        }
    }
}

/// A file the user dropped onto the window.
///
/// Either form can arrive: a desktop drop carries a path, a browser drop carries
/// the bytes, and neither shell gets to see the other's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// The file's own name, without any directory.
    pub name: String,
    pub path: Option<String>,
    pub bytes: Option<Vec<u8>>,
}

/// Numbers going into Typst source: enough precision to be faithful to the
/// gesture, no trailing zeros to clutter the user's file.
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

/// The path a file-reading expression names, if it names one literally.
///
/// Recognises typst's own readers, which is exactly the set a document can use to
/// get data from a file: `csv`, `cbor`, `json`, `toml`, `yaml`, `xml` and `read`.
/// A path built by an expression -- `csv("runs/" + name)` -- deliberately does
/// not match: the *file* is still tracked, because the compiler reports what it
/// read, but claiming to know which literal it was would be a guess.
fn read_path(source: &str) -> Option<String> {
    const READERS: [&str; 7] = ["csv", "cbor", "json", "toml", "yaml", "xml", "read"];
    for reader in READERS {
        let mut from = 0;
        while let Some(at) = source[from..].find(reader) {
            let at = from + at;
            from = at + reader.len();
            // Not a longer identifier that happens to end in `csv`.
            if source[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
            {
                continue;
            }
            let rest = source[from..].trim_start();
            let Some(rest) = rest.strip_prefix('(') else {
                continue;
            };
            let rest = rest.trim_start();
            let Some(rest) = rest.strip_prefix('"') else {
                continue; // Bytes, or a computed path: not a literal to name.
            };
            let mut path = String::new();
            let mut chars = rest.chars();
            loop {
                match chars.next() {
                    // The literal has to *be* the whole argument. In
                    // `csv("runs/" + name)` the path is computed, and answering
                    // "runs/" would be a confident wrong answer.
                    Some('"') => {
                        let after = chars.as_str().trim_start();
                        let whole = after.starts_with(')') || after.starts_with(',');
                        return whole.then_some(path);
                    }
                    Some('\\') => match chars.next() {
                        Some('n') => path.push('\n'),
                        Some(e) => path.push(e),
                        None => return None,
                    },
                    Some(c) => path.push(c),
                    None => return None,
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::read_path;

    #[test]
    fn a_files_path_is_read_out_of_the_expression_that_reads_it() {
        assert_eq!(
            read_path(r#"#let run = csv("run.csv", row-type: dictionary)"#).as_deref(),
            Some("run.csv")
        );
        assert_eq!(
            read_path(r#"#let d = cbor(".lilook/flux.cbor")"#).as_deref(),
            Some(".lilook/flux.cbor")
        );
        assert_eq!(
            read_path(r#"#let t = read("notes.txt")"#).as_deref(),
            Some("notes.txt")
        );
        // Escapes survive, since a file name can contain a quote.
        assert_eq!(
            read_path(r#"csv("a\"b.csv")"#).as_deref(),
            Some(r#"a"b.csv"#)
        );

        // Nothing to name.
        assert_eq!(read_path("#let x = (1, 2, 3)"), None);
        assert_eq!(read_path("#let x = lq.linspace(0, 10)"), None);
        // A computed path is a real link, but not a literal one, so lilook says
        // nothing rather than guessing which file it was.
        assert_eq!(read_path(r#"csv("runs/" + name)"#), None);
        assert_eq!(read_path("cbor(read(p, encoding: none))"), None);
        // An identifier that merely ends in a reader's name is not that reader.
        assert_eq!(read_path(r#"#let x = my-csv("run.csv")"#), None);
        assert_eq!(read_path(r#"#let x = d.read("run.csv")"#), None);
    }
}
