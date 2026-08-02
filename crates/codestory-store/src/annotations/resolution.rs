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
//!
//! The two inferences are looked up from opposite ends, because a rename and a
//! move change opposite halves of the anchor:
//!
//! - a *move* keeps the qualified name and changes the file, so it is found by
//!   name in another file and then checked against the normalized signature.
//!   A unique candidate whose signature disagrees is not the annotated code,
//!   which is a visible `SignatureChanged` orphan;
//! - a *rename* keeps the file and changes the name, so it is found by
//!   normalized signature within the same file and kind.
//!
//! Both depend on the normalized signature being independent of position and
//! of the symbol's own name. `callable_projection_state.signature_hash` is
//! not: it is an incremental-projection change detector that binds both, so a
//! ladder built on it can never fire.
//!
//! The two probes ask different things of that signature, because they rest on
//! different evidence. The move probe already knows *which* symbol it is
//! looking at — the qualified name identifies it — so the signature only has
//! to agree. The rename probe has nothing but the signature, so the signature
//! has to identify code on its own, and an `outline` signature (a callable
//! whose body projected nothing, leaving only kind and line count) cannot.

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
    /// How well this evidence separated the annotated symbol from its
    /// neighbours when it was last proven. Absent evidence refuses inference.
    #[serde(default)]
    pub discrimination: Option<AnchorDiscrimination>,
}

/// Whether an anchor's evidence identified exactly one symbol at bind time.
///
/// A rebind is an inference *from the last proven state*, so the question that
/// matters is not whether a candidate is unique now but whether the evidence
/// was ever discriminating. Normalized signatures collide by design — two
/// same-shaped callables share one — and a bookmark whose signature already
/// matched a sibling cannot become a rename witness merely because that
/// sibling was deleted. Recording the answer at bind time is the only way to
/// ask it, because the previous generation is gone by the time the next one
/// resolves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorDiscrimination {
    /// The normalized signature matched exactly one symbol of this kind in the
    /// anchor's own file.
    pub signature_unique_in_file: bool,
    /// The qualified name matched exactly one symbol of this kind anywhere.
    pub qualified_name_unique: bool,
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

    /// Candidates carrying this `(qualified_name, kind)` pair in any file.
    ///
    /// A move keeps the qualified name and changes the file, so this is the
    /// lookup the move probe needs; the anchor tuple cannot see it.
    fn candidates_by_qualified_name(
        &self,
        qualified_name: &str,
        kind: i64,
    ) -> Vec<CoreAnchorCandidate>;

    /// Candidates carrying this normalized signature inside one file and kind.
    ///
    /// Always scoped: an unscoped signature probe would be a workspace scan,
    /// and no rebind decision needs one.
    fn candidates_by_normalized_signature(
        &self,
        normalized_signature: &str,
        file_identity: &str,
        kind: i64,
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
            CandidateSet::Unique(candidate) => return bound(candidate, generation, index),
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
        CandidateSet::Unique(candidate) => return bound(candidate, generation, index),
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
    let discrimination = bookmark
        .last_known_evidence
        .as_ref()
        .and_then(|evidence| evidence.discrimination);

    // A move keeps the qualified name and changes the file, so it is found by
    // name and then *checked* against the normalized signature. Finding the
    // name somewhere else is not on its own evidence of a move — a signature
    // that disagrees means the code there is not the code the user annotated,
    // and that is a visible `SignatureChanged` orphan rather than a guess.
    let moved = index
        .candidates_by_qualified_name(qualified_name, kind)
        .into_iter()
        .filter(|candidate| candidate.file_identity.as_deref() != Some(file_identity))
        .collect::<Vec<_>>();
    match unique_candidate(moved) {
        CandidateSet::Unique(candidate) => {
            return if !discrimination.is_some_and(|it| it.qualified_name_unique) {
                // The name already named more than one symbol when the anchor
                // was last proven, so "the name turned up elsewhere" is not
                // evidence that this symbol went there.
                orphaned(OrphanReason::AmbiguousMatch)
            } else if candidate.normalized_signature.as_deref() == Some(signature) {
                bound(candidate, generation, index)
            } else {
                orphaned(OrphanReason::SignatureChanged)
            };
        }
        CandidateSet::Ambiguous => return orphaned(OrphanReason::AmbiguousMatch),
        CandidateSet::Empty => {}
    }

    // A rename keeps the file and changes the name, so the signature is the
    // *only* evidence left — which means it has to be evidence. An outline
    // signature says "a callable of this kind, this many lines long" and
    // nothing else, so every stub of the same length shares it; inferring a
    // rename from one would hand a bookmark on a deleted stub to whichever
    // stub happened to survive.
    if !is_shape_signature(signature) {
        return orphaned(OrphanReason::TargetDeleted);
    }
    let renamed_in_place = index.candidates_by_normalized_signature(signature, file_identity, kind);
    match unique_candidate(renamed_in_place) {
        CandidateSet::Unique(candidate) => {
            // A signature that already matched a sibling when the anchor was
            // last proven cannot become a rename witness just because the
            // annotated symbol disappeared: the surviving sibling would be a
            // guess, not an inference.
            if discrimination.is_some_and(|it| it.signature_unique_in_file) {
                bound(candidate, generation, index)
            } else {
                orphaned(OrphanReason::AmbiguousMatch)
            }
        }
        CandidateSet::Ambiguous => orphaned(OrphanReason::AmbiguousMatch),
        CandidateSet::Empty => orphaned(OrphanReason::TargetDeleted),
    }
}

