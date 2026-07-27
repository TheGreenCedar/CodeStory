//! Platform path identity helpers for product file access.
//!
//! SQLite's Windows VFS hands database paths straight to `CreateFileW`
//! without the extended-length (`\\?\`) conversion that Rust's standard
//! library performs internally. A process without a `longPathAware`
//! application manifest is therefore capped at `MAX_PATH` (260 characters)
//! for every SQLite file — including derived siblings such as `-journal`
//! and `-wal`, which SQLite names by appending to the main database path —
//! regardless of the `LongPathsEnabled` registry key. Deep cache roots
//! (service profiles, redirected homes, isolated test roots) cross that cap
//! in practice, so product SQLite opens must route through
//! [`sqlite_open_path`].

use std::path::Path;
use std::path::PathBuf;

/// Return a form of `path` that SQLite can open regardless of `MAX_PATH`.
///
/// On Windows the path is absolutized with `GetFullPathNameW` semantics and
/// converted to extended-length form (`\\?\C:\...` or `\\?\UNC\...`), which
/// `CreateFileW` accepts at any length even without a `longPathAware`
/// manifest or the `LongPathsEnabled` registry key. SQLite derives journal
/// and WAL names by appending to this exact string, so those siblings
/// inherit the prefix. Already-verbatim and device-namespace paths pass
/// through unchanged.
#[cfg(windows)]
pub fn sqlite_open_path(path: &Path) -> PathBuf {
    // `std::path::absolute` uses `GetFullPathNameW` semantics: it resolves
    // relative paths and `.`/`..` components and rewrites `/` separators.
    // That normalization must happen before prefixing because Win32 skips
    // normalization entirely for `\\?\` paths.
    let absolute = match std::path::absolute(path) {
        Ok(absolute) => absolute,
        // An empty or otherwise unabsolutizable path keeps today's behavior
        // and surfaces SQLite's own open error instead.
        Err(_) => return path.to_path_buf(),
    };
    match absolute.to_str().and_then(windows_extended_length_form) {
        Some(extended) => PathBuf::from(extended),
        // Non-UTF-8, already-verbatim, device-namespace, or otherwise
        // unrecognized forms pass through unchanged.
        None => absolute,
    }
}

/// Non-Windows platforms have no `MAX_PATH` cliff; the path is returned
/// unchanged.
#[cfg(not(windows))]
pub fn sqlite_open_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// Pure conversion of an absolute, already-normalized Windows path string
/// into its extended-length form.
///
/// Returns `None` when the path must pass through unchanged: already
/// verbatim (`\\?\`), a device-namespace path (`\\.\`), or not a recognized
/// drive/UNC absolute form.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_extended_length_form(path: &str) -> Option<String> {
    if path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return None;
    }
    if let Some(server_share) = path.strip_prefix(r"\\") {
        return Some(format!(r"\\?\UNC\{server_share}"));
    }
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return Some(format!(r"\\?\{path}"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_paths_gain_the_extended_length_prefix() {
        assert_eq!(
            windows_extended_length_form(r"C:\cw\CodeStory\vectors.sqlite3").as_deref(),
            Some(r"\\?\C:\cw\CodeStory\vectors.sqlite3")
        );
    }

    #[test]
    fn extended_length_form_carries_paths_beyond_max_path() {
        let deep = format!(r"C:\{}\vectors.sqlite3", "a".repeat(300));
        let extended = windows_extended_length_form(&deep).expect("extended-length form");
        assert_eq!(extended, format!(r"\\?\{deep}"));
        assert!(extended.len() > 260);
    }

    #[test]
    fn unc_paths_use_the_unc_namespace() {
        assert_eq!(
            windows_extended_length_form(r"\\server\share\cache\vectors.sqlite3").as_deref(),
            Some(r"\\?\UNC\server\share\cache\vectors.sqlite3")
        );
    }

    #[test]
    fn verbatim_and_device_namespace_paths_pass_through() {
        assert_eq!(
            windows_extended_length_form(r"\\?\C:\cw\vectors.sqlite3"),
            None
        );
        assert_eq!(
            windows_extended_length_form(r"\\?\UNC\server\share\vectors.sqlite3"),
            None
        );
        assert_eq!(windows_extended_length_form(r"\\.\pipe\codestory"), None);
    }

    #[test]
    fn relative_and_rootless_forms_are_not_prefixed() {
        assert_eq!(windows_extended_length_form(r"cache\vectors.sqlite3"), None);
        assert_eq!(windows_extended_length_form(r"C:vectors.sqlite3"), None);
        assert_eq!(windows_extended_length_form(""), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_paths_are_returned_unchanged() {
        let path = Path::new("/tmp/cache/retrieval/semantic/collections/vectors.sqlite3");
        assert_eq!(sqlite_open_path(path), path);
    }

    #[cfg(windows)]
    #[test]
    fn windows_open_path_round_trips_beyond_max_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let segment = "segment".repeat(12);
        let parent = dir.path().join(&segment).join(&segment).join(&segment);
        std::fs::create_dir_all(&parent).expect("create long parent");
        let path = parent.join("vectors.sqlite3");

        let open_path = sqlite_open_path(&path);
        let open_text = open_path.to_str().expect("UTF-8 open path");
        assert!(open_text.starts_with(r"\\?\"));
        assert!(open_text.len() > 260);
        assert_eq!(sqlite_open_path(&open_path), open_path);

        std::fs::write(&open_path, b"probe").expect("write via extended-length path");
        assert_eq!(
            std::fs::read(&path).expect("read via original path"),
            b"probe"
        );
    }
}
