//! Locating an error that names no location, by asking the compiler.
//!
//! lilaq validates through `elembic`, inside its own package, so most of what a
//! user hits arrives with no span: "Limit arrays must contain exactly two
//! items" and nothing about *which* limit array. `docs/findings.md` has the
//! measurement -- four of the six commonest failures are like this.
//!
//! The compiler can still answer, just not by being asked. Remove one thing from
//! a scratch copy, compile, and see whether the error survives; the removal that
//! clears it names the culprit. Delta debugging, on a document small enough that
//! it is simply affordable: **~4 ms a variant**, because a failing compile stops
//! early and comemo has already cached everything that did not change.
//!
//! Two properties make this better than matching on the message text:
//!
//! - It produces the **byte range the diagnostic was missing**, in the user's own
//!   buffer, so a frontend can point at it.
//! - It is *evidence* rather than a guess. The claim is falsifiable and was
//!   falsified: the document without this argument does not have this error.
//!
//! It never touches the user's document. Every variant is a scratch clone.

use lilook_core::{Blame, Document, Intent};

/// How many variants to compile before giving up.
///
/// A figure with more candidates than this is rare, and a user waiting on an
/// answer is not. At ~4 ms each this is a fifth of a second in the worst case.
const MAX_VARIANTS: usize = 48;

/// Find what is responsible for `message`, by removing candidates one at a time.
///
/// Returns every removal that cleared the error, in the order tried -- named
/// arguments first, because they are the most specific thing to point at, and a
/// figure that fails because of `xlim: ()` should blame the limit rather than the
/// diagram that carries it.
///
/// More than one result is meaningful rather than ambiguous: `schoolbook` with a
/// log axis fails because of the *pair*, and either removal fixes it. Saying so
/// is more use than picking one.
pub fn locate<L>(backend: &mut crate::Backend<L>, doc: &Document, message: &str) -> Vec<Blame>
where
    L: typst_kit::files::FileLoader + Send + Sync,
{
    let mut out = vec![];
    let mut budget = MAX_VARIANTS;
    let still_fails = |backend: &mut crate::Backend<L>, text: &str| {
        backend
            .render(text, 1.0)
            .errors()
            .any(|d| d.message == message)
    };

    // The document has to *have* this error before anything can be blamed for
    // it. Without this check an error nobody had is "cleared" by removing
    // anything at all, and the whole document is indicted.
    if !still_fails(backend, doc.text()) {
        return out;
    }

    // Named arguments first: the most specific thing that can be pointed at.
    for call in doc.calls() {
        for arg in &call.named {
            if budget == 0 {
                return out;
            }
            budget -= 1;
            let mut scratch = Document::new(doc.text());
            let ok = scratch
                .apply(Intent::RemoveNamedArg {
                    node: call.id,
                    param: arg.name.clone(),
                })
                .is_ok();
            if ok && !still_fails(backend, scratch.text()) {
                out.push(Blame {
                    node: call.id,
                    argument: Some(arg.name.clone()),
                    range: arg.value.clone(),
                    label: format!("{}: {}", arg.name, first_line(&arg.text)),
                });
            }
        }
    }
    // Then whole calls -- a series, a set rule, a theme.
    for call in doc.calls() {
        if budget == 0 {
            return out;
        }
        // A call already blamed through one of its arguments adds nothing.
        if out.iter().any(|b| b.node == call.id) {
            continue;
        }
        budget -= 1;
        let mut scratch = Document::new(doc.text());
        let ok = scratch.apply(Intent::RemoveNode { node: call.id }).is_ok();
        if ok && !still_fails(backend, scratch.text()) {
            out.push(Blame {
                node: call.id,
                argument: None,
                range: call.range.clone(),
                label: call.short_name().to_string(),
            });
        }
    }
    out
}

fn first_line(text: &str) -> String {
    let t = text.trim().lines().next().unwrap_or("").trim();
    match t.chars().count() > 24 {
        true => format!("{}…", t.chars().take(23).collect::<String>()),
        false => t.to_string(),
    }
}
