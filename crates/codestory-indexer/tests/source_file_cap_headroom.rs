//! IDX-SPLIT (#1801): keep this crate's own sources indexable by this crate.
//!
//! `tests/integration.rs` copies `codestory-indexer`'s Rust sources into a temp
//! repository and asserts they index with zero errors. A source file that
//! crosses `DEFAULT_SOURCE_FILE_BYTE_CAP` is skipped as oversized, so that test
//! fails — but it fails naming a temp path and a byte count, which says nothing
//! about which real file grew or why. This guard fails first, names the file,
//! and says what to do.

use codestory_contracts::workspace::DEFAULT_SOURCE_FILE_BYTE_CAP;
use std::fs;
use std::path::{Path, PathBuf};

/// Deliberately an absolute budget, not a fraction of
/// `DEFAULT_SOURCE_FILE_BYTE_CAP`. That cap is admission headroom and moves for
/// reasons that have nothing to do with this crate's file hygiene — when it
/// went from 1 MB to 2 MB, a 90% fraction would have moved the warning line to
/// 1.8 MB against a largest file of 495 KB, leaving the guard passing until
/// `lib.rs` had grown 3.6x. The absolute number is what keeps it honest.
const WARN_AT_BYTES: u64 = 900_000;

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read crate source directory") {
        let path = entry.expect("source dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_crate_source_approaches_the_oversized_source_cap() {
    let cap = DEFAULT_SOURCE_FILE_BYTE_CAP;
    // The guard exists to protect `tests/integration.rs`, which indexes this
    // crate's own sources against the cap; a budget above the cap could not.
    assert!(
        WARN_AT_BYTES < cap,
        "the warning budget must sit below the {cap}-byte cap it protects"
    );
    let warn_at = WARN_AT_BYTES;
    let mut files = Vec::new();
    rust_sources(&crate_src(), &mut files);
    assert!(!files.is_empty(), "found no crate sources to measure");

    let mut crowded = files
        .iter()
        .filter_map(|path| {
            let bytes = fs::metadata(path).ok()?.len();
            (bytes > warn_at).then(|| (bytes, path.clone()))
        })
        .collect::<Vec<_>>();
    crowded.sort_by_key(|(size, _)| std::cmp::Reverse(*size));

    assert!(
        crowded.is_empty(),
        "these files are past the {warn_at}-byte hygiene budget for this crate's \
         own sources (the oversized-source cap it enforces on other repositories \
         is {cap}), so tests/integration.rs is heading for a skip that fails with \
         a temp path instead of a cause. Split them along module seams \
         (see #1801):\n{}",
        crowded
            .iter()
            .map(|(bytes, path)| format!(
                "  {} — {bytes} bytes, {} left",
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy(),
                cap.saturating_sub(*bytes)
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
