use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use codestory_runtime::proof_qualification_support as proof;

pub(crate) const PUBLIC_VERIFY_TOOL_NAME: &str = "verify_indexed_direct_calls";
pub(crate) const LEGACY_PROOF_TOOL_NAME: &str = "prove_call_path";
pub(crate) const PUBLIC_CALL_PATH_DOMAIN: &str = "call-path/v1";
pub(crate) const PROVE_CALL_PATH_INPUT_MAX_BYTES: usize = 64 * 1024;

pub(crate) fn is_proof_tool_name(name: &str) -> bool {
    matches!(name, PUBLIC_VERIFY_TOOL_NAME | LEGACY_PROOF_TOOL_NAME)
}

pub(crate) fn project_public_verification_result(internal: Value) -> Result<Value, String> {
    let object = internal
        .as_object()
        .ok_or_else(|| "proof projection root must be an object".to_owned())?;
    let mut public = object.clone();
    public.insert(
        "domain".to_owned(),
        Value::String(PUBLIC_CALL_PATH_DOMAIN.to_owned()),
    );
    let translation_status = public
        .remove("contract_interpretation")
        .unwrap_or_else(|| Value::String("host_supplied".to_owned()));
    public.insert("translation_status".to_owned(), translation_status);
    public.insert(
        "graph_disposition".to_owned(),
        Value::String(
            public
                .get("disposition")
                .and_then(|disposition| disposition.get("kind"))
                .and_then(Value::as_str)
                .map(graph_disposition_from_internal)
                .unwrap_or("unknown")
                .to_owned(),
        ),
    );
    public.insert("runtime_execution_proven".to_owned(), Value::Bool(false));
    Ok(Value::Object(public))
}

