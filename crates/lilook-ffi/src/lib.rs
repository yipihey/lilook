//! C ABI over lilook-core.
//!
//! One boundary serves every non-Rust consumer: Swift on iOS and macOS, Julia
//! and Python for scripting. Intents cross as JSON so the vocabulary can grow
//! without changing the ABI.

// Every entry point here takes pointers the caller owns, and the contract is
// the same for all of them: a handle comes from `lilook_doc_new` and stays
// valid until `lilook_doc_free`, strings are NUL-terminated, and every returned
// string is released with `lilook_string_free`. That is stated once in
// `include/lilook.h` rather than repeated in twelve doc comments.
#![allow(clippy::missing_safety_doc)]

use lilook_core::{Document, Intent};
use std::ffi::{c_char, c_int, CStr, CString};

pub struct LilookDoc(Document);

fn cstr(p: *const c_char) -> Option<&'static str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

fn out(s: String) -> *mut c_char {
    CString::new(s)
        .map(|c| c.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// Create a document from Typst source. Returns null on invalid UTF-8.
#[no_mangle]
pub unsafe extern "C" fn lilook_doc_new(text: *const c_char) -> *mut LilookDoc {
    match cstr(text) {
        Some(t) => Box::into_raw(Box::new(LilookDoc(Document::new(t)))),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn lilook_doc_free(d: *mut LilookDoc) {
    if !d.is_null() {
        unsafe { drop(Box::from_raw(d)) }
    }
}

/// Free any string returned by this library.
#[no_mangle]
pub unsafe extern "C" fn lilook_string_free(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) }
    }
}

/// Current source text. Caller frees with `lilook_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lilook_doc_text(d: *const LilookDoc) -> *mut c_char {
    let Some(d) = (unsafe { d.as_ref() }) else {
        return std::ptr::null_mut();
    };
    out(d.0.text().to_string())
}

/// Indexed lilaq call sites, as JSON. Caller frees with `lilook_string_free`.
#[no_mangle]
pub unsafe extern "C" fn lilook_doc_calls_json(d: *const LilookDoc) -> *mut c_char {
    let Some(d) = (unsafe { d.as_ref() }) else {
        return std::ptr::null_mut();
    };
    let calls: Vec<_> =
        d.0.calls()
            .iter()
            .map(|c| {
                serde_json::json!({
                    "node": c.id,
                    "callee": c.callee,
                    "generated": c.generated,
                    "positional": c.positional.len(),
                    "named": c.named.iter().map(|a| serde_json::json!({
                        "param": a.name,
                        "value": a.text,
                        "editability": format!("{:?}", a.editability).to_lowercase(),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
    out(serde_json::to_string(&calls).unwrap_or_else(|_| "[]".into()))
}

/// Open a coalescing transaction. Which intents collapse into one edit is
/// decided per intent by the core, so there is nothing to declare here.
#[no_mangle]
pub unsafe extern "C" fn lilook_doc_begin(d: *mut LilookDoc, label: *const c_char) -> c_int {
    let Some(d) = (unsafe { d.as_mut() }) else {
        return -1;
    };
    d.0.begin(cstr(label).unwrap_or("edit"));
    0
}

#[no_mangle]
pub unsafe extern "C" fn lilook_doc_commit(d: *mut LilookDoc) -> c_int {
    let Some(d) = (unsafe { d.as_mut() }) else {
        return -1;
    };
    d.0.commit();
    0
}

/// Apply an intent given as JSON, e.g.
/// `{"op":"set-named-arg","node":2,"param":"stroke","value":"red"}`.
/// Returns 0 on success; on failure writes the message to `err` (caller frees).
#[no_mangle]
pub unsafe extern "C" fn lilook_doc_apply_json(
    d: *mut LilookDoc,
    intent_json: *const c_char,
    err: *mut *mut c_char,
) -> c_int {
    let Some(d) = (unsafe { d.as_mut() }) else {
        return -1;
    };
    let Some(s) = cstr(intent_json) else {
        return -1;
    };
    let intent: Intent = match serde_json::from_str(s) {
        Ok(i) => i,
        Err(e) => {
            if !err.is_null() {
                unsafe { *err = out(e.to_string()) };
            }
            return -2;
        }
    };
    match d.0.apply(intent) {
        Ok(()) => 0,
        Err(e) => {
            if !err.is_null() {
                unsafe { *err = out(e) };
            }
            -3
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn lilook_doc_undo(d: *mut LilookDoc) -> c_int {
    match unsafe { d.as_mut() } {
        Some(d) => d.0.undo() as c_int,
        None => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn lilook_doc_redo(d: *mut LilookDoc) -> c_int {
    match unsafe { d.as_mut() } {
        Some(d) => d.0.redo() as c_int,
        None => -1,
    }
}

/// Number of available undo steps.
#[no_mangle]
pub unsafe extern "C" fn lilook_doc_undo_depth(d: *const LilookDoc) -> usize {
    unsafe { d.as_ref() }.map_or(0, |d| d.0.history_depth().0)
}
