//! Closed contracts for the internal exact call-resolution projection.
//!
//! These facts are an additional proof authorization overlay on the ordinary
//! graph. They are not navigation edges and are not exposed by product DTOs.

use crate::graph::{EdgeId, NodeId};
use serde::{Deserialize, Serialize};

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
