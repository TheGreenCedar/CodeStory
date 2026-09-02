//! Sealed runtime facade for exact call-path verification and qualification.
//!
//! The CLI and benchmark share this pinned-core operation without exposing the
//! private proof kernel or allowing transport adapters to own orchestration.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use codestory_contracts::api::ApiError;
pub use codestory_contracts::call_path_public::{
    PUBLIC_CALL_PATH_DOMAIN, PublicCallPathResultDto, public_call_path_result_schema,
};
use serde_json::{Value, json};

pub use crate::call_path_kernel::{
    AdmittedRawCallEdge, BuiltCallPathFacts, COMPACT_PROOF_MAX_BYTES, CONTRACT_INTERPRETATION,
    CallPathSpec, CallableContainmentEvidence, ClauseAnchor, ClauseClassification, FactBuildGap,
    IndexedCallEdgeReceipt, IndexedLineWindow, InternalCorePublicationIdentity, InternalProjection,
    NonMaterialKind, PinnedNodeIdentity, ProofContractField, ProofHashes, RawAdmissionFailure,
    ReceiptRef, ResolvedNodeIdentity, TranslationGap, UnavailableReason, UnresolvedMaterialReason,
    UnvalidatedCallPathContract, UnvalidatedCallPathSpec, UnvalidatedDirectCallStep,
    UnvalidatedExactScopeSelector, UnvalidatedExactSymbolSelector, ValidatedCallPathContract,
    ValidatedContractRendering, ValidationOutcome, VerifiedDirectCallFact, VerifiedProofFact,
    check_built_call_path_integration, diagnose_raw_call_edge, project_internal_call_path_result,
    project_translation_unknown_result, validate_compact_projection, validate_contract,
};
pub use crate::indexed_source_call_path_v1::{
    CandidateFailure, CandidateFailureHistogram, CandidateGate, ContainmentFailure,
    FinalizationFailure, FinalizationTrace, IntegratedProjectedCallPathResult,
    MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP, MAX_QUALIFICATION_OBSERVED_RECEIPTS_PER_CASE,
    ObservedBuiltCallPathFacts, ObservedIntegratedProjectedCallPathResult, ProofQualificationTrace,
    ResolutionFactFailure, SelectorFailure, SelectorGateOutcome, SelectorQualificationTrace,
    SourceBindingFailure, StepQualificationOutcome, StepQualificationTrace,
};
use serde::Serialize;

/// Parse the public `call-path/v1` document inside the runtime boundary.
///
/// Transport adapters supply only the bounded UTF-8 document. They do not own
/// selector interpretation, clause classification, or proof-kernel inputs.
pub fn parse_public_call_path_document(
    document: &str,
) -> Result<UnvalidatedCallPathContract, String> {
    crate::call_path_grammar::parse_call_path_document(document).map_err(|error| error.message)
}

/// Validate the runtime-parsed contract through the proof kernel.
pub fn validate_public_call_path_contract(
    contract: UnvalidatedCallPathContract,
) -> Result<ValidationOutcome, String> {
    validate_contract(contract).map_err(|error| format!("{error:?}"))
}

/// Project a kernel-owned internal root into the one shared public DTO.
///
/// Adapters receive no opportunity to rewrite dispositions, claim runtime
/// execution, or manufacture their own budget envelope.
pub fn project_public_verification_result(
    internal: Value,
) -> Result<PublicCallPathResultDto, String> {
    validate_compact_projection(&internal)
        .map_err(|error| format!("invalid internal call-path projection: {error}"))?;
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
        .unwrap_or_else(|| Value::String(CONTRACT_INTERPRETATION.to_owned()));
    public.insert("translation_status".to_owned(), translation_status);
    rewrite_forbidden_public_absence(&mut public)?;
    public.insert(
        "graph_disposition".to_owned(),
        Value::String(
            public
                .get("disposition")
                .map(graph_disposition_from_disposition)
                .unwrap_or("unknown")
                .to_owned(),
        ),
    );
    public.insert("runtime_execution_proven".to_owned(), Value::Bool(false));
    attach_proof_provenance_capability(&mut public);
    apply_public_compact_budget(Value::Object(public))
}

