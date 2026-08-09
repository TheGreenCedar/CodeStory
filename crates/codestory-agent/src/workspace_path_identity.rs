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
