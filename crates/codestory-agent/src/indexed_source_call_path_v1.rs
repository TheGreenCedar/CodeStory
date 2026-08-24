//! Pure dark kernel for the `indexed_source_call_path_v1` proof domain.
//!
//! The domain is an ordered sequence of direct, outgoing, indexed source-level
//! `CALL` edges. A step's target is the next step's source. The kernel proves
//! neither runtime execution nor general reachability, data flow, ownership,
//! elapsed time, or the non-participation of excluded scopes. It performs no
//! discovery, fuzzy matching, graph traversal, store access, or source read.
//! Callers must resolve selectors and verify receipts before constructing the
//! facts accepted here.
//!
//! This module is compiled only for crate tests or sealed support features. No
//! production dispatcher enables either support feature.

#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, NodeId};
use codestory_contracts::proof_resolution::{
    EXACT_CALL_RESOLUTION_ALGORITHM, INTERNAL_RESOLUTION_PRODUCER,
    PROOF_RESOLUTION_FACT_SCHEMA_VERSION, ResolutionEvidence, ResolutionProvenance,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const PROOF_CONTRACT_SCHEMA_VERSION: u32 = 1;
pub const PROOF_DOMAIN: &str = "indexed_source_call_path_v1";
pub const CLAUSE_GUARD_VERSION: &str = "clause_guard_v1";
const DIGEST_DOMAIN_SEPARATOR: &[u8] = b"codestory.proof-contract.digest.v1\0";
const MIN_STEPS: usize = 1;
const MAX_STEPS: usize = 6;
const MAX_INDEXED_SCOPES: usize = u8::MAX as usize + 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedRawCallEdge {
    pub edge_id: EdgeId,
    pub file_node_id: NodeId,
    pub line: u32,
    pub column_or_ordinal: u32,
    pub raw_target: NodeId,
    pub callsite_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawCallEdgeAdmission {
    Admitted(AdmittedRawCallEdge),
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RawAdmissionFailure {
    WrongKind,
    WrongEffectiveSource,
    WrongEffectiveTarget,
    MissingExactResolvedTarget,
    CandidateAlternativesRetained,
    MissingFileNode,
    MissingLine,
    InvalidOrLegacyCallsiteIdentity,
    CallsiteFileMismatch,
    CallsiteLineMismatch,
    CallsiteRawTargetMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableContainmentEvidence {
    pub file_node_id: NodeId,
    pub owner_node_id: NodeId,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedLineWindow {
    pub kind: &'static str,
    pub project_file_components: Vec<String>,
    pub indexed_sha256: String,
    pub observed_sha256: String,
    pub anchor_line: u32,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedCallEdgeReceipt {
    pub receipt: ReceiptRef,
    pub source: ResolvedNodeIdentity,
    pub target: ResolvedNodeIdentity,
    pub resolution_fact_id: String,
    pub resolution_evidence_sha256: String,
    pub resolution_evidence_chain: Vec<ResolutionEvidence>,
    pub resolution_provenance: ResolutionProvenance,
    pub exact_callsite_start_byte: u64,
    pub callsite_identity: String,
    pub column_or_ordinal: u32,
    pub containment: CallableContainmentEvidence,
    pub line_window: IndexedLineWindow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalCorePublicationIdentity {
    pub project_id: String,
    pub generation_id: String,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum FactBuildGap {
    SelectorMissing { selector_index: usize },
    SelectorAmbiguous { selector_index: usize },
    NonCallableSelector { selector_index: usize },
    DirectCallMissing { step_index: usize },
    RecursiveCallNotRepresentable { step_index: usize },
    SourceWindowTooLarge { step_index: usize },
    InvalidUtf8 { step_index: usize },
    SourceLineOutOfRange { step_index: usize },
    EdgeContainmentUnproven { step_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltCallPathFacts {
    pub publication: InternalCorePublicationIdentity,
    pub facts: Vec<VerifiedProofFact>,
    pub receipts: Vec<IndexedCallEdgeReceipt>,
    pub gaps: Vec<FactBuildGap>,
    pub unavailable: Vec<UnavailableReason>,
}

/// Admit one persisted edge as direct-call evidence without rewriting either
/// endpoint. Exact-proof authorization is a separate receipt gate in runtime.
pub fn admit_raw_call_edge(
    edge: &Edge,
    expected_source: NodeId,
    expected_target: NodeId,
) -> RawCallEdgeAdmission {
    match diagnose_raw_call_edge(edge, expected_source, expected_target) {
        Ok(admitted) => RawCallEdgeAdmission::Admitted(admitted),
        Err(_) => RawCallEdgeAdmission::Rejected,
    }
}

pub fn diagnose_raw_call_edge(
    edge: &Edge,
    expected_source: NodeId,
    expected_target: NodeId,
) -> Result<AdmittedRawCallEdge, RawAdmissionFailure> {
    if edge.kind != EdgeKind::CALL {
        return Err(RawAdmissionFailure::WrongKind);
    }
    if edge.effective_source() != expected_source {
        return Err(RawAdmissionFailure::WrongEffectiveSource);
    }
    if edge.effective_target() != expected_target {
        return Err(RawAdmissionFailure::WrongEffectiveTarget);
    }
    if edge.resolved_target != Some(expected_target) {
        return Err(RawAdmissionFailure::MissingExactResolvedTarget);
    }
    if !edge.candidate_targets.is_empty() {
        return Err(RawAdmissionFailure::CandidateAlternativesRetained);
    }
    let file_node_id = edge
        .file_node_id
        .ok_or(RawAdmissionFailure::MissingFileNode)?;
    let line = edge
        .line
        .filter(|line| *line >= 1)
        .ok_or(RawAdmissionFailure::MissingLine)?;
    let callsite_identity = edge
        .callsite_identity
        .as_deref()
        .filter(|identity| !identity.is_empty())
        .ok_or(RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity)?;
    let pre_marker = callsite_identity
        .split_once('|')
        .map_or(callsite_identity, |(identity, _)| identity);
    let mut fields = pre_marker.split(':');
    let parsed = (
        fields.next().and_then(|value| value.parse::<i64>().ok()),
        fields.next().and_then(|value| value.parse::<u32>().ok()),
        fields.next().and_then(|value| value.parse::<u32>().ok()),
        fields.next().and_then(|value| value.parse::<i64>().ok()),
    );
    if fields.next().is_some() {
        return Err(RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity);
    }
    let (Some(parsed_file), Some(parsed_line), Some(column_or_ordinal), Some(parsed_target)) =
        parsed
    else {
        return Err(RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity);
    };
    if parsed_file != file_node_id.0 {
        return Err(RawAdmissionFailure::CallsiteFileMismatch);
    }
    if parsed_line != line {
        return Err(RawAdmissionFailure::CallsiteLineMismatch);
    }
    if parsed_target != edge.target.0 {
        return Err(RawAdmissionFailure::CallsiteRawTargetMismatch);
    }
    if format!("{parsed_file}:{parsed_line}:{column_or_ordinal}:{parsed_target}") != pre_marker {
        return Err(RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity);
    }
    Ok(AdmittedRawCallEdge {
        edge_id: edge.id,
        file_node_id,
        line,
        column_or_ordinal,
        raw_target: NodeId(parsed_target),
        callsite_identity: callsite_identity.to_owned(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnvalidatedCallPathContract {
    source_text: String,
    clauses: Vec<ClauseAnchor>,
    spec: UnvalidatedCallPathSpec,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClauseAnchor {
    pub clause_id: String,
    pub start: usize,
    pub end: usize,
    pub quote: String,
    pub classification: ClauseClassification,
}

// The dark contract's wire-facing variant names are intentionally stable.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClauseClassification {
    ResolvedMaterial { fields: Vec<ProofContractField> },
    UnresolvedMaterial { reason: UnresolvedMaterialReason },
    NonMaterial { kind: NonMaterialKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    fn canonical_name(self) -> &'static str {
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

    fn canonical_json(self) -> Value {
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
    fn canonical_name(&self) -> &'static str {
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
    fn canonical_name(&self) -> &'static str {
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
    target: ExactSymbolSelector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallPathSpec {
    start: ExactSymbolSelector,
    steps: Vec<DirectCallStep>,
    prohibit_traversal_through: Vec<ExactScopeSelector>,
    exclude_from_projection: Vec<ExactScopeSelector>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCallPathContract {
    spec: CallPathSpec,
    bound_hashes: ProofHashes,
}

impl ValidatedCallPathContract {
    pub fn spec(&self) -> &CallPathSpec {
        &self.spec
    }
}

impl CallPathSpec {
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

impl DirectCallStep {
    pub fn target(&self) -> &ExactSymbolSelector {
        &self.target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedContractRendering {
    normalized_clauses: Vec<NormalizedClause>,
}

impl ValidatedContractRendering {
    pub fn clause_count(&self) -> usize {
        self.normalized_clauses.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NormalizedClause {
    start: usize,
    end: usize,
    clause_id: String,
    classification: NormalizedClauseClassification,
    field: Option<ProofContractField>,
    quote: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NormalizedClauseClassification {
    Resolved,
    Unresolved(UnresolvedMaterialReason),
    Ignored(NonMaterialKind),
}

impl NormalizedClauseClassification {
    fn canonical_name(&self) -> &'static str {
        match self {
            Self::Resolved => "resolved_material",
            Self::Unresolved(_) => "unresolved_material",
            Self::Ignored(_) => "non_material",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationOutcome {
    Validated {
        contract: Box<ValidatedCallPathContract>,
        hashes: ProofHashes,
        rendering: ValidatedContractRendering,
    },
    Unknown {
        hashes: ProofHashes,
        gaps: Vec<TranslationGap>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum TranslationGap {
    UnclassifiedSourceText,
    UnresolvedMaterialClause {
        clause_id: String,
        reason: UnresolvedMaterialReason,
    },
    MaterialTokenMisclassified {
        clause_id: String,
        guard_families: Vec<ClauseGuardFamily>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    StepCountOutOfRange {
        actual: usize,
    },
    ScopeCountOutOfRange {
        field: ProofContractField,
        actual: usize,
    },
    InvalidSelector(SelectorValidationError),
    InvalidScope(SelectorValidationError),
    EmptyClauseId,
    EmptyClauseSpan {
        clause_id: String,
    },
    ClauseSpanOutOfBounds {
        clause_id: String,
    },
    ClauseSpanNotUtf8Boundary {
        clause_id: String,
    },
    ClauseQuoteMismatch {
        clause_id: String,
    },
    EmptyResolvedFieldSet {
        clause_id: String,
    },
    ClassificationConflict {
        clause_id: String,
    },
    MissingResolvedMaterialAnchor {
        field: ProofContractField,
        required: usize,
        found: usize,
    },
    OutOfRangeFieldReference {
        field: ProofContractField,
    },
    CanonicalJson(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorValidationError {
    EmptyIdentity,
    IdentityContainsNul,
    NonNormalizedQualifiedName,
    SignatureOrPatternSelector,
    RootPath,
    EmptyPathComponent,
    DotPathComponent,
    SeparatorInsidePathComponent,
    NulInsidePathComponent,
    PlatformEscape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofHashes {
    source_text_sha256: String,
    contract_digest: String,
}

impl ProofHashes {
    pub fn source_text_sha256(&self) -> &str {
        &self.source_text_sha256
    }

    pub fn contract_digest(&self) -> &str {
        &self.contract_digest
    }
}

#[derive(Debug, Clone, Copy)]
struct DigestDomain<'a> {
    schema_version: u32,
    proof_domain: &'a str,
    guard_version: &'a str,
}

impl Default for DigestDomain<'static> {
    fn default() -> Self {
        Self {
            schema_version: PROOF_CONTRACT_SCHEMA_VERSION,
            proof_domain: PROOF_DOMAIN,
            guard_version: CLAUSE_GUARD_VERSION,
        }
    }
}

pub fn validate_contract(
    input: UnvalidatedCallPathContract,
) -> Result<ValidationOutcome, ValidationError> {
    validate_contract_with_domain(input, DigestDomain::default())
}

fn validate_contract_with_domain(
    input: UnvalidatedCallPathContract,
    domain: DigestDomain<'_>,
) -> Result<ValidationOutcome, ValidationError> {
    let UnvalidatedCallPathContract {
        source_text,
        clauses,
        spec,
    } = input;
    let spec = validate_spec(spec)?;
    let normalized_clauses = validate_and_normalize_clauses(&source_text, clauses)?;
    validate_required_field_coverage(&normalized_clauses, &spec)?;
    let hashes = compute_hashes(&source_text, &normalized_clauses, &spec, domain)?;
    let gaps = classify_translation_gaps(&source_text, &normalized_clauses);
    if gaps.is_empty() {
        Ok(ValidationOutcome::Validated {
            contract: Box::new(ValidatedCallPathContract {
                spec,
                bound_hashes: hashes.clone(),
            }),
            hashes,
            rendering: ValidatedContractRendering { normalized_clauses },
        })
    } else {
        Ok(ValidationOutcome::Unknown { hashes, gaps })
    }
}

fn validate_spec(spec: UnvalidatedCallPathSpec) -> Result<CallPathSpec, ValidationError> {
    if !(MIN_STEPS..=MAX_STEPS).contains(&spec.steps.len()) {
        return Err(ValidationError::StepCountOutOfRange {
            actual: spec.steps.len(),
        });
    }
    if spec.prohibit_traversal_through.len() > MAX_INDEXED_SCOPES {
        return Err(ValidationError::ScopeCountOutOfRange {
            field: ProofContractField::TraversalProhibition { index: u8::MAX },
            actual: spec.prohibit_traversal_through.len(),
        });
    }
    if spec.exclude_from_projection.len() > MAX_INDEXED_SCOPES {
        return Err(ValidationError::ScopeCountOutOfRange {
            field: ProofContractField::ProjectionExclusion { index: u8::MAX },
            actual: spec.exclude_from_projection.len(),
        });
    }
    let start = validate_symbol_selector(spec.start).map_err(ValidationError::InvalidSelector)?;
    let steps = spec
        .steps
        .into_iter()
        .map(|step| {
            validate_symbol_selector(step.target)
                .map(|target| DirectCallStep { target })
                .map_err(ValidationError::InvalidSelector)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let prohibit_traversal_through = spec
        .prohibit_traversal_through
        .into_iter()
        .map(validate_scope_selector)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ValidationError::InvalidScope)?;
    let exclude_from_projection = spec
        .exclude_from_projection
        .into_iter()
        .map(validate_scope_selector)
        .collect::<Result<Vec<_>, _>>()
        .map_err(ValidationError::InvalidScope)?;
    Ok(CallPathSpec {
        start,
        steps,
        prohibit_traversal_through,
        exclude_from_projection,
    })
}

fn validate_symbol_selector(
    selector: UnvalidatedExactSymbolSelector,
) -> Result<ExactSymbolSelector, SelectorValidationError> {
    match selector {
        UnvalidatedExactSymbolSelector::PinnedNode(identity) => {
            validate_pinned_identity(&identity)?;
            Ok(ExactSymbolSelector::PinnedNode(identity))
        }
        UnvalidatedExactSymbolSelector::CanonicalId(canonical_id) => {
            validate_identity(&canonical_id)?;
            Ok(ExactSymbolSelector::CanonicalId(canonical_id))
        }
        UnvalidatedExactSymbolSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => {
            validate_qualified_name(&qualified_name)?;
            validate_optional_path(project_file_components.as_deref())?;
            Ok(ExactSymbolSelector::QualifiedName {
                qualified_name,
                project_file_components,
            })
        }
    }
}

fn validate_scope_selector(
    selector: UnvalidatedExactScopeSelector,
) -> Result<ExactScopeSelector, SelectorValidationError> {
    match selector {
        UnvalidatedExactScopeSelector::PinnedNode(identity) => {
            validate_pinned_identity(&identity)?;
            Ok(ExactScopeSelector::PinnedNode(identity))
        }
        UnvalidatedExactScopeSelector::CanonicalId(canonical_id) => {
            validate_identity(&canonical_id)?;
            Ok(ExactScopeSelector::CanonicalId(canonical_id))
        }
        UnvalidatedExactScopeSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => {
            validate_qualified_name(&qualified_name)?;
            validate_optional_path(project_file_components.as_deref())?;
            Ok(ExactScopeSelector::QualifiedName {
                qualified_name,
                project_file_components,
            })
        }
    }
}

fn validate_pinned_identity(identity: &PinnedNodeIdentity) -> Result<(), SelectorValidationError> {
    validate_identity(&identity.project_id)?;
    validate_identity(&identity.core_generation_id)?;
    validate_identity(&identity.core_run_id)?;
    validate_identity(&identity.node_id)
}

fn validate_identity(value: &str) -> Result<(), SelectorValidationError> {
    if value.is_empty() {
        return Err(SelectorValidationError::EmptyIdentity);
    }
    if value.contains('\0') {
        return Err(SelectorValidationError::IdentityContainsNul);
    }
    Ok(())
}

fn validate_qualified_name(value: &str) -> Result<(), SelectorValidationError> {
    validate_identity(value)?;
    if value.trim() != value {
        return Err(SelectorValidationError::NonNormalizedQualifiedName);
    }
    if value
        .chars()
        .any(|character| character.is_whitespace() || matches!(character, '(' | ')' | '*' | '?'))
    {
        return Err(SelectorValidationError::SignatureOrPatternSelector);
    }
    Ok(())
}

fn validate_optional_path(components: Option<&[String]>) -> Result<(), SelectorValidationError> {
    let Some(components) = components else {
        return Ok(());
    };
    if components.is_empty() {
        return Err(SelectorValidationError::RootPath);
    }
    for component in components {
        if component.is_empty() {
            return Err(SelectorValidationError::EmptyPathComponent);
        }
        if component == "." || component == ".." {
            return Err(SelectorValidationError::DotPathComponent);
        }
        if component.contains('/') || component.contains('\\') {
            return Err(SelectorValidationError::SeparatorInsidePathComponent);
        }
        if component.contains('\0') {
            return Err(SelectorValidationError::NulInsidePathComponent);
        }
        if component.starts_with('~') || component.contains(':') {
            return Err(SelectorValidationError::PlatformEscape);
        }
    }
    Ok(())
}

fn validate_and_normalize_clauses(
    source_text: &str,
    clauses: Vec<ClauseAnchor>,
) -> Result<Vec<NormalizedClause>, ValidationError> {
    let mut normalized = Vec::new();
    let mut classifications = BTreeMap::<(usize, usize, String), ClauseClassification>::new();
    for clause in clauses {
        if clause.clause_id.is_empty() {
            return Err(ValidationError::EmptyClauseId);
        }
        if clause.start == clause.end {
            return Err(ValidationError::EmptyClauseSpan {
                clause_id: clause.clause_id,
            });
        }
        if clause.start > clause.end || clause.end > source_text.len() {
            return Err(ValidationError::ClauseSpanOutOfBounds {
                clause_id: clause.clause_id,
            });
        }
        if !source_text.is_char_boundary(clause.start) || !source_text.is_char_boundary(clause.end)
        {
            return Err(ValidationError::ClauseSpanNotUtf8Boundary {
                clause_id: clause.clause_id,
            });
        }
        if source_text[clause.start..clause.end] != clause.quote {
            return Err(ValidationError::ClauseQuoteMismatch {
                clause_id: clause.clause_id,
            });
        }
        let conflict_key = (clause.start, clause.end, clause.clause_id.clone());
        if let Some(existing) = classifications.get(&conflict_key) {
            if classification_family(existing) != classification_family(&clause.classification)
                || classification_detail_conflicts(existing, &clause.classification)
            {
                return Err(ValidationError::ClassificationConflict {
                    clause_id: clause.clause_id,
                });
            }
        } else {
            classifications.insert(conflict_key, clause.classification.clone());
        }
        match clause.classification {
            ClauseClassification::ResolvedMaterial { mut fields } => {
                if fields.is_empty() {
                    return Err(ValidationError::EmptyResolvedFieldSet {
                        clause_id: clause.clause_id,
                    });
                }
                fields.sort();
                fields.dedup();
                for field in fields {
                    normalized.push(NormalizedClause {
                        start: clause.start,
                        end: clause.end,
                        clause_id: clause.clause_id.clone(),
                        quote: clause.quote.clone(),
                        classification: NormalizedClauseClassification::Resolved,
                        field: Some(field),
                    });
                }
            }
            ClauseClassification::UnresolvedMaterial { reason } => {
                normalized.push(NormalizedClause {
                    start: clause.start,
                    end: clause.end,
                    clause_id: clause.clause_id,
                    quote: clause.quote,
                    classification: NormalizedClauseClassification::Unresolved(reason),
                    field: None,
                });
            }
            ClauseClassification::NonMaterial { kind } => {
                normalized.push(NormalizedClause {
                    start: clause.start,
                    end: clause.end,
                    clause_id: clause.clause_id,
                    quote: clause.quote,
                    classification: NormalizedClauseClassification::Ignored(kind),
                    field: None,
                });
            }
        }
    }
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn classification_family(classification: &ClauseClassification) -> u8 {
    match classification {
        ClauseClassification::ResolvedMaterial { .. } => 0,
        ClauseClassification::UnresolvedMaterial { .. } => 1,
        ClauseClassification::NonMaterial { .. } => 2,
    }
}

fn classification_detail_conflicts(
    left: &ClauseClassification,
    right: &ClauseClassification,
) -> bool {
    match (left, right) {
        (
            ClauseClassification::UnresolvedMaterial { reason: left },
            ClauseClassification::UnresolvedMaterial { reason: right },
        ) => left != right,
        (
            ClauseClassification::NonMaterial { kind: left },
            ClauseClassification::NonMaterial { kind: right },
        ) => left != right,
        _ => false,
    }
}

fn validate_required_field_coverage(
    clauses: &[NormalizedClause],
    spec: &CallPathSpec,
) -> Result<(), ValidationError> {
    let fields = clauses
        .iter()
        .filter_map(|clause| clause.field)
        .collect::<Vec<_>>();
    for field in &fields {
        let in_range = match field {
            ProofContractField::Start => true,
            ProofContractField::StepTarget { step }
            | ProofContractField::Directness { step }
            | ProofContractField::Ordering { step }
            | ProofContractField::Relation { step } => usize::from(*step) < spec.steps.len(),
            ProofContractField::TraversalProhibition { index } => {
                usize::from(*index) < spec.prohibit_traversal_through.len()
            }
            ProofContractField::ProjectionExclusion { index } => {
                usize::from(*index) < spec.exclude_from_projection.len()
            }
        };
        if !in_range {
            return Err(ValidationError::OutOfRangeFieldReference { field: *field });
        }
    }

    let mut requirements = vec![ProofContractField::Start];
    for step in 0..spec.steps.len() {
        let step = u8::try_from(step).expect("step count is bounded below u8::MAX");
        requirements.extend([
            ProofContractField::StepTarget { step },
            ProofContractField::Directness { step },
            ProofContractField::Ordering { step },
            ProofContractField::Relation { step },
        ]);
    }
    requirements.extend((0..spec.prohibit_traversal_through.len()).map(|index| {
        ProofContractField::TraversalProhibition {
            index: u8::try_from(index).expect("scope count was validated"),
        }
    }));
    requirements.extend((0..spec.exclude_from_projection.len()).map(|index| {
        ProofContractField::ProjectionExclusion {
            index: u8::try_from(index).expect("scope count was validated"),
        }
    }));
    for field in requirements {
        let found = clauses
            .iter()
            .filter(|clause| clause.field == Some(field))
            .count();
        if found == 0 {
            return Err(ValidationError::MissingResolvedMaterialAnchor {
                field,
                required: 1,
                found,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Default)]
struct ByteCoverage {
    unresolved: bool,
    resolved: bool,
    non_material: bool,
}

fn classify_translation_gaps(
    source_text: &str,
    clauses: &[NormalizedClause],
) -> Vec<TranslationGap> {
    let mut coverage = vec![ByteCoverage::default(); source_text.len()];
    for clause in clauses {
        for byte in &mut coverage[clause.start..clause.end] {
            match clause.classification {
                NormalizedClauseClassification::Resolved => byte.resolved = true,
                NormalizedClauseClassification::Unresolved(_) => byte.unresolved = true,
                NormalizedClauseClassification::Ignored(_) => byte.non_material = true,
            }
        }
    }
    let mut gaps = BTreeSet::new();
    for clause in clauses {
        match &clause.classification {
            NormalizedClauseClassification::Unresolved(reason) => {
                gaps.insert(TranslationGap::UnresolvedMaterialClause {
                    clause_id: clause.clause_id.clone(),
                    reason: reason.clone(),
                });
            }
            NormalizedClauseClassification::Ignored(_) => {
                let guard_families = clause_guard_spans(&clause.quote)
                    .into_iter()
                    .filter(|span| {
                        (clause.start + span.start..clause.start + span.end).any(|offset| {
                            coverage[offset].non_material
                                && !coverage[offset].resolved
                                && !coverage[offset].unresolved
                                && !source_text.as_bytes()[offset].is_ascii_whitespace()
                        })
                    })
                    .map(|span| span.family)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>();
                if !guard_families.is_empty() {
                    gaps.insert(TranslationGap::MaterialTokenMisclassified {
                        clause_id: clause.clause_id.clone(),
                        guard_families,
                    });
                }
            }
            NormalizedClauseClassification::Resolved => {}
        }
    }
    let has_unclassified_material = source_text.char_indices().any(|(offset, character)| {
        let end = offset + character.len_utf8();
        !character.is_whitespace()
            && coverage[offset..end]
                .iter()
                .any(|byte| !byte.unresolved && !byte.resolved && !byte.non_material)
    });
    if has_unclassified_material {
        gaps.insert(TranslationGap::UnclassifiedSourceText);
    }
    gaps.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClauseGuardFamily {
    QuotedOrBacktickedIdentifier,
    ArrowOrRelationNotation,
    Directness,
    OrderingOrOrdinal,
    Only,
    NegationOrExclusion,
    PathLikeString,
    QualifiedSymbolNotation,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ClauseGuardSpan {
    start: usize,
    end: usize,
    family: ClauseGuardFamily,
}

pub fn clause_guard_families(text: &str) -> Vec<ClauseGuardFamily> {
    clause_guard_spans(text)
        .into_iter()
        .map(|span| span.family)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn clause_guard_spans(text: &str) -> Vec<ClauseGuardSpan> {
    let mut spans = BTreeSet::new();
    for delimiter in ['`', '"', '\''] {
        for (start, end) in nonempty_quoted_spans(text, delimiter) {
            spans.insert(ClauseGuardSpan {
                start,
                end,
                family: ClauseGuardFamily::QuotedOrBacktickedIdentifier,
            });
        }
    }
    for notation in ["->", "=>", "→"] {
        for (start, _) in text.match_indices(notation) {
            spans.insert(ClauseGuardSpan {
                start,
                end: start + notation.len(),
                family: ClauseGuardFamily::ArrowOrRelationNotation,
            });
        }
    }
    for (start, end) in word_spans(text) {
        let word = text[start..end].to_ascii_lowercase();
        let family = if matches!(
            word.as_str(),
            "call" | "calls" | "called" | "invoke" | "invokes" | "invoked"
        ) {
            Some(ClauseGuardFamily::ArrowOrRelationNotation)
        } else if matches!(
            word.as_str(),
            "direct" | "directly" | "immediate" | "immediately"
        ) {
            Some(ClauseGuardFamily::Directness)
        } else if matches!(
            word.as_str(),
            "first"
                | "second"
                | "third"
                | "fourth"
                | "fifth"
                | "sixth"
                | "then"
                | "before"
                | "after"
                | "ordered"
                | "order"
        ) || has_ordinal_suffix(&word)
        {
            Some(ClauseGuardFamily::OrderingOrOrdinal)
        } else if word == "only" {
            Some(ClauseGuardFamily::Only)
        } else if matches!(
            word.as_str(),
            "no" | "not"
                | "never"
                | "without"
                | "exclude"
                | "excludes"
                | "excluding"
                | "excluded"
                | "except"
                | "prohibit"
                | "prohibits"
                | "avoid"
        ) {
            Some(ClauseGuardFamily::NegationOrExclusion)
        } else {
            None
        };
        if let Some(family) = family {
            spans.insert(ClauseGuardSpan { start, end, family });
        }
    }
    for (start, end) in token_spans(text) {
        let token = &text[start..end];
        let lower = token.to_ascii_lowercase();
        if token.contains('/')
            || token.contains('\\')
            || [".rs", ".ts", ".tsx", ".js", ".py", ".go", ".java"]
                .iter()
                .any(|extension| lower.contains(extension))
        {
            spans.insert(ClauseGuardSpan {
                start,
                end,
                family: ClauseGuardFamily::PathLikeString,
            });
        }
        if token.contains("::") || contains_dotted_qualified_name(token) {
            spans.insert(ClauseGuardSpan {
                start,
                end,
                family: ClauseGuardFamily::QualifiedSymbolNotation,
            });
        }
    }
    spans.into_iter().collect()
}

fn nonempty_quoted_spans(text: &str, delimiter: char) -> Vec<(usize, usize)> {
    let positions = text
        .match_indices(delimiter)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    positions
        .chunks_exact(2)
        .filter(|pair| pair[0] + delimiter.len_utf8() < pair[1])
        .map(|pair| (pair[0], pair[1] + delimiter.len_utf8()))
        .collect()
}

fn word_spans(text: &str) -> Vec<(usize, usize)> {
    delimited_spans(text, |character| {
        character.is_alphanumeric() || character == '_'
    })
}

fn token_spans(text: &str) -> Vec<(usize, usize)> {
    delimited_spans(text, |character| {
        !character.is_whitespace() && !matches!(character, ',' | ';' | '(' | ')' | '[' | ']')
    })
}

fn delimited_spans(text: &str, keep: impl Fn(char) -> bool) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = None;
    for (offset, character) in text.char_indices() {
        if keep(character) {
            start.get_or_insert(offset);
        } else if let Some(start) = start.take() {
            spans.push((start, offset));
        }
    }
    if let Some(start) = start {
        spans.push((start, text.len()));
    }
    spans
}

fn has_ordinal_suffix(word: &str) -> bool {
    ["st", "nd", "rd", "th"].iter().any(|suffix| {
        word.strip_suffix(suffix).is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
    })
}

fn contains_dotted_qualified_name(text: &str) -> bool {
    let characters = text.chars().collect::<Vec<_>>();
    characters.windows(3).any(|window| {
        (window[0].is_alphanumeric() || window[0] == '_')
            && window[1] == '.'
            && (window[2].is_alphanumeric() || window[2] == '_')
    })
}

fn compute_hashes(
    source_text: &str,
    clauses: &[NormalizedClause],
    spec: &CallPathSpec,
    domain: DigestDomain<'_>,
) -> Result<ProofHashes, ValidationError> {
    let source_text_sha256 = sha256_hex(source_text.as_bytes());
    let contract_digest = compute_contract_digest(&source_text_sha256, clauses, spec, domain)?;
    Ok(ProofHashes {
        source_text_sha256,
        contract_digest,
    })
}

fn compute_contract_digest(
    source_text_sha256: &str,
    clauses: &[NormalizedClause],
    spec: &CallPathSpec,
    domain: DigestDomain<'_>,
) -> Result<String, ValidationError> {
    let digest_document = json!({
        "schema_version": domain.schema_version,
        "proof_domain": domain.proof_domain,
        "guard_version": domain.guard_version,
        "source_text_sha256": source_text_sha256,
        "clauses": clauses.iter().map(normalized_clause_json).collect::<Vec<_>>(),
        "spec": spec_json(spec),
    });
    let canonical = serde_json_canonicalizer::to_vec(&digest_document)
        .map_err(|error| ValidationError::CanonicalJson(error.to_string()))?;
    let mut digest_bytes = Vec::with_capacity(DIGEST_DOMAIN_SEPARATOR.len() + canonical.len());
    digest_bytes.extend_from_slice(DIGEST_DOMAIN_SEPARATOR);
    digest_bytes.extend_from_slice(&canonical);
    Ok(sha256_hex(&digest_bytes))
}

fn normalized_clause_json(clause: &NormalizedClause) -> Value {
    let (reason, non_material_kind) = match &clause.classification {
        NormalizedClauseClassification::Resolved => (None, None),
        NormalizedClauseClassification::Unresolved(reason) => (Some(reason.canonical_name()), None),
        NormalizedClauseClassification::Ignored(kind) => (None, Some(kind.canonical_name())),
    };
    json!({
        "start": clause.start,
        "end": clause.end,
        "clause_id": clause.clause_id,
        "quote": clause.quote,
        "classification": clause.classification.canonical_name(),
        "field": clause.field.map(ProofContractField::canonical_json),
        "reason": reason,
        "non_material_kind": non_material_kind,
    })
}

fn spec_json(spec: &CallPathSpec) -> Value {
    json!({
        "start": symbol_selector_json(&spec.start),
        "steps": spec.steps.iter().map(|step| json!({
            "relation": "direct_outgoing_call",
            "target": symbol_selector_json(&step.target),
        })).collect::<Vec<_>>(),
        "prohibit_traversal_through": spec.prohibit_traversal_through
            .iter().map(scope_selector_json).collect::<Vec<_>>(),
        "exclude_from_projection": spec.exclude_from_projection
            .iter().map(scope_selector_json).collect::<Vec<_>>(),
    })
}

fn symbol_selector_json(selector: &ExactSymbolSelector) -> Value {
    match selector {
        ExactSymbolSelector::PinnedNode(identity) => pinned_identity_json(identity),
        ExactSymbolSelector::CanonicalId(canonical_id) => json!({
            "kind": "canonical_id", "canonical_id": canonical_id,
        }),
        ExactSymbolSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => json!({
            "kind": "qualified_name",
            "qualified_name": qualified_name,
            "project_file_components": project_file_components,
        }),
    }
}

fn scope_selector_json(selector: &ExactScopeSelector) -> Value {
    match selector {
        ExactScopeSelector::PinnedNode(identity) => pinned_identity_json(identity),
        ExactScopeSelector::CanonicalId(canonical_id) => json!({
            "kind": "canonical_id", "canonical_id": canonical_id,
        }),
        ExactScopeSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => json!({
            "kind": "qualified_name",
            "qualified_name": qualified_name,
            "project_file_components": project_file_components,
        }),
    }
}

fn pinned_identity_json(identity: &PinnedNodeIdentity) -> Value {
    json!({
        "kind": "pinned_node",
        "project_id": identity.project_id,
        "core_generation_id": identity.core_generation_id,
        "core_run_id": identity.core_run_id,
        "node_id": identity.node_id,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ResolvedNodeIdentity {
    pub pinned: PinnedNodeIdentity,
    pub canonical_id: String,
    pub qualified_name: String,
    /// The file node pinned by the same core publication as this callable.
    pub file_node_id: NodeId,
    pub project_file_components: Vec<String>,
}

impl ResolvedNodeIdentity {
    pub fn new(
        pinned: PinnedNodeIdentity,
        canonical_id: impl Into<String>,
        qualified_name: impl Into<String>,
        file_node_id: NodeId,
        project_file_components: Vec<String>,
    ) -> Result<Self, SelectorValidationError> {
        validate_pinned_identity(&pinned)?;
        let canonical_id = canonical_id.into();
        let qualified_name = qualified_name.into();
        validate_identity(&canonical_id)?;
        validate_qualified_name(&qualified_name)?;
        validate_optional_path(Some(&project_file_components))?;
        Ok(Self {
            pinned,
            canonical_id,
            qualified_name,
            file_node_id,
            project_file_components,
        })
    }
}

fn symbol_selector_matches(selector: &ExactSymbolSelector, node: &ResolvedNodeIdentity) -> bool {
    match selector {
        ExactSymbolSelector::PinnedNode(identity) => identity == &node.pinned,
        ExactSymbolSelector::CanonicalId(canonical_id) => canonical_id == &node.canonical_id,
        ExactSymbolSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => {
            qualified_name == &node.qualified_name
                && project_file_components
                    .as_ref()
                    .is_none_or(|path| path == &node.project_file_components)
        }
    }
}

fn scope_selector_matches(selector: &ExactScopeSelector, node: &ResolvedNodeIdentity) -> bool {
    match selector {
        ExactScopeSelector::PinnedNode(identity) => identity == &node.pinned,
        ExactScopeSelector::CanonicalId(canonical_id) => canonical_id == &node.canonical_id,
        ExactScopeSelector::QualifiedName {
            qualified_name,
            project_file_components,
        } => {
            qualified_name == &node.qualified_name
                && project_file_components
                    .as_ref()
                    .is_none_or(|scope_path| node.project_file_components.starts_with(scope_path))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ReceiptRef {
    pub receipt_id: String,
    pub edge_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedDirectCallFact {
    pub receipt: ReceiptRef,
    pub source: ResolvedNodeIdentity,
    pub target: ResolvedNodeIdentity,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertifiedAbsenceFact {
    pub source: ResolvedNodeIdentity,
    pub expected_target: ExactSymbolSelector,
    pub extractor_capability_receipt_id: String,
    pub untruncated_enumeration_receipt_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableProofFact {
    pub reason: UnavailableReason,
}

// The dark domain fact layout is intentionally stable across qualification.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedProofFact {
    DirectCall(VerifiedDirectCallFact),
    #[cfg(any(test, feature = "test-support"))]
    CertifiedAbsence(CertifiedAbsenceFact),
    Unavailable(UnavailableProofFact),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofDisposition {
    ContractProven {
        contract_digest: String,
        receipts: Vec<ReceiptRef>,
    },
    ContractRefuted {
        contract_digest: String,
        refutation: Refutation,
    },
    Unknown {
        contract_digest: String,
        gaps: Vec<ProofGap>,
        connected_receipts: Vec<ReceiptRef>,
    },
    Unavailable {
        contract_digest: String,
        reasons: Vec<UnavailableReason>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refutation {
    ProhibitedScopeTraversal {
        step_index: usize,
        prohibition_index: usize,
        connected_receipts: Vec<ReceiptRef>,
    },
    #[cfg(any(test, feature = "test-support"))]
    CertifiedAbsence {
        step_index: usize,
        extractor_capability_receipt_id: String,
        untruncated_enumeration_receipt_id: String,
        connected_receipts: Vec<ReceiptRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofGap {
    FactBuild(FactBuildGap),
    MissingDirectCallReceipt { step_index: usize },
    ReceiptOrEdgeAlreadyUsed { step_index: usize },
    ProjectionExclusionConflictsWithRequiredReceipt { step_index: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnavailableReason {
    ValidatedContractHashMismatch,
    PublicationPinMismatch,
    SourceNotBoundToPublication,
    ProofFactsUnavailable,
    ProofSemanticProjectionUnavailable,
}

pub fn check_call_path(
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    facts: &[VerifiedProofFact],
) -> ProofDisposition {
    check_call_path_with_receipt_order(contract, hashes, facts, None)
}

fn check_call_path_with_receipt_order(
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    facts: &[VerifiedProofFact],
    receipt_order: Option<&BTreeMap<ReceiptRef, usize>>,
) -> ProofDisposition {
    if hashes != &contract.bound_hashes {
        return ProofDisposition::Unavailable {
            contract_digest: hashes.contract_digest.clone(),
            reasons: vec![UnavailableReason::ValidatedContractHashMismatch],
        };
    }
    let unavailable_reasons = facts
        .iter()
        .filter_map(|fact| match fact {
            VerifiedProofFact::Unavailable(fact) => Some(fact.reason.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !unavailable_reasons.is_empty() {
        return ProofDisposition::Unavailable {
            contract_digest: hashes.contract_digest.clone(),
            reasons: unavailable_reasons,
        };
    }
    let direct_facts = facts
        .iter()
        .filter_map(|fact| match fact {
            VerifiedProofFact::DirectCall(fact)
                if !fact.receipt.receipt_id.is_empty() && !fact.receipt.edge_id.is_empty() =>
            {
                Some(fact)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if let Some(path) = find_path(contract, &direct_facts, PathPolicy::Strict, receipt_order) {
        return ProofDisposition::ContractProven {
            contract_digest: hashes.contract_digest.clone(),
            receipts: path.into_iter().map(|fact| fact.receipt.clone()).collect(),
        };
    }
    if let Some(path) = find_path(
        contract,
        &direct_facts,
        PathPolicy::AllowProjectionExclusions,
        receipt_order,
    ) {
        let step_index = first_projection_conflict(contract, &path).unwrap_or(0);
        return ProofDisposition::Unknown {
            contract_digest: hashes.contract_digest.clone(),
            gaps: vec![ProofGap::ProjectionExclusionConflictsWithRequiredReceipt { step_index }],
            connected_receipts: path[..step_index]
                .iter()
                .map(|fact| fact.receipt.clone())
                .collect(),
        };
    }
    let mut reachable = reachable_prefixes(contract, &direct_facts, receipt_order);
    for source in facts.iter().filter_map(|fact| match fact {
        #[cfg(any(test, feature = "test-support"))]
        VerifiedProofFact::CertifiedAbsence(fact) => Some(&fact.source),
        _ => None,
    }) {
        if symbol_selector_matches(&contract.spec.start, source)
            && !reachable
                .iter()
                .any(|state| state.step_index == 0 && &state.current == source)
        {
            reachable.push(PrefixState {
                step_index: 0,
                current: source.clone(),
                used_receipts: BTreeSet::new(),
                used_edges: BTreeSet::new(),
                connected_receipts: Vec::new(),
                projection_conflict_step: None,
            });
        }
    }
    if let Some((state, prohibition_index)) = reachable.iter().find_map(|state| {
        (state.step_index > 0
            && state.step_index < contract.spec.steps.len()
            && state.projection_conflict_step.is_none())
        .then(|| {
            contract
                .spec
                .prohibit_traversal_through
                .iter()
                .position(|scope| scope_selector_matches(scope, &state.current))
                .map(|index| (state, index))
        })
        .flatten()
    }) {
        return ProofDisposition::ContractRefuted {
            contract_digest: hashes.contract_digest.clone(),
            refutation: Refutation::ProhibitedScopeTraversal {
                step_index: state.step_index - 1,
                prohibition_index,
                connected_receipts: state.connected_receipts.clone(),
            },
        };
    }
    if let Some(step_index) = reachable.iter().find_map(|state| {
        (state.step_index > 0
            && state.step_index < contract.spec.steps.len()
            && contract
                .spec
                .prohibit_traversal_through
                .iter()
                .any(|scope| scope_selector_matches(scope, &state.current)))
        .then_some(state.projection_conflict_step)
        .flatten()
    }) {
        return ProofDisposition::Unknown {
            contract_digest: hashes.contract_digest.clone(),
            gaps: vec![ProofGap::ProjectionExclusionConflictsWithRequiredReceipt { step_index }],
            connected_receipts: longest_clean_prefix(&reachable, receipt_order),
        };
    }
    let mut clean_states = reachable
        .iter()
        .filter(|state| state.projection_conflict_step.is_none())
        .collect::<Vec<_>>();
    clean_states.sort_by(|left, right| {
        right.step_index.cmp(&left.step_index).then_with(|| {
            compare_receipt_sequences(
                &left.connected_receipts,
                &right.connected_receipts,
                receipt_order,
            )
        })
    });
    for state in clean_states {
        if state.step_index >= contract.spec.steps.len() || state.projection_conflict_step.is_some()
        {
            continue;
        }
        #[cfg(any(test, feature = "test-support"))]
        let target = &contract.spec.steps[state.step_index].target;
        #[cfg(any(test, feature = "test-support"))]
        if let Some(absence) = facts
            .iter()
            .filter_map(|fact| match fact {
                VerifiedProofFact::CertifiedAbsence(fact) => Some(fact),
                _ => None,
            })
            .find(|fact| {
                fact.source == state.current
                    && &fact.expected_target == target
                    && !fact.extractor_capability_receipt_id.is_empty()
                    && !fact.untruncated_enumeration_receipt_id.is_empty()
            })
        {
            return ProofDisposition::ContractRefuted {
                contract_digest: hashes.contract_digest.clone(),
                refutation: Refutation::CertifiedAbsence {
                    step_index: state.step_index,
                    extractor_capability_receipt_id: absence
                        .extractor_capability_receipt_id
                        .clone(),
                    untruncated_enumeration_receipt_id: absence
                        .untruncated_enumeration_receipt_id
                        .clone(),
                    connected_receipts: state.connected_receipts.clone(),
                },
            };
        }
    }
    let furthest_clean_step = reachable
        .iter()
        .filter(|state| state.projection_conflict_step.is_none())
        .map(|state| state.step_index)
        .max()
        .unwrap_or(0);
    if let Some(step_index) = reachable
        .iter()
        .filter(|state| state.step_index > furthest_clean_step)
        .filter_map(|state| state.projection_conflict_step)
        .min()
    {
        return ProofDisposition::Unknown {
            contract_digest: hashes.contract_digest.clone(),
            gaps: vec![ProofGap::ProjectionExclusionConflictsWithRequiredReceipt { step_index }],
            connected_receipts: longest_clean_prefix(&reachable, receipt_order),
        };
    }
    let step_index = reachable
        .iter()
        .filter(|state| state.projection_conflict_step.is_none())
        .map(|state| state.step_index)
        .max()
        .unwrap_or(0)
        .min(contract.spec.steps.len() - 1);
    let reuse_blocked = reachable.iter().any(|state| {
        state.step_index == step_index
            && state.projection_conflict_step.is_none()
            && direct_facts.iter().any(|fact| {
                fact.source == state.current
                    && symbol_selector_matches(
                        &contract.spec.steps[step_index].target,
                        &fact.target,
                    )
                    && (state.used_receipts.contains(&fact.receipt.receipt_id)
                        || state.used_edges.contains(&fact.receipt.edge_id))
            })
    });
    ProofDisposition::Unknown {
        contract_digest: hashes.contract_digest.clone(),
        gaps: vec![if reuse_blocked {
            ProofGap::ReceiptOrEdgeAlreadyUsed { step_index }
        } else {
            ProofGap::MissingDirectCallReceipt { step_index }
        }],
        connected_receipts: longest_clean_prefix(&reachable, receipt_order),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathPolicy {
    Strict,
    AllowProjectionExclusions,
}

fn find_path<'a>(
    contract: &ValidatedCallPathContract,
    facts: &[&'a VerifiedDirectCallFact],
    policy: PathPolicy,
    receipt_order: Option<&BTreeMap<ReceiptRef, usize>>,
) -> Option<Vec<&'a VerifiedDirectCallFact>> {
    let mut ordered = facts.to_vec();
    ordered
        .sort_by(|left, right| compare_receipt_refs(&left.receipt, &right.receipt, receipt_order));
    search_path(
        contract,
        &ordered,
        0,
        None,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
        &mut Vec::new(),
        policy,
    )
}

#[allow(clippy::too_many_arguments)]
fn search_path<'a>(
    contract: &ValidatedCallPathContract,
    facts: &[&'a VerifiedDirectCallFact],
    step_index: usize,
    current: Option<&ResolvedNodeIdentity>,
    used_receipts: &mut BTreeSet<String>,
    used_edges: &mut BTreeSet<String>,
    path: &mut Vec<&'a VerifiedDirectCallFact>,
    policy: PathPolicy,
) -> Option<Vec<&'a VerifiedDirectCallFact>> {
    if step_index == contract.spec.steps.len() {
        return Some(path.clone());
    }
    let step = &contract.spec.steps[step_index];
    for fact in facts {
        let source_matches = current.map_or_else(
            || symbol_selector_matches(&contract.spec.start, &fact.source),
            |current| current == &fact.source,
        );
        if !source_matches || !symbol_selector_matches(&step.target, &fact.target) {
            continue;
        }
        if used_receipts.contains(&fact.receipt.receipt_id)
            || used_edges.contains(&fact.receipt.edge_id)
        {
            continue;
        }
        if policy != PathPolicy::AllowProjectionExclusions
            && receipt_hits_projection_exclusion(contract, fact)
        {
            continue;
        }
        if step_index + 1 < contract.spec.steps.len()
            && contract
                .spec
                .prohibit_traversal_through
                .iter()
                .any(|scope| scope_selector_matches(scope, &fact.target))
        {
            continue;
        }
        used_receipts.insert(fact.receipt.receipt_id.clone());
        used_edges.insert(fact.receipt.edge_id.clone());
        path.push(fact);
        if let Some(found) = search_path(
            contract,
            facts,
            step_index + 1,
            Some(&fact.target),
            used_receipts,
            used_edges,
            path,
            policy,
        ) {
            return Some(found);
        }
        path.pop();
        used_edges.remove(&fact.receipt.edge_id);
        used_receipts.remove(&fact.receipt.receipt_id);
    }
    None
}

fn receipt_hits_projection_exclusion(
    contract: &ValidatedCallPathContract,
    fact: &VerifiedDirectCallFact,
) -> bool {
    contract.spec.exclude_from_projection.iter().any(|scope| {
        scope_selector_matches(scope, &fact.source) || scope_selector_matches(scope, &fact.target)
    })
}

fn first_projection_conflict(
    contract: &ValidatedCallPathContract,
    path: &[&VerifiedDirectCallFact],
) -> Option<usize> {
    path.iter()
        .position(|fact| receipt_hits_projection_exclusion(contract, fact))
}

#[derive(Debug, Clone)]
struct PrefixState {
    step_index: usize,
    current: ResolvedNodeIdentity,
    used_receipts: BTreeSet<String>,
    used_edges: BTreeSet<String>,
    connected_receipts: Vec<ReceiptRef>,
    projection_conflict_step: Option<usize>,
}

fn reachable_prefixes(
    contract: &ValidatedCallPathContract,
    facts: &[&VerifiedDirectCallFact],
    receipt_order: Option<&BTreeMap<ReceiptRef, usize>>,
) -> Vec<PrefixState> {
    let mut facts = facts.to_vec();
    facts.sort_by(|left, right| compare_receipt_refs(&left.receipt, &right.receipt, receipt_order));
    let initial_nodes = facts
        .iter()
        .filter(|fact| symbol_selector_matches(&contract.spec.start, &fact.source))
        .map(|fact| fact.source.clone())
        .collect::<BTreeSet<_>>();
    let mut states = initial_nodes
        .into_iter()
        .map(|current| PrefixState {
            step_index: 0,
            current,
            used_receipts: BTreeSet::new(),
            used_edges: BTreeSet::new(),
            connected_receipts: Vec::new(),
            projection_conflict_step: None,
        })
        .collect::<Vec<_>>();
    let mut all = states.clone();
    for step_index in 0..contract.spec.steps.len() {
        let mut next = Vec::new();
        for state in states {
            for fact in &facts {
                if fact.source != state.current
                    || !symbol_selector_matches(
                        &contract.spec.steps[step_index].target,
                        &fact.target,
                    )
                    || state.used_receipts.contains(&fact.receipt.receipt_id)
                    || state.used_edges.contains(&fact.receipt.edge_id)
                {
                    continue;
                }
                let mut used_receipts = state.used_receipts.clone();
                let mut used_edges = state.used_edges.clone();
                let mut connected_receipts = state.connected_receipts.clone();
                used_receipts.insert(fact.receipt.receipt_id.clone());
                used_edges.insert(fact.receipt.edge_id.clone());
                connected_receipts.push(fact.receipt.clone());
                next.push(PrefixState {
                    step_index: step_index + 1,
                    current: fact.target.clone(),
                    used_receipts,
                    used_edges,
                    connected_receipts,
                    projection_conflict_step: state.projection_conflict_step.or_else(|| {
                        receipt_hits_projection_exclusion(contract, fact).then_some(step_index)
                    }),
                });
            }
        }
        if next.is_empty() {
            break;
        }
        all.extend(next.clone());
        states = next;
    }
    all
}

fn longest_clean_prefix(
    states: &[PrefixState],
    receipt_order: Option<&BTreeMap<ReceiptRef, usize>>,
) -> Vec<ReceiptRef> {
    states
        .iter()
        .filter(|state| state.projection_conflict_step.is_none())
        .map(|state| state.connected_receipts.clone())
        .min_by(|left, right| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| compare_receipt_sequences(left, right, receipt_order))
        })
        .unwrap_or_default()
}

fn compare_receipt_refs(
    left: &ReceiptRef,
    right: &ReceiptRef,
    receipt_order: Option<&BTreeMap<ReceiptRef, usize>>,
) -> Ordering {
    receipt_order
        .and_then(|order| order.get(left).zip(order.get(right)))
        .map_or_else(|| left.cmp(right), |(left, right)| left.cmp(right))
}

fn compare_receipt_sequences(
    left: &[ReceiptRef],
    right: &[ReceiptRef],
    receipt_order: Option<&BTreeMap<ReceiptRef, usize>>,
) -> Ordering {
    left.iter()
        .zip(right)
        .map(|(left, right)| compare_receipt_refs(left, right, receipt_order))
        .find(|ordering| *ordering != Ordering::Equal)
        .unwrap_or_else(|| left.len().cmp(&right.len()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedBuiltCallPathIntegration {
    contract: ValidatedCallPathContract,
    hashes: ProofHashes,
    rendering: ValidatedContractRendering,
    built: BuiltCallPathFacts,
    disposition: ProofDisposition,
    authoritative_receipts: Vec<IndexedCallEdgeReceipt>,
}

impl CheckedBuiltCallPathIntegration {
    pub fn built_facts(&self) -> &BuiltCallPathFacts {
        &self.built
    }

    pub fn disposition(&self) -> &ProofDisposition {
        &self.disposition
    }

    pub fn authoritative_receipts(&self) -> &[IndexedCallEdgeReceipt] {
        &self.authoritative_receipts
    }

    pub fn publication(&self) -> &InternalCorePublicationIdentity {
        &self.built.publication
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedIntegrationError {
    ValidatedContractHashMismatch,
    ContractDigestMismatch,
    CanonicalJson(String),
    PublicationBindingMismatch,
    FactReceiptMismatch,
    DuplicateFactReceipt,
    DuplicateAuthoritativeReceipt,
}

pub fn check_built_call_path_integration(
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    rendering: &ValidatedContractRendering,
    built: BuiltCallPathFacts,
) -> Result<CheckedBuiltCallPathIntegration, CheckedIntegrationError> {
    if hashes != &contract.bound_hashes {
        return Err(CheckedIntegrationError::ValidatedContractHashMismatch);
    }
    let recomputed_digest = compute_contract_digest(
        &hashes.source_text_sha256,
        &rendering.normalized_clauses,
        &contract.spec,
        DigestDomain::default(),
    )
    .map_err(|error| CheckedIntegrationError::CanonicalJson(format!("{error:?}")))?;
    if recomputed_digest != hashes.contract_digest {
        return Err(CheckedIntegrationError::ContractDigestMismatch);
    }
    validate_built_publication_and_receipts(&built)?;

    let disposition = integrate_built_disposition(contract, hashes, &built);
    let authoritative_refs = authoritative_receipt_refs(&disposition);
    let mut seen = BTreeSet::new();
    let mut authoritative_receipts = Vec::with_capacity(authoritative_refs.len());
    for receipt_ref in authoritative_refs {
        if !seen.insert(receipt_ref.clone()) {
            return Err(CheckedIntegrationError::DuplicateAuthoritativeReceipt);
        }
        let mut matches = built
            .receipts
            .iter()
            .filter(|receipt| receipt.receipt == *receipt_ref);
        let Some(receipt) = matches.next() else {
            return Err(CheckedIntegrationError::FactReceiptMismatch);
        };
        if matches.next().is_some() {
            return Err(CheckedIntegrationError::DuplicateFactReceipt);
        }
        authoritative_receipts.push(receipt.clone());
    }

    Ok(CheckedBuiltCallPathIntegration {
        contract: contract.clone(),
        hashes: hashes.clone(),
        rendering: rendering.clone(),
        built,
        disposition,
        authoritative_receipts,
    })
}

fn validate_built_publication_and_receipts(
    built: &BuiltCallPathFacts,
) -> Result<(), CheckedIntegrationError> {
    let publication = &built.publication;
    if publication.project_id.is_empty()
        || publication.generation_id.is_empty()
        || publication.run_id.is_empty()
    {
        return Err(CheckedIntegrationError::PublicationBindingMismatch);
    }
    let direct_facts = built
        .facts
        .iter()
        .filter_map(|fact| match fact {
            VerifiedProofFact::DirectCall(fact) => Some(fact),
            _ => None,
        })
        .collect::<Vec<_>>();
    for fact in &built.facts {
        let nodes = match fact {
            VerifiedProofFact::DirectCall(fact) => vec![&fact.source, &fact.target],
            #[cfg(any(test, feature = "test-support"))]
            VerifiedProofFact::CertifiedAbsence(fact) => vec![&fact.source],
            VerifiedProofFact::Unavailable(_) => Vec::new(),
        };
        if nodes
            .iter()
            .any(|node| !node_matches_publication(node, publication))
        {
            return Err(CheckedIntegrationError::PublicationBindingMismatch);
        }
    }
    for receipt in &built.receipts {
        if receipt.resolution_fact_id.len() != 64
            || receipt.resolution_evidence_sha256.len() != 64
            || receipt.resolution_evidence_chain.is_empty()
            || !valid_resolution_provenance(&receipt.resolution_provenance)
            || receipt.resolution_evidence_sha256 != receipt.resolution_provenance.evidence_sha256
            || !node_matches_publication(&receipt.source, publication)
            || !node_matches_publication(&receipt.target, publication)
        {
            return Err(CheckedIntegrationError::PublicationBindingMismatch);
        }
    }
    for fact in &direct_facts {
        let count = built
            .receipts
            .iter()
            .filter(|receipt| indexed_receipt_matches_fact(receipt, fact))
            .count();
        match count {
            1 => {}
            0 => return Err(CheckedIntegrationError::FactReceiptMismatch),
            _ => return Err(CheckedIntegrationError::DuplicateFactReceipt),
        }
    }
    for receipt in &built.receipts {
        let count = direct_facts
            .iter()
            .filter(|fact| indexed_receipt_matches_fact(receipt, fact))
            .count();
        match count {
            1 => {}
            0 => return Err(CheckedIntegrationError::FactReceiptMismatch),
            _ => return Err(CheckedIntegrationError::DuplicateFactReceipt),
        }
    }
    Ok(())
}

fn node_matches_publication(
    node: &ResolvedNodeIdentity,
    publication: &InternalCorePublicationIdentity,
) -> bool {
    node.pinned.project_id == publication.project_id
        && node.pinned.core_generation_id == publication.generation_id
        && node.pinned.core_run_id == publication.run_id
}

fn indexed_receipt_matches_fact(
    receipt: &IndexedCallEdgeReceipt,
    fact: &VerifiedDirectCallFact,
) -> bool {
    receipt.receipt == fact.receipt
        && receipt.source == fact.source
        && receipt.target == fact.target
}

fn indexed_receipt_source_order(
    left: &IndexedCallEdgeReceipt,
    right: &IndexedCallEdgeReceipt,
) -> Ordering {
    left.line_window
        .project_file_components
        .cmp(&right.line_window.project_file_components)
        .then_with(|| {
            left.exact_callsite_start_byte
                .cmp(&right.exact_callsite_start_byte)
        })
        .then_with(|| {
            match (
                left.receipt.edge_id.parse::<i64>(),
                right.receipt.edge_id.parse::<i64>(),
            ) {
                (Ok(left), Ok(right)) => left.cmp(&right),
                _ => left.receipt.edge_id.cmp(&right.receipt.edge_id),
            }
        })
        .then_with(|| left.receipt.receipt_id.cmp(&right.receipt.receipt_id))
}

fn authoritative_receipt_order(receipts: &[IndexedCallEdgeReceipt]) -> BTreeMap<ReceiptRef, usize> {
    let mut receipts = receipts.iter().collect::<Vec<_>>();
    receipts.sort_by(|left, right| indexed_receipt_source_order(left, right));
    receipts
        .into_iter()
        .enumerate()
        .map(|(rank, receipt)| (receipt.receipt.clone(), rank))
        .collect()
}

fn integrate_built_disposition(
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    built: &BuiltCallPathFacts,
) -> ProofDisposition {
    let mut facts = built.facts.clone();
    facts.extend(
        built
            .unavailable
            .iter()
            .cloned()
            .map(|reason| VerifiedProofFact::Unavailable(UnavailableProofFact { reason })),
    );
    let receipt_order = authoritative_receipt_order(&built.receipts);
    let raw = check_call_path_with_receipt_order(contract, hashes, &facts, Some(&receipt_order));
    if matches!(
        raw,
        ProofDisposition::Unavailable { .. } | ProofDisposition::ContractRefuted { .. }
    ) || built.gaps.is_empty()
    {
        return raw;
    }

    let (mut gaps, mut connected_receipts) = match raw {
        ProofDisposition::ContractProven { receipts, .. } => (Vec::new(), receipts),
        ProofDisposition::Unknown {
            gaps,
            connected_receipts,
            ..
        } => (gaps, connected_receipts),
        _ => unreachable!("unavailable and refuted dispositions returned above"),
    };
    let exact_builder_gaps = built
        .gaps
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let explained_steps = exact_builder_gaps
        .iter()
        .filter_map(|gap| fact_build_gap_step(gap, contract.spec.steps.len()))
        .collect::<BTreeSet<_>>();
    gaps.retain(|gap| {
        !matches!(
            gap,
            ProofGap::MissingDirectCallReceipt { step_index }
                if explained_steps.contains(step_index)
        )
    });
    gaps.extend(exact_builder_gaps.into_iter().map(ProofGap::FactBuild));
    gaps.sort();
    gaps.dedup();
    if let Some(first_blocked_step) = explained_steps.iter().next().copied() {
        connected_receipts.truncate(first_blocked_step);
    }
    ProofDisposition::Unknown {
        contract_digest: hashes.contract_digest.clone(),
        gaps,
        connected_receipts,
    }
}

fn fact_build_gap_step(gap: &FactBuildGap, step_count: usize) -> Option<usize> {
    match gap {
        FactBuildGap::SelectorMissing { selector_index }
        | FactBuildGap::SelectorAmbiguous { selector_index }
        | FactBuildGap::NonCallableSelector { selector_index } => match selector_index {
            0 => Some(0),
            index if *index <= step_count => Some(index - 1),
            _ => None,
        },
        FactBuildGap::DirectCallMissing { step_index }
        | FactBuildGap::RecursiveCallNotRepresentable { step_index }
        | FactBuildGap::SourceWindowTooLarge { step_index }
        | FactBuildGap::InvalidUtf8 { step_index }
        | FactBuildGap::SourceLineOutOfRange { step_index }
        | FactBuildGap::EdgeContainmentUnproven { step_index } => Some(*step_index),
    }
}

fn authoritative_receipt_refs(disposition: &ProofDisposition) -> &[ReceiptRef] {
    match disposition {
        ProofDisposition::ContractProven { receipts, .. } => receipts,
        ProofDisposition::ContractRefuted {
            refutation:
                Refutation::ProhibitedScopeTraversal {
                    connected_receipts, ..
                },
            ..
        } => connected_receipts,
        #[cfg(any(test, feature = "test-support"))]
        ProofDisposition::ContractRefuted {
            refutation:
                Refutation::CertifiedAbsence {
                    connected_receipts, ..
                },
            ..
        } => connected_receipts,
        ProofDisposition::Unknown {
            connected_receipts, ..
        } => connected_receipts,
        ProofDisposition::Unavailable { .. } => &[],
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactFileIdentity {
    file_node_id: Option<i64>,
    project_file_components: Option<Vec<String>>,
    indexed_sha256: Option<String>,
    observed_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactSymbolIdentity {
    node_id: String,
    canonical_id: Option<String>,
    qualified_name: Option<String>,
    file: Option<u32>,
}

#[derive(Default)]
struct CompactIdentityTables {
    files: Vec<CompactFileIdentity>,
    file_by_node_id: BTreeMap<i64, u32>,
    file_by_path: BTreeMap<Vec<String>, u32>,
    symbols: Vec<CompactSymbolIdentity>,
    symbol_by_node_id: BTreeMap<String, u32>,
    evidence: Vec<Value>,
    evidence_by_fact_id: BTreeMap<String, u32>,
}

impl CompactIdentityTables {
    fn intern_file(
        &mut self,
        file_node_id: Option<i64>,
        project_file_components: Option<Vec<String>>,
        indexed_sha256: Option<String>,
        observed_sha256: Option<String>,
    ) -> Result<u32, InternalProjectionError> {
        let by_id = file_node_id.and_then(|id| self.file_by_node_id.get(&id).copied());
        let by_path = project_file_components
            .as_ref()
            .and_then(|path| self.file_by_path.get(path).copied());
        if by_id
            .zip(by_path)
            .is_some_and(|(left, right)| left != right)
        {
            return Err(InternalProjectionError::InvalidCompactProjection(
                "conflicting_file_identity".to_owned(),
            ));
        }
        let index = by_id
            .or(by_path)
            .unwrap_or(bounded_index(self.files.len())?);
        if usize::try_from(index).expect("u32 index fits usize") == self.files.len() {
            self.files.push(CompactFileIdentity {
                file_node_id: None,
                project_file_components: None,
                indexed_sha256: None,
                observed_sha256: None,
            });
        }
        let file = &mut self.files[usize::try_from(index).expect("u32 index fits usize")];
        merge_compact_field(
            &mut file.file_node_id,
            file_node_id,
            "conflicting_file_identity",
        )?;
        merge_compact_field(
            &mut file.project_file_components,
            project_file_components,
            "conflicting_file_identity",
        )?;
        merge_compact_field(
            &mut file.indexed_sha256,
            indexed_sha256,
            "conflicting_file_identity",
        )?;
        merge_compact_field(
            &mut file.observed_sha256,
            observed_sha256,
            "conflicting_file_identity",
        )?;
        if let Some(id) = file.file_node_id {
            self.file_by_node_id.insert(id, index);
        }
        if let Some(path) = &file.project_file_components {
            self.file_by_path.insert(path.clone(), index);
        }
        Ok(index)
    }

    fn intern_resolved_symbol(
        &mut self,
        node: &ResolvedNodeIdentity,
    ) -> Result<u32, InternalProjectionError> {
        let file = self.intern_file(
            Some(node.file_node_id.0),
            Some(node.project_file_components.clone()),
            None,
            None,
        )?;
        let identity = CompactSymbolIdentity {
            node_id: node.pinned.node_id.clone(),
            canonical_id: Some(node.canonical_id.clone()),
            qualified_name: Some(node.qualified_name.clone()),
            file: Some(file),
        };
        if let Some(index) = self.symbol_by_node_id.get(&identity.node_id).copied() {
            let current = &mut self.symbols[usize::try_from(index).expect("u32 index fits usize")];
            if current.canonical_id.is_none()
                && current.qualified_name.is_none()
                && current.file.is_none()
            {
                *current = identity;
            } else if *current != identity {
                return Err(InternalProjectionError::InvalidCompactProjection(
                    "conflicting_symbol_identity".to_owned(),
                ));
            }
            return Ok(index);
        }
        let index = bounded_index(self.symbols.len())?;
        self.symbols.push(identity.clone());
        self.symbol_by_node_id.insert(identity.node_id, index);
        Ok(index)
    }

    fn intern_evidence_symbol(&mut self, node_id: NodeId) -> Result<u32, InternalProjectionError> {
        let node_id = node_id.0.to_string();
        if let Some(index) = self.symbol_by_node_id.get(&node_id).copied() {
            return Ok(index);
        }
        let index = bounded_index(self.symbols.len())?;
        self.symbols.push(CompactSymbolIdentity {
            node_id: node_id.clone(),
            canonical_id: None,
            qualified_name: None,
            file: None,
        });
        self.symbol_by_node_id.insert(node_id, index);
        Ok(index)
    }

    fn intern_evidence(
        &mut self,
        receipt: &IndexedCallEdgeReceipt,
        source: u32,
        target: u32,
    ) -> Result<u32, InternalProjectionError> {
        if receipt.resolution_evidence_chain.is_empty()
            || !valid_resolution_provenance(&receipt.resolution_provenance)
        {
            return Err(InternalProjectionError::InvalidCompactProjection(
                "resolution_provenance_invalid".to_owned(),
            ));
        }
        let chain = receipt
            .resolution_evidence_chain
            .iter()
            .map(|evidence| {
                Ok(json!({
                    "kind": evidence.kind().as_str(),
                    "symbols": evidence.node_ids().into_iter().map(|node| self.intern_evidence_symbol(node).map(Value::from)).collect::<Result<Vec<_>, _>>()?,
                }))
            })
            .collect::<Result<Vec<_>, InternalProjectionError>>()?;
        let dependency_files = receipt
            .resolution_provenance
            .dependency_file_hashes
            .iter()
            .map(|dependency| {
                self.intern_file(
                    Some(dependency.file_id.0),
                    None,
                    Some(dependency.source_sha256.clone()),
                    None,
                )
                .map(Value::from)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let provenance = &receipt.resolution_provenance;
        let row = json!({
            "fact_id": receipt.resolution_fact_id,
            "caller": source,
            "target": target,
            "edge_id": receipt.receipt.edge_id,
            "callsite_identity": receipt.callsite_identity,
            "chain": chain,
            "provenance": {
                "producer": provenance.producer,
                "fact_schema_version": provenance.fact_schema_version,
                "algorithm": provenance.algorithm,
                "language_adapter": provenance.language_adapter,
                "language_adapter_version": provenance.language_adapter_version,
                "parser_fingerprint": provenance.parser_fingerprint,
                "dependency_files": dependency_files,
                "evidence_sha256": provenance.evidence_sha256,
            },
        });
        if let Some(index) = self
            .evidence_by_fact_id
            .get(&receipt.resolution_fact_id)
            .copied()
        {
            if self.evidence[usize::try_from(index).expect("u32 index fits usize")] != row {
                return Err(InternalProjectionError::InvalidCompactProjection(
                    "conflicting_resolution_fact".to_owned(),
                ));
            }
            return Ok(index);
        }
        let index = bounded_index(self.evidence.len())?;
        self.evidence.push(row);
        self.evidence_by_fact_id
            .insert(receipt.resolution_fact_id.clone(), index);
        Ok(index)
    }

    fn receipt_json(
        &mut self,
        receipt: &IndexedCallEdgeReceipt,
    ) -> Result<Value, InternalProjectionError> {
        let file = self.intern_file(
            Some(receipt.containment.file_node_id.0),
            Some(receipt.line_window.project_file_components.clone()),
            Some(receipt.line_window.indexed_sha256.clone()),
            Some(receipt.line_window.observed_sha256.clone()),
        )?;
        let source = self.intern_resolved_symbol(&receipt.source)?;
        let target = self.intern_resolved_symbol(&receipt.target)?;
        let source_file = self.symbols[usize::try_from(source).expect("u32 index fits usize")].file;
        if source_file != Some(file) {
            return Err(InternalProjectionError::InvalidCompactProjection(
                "callsite_source_file_mismatch".to_owned(),
            ));
        }
        let owner = self.intern_evidence_symbol(receipt.containment.owner_node_id)?;
        let evidence = self.intern_evidence(receipt, source, target)?;
        Ok(json!({
            "receipt_id": receipt.receipt.receipt_id,
            "edge_id": receipt.receipt.edge_id,
            "source": source,
            "target": target,
            "evidence": evidence,
            "exact_callsite_start_byte": receipt.exact_callsite_start_byte,
            "callsite_identity": receipt.callsite_identity,
            "column_or_ordinal": receipt.column_or_ordinal,
            "containment": {
                "file": file,
                "owner": owner,
                "start_line": receipt.containment.start_line,
                "end_line": receipt.containment.end_line,
            },
            "line_window": {
                "kind": receipt.line_window.kind,
                "file": file,
                "anchor_line": receipt.line_window.anchor_line,
                "byte_start": receipt.line_window.byte_start,
                "byte_end": receipt.line_window.byte_end,
                "text": receipt.line_window.text,
            },
        }))
    }

    fn json(&self) -> Value {
        json!({
            "files": self.files.iter().map(|file| json!({
                "file_node_id": file.file_node_id.map(|id| id.to_string()),
                "project_file_components": file.project_file_components,
                "indexed_sha256": file.indexed_sha256,
                "observed_sha256": file.observed_sha256,
            })).collect::<Vec<_>>(),
            "symbols": self.symbols.iter().map(|symbol| json!({
                "node_id": symbol.node_id,
                "canonical_id": symbol.canonical_id,
                "qualified_name": symbol.qualified_name,
                "file": symbol.file,
            })).collect::<Vec<_>>(),
            "evidence": self.evidence,
        })
    }
}

fn bounded_index(length: usize) -> Result<u32, InternalProjectionError> {
    u32::try_from(length).map_err(|_| {
        InternalProjectionError::InvalidCompactProjection("compact_table_index_overflow".to_owned())
    })
}

fn merge_compact_field<T: PartialEq>(
    current: &mut Option<T>,
    incoming: Option<T>,
    code: &str,
) -> Result<(), InternalProjectionError> {
    match (current.as_ref(), incoming) {
        (Some(existing), Some(incoming)) if existing != &incoming => Err(
            InternalProjectionError::InvalidCompactProjection(code.to_owned()),
        ),
        (None, Some(incoming)) => {
            *current = Some(incoming);
            Ok(())
        }
        _ => Ok(()),
    }
}

fn valid_resolution_provenance(provenance: &ResolutionProvenance) -> bool {
    provenance.producer == INTERNAL_RESOLUTION_PRODUCER
        && provenance.fact_schema_version == PROOF_RESOLUTION_FACT_SCHEMA_VERSION
        && provenance.algorithm == EXACT_CALL_RESOLUTION_ALGORITHM
        && !provenance.language_adapter.is_empty()
        && !provenance.language_adapter_version.is_empty()
        && is_lower_hex_sha256(&provenance.parser_fingerprint)
        && is_lower_hex_sha256(&provenance.evidence_sha256)
        && !provenance.dependency_file_hashes.is_empty()
        && provenance
            .dependency_file_hashes
            .windows(2)
            .all(|pair| pair[0].file_id < pair[1].file_id)
        && provenance.dependency_file_hashes.iter().all(|dependency| {
            dependency.file_id.0 != 0 && is_lower_hex_sha256(&dependency.source_sha256)
        })
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Rejects compact roots whose numeric references no longer describe one
/// complete proof. The CLI calls this before serializing every revision.
pub fn validate_compact_projection(root: &Value) -> Result<(), String> {
    let root_object = compact_object(root, "compact_root_invalid")?;
    let kind = compact_string(root_object, "kind", "compact_root_kind_invalid")?;
    if kind == "budget_exceeded" {
        return validate_budget_projection(root_object);
    }
    if kind != "complete" {
        return Err("compact_root_kind_invalid".to_owned());
    }
    compact_closed_object(
        root_object,
        &[
            "kind",
            "schema_version",
            "domain",
            "contract_interpretation",
            "guard_version",
            "source_text_sha256",
            "contract_digest",
            "core_publication",
            "identities",
            "spec",
            "clauses",
            "disposition",
            "steps",
            "receipts",
        ],
        "compact_complete_shape_invalid",
    )?;
    validate_common_projection_fields(root_object)?;
    let identities = compact_object_field(root_object, "identities", "compact_identities_missing")?;
    compact_closed_object(
        identities,
        &["files", "symbols", "evidence"],
        "compact_identities_shape_invalid",
    )?;
    let files = compact_array(identities.get("files"), "compact_files_missing")?;
    let symbols = compact_array(identities.get("symbols"), "compact_symbols_missing")?;
    let evidence = compact_array(identities.get("evidence"), "compact_evidence_missing")?;
    let receipts = compact_array(root_object.get("receipts"), "compact_receipts_missing")?;
    let spec = compact_object_field(root_object, "spec", "compact_spec_missing")?;
    let spec_steps = compact_array(spec.get("steps"), "compact_spec_steps_missing")?;
    let steps = compact_array(root_object.get("steps"), "compact_steps_missing")?;
    if steps.len() != spec_steps.len() {
        return Err("compact_step_count_mismatch".to_owned());
    }

    let mut file_ids = BTreeSet::new();
    let mut file_paths = BTreeSet::new();
    for file in files {
        let file = compact_object(file, "compact_file_row_invalid")?;
        compact_closed_object(
            file,
            &[
                "file_node_id",
                "project_file_components",
                "indexed_sha256",
                "observed_sha256",
            ],
            "compact_file_shape_invalid",
        )?;
        let id = compact_i64(file, "file_node_id", "compact_file_id_invalid")?;
        let indexed = compact_hash(file, "indexed_sha256", "compact_file_hash_invalid")?;
        if !file_ids.insert(id) {
            return Err("compact_file_id_duplicate".to_owned());
        }
        if let Some(path) = compact_optional_path(file, "project_file_components")?
            && !file_paths.insert(path)
        {
            return Err("compact_file_path_duplicate".to_owned());
        }
        if let Some(observed) = file.get("observed_sha256").filter(|value| !value.is_null())
            && (observed.as_str() != Some(indexed) || !is_lower_hex_sha256(indexed))
        {
            return Err("compact_file_observed_hash_invalid".to_owned());
        }
    }

    let mut symbol_ids = BTreeSet::new();
    for symbol in symbols {
        let symbol = compact_object(symbol, "compact_symbol_row_invalid")?;
        compact_closed_object(
            symbol,
            &["node_id", "canonical_id", "qualified_name", "file"],
            "compact_symbol_shape_invalid",
        )?;
        let node_id = compact_string(symbol, "node_id", "compact_symbol_id_invalid")?;
        if node_id.is_empty() || !symbol_ids.insert(node_id) {
            return Err("compact_symbol_id_duplicate".to_owned());
        }
        if let Some(file) = symbol.get("file").filter(|file| !file.is_null()) {
            compact_index(file, files.len(), "compact_symbol_file_reference_invalid")?;
        }
    }
    let mut fact_ids = BTreeSet::new();
    for evidence_row in evidence {
        let evidence_row = compact_object(evidence_row, "compact_evidence_row_invalid")?;
        compact_closed_object(
            evidence_row,
            &[
                "fact_id",
                "caller",
                "target",
                "edge_id",
                "callsite_identity",
                "chain",
                "provenance",
            ],
            "compact_evidence_shape_invalid",
        )?;
        let fact_id = compact_hash(evidence_row, "fact_id", "compact_fact_id_invalid")?;
        if !fact_ids.insert(fact_id) {
            return Err("compact_fact_id_duplicate".to_owned());
        }
        compact_index(
            evidence_row.get("caller").unwrap_or(&Value::Null),
            symbols.len(),
            "compact_evidence_caller_reference_invalid",
        )?;
        compact_index(
            evidence_row.get("target").unwrap_or(&Value::Null),
            symbols.len(),
            "compact_evidence_target_reference_invalid",
        )?;
        compact_full_symbol(
            symbols,
            compact_index(
                evidence_row.get("caller").unwrap_or(&Value::Null),
                symbols.len(),
                "compact_evidence_caller_reference_invalid",
            )?,
            files.len(),
        )?;
        compact_full_symbol(
            symbols,
            compact_index(
                evidence_row.get("target").unwrap_or(&Value::Null),
                symbols.len(),
                "compact_evidence_target_reference_invalid",
            )?,
            files.len(),
        )?;
        if compact_string(evidence_row, "edge_id", "compact_evidence_edge_invalid")?.is_empty()
            || compact_string(
                evidence_row,
                "callsite_identity",
                "compact_evidence_callsite_invalid",
            )?
            .is_empty()
        {
            return Err("compact_evidence_key_invalid".to_owned());
        }
        for chain in compact_array(evidence_row.get("chain"), "compact_evidence_chain_missing")? {
            let chain = compact_object(chain, "compact_evidence_chain_invalid")?;
            compact_closed_object(
                chain,
                &["kind", "symbols"],
                "compact_evidence_chain_shape_invalid",
            )?;
            let kind = compact_string(chain, "kind", "compact_evidence_kind_invalid")?;
            let chain_symbols = compact_array(
                chain.get("symbols"),
                "compact_evidence_chain_symbols_missing",
            )?;
            let expected = match kind {
                "same_file_declaration"
                | "same_package_declaration"
                | "explicit_receiver_type"
                | "constructor_binding"
                | "implicit_receiver" => Some(1),
                "static_import_binding" => Some(2),
                "qualified_path" => None,
                _ => return Err("compact_evidence_kind_invalid".to_owned()),
            };
            if expected.is_some_and(|expected| chain_symbols.len() != expected)
                || kind == "qualified_path" && chain_symbols.is_empty()
            {
                return Err("compact_evidence_chain_arity_invalid".to_owned());
            }
            for symbol in chain_symbols {
                compact_index(
                    symbol,
                    symbols.len(),
                    "compact_evidence_symbol_reference_invalid",
                )?;
            }
        }
        let provenance = compact_object_field(
            evidence_row,
            "provenance",
            "compact_evidence_provenance_missing",
        )?;
        compact_closed_object(
            provenance,
            &[
                "producer",
                "fact_schema_version",
                "algorithm",
                "language_adapter",
                "language_adapter_version",
                "parser_fingerprint",
                "dependency_files",
                "evidence_sha256",
            ],
            "compact_provenance_shape_invalid",
        )?;
        if compact_string(provenance, "producer", "compact_provenance_invalid")?
            != INTERNAL_RESOLUTION_PRODUCER
            || compact_u64(
                provenance,
                "fact_schema_version",
                "compact_provenance_invalid",
            )? != u64::from(PROOF_RESOLUTION_FACT_SCHEMA_VERSION)
            || compact_string(provenance, "algorithm", "compact_provenance_invalid")?
                != EXACT_CALL_RESOLUTION_ALGORITHM
            || compact_string(provenance, "language_adapter", "compact_provenance_invalid")?
                .is_empty()
            || compact_string(
                provenance,
                "language_adapter_version",
                "compact_provenance_invalid",
            )?
            .is_empty()
        {
            return Err("compact_provenance_invalid".to_owned());
        }
        compact_hash(
            provenance,
            "parser_fingerprint",
            "compact_provenance_invalid",
        )?;
        compact_hash(provenance, "evidence_sha256", "compact_provenance_invalid")?;
        let dependencies = compact_array(
            provenance.get("dependency_files"),
            "compact_dependency_files_missing",
        )?;
        if dependencies.is_empty() {
            return Err("compact_dependency_files_missing".to_owned());
        }
        let mut prior_file_id = None;
        for file in dependencies {
            let index = compact_index(
                file,
                files.len(),
                "compact_dependency_file_reference_invalid",
            )?;
            let file_id = compact_i64(
                compact_object(&files[index], "compact_file_row_invalid")?,
                "file_node_id",
                "compact_file_id_invalid",
            )?;
            if prior_file_id.is_some_and(|prior| prior >= file_id) {
                return Err("compact_dependency_files_noncanonical".to_owned());
            }
            prior_file_id = Some(file_id);
        }
    }
    let mut receipt_ids = BTreeSet::new();
    let mut edge_ids = BTreeSet::new();
    let mut evidence_indices = BTreeSet::new();
    let mut source_files = BTreeSet::new();
    for receipt in receipts {
        let receipt = compact_object(receipt, "compact_receipt_row_invalid")?;
        compact_closed_object(
            receipt,
            &[
                "receipt_id",
                "edge_id",
                "source",
                "target",
                "evidence",
                "exact_callsite_start_byte",
                "callsite_identity",
                "column_or_ordinal",
                "containment",
                "line_window",
            ],
            "compact_receipt_shape_invalid",
        )?;
        let receipt_id = compact_string(receipt, "receipt_id", "compact_receipt_id_invalid")?;
        let edge_id = compact_string(receipt, "edge_id", "compact_receipt_edge_invalid")?;
        if receipt_id.is_empty() || !receipt_ids.insert(receipt_id) {
            return Err("compact_receipt_id_duplicate".to_owned());
        }
        if edge_id.is_empty() || !edge_ids.insert(edge_id) {
            return Err("compact_receipt_edge_duplicate".to_owned());
        }
        let source = compact_index(
            receipt.get("source").unwrap_or(&Value::Null),
            symbols.len(),
            "compact_receipt_source_reference_invalid",
        )?;
        let target = compact_index(
            receipt.get("target").unwrap_or(&Value::Null),
            symbols.len(),
            "compact_receipt_target_reference_invalid",
        )?;
        let evidence_index = compact_index(
            receipt.get("evidence").unwrap_or(&Value::Null),
            evidence.len(),
            "compact_receipt_evidence_reference_invalid",
        )?;
        if !evidence_indices.insert(evidence_index) {
            return Err("compact_receipt_evidence_duplicate".to_owned());
        }
        let evidence_row = compact_object(
            evidence
                .get(evidence_index)
                .expect("bounded evidence index"),
            "compact_evidence_row_invalid",
        )?;
        if compact_index(
            evidence_row.get("caller").unwrap_or(&Value::Null),
            symbols.len(),
            "compact_evidence_caller_reference_invalid",
        )? != source
            || compact_index(
                evidence_row.get("target").unwrap_or(&Value::Null),
                symbols.len(),
                "compact_evidence_target_reference_invalid",
            )? != target
            || compact_string(evidence_row, "edge_id", "compact_evidence_edge_invalid")? != edge_id
            || compact_string(
                evidence_row,
                "callsite_identity",
                "compact_evidence_callsite_invalid",
            )? != compact_string(
                receipt,
                "callsite_identity",
                "compact_receipt_callsite_invalid",
            )?
        {
            return Err("compact_receipt_evidence_correlation_invalid".to_owned());
        }
        let containment = compact_object_field(
            receipt,
            "containment",
            "compact_receipt_containment_missing",
        )?;
        compact_closed_object(
            containment,
            &["file", "owner", "start_line", "end_line"],
            "compact_containment_shape_invalid",
        )?;
        let line_window = compact_object_field(
            receipt,
            "line_window",
            "compact_receipt_line_window_missing",
        )?;
        compact_closed_object(
            line_window,
            &[
                "kind",
                "file",
                "anchor_line",
                "byte_start",
                "byte_end",
                "text",
            ],
            "compact_line_window_shape_invalid",
        )?;
        let containment_file = compact_index(
            containment.get("file").unwrap_or(&Value::Null),
            files.len(),
            "compact_containment_file_reference_invalid",
        )?;
        if compact_index(
            containment.get("owner").unwrap_or(&Value::Null),
            symbols.len(),
            "compact_containment_owner_reference_invalid",
        )? != source
        {
            return Err("compact_receipt_containment_correlation_invalid".to_owned());
        }
        let line_file = compact_index(
            line_window.get("file").unwrap_or(&Value::Null),
            files.len(),
            "compact_line_window_file_reference_invalid",
        )?;
        let source_file = compact_full_symbol(symbols, source, files.len())?;
        compact_full_symbol(symbols, target, files.len())?;
        if source_file != containment_file || containment_file != line_file {
            return Err("compact_receipt_file_reference_mismatch".to_owned());
        }
        source_files.insert(source_file);
        validate_compact_line_window(line_window, files, containment_file, containment, receipt)?;
    }
    if evidence_indices.len() != evidence.len() {
        return Err("compact_evidence_unreferenced".to_owned());
    }
    for (index, file) in files.iter().enumerate() {
        if compact_object(file, "compact_file_row_invalid")?
            .get("observed_sha256")
            .is_some_and(|value| !value.is_null())
            && !source_files.contains(&index)
        {
            return Err("compact_observed_hash_not_callsite_source".to_owned());
        }
    }
    let step_rows = validate_compact_steps(steps, spec_steps.len(), receipts.len())?;
    validate_disposition_receipts(
        compact_object_field(root_object, "disposition", "compact_disposition_missing")?,
        compact_string(
            root_object,
            "contract_digest",
            "compact_contract_digest_invalid",
        )?,
        &step_rows,
        receipts,
    )
}

fn compact_array<'a>(value: Option<&'a Value>, code: &str) -> Result<&'a Vec<Value>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| code.to_owned())
}

fn compact_index(value: &Value, length: usize, code: &str) -> Result<usize, String> {
    value
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < length)
        .ok_or_else(|| code.to_owned())
}

fn compact_object<'a>(
    value: &'a Value,
    code: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    value.as_object().ok_or_else(|| code.to_owned())
}

fn compact_object_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    code: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    object
        .get(field)
        .and_then(Value::as_object)
        .ok_or_else(|| code.to_owned())
}

fn compact_closed_object(
    object: &serde_json::Map<String, Value>,
    fields: &[&str],
    code: &str,
) -> Result<(), String> {
    if object.len() != fields.len()
        || fields.iter().any(|field| !object.contains_key(*field))
        || object.keys().any(|field| !fields.contains(&field.as_str()))
    {
        return Err(code.to_owned());
    }
    Ok(())
}

fn compact_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    code: &str,
) -> Result<&'a str, String> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| code.to_owned())
}

fn compact_u64(
    object: &serde_json::Map<String, Value>,
    field: &str,
    code: &str,
) -> Result<u64, String> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| code.to_owned())
}

fn compact_i64(
    object: &serde_json::Map<String, Value>,
    field: &str,
    code: &str,
) -> Result<i64, String> {
    compact_string(object, field, code)?
        .parse::<i64>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| code.to_owned())
}

fn compact_hash<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
    code: &str,
) -> Result<&'a str, String> {
    let value = compact_string(object, field, code)?;
    is_lower_hex_sha256(value)
        .then_some(value)
        .ok_or_else(|| code.to_owned())
}

fn compact_optional_path(
    object: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(value) = object.get(field) else {
        return Err("compact_file_path_invalid".to_owned());
    };
    if value.is_null() {
        return Ok(None);
    }
    let path = value
        .as_array()
        .filter(|path| !path.is_empty())
        .and_then(|path| {
            path.iter()
                .map(Value::as_str)
                .map(|part| part.filter(|part| !part.is_empty()).map(ToOwned::to_owned))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| "compact_file_path_invalid".to_owned())?;
    Ok(Some(path))
}

fn validate_common_projection_fields(root: &serde_json::Map<String, Value>) -> Result<(), String> {
    if compact_u64(root, "schema_version", "compact_schema_version_invalid")? != 1
        || compact_string(root, "domain", "compact_domain_invalid")? != PROOF_DOMAIN
        || compact_string(
            root,
            "contract_interpretation",
            "compact_interpretation_invalid",
        )? != "host_supplied"
        || compact_string(root, "guard_version", "compact_guard_version_invalid")?
            != CLAUSE_GUARD_VERSION
    {
        return Err("compact_common_fields_invalid".to_owned());
    }
    compact_hash(root, "source_text_sha256", "compact_source_hash_invalid")?;
    compact_hash(root, "contract_digest", "compact_contract_digest_invalid")?;
    let publication =
        compact_object_field(root, "core_publication", "compact_publication_missing")?;
    compact_closed_object(
        publication,
        &["project_id", "generation_id", "run_id"],
        "compact_publication_shape_invalid",
    )?;
    for field in ["project_id", "generation_id", "run_id"] {
        if compact_string(publication, field, "compact_publication_invalid")?.is_empty() {
            return Err("compact_publication_invalid".to_owned());
        }
    }
    Ok(())
}

fn validate_budget_projection(root: &serde_json::Map<String, Value>) -> Result<(), String> {
    compact_closed_object(
        root,
        &[
            "kind",
            "schema_version",
            "domain",
            "contract_interpretation",
            "guard_version",
            "source_text_sha256",
            "contract_digest",
            "core_publication",
            "disposition",
            "cap_bytes",
            "required_complete_size",
        ],
        "compact_budget_shape_invalid",
    )?;
    validate_common_projection_fields(root)?;
    if compact_u64(root, "cap_bytes", "compact_budget_cap_invalid")? == 0
        || compact_u64(
            root,
            "required_complete_size",
            "compact_budget_required_size_invalid",
        )? == 0
    {
        return Err("compact_budget_size_invalid".to_owned());
    }
    let disposition = compact_object_field(root, "disposition", "compact_disposition_missing")?;
    compact_closed_object(
        disposition,
        &["kind", "contract_digest", "gaps"],
        "compact_budget_disposition_shape_invalid",
    )?;
    if compact_string(disposition, "kind", "compact_disposition_kind_invalid")? != "unknown"
        || compact_string(
            disposition,
            "contract_digest",
            "compact_disposition_digest_invalid",
        )? != compact_string(root, "contract_digest", "compact_contract_digest_invalid")?
        || compact_array(disposition.get("gaps"), "compact_budget_gaps_missing")?.as_slice()
            != [json!({"kind":"output_budget_exceeded"})]
    {
        return Err("compact_budget_disposition_invalid".to_owned());
    }
    Ok(())
}

fn compact_full_symbol(
    symbols: &[Value],
    index: usize,
    file_count: usize,
) -> Result<usize, String> {
    let symbol = compact_object(
        symbols
            .get(index)
            .ok_or_else(|| "compact_symbol_reference_invalid".to_owned())?,
        "compact_symbol_row_invalid",
    )?;
    if compact_string(symbol, "canonical_id", "compact_symbol_not_full")?.is_empty()
        || compact_string(symbol, "qualified_name", "compact_symbol_not_full")?.is_empty()
    {
        return Err("compact_symbol_not_full".to_owned());
    }
    compact_index(
        symbol.get("file").unwrap_or(&Value::Null),
        file_count,
        "compact_symbol_not_full",
    )
}

fn validate_compact_line_window(
    line: &serde_json::Map<String, Value>,
    files: &[Value],
    file: usize,
    containment: &serde_json::Map<String, Value>,
    receipt: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    if compact_string(line, "kind", "compact_line_window_invalid")? != "indexed_line_v1" {
        return Err("compact_line_window_invalid".to_owned());
    }
    if compact_index(
        line.get("file").unwrap_or(&Value::Null),
        files.len(),
        "compact_line_window_file_reference_invalid",
    )? != file
    {
        return Err("compact_line_window_file_mismatch".to_owned());
    }
    let anchor_line = compact_u64(line, "anchor_line", "compact_line_window_invalid")?;
    let byte_start = compact_u64(line, "byte_start", "compact_line_window_invalid")?;
    let byte_end = compact_u64(line, "byte_end", "compact_line_window_invalid")?;
    let text = compact_string(line, "text", "compact_line_window_invalid")?;
    if byte_end < byte_start
        || byte_end - byte_start != u64::try_from(text.len()).unwrap_or(u64::MAX)
        || anchor_line < compact_u64(containment, "start_line", "compact_containment_invalid")?
        || anchor_line > compact_u64(containment, "end_line", "compact_containment_invalid")?
    {
        return Err("compact_line_window_invalid".to_owned());
    }
    let callsite = compact_string(
        receipt,
        "callsite_identity",
        "compact_receipt_callsite_invalid",
    )?;
    let parts = callsite
        .split('|')
        .next()
        .map(|prefix| prefix.split(':').collect::<Vec<_>>())
        .filter(|parts| parts.len() == 4)
        .ok_or_else(|| "compact_receipt_callsite_invalid".to_owned())?;
    let expected_file = compact_i64(
        compact_object(&files[file], "compact_file_row_invalid")?,
        "file_node_id",
        "compact_file_id_invalid",
    )?;
    let callsite_file = parts[0]
        .parse::<i64>()
        .ok()
        .ok_or_else(|| "compact_receipt_callsite_invalid".to_owned())?;
    let callsite_line = parts[1]
        .parse::<u64>()
        .ok()
        .ok_or_else(|| "compact_receipt_callsite_invalid".to_owned())?;
    parts[2]
        .parse::<u64>()
        .ok()
        .ok_or_else(|| "compact_receipt_callsite_invalid".to_owned())?;
    let exact_start = compact_u64(
        receipt,
        "exact_callsite_start_byte",
        "compact_receipt_start_invalid",
    )?;
    if callsite_file != expected_file
        || callsite_line != anchor_line
        || parts[3].is_empty()
        || exact_start < byte_start
        || exact_start >= byte_end
    {
        return Err("compact_receipt_callsite_correlation_invalid".to_owned());
    }
    Ok(())
}

fn validate_compact_steps(
    steps: &[Value],
    expected_count: usize,
    receipt_count: usize,
) -> Result<Vec<(String, Option<usize>)>, String> {
    if steps.is_empty() || steps.len() != expected_count || steps.len() > MAX_STEPS {
        return Err("compact_step_count_mismatch".to_owned());
    }
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let step = compact_object(step, "compact_step_row_invalid")?;
            compact_closed_object(
                step,
                &["step_index", "status", "receipt"],
                "compact_step_shape_invalid",
            )?;
            if compact_u64(step, "step_index", "compact_step_index_invalid")?
                != u64::try_from(index).unwrap()
            {
                return Err("compact_step_index_invalid".to_owned());
            }
            let status = compact_string(step, "status", "compact_step_status_invalid")?;
            if !matches!(
                status,
                "proven"
                    | "positive_contradiction"
                    | "certified_absence"
                    | "unavailable"
                    | "unknown"
            ) {
                return Err("compact_step_status_invalid".to_owned());
            }
            let receipt = step
                .get("receipt")
                .filter(|value| !value.is_null())
                .map(|receipt| {
                    compact_index(
                        receipt,
                        receipt_count,
                        "compact_step_receipt_reference_invalid",
                    )
                })
                .transpose()?;
            Ok((status.to_owned(), receipt))
        })
        .collect()
}

fn validate_disposition_receipts(
    disposition: &serde_json::Map<String, Value>,
    contract_digest: &str,
    steps: &[(String, Option<usize>)],
    receipts: &[Value],
) -> Result<(), String> {
    if compact_string(
        disposition,
        "contract_digest",
        "compact_disposition_digest_invalid",
    )? != contract_digest
    {
        return Err("compact_disposition_digest_invalid".to_owned());
    }
    match compact_string(disposition, "kind", "compact_disposition_kind_invalid")? {
        "contract_proven" => {
            compact_closed_object(
                disposition,
                &["kind", "contract_digest", "receipts"],
                "compact_disposition_shape_invalid",
            )?;
            let sequence = compact_receipt_sequence(disposition.get("receipts"), receipts)?;
            if sequence.len() != steps.len()
                || !steps
                    .iter()
                    .zip(&sequence)
                    .all(|((status, receipt), index)| {
                        status == "proven" && *receipt == Some(*index)
                    })
            {
                return Err("compact_proven_step_sequence_invalid".to_owned());
            }
            validate_receipt_sequence(&sequence, receipts)
        }
        "unknown" => {
            compact_closed_object(
                disposition,
                &["kind", "contract_digest", "gaps", "connected_receipts"],
                "compact_disposition_shape_invalid",
            )?;
            compact_array(disposition.get("gaps"), "compact_disposition_gaps_missing")?;
            let sequence =
                compact_receipt_sequence(disposition.get("connected_receipts"), receipts)?;
            validate_prefix_steps(steps, &sequence, "unknown", None)?;
            validate_receipt_sequence(&sequence, receipts)
        }
        "contract_refuted" => {
            compact_closed_object(
                disposition,
                &["kind", "contract_digest", "refutation"],
                "compact_disposition_shape_invalid",
            )?;
            let refutation =
                compact_object_field(disposition, "refutation", "compact_refutation_missing")?;
            let step_index = usize::try_from(compact_u64(
                refutation,
                "step_index",
                "compact_refutation_step_invalid",
            )?)
            .ok()
            .filter(|index| *index < steps.len())
            .ok_or_else(|| "compact_refutation_step_invalid".to_owned())?;
            match compact_string(refutation, "kind", "compact_refutation_kind_invalid")? {
                "prohibited_scope_traversal" => {
                    compact_closed_object(
                        refutation,
                        &[
                            "kind",
                            "step_index",
                            "prohibition_index",
                            "connected_receipts",
                        ],
                        "compact_refutation_shape_invalid",
                    )?;
                    compact_u64(
                        refutation,
                        "prohibition_index",
                        "compact_refutation_invalid",
                    )?;
                    let sequence =
                        compact_receipt_sequence(refutation.get("connected_receipts"), receipts)?;
                    if sequence.len() != step_index + 1 {
                        return Err("compact_refutation_sequence_invalid".to_owned());
                    }
                    validate_prefix_steps(
                        steps,
                        &sequence,
                        "unknown",
                        Some((step_index, "positive_contradiction")),
                    )?;
                    validate_receipt_sequence(&sequence, receipts)
                }
                "certified_absence" => {
                    compact_closed_object(
                        refutation,
                        &[
                            "kind",
                            "step_index",
                            "extractor_capability_receipt_id",
                            "untruncated_enumeration_receipt_id",
                            "connected_receipts",
                        ],
                        "compact_refutation_shape_invalid",
                    )?;
                    for field in [
                        "extractor_capability_receipt_id",
                        "untruncated_enumeration_receipt_id",
                    ] {
                        if compact_string(refutation, field, "compact_refutation_invalid")?
                            .is_empty()
                        {
                            return Err("compact_refutation_invalid".to_owned());
                        }
                    }
                    let sequence =
                        compact_receipt_sequence(refutation.get("connected_receipts"), receipts)?;
                    if sequence.len() != step_index {
                        return Err("compact_refutation_sequence_invalid".to_owned());
                    }
                    validate_prefix_steps(
                        steps,
                        &sequence,
                        "unknown",
                        Some((step_index, "certified_absence")),
                    )?;
                    validate_receipt_sequence(&sequence, receipts)
                }
                _ => Err("compact_refutation_kind_invalid".to_owned()),
            }
        }
        "unavailable" => {
            compact_closed_object(
                disposition,
                &["kind", "contract_digest", "reasons"],
                "compact_disposition_shape_invalid",
            )?;
            let reasons = compact_array(
                disposition.get("reasons"),
                "compact_unavailable_reasons_missing",
            )?;
            if reasons.is_empty() || reasons.iter().any(|reason| !reason.is_string()) {
                return Err("compact_unavailable_reasons_invalid".to_owned());
            }
            if steps
                .iter()
                .any(|(status, receipt)| status != "unavailable" || receipt.is_some())
            {
                return Err("compact_unavailable_step_sequence_invalid".to_owned());
            }
            Ok(())
        }
        _ => Err("compact_disposition_kind_invalid".to_owned()),
    }
}

fn compact_receipt_sequence(
    value: Option<&Value>,
    receipts: &[Value],
) -> Result<Vec<usize>, String> {
    let sequence = compact_array(value, "compact_disposition_receipts_missing")?
        .iter()
        .map(|receipt| {
            compact_index(
                receipt,
                receipts.len(),
                "compact_disposition_receipt_reference_invalid",
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if sequence.iter().collect::<BTreeSet<_>>().len() != sequence.len() {
        return Err("compact_disposition_receipts_duplicate".to_owned());
    }
    Ok(sequence)
}

fn validate_prefix_steps(
    steps: &[(String, Option<usize>)],
    sequence: &[usize],
    trailing: &str,
    terminal: Option<(usize, &str)>,
) -> Result<(), String> {
    let terminal_index = terminal.map(|(index, _)| index).unwrap_or(sequence.len());
    if sequence.len() > steps.len()
        || terminal_index >= steps.len()
        || !steps
            .iter()
            .take(terminal_index)
            .zip(sequence.iter().take(terminal_index))
            .all(|((status, receipt), index)| status == "proven" && *receipt == Some(*index))
    {
        return Err("compact_prefix_step_sequence_invalid".to_owned());
    }
    if let Some((index, status)) = terminal
        && (steps[index].0 != status
            || steps[index].1
                != if status == "certified_absence" {
                    None
                } else {
                    sequence.last().copied()
                })
    {
        return Err("compact_refutation_step_sequence_invalid".to_owned());
    }
    let suffix = terminal
        .map(|(index, _)| index + 1)
        .unwrap_or(sequence.len());
    if steps
        .iter()
        .skip(suffix)
        .any(|(status, receipt)| status != trailing || receipt.is_some())
    {
        return Err("compact_prefix_step_sequence_invalid".to_owned());
    }
    Ok(())
}

fn validate_receipt_sequence(sequence: &[usize], receipts: &[Value]) -> Result<(), String> {
    for pair in sequence.windows(2) {
        let left = compact_object(&receipts[pair[0]], "compact_receipt_row_invalid")?;
        let right = compact_object(&receipts[pair[1]], "compact_receipt_row_invalid")?;
        if left.get("target") != right.get("source") {
            return Err("compact_receipt_sequence_disconnected".to_owned());
        }
    }
    Ok(())
}

pub fn project_internal_call_path_result(
    integration: &CheckedBuiltCallPathIntegration,
) -> Result<InternalProjection, InternalProjectionError> {
    let complete = complete_projection_json(integration)?;
    Ok(InternalProjection::Complete {
        serialized_size: serialized_json_size(&complete)?,
        root: complete,
    })
}

fn complete_projection_json(
    integration: &CheckedBuiltCallPathIntegration,
) -> Result<Value, InternalProjectionError> {
    let mut tables = CompactIdentityTables::default();
    let mut receipts = Vec::with_capacity(integration.authoritative_receipts.len());
    let mut receipt_refs = BTreeMap::new();
    for receipt in &integration.authoritative_receipts {
        let receipt_index = bounded_index(receipts.len())?;
        if receipt_refs
            .insert(receipt.receipt.clone(), receipt_index)
            .is_some()
        {
            return Err(InternalProjectionError::InvalidCompactProjection(
                "duplicate_receipt_reference".to_owned(),
            ));
        }
        receipts.push(tables.receipt_json(receipt)?);
    }
    let complete = json!({
        "kind": "complete",
        "schema_version": PROOF_CONTRACT_SCHEMA_VERSION,
        "domain": PROOF_DOMAIN,
        "contract_interpretation": "host_supplied",
        "guard_version": CLAUSE_GUARD_VERSION,
        "source_text_sha256": integration.hashes.source_text_sha256,
        "contract_digest": integration.hashes.contract_digest,
        "core_publication": publication_json(&integration.built.publication),
        "identities": tables.json(),
        "spec": spec_json(&integration.contract.spec),
        "clauses": integration
            .rendering
            .normalized_clauses
            .iter()
            .map(normalized_clause_json)
            .collect::<Vec<_>>(),
        "disposition": disposition_json(&integration.disposition, &receipt_refs)?,
        "steps": step_results_json(&integration.contract, &integration.disposition, &receipt_refs)?,
        "receipts": receipts,
    });
    validate_compact_projection(&complete)
        .map_err(InternalProjectionError::InvalidCompactProjection)?;
    Ok(complete)
}

fn publication_json(publication: &InternalCorePublicationIdentity) -> Value {
    json!({
        "project_id": publication.project_id,
        "generation_id": publication.generation_id,
        "run_id": publication.run_id,
    })
}

fn disposition_json(
    disposition: &ProofDisposition,
    receipt_refs: &BTreeMap<ReceiptRef, u32>,
) -> Result<Value, InternalProjectionError> {
    match disposition {
        ProofDisposition::ContractProven {
            contract_digest,
            receipts,
        } => Ok(json!({
            "kind": "contract_proven",
            "contract_digest": contract_digest,
            "receipts": receipt_indices_json(receipts, receipt_refs)?,
        })),
        ProofDisposition::ContractRefuted {
            contract_digest,
            refutation,
        } => Ok(json!({
            "kind": "contract_refuted",
            "contract_digest": contract_digest,
            "refutation": refutation_json(refutation, receipt_refs)?,
        })),
        ProofDisposition::Unknown {
            contract_digest,
            gaps,
            connected_receipts,
        } => Ok(json!({
            "kind": "unknown",
            "contract_digest": contract_digest,
            "gaps": gaps.iter().map(proof_gap_json).collect::<Vec<_>>(),
            "connected_receipts": receipt_indices_json(connected_receipts, receipt_refs)?,
        })),
        ProofDisposition::Unavailable {
            contract_digest,
            reasons,
        } => Ok(json!({
            "kind": "unavailable",
            "contract_digest": contract_digest,
            "reasons": reasons.iter().map(unavailable_reason_name).collect::<Vec<_>>(),
        })),
    }
}

fn refutation_json(
    refutation: &Refutation,
    receipt_refs: &BTreeMap<ReceiptRef, u32>,
) -> Result<Value, InternalProjectionError> {
    match refutation {
        Refutation::ProhibitedScopeTraversal {
            step_index,
            prohibition_index,
            connected_receipts,
        } => Ok(json!({
            "kind": "prohibited_scope_traversal",
            "step_index": step_index,
            "prohibition_index": prohibition_index,
            "connected_receipts": receipt_indices_json(connected_receipts, receipt_refs)?,
        })),
        #[cfg(any(test, feature = "test-support"))]
        Refutation::CertifiedAbsence {
            step_index,
            extractor_capability_receipt_id,
            untruncated_enumeration_receipt_id,
            connected_receipts,
        } => Ok(json!({
            "kind": "certified_absence",
            "step_index": step_index,
            "extractor_capability_receipt_id": extractor_capability_receipt_id,
            "untruncated_enumeration_receipt_id": untruncated_enumeration_receipt_id,
            "connected_receipts": receipt_indices_json(connected_receipts, receipt_refs)?,
        })),
    }
}

fn proof_gap_json(gap: &ProofGap) -> Value {
    match gap {
        ProofGap::FactBuild(gap) => fact_build_gap_json(gap),
        ProofGap::MissingDirectCallReceipt { step_index } => {
            json!({ "kind": "missing_direct_call_receipt", "step_index": step_index })
        }
        ProofGap::ReceiptOrEdgeAlreadyUsed { step_index } => {
            json!({ "kind": "receipt_or_edge_already_used", "step_index": step_index })
        }
        ProofGap::ProjectionExclusionConflictsWithRequiredReceipt { step_index } => json!({
            "kind": "projection_exclusion_conflicts_with_required_receipt",
            "step_index": step_index,
        }),
    }
}

fn fact_build_gap_json(gap: &FactBuildGap) -> Value {
    match gap {
        FactBuildGap::SelectorMissing { selector_index } => {
            json!({ "kind": "selector_missing", "selector_index": selector_index })
        }
        FactBuildGap::SelectorAmbiguous { selector_index } => {
            json!({ "kind": "selector_ambiguous", "selector_index": selector_index })
        }
        FactBuildGap::NonCallableSelector { selector_index } => {
            json!({ "kind": "non_callable_selector", "selector_index": selector_index })
        }
        FactBuildGap::DirectCallMissing { step_index } => {
            json!({ "kind": "direct_call_missing", "step_index": step_index })
        }
        FactBuildGap::RecursiveCallNotRepresentable { step_index } => json!({
            "kind": "recursive_call_not_representable",
            "step_index": step_index,
        }),
        FactBuildGap::SourceWindowTooLarge { step_index } => {
            json!({ "kind": "source_window_too_large", "step_index": step_index })
        }
        FactBuildGap::InvalidUtf8 { step_index } => {
            json!({ "kind": "invalid_utf8", "step_index": step_index })
        }
        FactBuildGap::SourceLineOutOfRange { step_index } => {
            json!({ "kind": "source_line_out_of_range", "step_index": step_index })
        }
        FactBuildGap::EdgeContainmentUnproven { step_index } => {
            json!({ "kind": "edge_containment_unproven", "step_index": step_index })
        }
    }
}

fn unavailable_reason_name(reason: &UnavailableReason) -> &'static str {
    match reason {
        UnavailableReason::ValidatedContractHashMismatch => "validated_contract_hash_mismatch",
        UnavailableReason::PublicationPinMismatch => "publication_pin_mismatch",
        UnavailableReason::SourceNotBoundToPublication => "source_not_bound_to_publication",
        UnavailableReason::ProofFactsUnavailable => "proof_facts_unavailable",
        UnavailableReason::ProofSemanticProjectionUnavailable => {
            "proof_semantic_projection_unavailable"
        }
    }
}

fn step_results_json(
    contract: &ValidatedCallPathContract,
    disposition: &ProofDisposition,
    receipt_refs: &BTreeMap<ReceiptRef, u32>,
) -> Result<Vec<Value>, InternalProjectionError> {
    contract
        .spec
        .steps
        .iter()
        .enumerate()
        .map(|(step_index, _step)| {
            let (status, receipt) = match disposition {
                ProofDisposition::ContractProven { receipts, .. } => {
                    ("proven", receipts.get(step_index))
                }
                ProofDisposition::Unknown {
                    connected_receipts, ..
                } if step_index < connected_receipts.len() => {
                    ("proven", connected_receipts.get(step_index))
                }
                ProofDisposition::ContractRefuted {
                    refutation:
                        Refutation::ProhibitedScopeTraversal {
                            step_index: refutation_step,
                            connected_receipts,
                            ..
                        },
                    ..
                } if step_index < *refutation_step => {
                    ("proven", connected_receipts.get(step_index))
                }
                ProofDisposition::ContractRefuted {
                    refutation:
                        Refutation::ProhibitedScopeTraversal {
                            step_index: refutation_step,
                            connected_receipts,
                            ..
                        },
                    ..
                } if step_index == *refutation_step => {
                    ("positive_contradiction", connected_receipts.get(step_index))
                }
                #[cfg(any(test, feature = "test-support"))]
                ProofDisposition::ContractRefuted {
                    refutation:
                        Refutation::CertifiedAbsence {
                            step_index: absence_step,
                            connected_receipts,
                            ..
                        },
                    ..
                } if step_index < *absence_step => ("proven", connected_receipts.get(step_index)),
                #[cfg(any(test, feature = "test-support"))]
                ProofDisposition::ContractRefuted {
                    refutation:
                        Refutation::CertifiedAbsence {
                            step_index: absence_step,
                            ..
                        },
                    ..
                } if step_index == *absence_step => ("certified_absence", None),
                ProofDisposition::Unavailable { .. } => ("unavailable", None),
                _ => ("unknown", None),
            };
            Ok(json!({
                "step_index": step_index,
                "status": status,
                "receipt": receipt.map(|receipt| receipt_index_json(receipt, receipt_refs)).transpose()?,
            }))
        })
        .collect::<Result<Vec<_>, _>>()
}

fn receipt_indices_json(
    receipts: &[ReceiptRef],
    receipt_refs: &BTreeMap<ReceiptRef, u32>,
) -> Result<Vec<Value>, InternalProjectionError> {
    receipts
        .iter()
        .map(|receipt| receipt_index_json(receipt, receipt_refs))
        .collect()
}

fn receipt_index_json(
    receipt: &ReceiptRef,
    receipt_refs: &BTreeMap<ReceiptRef, u32>,
) -> Result<Value, InternalProjectionError> {
    receipt_refs
        .get(receipt)
        .copied()
        .map(Value::from)
        .ok_or_else(|| {
            InternalProjectionError::InvalidCompactProjection(
                "dangling_receipt_reference".to_owned(),
            )
        })
}

fn serialized_json_size(value: &Value) -> Result<usize, InternalProjectionError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|error| InternalProjectionError::Serialization(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, NodeId};
    use codestory_contracts::proof_resolution::{FileId, ResolutionEvidence, ResolutionProvenance};

    fn canonical_selector(name: &str) -> UnvalidatedExactSymbolSelector {
        UnvalidatedExactSymbolSelector::CanonicalId(name.to_owned())
    }

    fn canonical_scope(name: &str) -> UnvalidatedExactScopeSelector {
        UnvalidatedExactScopeSelector::CanonicalId(name.to_owned())
    }

    fn spec(targets: &[&str]) -> UnvalidatedCallPathSpec {
        UnvalidatedCallPathSpec {
            start: canonical_selector("A"),
            steps: targets
                .iter()
                .map(|target| UnvalidatedDirectCallStep {
                    target: canonical_selector(target),
                })
                .collect(),
            prohibit_traversal_through: Vec::new(),
            exclude_from_projection: Vec::new(),
        }
    }

    fn start_anchor(source: &str) -> ClauseAnchor {
        ClauseAnchor {
            clause_id: "whole".to_owned(),
            start: 0,
            end: source.len(),
            quote: source.to_owned(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::Start],
            },
        }
    }

    fn valid_input(targets: &[&str]) -> UnvalidatedCallPathContract {
        let source = "exact direct ordered call path";
        let mut clauses = vec![start_anchor(source)];
        for (index, _) in targets.iter().enumerate() {
            let step = u8::try_from(index).expect("test step index fits u8");
            clauses.push(ClauseAnchor {
                clause_id: format!("target-{index}"),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::StepTarget { step },
                        ProofContractField::Directness { step },
                        ProofContractField::Ordering { step },
                        ProofContractField::Relation { step },
                    ],
                },
            });
        }
        UnvalidatedCallPathContract::new(source, clauses, spec(targets))
    }

    fn validate(targets: &[&str]) -> (ValidatedCallPathContract, ProofHashes) {
        match validate_contract(valid_input(targets)).expect("valid contract") {
            ValidationOutcome::Validated {
                contract, hashes, ..
            } => (*contract, hashes),
            other => panic!("expected validated contract, got {other:?}"),
        }
    }

    fn validate_for_projection(
        targets: &[&str],
    ) -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
    ) {
        match validate_contract(valid_input(targets)).expect("valid contract") {
            ValidationOutcome::Validated {
                contract,
                hashes,
                rendering,
            } => (*contract, hashes, rendering),
            other => panic!("expected validated contract, got {other:?}"),
        }
    }

    fn node(name: &str) -> ResolvedNodeIdentity {
        let file_node_id = i64::from(
            name.bytes()
                .next()
                .expect("fixture names are nonempty")
                .saturating_sub(b'A')
                + 1,
        );
        let node_id = (file_node_id * 10).to_string();
        ResolvedNodeIdentity::new(
            PinnedNodeIdentity {
                project_id: "project".to_owned(),
                core_generation_id: "generation".to_owned(),
                core_run_id: "run".to_owned(),
                node_id: node_id.clone(),
            },
            name,
            format!("crate::{name}"),
            NodeId(file_node_id),
            vec!["src".to_owned(), format!("{name}.rs")],
        )
        .expect("valid node")
    }

    fn call(receipt: &str, edge: &str, source: &str, target: &str) -> VerifiedProofFact {
        VerifiedProofFact::DirectCall(VerifiedDirectCallFact {
            receipt: ReceiptRef {
                receipt_id: receipt.to_owned(),
                edge_id: edge.to_owned(),
            },
            source: node(source),
            target: node(target),
        })
    }

    fn indexed_receipt(
        index: usize,
        source: &str,
        target: &str,
        text: String,
    ) -> IndexedCallEdgeReceipt {
        IndexedCallEdgeReceipt {
            receipt: ReceiptRef {
                receipt_id: format!("receipt-{index}"),
                edge_id: format!("edge-{index}"),
            },
            source: node(source),
            target: node(target),
            resolution_fact_id: format!("{:064x}", index + 1),
            resolution_evidence_sha256: format!("{:064x}", index + 2),
            resolution_evidence_chain: vec![ResolutionEvidence::SameFileDeclaration {
                declaration: NodeId(node(target).pinned.node_id.parse().unwrap()),
            }],
            resolution_provenance: ResolutionProvenance {
                producer: "codestory-internal".to_owned(),
                fact_schema_version: 1,
                algorithm: "exact-call-resolution-v1".to_owned(),
                language_adapter: "rust".to_owned(),
                language_adapter_version: "test-v1".to_owned(),
                parser_fingerprint: "f".repeat(64),
                dependency_file_hashes: vec![
                    codestory_contracts::proof_resolution::DependencyFileHash {
                        file_id: FileId(node(source).file_node_id.0),
                        source_sha256: format!("{:064x}", index + 3),
                    },
                    codestory_contracts::proof_resolution::DependencyFileHash {
                        file_id: FileId(node(target).file_node_id.0),
                        source_sha256: format!("{:064x}", index + 4),
                    },
                ],
                evidence_sha256: format!("{:064x}", index + 2),
            },
            exact_callsite_start_byte: (index * 10) as u64,
            callsite_identity: format!(
                "{}:{}:1:{}|rust",
                node(source).file_node_id.0,
                index + 1,
                node(target).pinned.node_id
            ),
            column_or_ordinal: 1,
            containment: CallableContainmentEvidence {
                file_node_id: node(source).file_node_id,
                owner_node_id: NodeId(node(source).pinned.node_id.parse().unwrap()),
                start_line: u32::try_from(index + 1).unwrap(),
                end_line: u32::try_from(index + 1).unwrap(),
            },
            line_window: IndexedLineWindow {
                kind: "indexed_line_v1",
                project_file_components: vec!["src".to_owned(), format!("{source}.rs")],
                indexed_sha256: format!("{:064x}", index + 3),
                observed_sha256: format!("{:064x}", index + 3),
                anchor_line: u32::try_from(index + 1).unwrap(),
                byte_start: index * 10,
                byte_end: index * 10 + text.len(),
                text,
            },
        }
    }

    fn publication() -> InternalCorePublicationIdentity {
        InternalCorePublicationIdentity {
            project_id: "project".to_owned(),
            generation_id: "generation".to_owned(),
            run_id: "run".to_owned(),
        }
    }

    fn built_from_receipts(
        receipts: Vec<IndexedCallEdgeReceipt>,
        gaps: Vec<FactBuildGap>,
        unavailable: Vec<UnavailableReason>,
    ) -> BuiltCallPathFacts {
        let facts = receipts
            .iter()
            .map(|receipt| {
                VerifiedProofFact::DirectCall(VerifiedDirectCallFact {
                    receipt: receipt.receipt.clone(),
                    source: receipt.source.clone(),
                    target: receipt.target.clone(),
                })
            })
            .collect();
        BuiltCallPathFacts {
            publication: publication(),
            facts,
            receipts,
            gaps,
            unavailable,
        }
    }

    fn validated_with_policies(
        targets: &[&str],
        prohibitions: &[&str],
        exclusions: &[&str],
    ) -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
    ) {
        let mut input = valid_input(targets);
        input.spec.prohibit_traversal_through = prohibitions
            .iter()
            .map(|scope| canonical_scope(scope))
            .collect();
        input.spec.exclude_from_projection = exclusions
            .iter()
            .map(|scope| canonical_scope(scope))
            .collect();
        for (index, _) in prohibitions.iter().enumerate() {
            input.clauses.push(ClauseAnchor {
                clause_id: format!("prohibition-{index}"),
                start: 0,
                end: input.source_text.len(),
                quote: input.source_text.clone(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![ProofContractField::TraversalProhibition {
                        index: u8::try_from(index).unwrap(),
                    }],
                },
            });
        }
        for (index, _) in exclusions.iter().enumerate() {
            input.clauses.push(ClauseAnchor {
                clause_id: format!("exclusion-{index}"),
                start: 0,
                end: input.source_text.len(),
                quote: input.source_text.clone(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![ProofContractField::ProjectionExclusion {
                        index: u8::try_from(index).unwrap(),
                    }],
                },
            });
        }
        match validate_contract(input).unwrap() {
            ValidationOutcome::Validated {
                contract,
                hashes,
                rendering,
            } => (*contract, hashes, rendering),
            other => panic!("expected validated contract, got {other:?}"),
        }
    }

    fn checked_integration(
        contract: &ValidatedCallPathContract,
        hashes: &ProofHashes,
        rendering: &ValidatedContractRendering,
        built: BuiltCallPathFacts,
    ) -> CheckedBuiltCallPathIntegration {
        check_built_call_path_integration(contract, hashes, rendering, built).unwrap()
    }

    #[test]
    fn checked_integration_preserves_all_builder_gaps_exactly_and_unavailable_wins() {
        let (contract, hashes, rendering) = validate_for_projection(&["B"]);
        let receipt = indexed_receipt(0, "A", "B", "A calls B();\n".to_owned());
        let all_gaps = vec![
            FactBuildGap::EdgeContainmentUnproven { step_index: 0 },
            FactBuildGap::SelectorMissing { selector_index: 0 },
            FactBuildGap::InvalidUtf8 { step_index: 0 },
            FactBuildGap::SelectorAmbiguous { selector_index: 1 },
            FactBuildGap::DirectCallMissing { step_index: 0 },
            FactBuildGap::NonCallableSelector { selector_index: 1 },
            FactBuildGap::SourceLineOutOfRange { step_index: 0 },
            FactBuildGap::RecursiveCallNotRepresentable { step_index: 0 },
            FactBuildGap::SourceWindowTooLarge { step_index: 0 },
            FactBuildGap::DirectCallMissing { step_index: 0 },
        ];
        let integrated = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(vec![receipt.clone()], all_gaps.clone(), Vec::new()),
        );
        assert_eq!(
            integrated.disposition(),
            &ProofDisposition::Unknown {
                contract_digest: hashes.contract_digest().to_owned(),
                connected_receipts: Vec::new(),
                gaps: vec![
                    ProofGap::FactBuild(FactBuildGap::SelectorMissing { selector_index: 0 }),
                    ProofGap::FactBuild(FactBuildGap::SelectorAmbiguous { selector_index: 1 }),
                    ProofGap::FactBuild(FactBuildGap::NonCallableSelector { selector_index: 1 }),
                    ProofGap::FactBuild(FactBuildGap::DirectCallMissing { step_index: 0 }),
                    ProofGap::FactBuild(FactBuildGap::RecursiveCallNotRepresentable {
                        step_index: 0,
                    }),
                    ProofGap::FactBuild(FactBuildGap::SourceWindowTooLarge { step_index: 0 }),
                    ProofGap::FactBuild(FactBuildGap::InvalidUtf8 { step_index: 0 }),
                    ProofGap::FactBuild(FactBuildGap::SourceLineOutOfRange { step_index: 0 }),
                    ProofGap::FactBuild(FactBuildGap::EdgeContainmentUnproven { step_index: 0 }),
                ],
            }
        );

        let unavailable = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(
                vec![receipt],
                all_gaps,
                vec![UnavailableReason::SourceNotBoundToPublication],
            ),
        );
        assert_eq!(
            unavailable.disposition(),
            &ProofDisposition::Unavailable {
                contract_digest: hashes.contract_digest().to_owned(),
                reasons: vec![UnavailableReason::SourceNotBoundToPublication],
            }
        );
        assert!(unavailable.authoritative_receipts().is_empty());
    }

    #[test]
    fn unknown_projection_keeps_the_longest_clean_prefix_and_exact_gap_only() {
        let (contract, hashes, rendering) = validate_for_projection(&["B", "C", "D"]);
        let r0 = indexed_receipt(0, "A", "B", "A calls B();\n".to_owned());
        let r1 = indexed_receipt(1, "B", "C", "B calls C();\n".to_owned());
        let unused = indexed_receipt(9, "X", "Y", "X calls Y();\n".to_owned());
        let integrated = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(
                vec![r0.clone(), r1.clone(), unused],
                vec![FactBuildGap::DirectCallMissing { step_index: 2 }],
                Vec::new(),
            ),
        );
        assert_eq!(
            integrated.disposition(),
            &ProofDisposition::Unknown {
                contract_digest: hashes.contract_digest().to_owned(),
                connected_receipts: vec![r0.receipt.clone(), r1.receipt.clone()],
                gaps: vec![ProofGap::FactBuild(FactBuildGap::DirectCallMissing {
                    step_index: 2,
                })],
            }
        );
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integrated).unwrap()
        else {
            panic!("small checked integration fits")
        };
        assert_eq!(
            root["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|step| step["status"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["proven", "proven", "unknown"]
        );
        assert_eq!(
            root["disposition"]["gaps"],
            json!([{ "kind": "direct_call_missing", "step_index": 2 }])
        );
        assert_eq!(
            root["receipts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|receipt| receipt["receipt_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["receipt-0", "receipt-1"]
        );
    }

    #[test]
    fn unknown_prefix_ties_choose_the_smallest_receipt_sequence() {
        let (contract, hashes) = validate(&["B", "C"]);
        let facts = [
            call("receipt-b", "edge-a", "A", "B"),
            call("receipt-a", "edge-z", "A", "B"),
        ];
        assert_eq!(
            check_call_path(&contract, &hashes, &facts),
            ProofDisposition::Unknown {
                contract_digest: hashes.contract_digest().to_owned(),
                gaps: vec![ProofGap::MissingDirectCallReceipt { step_index: 1 }],
                connected_receipts: vec![ReceiptRef {
                    receipt_id: "receipt-a".to_owned(),
                    edge_id: "edge-z".to_owned(),
                }],
            }
        );
    }

    #[test]
    fn authoritative_receipts_follow_authenticated_source_order() {
        let (contract, hashes, rendering) = validate_for_projection(&["B"]);
        let mut earlier_file = indexed_receipt(0, "A", "B", "first();\n".to_owned());
        earlier_file.receipt = ReceiptRef {
            receipt_id: "receipt-z-file".to_owned(),
            edge_id: "200".to_owned(),
        };
        earlier_file.line_window.project_file_components =
            vec!["src".to_owned(), "a.rs".to_owned()];
        earlier_file.containment.file_node_id = NodeId(99);
        earlier_file.exact_callsite_start_byte = 40;

        let mut later_file = indexed_receipt(1, "A", "B", "second();\n".to_owned());
        later_file.receipt = ReceiptRef {
            receipt_id: "receipt-a-file".to_owned(),
            edge_id: "100".to_owned(),
        };
        later_file.line_window.project_file_components = vec!["src".to_owned(), "z.rs".to_owned()];
        later_file.containment.file_node_id = NodeId(1);
        later_file.exact_callsite_start_byte = 40;

        let integrated = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(
                vec![later_file, earlier_file.clone()],
                Vec::new(),
                Vec::new(),
            ),
        );
        assert_eq!(
            integrated.authoritative_receipts(),
            [earlier_file],
            "native-bound project file identity must outrank graph file and edge identifiers"
        );

        let mut earlier_byte = indexed_receipt(2, "A", "B", "earlier();\n".to_owned());
        earlier_byte.receipt = ReceiptRef {
            receipt_id: "receipt-z-byte".to_owned(),
            edge_id: "200".to_owned(),
        };
        earlier_byte.line_window.project_file_components =
            vec!["src".to_owned(), "same.rs".to_owned()];
        earlier_byte.containment.file_node_id = NodeId(99);
        earlier_byte.exact_callsite_start_byte = 20;
        let mut later_byte = indexed_receipt(3, "A", "B", "later();\n".to_owned());
        later_byte.receipt = ReceiptRef {
            receipt_id: "receipt-a-byte".to_owned(),
            edge_id: "100".to_owned(),
        };
        later_byte.line_window.project_file_components =
            earlier_byte.line_window.project_file_components.clone();
        later_byte.containment.file_node_id = NodeId(1);
        later_byte.exact_callsite_start_byte = 40;
        let integrated = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(
                vec![later_byte, earlier_byte.clone()],
                Vec::new(),
                Vec::new(),
            ),
        );
        assert_eq!(integrated.authoritative_receipts(), [earlier_byte]);

        let mut edge_two = indexed_receipt(4, "A", "B", "edge_two();\n".to_owned());
        edge_two.receipt = ReceiptRef {
            receipt_id: "receipt-z-edge".to_owned(),
            edge_id: "2".to_owned(),
        };
        edge_two.line_window.project_file_components = vec!["src".to_owned(), "same.rs".to_owned()];
        edge_two.containment.file_node_id = NodeId(7);
        edge_two.exact_callsite_start_byte = 40;
        let mut edge_ten = indexed_receipt(5, "A", "B", "edge_ten();\n".to_owned());
        edge_ten.receipt = ReceiptRef {
            receipt_id: "receipt-a-edge".to_owned(),
            edge_id: "10".to_owned(),
        };
        edge_ten.line_window.project_file_components =
            edge_two.line_window.project_file_components.clone();
        edge_ten.containment.file_node_id = NodeId(7);
        edge_ten.exact_callsite_start_byte = 40;
        let integrated = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(vec![edge_ten, edge_two.clone()], Vec::new(), Vec::new()),
        );
        assert_eq!(integrated.authoritative_receipts(), [edge_two]);
    }

    #[test]
    fn prohibited_projection_marks_only_the_refuting_step_and_drops_later_candidates() {
        let (contract, hashes, rendering) = validated_with_policies(&["B", "C", "D"], &["C"], &[]);
        let r0 = indexed_receipt(0, "A", "B", "A calls B();\n".to_owned());
        let r1 = indexed_receipt(1, "B", "C", "B calls C();\n".to_owned());
        let later = indexed_receipt(2, "C", "D", "C calls D();\n".to_owned());
        let integrated = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(vec![r0, r1, later], Vec::new(), Vec::new()),
        );
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integrated).unwrap()
        else {
            panic!("small checked integration fits")
        };
        assert_eq!(
            root["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|step| step["status"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["proven", "positive_contradiction", "unknown"]
        );
        assert_eq!(
            root["receipts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|receipt| receipt["receipt_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["receipt-0", "receipt-1"]
        );
    }

    #[test]
    fn certified_absence_projection_keeps_only_the_proven_prefix_receipts() {
        let (contract, hashes, rendering) = validate_for_projection(&["B", "C", "D"]);
        let r0 = indexed_receipt(0, "A", "B", "A calls B();\n".to_owned());
        let r1 = indexed_receipt(1, "B", "C", "B calls C();\n".to_owned());
        let unused = indexed_receipt(9, "X", "Y", "X calls Y();\n".to_owned());
        let mut built = built_from_receipts(vec![r0, r1, unused], Vec::new(), Vec::new());
        built
            .facts
            .push(VerifiedProofFact::CertifiedAbsence(CertifiedAbsenceFact {
                source: node("C"),
                expected_target: ExactSymbolSelector::CanonicalId("D".to_owned()),
                extractor_capability_receipt_id: "capability".to_owned(),
                untruncated_enumeration_receipt_id: "enumeration".to_owned(),
            }));
        let integrated = checked_integration(&contract, &hashes, &rendering, built);
        assert!(matches!(
            integrated.disposition(),
            ProofDisposition::ContractRefuted {
                refutation: Refutation::CertifiedAbsence {
                    step_index: 2,
                    connected_receipts,
                    ..
                },
                ..
            } if connected_receipts.iter().map(|receipt| receipt.receipt_id.as_str()).collect::<Vec<_>>() == ["receipt-0", "receipt-1"]
        ));
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integrated).unwrap()
        else {
            panic!("small checked integration fits")
        };
        assert_eq!(
            root["steps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|step| step["status"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["proven", "proven", "certified_absence"]
        );
        assert_eq!(
            root["receipts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|receipt| receipt["receipt_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["receipt-0", "receipt-1"]
        );
    }

    #[test]
    fn checked_integration_rejects_every_mixable_projection_binding() {
        type CheckedBoundary =
            fn(
                &ValidatedCallPathContract,
                &ProofHashes,
                &ValidatedContractRendering,
                BuiltCallPathFacts,
            ) -> Result<CheckedBuiltCallPathIntegration, CheckedIntegrationError>;
        type ProjectionBoundary = fn(
            &CheckedBuiltCallPathIntegration,
        )
            -> Result<InternalProjection, InternalProjectionError>;
        let _checked_boundary: CheckedBoundary = check_built_call_path_integration;
        let _projection_boundary: ProjectionBoundary = project_internal_call_path_result;

        let (contract, hashes, rendering) = validate_for_projection(&["B"]);
        let receipt = indexed_receipt(0, "A", "B", "A calls B();\n".to_owned());

        let mut foreign_input = valid_input(&["B"]);
        foreign_input.source_text.push('!');
        for clause in &mut foreign_input.clauses {
            clause.end += 1;
            clause.quote.push('!');
        }
        let foreign_rendering = match validate_contract(foreign_input).unwrap() {
            ValidationOutcome::Validated { rendering, .. } => rendering,
            other => panic!("expected validated foreign rendering, got {other:?}"),
        };
        assert!(matches!(
            check_built_call_path_integration(
                &contract,
                &hashes,
                &foreign_rendering,
                built_from_receipts(vec![receipt.clone()], Vec::new(), Vec::new()),
            ),
            Err(CheckedIntegrationError::ContractDigestMismatch)
        ));

        let mut forged_hashes = hashes.clone();
        forged_hashes.contract_digest = "forged".to_owned();
        assert!(matches!(
            check_built_call_path_integration(
                &contract,
                &forged_hashes,
                &rendering,
                built_from_receipts(vec![receipt.clone()], Vec::new(), Vec::new()),
            ),
            Err(CheckedIntegrationError::ValidatedContractHashMismatch)
        ));

        let mut wrong_publication =
            built_from_receipts(vec![receipt.clone()], Vec::new(), Vec::new());
        wrong_publication.publication.run_id = "other-run".to_owned();
        assert!(matches!(
            check_built_call_path_integration(&contract, &hashes, &rendering, wrong_publication,),
            Err(CheckedIntegrationError::PublicationBindingMismatch)
        ));

        let mut mismatched_receipt =
            built_from_receipts(vec![receipt.clone()], Vec::new(), Vec::new());
        mismatched_receipt.receipts[0].receipt.edge_id = "other-edge".to_owned();
        assert!(matches!(
            check_built_call_path_integration(&contract, &hashes, &rendering, mismatched_receipt,),
            Err(CheckedIntegrationError::FactReceiptMismatch)
        ));

        let mut duplicate = built_from_receipts(vec![receipt], Vec::new(), Vec::new());
        duplicate.receipts.push(duplicate.receipts[0].clone());
        assert!(matches!(
            check_built_call_path_integration(&contract, &hashes, &rendering, duplicate),
            Err(CheckedIntegrationError::DuplicateFactReceipt)
        ));
    }

    fn six_step_projection_fixture(
        texts: Vec<String>,
    ) -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
        Vec<IndexedCallEdgeReceipt>,
    ) {
        assert_eq!(texts.len(), 6);
        let names = ["A", "B", "C", "D", "E", "F", "G"];
        let (contract, hashes, rendering) = validate_for_projection(&names[1..]);
        let receipts = texts
            .into_iter()
            .enumerate()
            .map(|(index, text)| indexed_receipt(index, names[index], names[index + 1], text))
            .collect::<Vec<_>>();
        (contract, hashes, rendering, receipts)
    }

    #[test]
    fn internal_projection_is_a_complete_tagged_root_with_every_authoritative_receipt() {
        let (contract, hashes, rendering) = validate_for_projection(&["B"]);
        let receipts = vec![indexed_receipt(0, "A", "B", "A calls B();\n".to_owned())];
        let integration = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(receipts, Vec::new(), Vec::new()),
        );
        let projected = project_internal_call_path_result(&integration).unwrap();

        let InternalProjection::Complete {
            root,
            serialized_size,
        } = projected
        else {
            panic!("small complete proof must fit")
        };
        assert_eq!(root["kind"], "complete");
        assert_eq!(root["schema_version"], 1);
        assert_eq!(root["domain"], "indexed_source_call_path_v1");
        assert_eq!(root["contract_interpretation"], "host_supplied");
        assert_eq!(root["guard_version"], "clause_guard_v1");
        assert_eq!(root["identities"]["files"].as_array().unwrap().len(), 2);
        assert_eq!(root["identities"]["symbols"].as_array().unwrap().len(), 2);
        assert_eq!(root["identities"]["evidence"].as_array().unwrap().len(), 1);
        assert_eq!(root["receipts"].as_array().unwrap().len(), 1);
        assert_eq!(root["receipts"][0]["source"], 0);
        assert_eq!(root["receipts"][0]["target"], 1);
        assert_eq!(root["receipts"][0]["evidence"], 0);
        assert_eq!(root["steps"][0]["receipt"], 0);
        assert_eq!(root["disposition"]["receipts"], json!([0]));
        assert_eq!(root["steps"].as_array().unwrap().len(), 1);
        assert_eq!(serialized_size, serde_json::to_vec(&root).unwrap().len());
    }

    #[test]
    fn internal_projection_interns_shared_identities_in_authoritative_receipt_order() {
        let (contract, hashes, rendering) = validate_for_projection(&["B", "C"]);
        let first = indexed_receipt(0, "A", "B", "first\n".to_owned());
        let second = indexed_receipt(1, "B", "C", "second\n".to_owned());
        let integration = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(vec![first, second], Vec::new(), Vec::new()),
        );

        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integration).unwrap()
        else {
            panic!("compact projection should fit");
        };
        assert_eq!(root["identities"]["symbols"].as_array().unwrap().len(), 3);
        assert_eq!(root["identities"]["evidence"].as_array().unwrap().len(), 2);
        assert_eq!(root["receipts"][0]["source"], 0);
        assert_eq!(root["receipts"][0]["target"], 1);
        assert_eq!(root["receipts"][1]["source"], 1);
        assert_ne!(root["receipts"][1]["target"], root["receipts"][1]["source"]);
        assert_eq!(root["disposition"]["receipts"], json!([0, 1]));
        assert_eq!(root["steps"][0]["receipt"], 0);
        assert_eq!(root["steps"][1]["receipt"], 1);
    }

    #[test]
    fn compact_projection_rejects_dangling_swapped_and_incomplete_evidence_references() {
        let (contract, hashes, rendering) = validate_for_projection(&["B"]);
        let receipt = indexed_receipt(0, "A", "B", "call\n".to_owned());
        let integration = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(vec![receipt], Vec::new(), Vec::new()),
        );
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integration).unwrap()
        else {
            panic!("compact projection should fit");
        };

        let mut dangling = root.clone();
        dangling["receipts"][0]["evidence"] = json!(99);
        assert_eq!(
            validate_compact_projection(&dangling),
            Err("compact_receipt_evidence_reference_invalid".to_owned())
        );

        let mut swapped = root.clone();
        swapped["receipts"][0]["source"] = json!(1);
        swapped["receipts"][0]["target"] = json!(0);
        assert_eq!(
            validate_compact_projection(&swapped),
            Err("compact_receipt_evidence_correlation_invalid".to_owned())
        );

        let mut missing_provenance = root;
        missing_provenance["identities"]["evidence"][0]
            .as_object_mut()
            .unwrap()
            .remove("provenance");
        assert_eq!(
            validate_compact_projection(&missing_provenance),
            Err("compact_evidence_shape_invalid".to_owned())
        );

        let mut invalid_receipt = indexed_receipt(1, "A", "B", "call\n".to_owned());
        invalid_receipt.resolution_provenance.producer.clear();
        assert_eq!(
            check_built_call_path_integration(
                &contract,
                &hashes,
                &rendering,
                built_from_receipts(vec![invalid_receipt], Vec::new(), Vec::new()),
            ),
            Err(CheckedIntegrationError::PublicationBindingMismatch)
        );
    }

    #[test]
    fn compact_projection_rejects_relational_identity_provenance_and_disposition_mutations() {
        let (contract, hashes, rendering) = validate_for_projection(&["B", "C"]);
        let integration = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(
                vec![
                    indexed_receipt(0, "A", "B", "A calls B();\n".to_owned()),
                    indexed_receipt(1, "B", "C", "B calls C();\n".to_owned()),
                ],
                Vec::new(),
                Vec::new(),
            ),
        );
        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integration).unwrap()
        else {
            panic!("small checked integration fits")
        };

        let mut mutations = Vec::new();

        let mut swapped_steps = root.clone();
        swapped_steps["steps"][0]["receipt"] = json!(1);
        swapped_steps["steps"][1]["receipt"] = json!(0);
        mutations.push(swapped_steps);

        let mut wrong_owner = root.clone();
        wrong_owner["receipts"][0]["containment"]["owner"] = json!(1);
        mutations.push(wrong_owner);

        let mut duplicate_receipt = root.clone();
        duplicate_receipt["receipts"][1]["receipt_id"] =
            duplicate_receipt["receipts"][0]["receipt_id"].clone();
        mutations.push(duplicate_receipt);

        let mut duplicate_symbol = root.clone();
        duplicate_symbol["identities"]["symbols"][1]["node_id"] =
            duplicate_symbol["identities"]["symbols"][0]["node_id"].clone();
        mutations.push(duplicate_symbol);

        let mut empty_dependencies = root.clone();
        empty_dependencies["identities"]["evidence"][0]["provenance"]["dependency_files"] =
            json!([]);
        mutations.push(empty_dependencies);

        let mut invalid_chain_arity = root.clone();
        invalid_chain_arity["identities"]["evidence"][0]["chain"][0]["symbols"] = json!([]);
        mutations.push(invalid_chain_arity);

        let mut swapped_callsite_evidence = root.clone();
        swapped_callsite_evidence["identities"]["evidence"][0]["callsite_identity"] =
            json!("different-callsite");
        mutations.push(swapped_callsite_evidence);

        let mut unknown_prefix = root.clone();
        unknown_prefix["disposition"] = json!({
            "kind":"unknown",
            "contract_digest": hashes.contract_digest(),
            "gaps": [{"kind":"direct_call_missing","step_index":1}],
            "connected_receipts":[0]
        });
        unknown_prefix["steps"] = json!([
            {"step_index":0,"status":"proven","receipt":0},
            {"step_index":1,"status":"unknown","receipt":1}
        ]);
        mutations.push(unknown_prefix);

        let mut positive_contradiction = root.clone();
        positive_contradiction["disposition"] = json!({
            "kind":"contract_refuted",
            "contract_digest": hashes.contract_digest(),
            "refutation": {
                "kind":"prohibited_scope_traversal",
                "step_index":1,
                "prohibition_index":0,
                "connected_receipts":[0, 1]
            }
        });
        positive_contradiction["steps"] = json!([
            {"step_index":0,"status":"proven","receipt":0},
            {"step_index":1,"status":"positive_contradiction","receipt":0}
        ]);
        mutations.push(positive_contradiction);

        let mut certified_absence = root.clone();
        certified_absence["disposition"] = json!({
            "kind":"contract_refuted",
            "contract_digest": hashes.contract_digest(),
            "refutation": {
                "kind":"certified_absence",
                "step_index":1,
                "extractor_capability_receipt_id":"extractor:1",
                "untruncated_enumeration_receipt_id":"enumeration:1",
                "connected_receipts":[0]
            }
        });
        certified_absence["steps"] = json!([
            {"step_index":0,"status":"proven","receipt":0},
            {"step_index":1,"status":"certified_absence","receipt":1}
        ]);
        mutations.push(certified_absence);

        let mut unavailable = root;
        unavailable["disposition"] = json!({
            "kind":"unavailable",
            "contract_digest": hashes.contract_digest(),
            "reasons":["source_not_bound_to_publication"]
        });
        unavailable["steps"] = json!([
            {"step_index":0,"status":"unavailable","receipt":0},
            {"step_index":1,"status":"unavailable","receipt":null}
        ]);
        mutations.push(unavailable);

        for mutation in mutations {
            assert!(
                validate_compact_projection(&mutation).is_err(),
                "validator accepted relational mutation: {mutation}"
            );
        }
    }

    #[test]
    fn compact_projection_merges_cross_file_dependencies_before_receipt_indices_freeze() {
        let (contract, hashes, rendering) = validate_for_projection(&["B", "C"]);
        let first = indexed_receipt(0, "A", "B", "A calls B();\n".to_owned());
        let second = indexed_receipt(1, "B", "C", "B calls C();\n".to_owned());
        let integration = checked_integration(
            &contract,
            &hashes,
            &rendering,
            built_from_receipts(vec![first, second], Vec::new(), Vec::new()),
        );

        let InternalProjection::Complete { root, .. } =
            project_internal_call_path_result(&integration).expect("cross-file projection")
        else {
            panic!("cross-file projection must remain complete")
        };
        assert_eq!(root["identities"]["files"].as_array().unwrap().len(), 3);
        assert_eq!(root["receipts"][0]["target"], root["receipts"][1]["source"]);
    }

    #[test]
    fn validates_a_fully_anchored_one_step_direct_call_contract() {
        assert!(matches!(
            validate_contract(valid_input(&["B"])),
            Ok(ValidationOutcome::Validated { .. })
        ));
    }

    #[test]
    fn raw_call_edge_admission_requires_the_persisted_canonical_identity() {
        let edge = Edge {
            id: EdgeId(41),
            source: NodeId(7),
            target: NodeId(19),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(-3)),
            line: Some(12),
            resolved_source: Some(NodeId(11)),
            resolved_target: Some(NodeId(23)),
            callsite_identity: Some("-3:12:0:19|collector-marker".to_owned()),
            candidate_targets: Vec::new(),
            ..Default::default()
        };

        assert_eq!(
            admit_raw_call_edge(&edge, NodeId(11), NodeId(23)),
            RawCallEdgeAdmission::Admitted(AdmittedRawCallEdge {
                edge_id: EdgeId(41),
                file_node_id: NodeId(-3),
                line: 12,
                column_or_ordinal: 0,
                raw_target: NodeId(19),
                callsite_identity: "-3:12:0:19|collector-marker".to_owned(),
            })
        );
    }

    #[test]
    fn raw_admission_diagnostics_share_the_product_leaf() {
        type HostileCase = (
            &'static str,
            RawAdmissionFailure,
            Box<dyn Fn(&mut Edge, &mut NodeId, &mut NodeId)>,
        );

        let lawful = Edge {
            id: EdgeId(41),
            source: NodeId(7),
            target: NodeId(19),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(-3)),
            line: Some(12),
            resolved_source: Some(NodeId(11)),
            resolved_target: Some(NodeId(23)),
            callsite_identity: Some("-3:12:0:19|collector-marker".to_owned()),
            candidate_targets: Vec::new(),
            ..Default::default()
        };
        let admitted = AdmittedRawCallEdge {
            edge_id: EdgeId(41),
            file_node_id: NodeId(-3),
            line: 12,
            column_or_ordinal: 0,
            raw_target: NodeId(19),
            callsite_identity: "-3:12:0:19|collector-marker".to_owned(),
        };
        assert_eq!(
            diagnose_raw_call_edge(&lawful, NodeId(11), NodeId(23)),
            Ok(admitted.clone())
        );
        assert_eq!(
            admit_raw_call_edge(&lawful, NodeId(11), NodeId(23)),
            RawCallEdgeAdmission::Admitted(admitted)
        );

        let cases: Vec<HostileCase> = vec![
            (
                "wrong kind",
                RawAdmissionFailure::WrongKind,
                Box::new(|edge, _, _| edge.kind = EdgeKind::USAGE),
            ),
            (
                "wrong effective source",
                RawAdmissionFailure::WrongEffectiveSource,
                Box::new(|edge, _, _| edge.resolved_source = Some(NodeId(12))),
            ),
            (
                "wrong effective target",
                RawAdmissionFailure::WrongEffectiveTarget,
                Box::new(|edge, _, _| edge.resolved_target = Some(NodeId(24))),
            ),
            (
                "missing exact resolved target",
                RawAdmissionFailure::MissingExactResolvedTarget,
                Box::new(|edge, _, _| {
                    edge.target = NodeId(23);
                    edge.resolved_target = None;
                    edge.callsite_identity = Some("-3:12:0:23|collector-marker".to_owned());
                }),
            ),
            (
                "candidate alternatives retained",
                RawAdmissionFailure::CandidateAlternativesRetained,
                Box::new(|edge, _, _| edge.candidate_targets = vec![NodeId(24)]),
            ),
            (
                "missing file node",
                RawAdmissionFailure::MissingFileNode,
                Box::new(|edge, _, _| edge.file_node_id = None),
            ),
            (
                "missing line",
                RawAdmissionFailure::MissingLine,
                Box::new(|edge, _, _| edge.line = None),
            ),
            (
                "invalid or legacy callsite identity",
                RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity,
                Box::new(|edge, _, _| edge.callsite_identity = Some("opaque-legacy-id".to_owned())),
            ),
            (
                "callsite file mismatch",
                RawAdmissionFailure::CallsiteFileMismatch,
                Box::new(|edge, _, _| {
                    edge.callsite_identity = Some("-4:12:0:19|collector-marker".to_owned())
                }),
            ),
            (
                "callsite line mismatch",
                RawAdmissionFailure::CallsiteLineMismatch,
                Box::new(|edge, _, _| {
                    edge.callsite_identity = Some("-3:13:0:19|collector-marker".to_owned())
                }),
            ),
            (
                "callsite raw target mismatch",
                RawAdmissionFailure::CallsiteRawTargetMismatch,
                Box::new(|edge, _, _| edge.target = NodeId(18)),
            ),
        ];

        for (label, expected_reason, mutate) in cases {
            let mut edge = lawful.clone();
            let mut expected_source = NodeId(11);
            let mut expected_target = NodeId(23);
            mutate(&mut edge, &mut expected_source, &mut expected_target);
            assert_eq!(
                diagnose_raw_call_edge(&edge, expected_source, expected_target),
                Err(expected_reason),
                "{label}"
            );
            assert_eq!(
                admit_raw_call_edge(&edge, expected_source, expected_target),
                RawCallEdgeAdmission::Rejected,
                "{label}"
            );
        }
    }

    #[test]
    fn raw_call_edge_admission_rejects_each_hostile_mutation() {
        type EdgeMutation = (&'static str, Box<dyn Fn(&mut Edge)>);

        let lawful = Edge {
            id: EdgeId(41),
            source: NodeId(7),
            target: NodeId(19),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(-3)),
            line: Some(12),
            resolved_source: Some(NodeId(11)),
            resolved_target: Some(NodeId(23)),
            callsite_identity: Some("-3:12:0:19|collector-marker".to_owned()),
            candidate_targets: Vec::new(),
            ..Default::default()
        };
        let mut mutations: Vec<EdgeMutation> = vec![
            ("wrong kind", Box::new(|edge| edge.kind = EdgeKind::USAGE)),
            (
                "wrong raw source",
                Box::new(|edge| {
                    edge.source = NodeId(12);
                    edge.resolved_source = None;
                }),
            ),
            (
                "wrong raw target",
                Box::new(|edge| edge.target = NodeId(18)),
            ),
            (
                "wrong effective source",
                Box::new(|edge| edge.resolved_source = Some(NodeId(12))),
            ),
            (
                "wrong resolved target",
                Box::new(|edge| edge.resolved_target = Some(NodeId(24))),
            ),
            (
                "missing resolved target",
                Box::new(|edge| edge.resolved_target = None),
            ),
            (
                "candidates present",
                Box::new(|edge| edge.candidate_targets = vec![NodeId(23)]),
            ),
            ("file absent", Box::new(|edge| edge.file_node_id = None)),
            ("line absent", Box::new(|edge| edge.line = None)),
            ("line zero", Box::new(|edge| edge.line = Some(0))),
        ];
        for identity in [
            "",
            " ",
            "|marker",
            "-3:12:19",
            "-3:12:0:19:5",
            "x:12:0:19",
            "-3:x:0:19",
            "-3:12:x:19",
            "-3:12:0:x",
            " -3:12:0:19",
            "-03:12:0:19",
            "-3:012:0:19",
            "-3:12:00:19",
            "-3:12:0:+19",
            "opaque-legacy-id",
            "-3:12:0:18",
        ] {
            let identity = identity.to_owned();
            mutations.push((
                "malformed identity",
                Box::new(move |edge| {
                    edge.callsite_identity = Some(identity.clone());
                }),
            ));
        }
        mutations.push((
            "identity absent",
            Box::new(|edge| edge.callsite_identity = None),
        ));

        for (label, mutate) in mutations {
            let mut edge = lawful.clone();
            mutate(&mut edge);
            assert_eq!(
                admit_raw_call_edge(&edge, NodeId(11), NodeId(23)),
                RawCallEdgeAdmission::Rejected,
                "{label}"
            );
        }

        let mut same_display_different_resolution = lawful;
        same_display_different_resolution.resolved_target = Some(NodeId(24));
        assert_eq!(
            same_display_different_resolution
                .callsite_identity
                .as_deref(),
            Some("-3:12:0:19|collector-marker")
        );
        assert_eq!(
            admit_raw_call_edge(&same_display_different_resolution, NodeId(11), NodeId(23)),
            RawCallEdgeAdmission::Rejected
        );
    }

    #[test]
    fn rejects_zero_and_seven_steps() {
        for actual in [0, 7] {
            let targets = (0..actual).map(|_| "B").collect::<Vec<_>>();
            assert_eq!(
                validate_contract(valid_input(&targets)),
                Err(ValidationError::StepCountOutOfRange { actual })
            );
        }
    }

    #[test]
    fn rejects_invalid_selector_paths_and_accepts_normalized_components() {
        let invalid: Vec<(Vec<&str>, SelectorValidationError)> = vec![
            (vec![], SelectorValidationError::RootPath),
            (vec![""], SelectorValidationError::EmptyPathComponent),
            (vec!["."], SelectorValidationError::DotPathComponent),
            (vec![".."], SelectorValidationError::DotPathComponent),
            (
                vec!["src/lib.rs"],
                SelectorValidationError::SeparatorInsidePathComponent,
            ),
            (
                vec!["src\\lib.rs"],
                SelectorValidationError::SeparatorInsidePathComponent,
            ),
            (
                vec!["bad\0name"],
                SelectorValidationError::NulInsidePathComponent,
            ),
            (vec!["C:"], SelectorValidationError::PlatformEscape),
            (vec!["~escape"], SelectorValidationError::PlatformEscape),
        ];
        for (components, expected) in invalid {
            let result = validate_symbol_selector(UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name: "crate::symbol".to_owned(),
                project_file_components: Some(components.into_iter().map(str::to_owned).collect()),
            });
            assert_eq!(result, Err(expected));
        }
        assert!(
            validate_symbol_selector(UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name: "crate::symbol".to_owned(),
                project_file_components: Some(vec!["src".to_owned(), "lib.rs".to_owned()]),
            })
            .is_ok()
        );
    }

    #[test]
    fn exact_selector_forms_reject_empty_nul_and_non_normalized_values() {
        assert_eq!(
            validate_symbol_selector(canonical_selector("")),
            Err(SelectorValidationError::EmptyIdentity)
        );
        assert_eq!(
            validate_symbol_selector(canonical_selector("bad\0id")),
            Err(SelectorValidationError::IdentityContainsNul)
        );
        assert_eq!(
            validate_symbol_selector(UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name: " crate::A".to_owned(),
                project_file_components: None,
            }),
            Err(SelectorValidationError::NonNormalizedQualifiedName)
        );
        assert!(
            validate_symbol_selector(UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name: "crate::A()".to_owned(),
                project_file_components: None,
            })
            .is_err(),
            "signature syntax is not an exact qualified-name selector"
        );
        let pinned = PinnedNodeIdentity {
            project_id: "project".to_owned(),
            core_generation_id: "generation".to_owned(),
            core_run_id: "run".to_owned(),
            node_id: "node".to_owned(),
        };
        assert!(
            validate_symbol_selector(UnvalidatedExactSymbolSelector::PinnedNode(pinned)).is_ok()
        );
    }

    #[test]
    fn scope_paths_match_only_at_component_boundaries() {
        let candidate = ResolvedNodeIdentity::new(
            node("A").pinned,
            "A",
            "crate::A",
            NodeId(1),
            vec!["src".to_owned(), "indexer".to_owned(), "lib.rs".to_owned()],
        )
        .unwrap();
        let exact_prefix = validate_scope_selector(UnvalidatedExactScopeSelector::QualifiedName {
            qualified_name: "crate::A".to_owned(),
            project_file_components: Some(vec!["src".to_owned(), "indexer".to_owned()]),
        })
        .unwrap();
        let lexical_prefix =
            validate_scope_selector(UnvalidatedExactScopeSelector::QualifiedName {
                qualified_name: "crate::A".to_owned(),
                project_file_components: Some(vec!["src".to_owned(), "index".to_owned()]),
            })
            .unwrap();
        assert!(scope_selector_matches(&exact_prefix, &candidate));
        assert!(!scope_selector_matches(&lexical_prefix, &candidate));
    }

    #[test]
    fn rejects_utf8_boundary_and_quote_mismatch() {
        let source = "é direct call";
        let mut boundary = valid_input(&["B"]);
        boundary.source_text = source.to_owned();
        boundary.clauses = vec![ClauseAnchor {
            clause_id: "bad-boundary".to_owned(),
            start: 1,
            end: source.len(),
            quote: source[2..].to_owned(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::Start],
            },
        }];
        assert_eq!(
            validate_contract(boundary),
            Err(ValidationError::ClauseSpanNotUtf8Boundary {
                clause_id: "bad-boundary".to_owned()
            })
        );
        let mut mismatch = valid_input(&["B"]);
        mismatch.clauses[0].quote = "different bytes".to_owned();
        assert_eq!(
            validate_contract(mismatch),
            Err(ValidationError::ClauseQuoteMismatch {
                clause_id: "whole".to_owned()
            })
        );
    }

    #[test]
    fn accepts_overlapping_and_nested_spans() {
        let mut input = valid_input(&["B"]);
        input.clauses.push(ClauseAnchor {
            clause_id: "nested".to_owned(),
            start: 6,
            end: 12,
            quote: input.source_text[6..12].to_owned(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::Directness { step: 0 }],
            },
        });
        assert!(matches!(
            validate_contract(input),
            Ok(ValidationOutcome::Validated { .. })
        ));
    }

    #[test]
    fn whitespace_may_be_uncovered_but_material_bytes_may_not() {
        let source = "A B";
        let mut input = valid_input(&["B"]);
        input.source_text = source.to_owned();
        input.clauses = vec![
            ClauseAnchor {
                clause_id: "a".to_owned(),
                start: 0,
                end: 1,
                quote: "A".to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::Start,
                        ProofContractField::Directness { step: 0 },
                        ProofContractField::Ordering { step: 0 },
                        ProofContractField::Relation { step: 0 },
                    ],
                },
            },
            ClauseAnchor {
                clause_id: "b".to_owned(),
                start: 2,
                end: 3,
                quote: "B".to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![ProofContractField::StepTarget { step: 0 }],
                },
            },
        ];
        assert!(matches!(
            validate_contract(input.clone()),
            Ok(ValidationOutcome::Validated { .. })
        ));
        input.clauses.pop();
        input.clauses[0].classification = ClauseClassification::ResolvedMaterial {
            fields: vec![
                ProofContractField::Start,
                ProofContractField::StepTarget { step: 0 },
                ProofContractField::Directness { step: 0 },
                ProofContractField::Ordering { step: 0 },
                ProofContractField::Relation { step: 0 },
            ],
        };
        assert!(matches!(
            validate_contract(input),
            Ok(ValidationOutcome::Unknown { gaps, .. })
                if gaps.contains(&TranslationGap::UnclassifiedSourceText)
        ));
    }

    #[test]
    fn rejects_missing_typed_field_coverage() {
        let mut input = valid_input(&["B"]);
        input
            .clauses
            .retain(|clause| clause.clause_id != "target-0");
        assert_eq!(
            validate_contract(input),
            Err(ValidationError::MissingResolvedMaterialAnchor {
                field: ProofContractField::StepTarget { step: 0 },
                required: 1,
                found: 0,
            })
        );
    }

    #[test]
    fn duplicate_step_zero_anchors_cannot_cover_step_one() {
        let mut input = valid_input(&["B", "C"]);
        input.clauses[2].classification = ClauseClassification::ResolvedMaterial {
            fields: vec![
                ProofContractField::StepTarget { step: 0 },
                ProofContractField::Directness { step: 0 },
                ProofContractField::Ordering { step: 0 },
                ProofContractField::Relation { step: 0 },
            ],
        };
        assert_eq!(
            validate_contract(input),
            Err(ValidationError::MissingResolvedMaterialAnchor {
                field: ProofContractField::StepTarget { step: 1 },
                required: 1,
                found: 0,
            })
        );
    }

    #[test]
    fn rejects_out_of_range_or_unpopulated_indexed_field_references() {
        for field in [
            ProofContractField::Relation { step: 1 },
            ProofContractField::TraversalProhibition { index: 0 },
            ProofContractField::ProjectionExclusion { index: 0 },
        ] {
            let mut input = valid_input(&["B"]);
            input.clauses.push(ClauseAnchor {
                clause_id: format!("out-of-range-{field:?}"),
                start: 0,
                end: input.source_text.len(),
                quote: input.source_text.clone(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![field],
                },
            });
            assert_eq!(
                validate_contract(input),
                Err(ValidationError::OutOfRangeFieldReference { field })
            );
        }
    }

    #[test]
    fn canonical_clause_json_commits_the_typed_field_index() {
        let clause = NormalizedClause {
            start: 0,
            end: 1,
            clause_id: "target".to_owned(),
            classification: NormalizedClauseClassification::Resolved,
            field: Some(ProofContractField::StepTarget { step: 2 }),
            quote: "B".to_owned(),
        };
        assert_eq!(
            normalized_clause_json(&clause)["field"],
            json!({ "kind": "step_target", "step": 2 })
        );
    }

    #[test]
    fn rejects_classification_conflicts() {
        let mut input = valid_input(&["B"]);
        input.clauses.push(ClauseAnchor {
            clause_id: "whole".to_owned(),
            start: 0,
            end: input.source_text.len(),
            quote: input.source_text.clone(),
            classification: ClauseClassification::NonMaterial {
                kind: NonMaterialKind::Commentary,
            },
        });
        assert_eq!(
            validate_contract(input),
            Err(ValidationError::ClassificationConflict {
                clause_id: "whole".to_owned()
            })
        );
    }

    #[test]
    fn unresolved_material_is_a_typed_unknown() {
        let mut input = valid_input(&["B"]);
        input.clauses.push(ClauseAnchor {
            clause_id: "uncertain".to_owned(),
            start: 0,
            end: input.source_text.len(),
            quote: input.source_text.clone(),
            classification: ClauseClassification::UnresolvedMaterial {
                reason: UnresolvedMaterialReason::AmbiguousSelectorResolution,
            },
        });
        assert!(matches!(
            validate_contract(input),
            Ok(ValidationOutcome::Unknown { gaps, .. })
                if gaps.contains(&TranslationGap::UnresolvedMaterialClause {
                    clause_id: "uncertain".to_owned(),
                    reason: UnresolvedMaterialReason::AmbiguousSelectorResolution,
                })
        ));
    }

    #[test]
    fn clause_guard_covers_every_declared_family() {
        let cases = [
            (
                "`crate::A`",
                ClauseGuardFamily::QuotedOrBacktickedIdentifier,
            ),
            ("A -> B", ClauseGuardFamily::ArrowOrRelationNotation),
            ("directly", ClauseGuardFamily::Directness),
            ("then 2nd", ClauseGuardFamily::OrderingOrOrdinal),
            ("only", ClauseGuardFamily::Only),
            ("without excluded", ClauseGuardFamily::NegationOrExclusion),
            ("src/lib.rs", ClauseGuardFamily::PathLikeString),
            ("lib.rs", ClauseGuardFamily::PathLikeString),
            (
                "crate::module::A",
                ClauseGuardFamily::QualifiedSymbolNotation,
            ),
        ];
        for (text, expected) in cases {
            assert!(
                clause_guard_families(text).contains(&expected),
                "{text:?} did not trigger {expected:?}"
            );
        }
    }

    #[test]
    fn guarded_material_with_only_non_material_coverage_is_unknown() {
        let source = "only";
        let mut input = valid_input(&["B"]);
        input.source_text = source.to_owned();
        input.clauses = vec![
            ClauseAnchor {
                clause_id: "fields".to_owned(),
                start: 0,
                end: 1,
                quote: "o".to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::Start,
                        ProofContractField::StepTarget { step: 0 },
                        ProofContractField::Directness { step: 0 },
                        ProofContractField::Ordering { step: 0 },
                        ProofContractField::Relation { step: 0 },
                    ],
                },
            },
            ClauseAnchor {
                clause_id: "guarded".to_owned(),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::NonMaterial {
                    kind: NonMaterialKind::Commentary,
                },
            },
        ];
        assert!(matches!(
            validate_contract(input),
            Ok(ValidationOutcome::Unknown { gaps, .. })
                if gaps.contains(&TranslationGap::MaterialTokenMisclassified {
                    clause_id: "guarded".to_owned(),
                    guard_families: vec![ClauseGuardFamily::Only],
                })
        ));
    }

    #[test]
    fn resolved_guard_span_wins_over_a_broader_non_material_anchor() {
        let source = "only x";
        let mut input = valid_input(&["B"]);
        input.source_text = source.to_owned();
        input.clauses = vec![
            ClauseAnchor {
                clause_id: "resolved-guard".to_owned(),
                start: 0,
                end: 4,
                quote: "only".to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::Start,
                        ProofContractField::StepTarget { step: 0 },
                        ProofContractField::Directness { step: 0 },
                        ProofContractField::Ordering { step: 0 },
                        ProofContractField::Relation { step: 0 },
                    ],
                },
            },
            ClauseAnchor {
                clause_id: "broad-commentary".to_owned(),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::NonMaterial {
                    kind: NonMaterialKind::Commentary,
                },
            },
        ];
        assert!(matches!(
            validate_contract(input),
            Ok(ValidationOutcome::Validated { .. })
        ));
    }

    #[test]
    fn digest_is_stable_under_clause_order_and_exact_duplicates() {
        let input = valid_input(&["B"]);
        let (_, baseline) = validate(&["B"]);
        let mut reordered = input.clone();
        reordered.clauses.reverse();
        let reordered = match validate_contract(reordered).unwrap() {
            ValidationOutcome::Validated { hashes, .. } => hashes,
            other => panic!("unexpected {other:?}"),
        };
        let mut duplicated = input;
        duplicated.clauses.push(duplicated.clauses[0].clone());
        let duplicated = match validate_contract(duplicated).unwrap() {
            ValidationOutcome::Validated { hashes, .. } => hashes,
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(baseline, reordered);
        assert_eq!(baseline, duplicated);
    }

    #[test]
    fn digest_changes_with_schema_guard_domain_source_and_ordered_spec() {
        let input = valid_input(&["B", "C"]);
        let baseline = match validate_contract(input.clone()).unwrap() {
            ValidationOutcome::Validated { hashes, .. } => hashes,
            other => panic!("unexpected {other:?}"),
        };
        for domain in [
            DigestDomain {
                schema_version: 2,
                ..DigestDomain::default()
            },
            DigestDomain {
                guard_version: "clause_guard_v2",
                ..DigestDomain::default()
            },
            DigestDomain {
                proof_domain: "other_domain",
                ..DigestDomain::default()
            },
        ] {
            let changed = match validate_contract_with_domain(input.clone(), domain).unwrap() {
                ValidationOutcome::Validated { hashes, .. } => hashes,
                other => panic!("unexpected {other:?}"),
            };
            assert_ne!(baseline.contract_digest(), changed.contract_digest());
        }
        let mut changed_source = input.clone();
        changed_source.source_text.push('!');
        changed_source.clauses[0].end += 1;
        changed_source.clauses[0].quote.push('!');
        let changed_source = match validate_contract(changed_source).unwrap() {
            ValidationOutcome::Validated { hashes, .. } => hashes,
            other => panic!("unexpected {other:?}"),
        };
        assert_ne!(baseline.contract_digest(), changed_source.contract_digest());
        let reordered_spec = match validate_contract(valid_input(&["C", "B"])).unwrap() {
            ValidationOutcome::Validated { hashes, .. } => hashes,
            other => panic!("unexpected {other:?}"),
        };
        assert_ne!(baseline.contract_digest(), reordered_spec.contract_digest());
    }

    #[test]
    fn scope_input_order_is_committed_to_the_contract_digest() {
        let mut first = valid_input(&["B"]);
        first.spec.prohibit_traversal_through = vec![canonical_scope("B"), canonical_scope("C")];
        for index in 0..2_u8 {
            first.clauses.push(ClauseAnchor {
                clause_id: format!("prohibition-{index}"),
                start: 0,
                end: first.source_text.len(),
                quote: first.source_text.clone(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![ProofContractField::TraversalProhibition { index }],
                },
            });
        }
        let mut second = first.clone();
        second.spec.prohibit_traversal_through.reverse();
        let digest = |input| match validate_contract(input).unwrap() {
            ValidationOutcome::Validated { hashes, .. } => hashes.contract_digest,
            other => panic!("unexpected {other:?}"),
        };
        assert_ne!(digest(first), digest(second));
    }

    #[test]
    fn rfc8785_appendix_number_and_string_vector_is_exact() {
        let value: Value = serde_json::from_str(
            r#"{"numbers":[333333333.33333329,1E30,4.50,2e-3,0.000000000000000000000000001],"string":"€$\u000f\nA'B\"\\\\\"/","literals":[null,true,false]}"#,
        )
        .unwrap();
        let canonical =
            String::from_utf8(serde_json_canonicalizer::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            canonical,
            r#"{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"string":"€$\u000f\nA'B\"\\\\\"/"}"#
        );
    }

    #[test]
    fn rfc8785_orders_unicode_keys_by_utf16_code_units() {
        let value = json!({"\u{e000}": "bmp", "\u{10000}": "supplementary", "a": "ascii"});
        let canonical =
            String::from_utf8(serde_json_canonicalizer::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            canonical,
            "{\"a\":\"ascii\",\"𐀀\":\"supplementary\",\"\":\"bmp\"}"
        );
    }

    #[test]
    fn proves_only_a_connected_ordered_direct_fact_per_step_path() {
        let (contract, hashes) = validate(&["B", "C"]);
        let facts = vec![
            call("receipt-2", "edge-2", "B", "C"),
            call("receipt-1", "edge-1", "A", "B"),
        ];
        assert!(matches!(
            check_call_path(&contract, &hashes, &facts),
            ProofDisposition::ContractProven { receipts, .. }
                if receipts.iter().map(|receipt| receipt.edge_id.as_str()).collect::<Vec<_>>()
                    == ["edge-1", "edge-2"]
        ));
        for hostile in [
            vec![call("r1", "e1", "A", "B"), call("r2", "e2", "D", "C")],
            vec![call("r1", "e1", "A", "C"), call("r2", "e2", "C", "B")],
        ] {
            assert!(matches!(
                check_call_path(&contract, &hashes, &hostile),
                ProofDisposition::Unknown { .. }
            ));
        }
    }

    #[test]
    fn repeated_vertices_and_self_edges_are_valid_with_distinct_receipts() {
        let (contract, hashes) = validate(&["A", "A"]);
        let distinct = vec![
            call("receipt-1", "edge-1", "A", "A"),
            call("receipt-2", "edge-2", "A", "A"),
        ];
        assert!(matches!(
            check_call_path(&contract, &hashes, &distinct),
            ProofDisposition::ContractProven { .. }
        ));
        for duplicate in [
            vec![
                call("receipt-1", "edge-1", "A", "A"),
                call("receipt-1", "edge-2", "A", "A"),
            ],
            vec![
                call("receipt-1", "edge-1", "A", "A"),
                call("receipt-2", "edge-1", "A", "A"),
            ],
        ] {
            assert!(matches!(
                check_call_path(&contract, &hashes, &duplicate),
                ProofDisposition::Unknown { gaps, .. }
                    if gaps == [ProofGap::ReceiptOrEdgeAlreadyUsed { step_index: 1 }]
            ));
        }
    }

    #[test]
    fn wrong_relation_direction_or_target_cannot_refute_a_required_direct_edge() {
        enum NonDirectObservation {
            OtherRelation,
            ReversedDirection,
            DifferentTarget,
        }
        let (contract, hashes) = validate(&["B"]);
        for observation in [
            NonDirectObservation::OtherRelation,
            NonDirectObservation::ReversedDirection,
            NonDirectObservation::DifferentTarget,
        ] {
            let admitted_facts = match observation {
                NonDirectObservation::OtherRelation => Vec::new(),
                NonDirectObservation::ReversedDirection => vec![call("r", "e", "B", "A")],
                NonDirectObservation::DifferentTarget => vec![call("r", "e", "A", "C")],
            };
            assert!(matches!(
                check_call_path(&contract, &hashes, &admitted_facts),
                ProofDisposition::Unknown { .. }
            ));
        }
    }

    #[test]
    fn fixture_absence_requires_both_completeness_receipts() {
        let (contract, hashes) = validate(&["B"]);
        let absence = VerifiedProofFact::CertifiedAbsence(CertifiedAbsenceFact {
            source: node("A"),
            expected_target: ExactSymbolSelector::CanonicalId("B".to_owned()),
            extractor_capability_receipt_id: "capability".to_owned(),
            untruncated_enumeration_receipt_id: "enumeration".to_owned(),
        });
        assert!(matches!(
            check_call_path(&contract, &hashes, &[absence]),
            ProofDisposition::ContractRefuted {
                refutation: Refutation::CertifiedAbsence { .. },
                ..
            }
        ));
        let incomplete_absence = VerifiedProofFact::CertifiedAbsence(CertifiedAbsenceFact {
            source: node("A"),
            expected_target: ExactSymbolSelector::CanonicalId("B".to_owned()),
            extractor_capability_receipt_id: "capability".to_owned(),
            untruncated_enumeration_receipt_id: String::new(),
        });
        assert!(matches!(
            check_call_path(&contract, &hashes, &[incomplete_absence]),
            ProofDisposition::Unknown { .. }
        ));
    }

    #[test]
    fn missing_completeness_is_unknown_and_unavailability_is_distinct() {
        let (contract, hashes) = validate(&["B"]);
        assert!(matches!(
            check_call_path(&contract, &hashes, &[]),
            ProofDisposition::Unknown { gaps, .. }
                if gaps == [ProofGap::MissingDirectCallReceipt { step_index: 0 }]
        ));
        assert!(matches!(
            check_call_path(
                &contract,
                &hashes,
                &[VerifiedProofFact::Unavailable(UnavailableProofFact {
                    reason: UnavailableReason::SourceNotBoundToPublication,
                })]
            ),
            ProofDisposition::Unavailable { .. }
        ));
        assert!(matches!(
            check_call_path(&contract, &hashes, &[call("", "edge", "A", "B")]),
            ProofDisposition::Unknown { .. }
        ));
    }

    #[test]
    fn projection_exclusion_hiding_required_receipt_is_unknown() {
        let mut input = valid_input(&["B"]);
        input.spec.exclude_from_projection = vec![canonical_scope("B")];
        input.clauses.push(ClauseAnchor {
            clause_id: "projection".to_owned(),
            start: 0,
            end: input.source_text.len(),
            quote: input.source_text.clone(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::ProjectionExclusion { index: 0 }],
            },
        });
        let (contract, hashes) = match validate_contract(input).unwrap() {
            ValidationOutcome::Validated {
                contract, hashes, ..
            } => (contract, hashes),
            other => panic!("unexpected {other:?}"),
        };
        assert!(matches!(
            check_call_path(&contract, &hashes, &[call("r", "e", "A", "B")]),
            ProofDisposition::Unknown { gaps, .. }
                if gaps == [ProofGap::ProjectionExclusionConflictsWithRequiredReceipt { step_index: 0 }]
        ));
    }

    #[test]
    fn fixture_absence_reached_only_through_an_excluded_receipt_stays_unknown() {
        let mut input = valid_input(&["B", "C"]);
        input.spec.exclude_from_projection = vec![canonical_scope("B")];
        input.clauses.push(ClauseAnchor {
            clause_id: "projection".to_owned(),
            start: 0,
            end: input.source_text.len(),
            quote: input.source_text.clone(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::ProjectionExclusion { index: 0 }],
            },
        });
        let (contract, hashes) = match validate_contract(input).unwrap() {
            ValidationOutcome::Validated {
                contract, hashes, ..
            } => (contract, hashes),
            other => panic!("unexpected {other:?}"),
        };
        let facts = [
            call("r1", "e1", "A", "B"),
            VerifiedProofFact::CertifiedAbsence(CertifiedAbsenceFact {
                source: node("B"),
                expected_target: ExactSymbolSelector::CanonicalId("C".to_owned()),
                extractor_capability_receipt_id: "capability".to_owned(),
                untruncated_enumeration_receipt_id: "enumeration".to_owned(),
            }),
        ];
        assert!(matches!(
            check_call_path(&contract, &hashes, &facts),
            ProofDisposition::Unknown { gaps, .. }
                if gaps == [ProofGap::ProjectionExclusionConflictsWithRequiredReceipt {
                    step_index: 0,
                }]
        ));
    }

    #[test]
    fn traversal_prohibition_is_receipt_backed_positive_refutation() {
        let mut input = valid_input(&["B", "C", "D"]);
        input.spec.prohibit_traversal_through = vec![canonical_scope("C")];
        input.clauses.push(ClauseAnchor {
            clause_id: "prohibition".to_owned(),
            start: 0,
            end: input.source_text.len(),
            quote: input.source_text.clone(),
            classification: ClauseClassification::ResolvedMaterial {
                fields: vec![ProofContractField::TraversalProhibition { index: 0 }],
            },
        });
        let (contract, hashes) = match validate_contract(input).unwrap() {
            ValidationOutcome::Validated {
                contract, hashes, ..
            } => (contract, hashes),
            other => panic!("unexpected {other:?}"),
        };
        let facts = [
            call("r1", "e1", "A", "B"),
            call("r2", "e2", "B", "C"),
            call("r3", "e3", "C", "D"),
        ];
        assert!(matches!(
            check_call_path(&contract, &hashes, &facts),
            ProofDisposition::ContractRefuted {
                refutation: Refutation::ProhibitedScopeTraversal {
                    step_index: 1,
                    prohibition_index: 0,
                    connected_receipts,
                },
                ..
            } if connected_receipts == [
                ReceiptRef { receipt_id: "r1".to_owned(), edge_id: "e1".to_owned() },
                ReceiptRef { receipt_id: "r2".to_owned(), edge_id: "e2".to_owned() },
            ]
        ));
    }

    #[test]
    fn checker_boundary_accepts_validated_contract_hashes_and_facts_only() {
        type CheckerBoundary =
            fn(&ValidatedCallPathContract, &ProofHashes, &[VerifiedProofFact]) -> ProofDisposition;
        let checker: CheckerBoundary = check_call_path;
        let (contract, hashes) = validate(&["B"]);
        assert!(matches!(
            checker(&contract, &hashes, &[call("r", "e", "A", "B")]),
            ProofDisposition::ContractProven { .. }
        ));
    }
}