/// Extract and project the observed result of one runtime-owned operation.
pub fn project_observed_public_operation(
    operation: &crate::PublicOperation<ObservedIntegratedProjectedCallPathResult>,
) -> Result<PublicCallPathResultDto, String> {
    let result = operation
        .value
        .result
        .as_ref()
        .map_err(|error| error.message.clone())?;
    project_internal_projection(&result.projection)
}

/// Project an internal result without exposing its raw root to an adapter.
pub fn project_internal_projection(
    projection: &InternalProjection,
) -> Result<PublicCallPathResultDto, String> {
    let root = match projection {
        InternalProjection::Complete { root, .. }
        | InternalProjection::BudgetExceeded { root, .. } => root.clone(),
    };
    project_public_verification_result(root)
}

/// Replace a complete public result with the runtime-owned typed budget result
/// when a transport envelope duplicates or escapes the compact JSON.
pub fn project_public_transport_budget_result(
    complete: &PublicCallPathResultDto,
    required_transport_size: usize,
) -> Result<PublicCallPathResultDto, String> {
    let object = complete
        .as_value()
        .as_object()
        .ok_or_else(|| "public proof projection root must be an object".to_owned())?;
    if object.get("kind") != Some(&json!("complete")) {
        return Err("only a complete public call-path result can be budget-projected".to_owned());
    }
    let required = |name: &str| {
        object
            .get(name)
            .cloned()
            .ok_or_else(|| format!("public call-path result is missing `{name}`"))
    };
    let contract_digest = required("contract_digest")?;
    PublicCallPathResultDto::try_from_projected_value(json!({
        "kind": "budget_exceeded",
        "schema_version": required("schema_version")?,
        "domain": PUBLIC_CALL_PATH_DOMAIN,
        "translation_status": required("translation_status")?,
        "graph_disposition": "unknown",
        "runtime_execution_proven": false,
        "guard_version": required("guard_version")?,
        "source_text_sha256": required("source_text_sha256")?,
        "contract_digest": contract_digest,
        "core_publication": required("core_publication")?,
        "provenance": { "availability": "unavailable" },
        "disposition": {
            "kind": "unknown",
            "contract_digest": contract_digest,
            "gaps": [{"kind":"output_budget_exceeded"}]
        },
        "cap_bytes": COMPACT_PROOF_MAX_BYTES,
        "required_complete_size": required_transport_size
    }))
}

fn apply_public_compact_budget(root: Value) -> Result<PublicCallPathResultDto, String> {
    let serialized = serde_json::to_vec(&root)
        .map_err(|error| format!("serialize public verification result: {error}"))?;
    if serialized.len() <= COMPACT_PROOF_MAX_BYTES {
        return PublicCallPathResultDto::try_from_projected_value(root);
    }
    let object = root
        .as_object()
        .ok_or_else(|| "proof projection root must be an object".to_owned())?;
    let contract_digest = object.get("contract_digest").cloned().unwrap_or(json!(""));
    let compact = json!({
        "kind": "budget_exceeded",
        "schema_version": object.get("schema_version").cloned().unwrap_or(json!(1)),
        "domain": PUBLIC_CALL_PATH_DOMAIN,
        "translation_status": object.get("translation_status").cloned().unwrap_or(json!(CONTRACT_INTERPRETATION)),
        "graph_disposition": "unknown",
        "runtime_execution_proven": false,
        "guard_version": object.get("guard_version").cloned().unwrap_or(json!(codestory_contracts::call_path::CLAUSE_GUARD_VERSION)),
        "source_text_sha256": object.get("source_text_sha256").cloned().unwrap_or(json!("")),
        "contract_digest": contract_digest.clone(),
        "core_publication": object.get("core_publication").cloned().unwrap_or(json!({})),
        "provenance": { "availability": "unavailable" },
        "disposition": {
            "kind": "unknown",
            "contract_digest": contract_digest,
            "gaps": [{"kind": "output_budget_exceeded"}]
        },
        "cap_bytes": COMPACT_PROOF_MAX_BYTES,
        "required_complete_size": serialized.len(),
    });
    let compact_bytes = serde_json::to_vec(&compact)
        .map_err(|error| format!("serialize public verification budget envelope: {error}"))?;
    if compact_bytes.len() > COMPACT_PROOF_MAX_BYTES {
        return Err(format!(
            "public verification result exceeds {COMPACT_PROOF_MAX_BYTES} bytes even after budget projection ({} bytes)",
            compact_bytes.len()
        ));
    }
    PublicCallPathResultDto::try_from_projected_value(compact)
}

