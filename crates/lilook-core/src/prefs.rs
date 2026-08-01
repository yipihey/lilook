//! The user's own library: palettes, colour maps and themes they have saved.
//!
//! **A library, never an input to rendering.** Choosing a saved palette writes
//! its colours into the document, exactly as choosing a built-in one does. What
//! is stored here is the *offer*, not the figure -- because a figure whose
//! appearance depends on the machine it is opened on is a figure that cannot be
//! sent to a co-author, and every other decision in lilook is built on the
//! document being the whole truth. Delete this file and no figure changes.
//!
//! No file system and no browser here. The core owns the shape of a library and
//! how it is written down; where it lives is the shell's business -- a file on a
//! desktop, `localStorage` in a page -- the same split `Extraction` already
//! makes for a figure written out to disk.
//!
//! One table for three kinds, because they are the same thing: a name bound to a
//! typst expression, offered wherever that kind of expression is chosen. A
//! second table per kind would be three formats to migrate the first time one of
//! them grew a field.

use serde::{Deserialize, Serialize};

/// What a saved value is for, which decides the menu it appears in and the
/// argument it is written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A palette for a diagram's series: an array of colours, written to
    /// `cycle`.
    Cycle,
    /// A colour ramp for a field: a gradient or an array, written to `map`.
    Colormap,
    /// A theme: the body of a `#let name = it => {..}`, inserted as a binding
    /// with a show rule to match.
    Theme,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Cycle => "cycle",
            Kind::Colormap => "colormap",
            Kind::Theme => "theme",
        }
    }
}

/// One thing the user saved: a name, and the typst it stands for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub kind: Kind,
    pub name: String,
    /// The expression written into the document when this is chosen. Checked
    /// before it is ever stored -- a library that can hold something unwritable
    /// is a library that breaks a figure later, at a moment nobody connects
    /// with saving it.
    pub value: String,
}

/// Everything the user has saved, as it is written down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// The format's version, so a later one can be read rather than guessed at.
    /// Absent in a file written by hand, which is the same as the current one.
    #[serde(default = "current_version")]
    pub version: u32,
    #[serde(default)]
    pub saved: Vec<Saved>,
}

fn current_version() -> u32 {
    Prefs::VERSION
}

/// An empty library is a *current* one. Derived, this said version 0 -- so the
/// first library anyone saved was stamped with a format that never existed, and
/// a later lilook would have had to guess what 0 meant.
impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            version: Prefs::VERSION,
            saved: vec![],
        }
    }
}

impl Prefs {
    pub const VERSION: u32 = 1;

    /// Read a library. A file that cannot be parsed is reported, never
    /// discarded: silently starting empty is how someone loses a year of
    /// palettes to a stray keystroke in a config file.
    pub fn from_toml(text: &str) -> Result<Prefs, String> {
        let prefs: Prefs = toml::from_str(text).map_err(|e| e.to_string())?;
        if prefs.version > Prefs::VERSION {
            return Err(format!(
                "this library was written by a newer lilook (format {}, this one reads {})",
                prefs.version,
                Prefs::VERSION
            ));
        }
        Ok(prefs)
    }

    /// Write a library, in a form someone can also edit by hand.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// What has been saved of one kind, in the order it was saved.
    pub fn of(&self, kind: Kind) -> impl Iterator<Item = &Saved> {
        self.saved.iter().filter(move |s| s.kind == kind)
    }

    pub fn get(&self, kind: Kind, name: &str) -> Option<&Saved> {
        self.of(kind).find(|s| s.name == name)
    }

    /// Save one, replacing anything of the same kind and name.
    ///
    /// Refuses a value that would not reparse, which is the same rule
    /// `Document::resolve` applies to an edit: the library and the buffer accept
    /// exactly what typst accepts, so nothing can be saved that cannot be used.
    pub fn save(&mut self, kind: Kind, name: &str, value: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a saved value needs a name".into());
        }
        crate::check_expr(value)?;
        let entry = Saved {
            kind,
            name: name.to_string(),
            value: value.to_string(),
        };
        match self
            .saved
            .iter_mut()
            .find(|s| s.kind == kind && s.name == name)
        {
            Some(slot) => *slot = entry,
            None => self.saved.push(entry),
        }
        Ok(())
    }

    /// Forget one. True when there was one to forget.
    pub fn remove(&mut self, kind: Kind, name: &str) -> bool {
        let before = self.saved.len();
        self.saved.retain(|s| !(s.kind == kind && s.name == name));
        self.saved.len() != before
    }

    /// A name that is not taken yet, for "save this as…" starting from a
    /// suggestion. `mine`, then `mine 2`, and so on.
    pub fn free_name(&self, kind: Kind, wanted: &str) -> String {
        let wanted = match wanted.trim().is_empty() {
            true => "my palette",
            false => wanted.trim(),
        };
        if self.get(kind, wanted).is_none() {
            return wanted.to_string();
        }
        (2..)
            .map(|n| format!("{wanted} {n}"))
            .find(|n| self.get(kind, n).is_none())
            .unwrap_or_else(|| wanted.to_string())
    }
}
