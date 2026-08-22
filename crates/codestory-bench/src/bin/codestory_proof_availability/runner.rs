use super::contracts::{
    ActualProductResultV1, CaseReportV1, CohortPathFileV1, FailureBucketV1, FailureFunnelReportV1,
    FunnelOutcomeV1, MissingOracleStepV1, NegativeMutationResultV1, ObservedReceiptV1,
    ProjectMaterializationEvidenceV1, ProofQualificationTraceV1, ReceiptEvidenceBuildOutcomeV1,
    ReceiptEvidenceV1, ReceiptOracleComparisonV1, ReceiptOracleStepV1, StageDurationsV1,
    StepQualificationOutcomeV1, ThresholdsV1, TransportErrorV1, TransportEvidenceV1,
    actionable_exact_gap_for_case, negative_mutation_product_contract,
    observed_product_disposition_to_report, observed_receipt_from_task6,
    oracle_path_product_contract, require_expected_product_contract_digest,
};
use super::corpus::LoadedCorpusV1;
use super::inventory::analyze_store;
use super::materialize::{
    OperationalEnvironmentV1, OperationalRepositoryV1, core_only_runtime, revalidate_case_source,
    validate_operational_environment,
};
use super::report::QualificationReportInputV1;
use super::trails::count_store_trails;
use anyhow::{Context, Result, bail};
use codestory_agent::proof_qualification_support::{
    InternalProjection, ValidationOutcome, validate_contract,
};
use codestory_runtime::proof_qualification_support::{
    ObservedIntegratedProjectedCallPathResult, StepQualificationOutcome,
    run_observed_call_path_public_operation,
};
use codestory_store::Store;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

pub(crate) fn run_qualification(
    loaded: &LoadedCorpusV1,
    thresholds: &ThresholdsV1,
    operational: &OperationalEnvironmentV1,
) -> Result<QualificationReportInputV1> {
    loaded.corpus.validate_against_thresholds(thresholds)?;
    validate_operational_environment(loaded, operational)?;
    let mut inventory = Vec::with_capacity(operational.repositories.len());
    let mut trails = Vec::with_capacity(operational.repositories.len());
    let mut cases = Vec::with_capacity(loaded.corpus.positive_request_count as usize);
    for path_file in &loaded.path_files {
        let repository = repository_for(operational, &path_file.repository_id)?;
        let expected_publication = operational
            .environment
            .projects
            .iter()
            .find(|project| project.repository_id == path_file.repository_id)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_environment_project_missing"))?;
        revalidate_case_source(
            path_file,
            &repository.checkout_root,
            &repository.project_root,
        )?;
        let store = Store::open_observational(&repository.database_path)
            .context("open proof availability store observationally")?;
        inventory.push(analyze_store(&path_file.repository_id, &store)?.report);
        trails.push(count_store_trails(&path_file.repository_id, &store)?);
        drop(store);
        let runtime = core_only_runtime(
            &repository.project_root,
            &operational.cache_root.join("qualification-runtime"),
        );
        runtime
            .project_service()
            .open_project_summary_with_storage_path(
                repository.project_root.clone(),
                repository.database_path.clone(),
            )
            .map_err(|error| anyhow::anyhow!(error.message))?;
        for path in &path_file.paths {
            revalidate_case_source(
                path_file,
                &repository.checkout_root,
                &repository.project_root,
            )?;
            cases.push(run_case(&runtime, path_file, expected_publication, path)?);
        }
        drop(runtime);
        revalidate_case_source(
            path_file,
            &repository.checkout_root,
            &repository.project_root,
        )?;
    }
    inventory.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    trails.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    cases.sort_by(|left, right| left.case_id.cmp(&right.case_id));
    let failure_funnel = build_failure_funnel(&cases)?;
    Ok(QualificationReportInputV1 {
        qualification_id: operational.environment.environment_id.clone(),
        source_commit: operational.environment.qualification_source_commit.clone(),
        source_tree: operational.environment.qualification_source_tree.clone(),
        environment: operational.environment.clone(),
        inventory,
        trails,
        cases,
        failure_funnel,
    })
}

fn repository_for<'a>(
    operational: &'a OperationalEnvironmentV1,
    repository_id: &str,
) -> Result<&'a OperationalRepositoryV1> {
    operational
        .repositories
        .iter()
        .find(|repository| repository.repository_id == repository_id)
        .ok_or_else(|| anyhow::anyhow!("proof_availability_operational_repository_missing"))
}

