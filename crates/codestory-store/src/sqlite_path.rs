//! SQLite open-path normalization for the core store.
//!
//! The core database runs in WAL mode, so every live open materializes
//! `-wal` and `-shm` siblings whose names SQLite derives by *appending* to
//! the main database path. SQLite's Win32 VFS hands those names straight to
//! `CreateFileW` without the extended-length conversion Rust's standard
//! library performs internally, so a process without a `longPathAware`
//! application manifest is capped at `MAX_PATH` (260 characters) no matter
//! what the host's `LongPathsEnabled` registry key says. A main database that
//! opens at 258 characters therefore still fails when SQLite reaches for its
//! 262-character `-wal` sibling, and the error is a bare
//! "unable to open database file".
//!
//! Every production SQLite open in this crate routes its path through
//! [`open_path`] (or [`attach_argument`] / [`observational_uri`], which wrap
//! it) so the extended-length prefix is present before SQLite derives any
//! sibling name.
//!
//! Two invariants keep this conversion safe:
//!
//! * It is applied only at the moment of opening. The converted form is never
//!   stored, hashed, or compared, so project ids, workspace ids, artifact
//!   scope ids, cache keys, and publication digests continue to be computed
//!   from the original path. A verbatim prefix leaking into an identity would
//!   invalidate every existing user's store.
//! * Diagnostics keep the caller's original path. Error messages render the
//!   path the user configured, not `\\?\`-prefixed noise.

use std::path::{Path, PathBuf};

/// SQLite filenames that are not filesystem paths and must never be
/// absolutized into a working-directory-relative file.
const MAGIC_SQLITE_FILENAMES: [&str; 1] = [":memory:"];

/// SQLite filename prefixes that select URI parsing rather than a plain path.
const SQLITE_URI_PREFIX: &str = "file:";

/// Return a form of `path` that SQLite can open regardless of `MAX_PATH`.
///
/// On Windows this is the extended-length (`\\?\`) form; elsewhere it is the
/// path unchanged. Magic SQLite filenames (`:memory:`, an empty temporary
/// name, or a `file:` URI) pass through untouched: absolutizing them would
/// turn a private in-memory or URI-configured database into a stray file in
/// the current working directory.
pub(crate) fn open_path(path: &Path) -> PathBuf {
    if is_magic_sqlite_filename(path) {
        return path.to_path_buf();
    }
    codestory_workspace::paths::sqlite_open_path(path)
}

/// Return the `ATTACH DATABASE` argument for `path`.
///
/// An attached database gets its own `-wal`/`-shm` siblings, so it needs the
/// same extended-length treatment as a primary open.
pub(crate) fn attach_argument(path: &Path) -> String {
    open_path(path).to_string_lossy().into_owned()
}

/// Build the read-only observational URI for `path`.
///
/// `immutable=1` is only expressible through a URI, so the observational
/// reader cannot fall back to a plain filename. The path is converted first
/// and then percent-encoded, which matters on Windows: the extended-length
/// prefix must survive as literal backslashes. A forward-slash spelling
/// (`//?/C:/...`) is *not* equivalent — Win32 disables path normalization
/// only for the literal `\\?\` prefix — and it would additionally make SQLite
/// parse `?` as a URI authority and reject the filename. Percent-encoding
/// every backslash keeps the first character after `file:` off `/`, so
/// SQLite skips authority parsing and decodes the exact verbatim path.
pub(crate) fn observational_uri(path: &Path, immutable: bool) -> String {
    let open_path = open_path(path);
    let encoded = percent_encode_sqlite_uri_path(&sqlite_uri_path_bytes(&open_path));
    format!(
        "file:{encoded}?mode=ro{}",
        if immutable { "&immutable=1" } else { "" }
    )
}

/// True when `path` is a SQLite magic filename rather than a real path.
fn is_magic_sqlite_filename(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    text.is_empty() || MAGIC_SQLITE_FILENAMES.contains(&text) || text.starts_with(SQLITE_URI_PREFIX)
}

