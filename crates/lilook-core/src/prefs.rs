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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// What came of taking in another library.
#[derive(Default)]
pub struct Merged {
    /// Arrived under the name it had.
    pub added: usize,
    /// Arrived, but under a new name: `(theirs, here)`.
    pub renamed: Vec<(String, String)>,
    /// The same name and the same value -- already here, so nothing to do.
    pub already: usize,
}

impl Merged {
    /// What to tell the user, in one line.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("{} added", self.added));
        }
        if !self.renamed.is_empty() {
            let names: Vec<String> = self
                .renamed
                .iter()
                .map(|(theirs, here)| format!("{theirs} as {here}"))
                .collect();
            parts.push(format!(
                "{} renamed to keep yours: {}",
                self.renamed.len(),
                names.join(", ")
            ));
        }
        if self.already > 0 {
            parts.push(format!("{} already here", self.already));
        }
        match parts.is_empty() {
            true => "nothing in that library".into(),
            false => parts.join("; "),
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
        // Read through a shape that can tell "absent" from "defaulted". Any TOML
        // at all parses as `Prefs` -- both fields have defaults -- so without
        // this a dropped `Cargo.toml` was a library with nothing in it, and the
        // import said so cheerfully instead of saying it was the wrong file.
        #[derive(Deserialize)]
        struct Raw {
            version: Option<u32>,
            #[serde(default)]
            saved: Vec<Saved>,
        }
        let raw: Raw = toml::from_str(text).map_err(|e| e.to_string())?;
        if raw.version.is_none() && raw.saved.is_empty() {
            return Err("no version and nothing saved, so this is not a library".into());
        }
        let prefs = Prefs {
            // Absent in a file written by hand, which is the same as the
            // current format.
            version: raw.version.unwrap_or(Prefs::VERSION),
            saved: raw.saved,
        };
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
    /// Rename something in the library.
    ///
    /// The name is how a saved thing is chosen and how a theme is bound in a
    /// document, so the same rules apply as on the way in: not empty, and not
    /// one already taken by something of its kind.
    pub fn rename(&mut self, kind: Kind, from: &str, to: &str) -> Result<(), String> {
        let to = to.trim();
        if to.is_empty() {
            return Err("a saved thing needs a name".into());
        }
        if to == from {
            return Ok(());
        }
        if self.get(kind, to).is_some() {
            return Err(format!("you already have a {} called {to}", kind.as_str()));
        }
        match self
            .saved
            .iter_mut()
            .find(|s| s.kind == kind && s.name == from)
        {
            Some(s) => {
                s.name = to.to_string();
                Ok(())
            }
            None => Err(format!("no {} called {from}", kind.as_str())),
        }
    }

    /// Take in another library, keeping everything from both.
    ///
    /// A clash renames the *incoming* thing rather than replacing what is here:
    /// someone else's `warm` is not your `warm`, and an import that quietly
    /// overwrote yours would be a worse way to lose a palette than the file
    /// truncation this whole path was built to prevent. An entry identical in
    /// name *and* value is the same thing twice, and is simply already here.
    pub fn merge(&mut self, other: Prefs) -> Merged {
        let mut report = Merged::default();
        for incoming in other.saved {
            match self.get(incoming.kind, &incoming.name) {
                Some(mine) if mine.value == incoming.value => {
                    report.already += 1;
                    continue;
                }
                Some(_) => {
                    let name = self.free_name(incoming.kind, &incoming.name);
                    report.renamed.push((incoming.name.clone(), name.clone()));
                    self.saved.push(Saved { name, ..incoming });
                }
                None => {
                    report.added += 1;
                    self.saved.push(incoming);
                }
            }
        }
        report
    }

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

#[cfg(test)]
mod sharing {
    use super::*;

    fn with(entries: &[(&str, &str)]) -> Prefs {
        let mut p = Prefs::default();
        for (name, value) in entries {
            p.save(Kind::Cycle, name, value).expect("saved");
        }
        p
    }

    /// The rule that makes importing safe: what is already here wins its name.
    #[test]
    fn an_import_never_overwrites_what_is_here() {
        let mut mine = with(&[("warm", "(red, orange)")]);
        let theirs = with(&[("warm", "(pink, brown)")]);
        let report = mine.merge(theirs);
        assert_eq!(
            mine.get(Kind::Cycle, "warm").map(|s| s.value.as_str()),
            Some("(red, orange)"),
            "mine, untouched"
        );
        assert_eq!(
            report.renamed.len(),
            1,
            "and theirs kept under another name"
        );
        let (theirs, here) = &report.renamed[0];
        assert_eq!(theirs, "warm");
        assert_ne!(here, "warm");
        assert!(mine.get(Kind::Cycle, here).is_some(), "and really here");
    }

    /// Importing the same library twice does not double it.
    #[test]
    fn the_same_thing_twice_is_already_here() {
        let mut mine = with(&[("warm", "(red, orange)")]);
        let report = mine.merge(with(&[("warm", "(red, orange)")]));
        assert_eq!(report.already, 1);
        assert_eq!(mine.of(Kind::Cycle).count(), 1, "still one");
    }

    #[test]
    fn renaming_holds_the_same_line_as_saving() {
        let mut p = with(&[("a", "(red,)"), ("b", "(blue,)")]);
        assert!(p.rename(Kind::Cycle, "a", "").is_err(), "no empty names");
        assert!(p.rename(Kind::Cycle, "a", "b").is_err(), "no clashes");
        assert!(
            p.rename(Kind::Cycle, "a", "a").is_ok(),
            "its own name is fine"
        );
        p.rename(Kind::Cycle, "a", "c").expect("renamed");
        assert!(p.get(Kind::Cycle, "c").is_some());
        assert!(p.get(Kind::Cycle, "a").is_none());
        assert!(
            p.rename(Kind::Colormap, "c", "d").is_err(),
            "kinds are separate"
        );
    }

    /// A library survives the round trip it will actually make: written to
    /// TOML, sent to someone else, read back, merged.
    #[test]
    fn a_library_travels() {
        let mut mine = Prefs::default();
        mine.save(Kind::Cycle, "warm", "(red, orange)")
            .expect("saved");
        mine.save(Kind::Colormap, "mine", "(red, blue)")
            .expect("saved");
        mine.save(Kind::Theme, "ocean-ish", "it => it")
            .expect("saved");
        let text = mine.to_toml();

        let mut theirs = Prefs::default();
        let read = Prefs::from_toml(&text).expect("reads back");
        let report = theirs.merge(read);
        assert_eq!(report.added, 3, "all three kinds: {}", report.summary());
        assert_eq!(theirs.of(Kind::Theme).count(), 1);
        assert_eq!(
            theirs.to_toml(),
            text,
            "and comes out the same as it went in"
        );
    }
}

#[cfg(test)]
mod what_counts_as_a_library {
    use super::*;

    /// A file has to say it is one. Every field has a default, so without this
    /// any TOML at all read as a library with nothing in it -- and dropping a
    /// `Cargo.toml` on the window was answered with "nothing in that library"
    /// rather than "that is the wrong file".
    #[test]
    fn an_unrelated_toml_is_not_a_library() {
        let why =
            Prefs::from_toml("[package]\nname = \"something-else\"\n").expect_err("not a library");
        assert!(why.contains("not a library"), "{why}");
    }

    /// Written by hand, with no version line: still a library, because it says
    /// what a library says.
    #[test]
    fn a_hand_written_library_needs_no_version() {
        let p = Prefs::from_toml(
            "[[saved]]\nkind = \"cycle\"\nname = \"mine\"\nvalue = \"(red, blue)\"\n",
        )
        .expect("a library");
        assert_eq!(p.version, Prefs::VERSION, "read as the current format");
        assert_eq!(p.saved.len(), 1);
    }

    /// An empty library that lilook itself wrote is still one.
    #[test]
    fn an_empty_library_lilook_wrote_is_a_library() {
        let p = Prefs::from_toml(&Prefs::default().to_toml()).expect("a library");
        assert!(p.saved.is_empty());
    }
}