fn run_case(
    runtime: &codestory_runtime::Runtime,
    path_file: &CohortPathFileV1,
    expected_publication: &ProjectMaterializationEvidenceV1,
    path: &super::contracts::OraclePathV1,
) -> Result<CaseReportV1> {
    let validation_started = Instant::now();
    let (contract, hashes, rendering) = match validate_contract(oracle_path_product_contract(path)?)
    {
        Ok(ValidationOutcome::Validated {
            contract,
            hashes,
            rendering,
        }) => (contract, hashes, rendering),
        Ok(ValidationOutcome::Unknown { .. }) => {
            bail!("proof_availability_positive_contract_translation_incomplete")
        }
        Err(error) => bail!("proof_availability_positive_contract_invalid: {error:?}"),
    };
    let validation_duration = validation_started.elapsed();
    let operation_started = Instant::now();
    let operation = run_observed_call_path_public_operation(
        runtime,
        &contract,
        &hashes,
        &rendering,
        Arc::new(AtomicBool::new(false)),
    )
    .map_err(|error| anyhow::anyhow!(error.message))?;
    let operation_duration = operation_started.elapsed();
    let publication = operation
        .core_publication
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("proof_availability_core_publication_missing"))?;
    if operation.retrieval_publication.is_some()
        || publication.generation != expected_publication.core_generation
        || publication.generation_id != expected_publication.identity.core_generation_id
        || publication.run_id != expected_publication.identity.core_run_id
    {
        bail!("proof_availability_publication_binding_invalid")
    }
    let observed = &operation.value;
    let observed_disposition = observed_product_disposition_to_report(observed)?;
    require_expected_product_contract_digest(
        &observed_disposition.actual,
        hashes.contract_digest(),
    )?;
    let mut negative_mutations = Vec::with_capacity(path.negative_mutations.len());
    for mutation in &path.negative_mutations {
        let outcome = validate_contract(negative_mutation_product_contract(
            path,
            &mutation.mutation_id,
        )?)
        .map_err(|error| anyhow::anyhow!("proof_availability_mutation_invalid: {error:?}"))?;
        let ValidationOutcome::Validated {
            contract,
            hashes,
            rendering,
        } = outcome
        else {
            bail!("proof_availability_mutation_translation_incomplete")
        };
        let operation = run_observed_call_path_public_operation(
            runtime,
            &contract,
            &hashes,
            &rendering,
            Arc::new(AtomicBool::new(false)),
        )
        .map_err(|error| anyhow::anyhow!(error.message))?;
        if operation.retrieval_publication.is_some() {
            bail!("proof_availability_retrieval_publication_forbidden")
        }
        let publication = operation
            .core_publication
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("proof_availability_core_publication_missing"))?;
        if publication.generation != expected_publication.core_generation
            || publication.generation_id != expected_publication.identity.core_generation_id
            || publication.run_id != expected_publication.identity.core_run_id
        {
            bail!("proof_availability_publication_binding_invalid")
        }
        let disposition = observed_product_disposition_to_report(&operation.value)?;
        negative_mutations.push(NegativeMutationResultV1 {
            mutation_id: mutation.mutation_id.clone(),
            path_id: mutation.path_id.clone(),
            kind: mutation.kind,
            step_index: mutation.step_index,
            mutated_spec: mutation.mutated_spec.clone(),
            contract_proven: matches!(
                disposition.actual,
                ActualProductResultV1::ContractProven { .. }
            ),
        });
    }
    assemble_case_report(
        &path_file.repository_id,
        path,
        observed,
        validation_duration,
        operation_duration,
        negative_mutations,
    )
}