fn graph_disposition_from_internal(kind: &str) -> &'static str {
    match kind {
        "contract_proven" => "proven",
        "contract_refuted" => "refuted",
        _ => "unknown",
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProveCallPathRequestDto {
    source_text: String,
    clauses: Vec<ClauseAnchorDto>,
    spec: CallPathSpecDto,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClauseAnchorDto {
    clause_id: String,
    start_byte: u32,
    end_byte_exclusive: u32,
    quote: String,
    classification: ClauseClassificationDto,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::enum_variant_names)]
enum ClauseClassificationDto {
    ResolvedMaterial { fields: Vec<ProofContractFieldDto> },
    UnresolvedMaterial { reason: UnresolvedMaterialReasonDto },
    NonMaterial { reason: NonMaterialReasonDto },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ProofContractFieldDto {
    Start,
    StepTarget { step: u8 },
    Directness { step: u8 },
    Ordering { step: u8 },
    Relation { step: u8 },
    TraversalProhibition { index: u8 },
    ProjectionExclusion { index: u8 },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UnresolvedMaterialReasonDto {
    MissingSelectorResolution,
    AmbiguousSelectorResolution,
    UnsupportedInterpretation,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NonMaterialReasonDto {
    Whitespace,
    Punctuation,
    Connector,
    Commentary,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct CallPathSpecDto {
    start: ExactSymbolSelectorDto,
    steps: Vec<DirectCallStepDto>,
    prohibit_traversal_through: Vec<ExactScopeSelectorDto>,
    exclude_from_projection: Vec<ExactScopeSelectorDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectCallStepDto {
    target: ExactSymbolSelectorDto,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ExactSymbolSelectorDto {
    PinnedNode {
        project_id: String,
        core_generation_id: String,
        core_run_id: String,
        node_id: String,
    },
    CanonicalId {
        canonical_id: String,
    },
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ExactScopeSelectorDto {
    PinnedNode {
        project_id: String,
        core_generation_id: String,
        core_run_id: String,
        node_id: String,
    },
    CanonicalId {
        canonical_id: String,
    },
    QualifiedName {
        qualified_name: String,
        project_file_components: Option<Vec<String>>,
    },
}

impl ProveCallPathRequestDto {
    fn into_contract(self) -> proof::UnvalidatedCallPathContract {
        proof::UnvalidatedCallPathContract::new(
            self.source_text,
            self.clauses
                .into_iter()
                .map(ClauseAnchorDto::into_contract)
                .collect(),
            self.spec.into_contract(),
        )
    }
}

impl ClauseAnchorDto {
    fn into_contract(self) -> proof::ClauseAnchor {
        proof::ClauseAnchor {
            clause_id: self.clause_id,
            start: self.start_byte as usize,
            end: self.end_byte_exclusive as usize,
            quote: self.quote,
            classification: match self.classification {
                ClauseClassificationDto::ResolvedMaterial { fields } => {
                    proof::ClauseClassification::ResolvedMaterial {
                        fields: fields.into_iter().map(Into::into).collect(),
                    }
                }
                ClauseClassificationDto::UnresolvedMaterial { reason } => {
                    proof::ClauseClassification::UnresolvedMaterial {
                        reason: reason.into(),
                    }
                }
                ClauseClassificationDto::NonMaterial { reason } => {
                    proof::ClauseClassification::NonMaterial {
                        kind: reason.into(),
                    }
                }
            },
        }
    }
}

impl CallPathSpecDto {
    fn into_contract(self) -> proof::UnvalidatedCallPathSpec {
        proof::UnvalidatedCallPathSpec {
            start: self.start.into_contract(),
            steps: self
                .steps
                .into_iter()
                .map(|step| proof::UnvalidatedDirectCallStep {
                    target: step.target.into_contract(),
                })
                .collect(),
            prohibit_traversal_through: self
                .prohibit_traversal_through
                .into_iter()
                .map(ExactScopeSelectorDto::into_contract)
                .collect(),
            exclude_from_projection: self
                .exclude_from_projection
                .into_iter()
                .map(ExactScopeSelectorDto::into_contract)
                .collect(),
        }
    }
}

impl ExactSymbolSelectorDto {
    fn into_contract(self) -> proof::UnvalidatedExactSymbolSelector {
        match self {
            Self::PinnedNode {
                project_id,
                core_generation_id,
                core_run_id,
                node_id,
            } => proof::UnvalidatedExactSymbolSelector::PinnedNode(proof::PinnedNodeIdentity {
                project_id,
                core_generation_id,
                core_run_id,
                node_id,
            }),
            Self::CanonicalId { canonical_id } => {
                proof::UnvalidatedExactSymbolSelector::CanonicalId(canonical_id)
            }
            Self::QualifiedName {
                qualified_name,
                project_file_components,
            } => proof::UnvalidatedExactSymbolSelector::QualifiedName {
                qualified_name,
                project_file_components,
            },
        }
    }
}

impl ExactScopeSelectorDto {
    fn into_contract(self) -> proof::UnvalidatedExactScopeSelector {
        match self {
            Self::PinnedNode {
                project_id,
                core_generation_id,
                core_run_id,
                node_id,
            } => proof::UnvalidatedExactScopeSelector::PinnedNode(proof::PinnedNodeIdentity {
                project_id,
                core_generation_id,
                core_run_id,
                node_id,
            }),
            Self::CanonicalId { canonical_id } => {
                proof::UnvalidatedExactScopeSelector::CanonicalId(canonical_id)
            }
            Self::QualifiedName {
                qualified_name,
                project_file_components,
            } => proof::UnvalidatedExactScopeSelector::QualifiedName {
                qualified_name,
                project_file_components,
            },
        }
    }
}

impl From<ProofContractFieldDto> for proof::ProofContractField {
    fn from(value: ProofContractFieldDto) -> Self {
        match value {
            ProofContractFieldDto::Start => Self::Start,
            ProofContractFieldDto::StepTarget { step } => Self::StepTarget { step },
            ProofContractFieldDto::Directness { step } => Self::Directness { step },
            ProofContractFieldDto::Ordering { step } => Self::Ordering { step },
            ProofContractFieldDto::Relation { step } => Self::Relation { step },
            ProofContractFieldDto::TraversalProhibition { index } => {
                Self::TraversalProhibition { index }
            }
            ProofContractFieldDto::ProjectionExclusion { index } => {
                Self::ProjectionExclusion { index }
            }
        }
    }
}

impl From<UnresolvedMaterialReasonDto> for proof::UnresolvedMaterialReason {
    fn from(value: UnresolvedMaterialReasonDto) -> Self {
        match value {
            UnresolvedMaterialReasonDto::MissingSelectorResolution => {
                Self::MissingSelectorResolution
            }
            UnresolvedMaterialReasonDto::AmbiguousSelectorResolution => {
                Self::AmbiguousSelectorResolution
            }
            UnresolvedMaterialReasonDto::UnsupportedInterpretation => {
                Self::UnsupportedInterpretation
            }
        }
    }
}

impl From<NonMaterialReasonDto> for proof::NonMaterialKind {
    fn from(value: NonMaterialReasonDto) -> Self {
        match value {
            NonMaterialReasonDto::Whitespace => Self::Whitespace,
            NonMaterialReasonDto::Punctuation => Self::Punctuation,
            NonMaterialReasonDto::Connector => Self::Connector,
            NonMaterialReasonDto::Commentary => Self::Commentary,
        }
    }
}

pub(crate) fn parse_request(value: Value) -> Result<ProveCallPathRequestDto, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub(crate) fn validate_request(
    request: ProveCallPathRequestDto,
) -> Result<proof::ValidationOutcome, String> {
    proof::validate_contract(request.into_contract()).map_err(|error| format!("{error:?}"))
}

pub(crate) fn projection_root(
    operation: &codestory_runtime::PublicOperation<
        proof::ObservedIntegratedProjectedCallPathResult,
    >,
) -> Result<Value, String> {
    let result = operation
        .value
        .result
        .as_ref()
        .map_err(|error| error.message.clone())?;
    let root = match &result.projection {
        proof::InternalProjection::Complete { root, .. }
        | proof::InternalProjection::BudgetExceeded { root, .. } => root,
    };
    Ok(root.clone())
}

pub(crate) fn internal_projection_root(projection: &proof::InternalProjection) -> Value {
    match projection {
        proof::InternalProjection::Complete { root, .. }
        | proof::InternalProjection::BudgetExceeded { root, .. } => root.clone(),
    }
}

pub(crate) fn read_bounded_spec(path: &Path) -> Result<Vec<u8>> {
    if path.as_os_str() == "-" {
        read_bounded(std::io::stdin().lock(), "stdin")
    } else {
        let file = std::fs::File::open(path)
            .with_context(|| format!("open proof spec {}", path.display()))?;
        read_bounded(file, &format!("proof spec {}", path.display()))
    }
}

fn read_bounded(reader: impl Read, source: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(PROVE_CALL_PATH_INPUT_MAX_BYTES.min(8 * 1024));
    reader
        .take((PROVE_CALL_PATH_INPUT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {source}"))?;
    if bytes.len() > PROVE_CALL_PATH_INPUT_MAX_BYTES {
        bail!(
            "proof spec exceeds the {} byte input limit",
            PROVE_CALL_PATH_INPUT_MAX_BYTES
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn internal_fixture(disposition_kind: &str) -> Value {
        json!({
            "kind": "complete",
            "schema_version": 1,
            "domain": "indexed_source_call_path_v1",
            "contract_interpretation": "host_supplied",
            "guard_version": "clause_guard_v1",
            "source_text_sha256": "a".repeat(64),
            "contract_digest": "b".repeat(64),
            "core_publication": {"project_id":"p","generation_id":"g","run_id":"r"},
            "disposition": {"kind": disposition_kind, "contract_digest":"b".repeat(64), "gaps":[{"kind":"direct_call_missing","step_index":0}], "connected_receipts":[]},
            "identities": {"files":[],"symbols":[],"provenance_profiles":[],"evidence":[]},
            "spec": {"start":{"kind":"canonical_id","canonical_id":"A"},"steps":[{"relation":"direct_outgoing_call","target":{"kind":"canonical_id","canonical_id":"B"}}],"prohibit_traversal_through":[],"exclude_from_projection":[]},
            "clauses": [{"start":0,"end":1,"clause_id":"c","quote":"x","classification":"resolved_material","fields":[{"kind":"start"}],"reason":null,"non_material_kind":null}],
            "steps": [{"step_index":0,"status":"unknown","receipt":null}],
            "receipts": []
        })
    }

    #[test]
    fn public_projection_exposes_phase6_verification_fields() {
        for (internal_kind, graph_disposition) in [
            ("contract_proven", "proven"),
            ("contract_refuted", "refuted"),
            ("unknown", "unknown"),
            ("unavailable", "unknown"),
        ] {
            let public = project_public_verification_result(internal_fixture(internal_kind))
                .expect("project public verification result");
            assert_eq!(public["domain"], "call-path/v1");
            assert_eq!(public["translation_status"], "host_supplied");
            assert_eq!(public["graph_disposition"], graph_disposition);
            assert_eq!(public["runtime_execution_proven"], false);
            assert!(public.get("contract_interpretation").is_none());
        }
    }

    #[test]
    fn proof_tool_names_accept_legacy_and_public_aliases() {
        assert!(is_proof_tool_name(PUBLIC_VERIFY_TOOL_NAME));
        assert!(is_proof_tool_name(LEGACY_PROOF_TOOL_NAME));
        assert!(!is_proof_tool_name("packet"));
    }
}
