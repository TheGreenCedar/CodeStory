//! Closed contracts for the internal exact call-resolution projection.
//!
//! These facts are an additional proof authorization overlay on the ordinary
//! graph. They are not navigation edges and are not exposed by product DTOs.

use crate::graph::{EdgeId, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PROOF_RESOLUTION_FACT_SCHEMA_VERSION: u32 = 1;
pub const INTERNAL_RESOLUTION_PRODUCER: &str = "codestory-internal";
pub const EXACT_CALL_RESOLUTION_ALGORITHM: &str = "exact-call-resolution-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FileId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalleeForm {
    Identifier,
    NamedImport,
    QualifiedPath,
    ExplicitReceiver,
    ImplicitReceiver,
    Constructor,
    DynamicAccess,
}

impl CalleeForm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identifier => "identifier",
            Self::NamedImport => "named_import",
            Self::QualifiedPath => "qualified_path",
            Self::ExplicitReceiver => "explicit_receiver",
            Self::ImplicitReceiver => "implicit_receiver",
            Self::Constructor => "constructor",
            Self::DynamicAccess => "dynamic_access",
        }
    }

    pub fn from_label(value: &str) -> Option<Self> {
        match value {
            "identifier" => Some(Self::Identifier),
            "named_import" => Some(Self::NamedImport),
            "qualified_path" => Some(Self::QualifiedPath),
            "explicit_receiver" => Some(Self::ExplicitReceiver),
            "implicit_receiver" => Some(Self::ImplicitReceiver),
            "constructor" => Some(Self::Constructor),
            "dynamic_access" => Some(Self::DynamicAccess),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExactCallsite {
    pub file_id: FileId,
    pub source_sha256: String,
    pub start_byte: u64,
    pub end_byte_exclusive: u64,
    pub line: u32,
    pub column: u32,
    pub callee_form: CalleeForm,
    pub raw_target: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ExactSyntaxCallsiteCorrelationInput<'a> {
    pub file_id: FileId,
    pub line: u32,
    pub start_byte: u64,
    pub end_byte_exclusive: u64,
    pub column: u32,
    pub caller: NodeId,
    pub target: NodeId,
    pub raw_target: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct OrdinaryCallEdgeCorrelationInput<'a> {
    pub file_id: Option<FileId>,
    pub line: Option<u32>,
    pub caller: NodeId,
    pub target: NodeId,
    pub raw_edge_target: NodeId,
    pub raw_file_id: Option<FileId>,
    pub raw_line: Option<u32>,
    pub raw_target: &'a str,
    pub callsite_identity: Option<&'a str>,
    pub semantic_exact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactCallsiteCorrelationFailure {
    Missing,
    Ambiguous,
}

#[derive(Debug, Clone, Copy)]
struct CanonicalCallsiteIdentity {
    file_id: FileId,
    line: u32,
    column_or_ordinal: u32,
    raw_target: NodeId,
}

fn parse_canonical_callsite_identity(identity: &str) -> Option<CanonicalCallsiteIdentity> {
    let mut fields = identity.split('|').next()?.split(':');
    let parsed = CanonicalCallsiteIdentity {
        file_id: FileId(fields.next()?.parse().ok()?),
        line: fields.next()?.parse().ok()?,
        column_or_ordinal: fields.next()?.parse().ok()?,
        raw_target: NodeId(fields.next()?.parse().ok()?),
    };
    (fields.next().is_none() && parsed.column_or_ordinal > 0).then_some(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CorrelationGroupKey<'a> {
    file_id: FileId,
    line: u32,
    caller: NodeId,
    target: NodeId,
    raw_target: &'a str,
}

/// Correlate parser-derived exact syntax claims with unchanged ordinary CALL
/// edges. Each returned edge index belongs to the corresponding syntax input.
/// The third canonical identity component is admitted only when the complete
/// group proves one consistent column mapping or one contiguous ordinal map.
pub fn correlate_exact_syntax_callsites(
    syntax: &[ExactSyntaxCallsiteCorrelationInput<'_>],
    edges: &[OrdinaryCallEdgeCorrelationInput<'_>],
) -> Vec<Result<usize, ExactCallsiteCorrelationFailure>> {
    let mut results = vec![Err(ExactCallsiteCorrelationFailure::Missing); syntax.len()];
    let mut syntax_groups = BTreeMap::<CorrelationGroupKey<'_>, Vec<usize>>::new();
    for (index, input) in syntax.iter().enumerate() {
        syntax_groups
            .entry(CorrelationGroupKey {
                file_id: input.file_id,
                line: input.line,
                caller: input.caller,
                target: input.target,
                raw_target: input.raw_target,
            })
            .or_default()
            .push(index);
    }
    let mut edge_groups = BTreeMap::<CorrelationGroupKey<'_>, BTreeSet<usize>>::new();
    for (index, edge) in edges.iter().enumerate() {
        let mut coordinates = BTreeSet::new();
        if let (Some(file_id), Some(line)) = (edge.file_id, edge.line) {
            coordinates.insert((file_id, line));
        }
        if let (Some(file_id), Some(line)) = (edge.raw_file_id, edge.raw_line) {
            coordinates.insert((file_id, line));
        }
        if let Some(identity) = edge
            .callsite_identity
            .and_then(parse_canonical_callsite_identity)
        {
            coordinates.insert((identity.file_id, identity.line));
        }
        for (file_id, line) in coordinates {
            edge_groups
                .entry(CorrelationGroupKey {
                    file_id,
                    line,
                    caller: edge.caller,
                    target: edge.target,
                    raw_target: edge.raw_target,
                })
                .or_default()
                .insert(index);
        }
    }

    for (key, mut syntax_indices) in syntax_groups {
        syntax_indices.sort_by_key(|index| {
            let input = syntax[*index];
            (input.start_byte, input.end_byte_exclusive)
        });
        let edge_indices = edge_groups
            .get(&key)
            .into_iter()
            .flat_map(|indices| indices.iter().copied())
            .collect::<Vec<_>>();
        let mut raw_groups = BTreeMap::<NodeId, Vec<usize>>::new();
        for edge_index in &edge_indices {
            raw_groups
                .entry(edges[*edge_index].raw_edge_target)
                .or_default()
                .push(*edge_index);
        }
        let mut valid_mappings = Vec::new();
        let mut ambiguous_invalid_mapping = edge_indices.len() > syntax_indices.len();
        for (raw_target, raw_indices) in raw_groups {
            if raw_indices.len() != syntax_indices.len()
                || raw_indices
                    .iter()
                    .any(|index| !edges[*index].semantic_exact)
            {
                continue;
            }
            let parsed = raw_indices
                .iter()
                .map(|index| {
                    let edge = edges[*index];
                    let identity = parse_canonical_callsite_identity(edge.callsite_identity?)?;
                    (identity.file_id == key.file_id
                        && identity.line == key.line
                        && identity.raw_target == raw_target
                        && edge.raw_file_id == Some(key.file_id)
                        && edge.raw_line == Some(key.line))
                    .then_some((identity.column_or_ordinal, *index))
                })
                .collect::<Option<Vec<_>>>();
            let Some(parsed) = parsed else {
                continue;
            };
            let distinct_values = parsed
                .iter()
                .map(|(value, _)| *value)
                .collect::<BTreeSet<_>>();
            if distinct_values.len() != parsed.len() {
                ambiguous_invalid_mapping = true;
                continue;
            }
            let column_mapping = syntax_indices
                .iter()
                .map(|syntax_index| {
                    let column = syntax[*syntax_index].column;
                    parsed
                        .iter()
                        .find_map(|(value, edge_index)| (*value == column).then_some(*edge_index))
                })
                .collect::<Option<Vec<_>>>();
            let mut ordinal_mapping = parsed;
            ordinal_mapping.sort_by_key(|(value, _)| *value);
            let ordinal_mapping = ordinal_mapping
                .iter()
                .enumerate()
                .all(|(index, (value, _))| {
                    u32::try_from(index + 1).is_ok_and(|expected| *value == expected)
                })
                .then(|| {
                    ordinal_mapping
                        .into_iter()
                        .map(|(_, edge_index)| edge_index)
                        .collect::<Vec<_>>()
                });
            match (column_mapping, ordinal_mapping) {
                (Some(columns), Some(ordinals)) if columns == ordinals => {
                    valid_mappings.push(columns)
                }
                (Some(_), Some(_)) => ambiguous_invalid_mapping = true,
                (Some(columns), None) => valid_mappings.push(columns),
                (None, Some(ordinals)) => valid_mappings.push(ordinals),
                (None, None) => {}
            }
        }
        match valid_mappings.as_slice() {
            [mapping] if !ambiguous_invalid_mapping => {
                for (syntax_index, edge_index) in
                    syntax_indices.into_iter().zip(mapping.iter().copied())
                {
                    results[syntax_index] = Ok(edge_index);
                }
            }
            _ if valid_mappings.len() > 1 || ambiguous_invalid_mapping => {
                for syntax_index in syntax_indices {
                    results[syntax_index] = Err(ExactCallsiteCorrelationFailure::Ambiguous);
                }
            }
            _ => {}
        }
    }
    results
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofResolutionStatus {
    Exact,
    Ambiguous,
    Unsupported,
    MissingBinding,
    IncompleteDomain,
}

impl ProofResolutionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Ambiguous => "ambiguous",
            Self::Unsupported => "unsupported",
            Self::MissingBinding => "missing_binding",
            Self::IncompleteDomain => "incomplete_domain",
        }
    }

    pub fn from_label(value: &str) -> Option<Self> {
        match value {
            "exact" => Some(Self::Exact),
            "ambiguous" => Some(Self::Ambiguous),
            "unsupported" => Some(Self::Unsupported),
            "missing_binding" => Some(Self::MissingBinding),
            "incomplete_domain" => Some(Self::IncompleteDomain),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofResolutionReason {
    ExactResolution,
    MultipleBindings,
    UnsupportedConstruct,
    MissingBinding,
    LookupDomainIncomplete,
}

impl ProofResolutionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ExactResolution => "exact_resolution",
            Self::MultipleBindings => "multiple_bindings",
            Self::UnsupportedConstruct => "unsupported_construct",
            Self::MissingBinding => "missing_binding",
            Self::LookupDomainIncomplete => "lookup_domain_incomplete",
        }
    }

    pub fn from_label(value: &str) -> Option<Self> {
        match value {
            "exact_resolution" => Some(Self::ExactResolution),
            "multiple_bindings" => Some(Self::MultipleBindings),
            "unsupported_construct" => Some(Self::UnsupportedConstruct),
            "missing_binding" => Some(Self::MissingBinding),
            "lookup_domain_incomplete" => Some(Self::LookupDomainIncomplete),
            _ => None,
        }
    }

    pub fn matches_status(self, status: ProofResolutionStatus) -> bool {
        matches!(
            (self, status),
            (Self::ExactResolution, ProofResolutionStatus::Exact)
                | (Self::MultipleBindings, ProofResolutionStatus::Ambiguous)
                | (
                    Self::UnsupportedConstruct,
                    ProofResolutionStatus::Unsupported
                )
                | (Self::MissingBinding, ProofResolutionStatus::MissingBinding)
                | (
                    Self::LookupDomainIncomplete,
                    ProofResolutionStatus::IncompleteDomain
                )
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionEvidenceKind {
    SameFileDeclaration,
    SamePackageDeclaration,
    StaticImportBinding,
    QualifiedPath,
    ExplicitReceiverType,
    ConstructorBinding,
    ImplicitReceiver,
}

impl ResolutionEvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SameFileDeclaration => "same_file_declaration",
            Self::SamePackageDeclaration => "same_package_declaration",
            Self::StaticImportBinding => "static_import_binding",
            Self::QualifiedPath => "qualified_path",
            Self::ExplicitReceiverType => "explicit_receiver_type",
            Self::ConstructorBinding => "constructor_binding",
            Self::ImplicitReceiver => "implicit_receiver",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionEvidence {
    SameFileDeclaration { declaration: NodeId },
    SamePackageDeclaration { declaration: NodeId },
    StaticImportBinding { import: NodeId, declaration: NodeId },
    QualifiedPath { components: Vec<NodeId> },
    ExplicitReceiverType { receiver_type: NodeId },
    ConstructorBinding { constructor: NodeId },
    ImplicitReceiver { owner: NodeId },
}

impl ResolutionEvidence {
    pub fn kind(&self) -> ResolutionEvidenceKind {
        match self {
            Self::SameFileDeclaration { .. } => ResolutionEvidenceKind::SameFileDeclaration,
            Self::SamePackageDeclaration { .. } => ResolutionEvidenceKind::SamePackageDeclaration,
            Self::StaticImportBinding { .. } => ResolutionEvidenceKind::StaticImportBinding,
            Self::QualifiedPath { .. } => ResolutionEvidenceKind::QualifiedPath,
            Self::ExplicitReceiverType { .. } => ResolutionEvidenceKind::ExplicitReceiverType,
            Self::ConstructorBinding { .. } => ResolutionEvidenceKind::ConstructorBinding,
            Self::ImplicitReceiver { .. } => ResolutionEvidenceKind::ImplicitReceiver,
        }
    }

    pub fn node_ids(&self) -> Vec<NodeId> {
        match self {
            Self::SameFileDeclaration { declaration }
            | Self::SamePackageDeclaration { declaration }
            | Self::ConstructorBinding {
                constructor: declaration,
            }
            | Self::ImplicitReceiver { owner: declaration }
            | Self::ExplicitReceiverType {
                receiver_type: declaration,
            } => vec![*declaration],
            Self::StaticImportBinding {
                import,
                declaration,
            } => vec![*import, *declaration],
            Self::QualifiedPath { components } => components.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DependencyFileHash {
    pub file_id: FileId,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionProvenance {
    pub producer: String,
    pub fact_schema_version: u32,
    pub algorithm: String,
    pub language_adapter: String,
    pub language_adapter_version: String,
    pub parser_fingerprint: String,
    pub dependency_file_hashes: Vec<DependencyFileHash>,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallResolutionFact {
    pub fact_id: String,
    pub edge_id: Option<EdgeId>,
    /// The unchanged raw CALL endpoint captured by the parser-span
    /// corroboration. This is deliberately distinct from `target`.
    pub raw_edge_target: Option<NodeId>,
    /// The unchanged canonical identity carried by the ordinary CALL edge.
    pub raw_callsite_identity: Option<String>,
    pub callsite: ExactCallsite,
    pub caller: NodeId,
    pub target: Option<NodeId>,
    pub status: ProofResolutionStatus,
    pub reason: ProofResolutionReason,
    pub evidence_chain: Vec<ResolutionEvidence>,
    pub lookup_domain_complete: bool,
    pub provenance: ResolutionProvenance,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProofResolutionAdapter {
    pub language: String,
    pub adapter_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofResolutionFunnelCounts {
    pub syntax_calls: u64,
    pub adapter_supported: u64,
    pub exact: u64,
    pub ambiguous: u64,
    pub missing_binding: u64,
    pub incomplete_domain: u64,
    pub unsupported: u64,
    pub exact_call_linked: u64,
    pub proof_shape_admitted: u64,
    pub authoritative_receipts: u64,
    pub complete_proofs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofResolutionFunnelRow {
    pub language: String,
    pub callee_form: Option<CalleeForm>,
    pub evidence_kind: Option<ResolutionEvidenceKind>,
    pub counts: ProofResolutionFunnelCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofResolutionProjection {
    pub adapter_roster: Vec<ProofResolutionAdapter>,
    pub facts: Vec<CallResolutionFact>,
    pub funnel: Vec<ProofResolutionFunnelRow>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn syntax() -> Vec<ExactSyntaxCallsiteCorrelationInput<'static>> {
        vec![
            ExactSyntaxCallsiteCorrelationInput {
                file_id: FileId(1),
                line: 2,
                start_byte: 29,
                end_byte_exclusive: 35,
                column: 15,
                caller: NodeId(10),
                target: NodeId(20),
                raw_target: "target",
            },
            ExactSyntaxCallsiteCorrelationInput {
                file_id: FileId(1),
                line: 2,
                start_byte: 39,
                end_byte_exclusive: 45,
                column: 25,
                caller: NodeId(10),
                target: NodeId(20),
                raw_target: "target",
            },
        ]
    }

    fn edge(identity: &'static str) -> OrdinaryCallEdgeCorrelationInput<'static> {
        OrdinaryCallEdgeCorrelationInput {
            file_id: Some(FileId(1)),
            line: Some(2),
            caller: NodeId(10),
            target: NodeId(20),
            raw_edge_target: NodeId(30),
            raw_file_id: Some(FileId(1)),
            raw_line: Some(2),
            raw_target: "target",
            callsite_identity: Some(identity),
            semantic_exact: true,
        }
    }

    fn assert_all_fail(
        syntax: &[ExactSyntaxCallsiteCorrelationInput<'_>],
        edges: &[OrdinaryCallEdgeCorrelationInput<'_>],
    ) {
        assert!(
            correlate_exact_syntax_callsites(syntax, edges)
                .iter()
                .all(Result::is_err)
        );
    }

    #[test]
    fn exact_callsite_correlation_accepts_only_complete_column_or_ordinal_domains() {
        let syntax = syntax();
        assert_eq!(
            correlate_exact_syntax_callsites(&syntax, &[edge("1:2:1:30"), edge("1:2:2:30")],),
            [Ok(0), Ok(1)]
        );
        assert_eq!(
            correlate_exact_syntax_callsites(&syntax, &[edge("1:2:15:30"), edge("1:2:25:30")],),
            [Ok(0), Ok(1)]
        );

        for edges in [
            vec![edge("1:2:1:30"), edge("1:2:1:30")],
            vec![edge("1:2:1:30"), edge("1:2:3:30")],
            vec![edge("opaque"), edge("1:2:2:30")],
            vec![edge("9:2:1:30"), edge("1:2:2:30")],
            vec![edge("1:9:1:30"), edge("1:2:2:30")],
            vec![edge("1:2:1:99"), edge("1:2:2:30")],
        ] {
            assert_all_fail(&syntax, &edges);
        }

        let mut candidates = [edge("1:2:1:30"), edge("1:2:2:30")];
        candidates[1].semantic_exact = false;
        assert_all_fail(&syntax, &candidates);

        let mut wrong_source = [edge("1:2:1:30"), edge("1:2:2:30")];
        wrong_source[1].caller = NodeId(11);
        assert_all_fail(&syntax, &wrong_source);
        let mut wrong_target = [edge("1:2:1:30"), edge("1:2:2:30")];
        wrong_target[1].target = NodeId(21);
        assert_all_fail(&syntax, &wrong_target);

        let mut extra_edge = vec![edge("1:2:1:30"), edge("1:2:2:30")];
        extra_edge.push(edge("1:2:3:30"));
        assert_all_fail(&syntax, &extra_edge);
        let mut extra_input = syntax.clone();
        extra_input.push(ExactSyntaxCallsiteCorrelationInput {
            start_byte: 49,
            end_byte_exclusive: 55,
            column: 35,
            ..syntax[0]
        });
        assert_all_fail(&extra_input, &[edge("1:2:1:30"), edge("1:2:2:30")]);

        let mut second_group_one = edge("1:2:1:31");
        second_group_one.raw_edge_target = NodeId(31);
        let mut second_group_two = edge("1:2:2:31");
        second_group_two.raw_edge_target = NodeId(31);
        assert_all_fail(
            &syntax,
            &[
                edge("1:2:1:30"),
                edge("1:2:2:30"),
                second_group_one,
                second_group_two,
            ],
        );

        let mut malformed_extra_group = edge("opaque");
        malformed_extra_group.raw_edge_target = NodeId(31);
        assert_all_fail(
            &syntax,
            &[edge("1:2:1:30"), edge("1:2:2:30"), malformed_extra_group],
        );

        let mut missing_edge_coordinates = edge("1:2:3:31");
        missing_edge_coordinates.file_id = None;
        missing_edge_coordinates.line = None;
        missing_edge_coordinates.raw_edge_target = NodeId(31);
        assert_all_fail(
            &syntax,
            &[edge("1:2:1:30"), edge("1:2:2:30"), missing_edge_coordinates],
        );

        let disagreeing_syntax = [
            ExactSyntaxCallsiteCorrelationInput {
                column: 2,
                ..syntax[0]
            },
            ExactSyntaxCallsiteCorrelationInput {
                column: 1,
                ..syntax[1]
            },
        ];
        assert_all_fail(&disagreeing_syntax, &[edge("1:2:1:30"), edge("1:2:2:30")]);
    }
}
