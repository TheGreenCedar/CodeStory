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
//! This module is compiled only for crate tests or the explicit `test-support`
//! feature. No production dispatcher enables that feature.

use std::collections::{BTreeMap, BTreeSet};

use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, NodeId, ResolutionCertainty};
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
    pub certainty: ResolutionCertainty,
    pub callsite_identity: String,
    pub containment: CallableContainmentEvidence,
    pub line_window: IndexedLineWindow,
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
    pub facts: Vec<VerifiedProofFact>,
    pub receipts: Vec<IndexedCallEdgeReceipt>,
    pub gaps: Vec<FactBuildGap>,
    pub unavailable: Vec<UnavailableReason>,
}

/// Admit one persisted edge as direct-call evidence without rewriting either
/// endpoint or deriving certainty from its diagnostic confidence score.
pub fn admit_raw_call_edge(
    edge: &Edge,
    expected_source: NodeId,
    expected_target: NodeId,
) -> RawCallEdgeAdmission {
    if edge.kind != EdgeKind::CALL
        || edge.certainty != Some(ResolutionCertainty::Certain)
        || edge.effective_source() != expected_source
        || edge.effective_target() != expected_target
        || edge.resolved_target != Some(expected_target)
        || !edge.candidate_targets.is_empty()
    {
        return RawCallEdgeAdmission::Rejected;
    }
    let (Some(file_node_id), Some(line), Some(callsite_identity)) = (
        edge.file_node_id,
        edge.line.filter(|line| *line >= 1),
        edge.callsite_identity.as_deref(),
    ) else {
        return RawCallEdgeAdmission::Rejected;
    };
    if callsite_identity.is_empty() {
        return RawCallEdgeAdmission::Rejected;
    }
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
        return RawCallEdgeAdmission::Rejected;
    }
    let (Some(parsed_file), Some(parsed_line), Some(column_or_ordinal), Some(parsed_target)) =
        parsed
    else {
        return RawCallEdgeAdmission::Rejected;
    };
    if parsed_file != file_node_id.0
        || parsed_line != line
        || parsed_target != edge.target.0
        || format!("{parsed_file}:{parsed_line}:{column_or_ordinal}:{parsed_target}") != pre_marker
    {
        return RawCallEdgeAdmission::Rejected;
    }
    RawCallEdgeAdmission::Admitted(AdmittedRawCallEdge {
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
    Ok(ProofHashes {
        source_text_sha256,
        contract_digest: sha256_hex(&digest_bytes),
    })
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
    pub project_file_components: Vec<String>,
}

impl ResolvedNodeIdentity {
    pub fn new(
        pinned: PinnedNodeIdentity,
        canonical_id: impl Into<String>,
        qualified_name: impl Into<String>,
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
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProofGap {
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
}

pub fn check_call_path(
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    facts: &[VerifiedProofFact],
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
    if let Some(path) = find_path(contract, &direct_facts, PathPolicy::Strict) {
        return ProofDisposition::ContractProven {
            contract_digest: hashes.contract_digest.clone(),
            receipts: path.into_iter().map(|fact| fact.receipt.clone()).collect(),
        };
    }
    if let Some(path) = find_path(
        contract,
        &direct_facts,
        PathPolicy::AllowProjectionExclusions,
    ) {
        let step_index = first_projection_conflict(contract, &path).unwrap_or(0);
        return ProofDisposition::Unknown {
            contract_digest: hashes.contract_digest.clone(),
            gaps: vec![ProofGap::ProjectionExclusionConflictsWithRequiredReceipt { step_index }],
        };
    }
    let mut reachable = reachable_prefixes(contract, &direct_facts);
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
        };
    }
    for state in reachable.iter().rev() {
        if state.step_index >= contract.spec.steps.len() || state.projection_conflict_step.is_some()
        {
            continue;
        }
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
) -> Option<Vec<&'a VerifiedDirectCallFact>> {
    let mut ordered = facts.to_vec();
    ordered.sort_by(|left, right| left.receipt.cmp(&right.receipt));
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
) -> Vec<PrefixState> {
    let mut facts = facts.to_vec();
    facts.sort_by(|left, right| left.receipt.cmp(&right.receipt));
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

pub const INTERNAL_PROOF_CAP_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalCorePublicationIdentity {
    pub project_id: String,
    pub generation_id: String,
    pub run_id: String,
}

pub struct InternalProjectionInput<'a> {
    pub contract: &'a ValidatedCallPathContract,
    pub hashes: &'a ProofHashes,
    pub rendering: &'a ValidatedContractRendering,
    pub publication: InternalCorePublicationIdentity,
    pub disposition: &'a ProofDisposition,
    pub receipts: &'a [IndexedCallEdgeReceipt],
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
    MissingAuthoritativeReceipt {
        receipt_id: String,
        edge_id: String,
    },
    Serialization(String),
    FallbackExceedsCap {
        cap_bytes: usize,
        serialized_size: usize,
    },
}

pub fn project_internal_call_path_result(
    input: InternalProjectionInput<'_>,
) -> Result<InternalProjection, InternalProjectionError> {
    project_internal_call_path_result_with_cap(input, INTERNAL_PROOF_CAP_BYTES)
}

fn project_internal_call_path_result_with_cap(
    input: InternalProjectionInput<'_>,
    cap_bytes: usize,
) -> Result<InternalProjection, InternalProjectionError> {
    validate_authoritative_receipts(input.disposition, input.receipts)?;
    let complete = complete_projection_json(&input);
    let required_complete_size = serialized_json_size(&complete)?;
    if required_complete_size <= cap_bytes {
        return Ok(InternalProjection::Complete {
            root: complete,
            serialized_size: required_complete_size,
        });
    }

    let fallback = json!({
        "kind": "budget_exceeded",
        "schema_version": PROOF_CONTRACT_SCHEMA_VERSION,
        "domain": PROOF_DOMAIN,
        "contract_interpretation": "host_supplied",
        "guard_version": CLAUSE_GUARD_VERSION,
        "source_text_sha256": input.hashes.source_text_sha256,
        "contract_digest": input.hashes.contract_digest,
        "core_publication": publication_json(&input.publication),
        "disposition": {
            "kind": "unknown",
            "contract_digest": input.hashes.contract_digest,
            "gaps": [{ "kind": "output_budget_exceeded" }],
        },
        "cap_bytes": cap_bytes,
        "required_complete_size": required_complete_size,
    });
    let fallback_size = serialized_json_size(&fallback)?;
    if fallback_size > cap_bytes {
        return Err(InternalProjectionError::FallbackExceedsCap {
            cap_bytes,
            serialized_size: fallback_size,
        });
    }
    Ok(InternalProjection::BudgetExceeded {
        root: fallback,
        required_complete_size,
        serialized_size: fallback_size,
    })
}

fn validate_authoritative_receipts(
    disposition: &ProofDisposition,
    receipts: &[IndexedCallEdgeReceipt],
) -> Result<(), InternalProjectionError> {
    let required = match disposition {
        ProofDisposition::ContractProven { receipts, .. } => receipts.as_slice(),
        ProofDisposition::ContractRefuted {
            refutation:
                Refutation::ProhibitedScopeTraversal {
                    connected_receipts, ..
                },
            ..
        } => connected_receipts.as_slice(),
        _ => &[],
    };
    for receipt in required {
        if !receipts
            .iter()
            .any(|candidate| candidate.receipt == *receipt)
        {
            return Err(InternalProjectionError::MissingAuthoritativeReceipt {
                receipt_id: receipt.receipt_id.clone(),
                edge_id: receipt.edge_id.clone(),
            });
        }
    }
    Ok(())
}

fn complete_projection_json(input: &InternalProjectionInput<'_>) -> Value {
    json!({
        "kind": "complete",
        "schema_version": PROOF_CONTRACT_SCHEMA_VERSION,
        "domain": PROOF_DOMAIN,
        "contract_interpretation": "host_supplied",
        "guard_version": CLAUSE_GUARD_VERSION,
        "source_text_sha256": input.hashes.source_text_sha256,
        "contract_digest": input.hashes.contract_digest,
        "core_publication": publication_json(&input.publication),
        "spec": spec_json(&input.contract.spec),
        "clauses": input
            .rendering
            .normalized_clauses
            .iter()
            .map(normalized_clause_json)
            .collect::<Vec<_>>(),
        "disposition": disposition_json(input.disposition),
        "steps": step_results_json(input.contract, input.disposition),
        "receipts": input.receipts.iter().map(indexed_receipt_json).collect::<Vec<_>>(),
    })
}

fn publication_json(publication: &InternalCorePublicationIdentity) -> Value {
    json!({
        "project_id": publication.project_id,
        "generation_id": publication.generation_id,
        "run_id": publication.run_id,
    })
}

fn disposition_json(disposition: &ProofDisposition) -> Value {
    match disposition {
        ProofDisposition::ContractProven {
            contract_digest,
            receipts,
        } => json!({
            "kind": "contract_proven",
            "contract_digest": contract_digest,
            "receipts": receipts.iter().map(receipt_ref_json).collect::<Vec<_>>(),
        }),
        ProofDisposition::ContractRefuted {
            contract_digest,
            refutation,
        } => json!({
            "kind": "contract_refuted",
            "contract_digest": contract_digest,
            "refutation": refutation_json(refutation),
        }),
        ProofDisposition::Unknown {
            contract_digest,
            gaps,
        } => json!({
            "kind": "unknown",
            "contract_digest": contract_digest,
            "gaps": gaps.iter().map(proof_gap_json).collect::<Vec<_>>(),
        }),
        ProofDisposition::Unavailable {
            contract_digest,
            reasons,
        } => json!({
            "kind": "unavailable",
            "contract_digest": contract_digest,
            "reasons": reasons.iter().map(unavailable_reason_name).collect::<Vec<_>>(),
        }),
    }
}

fn refutation_json(refutation: &Refutation) -> Value {
    match refutation {
        Refutation::ProhibitedScopeTraversal {
            step_index,
            prohibition_index,
            connected_receipts,
        } => json!({
            "kind": "prohibited_scope_traversal",
            "step_index": step_index,
            "prohibition_index": prohibition_index,
            "connected_receipts": connected_receipts
                .iter()
                .map(receipt_ref_json)
                .collect::<Vec<_>>(),
        }),
        #[cfg(any(test, feature = "test-support"))]
        Refutation::CertifiedAbsence {
            step_index,
            extractor_capability_receipt_id,
            untruncated_enumeration_receipt_id,
        } => json!({
            "kind": "certified_absence",
            "step_index": step_index,
            "extractor_capability_receipt_id": extractor_capability_receipt_id,
            "untruncated_enumeration_receipt_id": untruncated_enumeration_receipt_id,
        }),
    }
}

fn proof_gap_json(gap: &ProofGap) -> Value {
    match gap {
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

fn unavailable_reason_name(reason: &UnavailableReason) -> &'static str {
    match reason {
        UnavailableReason::ValidatedContractHashMismatch => "validated_contract_hash_mismatch",
        UnavailableReason::PublicationPinMismatch => "publication_pin_mismatch",
        UnavailableReason::SourceNotBoundToPublication => "source_not_bound_to_publication",
        UnavailableReason::ProofFactsUnavailable => "proof_facts_unavailable",
    }
}

fn step_results_json(
    contract: &ValidatedCallPathContract,
    disposition: &ProofDisposition,
) -> Vec<Value> {
    let receipts = match disposition {
        ProofDisposition::ContractProven { receipts, .. } => receipts.as_slice(),
        ProofDisposition::ContractRefuted {
            refutation:
                Refutation::ProhibitedScopeTraversal {
                    connected_receipts, ..
                },
            ..
        } => connected_receipts.as_slice(),
        _ => &[],
    };
    contract
        .spec
        .steps
        .iter()
        .enumerate()
        .map(|(step_index, step)| {
            let status = match disposition {
                ProofDisposition::ContractProven { .. } => "proven",
                ProofDisposition::ContractRefuted { .. } if step_index < receipts.len() => {
                    "positive_contradiction"
                }
                ProofDisposition::Unavailable { .. } => "unavailable",
                _ => "unknown",
            };
            json!({
                "step_index": step_index,
                "target": symbol_selector_json(&step.target),
                "status": status,
                "receipt": receipts.get(step_index).map(receipt_ref_json),
            })
        })
        .collect()
}

fn receipt_ref_json(receipt: &ReceiptRef) -> Value {
    json!({
        "receipt_id": receipt.receipt_id,
        "edge_id": receipt.edge_id,
    })
}

fn indexed_receipt_json(receipt: &IndexedCallEdgeReceipt) -> Value {
    json!({
        "receipt": receipt_ref_json(&receipt.receipt),
        "source": resolved_node_json(&receipt.source),
        "target": resolved_node_json(&receipt.target),
        "certainty": match receipt.certainty {
            ResolutionCertainty::Certain => "certain",
            ResolutionCertainty::Probable => "probable",
            ResolutionCertainty::Uncertain => "uncertain",
        },
        "callsite_identity": receipt.callsite_identity,
        "containment": {
            "file_node_id": receipt.containment.file_node_id.0.to_string(),
            "owner_node_id": receipt.containment.owner_node_id.0.to_string(),
            "start_line": receipt.containment.start_line,
            "end_line": receipt.containment.end_line,
        },
        "line_window": {
            "kind": receipt.line_window.kind,
            "project_file_components": receipt.line_window.project_file_components,
            "indexed_sha256": receipt.line_window.indexed_sha256,
            "observed_sha256": receipt.line_window.observed_sha256,
            "anchor_line": receipt.line_window.anchor_line,
            "byte_start": receipt.line_window.byte_start,
            "byte_end": receipt.line_window.byte_end,
            "text": receipt.line_window.text,
        },
    })
}

fn resolved_node_json(node: &ResolvedNodeIdentity) -> Value {
    json!({
        "pinned": pinned_identity_json(&node.pinned),
        "canonical_id": node.canonical_id,
        "qualified_name": node.qualified_name,
        "project_file_components": node.project_file_components,
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
    use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, NodeId, ResolutionCertainty};

    const MAX_SOURCE_RECEIPT_LINE_BYTES: usize = 8_192;

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
        ResolvedNodeIdentity::new(
            PinnedNodeIdentity {
                project_id: "project".to_owned(),
                core_generation_id: "generation".to_owned(),
                core_run_id: "run".to_owned(),
                node_id: format!("node-{name}"),
            },
            name,
            format!("crate::{name}"),
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
            certainty: ResolutionCertainty::Certain,
            callsite_identity: format!("{index}:{}:0:{}|rust", index + 1, index + 2),
            containment: CallableContainmentEvidence {
                file_node_id: NodeId(i64::try_from(index + 1).unwrap()),
                owner_node_id: NodeId(i64::try_from(index + 10).unwrap()),
                start_line: u32::try_from(index + 1).unwrap(),
                end_line: u32::try_from(index + 1).unwrap(),
            },
            line_window: IndexedLineWindow {
                kind: "indexed_line_v1",
                project_file_components: vec!["src".to_owned(), format!("step{index}.rs")],
                indexed_sha256: format!("indexed-{index}"),
                observed_sha256: format!("indexed-{index}"),
                anchor_line: u32::try_from(index + 1).unwrap(),
                byte_start: index * 10,
                byte_end: index * 10 + text.len(),
                text,
            },
        }
    }

    fn six_step_projection_fixture(
        texts: Vec<String>,
    ) -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
        Vec<IndexedCallEdgeReceipt>,
        ProofDisposition,
    ) {
        assert_eq!(texts.len(), 6);
        let names = ["A", "B", "C", "D", "E", "F", "G"];
        let (contract, hashes, rendering) = validate_for_projection(&names[1..]);
        let mut receipts = texts
            .into_iter()
            .enumerate()
            .map(|(index, text)| indexed_receipt(index, names[index], names[index + 1], text))
            .collect::<Vec<_>>();
        for receipt in &mut receipts {
            receipt.line_window.byte_start = 0;
            receipt.line_window.byte_end = MAX_SOURCE_RECEIPT_LINE_BYTES;
        }
        let disposition = ProofDisposition::ContractProven {
            contract_digest: hashes.contract_digest().to_owned(),
            receipts: receipts
                .iter()
                .map(|receipt| receipt.receipt.clone())
                .collect(),
        };
        (contract, hashes, rendering, receipts, disposition)
    }

    #[test]
    fn internal_projection_is_a_complete_tagged_root_with_every_authoritative_receipt() {
        let (contract, hashes, rendering) = validate_for_projection(&["B"]);
        let receipts = vec![indexed_receipt(0, "A", "B", "A calls B();\n".to_owned())];
        let disposition = ProofDisposition::ContractProven {
            contract_digest: hashes.contract_digest().to_owned(),
            receipts: vec![receipts[0].receipt.clone()],
        };
        let projected = project_internal_call_path_result(InternalProjectionInput {
            contract: &contract,
            hashes: &hashes,
            rendering: &rendering,
            publication: InternalCorePublicationIdentity {
                project_id: "project".to_owned(),
                generation_id: "generation".to_owned(),
                run_id: "run".to_owned(),
            },
            disposition: &disposition,
            receipts: &receipts,
        })
        .unwrap();

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
        assert_eq!(root["receipts"].as_array().unwrap().len(), 1);
        assert_eq!(root["steps"].as_array().unwrap().len(), 1);
        assert_eq!(serialized_size, serde_json::to_vec(&root).unwrap().len());
    }

    #[test]
    fn internal_projection_accepts_exactly_64_kib_and_replaces_64_kib_plus_one_whole() {
        let (contract, hashes, rendering, mut receipts, mut disposition) =
            six_step_projection_fixture(vec![String::new(); 6]);
        let baseline = project_internal_call_path_result_with_cap(
            InternalProjectionInput {
                contract: &contract,
                hashes: &hashes,
                rendering: &rendering,
                publication: InternalCorePublicationIdentity {
                    project_id: "project".to_owned(),
                    generation_id: "generation".to_owned(),
                    run_id: "run".to_owned(),
                },
                disposition: &disposition,
                receipts: &receipts,
            },
            usize::MAX,
        )
        .unwrap();
        let InternalProjection::Complete {
            serialized_size: baseline_size,
            ..
        } = baseline
        else {
            panic!("unbounded baseline must be complete")
        };
        let mut remaining = INTERNAL_PROOF_CAP_BYTES - baseline_size;
        for receipt in &mut receipts {
            let quote_count = (remaining / 2).min(MAX_SOURCE_RECEIPT_LINE_BYTES);
            receipt.line_window.text.push_str(&"\"".repeat(quote_count));
            remaining -= quote_count * 2;
            if remaining == 1 && receipt.line_window.text.len() < MAX_SOURCE_RECEIPT_LINE_BYTES {
                receipt.line_window.text.push('x');
                remaining = 0;
            }
        }
        assert_eq!(remaining, 0, "six bounded lines can reach the exact cap");

        let exact = project_internal_call_path_result(InternalProjectionInput {
            contract: &contract,
            hashes: &hashes,
            rendering: &rendering,
            publication: InternalCorePublicationIdentity {
                project_id: "project".to_owned(),
                generation_id: "generation".to_owned(),
                run_id: "run".to_owned(),
            },
            disposition: &disposition,
            receipts: &receipts,
        })
        .unwrap();
        assert!(matches!(
            exact,
            InternalProjection::Complete {
                serialized_size: INTERNAL_PROOF_CAP_BYTES,
                ..
            }
        ));

        receipts
            .iter_mut()
            .find(|receipt| receipt.line_window.text.len() < MAX_SOURCE_RECEIPT_LINE_BYTES)
            .expect("one line retains room for the cap+1 byte")
            .line_window
            .text
            .push('x');
        let required_receipts = receipts
            .iter()
            .map(|receipt| receipt.receipt.clone())
            .collect::<Vec<_>>();
        let ProofDisposition::ContractProven {
            receipts: disposition_receipts,
            ..
        } = &mut disposition
        else {
            unreachable!()
        };
        *disposition_receipts = required_receipts;
        let plus_one = project_internal_call_path_result(InternalProjectionInput {
            contract: &contract,
            hashes: &hashes,
            rendering: &rendering,
            publication: InternalCorePublicationIdentity {
                project_id: "project".to_owned(),
                generation_id: "generation".to_owned(),
                run_id: "run".to_owned(),
            },
            disposition: &disposition,
            receipts: &receipts,
        })
        .unwrap();
        let InternalProjection::BudgetExceeded {
            root,
            required_complete_size,
            serialized_size,
        } = plus_one
        else {
            panic!("cap+1 must replace the complete result")
        };
        assert_eq!(required_complete_size, INTERNAL_PROOF_CAP_BYTES + 1);
        assert!(serialized_size < INTERNAL_PROOF_CAP_BYTES);
        assert_eq!(root["kind"], "budget_exceeded");
        assert_eq!(root["disposition"]["kind"], "unknown");
        assert_eq!(
            root["disposition"]["gaps"],
            json!([{ "kind": "output_budget_exceeded" }])
        );
        for forbidden in ["spec", "clauses", "steps", "receipts"] {
            assert!(
                root.get(forbidden).is_none(),
                "fallback carried {forbidden}"
            );
        }
    }

    #[test]
    fn internal_projection_measures_escaping_and_never_transports_partial_receipts() {
        let escape_unit = "\"\\\u{0000}é";
        let escape_heavy = escape_unit.repeat(MAX_SOURCE_RECEIPT_LINE_BYTES / escape_unit.len());
        assert!(escape_heavy.len() <= MAX_SOURCE_RECEIPT_LINE_BYTES);
        assert!(escape_heavy.contains('é'));
        let (contract, hashes, rendering, receipts, disposition) =
            six_step_projection_fixture(vec![escape_heavy; 6]);
        let projected = project_internal_call_path_result(InternalProjectionInput {
            contract: &contract,
            hashes: &hashes,
            rendering: &rendering,
            publication: InternalCorePublicationIdentity {
                project_id: "project".to_owned(),
                generation_id: "generation".to_owned(),
                run_id: "run".to_owned(),
            },
            disposition: &disposition,
            receipts: &receipts,
        })
        .unwrap();
        let InternalProjection::BudgetExceeded { root, .. } = projected else {
            panic!("escaped receipt JSON must be measured after escaping")
        };
        assert!(root.get("receipts").is_none());
        assert!(root.get("steps").is_none());

        let mut missing = receipts.clone();
        let omitted = missing.pop().unwrap();
        assert_eq!(
            project_internal_call_path_result(InternalProjectionInput {
                contract: &contract,
                hashes: &hashes,
                rendering: &rendering,
                publication: InternalCorePublicationIdentity {
                    project_id: "project".to_owned(),
                    generation_id: "generation".to_owned(),
                    run_id: "run".to_owned(),
                },
                disposition: &disposition,
                receipts: &missing,
            }),
            Err(InternalProjectionError::MissingAuthoritativeReceipt {
                receipt_id: omitted.receipt.receipt_id,
                edge_id: omitted.receipt.edge_id,
            })
        );
    }

    #[test]
    fn six_maximum_lines_are_kept_complete_or_replaced_as_one_and_tiny_fallbacks_error() {
        let maximum = "x".repeat(MAX_SOURCE_RECEIPT_LINE_BYTES);
        let (contract, hashes, rendering, receipts, disposition) =
            six_step_projection_fixture(vec![maximum; 6]);
        let projected = project_internal_call_path_result(InternalProjectionInput {
            contract: &contract,
            hashes: &hashes,
            rendering: &rendering,
            publication: InternalCorePublicationIdentity {
                project_id: "project".to_owned(),
                generation_id: "generation".to_owned(),
                run_id: "run".to_owned(),
            },
            disposition: &disposition,
            receipts: &receipts,
        })
        .unwrap();
        match projected {
            InternalProjection::Complete {
                root,
                serialized_size,
            } => {
                assert!(serialized_size <= INTERNAL_PROOF_CAP_BYTES);
                assert_eq!(root["receipts"].as_array().unwrap().len(), 6);
            }
            InternalProjection::BudgetExceeded { root, .. } => {
                assert!(root.get("receipts").is_none());
                assert!(root.get("steps").is_none());
            }
        }

        assert!(matches!(
            project_internal_call_path_result_with_cap(
                InternalProjectionInput {
                    contract: &contract,
                    hashes: &hashes,
                    rendering: &rendering,
                    publication: InternalCorePublicationIdentity {
                        project_id: "project".to_owned(),
                        generation_id: "generation".to_owned(),
                        run_id: "run".to_owned(),
                    },
                    disposition: &disposition,
                    receipts: &receipts,
                },
                1,
            ),
            Err(InternalProjectionError::FallbackExceedsCap { cap_bytes: 1, .. })
        ));
    }

    #[test]
    fn validates_a_fully_anchored_one_step_direct_call_contract() {
        assert!(matches!(
            validate_contract(valid_input(&["B"])),
            Ok(ValidationOutcome::Validated { .. })
        ));
    }

    #[test]
    fn raw_call_edge_admission_requires_the_persisted_certain_canonical_identity() {
        let edge = Edge {
            id: EdgeId(41),
            source: NodeId(7),
            target: NodeId(19),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(-3)),
            line: Some(12),
            resolved_source: Some(NodeId(11)),
            resolved_target: Some(NodeId(23)),
            confidence: Some(1.0),
            certainty: Some(ResolutionCertainty::Certain),
            callsite_identity: Some("-3:12:0:19|collector-marker".to_owned()),
            candidate_targets: Vec::new(),
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

        let mut probable = edge;
        probable.certainty = Some(ResolutionCertainty::Probable);
        assert_eq!(
            admit_raw_call_edge(&probable, NodeId(11), NodeId(23)),
            RawCallEdgeAdmission::Rejected
        );
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
            confidence: Some(1.0),
            certainty: Some(ResolutionCertainty::Certain),
            callsite_identity: Some("-3:12:0:19|collector-marker".to_owned()),
            candidate_targets: Vec::new(),
        };
        let mut mutations: Vec<EdgeMutation> = vec![
            ("wrong kind", Box::new(|edge| edge.kind = EdgeKind::USAGE)),
            ("missing certainty", Box::new(|edge| edge.certainty = None)),
            (
                "probable certainty",
                Box::new(|edge| edge.certainty = Some(ResolutionCertainty::Probable)),
            ),
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
