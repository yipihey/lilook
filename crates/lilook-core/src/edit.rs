//! Text edits, anchors and undo history.
//!
//! The document model is the Typst source itself, so history is a text-edit
//! history rather than a widget-tree history. Every entry stores both the
//! replaced and the inserted text, which makes it trivially invertible.

use std::ops::Range;

/// A single applied change: what was there, and what replaced it.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedEdit {
    pub range: Range<usize>,
    pub before: String,
    pub after: String,
}

impl AppliedEdit {
    /// Byte range this edit occupies *after* application.
    pub fn range_after(&self) -> Range<usize> {
        self.range.start..self.range.start + self.after.len()
    }

    pub fn inverse(&self) -> AppliedEdit {
        AppliedEdit {
            range: self.range_after(),
            before: self.after.clone(),
            after: self.before.clone(),
        }
    }
}

/// A group of edits committed as one undo step.
///
/// The edits form a chain: each one's range is expressed in the buffer as it
/// stood after the preceding edit. That is why undo replays them in reverse and
/// redo replays them forwards.
#[derive(Debug, Clone, Default)]
pub struct Transaction {
    pub label: String,
    pub edits: Vec<AppliedEdit>,
}

/// Which target an intent is rewriting. Intents reporting the same key inside
/// one open transaction collapse into a single edit -- this is what makes a
/// slider drag produce one undo entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoalesceKey {
    pub node: usize,
    pub param: String,
}

/// One target being rewritten repeatedly inside an open transaction.
///
/// A pan sets `xlim` *and* `ylim` on every frame, so coalescing against "the
/// last edit" collapses nothing: the two targets interleave and a two-second
/// drag accumulates one edit per parameter per frame. Slots are per target,
/// which is what makes a multi-parameter gesture cost two edits rather than a
/// hundred and twenty.
#[derive(Debug)]
struct Slot {
    key: CoalesceKey,
    /// Range in the buffer as it stood when this slot group began. The emitted
    /// chain is expressed in that space.
    origin: Range<usize>,
    before: String,
    after: String,
    /// Where this slot's text sits in the *live* buffer. Only used to recognise
    /// the next intent for the same target.
    current: Range<usize>,
}

impl Slot {
    fn delta(&self) -> isize {
        self.after.len() as isize - self.before.len() as isize
    }
}

fn shift(r: &Range<usize>, by: isize) -> Range<usize> {
    ((r.start as isize + by) as usize)..((r.end as isize + by) as usize)
}

#[derive(Debug, Default)]
struct Open {
    tx: Transaction,
    slots: Vec<Slot>,
}

impl Open {
    fn record_keyed(&mut self, edit: AppliedEdit, key: CoalesceKey) {
        if let Some(i) = self.slots.iter().position(|s| s.key == key) {
            if self.slots[i].current == edit.range {
                let start = self.slots[i].current.start;
                let delta = edit.after.len() as isize - self.slots[i].after.len() as isize;
                self.slots[i].after = edit.after;
                self.slots[i].current = start..start + self.slots[i].after.len();
                if delta != 0 {
                    // Everything positioned after this slot just moved in the
                    // live buffer; their `origin` is unaffected.
                    for (j, s) in self.slots.iter_mut().enumerate() {
                        if j != i && s.current.start > start {
                            s.current = shift(&s.current, delta);
                        }
                    }
                }
                return;
            }
            // Something else has moved this target since -- the slot can no
            // longer be extended in place, so materialise and start over.
            self.flush();
        }

        // Slots must be disjoint: the shifting below assumes that rewriting one
        // cannot move text *inside* another. Two intents can nest -- replacing
        // a whole positional argument and replacing one element of the array in
        // it are different targets over the same bytes -- so an overlapping
        // arrival materialises everything and starts a fresh group. Found by
        // the random-intent test, which produced exactly that pair.
        if self
            .slots
            .iter()
            .any(|s| s.current.start < edit.range.end && edit.range.start < s.current.end)
        {
            self.flush();
        }

        // A new slot's `origin` is its position before any slot in this group
        // changed length.
        let acc: isize = self
            .slots
            .iter()
            .filter(|s| s.current.start < edit.range.start)
            .map(Slot::delta)
            .sum();
        let after_len = edit.after.len();
        self.slots.push(Slot {
            key,
            origin: shift(&edit.range, -acc),
            before: edit.before,
            after: edit.after,
            current: edit.range.start..edit.range.start + after_len,
        });
    }

