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
    pub changed_files: Vec<String>,
    pub follow_files: bool,
    pub link_path: String,
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
            changed_files: vec![],
            follow_files: false,
            link_path: String::new(),
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
