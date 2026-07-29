//! Bake the lilaq package tree into the binary.
//!
//! A browser build has no file system and no package downloader, so every file
//! a compile can read has to be present before it starts. The bundle is built
//! from the local typst package cache rather than vendored into the repository:
//! lilaq is upstream's code, and committing a copy would make the next version
//! bump a merge.
//!
//! Format: repeated `u32 key_len, key, u32 data_len, data`, little-endian. The
//! keys are `MemoryFiles::key` strings, so the runtime does no path handling.

use std::io::Write;
use std::path::{Path, PathBuf};

/// lilaq 0.6.0 and everything it imports. A missing entry fails the build with
/// the package named, rather than failing at runtime inside a browser.
const PACKAGES: &[(&str, &str)] = &[
    ("lilaq", "0.6.0"),
    ("elembic", "1.1.1"),
    ("zero", "0.6.1"),
    ("tiptoe", "0.4.0"),
];

/// What a lilaq compile actually reads. Fonts come from typst-assets instead.
const WANTED: &[&str] = &["typ", "toml"];

fn cache() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("TYPST_PACKAGE_CACHE_PATH") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var("HOME").ok()?;
    [
        format!("{home}/Library/Caches/typst/packages"),
        format!("{home}/.cache/typst/packages"),
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}

fn collect(root: &Path, dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, prefix, out);
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !WANTED.contains(&ext) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path) {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            out.push((format!("{prefix}/{}", rel.to_string_lossy()), bytes));
        }
    }
}

fn main() {
    println!("cargo:rerun-if-env-changed=TYPST_PACKAGE_CACHE_PATH");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("packages.bin");

    let cache = cache().expect(
        "no typst package cache found. Compile any lilaq document once with the \
         typst CLI, or set TYPST_PACKAGE_CACHE_PATH.",
    );

    let mut files: Vec<(String, Vec<u8>)> = vec![];
    for (name, version) in PACKAGES {
        let dir = cache.join("preview").join(name).join(version);
        assert!(
            dir.exists(),
            "{name}:{version} is not in the package cache ({})",
            dir.display()
        );
        println!("cargo:rerun-if-changed={}", dir.display());
        collect(
            &dir,
            &dir,
            &format!("package/preview/{name}/{version}"),
            &mut files,
        );
    }
    files.sort();

    let mut blob = Vec::new();
    for (key, data) in &files {
        blob.write_all(&(key.len() as u32).to_le_bytes()).unwrap();
        blob.write_all(key.as_bytes()).unwrap();
        blob.write_all(&(data.len() as u32).to_le_bytes()).unwrap();
        blob.write_all(data).unwrap();
    }
    std::fs::write(&out, &blob).unwrap();
    println!(
        "cargo:warning=bundled {} package files, {} KiB",
        files.len(),
        blob.len() / 1024
    );
}