    /// Turn the open slots into a valid edit chain and append it.
    fn flush(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        let mut slots = std::mem::take(&mut self.slots);
        slots.sort_by_key(|s| s.origin.start);
        let mut acc: isize = 0;
        for s in slots {
            let range = shift(&s.origin, acc);
            acc += s.delta();
            // A drag that ended where it started leaves no trace.
            if s.before != s.after {
                self.tx.edits.push(AppliedEdit {
                    range,
                    before: s.before,
                    after: s.after,
                });
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    done: Vec<Transaction>,
    undone: Vec<Transaction>,
    open: Option<Open>,
}

impl History {
    pub fn begin(&mut self, label: impl Into<String>) {
        // An already-open transaction is committed rather than dropped.
        self.commit();
        self.open = Some(Open {
            tx: Transaction {
                label: label.into(),
                edits: vec![],
            },
            slots: vec![],
        });
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Record an edit into the open transaction, or as its own atomic step.
    ///
    /// `key` is the target the edit rewrites, as reported by the intent.
    /// Coalescing policy lives here and nowhere else.
    pub fn record(&mut self, edit: AppliedEdit, key: Option<CoalesceKey>) {
        match &mut self.open {
            Some(open) => match key {
                Some(k) => open.record_keyed(edit, k),
                None => {
                    // An unkeyed edit is a hard boundary: slots recorded before
                    // it must be materialised so the chain stays ordered.
                    open.flush();
                    open.tx.edits.push(edit);
                }
            },
            None => {
                self.done.push(Transaction {
                    label: "edit".into(),
                    edits: vec![edit],
                });
                self.undone.clear();
            }
        }
    }

    pub fn commit(&mut self) {
        if let Some(mut open) = self.open.take() {
            open.flush();
            if !open.tx.edits.is_empty() {
                self.done.push(open.tx);
                self.undone.clear();
            }
        }
    }

    pub fn take_undo(&mut self) -> Option<Transaction> {
        self.commit();
        self.done.pop()
    }

    pub fn push_undone(&mut self, tx: Transaction) {
        self.undone.push(tx);
    }

    pub fn take_redo(&mut self) -> Option<Transaction> {
        self.undone.pop()
    }

    pub fn push_done(&mut self, tx: Transaction) {
        self.done.push(tx);
    }

    pub fn depth(&self) -> (usize, usize) {
        (self.done.len(), self.undone.len())
    }
}

/// The smallest replacement that turns `old` into `new`.
///
/// Typing in a source pane produces a whole new buffer, but replacing the whole
/// document on every keystroke would throw away every anchor and make each
/// character a document-sized undo entry. Trimming the common prefix and suffix
/// recovers the edit the user actually made, which is nearly always a few bytes.
///
/// Returns `None` when the texts are equal.
pub fn minimal_replacement(old: &str, new: &str) -> Option<(Range<usize>, String)> {
    if old == new {
        return None;
    }
    let max = old.len().min(new.len());
    let mut start = 0;
    while start < max && old.as_bytes()[start] == new.as_bytes()[start] {
        start += 1;
    }
    // Back off to a character boundary; a split multi-byte character would
    // produce ranges the document could not slice.
    while start > 0 && (!old.is_char_boundary(start) || !new.is_char_boundary(start)) {
        start -= 1;
    }

    let mut back = 0;
    while back < max - start
        && old.as_bytes()[old.len() - 1 - back] == new.as_bytes()[new.len() - 1 - back]
    {
        back += 1;
    }
    while back > 0
        && (!old.is_char_boundary(old.len() - back) || !new.is_char_boundary(new.len() - back))
    {
        back -= 1;
    }

    Some((
        start..old.len() - back,
        new[start..new.len() - back].to_string(),
    ))
}

/// A byte position that survives edits, so GUI selection is not lost on undo.
/// Spans get renumbered by the reparser; anchors do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub offset: usize,
    /// On an edit that starts exactly here, stick to the left edge or the right.
    pub bias_left: bool,
}

impl Anchor {
    pub fn new(offset: usize) -> Self {
        Anchor {
            offset,
            bias_left: true,
        }
    }

    pub fn transform(&mut self, edit: &AppliedEdit) {
        let (start, end) = (edit.range.start, edit.range.end);
        let delta = edit.after.len() as isize - edit.before.len() as isize;
        if self.offset > end || (self.offset == end && !self.bias_left) {
            self.offset = (self.offset as isize + delta) as usize;
        } else if self.offset > start {
            // Inside the replaced span: clamp to whichever edge we bias to.
            self.offset = if self.bias_left {
                start
            } else {
                start + edit.after.len()
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_character_is_a_one_byte_edit() {
        let old = "#lq.plot(x, y, stroke: red)";
        let new = "#lq.plot(x, y, stroke: reds)";
        let (range, text) = minimal_replacement(old, new).unwrap();
        assert_eq!(text, "s");
        assert_eq!(range.len(), 0, "an insertion replaces nothing");
        let mut applied = old.to_string();
        applied.replace_range(range, &text);
        assert_eq!(applied, new);
    }

    #[test]
    fn deletion_and_replacement_round_trip() {
        for (old, new) in [
            ("abc def", "abc"),
            ("abc", "abc def"),
            ("width: 8cm", "width: 12cm"),
            ("", "hello"),
            ("hello", ""),
        ] {
            let (range, text) = minimal_replacement(old, new).expect("they differ");
            let mut applied = old.to_string();
            applied.replace_range(range, &text);
            assert_eq!(applied, new, "{old:?} -> {new:?}");
        }
        assert!(minimal_replacement("same", "same").is_none());
    }

    /// A prefix that ends inside a multi-byte character would produce a range
    /// the document cannot slice.
    #[test]
    fn ranges_land_on_character_boundaries() {
        let old = "label: [Zeit — jetzt]";
        let new = "label: [Zeit – jetzt]";
        let (range, text) = minimal_replacement(old, new).unwrap();
        assert!(old.is_char_boundary(range.start) && old.is_char_boundary(range.end));
        let mut applied = old.to_string();
        applied.replace_range(range, &text);
        assert_eq!(applied, new);
    }
}
