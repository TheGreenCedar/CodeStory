//! The path-identity seam between packet sufficiency and the workspace crate.
//!
//! Sufficiency needs exactly one question answered — do two spellings name the
//! same workspace file? — and answering it takes the filesystem identity
//! machinery in `codestory_workspace::same_workspace_path`, which this crate
//! must never link. The trait is the whole seam: the runtime implements it
//! over the real filesystem probe and threads it into sufficiency wherever a
//! packet is assembled.
//!
//! The implementation arrives as a REQUIRED argument at every consumer. The
//! seam must never grow a `Default`, and no consumer may accept an `Option` of
//! it: a defaulted seam is how the LanguageExtraction defect happened — a
//! sibling construction site compiled against the default and silently ran
//! without the real dependency. With no fallback, every new construction site
//! is a compile error until it threads the seam explicitly.

use std::path::Path;

/// Decides whether two paths name the same workspace file.
pub trait WorkspacePathIdentity {
    /// `true` exactly when `left` and `right` resolve to the same workspace
    /// file identity, however differently they are spelled.
    fn same_workspace_path(&self, left: &Path, right: &Path) -> bool;
}

/// Missing-path spelling identity for planning fixtures: two spellings name
/// the same file exactly when their dot-segment-normalized components are
/// equal.
///
/// Planning fixtures name files that never exist on disk, and for a missing
/// path `codestory_workspace::same_workspace_path` falls back to comparing the
/// normalized lexical spelling — which is the verdict this adapter reproduces
/// without linking the workspace crate. It compiles for tests and
/// `test-support` builds only and is never a production stand-in: every
/// production construction site still threads the runtime adapter explicitly,
/// and this type deliberately has no `Default`.
#[cfg(any(test, feature = "test-support"))]
pub struct MissingPathSpellingIdentity;

#[cfg(any(test, feature = "test-support"))]
impl WorkspacePathIdentity for MissingPathSpellingIdentity {
    fn same_workspace_path(&self, left: &Path, right: &Path) -> bool {
        normalize_missing_path_spelling(left) == normalize_missing_path_spelling(right)
    }
}

#[cfg(any(test, feature = "test-support"))]
fn normalize_missing_path_spelling(path: &Path) -> std::path::PathBuf {
    use std::path::Component;

    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized
                    .file_name()
                    .is_some_and(|name| name != std::ffi::OsStr::new(".."))
                {
                    normalized.pop();
                } else if !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}