fn rewrite_forbidden_public_absence(
    public: &mut serde_json::Map<String, Value>,
) -> Result<(), String> {
    let Some(disposition) = public.get("disposition").cloned() else {
        return Ok(());
    };
    let is_certified_absence = disposition.get("kind").and_then(Value::as_str)
        == Some("contract_refuted")
        && disposition
            .pointer("/refutation/kind")
            .and_then(Value::as_str)
            == Some("certified_absence");
    if !is_certified_absence {
        return Ok(());
    }
    let contract_digest = disposition
        .get("contract_digest")
        .cloned()
        .ok_or_else(|| "certified_absence refutation missing contract_digest".to_owned())?;
    public.insert(
        "disposition".to_owned(),
        json!({
            "kind": "unavailable",
            "contract_digest": contract_digest,
            "reasons": ["proof_facts_unavailable"]
        }),
    );
    if let Some(steps) = public.get_mut("steps").and_then(Value::as_array_mut) {
        for step in steps {
            if step.get("status").and_then(Value::as_str) == Some("certified_absence") {
                step["status"] = json!("unavailable");
            }
        }
    }
    Ok(())
}

fn graph_disposition_from_disposition(disposition: &Value) -> &'static str {
    match disposition.get("kind").and_then(Value::as_str) {
        Some("contract_proven") => "proven",
        Some("contract_refuted")
            if disposition
                .pointer("/refutation/kind")
                .and_then(Value::as_str)
                != Some("certified_absence") =>
        {
            "refuted"
        }
        _ => "unknown",
    }
}

fn attach_proof_provenance_capability(public: &mut serde_json::Map<String, Value>) {
    public
        .entry("provenance".to_owned())
        .or_insert_with(|| json!({ "availability": "unavailable" }));
}

/// Executes one proof through the runtime's existing core-only public
/// operation. Callers cannot obtain the controller or add a second publication
/// retry around this call.
pub fn run_observed_call_path_public_operation(
    runtime: &crate::Runtime,
    contract: &ValidatedCallPathContract,
    hashes: &ProofHashes,
    rendering: &ValidatedContractRendering,
    cancelled: Arc<AtomicBool>,
) -> Result<crate::PublicOperation<ObservedIntegratedProjectedCallPathResult>, ApiError> {
    runtime.controller.arm_proof_publication_validation();
    let result =
        runtime
            .public_operation_service()
            .run_with_cancel(proof_domain(), cancelled, || {
                let (observed, validation) = crate::indexed_source_call_path_v1::
                build_observed_indexed_source_call_path_facts_with_prepared_validation(
                    &runtime.controller,
                    contract,
                )?;
                let result = crate::indexed_source_call_path_v1::finalize_observed_call_path(
                    contract, hashes, rendering, observed,
                );
                runtime
                    .controller
                    .finish_proof_publication_validation(validation)?;
                Ok(result)
            });
    runtime
        .controller
        .finish_proof_publication_validation_operation();
    result
}