fn assemble_case_report(
    repository_id: &str,
    path: &super::contracts::OraclePathV1,
    observed: &ObservedIntegratedProjectedCallPathResult,
    validation_duration: Duration,
    operation_duration: Duration,
    mut negative_mutations: Vec<NegativeMutationResultV1>,
) -> Result<CaseReportV1> {
    let product_disposition = observed_product_disposition_to_report(observed)?;
    let proof_trace: ProofQualificationTraceV1 = observed.trace.clone().try_into()?;
    let unclassified_step_indices = unclassified_steps(path, &proof_trace)?;
    let (receipt_evidence, complete_projection_bytes, transport, transport_wall) = if observed
        .result
        .is_ok()
    {
        let (root, complete_projection_bytes) = projected_root(observed)?;
        let transport_started = Instant::now();
        let transport = TransportEvidenceV1::try_from(
            codestory_cli::proof_qualification_support::measure_revision_native_proof_result(root),
        )?;
        (
            receipt_evidence(path, observed)?,
            complete_projection_bytes,
            transport,
            transport_started.elapsed(),
        )
    } else {
        let missing_oracle_steps = path
            .oracle_steps
            .iter()
            .enumerate()
            .map(|(index, oracle)| {
                Ok(MissingOracleStepV1 {
                    step_index: u8::try_from(index)?,
                    oracle_step: ReceiptOracleStepV1::from(oracle),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        (
            ReceiptEvidenceV1 {
                observed_receipts: Vec::new(),
                missing_oracle_steps,
            },
            0,
            TransportEvidenceV1::Error {
                error: TransportErrorV1::InvalidProjection {
                    projection: "product_tool_failure".to_owned(),
                },
            },
            Duration::ZERO,
        )
    };
    let actionable_exact_gap = actionable_exact_gap_for_case(
        &product_disposition,
        &receipt_evidence,
        u8::try_from(path.spec.steps.len())?,
        &proof_trace,
    )?;
    negative_mutations.sort_by(|left, right| left.mutation_id.cmp(&right.mutation_id));
    Ok(CaseReportV1 {
        case_id: path.case_id.clone(),
        repository_id: repository_id.to_owned(),
        product_disposition,
        actionable_exact_gap,
        warm_end_to_end_ms: duration_sum_millis([
            validation_duration,
            operation_duration,
            transport_wall,
        ])?,
        stage_durations_ms: StageDurationsV1 {
            validation: duration_millis(validation_duration)?,
            operation: duration_millis(operation_duration)?,
        },
        attempted_step_count: u8::try_from(path.spec.steps.len())?,
        unclassified_step_indices,
        receipt_evidence,
        complete_projection_bytes,
        transport,
        negative_mutations,
        proof_trace,
    })
}

fn projected_root(
    observed: &ObservedIntegratedProjectedCallPathResult,
) -> Result<(&serde_json::Value, u64)> {
    let result = observed
        .result
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.message.clone()))?;
    match &result.projection {
        InternalProjection::Complete {
            root,
            serialized_size,
        }
        | InternalProjection::BudgetExceeded {
            root,
            serialized_size,
            ..
        } => Ok((root, u64::try_from(*serialized_size)?)),
    }
}

fn receipt_evidence(
    path: &super::contracts::OraclePathV1,
    observed: &ObservedIntegratedProjectedCallPathResult,
) -> Result<ReceiptEvidenceV1> {
    let result = observed
        .result
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.message.clone()))?;
    let mut step_by_edge = BTreeMap::<i64, u8>::new();
    for step in &observed.trace.steps {
        if let StepQualificationOutcome::Admitted { edge_ids } = &step.outcome {
            for edge_id in edge_ids {
                if step_by_edge
                    .insert(edge_id.0, u8::try_from(step.step_index)?)
                    .is_some()
                {
                    bail!("proof_availability_trace_edge_not_bijective")
                }
            }
        }
    }
    let mut receipts = Vec::<ObservedReceiptV1>::new();
    for receipt in &result.integration.built_facts().receipts {
        let edge_id: i64 = receipt.receipt.edge_id.parse()?;
        let step_index = *step_by_edge
            .get(&edge_id)
            .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_trace_binding_missing"))?;
        receipts.push(observed_receipt_from_task6(
            step_index,
            receipt,
            path.oracle_steps
                .get(usize::from(step_index))
                .ok_or_else(|| anyhow::anyhow!("proof_availability_receipt_oracle_missing"))?,
        )?);
    }
    if receipts.len() != step_by_edge.len() {
        bail!("proof_availability_receipt_trace_binding_incomplete")
    }
    let authoritative = match &result.projection {
        InternalProjection::Complete { .. } => result
            .integration
            .authoritative_receipts()
            .iter()
            .map(|receipt| {
                let edge = receipt.receipt.edge_id.parse::<i64>()?;
                Ok((receipt.receipt.receipt_id.clone(), edge))
            })
            .collect::<Result<BTreeSet<_>>>()?,
        InternalProjection::BudgetExceeded { .. } => BTreeSet::new(),
    };
    let exact_authoritative_steps = receipts
        .iter()
        .filter(|receipt| {
            authoritative.contains(&(receipt.receipt_id.clone(), receipt.edge_id))
                && matches!(
                    receipt.oracle_comparison,
                    ReceiptOracleComparisonV1::Exact { .. }
                )
        })
        .map(|receipt| receipt.step_index)
        .collect::<BTreeSet<_>>();
    let missing = path
        .oracle_steps
        .iter()
        .enumerate()
        .filter_map(|(index, oracle)| {
            let step_index = u8::try_from(index).ok()?;
            (!exact_authoritative_steps.contains(&step_index)).then(|| MissingOracleStepV1 {
                step_index,
                oracle_step: ReceiptOracleStepV1::from(oracle),
            })
        })
        .collect::<Vec<_>>();
    receipts.sort_by_key(|receipt| (receipt.step_index, receipt.edge_id));
    match ReceiptEvidenceV1::bounded(receipts, missing) {
        ReceiptEvidenceBuildOutcomeV1::Complete(evidence) => Ok(evidence),
        ReceiptEvidenceBuildOutcomeV1::LimitExceeded { .. } => {
            bail!("proof_availability_receipt_evidence_limit_exceeded")
        }
    }
}

