//! Conservative annotation rebinding.
//!
//! Two different questions are answered here and they carry different burdens
//! of proof. *Re-resolving* an unchanged anchor — the same durable canonical id,
//! or the same `(file_identity, qualified_name, kind)` tuple — is an identity
//! lookup, so a position-shifting edit or a rebuilt projection simply finds the
//! symbol again. *Rebinding* a changed anchor — a rename or a move — is an
//! inference, so it requires all of: an adjacent core generation, agreeing
//! normalized-signature evidence, and a unique candidate. Uniqueness alone is
//! never enough, and an ambiguous match never guesses: it becomes a visible,
//! user-owned orphan carrying its last known evidence.

use serde::{Deserialize, Serialize};

use crate::annotations::AnnotationBookmark;

/// Durable resolution state stored beside each bookmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionStatus {
    Bound,
    Orphaned,
}

impl ResolutionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bound => "bound",
            Self::Orphaned => "orphaned",
        }
    }

    pub(crate) fn from_str(value: &str) -> Self {
        match value {
            "bound" => Self::Bound,
            _ => Self::Orphaned,
        }
    }
}

/// Why an annotation is not currently bound to a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrphanReason {
    /// No candidate matched the recorded anchor.
    TargetDeleted,
    /// More than one candidate matched; rebinding would be a guess.
    AmbiguousMatch,
    /// Core advanced past the generation the anchor was last proven against,
    /// so the intervening file history is unobserved.
    GenerationGap,
    /// A unique candidate exists but its normalized signature disagrees.
    SignatureChanged,
    /// The anchor carries no durable evidence to resolve from.
    UnresolvableAnchor,
}

impl OrphanReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetDeleted => "target_deleted",
            Self::AmbiguousMatch => "ambiguous_match",
            Self::GenerationGap => "generation_gap",
            Self::SignatureChanged => "signature_changed",
            Self::UnresolvableAnchor => "unresolvable_anchor",
        }
    }

    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "target_deleted" => Some(Self::TargetDeleted),
            "ambiguous_match" => Some(Self::AmbiguousMatch),
            "generation_gap" => Some(Self::GenerationGap),
            "signature_changed" => Some(Self::SignatureChanged),
            "unresolvable_anchor" => Some(Self::UnresolvableAnchor),
            _ => None,
        }
    }
}

/// Evidence observed at the last successful bind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkAnchorEvidence {
    pub generation: Option<i64>,
    pub node_id: Option<i64>,
    pub canonical_id: Option<String>,
    pub file_identity: Option<String>,
    pub qualified_name: Option<String>,
    pub kind: Option<i64>,
    pub normalized_signature: Option<String>,
    pub start_line: Option<i64>,
}

/// Outcome of resolving one bookmark against the live core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationResolution {
    Bound {
        node_id: i64,
        evidence: BookmarkAnchorEvidence,
    },
    Orphaned {
        reason: OrphanReason,
    },
}

impl AnnotationResolution {
    pub fn status(&self) -> ResolutionStatus {
        match self {
            Self::Bound { .. } => ResolutionStatus::Bound,
            Self::Orphaned { .. } => ResolutionStatus::Orphaned,
        }
    }

    pub fn orphan_reason(&self) -> Option<OrphanReason> {
        match self {
            Self::Bound { .. } => None,
            Self::Orphaned { reason } => Some(*reason),
        }
    }

    pub fn node_id(&self) -> Option<i64> {
        match self {
            Self::Bound { node_id, .. } => Some(*node_id),
            Self::Orphaned { .. } => None,
        }
    }
}

/// One core symbol candidate considered during resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreAnchorCandidate {
    pub node_id: i64,
    pub canonical_id: Option<String>,
    pub file_identity: Option<String>,
    pub qualified_name: Option<String>,
    pub kind: Option<i64>,
    pub normalized_signature: Option<String>,
    pub start_line: Option<i64>,
}

/// Core lookups the rebind pass is allowed to perform.
///
/// The trait keeps the policy free of SQL so the ladder can be proven directly,
/// and keeps every lookup selective: no consumer may scan the workspace graph
/// to resolve an annotation.
pub trait CoreAnchorIndex {
    /// Current core publication generation, `None` when core has never
    /// published.
    fn current_generation(&self) -> Option<i64>;

    /// Candidates carrying exactly this durable canonical symbol id.
    fn candidates_by_canonical_id(&self, canonical_id: &str) -> Vec<CoreAnchorCandidate>;

    /// Candidates matching the complete `(file_identity, qualified_name, kind)`
    /// tuple.
    fn candidates_by_anchor_tuple(
        &self,
        file_identity: &str,
        qualified_name: &str,
        kind: i64,
    ) -> Vec<CoreAnchorCandidate>;

