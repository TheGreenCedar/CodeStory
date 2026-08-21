//! Sealed observations used only by the proof-qualification benchmark.
//!
//! This facade is intentionally feature-gated and owns no product route. The
//! benchmark receives its domain identity and gate observations through this
//! module without exposing the dark kernel or registering a product route.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use codestory_contracts::api::ApiError;

pub use crate::indexed_source_call_path_v1::{
    CandidateFailure, CandidateFailureHistogram, CandidateGate, ContainmentFailure,
    FinalizationFailure, FinalizationTrace, IntegratedProjectedCallPathResult,
    MAX_QUALIFICATION_CANDIDATE_EDGES_PER_STEP, MAX_QUALIFICATION_OBSERVED_RECEIPTS_PER_CASE,
    ObservedBuiltCallPathFacts, ObservedIntegratedProjectedCallPathResult, ProofQualificationTrace,
    SelectorFailure, SelectorGateOutcome, SelectorQualificationTrace, SourceBindingFailure,
    StepQualificationOutcome, StepQualificationTrace,
};

/// Builds the same product facts plus benchmark-only observations. The caller
/// must already be inside the existing core-pinned public operation.
pub fn build_observed_indexed_source_call_path_facts(
    controller: &crate::AppController,
    contract: &codestory_agent::proof_qualification_support::ValidatedCallPathContract,
) -> Result<ObservedBuiltCallPathFacts, codestory_contracts::api::ApiError> {
    crate::indexed_source_call_path_v1::build_observed_indexed_source_call_path_facts(
        controller, contract,
    )
}

/// Runs the existing checked integration and projection over observed facts,
/// retaining a typed finalization failure for qualification reports.
pub fn finalize_observed_call_path(
    contract: &codestory_agent::proof_qualification_support::ValidatedCallPathContract,
    hashes: &codestory_agent::proof_qualification_support::ProofHashes,
    rendering: &codestory_agent::proof_qualification_support::ValidatedContractRendering,
    observed: ObservedBuiltCallPathFacts,
) -> ObservedIntegratedProjectedCallPathResult {
    crate::indexed_source_call_path_v1::finalize_observed_call_path(
        contract, hashes, rendering, observed,
    )
}

/// Executes one observed proof through the runtime's existing core-only public
/// operation. The benchmark cannot obtain the controller or add a second
/// publication retry around this call.
pub fn run_observed_call_path_public_operation(
    runtime: &crate::Runtime,
    contract: &codestory_agent::proof_qualification_support::ValidatedCallPathContract,
    hashes: &codestory_agent::proof_qualification_support::ProofHashes,
    rendering: &codestory_agent::proof_qualification_support::ValidatedContractRendering,
    cancelled: Arc<AtomicBool>,
) -> Result<crate::PublicOperation<ObservedIntegratedProjectedCallPathResult>, ApiError> {
    runtime
        .public_operation_service()
        .run_with_cancel(proof_domain(), cancelled, || {
            let observed =
                build_observed_indexed_source_call_path_facts(&runtime.controller, contract)?;
            Ok(finalize_observed_call_path(
                contract, hashes, rendering, observed,
            ))
        })
}

/// Identifies the request domain observed by proof qualification.
pub fn proof_domain() -> &'static str {
    codestory_agent::proof_qualification_support::proof_domain()
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

    use codestory_agent::proof_qualification_test_support::{
        ClauseAnchor, ClauseClassification, ProofContractField, UnvalidatedCallPathContract,
        UnvalidatedCallPathSpec, UnvalidatedDirectCallStep, UnvalidatedExactSymbolSelector,
        ValidationOutcome, validate_contract,
    };
    use codestory_contracts::api::IndexMode;
    use codestory_contracts::graph::{Node, NodeKind};
    use codestory_store::Store;

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
        codestory_agent::proof_qualification_support::ValidatedCallPathContract,
        codestory_agent::proof_qualification_support::ProofHashes,
        codestory_agent::proof_qualification_support::ValidatedContractRendering,
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
            codestory_agent::proof_qualification_test_support::InternalProjection::Complete {
                root,
                ..
            }
            | codestory_agent::proof_qualification_test_support::InternalProjection::BudgetExceeded {
                root,
                ..
            } => root,
        };
        root["disposition"]["kind"]
            .as_str()
            .expect("disposition kind")
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
