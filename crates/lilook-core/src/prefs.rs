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

/// What a shell found where the library is kept.
///
/// More than the library itself, because "there is something here I cannot read"
/// is a state a shell has to *act* on rather than merely report.
pub struct Loaded {
    /// What to work with -- empty when nothing could be read.
    pub prefs: Prefs,
    /// The text lilook could not read, carried back rather than dropped.
    ///
    /// `Some` is an instruction to the shell: **do not write over the original**
    /// -- put this somewhere safe first. Reporting the problem is not enough on
    /// its own, because the session carries on with an empty library and the
    /// next thing the user saves would otherwise land on top of everything they
    /// had.
    ///
    /// The case that matters most is a library written by a *newer* lilook. The
    /// older binary understands less by definition, so left to itself it would
    /// replace a format it cannot read with one it can.
    pub rescue: Option<String>,
    /// What to tell the user, in their own terms.
    pub complaint: Option<String>,
}

impl Prefs {
    pub const VERSION: u32 = 1;

    /// Read what a shell found where a library is kept: a file's contents, a
    /// value out of the browser's storage, or `None` for nothing there yet.
    ///
    /// The one place that decides what an unreadable library means, so the
    /// desktop and the browser cannot come to different conclusions about it.
    pub fn load(found: Option<String>) -> Loaded {
        // Nothing there is the ordinary case on a first run, not a problem to
        // announce and nothing to rescue.
        let Some(text) = found else {
            return Loaded {
                prefs: Prefs::default(),
                rescue: None,
                complaint: None,
            };
        };
        match Prefs::from_toml(&text) {
            Ok(prefs) => Loaded {
                prefs,
                rescue: None,
                complaint: None,
            },
            Err(why) => Loaded {
                prefs: Prefs::default(),
                rescue: Some(text),
                complaint: Some(why),
            },
        }
    }

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

#[cfg(test)]
mod loading {
    use super::*;

    #[test]
    fn nothing_yet_is_not_a_problem() {
        let l = Prefs::load(None);
        assert!(l.prefs.saved.is_empty());
        assert!(l.rescue.is_none(), "nothing to rescue");
        assert!(l.complaint.is_none(), "and nothing to complain about");
    }

    #[test]
    fn an_unreadable_library_is_carried_back_whole() {
        // A stray keystroke in a file someone edited by hand.
        let text = "version = 1\n[[saved]]\nkind = \"cycle\"\nname = ";
        let l = Prefs::load(Some(text.into()));
        assert!(l.complaint.is_some(), "said so");
        assert_eq!(
            l.rescue.as_deref(),
            Some(text),
            "and handed back byte for byte, so the shell can set it aside"
        );
    }

    /// The case the rescue exists for: an older lilook opening a newer library.
    ///
    /// It understands less by definition, so it must not be the one to decide
    /// the file's fate.
    #[test]
    fn a_newer_library_is_never_written_over() {
        let text = format!(
            "version = {}\n[[saved]]\nkind = \"cycle\"\nname = \"a\"\nvalue = \"(red,)\"\n",
            Prefs::VERSION + 1
        );
        let l = Prefs::load(Some(text.clone()));
        assert!(l.prefs.saved.is_empty(), "nothing it can safely offer");
        assert_eq!(l.rescue.as_deref(), Some(text.as_str()), "kept in full");
        let complaint = l.complaint.expect("a reason");
        assert!(
            complaint.contains("newer lilook"),
            "in plain words: {complaint}"
        );
    }

    /// A library this lilook *can* read is stored over in the ordinary way --
    /// otherwise every save would pile up rescue copies.
    #[test]
    fn a_good_library_needs_no_rescue() {
        let mut prefs = Prefs::default();
        prefs
            .save(Kind::Cycle, "mine", "(red, blue)")
            .expect("saved");
        let l = Prefs::load(Some(prefs.to_toml()));
        assert_eq!(l.prefs.saved.len(), 1);
        assert!(l.rescue.is_none());
    }
}
