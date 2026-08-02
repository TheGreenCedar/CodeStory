//! Operation-scoped memo for stored-file freshness verdicts.
//!
//! Refresh planning verifies a stored file by hashing its content whenever the
//! observed mtime still matches the stored mtime — the hash *is* the
//! verification, not a redundant double-check, because same-mtime drift is a
//! deliberately defended invariant. One public operation nevertheless derives
//! the same verdict several times: the runtime checks source freshness before
//! and after every build, the MCP transport wraps the same request in a second
//! public operation, and strict retrieval readiness builds its own execution
//! plan. Each of those passes re-hashes every unchanged file.
//!
//! This memo keeps the hash as the verdict and caches the *verdict* for the
//! duration of one armed scope. The key carries everything that could change
//! the answer:
//!
//! * the absolute path,
//! * the observed modification time the verdict was taken at,
//! * the observed byte length the verdict was taken at (a size mismatch proves
//!   drift; sameness proves nothing, which is why the hash still runs on the
//!   first observation), and
//! * the *stored* content hash the observation was compared against, so
//!   re-indexing a file invalidates its memo entry instead of hiding behind it.
//!
//! Two rules keep the memo from weakening the guards it sits behind:
//!
//! 1. Nothing is recorded unless the hashing read completed under the
//!    torn-read guard *and* the metadata it observed still agrees with the
//!    metadata the key was built from. A read that raced a writer records
//!    nothing.
//! 2. The memo is inert unless a scope is armed. Every caller that has not
//!    opted in behaves exactly as it did before this module existed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Upper bound on memoized verdicts inside one scope.
///
/// Bounded discovery caps a freshness check at 25,000 files, but strict
/// retrieval readiness builds an unbounded execution plan. Past this bound the
/// memo stops recording and every further verdict is recomputed from content —
/// slower, never wrong.
const SOURCE_FRESHNESS_MEMO_MAX_ENTRIES: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct VerdictKey {
    path: PathBuf,
    modified_ms: i64,
    len: u64,
    stored_content_hash: String,
}

#[derive(Debug, Default)]
struct ScopeState {
    depth: u32,
    verdicts: HashMap<VerdictKey, bool>,
    content_hash_reads: u64,
    verdict_reuses: u64,
    readiness_fingerprint_passes: u64,
}

thread_local! {
    static SCOPE: RefCell<ScopeState> = RefCell::new(ScopeState::default());
}

/// Counters describing the source-content work one armed scope performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceFreshnessCounts {
    /// Stored files whose content was read and hashed to produce a freshness
    /// verdict inside this scope.
    pub content_hash_reads: u64,
    /// Freshness verdicts served from the memo instead of re-hashing.
    pub verdict_reuses: u64,
    /// Strict-readiness fingerprint passes — each one reads the repository's
    /// lexical source live off disk and streams the projection tables. A warm
    /// operation over an unchanged repository performs exactly one.
    pub readiness_fingerprint_passes: u64,
}

/// Arm the operation-scoped freshness memo for as long as the guard lives.
///
/// Scopes nest: a second `enter` inside an armed scope shares the outer memo
/// and counters, and only the outermost guard clears them. That is what makes
/// the doubly wrapped MCP request path hash a file once instead of twice.
#[derive(Debug)]
pub struct SourceFreshnessScope {
    /// The guard is thread-bound: the memo lives in a thread local, so moving
    /// the guard to another thread would clear a scope it never armed.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl SourceFreshnessScope {
    pub fn enter() -> Self {
        SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            if scope.depth == 0 {
                scope.verdicts.clear();
                scope.content_hash_reads = 0;
                scope.verdict_reuses = 0;
                scope.readiness_fingerprint_passes = 0;
            }
            scope.depth = scope.depth.saturating_add(1);
        });
        Self {
            _not_send: std::marker::PhantomData,
        }
    }
}

impl Drop for SourceFreshnessScope {
    fn drop(&mut self) {
        SCOPE.with(|scope| {
            let mut scope = scope.borrow_mut();
            scope.depth = scope.depth.saturating_sub(1);
            if scope.depth == 0 {
                scope.verdicts.clear();
            }
        });
    }
}

/// Counters for the currently armed scope, or `None` when nothing is armed.
pub fn source_freshness_counts() -> Option<SourceFreshnessCounts> {
    SCOPE.with(|scope| {
        let scope = scope.borrow();
        (scope.depth > 0).then_some(SourceFreshnessCounts {
            content_hash_reads: scope.content_hash_reads,
            verdict_reuses: scope.verdict_reuses,
            readiness_fingerprint_passes: scope.readiness_fingerprint_passes,
        })
    })
}