/// Evidence to record for a candidate that is about to become an anchor.
///
/// Shared by the rebind pass and by annotation creation so both record the
/// same discrimination, which is what a later rename or move inference is
/// allowed to rely on.
pub fn anchor_evidence(
    candidate: &CoreAnchorCandidate,
    generation: Option<i64>,
    index: &dyn CoreAnchorIndex,
) -> BookmarkAnchorEvidence {
    BookmarkAnchorEvidence {
        generation,
        node_id: Some(candidate.node_id),
        canonical_id: candidate.canonical_id.clone(),
        file_identity: candidate.file_identity.clone(),
        qualified_name: candidate.qualified_name.clone(),
        kind: candidate.kind,
        normalized_signature: candidate.normalized_signature.clone(),
        start_line: candidate.start_line,
        discrimination: anchor_discrimination(candidate, index),
    }
}

fn anchor_discrimination(
    candidate: &CoreAnchorCandidate,
    index: &dyn CoreAnchorIndex,
) -> Option<AnchorDiscrimination> {
    let kind = candidate.kind?;
    let signature_unique_in_file = match (
        candidate.normalized_signature.as_deref(),
        candidate.file_identity.as_deref(),
    ) {
        (Some(signature), Some(file_identity)) => {
            index
                .candidates_by_normalized_signature(signature, file_identity, kind)
                .len()
                == 1
        }
        _ => false,
    };
    let qualified_name_unique = candidate
        .qualified_name
        .as_deref()
        .is_some_and(|qualified_name| {
            index
                .candidates_by_qualified_name(qualified_name, kind)
                .len()
                == 1
        });
    Some(AnchorDiscrimination {
        signature_unique_in_file,
        qualified_name_unique,
    })
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

/// Tag prefix for a normalized signature that actually projected a body.
///
/// Mirrors `codestory_indexer::CALLABLE_SHAPE_SIGNATURE_TAG`; the store cannot
/// depend on the indexer, so the tag is part of the durable anchor contract.
const SHAPE_SIGNATURE_PREFIX: &str = "shape:";

/// Whether a normalized signature is strong enough to identify code by itself.
fn is_shape_signature(signature: &str) -> bool {
    signature.starts_with(SHAPE_SIGNATURE_PREFIX)
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

fn bound(
    candidate: CoreAnchorCandidate,
    generation: Option<i64>,
    index: &dyn CoreAnchorIndex,
) -> AnnotationResolution {
    AnnotationResolution::Bound {
        node_id: candidate.node_id,
        evidence: anchor_evidence(&candidate, generation, index),
    }
}

fn orphaned(reason: OrphanReason) -> AnnotationResolution {
    AnnotationResolution::Orphaned { reason }
}