/// Raw bytes of the filename SQLite should decode from the URI.
#[cfg(unix)]
fn sqlite_uri_path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn sqlite_uri_path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

/// Percent-encode a filename for the path component of a SQLite `file:` URI.
///
/// Only characters SQLite can safely carry through URI parsing are left
/// literal. Backslash is deliberately *not* literal so Windows verbatim
/// prefixes round-trip as `%5C%5C%3F%5C`.
fn percent_encode_sqlite_uri_path(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() + 24);
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_filenames_are_never_absolutized() {
        for magic in [":memory:", "", "file:cache?mode=memory&cache=shared"] {
            let path = Path::new(magic);
            assert_eq!(open_path(path), path, "{magic} must pass through unchanged");
        }
    }

    #[test]
    fn ordinary_relative_paths_are_not_treated_as_magic() {
        assert!(!is_magic_sqlite_filename(Path::new("cache/codestory.db")));
        assert!(!is_magic_sqlite_filename(Path::new("filestore.db")));
    }

    #[test]
    fn uri_encoder_keeps_posix_paths_readable() {
        assert_eq!(
            percent_encode_sqlite_uri_path(b"/home/dev/.cache/codestory/core.db"),
            "/home/dev/.cache/codestory/core.db"
        );
    }

    #[test]
    fn uri_encoder_escapes_uri_delimiters() {
        assert_eq!(
            percent_encode_sqlite_uri_path(b"/tmp/a b?c#d/core.db"),
            "/tmp/a%20b%3Fc%23d/core.db"
        );
    }

    /// The extended-length prefix must survive URI parsing as literal
    /// backslashes, and the encoded form must not begin with `//` — SQLite
    /// would then parse an authority and reject the filename.
    #[test]
    fn uri_encoder_preserves_a_windows_verbatim_prefix() {
        let encoded = percent_encode_sqlite_uri_path(br"\\?\C:\cache\codestory\core.db");
        assert_eq!(
            encoded, "%5C%5C%3F%5CC:%5Ccache%5Ccodestory%5Ccore.db",
            "verbatim prefix must percent-encode rather than become forward slashes"
        );
        let uri = format!("file:{encoded}?mode=ro");
        assert!(
            !uri[SQLITE_URI_PREFIX.len()..].starts_with('/'),
            "an encoded filename starting with '/' risks SQLite authority parsing: {uri}"
        );
    }

    #[test]
    fn uri_encoder_preserves_a_windows_unc_verbatim_prefix() {
        assert_eq!(
            percent_encode_sqlite_uri_path(br"\\?\UNC\server\share\core.db"),
            "%5C%5C%3F%5CUNC%5Cserver%5Cshare%5Ccore.db"
        );
    }

    #[test]
    fn observational_uri_carries_the_immutable_flag() {
        let path = Path::new("/tmp/codestory/core.db");
        assert!(observational_uri(path, true).ends_with("?mode=ro&immutable=1"));
        assert!(observational_uri(path, false).ends_with("?mode=ro"));
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_open_paths_are_unchanged() {
        // Identity safety: a non-Windows store must keep byte-identical paths
        // so nothing derived from a path can shift under this change.
        for candidate in [
            "/home/dev/.cache/codestory/projects/abc/core.db",
            "/tmp/staged.sqlite",
            "relative/core.db",
        ] {
            let path = Path::new(candidate);
            assert_eq!(open_path(path), path);
            assert_eq!(attach_argument(path), candidate);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_deep_paths_are_unchanged() {
        let deep = format!("/tmp/{}/core.db", "segment".repeat(60));
        let path = Path::new(&deep);
        assert!(deep.len() > 260);
        assert_eq!(open_path(path), path);
    }

    #[cfg(windows)]
    #[test]
    fn windows_open_paths_gain_the_extended_length_prefix() {
        let path = Path::new(r"C:\cache\codestory\core.db");
        let open = open_path(path);
        assert_eq!(open, Path::new(r"\\?\C:\cache\codestory\core.db"));
        assert_eq!(attach_argument(path), r"\\?\C:\cache\codestory\core.db");
    }
}