fn unclassified_steps(
    path: &super::contracts::OraclePathV1,
    trace: &ProofQualificationTraceV1,
) -> Result<Vec<u8>> {
    let classified = trace
        .steps
        .iter()
        .map(|step| u8::try_from(step.step_index))
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    Ok((0..path.spec.steps.len())
        .filter_map(|index| {
            let index = u8::try_from(index).ok()?;
            (!classified.contains(&index)).then_some(index)
        })
        .collect())
}

fn build_failure_funnel(cases: &[CaseReportV1]) -> Result<FailureFunnelReportV1> {
    let mut buckets = BTreeMap::<Vec<u8>, (FunnelOutcomeV1, u128)>::new();
    let mut classified = 0u16;
    let mut unclassified = 0u16;
    let mut attempted = 0u16;
    for case in cases {
        attempted = attempted
            .checked_add(u16::from(case.attempted_step_count))
            .context("proof_availability_attempted_steps_overflow")?;
        unclassified = unclassified
            .checked_add(u16::try_from(case.unclassified_step_indices.len())?)
            .context("proof_availability_unclassified_overflow")?;
        for step in &case.proof_trace.steps {
            let outcome = match &step.outcome {
                StepQualificationOutcomeV1::Admitted { .. } => Some(FunnelOutcomeV1::Admitted),
                StepQualificationOutcomeV1::FirstZeroSurvivor { gate, histogram } => {
                    Some(FunnelOutcomeV1::FirstZeroSurvivor {
                        gate: gate.clone(),
                        histogram: histogram.clone(),
                    })
                }
                StepQualificationOutcomeV1::CandidateLimitExceeded { .. } => None,
            };
            if let Some(outcome) = outcome {
                classified = classified
                    .checked_add(1)
                    .context("proof_availability_classified_overflow")?;
                let key =
                    codestory_agent::proof_qualification_support::canonical_json_bytes(&outcome)
                        .map_err(|error| anyhow::anyhow!(error))?;
                let entry = buckets.entry(key).or_insert((outcome, 0));
                entry.1 = entry
                    .1
                    .checked_add(1)
                    .context("proof_availability_funnel_count_overflow")?;
            } else {
                unclassified = unclassified
                    .checked_add(1)
                    .context("proof_availability_unclassified_overflow")?;
            }
        }
    }
    Ok(FailureFunnelReportV1 {
        attempted_positive_steps: attempted,
        classified_positive_steps: classified,
        unclassified_positive_steps: unclassified,
        buckets: buckets
            .into_values()
            .map(|(outcome, count)| FailureBucketV1 { outcome, count })
            .collect(),
    })
}

fn duration_millis(duration: Duration) -> Result<u64> {
    let rounded = duration
        .as_nanos()
        .checked_add(999_999)
        .context("proof_availability_duration_overflow")?
        / 1_000_000;
    u64::try_from(rounded).context("proof_availability_duration_overflow")
}