/// Projects an incomplete host translation against the same pinned core
/// publication without reading graph or retrieval state.
pub fn run_translation_unknown_public_operation(
    runtime: &crate::Runtime,
    spec: &CallPathSpec,
    hashes: &ProofHashes,
    rendering: &ValidatedContractRendering,
    gaps: &[TranslationGap],
    cancelled: Arc<AtomicBool>,
) -> Result<crate::PublicOperation<InternalProjection>, ApiError> {
    runtime
        .public_operation_service()
        .run_observational_with_cancel(proof_domain(), cancelled, || {
            let publication = runtime
                .controller
                .active_core_publication()
                .ok_or_else(|| {
                    ApiError::new(
                        "proof_semantic_projection_unavailable",
                        "the exact proof core publication is unavailable",
                    )
                })?;
            let project_root = runtime.controller.require_project_root()?;
            let project_id = codestory_workspace::project_identity_v3(&project_root).project_id;
            project_translation_unknown_result(
                spec,
                hashes,
                rendering,
                gaps,
                &InternalCorePublicationIdentity {
                    project_id,
                    generation_id: publication.generation_id,
                    run_id: publication.run_id,
                },
            )
            .map_err(|error| ApiError::new("invalid_proof_projection", format!("{error:?}")))
        })
}

/// Identifies the request domain observed by proof qualification.
pub fn proof_domain() -> &'static str {
    crate::call_path_kernel::PROOF_DOMAIN
}