    /// Candidates carrying this normalized signature, optionally restricted to
    /// one file identity.
    fn candidates_by_normalized_signature(
        &self,
        normalized_signature: &str,
        file_identity: Option<&str>,
    ) -> Vec<CoreAnchorCandidate>;
}

/// Resolve one bookmark against the live core without ever guessing.
pub fn resolve_bookmark(
    bookmark: &AnnotationBookmark,
    index: &dyn CoreAnchorIndex,
) -> AnnotationResolution {
    let generation = index.current_generation();

    if let Some(canonical_id) = bookmark.canonical_id.as_deref() {
        match unique_candidate(index.candidates_by_canonical_id(canonical_id)) {
            CandidateSet::Unique(candidate) => return bound(candidate, generation),
            CandidateSet::Ambiguous => {
                return orphaned(OrphanReason::AmbiguousMatch);
            }
            CandidateSet::Empty => {}
        }
    }

    let tuple = bookmark
        .file_identity
        .as_deref()
        .zip(bookmark.qualified_name.as_deref())
        .zip(bookmark.kind);
    let Some(((file_identity, qualified_name), kind)) = tuple else {
        return orphaned(if bookmark.canonical_id.is_some() {
            OrphanReason::TargetDeleted
        } else {
            OrphanReason::UnresolvableAnchor
        });
    };

    match unique_candidate(index.candidates_by_anchor_tuple(file_identity, qualified_name, kind)) {
        CandidateSet::Unique(candidate) => return bound(candidate, generation),
        CandidateSet::Ambiguous => return orphaned(OrphanReason::AmbiguousMatch),
        CandidateSet::Empty => {}
    }

    // Beyond this point the anchor itself has to change, so the conservative
    // rebind gates apply.
    let Some(signature) = bookmark.normalized_signature.as_deref() else {
        return orphaned(OrphanReason::TargetDeleted);
    };
    if !is_adjacent_generation(bookmark, generation) {
        return orphaned(OrphanReason::GenerationGap);
    }

    let renamed_in_place = index
        .candidates_by_normalized_signature(signature, Some(file_identity))
        .into_iter()
        .filter(|candidate| candidate.kind == Some(kind))
        .collect::<Vec<_>>();
    match unique_candidate(renamed_in_place) {
        CandidateSet::Unique(candidate) => return bound(candidate, generation),
        CandidateSet::Ambiguous => return orphaned(OrphanReason::AmbiguousMatch),
        CandidateSet::Empty => {}
    }

    let moved = index
        .candidates_by_normalized_signature(signature, None)
        .into_iter()
        .filter(|candidate| {
            candidate.kind == Some(kind)
                && candidate.qualified_name.as_deref() == Some(qualified_name)
        })
        .collect::<Vec<_>>();
    match unique_candidate(moved) {
        CandidateSet::Unique(candidate) => bound(candidate, generation),
        CandidateSet::Ambiguous => orphaned(OrphanReason::AmbiguousMatch),
        CandidateSet::Empty => orphaned(OrphanReason::TargetDeleted),
    }
}

/// Whether the recorded evidence is close enough for an inferred rebind.
///
/// The rebind pass runs on every core-replacing operation, so ordinary use
/// advances one generation at a time. A gap means intervening file history was
/// never observed by this writer, and unobserved history cannot support an
/// inference.
fn is_adjacent_generation(bookmark: &AnnotationBookmark, generation: Option<i64>) -> bool {
    let (Some(current), Some(recorded)) = (
        generation,
        bookmark
            .last_known_evidence
            .as_ref()
            .and_then(|evidence| evidence.generation),
    ) else {
        return false;
    };
    current == recorded || current == recorded + 1
}

enum CandidateSet {
    Empty,
    Unique(CoreAnchorCandidate),
    Ambiguous,
}

fn unique_candidate(mut candidates: Vec<CoreAnchorCandidate>) -> CandidateSet {
    match candidates.len() {
        0 => CandidateSet::Empty,
        1 => CandidateSet::Unique(candidates.remove(0)),
        _ => CandidateSet::Ambiguous,
    }
}

fn bound(candidate: CoreAnchorCandidate, generation: Option<i64>) -> AnnotationResolution {
    AnnotationResolution::Bound {
        node_id: candidate.node_id,
        evidence: BookmarkAnchorEvidence {
            generation,
            node_id: Some(candidate.node_id),
            canonical_id: candidate.canonical_id,
            file_identity: candidate.file_identity,
            qualified_name: candidate.qualified_name,
            kind: candidate.kind,
            normalized_signature: candidate.normalized_signature,
            start_line: candidate.start_line,
        },
    }
}

fn orphaned(reason: OrphanReason) -> AnnotationResolution {
    AnnotationResolution::Orphaned { reason }
}