/// Record one strict-readiness fingerprint pass.
///
/// Called by the retrieval crate's readiness fingerprint so the cost ARCH-002
/// measured is observable in the operation that pays it instead of invisible.
pub fn record_readiness_fingerprint_pass() {
    SCOPE.with(|scope| {
        let mut scope = scope.borrow_mut();
        if scope.depth == 0 {
            return;
        }
        scope.readiness_fingerprint_passes = scope.readiness_fingerprint_passes.saturating_add(1);
    });
}

/// Look up a memoized verdict, counting the reuse when one is found.
pub(crate) fn memoized_verdict(
    path: &Path,
    modified_ms: i64,
    len: u64,
    stored_content_hash: &str,
) -> Option<bool> {
    SCOPE.with(|scope| {
        let mut scope = scope.borrow_mut();
        if scope.depth == 0 {
            return None;
        }
        let key = VerdictKey {
            path: path.to_path_buf(),
            modified_ms,
            len,
            stored_content_hash: stored_content_hash.to_string(),
        };
        let verdict = scope.verdicts.get(&key).copied();
        if verdict.is_some() {
            scope.verdict_reuses = scope.verdict_reuses.saturating_add(1);
        }
        verdict
    })
}

/// Record that a stored file's content was read and hashed for a verdict.
pub(crate) fn record_content_hash_read() {
    SCOPE.with(|scope| {
        let mut scope = scope.borrow_mut();
        if scope.depth == 0 {
            return;
        }
        scope.content_hash_reads = scope.content_hash_reads.saturating_add(1);
    });
}

/// Record a verdict produced by a clean, metadata-agreeing hashing read.
pub(crate) fn record_verdict(
    path: &Path,
    modified_ms: i64,
    len: u64,
    stored_content_hash: &str,
    verdict: bool,
) {
    SCOPE.with(|scope| {
        let mut scope = scope.borrow_mut();
        if scope.depth == 0 || scope.verdicts.len() >= SOURCE_FRESHNESS_MEMO_MAX_ENTRIES {
            return;
        }
        scope.verdicts.insert(
            VerdictKey {
                path: path.to_path_buf(),
                modified_ms,
                len,
                stored_content_hash: stored_content_hash.to_string(),
            },
            verdict,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_memoized_or_counted_without_an_armed_scope() {
        let path = Path::new("/tmp/unscoped-source-freshness");
        record_content_hash_read();
        record_verdict(path, 7, 11, "hash", false);
        assert_eq!(memoized_verdict(path, 7, 11, "hash"), None);
        assert_eq!(source_freshness_counts(), None);
    }

    #[test]
    fn a_scope_reuses_a_recorded_verdict_and_counts_the_reuse() {
        let _scope = SourceFreshnessScope::enter();
        let path = Path::new("/tmp/scoped-source-freshness");
        record_content_hash_read();
        record_readiness_fingerprint_pass();
        record_verdict(path, 7, 11, "hash", false);
        assert_eq!(memoized_verdict(path, 7, 11, "hash"), Some(false));
        assert_eq!(
            source_freshness_counts(),
            Some(SourceFreshnessCounts {
                content_hash_reads: 1,
                verdict_reuses: 1,
                readiness_fingerprint_passes: 1,
            })
        );
    }

    #[test]
    fn every_key_component_separates_verdicts() {
        let _scope = SourceFreshnessScope::enter();
        let path = Path::new("/tmp/keyed-source-freshness");
        record_verdict(path, 7, 11, "hash", false);
        assert_eq!(
            memoized_verdict(Path::new("/tmp/other"), 7, 11, "hash"),
            None
        );
        assert_eq!(memoized_verdict(path, 8, 11, "hash"), None);
        assert_eq!(memoized_verdict(path, 7, 12, "hash"), None);
        assert_eq!(memoized_verdict(path, 7, 11, "other-hash"), None);
    }

    #[test]
    fn a_nested_scope_shares_the_outer_memo_and_only_the_outer_drop_clears_it() {
        let path = Path::new("/tmp/nested-source-freshness");
        let outer = SourceFreshnessScope::enter();
        record_content_hash_read();
        record_verdict(path, 7, 11, "hash", true);
        {
            let _inner = SourceFreshnessScope::enter();
            assert_eq!(memoized_verdict(path, 7, 11, "hash"), Some(true));
        }
        assert_eq!(memoized_verdict(path, 7, 11, "hash"), Some(true));
        assert_eq!(
            source_freshness_counts().map(|counts| counts.content_hash_reads),
            Some(1)
        );
        drop(outer);

        let _next_operation = SourceFreshnessScope::enter();
        assert_eq!(memoized_verdict(path, 7, 11, "hash"), None);
        assert_eq!(
            source_freshness_counts(),
            Some(SourceFreshnessCounts::default())
        );
    }
}
