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

/// Fail while there is still room to react. A file past this fraction of the
/// cap is one ordinary change away from breaking the integration suite.
const HEADROOM_FRACTION: f64 = 0.90;

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
    let warn_at = (cap as f64 * HEADROOM_FRACTION) as u64;
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
        "these files are within {}% of the {cap}-byte oversized-source cap this \
         crate enforces on its own sources, so tests/integration.rs is about to \
         start skipping them and failing with a temp path instead of a cause. \
         Split them along module seams (see #1801):\n{}",
        ((1.0 - HEADROOM_FRACTION) * 100.0) as u32,
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
