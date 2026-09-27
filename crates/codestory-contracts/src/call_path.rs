//! Shared `call-path/v1` request, selector, clause, validated-contract, hash,
//! and projection types.
//!
//! Validation, fact checking, and store adapters stay in runtime.

use serde_json::{Value, json};

/// The contract and its clause anchors come from parsing the published
/// `call-path/v1` grammar, not from a translation the caller supplied.
pub const CONTRACT_INTERPRETATION: &str = "host_supplied";
pub const CLAUSE_GUARD_VERSION: &str = "clause_guard_v1";
pub const COMPACT_PROOF_MAX_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnvalidatedCallPathContract {
    pub source_text: String,
    pub clauses: Vec<ClauseAnchor>,
    pub spec: UnvalidatedCallPathSpec,
}

impl UnvalidatedCallPathContract {
    pub fn new(
        source_text: impl Into<String>,
        clauses: Vec<ClauseAnchor>,
        spec: UnvalidatedCallPathSpec,
    ) -> Self {
        Self {
            source_text: source_text.into(),
            clauses,
            spec,
        }
    }

    pub fn source_text(&self) -> &str {
        &self.source_text
    }

    pub fn clauses(&self) -> &[ClauseAnchor] {
        &self.clauses
    }

    pub fn spec(&self) -> &UnvalidatedCallPathSpec {
        &self.spec
    }

    pub fn into_parts(self) -> (String, Vec<ClauseAnchor>, UnvalidatedCallPathSpec) {
        (self.source_text, self.clauses, self.spec)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClauseAnchor {
    pub clause_id: String,
    pub start: usize,
    pub end: usize,
    pub quote: String,
    pub classification: ClauseClassification,
}

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClauseClassification {
    ResolvedMaterial { fields: Vec<ProofContractField> },
    UnresolvedMaterial { reason: UnresolvedMaterialReason },
    NonMaterial { kind: NonMaterialKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProofContractField {
    Start,
    StepTarget { step: u8 },
    Directness { step: u8 },
    Ordering { step: u8 },
    Relation { step: u8 },
    TraversalProhibition { index: u8 },
    ProjectionExclusion { index: u8 },
}

impl ProofContractField {
    pub fn canonical_name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::StepTarget { .. } => "step_target",
            Self::Directness { .. } => "directness",
            Self::Ordering { .. } => "ordering",
            Self::Relation { .. } => "relation",
            Self::TraversalProhibition { .. } => "traversal_prohibition",
            Self::ProjectionExclusion { .. } => "projection_exclusion",
        }
    }

    pub fn canonical_json(self) -> Value {
        match self {
            Self::Start => json!({ "kind": self.canonical_name() }),
            Self::StepTarget { step }
            | Self::Directness { step }
            | Self::Ordering { step }
            | Self::Relation { step } => json!({
                "kind": self.canonical_name(),
                "step": step,
            }),
            Self::TraversalProhibition { index } | Self::ProjectionExclusion { index } => json!({
                "kind": self.canonical_name(),
                "index": index,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnresolvedMaterialReason {
    MissingSelectorResolution,
    AmbiguousSelectorResolution,
    UnsupportedInterpretation,
}

impl UnresolvedMaterialReason {
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Self::MissingSelectorResolution => "missing_selector_resolution",
            Self::AmbiguousSelectorResolution => "ambiguous_selector_resolution",
            Self::UnsupportedInterpretation => "unsupported_interpretation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum NonMaterialKind {
    Whitespace,
    Punctuation,
    Connector,
    Commentary,
}

impl NonMaterialKind {
    pub fn canonical_name(&self) -> &'static str {
        match self {
            Self::Whitespace => "whitespace",
            Self::Punctuation => "punctuation",
            Self::Connector => "connector",
            Self::Commentary => "commentary",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnvalidatedCallPathSpec {
    pub start: UnvalidatedExactSymbolSelector,
    pub steps: Vec<UnvalidatedDirectCallStep>,
    pub prohibit_traversal_through: Vec<UnvalidatedExactScopeSelector>,
    pub exclude_from_projection: Vec<UnvalidatedExactScopeSelector>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnvalidatedDirectCallStep {
    pub target: UnvalidatedExactSymbolSelector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnvalidatedExactSymbolSelector {
    PinnedNode(PinnedNodeIdentity),
    CanonicalId(String),
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnvalidatedExactScopeSelector {
    PinnedNode(PinnedNodeIdentity),
    CanonicalId(String),
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PinnedNodeIdentity {
    pub project_id: String,
    pub core_generation_id: String,
    pub core_run_id: String,
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExactSymbolSelector {
    PinnedNode(PinnedNodeIdentity),
    CanonicalId(String),
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExactScopeSelector {
    PinnedNode(PinnedNodeIdentity),
    CanonicalId(String),
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectCallStep {
    pub target: ExactSymbolSelector,
}

impl DirectCallStep {
    pub fn new(target: ExactSymbolSelector) -> Self {
        Self { target }
    }

    pub fn target(&self) -> &ExactSymbolSelector {
        &self.target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPathSpec {
    pub start: ExactSymbolSelector,
    pub steps: Vec<DirectCallStep>,
    pub prohibit_traversal_through: Vec<ExactScopeSelector>,
    pub exclude_from_projection: Vec<ExactScopeSelector>,
}

impl CallPathSpec {
    pub fn new(
        start: ExactSymbolSelector,
        steps: Vec<DirectCallStep>,
        prohibit_traversal_through: Vec<ExactScopeSelector>,
        exclude_from_projection: Vec<ExactScopeSelector>,
    ) -> Self {
        Self {
            start,
            steps,
            prohibit_traversal_through,
            exclude_from_projection,
        }
    }

    pub fn start(&self) -> &ExactSymbolSelector {
        &self.start
    }

    pub fn steps(&self) -> &[DirectCallStep] {
        &self.steps
    }

    pub fn traversal_prohibitions(&self) -> &[ExactScopeSelector] {
        &self.prohibit_traversal_through
    }

    pub fn projection_exclusions(&self) -> &[ExactScopeSelector] {
        &self.exclude_from_projection
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCallPathContract {
    pub spec: CallPathSpec,
    pub bound_hashes: ProofHashes,
}

impl ValidatedCallPathContract {
    pub fn new(spec: CallPathSpec, bound_hashes: ProofHashes) -> Self {
        Self { spec, bound_hashes }
    }

    pub fn spec(&self) -> &CallPathSpec {
        &self.spec
    }

    pub fn bound_hashes(&self) -> &ProofHashes {
        &self.bound_hashes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofHashes {
    pub source_text_sha256: String,
    pub contract_digest: String,
}

impl ProofHashes {
    pub fn new(source_text_sha256: impl Into<String>, contract_digest: impl Into<String>) -> Self {
        Self {
            source_text_sha256: source_text_sha256.into(),
            contract_digest: contract_digest.into(),
        }
    }

    pub fn source_text_sha256(&self) -> &str {
        &self.source_text_sha256
    }

    pub fn contract_digest(&self) -> &str {
        &self.contract_digest
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InternalProjection {
    Complete {
        root: Value,
        serialized_size: usize,
    },
    BudgetExceeded {
        root: Value,
        required_complete_size: usize,
        serialized_size: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InternalProjectionError {
    Serialization(String),
    InvalidCompactProjection(String),
}