fn duration_sum_millis<const N: usize>(durations: [Duration; N]) -> Result<u64> {
    let nanos = durations.into_iter().try_fold(0u128, |total, duration| {
        total
            .checked_add(duration.as_nanos())
            .context("proof_availability_duration_overflow")
    })?;
    u64::try_from(
        nanos
            .checked_add(999_999)
            .context("proof_availability_duration_overflow")?
            / 1_000_000,
    )
    .context("proof_availability_duration_overflow")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codestory_agent::proof_qualification_support::{
        ClauseAnchor, ClauseClassification, ProofContractField, UnvalidatedCallPathContract,
        UnvalidatedCallPathSpec, UnvalidatedDirectCallStep, UnvalidatedExactSymbolSelector,
    };
    use codestory_contracts::api::IndexMode;
    use codestory_contracts::graph::NodeKind;
    use std::fs;

    #[test]
    fn qualification_runner_has_no_retrieval_or_indexing_execution_path() {
        let production = include_str!("runner.rs")
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        for forbidden in [
            "run_indexing",
            "RuntimeRetrievalProfile::Agent",
            "search_service",
            "grounding_service",
            "retrieval_publication_service",
        ] {
            assert!(
                !production.contains(forbidden),
                "forbidden runner path {forbidden}"
            );
        }
        assert!(production.contains("run_observed_call_path_public_operation"));
    }

    #[test]
    fn source_built_case_runs_through_the_runtime_owned_kernel() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("src")).unwrap();
        fs::write(
            project.path().join("src/lib.rs"),
            "pub fn callee() {}\npub fn caller() { callee(); }\n",
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let database = project.path().join("core.sqlite3");
        let runtime = core_only_runtime(project.path(), cache.path());
        runtime
            .project_service()
            .open_project_summary_with_storage_path(project.path().to_path_buf(), database.clone())
            .unwrap();
        runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .unwrap();
        let store = Store::open_observational(&database).unwrap();
        let callables = store
            .get_nodes()
            .unwrap()
            .into_iter()
            .filter(|node| matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
            .collect::<Vec<_>>();
        let canonical = |name: &str| {
            callables
                .iter()
                .find(|node| node.serialized_name == name)
                .unwrap()
                .canonical_id
                .clone()
                .unwrap()
        };
        let caller = canonical("caller");
        let callee = canonical("callee");
        drop(store);
        let execute = |start: String, target: String| {
            let source = "exact direct ordered call path";
            let outcome = validate_contract(UnvalidatedCallPathContract::new(
                source,
                vec![ClauseAnchor {
                    clause_id: "contract".into(),
                    start: 0,
                    end: source.len(),
                    quote: source.into(),
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
                    start: UnvalidatedExactSymbolSelector::CanonicalId(start),
                    steps: vec![UnvalidatedDirectCallStep {
                        target: UnvalidatedExactSymbolSelector::CanonicalId(target),
                    }],
                    prohibit_traversal_through: vec![],
                    exclude_from_projection: vec![],
                },
            ))
            .unwrap();
            let ValidationOutcome::Validated {
                contract,
                hashes,
                rendering,
            } = outcome
            else {
                panic!("validated fixture")
            };
            let operation = run_observed_call_path_public_operation(
                &runtime,
                &contract,
                &hashes,
                &rendering,
                Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
            assert!(operation.retrieval_publication.is_none());
            observed_product_disposition_to_report(&operation.value).unwrap()
        };
        assert!(matches!(
            execute(caller.clone(), callee.clone()).actual,
            ActualProductResultV1::ContractProven { .. }
        ));
        assert!(!matches!(
            execute(caller, "fixture::absent_target".into()).actual,
            ActualProductResultV1::ContractProven { .. }
        ));
        assert!(!matches!(
            execute("fixture::absent_source".into(), callee).actual,
            ActualProductResultV1::ContractProven { .. }
        ));
    }

    #[test]
    fn finalization_tool_failure_produces_a_complete_invalid_case_report() {
        let path: super::super::contracts::OraclePathV1 = serde_json::from_value(
            serde_json::json!({
                "case_id":"fixture-case",
                "language":"rust",
                "source_text":"",
                "clauses":[],
                "spec":{
                    "start":{"kind":"canonical_id","canonical_id":"fixture::caller"},
                    "steps":[{"target":{"kind":"canonical_id","canonical_id":"fixture::callee"}}],
                    "prohibit_traversal_through":[],
                    "exclude_from_projection":[]
                },
                "oracle_steps":[{
                    "caller":{"symbol":"fixture::caller","selector":{"kind":"canonical_id","canonical_id":"fixture::caller"},"range":{"path":"src/lib.rs","start_byte":0,"end_byte":1,"file_byte_length":2,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
                    "callsite_line":1,
                    "callsite_expression":{"path":"src/lib.rs","start_byte":0,"end_byte":1,"file_byte_length":2,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                    "receipt_line_window":{"path":"src/lib.rs","start_byte":0,"end_byte":2,"file_byte_length":2,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                    "target":{"symbol":"fixture::callee","selector":{"kind":"canonical_id","canonical_id":"fixture::callee"},"range":{"path":"src/lib.rs","start_byte":1,"end_byte":2,"file_byte_length":2,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}
                }],
                "negative_mutations":[],
                "audit":{"source_area":"fixture","curator":"fixture-a","reviewer":"fixture-b","review_date":"2026-08-21"}
            }),
        )
        .expect("shaped fixture path");
        let observed = ObservedIntegratedProjectedCallPathResult {
            result: Err(codestory_contracts::api::ApiError::internal(
                "fixture finalization failure",
            )),
            trace: codestory_runtime::proof_qualification_support::ProofQualificationTrace {
                selectors: vec![
                    codestory_runtime::proof_qualification_support::SelectorQualificationTrace {
                        selector_index: 0,
                        outcome: codestory_runtime::proof_qualification_support::SelectorGateOutcome::Resolved {
                            node_id: codestory_contracts::graph::NodeId(1),
                        },
                    },
                    codestory_runtime::proof_qualification_support::SelectorQualificationTrace {
                        selector_index: 1,
                        outcome: codestory_runtime::proof_qualification_support::SelectorGateOutcome::Resolved {
                            node_id: codestory_contracts::graph::NodeId(2),
                        },
                    },
                ],
                selector_early_return: false,
                steps: vec![codestory_runtime::proof_qualification_support::StepQualificationTrace {
                    step_index: 0,
                    candidate_edge_ids: vec![codestory_contracts::graph::EdgeId(7)],
                    outcome: codestory_runtime::proof_qualification_support::StepQualificationOutcome::Admitted {
                        edge_ids: vec![codestory_contracts::graph::EdgeId(7)],
                    },
                }],
                finalization:
                    codestory_runtime::proof_qualification_support::FinalizationTrace::Failed(
                        codestory_runtime::proof_qualification_support::FinalizationFailure::ReceiptIntegration,
                    ),
            },
        };
        let negative_mutations = [
            super::super::contracts::NegativeMutationKindV1::ReplaceStepTarget,
            super::super::contracts::NegativeMutationKindV1::ReplaceStepSource,
        ]
        .into_iter()
        .enumerate()
        .map(|(index, kind)| NegativeMutationResultV1 {
            mutation_id: format!("fixture-mutation-{index}"),
            path_id: path.case_id.clone(),
            kind,
            step_index: 0,
            mutated_spec: path.spec.clone(),
            contract_proven: false,
        })
        .collect();

        let report = assemble_case_report(
            "fixture-repository",
            &path,
            &observed,
            Duration::from_millis(1),
            Duration::from_millis(2),
            negative_mutations,
        )
        .expect("tool failure remains a case result");

        assert!(matches!(
            report.product_disposition.actual,
            ActualProductResultV1::Invalid { ref failure }
                if failure.stage == super::super::contracts::ProductFailureStageV1::ToolExecution
                    && failure.code == "internal"
        ));
        assert_eq!(
            report.product_disposition.kind,
            super::super::contracts::ProductDispositionKindV1::Invalid
        );
        assert!(report.product_disposition.authoritative_receipts.is_empty());
        assert!(report.receipt_evidence.observed_receipts.is_empty());
        assert_eq!(report.receipt_evidence.missing_oracle_steps.len(), 1);
        assert_eq!(report.complete_projection_bytes, 0);
        assert!(matches!(
            report.transport,
            TransportEvidenceV1::Error {
                error: super::super::contracts::TransportErrorV1::InvalidProjection { ref projection }
            } if projection == "product_tool_failure"
        ));
        assert_eq!(report.negative_mutations.len(), 2);
        assert!(
            report
                .evaluable_facts()
                .expect("evaluable invalid case")
                .product_disposition_matches_evidence
        );
    }
}