/// Serialize a qualification artifact with the repository-pinned RFC 8785
/// implementation without exposing that dependency to the benchmark crate.
pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json_canonicalizer::to_vec(value).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        ObservedIntegratedProjectedCallPathResult, run_observed_call_path_public_operation,
    };
    use std::cell::Cell;
    use std::fs;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::{
        ClauseAnchor, ClauseClassification, InternalProjection, ProofContractField, ProofHashes,
        UnvalidatedCallPathContract, UnvalidatedCallPathSpec, UnvalidatedDirectCallStep,
        UnvalidatedExactSymbolSelector, ValidatedCallPathContract, ValidatedContractRendering,
        ValidationOutcome, validate_contract,
    };
    use codestory_contracts::api::IndexMode;
    use codestory_contracts::graph::{Node, NodeKind};
    use codestory_store::Store;

    use crate::indexed_source_call_path_v1::{
        full_proof_publication_validation_count, reset_full_proof_publication_validations,
    };

    fn callable(store: &Store, terminal_name: &str) -> Node {
        let mut matches = store
            .get_nodes()
            .unwrap()
            .into_iter()
            .filter(|node| {
                matches!(
                    node.kind,
                    NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO
                ) && (node.serialized_name == terminal_name
                    || node.qualified_name.as_deref().is_some_and(|qualified| {
                        qualified.rsplit("::").next() == Some(terminal_name)
                    }))
            })
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1, "fixture callable {terminal_name}");
        matches.remove(0)
    }

    fn validated(
        start: &str,
        target: &str,
    ) -> (
        ValidatedCallPathContract,
        ProofHashes,
        ValidatedContractRendering,
    ) {
        let source = "exact direct ordered call path";
        let outcome = validate_contract(UnvalidatedCallPathContract::new(
            source,
            vec![ClauseAnchor {
                clause_id: "contract".to_owned(),
                start: 0,
                end: source.len(),
                quote: source.to_owned(),
                classification: ClauseClassification::ResolvedMaterial {
                    fields: vec![
                        ProofContractField::Start,
                        ProofContractField::StepTarget { step: 0 },
                        ProofContractField::Directness { step: 0 },
                        ProofContractField::Ordering { step: 0 },
                        ProofContractField::Relation { step: 0 },
                    ],
                },
            }],
            UnvalidatedCallPathSpec {
                start: UnvalidatedExactSymbolSelector::CanonicalId(start.to_owned()),
                steps: vec![UnvalidatedDirectCallStep {
                    target: UnvalidatedExactSymbolSelector::CanonicalId(target.to_owned()),
                }],
                prohibit_traversal_through: Vec::new(),
                exclude_from_projection: Vec::new(),
            },
        ))
        .unwrap();
        match outcome {
            ValidationOutcome::Validated {
                contract,
                hashes,
                rendering,
            } => (*contract, hashes, rendering),
            other => panic!("expected validated fixture contract, got {other:?}"),
        }
    }

    fn disposition_kind(
        operation: &crate::PublicOperation<ObservedIntegratedProjectedCallPathResult>,
    ) -> &str {
        let result = operation.value.result.as_ref().expect("product result");
        let root = match &result.projection {
            InternalProjection::Complete { root, .. }
            | InternalProjection::BudgetExceeded { root, .. } => root,
        };
        root["disposition"]["kind"]
            .as_str()
            .expect("disposition kind")
    }

    #[test]
    fn qualification_facade_exposes_only_runtime_owned_execution() {
        let source = include_str!("proof_qualification_support.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production source");
        assert!(!production.contains("pub fn build_observed_indexed_source_call_path_facts"));
        assert!(!production.contains("pub fn finalize_observed_call_path"));
        assert!(!production.contains("AppController"));
        assert!(production.contains("pub fn run_observed_call_path_public_operation"));
    }

    #[test]
    fn proof_qualification_runtime_helper_is_one_core_only_product_operation() {
        let project = tempfile::tempdir().unwrap();
        let source_path = project.path().join("src/lib.rs");
        fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        fs::write(
            &source_path,
            "pub fn callee() {}\npub fn other() {}\npub fn caller() { callee(); }\n",
        )
        .unwrap();
        let storage_path = project.path().join(".codestory-test/codestory.db");
        let runtime = crate::Runtime::new_with_config(crate::test_sidecar_runtime_from_env());
        runtime
            .project_service()
            .open_project_summary_with_storage_path(
                project.path().to_path_buf(),
                storage_path.clone(),
            )
            .unwrap();
        runtime
            .project_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .unwrap();
        let store = Store::open(&storage_path).unwrap();
        let caller = callable(&store, "caller");
        let callee = callable(&store, "callee");
        let other = callable(&store, "other");
        let caller_id = caller.canonical_id.as_deref().unwrap();
        let callee_id = callee.canonical_id.as_deref().unwrap();
        let other_id = other.canonical_id.as_deref().unwrap();
        let positive = validated(caller_id, callee_id);
        let unknown = validated(caller_id, other_id);
        drop(store);

        let retrieval_pin_calls = Rc::new(Cell::new(0));
        let observed_calls = Rc::clone(&retrieval_pin_calls);
        crate::set_before_retrieval_pin_test_hook(move || {
            observed_calls.set(observed_calls.get() + 1);
        });
        reset_full_proof_publication_validations();
        let positive_operation = run_observed_call_path_public_operation(
            &runtime,
            &positive.0,
            &positive.1,
            &positive.2,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(positive_operation.attempt, 1);
        assert!(positive_operation.core_publication.is_some());
        assert_eq!(positive_operation.retrieval_publication, None);
        assert_eq!(disposition_kind(&positive_operation), "contract_proven");

        let unknown_operation = run_observed_call_path_public_operation(
            &runtime,
            &unknown.0,
            &unknown.1,
            &unknown.2,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(unknown_operation.attempt, 1);
        assert!(unknown_operation.core_publication.is_some());
        assert_eq!(unknown_operation.retrieval_publication, None);
        assert_eq!(disposition_kind(&unknown_operation), "unknown");
        assert_eq!(
            full_proof_publication_validation_count(),
            2,
            "a sealed generation re-validates because no mutable WAL observer can fence reuse"
        );
        assert_eq!(retrieval_pin_calls.get(), 0);
        assert_eq!(
            Store::open(&storage_path)
                .unwrap()
                .get_retrieval_index_publication(
                    &codestory_workspace::project_identity_v3(project.path()).project_id,
                )
                .unwrap(),
            None
        );
        let _ = runtime.public_operation_service().run_with_cancel(
            "search",
            Arc::new(AtomicBool::new(false)),
            || Ok(()),
        );
        assert_eq!(
            retrieval_pin_calls.get(),
            1,
            "consume the armed test hook after proving neither proof operation reached it"
        );

        fs::write(
            &source_path,
            "pub fn callee() {}\npub fn other() {}\npub fn caller() { other();  }\n",
        )
        .unwrap();
        let stale = run_observed_call_path_public_operation(
            &runtime,
            &positive.0,
            &positive.1,
            &positive.2,
            Arc::new(AtomicBool::new(false)),
        )
        .expect_err("the helper must own the public-operation freshness fence");
        assert_eq!(stale.code, "project_unavailable");
    }
}
