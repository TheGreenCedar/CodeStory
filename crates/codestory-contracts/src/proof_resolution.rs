//! Closed contracts for the internal exact call-resolution projection.
//!
//! These facts are an additional proof authorization overlay on the ordinary
//! graph. They are not navigation edges and are not exposed by product DTOs.

use crate::graph::{EdgeId, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Canonical four-field prefix of a stored callsite identity.
pub struct CanonicalCallsiteIdentity {
    pub file_id: FileId,
    pub line: u32,
    pub column_or_ordinal: u32,
    pub raw_target: NodeId,
}

/// Parses an exact canonical prefix while leaving any non-empty marker suffix opaque.
pub fn parse_canonical_callsite_identity(identity: &str) -> Option<CanonicalCallsiteIdentity> {
    let (prefix, marker) = identity
        .split_once('|')
        .map_or((identity, None), |(prefix, marker)| (prefix, Some(marker)));
    if marker.is_some_and(str::is_empty) {
        return None;
    }
    let mut fields = prefix.split(':');
    let parsed = CanonicalCallsiteIdentity {
        file_id: FileId(fields.next()?.parse().ok()?),
        line: fields.next()?.parse().ok()?,
        column_or_ordinal: fields.next()?.parse().ok()?,
        raw_target: NodeId(fields.next()?.parse().ok()?),
    };
    (fields.next().is_none()
        && parsed.line > 0
        && parsed.column_or_ordinal > 0
        && format!(
            "{}:{}:{}:{}",
            parsed.file_id.0, parsed.line, parsed.column_or_ordinal, parsed.raw_target.0
        ) == prefix)
        .then_some(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CorrelationGroupKey<'a> {
    file_id: FileId,
    line: u32,
    caller: NodeId,
    target: NodeId,
    raw_target: &'a str,
}

#[derive(Default)]
struct PreparedRawEdgeGroup {
    edge_count: usize,
    invalid: bool,
    by_discriminator: HashMap<u32, usize>,
}

#[derive(Default)]
struct PreparedEdgeGroup {
    edge_count: usize,
    raw_groups: HashMap<NodeId, PreparedRawEdgeGroup>,
}

#[cfg(test)]
thread_local! {
    static CORRELATION_WORK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
fn count_correlation_work(amount: usize) {
    #[cfg(test)]
    CORRELATION_WORK.with(|work| work.set(work.get().saturating_add(amount)));
    #[cfg(not(test))]
    let _ = amount;
}

#[cfg(test)]
fn reset_correlation_work() {
    CORRELATION_WORK.with(|work| work.set(0));
}

#[cfg(test)]
fn correlation_work() -> usize {
    CORRELATION_WORK.with(std::cell::Cell::get)
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
    let mut syntax_groups = HashMap::<CorrelationGroupKey<'_>, Vec<usize>>::new();
    for (index, input) in syntax.iter().enumerate() {
        count_correlation_work(1);
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
    let mut edge_groups = HashMap::<CorrelationGroupKey<'_>, PreparedEdgeGroup>::new();
    for (index, edge) in edges.iter().enumerate() {
        count_correlation_work(1);
        let parsed = edge
            .callsite_identity
            .and_then(parse_canonical_callsite_identity);
        let mut coordinates = [None; 3];
        if let (Some(file_id), Some(line)) = (edge.file_id, edge.line) {
            coordinates[0] = Some((file_id, line));
        }
        if let (Some(file_id), Some(line)) = (edge.raw_file_id, edge.raw_line) {
            coordinates[1] = Some((file_id, line));
        }
        if let Some(identity) = parsed {
            coordinates[2] = Some((identity.file_id, identity.line));
        }
        for coordinate_index in 0..coordinates.len() {
            let Some((file_id, line)) = coordinates[coordinate_index] else {
                continue;
            };
            if coordinates[..coordinate_index]
                .iter()
                .any(|coordinate| *coordinate == Some((file_id, line)))
            {
                continue;
            }
            count_correlation_work(1);
            let group = edge_groups
                .entry(CorrelationGroupKey {
                    file_id,
                    line,
                    caller: edge.caller,
                    target: edge.target,
                    raw_target: edge.raw_target,
                })
                .or_default();
            group.edge_count += 1;
            let raw_group = group.raw_groups.entry(edge.raw_edge_target).or_default();
            raw_group.edge_count += 1;
            let Some(identity) = parsed.filter(|identity| {
                identity.file_id == file_id
                    && identity.line == line
                    && identity.raw_target == edge.raw_edge_target
                    && edge.raw_file_id == Some(file_id)
                    && edge.raw_line == Some(line)
                    && edge.semantic_exact
            }) else {
                raw_group.invalid = true;
                continue;
            };
            if raw_group
                .by_discriminator
                .insert(identity.column_or_ordinal, index)
                .is_some()
            {
                raw_group.invalid = true;
            }
        }
    }

    for (key, syntax_indices) in syntax_groups {
        count_correlation_work(1);
        if syntax_indices.windows(2).any(|pair| {
            let left = syntax[pair[0]];
            let right = syntax[pair[1]];
            (left.start_byte, left.end_byte_exclusive)
                >= (right.start_byte, right.end_byte_exclusive)
        }) {
            continue;
        }
        let Some(edge_group) = edge_groups.get(&key) else {
            continue;
        };
        let mut valid_mappings = Vec::new();
        let mut ambiguous_invalid_mapping = edge_group.edge_count > syntax_indices.len();
        for raw_group in edge_group.raw_groups.values() {
            count_correlation_work(1);
            if raw_group.edge_count != syntax_indices.len() || raw_group.invalid {
                continue;
            }
            let column_mapping = syntax_indices
                .iter()
                .map(|syntax_index| {
                    count_correlation_work(1);
                    raw_group
                        .by_discriminator
                        .get(&syntax[*syntax_index].column)
                        .copied()
                })
                .collect::<Option<Vec<_>>>();
            let ordinal_mapping = (1..=syntax_indices.len())
                .map(|ordinal| {
                    count_correlation_work(1);
                    u32::try_from(ordinal)
                        .ok()
                        .and_then(|ordinal| raw_group.by_discriminator.get(&ordinal).copied())
                })
                .collect::<Option<Vec<_>>>();
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

    #[test]
    fn canonical_callsite_identity_rejects_noncanonical_spellings() {
        assert_eq!(
            parse_canonical_callsite_identity("-1:2:3:-4|syntax:rust"),
            Some(CanonicalCallsiteIdentity {
                file_id: FileId(-1),
                line: 2,
                column_or_ordinal: 3,
                raw_target: NodeId(-4),
            })
        );
        for identity in [
            "01:2:3:4",
            "+1:2:3:4",
            "-0:2:3:4",
            "1:02:3:4",
            "1:+2:3:4",
            "1:0:3:4",
            "1:2:03:4",
            "1:2:+3:4",
            "1:2:0:4",
            "1:2:3:04",
            "1:2:3:+4",
            "1:2:3:-0",
            "1:2:4294967296:4",
            "9223372036854775808:2:3:4",
            "1:2:3:9223372036854775808",
            "1:2:3",
            "1:2:3:4:5",
            "|marker",
            "opaque",
            "1:2:3:4|",
        ] {
            assert_eq!(
                parse_canonical_callsite_identity(identity),
                None,
                "accepted noncanonical identity {identity:?}"
            );
        }
    }

    fn measured_same_line_correlation(count: usize, columns: bool) -> usize {
        let syntax = (0..count)
            .map(|index| ExactSyntaxCallsiteCorrelationInput {
                file_id: FileId(1),
                line: 2,
                start_byte: u64::try_from(index * 10).expect("test byte"),
                end_byte_exclusive: u64::try_from(index * 10 + 6).expect("test byte"),
                column: u32::try_from(100 + index * 2).expect("test column"),
                caller: NodeId(10),
                target: NodeId(20),
                raw_target: "target",
            })
            .collect::<Vec<_>>();
        let identities = (0..count)
            .rev()
            .map(|index| {
                let discriminator = if columns {
                    syntax[index].column
                } else {
                    u32::try_from(index + 1).expect("test ordinal")
                };
                format!("1:2:{discriminator}:30")
            })
            .collect::<Vec<_>>();
        let edges = identities
            .iter()
            .map(|identity| OrdinaryCallEdgeCorrelationInput {
                callsite_identity: Some(identity),
                ..edge("1:2:1:30")
            })
            .collect::<Vec<_>>();
        reset_correlation_work();
        let result = correlate_exact_syntax_callsites(&syntax, &edges);
        assert!(result.iter().all(Result::is_ok), "{result:?}");
        correlation_work()
    }

    #[test]
    fn same_line_column_and_ordinal_correlation_work_is_linear_with_reversed_edges() {
        for columns in [false, true] {
            let small = measured_same_line_correlation(64, columns);
            let large = measured_same_line_correlation(128, columns);
            assert!(small >= 64 * 4, "correlation work was not counted: {small}");
            assert!(
                large <= small * 2 + 16,
                "correlation work grew superlinearly: {small} -> {large}"
            );
        }
    }
}
