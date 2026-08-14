//! Runtime integration tests for the agent-owned packet obligation planner.

use crate::agent::packet_claims::packet_supported_claims;
use crate::agent::packet_freshness::fresh_index_observation;
use crate::agent::packet_obligations::*;
use crate::agent::packet_sufficiency::build_packet_sufficiency_with_obligation_context;
use crate::agent::path_identity::RuntimeWorkspacePathIdentity;
use codestory_contracts::api::*;
use std::path::Path;

const INDEXING_QUESTION: &str = "Explain the indexing runtime, persistence, and snapshot flow.";

fn packet_supported_claims_with_telemetry(answer: &AgentAnswerDto) -> (Vec<PacketClaimDto>, ()) {
    (packet_supported_claims(answer), ())
}

fn citation(name: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
    AgentCitationDto {
        node_id: NodeId(name.to_string()),
        display_name: name.to_string(),
        kind,
        file_path: Some(path.to_string()),
        line: Some(1),
        score: 1.0,
        origin: SearchHitOrigin::IndexedSymbol,
        target: None,
        resolvable: true,
        subgraph_id: None,
        evidence_edge_ids: Vec::new(),
        retrieval_score_breakdown: None,
        evidence_tier: Some(PacketEvidenceTierDto::ResolvedGraph),
        evidence_producer: Some("test".to_string()),
        resolution_status: Some(PacketEvidenceResolutionDto::Resolved),
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: Some(true),
    }
}

fn answer(citations: Vec<AgentCitationDto>) -> AgentAnswerDto {
    AgentAnswerDto {
        source_coverage: Vec::new(),
        answer_id: "obligation-test".to_string(),
        prompt: INDEXING_QUESTION.to_string(),
        summary: "test".to_string(),
        freshness: Some(fresh_index_observation()),
        sections: Vec::new(),
        citations,
        subgraph_ids: Vec::new(),
        retrieval_version: "test".to_string(),
        graphs: Vec::new(),
        retrieval_trace: AgentRetrievalTraceDto {
            request_id: "obligation-test".to_string(),
            retrieval_publication: None,
            resolved_profile: AgentRetrievalPresetDto::Architecture,
            policy_mode: AgentRetrievalPolicyModeDto::LatencyFirst,
            total_latency_ms: 1,
            sla_target_ms: None,
            sla_missed: false,
            semantic_fallback_count: 0,
            semantic_fallbacks: Vec::new(),
            semantic_stage_timeout_zero_hits: 0,
            semantic_abstained_count: 0,
            annotations: Vec::new(),
            packet_claim_profile_telemetry: None,
            source_freshness_telemetry: None,
            steps: Vec::new(),
            packet_sidecar_diagnostics: Vec::new(),
            retrieval_shadow: None,
        },
    }
}

fn budget() -> PacketBudgetDto {
    PacketBudgetDto {
        requested: PacketBudgetModeDto::Standard,
        limits: PacketBudgetLimitsDto {
            max_anchors: 16,
            max_files: 16,
            max_snippets: 16,
            max_trail_edges: 16,
            max_output_bytes: 64_000,
        },
        used: PacketBudgetUsageDto {
            anchors: 1,
            files: 1,
            snippets: 1,
            trail_edges: 0,
            output_bytes: 1_000,
        },
        truncated: false,
        omitted_sections: Vec::new(),
        next_deeper_command: None,
    }
}

fn finalized_obligation(
    id: &str,
    kind: PacketClaimObligationKindDto,
    proof_status: PacketObligationProofStatusDto,
    carrier: Option<&AgentCitationDto>,
    reason: Option<&str>,
) -> PacketClaimObligationDto {
    PacketClaimObligationDto {
        id: id.to_string(),
        kind,
        binding_terms: Vec::new(),
        probe_binding: None,
        material: true,
        allowed_node_kinds: Vec::new(),
        required_edge_kind: None,
        requires_complete_discovery: false,
        proof_status,
        reason: reason.map(str::to_string),
        carrier_node_ids: carrier
            .into_iter()
            .map(|citation| citation.node_id.clone())
            .collect(),
        carrier_paths: carrier
            .and_then(|citation| citation.file_path.clone())
            .into_iter()
            .collect(),
        carrier_edge_proofs: Vec::new(),
        open_next_candidates: Vec::new(),
    }
}

fn query_diagnostic(
    query: &str,
    completion: PacketQueryCompletionDto,
) -> PacketSidecarQueryDiagnosticDto {
    PacketSidecarQueryDiagnosticDto {
        query: query.to_string(),
        completion,
        retrieval_mode: "full".to_string(),
        sidecar_query_ms: Some(1),
        candidate_resolution_ms: Some(0),
        total_elapsed_ms: Some(1),
        sidecar_stage_count: 1,
        sidecar_stage_total_ms: Some(1),
        batch_query_wall_ms: Some(1),
        candidate_count: 0,
        resolved_hit_count: 0,
        unresolved_candidate_count: 0,
        blocking_unresolved_candidate_count: 0,
        semantic_stage_timeout_zero_hits: false,
        semantic_abstained: false,
        diagnostic: None,
    }
}

fn lexical_citation(name: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
    let mut citation = citation(name, path, kind);
    citation.evidence_tier = Some(PacketEvidenceTierDto::LexicalSource);
    citation.resolution_status = Some(PacketEvidenceResolutionDto::SourceRangeOnly);
    citation
}

fn answer_with_call_edge(question: &str, carrier_name: &str, carrier_path: &str) -> AgentAnswerDto {
    let mut carrier = citation(carrier_name, carrier_path, NodeKind::METHOD);
    carrier.evidence_edge_ids = vec![EdgeId("requested-call".to_string())];
    let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
    let mut answer = answer(vec![carrier.clone(), target.clone()]);
    answer.prompt = question.to_string();
    answer.graphs.push(GraphArtifactDto::Uml {
        id: "requested-flow".to_string(),
        title: "Requested flow".to_string(),
        graph: GraphResponse {
            center_id: carrier.node_id.clone(),
            nodes: Vec::new(),
            edges: vec![GraphEdgeDto {
                id: EdgeId("requested-call".to_string()),
                source: carrier.node_id,
                target: target.node_id,
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".to_string()),
                callsite_identity: Some("test:1".to_string()),
                candidate_targets: Vec::new(),
            }],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        },
    });
    answer
}

fn indexing_entrypoint_plan() -> PacketObligationPlanDto {
    let mut plan = build_packet_obligation_plan(
        INDEXING_QUESTION,
        PacketTaskClassDto::ArchitectureExplanation,
        &[],
    );
    plan.claim_obligations
        .retain(|obligation| obligation.id == "indexing_entrypoint");
    plan.query_obligations.clear();
    plan
}

#[test]
fn cancelled_diagnostic_query_does_not_block_a_complete_material_ledger() {
    let question = "Explain shell installer function dispatch and completion.";
    let mut plan =
        build_packet_obligation_plan(question, PacketTaskClassDto::ArchitectureExplanation, &[]);
    let mut answer = answer(vec![
        lexical_citation("nvm_download", "install.sh", NodeKind::FUNCTION),
        lexical_citation("nvm_command_dispatch", "nvm.sh", NodeKind::FUNCTION),
        citation("Helper::noop", "src/helper.rs", NodeKind::METHOD),
    ]);
    answer.prompt = question.to_string();
    let material_queries = plan
        .query_obligations
        .iter()
        .filter(|obligation| obligation.material)
        .map(|obligation| obligation.query.clone())
        .collect::<Vec<_>>();
    for query in material_queries {
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(query_diagnostic(
                &query,
                PacketQueryCompletionDto::Completed,
            ));
    }
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .push(query_diagnostic(
            "shell completion",
            PacketQueryCompletionDto::Cancelled {
                reason: "diagnostic_deadline".to_string(),
            },
        ));

    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::ArchitectureExplanation,
        &mut plan,
        &answer,
        &budget(),
    );

    assert!(plan.query_obligations.iter().any(|obligation| {
        !obligation.material
            && obligation.query == "shell completion"
            && matches!(
                obligation.completion,
                Some(PacketQueryCompletionDto::Cancelled { ref reason })
                    if reason == "diagnostic_deadline"
            )
    }));
    assert!(material_packet_obligations_are_proven(&plan), "{plan:#?}");
    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::ArchitectureExplanation,
        &answer,
        &budget(),
        &[],
        &[],
        &plan,
    );
    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
    assert!(
        sufficiency
            .coverage_report
            .as_ref()
            .is_some_and(|coverage| coverage.unresolved == ["shell completion"]),
        "diagnostic cancellation stays visible without changing the verdict: {sufficiency:?}"
    );
}

/// EV-8. A required query whose semantic stage timed out with zero hits reaches the ledger as
/// a typed cancellation, and the ledger is what demotes the verdict. The demotion lands on
/// the obligation that lost its evidence rather than on the packet as an undifferentiated
/// whole, so the caller is told which query to re-run.
#[test]
fn a_required_query_whose_semantic_stage_timed_out_demotes_through_its_obligation() {
    let question = "Explain shell installer function dispatch and completion.";
    let mut plan =
        build_packet_obligation_plan(question, PacketTaskClassDto::ArchitectureExplanation, &[]);
    let mut answer = answer(vec![
        lexical_citation("nvm_download", "install.sh", NodeKind::FUNCTION),
        lexical_citation("nvm_command_dispatch", "nvm.sh", NodeKind::FUNCTION),
        citation("Helper::noop", "src/helper.rs", NodeKind::METHOD),
    ]);
    answer.prompt = question.to_string();
    let material_queries = plan
        .query_obligations
        .iter()
        .filter(|obligation| obligation.material)
        .map(|obligation| obligation.query.clone())
        .collect::<Vec<_>>();
    let (timed_out_query, completed_queries) = material_queries
        .split_first()
        .expect("architecture explanation plans at least one material query");
    for query in completed_queries {
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(query_diagnostic(query, PacketQueryCompletionDto::Completed));
    }
    let mut timed_out = query_diagnostic(
        timed_out_query,
        PacketQueryCompletionDto::Cancelled {
            reason: "semantic_stage_timeout_zero_hits".to_string(),
        },
    );
    timed_out.semantic_stage_timeout_zero_hits = true;
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .push(timed_out);

    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::ArchitectureExplanation,
        &mut plan,
        &answer,
        &budget(),
    );

    assert!(
        plan.query_obligations.iter().any(|obligation| {
            obligation.material
                && &obligation.query == timed_out_query
                && matches!(
                    obligation.completion,
                    Some(PacketQueryCompletionDto::Cancelled { ref reason })
                        if reason == "semantic_stage_timeout_zero_hits"
                )
        }),
        "the typed cancellation must reach the ledger: {plan:#?}"
    );

    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::ArchitectureExplanation,
        &answer,
        &budget(),
        &[],
        &[],
        &plan,
    );

    assert_eq!(
        sufficiency.status,
        PacketSufficiencyStatusDto::Partial,
        "{sufficiency:?}"
    );
    assert!(
        sufficiency.gaps.iter().any(|gap| {
            gap.contains("query obligation") && gap.contains("semantic_stage_timeout_zero_hits")
        }),
        "the gap must name the query obligation that lost its lane: {:?}",
        sufficiency.gaps
    );
    assert!(
        sufficiency
            .follow_up_commands
            .iter()
            .any(|command| command.contains(timed_out_query.as_str())),
        "the caller is told which query to re-run: {sufficiency:?}"
    );
}

#[test]
fn indexing_storage_claim_cannot_borrow_a_proven_runtime_entrypoint() {
    let question = "Explain how a full indexing run moves from the CLI into runtime orchestration, workspace file discovery, symbol extraction, persistence, and snapshot refresh.";
    let mut runtime = citation(
        "IndexService::run_indexing_blocking_without_runtime_refresh",
        "crates/codestory-runtime/src/services.rs",
        NodeKind::METHOD,
    );
    runtime.evidence_edge_ids = vec![EdgeId("indexing-entry-call".to_string())];
    let target = citation(
        "Worker::execute",
        "crates/example/src/worker.rs",
        NodeKind::METHOD,
    );
    let mut answer = answer(vec![runtime.clone(), target.clone()]);
    answer.prompt = question.to_string();
    answer.graphs.push(GraphArtifactDto::Uml {
        id: "indexing-entrypoint-flow".to_string(),
        title: "Indexing entrypoint flow".to_string(),
        graph: GraphResponse {
            center_id: runtime.node_id.clone(),
            nodes: Vec::new(),
            edges: vec![GraphEdgeDto {
                id: EdgeId("indexing-entry-call".to_string()),
                source: runtime.node_id.clone(),
                target: target.node_id,
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".to_string()),
                callsite_identity: Some("test:1".to_string()),
                candidate_targets: Vec::new(),
            }],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        },
    });
    let mut plan =
        build_packet_obligation_plan(question, PacketTaskClassDto::ArchitectureExplanation, &[]);
    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::ArchitectureExplanation,
        &mut plan,
        &answer,
        &budget(),
    );
    assert_eq!(
        plan.claim_obligations
            .iter()
            .find(|obligation| obligation.id == "indexing_entrypoint")
            .map(|obligation| obligation.proof_status),
        Some(PacketObligationProofStatusDto::Proven)
    );
    let storage_obligation = plan
        .claim_obligations
        .iter()
        .find(|obligation| obligation.id == "indexing_storage")
        .expect("indexing storage obligation");
    assert_eq!(
        storage_obligation.proof_status,
        PacketObligationProofStatusDto::Unsupported
    );
    assert_eq!(
        storage_obligation.reason.as_deref(),
        Some("required_carrier_missing")
    );

    let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(&answer);
    let mut claims = packet_claims_with_obligation_receipts(
        &answer,
        PacketTaskClassDto::ArchitectureExplanation,
        &plan,
        supported_claims_with_telemetry,
    );
    bind_claims_to_packet_obligations(&plan, &mut claims);
    let storage_claim = claims
        .iter()
        .find(|claim| claim.required_obligation_ids == ["indexing_storage"])
        .expect("material storage row emits an exact receipt claim");
    assert_eq!(storage_claim.required_obligation_ids, ["indexing_storage"]);
    assert_eq!(
        storage_claim.proof_status,
        Some(PacketProofStatusDto::Unsupported)
    );
    assert_eq!(storage_claim.eligible_for_sufficiency, Some(false));
    assert!(storage_claim.claim.contains("required_carrier_missing"));
    let entrypoint_claim = claims
        .iter()
        .find(|claim| claim.required_obligation_ids == ["indexing_entrypoint"])
        .expect("material entrypoint row emits an exact receipt claim");
    assert_eq!(
        entrypoint_claim.proof_status,
        Some(PacketProofStatusDto::Proven)
    );
    assert_eq!(entrypoint_claim.citations.len(), 1);
}

#[test]
fn obligation_receipts_use_only_their_exact_rows_own_carriers() {
    let entrypoint = citation("Cli::run", "src/cli.rs", NodeKind::METHOD);
    let storage = citation("Store::write", "src/store.rs", NodeKind::METHOD);
    let answer = answer(vec![entrypoint.clone(), storage.clone()]);
    let plan = PacketObligationPlanDto {
        version: PACKET_OBLIGATION_PLAN_VERSION,
        binding_terms: Vec::new(),
        claim_obligations: vec![
            finalized_obligation(
                "flow_entrypoint",
                PacketClaimObligationKindDto::Entrypoint,
                PacketObligationProofStatusDto::Proven,
                Some(&entrypoint),
                None,
            ),
            finalized_obligation(
                "flow_storage",
                PacketClaimObligationKindDto::StateWrite,
                PacketObligationProofStatusDto::Proven,
                Some(&storage),
                None,
            ),
        ],
        query_obligations: Vec::new(),
    };

    let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(&answer);
    let mut claims = packet_claims_with_obligation_receipts(
        &answer,
        PacketTaskClassDto::ArchitectureExplanation,
        &plan,
        supported_claims_with_telemetry,
    );
    bind_claims_to_packet_obligations(&plan, &mut claims);
    let receipts = claims
        .iter()
        .filter(|claim| {
            claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 2);
    let entrypoint_receipt = receipts
        .iter()
        .find(|claim| claim.required_obligation_ids == ["flow_entrypoint"])
        .expect("entrypoint receipt");
    assert_eq!(entrypoint_receipt.citations.len(), 1);
    assert_eq!(entrypoint_receipt.citations[0].node_id, entrypoint.node_id);
    let storage_receipt = receipts
        .iter()
        .find(|claim| claim.required_obligation_ids == ["flow_storage"])
        .expect("storage receipt");
    assert_eq!(storage_receipt.citations.len(), 1);
    assert_eq!(storage_receipt.citations[0].node_id, storage.node_id);

    let mut borrowed = vec![PacketClaimDto {
        claim: "The entrypoint row is proven.".to_string(),
        required_obligation_ids: vec!["flow_entrypoint".to_string()],
        required_obligation_kinds: vec![PacketClaimObligationKindDto::Entrypoint],
        proof_status: Some(PacketProofStatusDto::Proven),
        required_evidence_role: None,
        citations: vec![storage],
        coverage_role: Some("fixture".to_string()),
        eligible_for_sufficiency: Some(true),
    }];
    bind_claims_to_packet_obligations(&plan, &mut borrowed);
    assert_eq!(
        borrowed[0].proof_status,
        Some(PacketProofStatusDto::Reported)
    );
    assert_eq!(borrowed[0].eligible_for_sufficiency, Some(false));
}

#[test]
fn receipt_rows_preserve_non_proven_status_reason_and_deduplicate_ids() {
    let reported_carrier = citation("MaybeStore", "src/store.rs", NodeKind::METHOD);
    let plan = PacketObligationPlanDto {
        version: PACKET_OBLIGATION_PLAN_VERSION,
        binding_terms: Vec::new(),
        claim_obligations: vec![
            finalized_obligation(
                "reported_storage",
                PacketClaimObligationKindDto::StateWrite,
                PacketObligationProofStatusDto::Reported,
                Some(&reported_carrier),
                Some("required_evidence_edge_missing"),
            ),
            finalized_obligation(
                "reported_storage",
                PacketClaimObligationKindDto::StateWrite,
                PacketObligationProofStatusDto::Reported,
                Some(&reported_carrier),
                Some("duplicate_row"),
            ),
            finalized_obligation(
                "unsupported_dispatch",
                PacketClaimObligationKindDto::Dispatch,
                PacketObligationProofStatusDto::Unsupported,
                None,
                Some("required_carrier_missing"),
            ),
        ],
        query_obligations: Vec::new(),
    };
    let answer = answer(vec![reported_carrier]);
    let supported_claims_with_telemetry = packet_supported_claims_with_telemetry(&answer);
    let mut claims = packet_claims_with_obligation_receipts(
        &answer,
        PacketTaskClassDto::ArchitectureExplanation,
        &plan,
        supported_claims_with_telemetry,
    );
    bind_claims_to_packet_obligations(&plan, &mut claims);
    let receipts = claims
        .iter()
        .filter(|claim| {
            claim.coverage_role.as_deref() == Some(PACKET_OBLIGATION_RECEIPT_COVERAGE_ROLE)
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 2, "duplicate row IDs emit one receipt");
    let reported = receipts
        .iter()
        .find(|claim| claim.required_obligation_ids == ["reported_storage"])
        .expect("reported receipt");
    assert_eq!(reported.proof_status, Some(PacketProofStatusDto::Reported));
    assert_eq!(reported.eligible_for_sufficiency, Some(false));
    assert!(reported.claim.contains("required_evidence_edge_missing"));
    let unsupported = receipts
        .iter()
        .find(|claim| claim.required_obligation_ids == ["unsupported_dispatch"])
        .expect("unsupported receipt");
    assert_eq!(
        unsupported.proof_status,
        Some(PacketProofStatusDto::Unsupported)
    );
    assert!(unsupported.claim.contains("required_carrier_missing"));
}

#[test]
fn sufficient_profile_closes_incidental_nonmaterial_guard_path() {
    let question = "Find RuntimeService::run.";
    let mut carrier = citation(
        "RuntimeService::run",
        "src/runtime_service.rs",
        NodeKind::METHOD,
    );
    carrier.evidence_edge_ids = vec![EdgeId("generic-call".to_string())];
    let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
    let incidental_guard_carrier = citation(
        "HttpTransport::send",
        "src/incidental_transport.rs",
        NodeKind::METHOD,
    );
    let mut answer = answer(vec![
        carrier.clone(),
        target.clone(),
        incidental_guard_carrier.clone(),
    ]);
    answer.prompt = question.to_string();
    answer
        .retrieval_trace
        .packet_sidecar_diagnostics
        .push(query_diagnostic(
            "RuntimeService::run",
            PacketQueryCompletionDto::Completed,
        ));
    answer.graphs.push(GraphArtifactDto::Uml {
        id: "generic-flow".to_string(),
        title: "Generic flow".to_string(),
        graph: GraphResponse {
            center_id: carrier.node_id.clone(),
            nodes: Vec::new(),
            edges: vec![GraphEdgeDto {
                id: EdgeId("generic-call".to_string()),
                source: carrier.node_id,
                target: target.node_id,
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".to_string()),
                callsite_identity: Some("test:1".to_string()),
                candidate_targets: Vec::new(),
            }],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        },
    });
    let mut plan = build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
    plan.claim_obligations.push(PacketClaimObligationDto {
        id: "incidental_external_io_guard".to_string(),
        kind: PacketClaimObligationKindDto::ExternalIo,
        binding_terms: vec![incidental_guard_carrier.display_name.clone()],
        probe_binding: None,
        material: false,
        allowed_node_kinds: vec![NodeKind::FUNCTION, NodeKind::METHOD, NodeKind::MACRO],
        required_edge_kind: Some(EdgeKind::CALL),
        requires_complete_discovery: false,
        proof_status: PacketObligationProofStatusDto::Planned,
        reason: None,
        carrier_node_ids: Vec::new(),
        carrier_paths: Vec::new(),
        carrier_edge_proofs: Vec::new(),
        open_next_candidates: Vec::new(),
    });
    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::SymbolOwnership,
        &mut plan,
        &answer,
        &budget(),
    );

    assert_eq!(
        plan.claim_obligations[0].proof_status,
        PacketObligationProofStatusDto::Proven
    );
    assert_eq!(plan.claim_obligations[0].reason, None);
    assert!(material_packet_obligations_are_proven(&plan), "{plan:#?}");
    assert!(plan.query_obligations.iter().any(|obligation| {
        obligation.material
            && obligation.query == "RuntimeService::run"
            && obligation.completion == Some(PacketQueryCompletionDto::Completed)
    }));
    let incidental_guard = plan
        .claim_obligations
        .iter()
        .find(|obligation| obligation.id == "incidental_external_io_guard")
        .expect("incidental guard remains in the finalized ledger");
    assert!(!incidental_guard.material);
    assert_eq!(
        incidental_guard.proof_status,
        PacketObligationProofStatusDto::Reported
    );
    assert_eq!(
        incidental_guard.carrier_paths,
        vec!["src/incidental_transport.rs".to_string()]
    );
    assert!(
        packet_obligation_open_next_candidates(&plan)
            .contains(&"src/incidental_transport.rs".to_string()),
        "the raw unproven guard receipt must exercise the terminal open-next gate"
    );
    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::SymbolOwnership,
        &answer,
        &budget(),
        &[],
        &[],
        &plan,
    );
    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Sufficient);
    assert!(
        sufficiency
            .covered_claims
            .iter()
            .any(|claim| claim.proof_status == Some(PacketProofStatusDto::Proven))
    );
    assert!(
        !sufficiency
            .avoid_opening_paths
            .contains(&"src/incidental_transport.rs".to_string()),
        "an unproven incidental guard carrier cannot become avoid-opening"
    );
    let coverage = sufficiency
        .coverage_report
        .as_ref()
        .expect("sufficiency publishes claim-level blockers");
    assert!(
        coverage.ineligible.iter().any(|entry| {
            entry.contains("RuntimeService::run") && entry.contains("claim marked diagnostic")
        }),
        "the per-file safety blocker must remain visible: {coverage:?}"
    );
    assert!(
        sufficiency.open_next.is_empty(),
        "Sufficient is terminal even when a nonmaterial Reported guard carries a path: {sufficiency:?}"
    );
}

#[test]
fn case_distinct_exact_symbols_need_separate_carriers() {
    let question = "Find Foo::run and foo::run.";
    let mut answer = answer_with_call_edge(question, "Foo::run", "src/runtime.rs");
    for query in ["Foo::run", "foo::run"] {
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(query_diagnostic(query, PacketQueryCompletionDto::Completed));
    }
    let mut plan = build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);

    assert!(
        plan.query_obligations
            .iter()
            .any(|obligation| { obligation.material && obligation.query == "Foo::run" })
    );
    assert!(
        plan.query_obligations
            .iter()
            .any(|obligation| { obligation.material && obligation.query == "foo::run" })
    );
    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::SymbolOwnership,
        &mut plan,
        &answer,
        &budget(),
    );

    let upper = plan
        .claim_obligations
        .iter()
        .find(|obligation| obligation.binding_terms == ["Foo::run"])
        .unwrap();
    let lower = plan
        .claim_obligations
        .iter()
        .find(|obligation| obligation.binding_terms == ["foo::run"])
        .unwrap();
    assert_eq!(upper.proof_status, PacketObligationProofStatusDto::Proven);
    assert_eq!(
        lower.proof_status,
        PacketObligationProofStatusDto::Unsupported
    );
    assert!(lower.carrier_node_ids.is_empty());
    assert!(!material_packet_obligations_are_proven(&plan));

    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::SymbolOwnership,
        &answer,
        &budget(),
        &[],
        &[],
        &plan,
    );
    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
}

#[test]
fn case_distinct_slash_qualified_symbols_need_separate_carriers() {
    let question = "Find Foo/run and foo/run.";
    let mut answer = answer_with_call_edge(question, "Foo/run", "src/runtime.rs");
    for query in ["Foo/run", "foo/run"] {
        answer
            .retrieval_trace
            .packet_sidecar_diagnostics
            .push(query_diagnostic(query, PacketQueryCompletionDto::Completed));
    }
    let mut plan = build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);

    for expected in ["Foo/run", "foo/run"] {
        assert!(
            plan.query_obligations
                .iter()
                .any(|obligation| { obligation.material && obligation.query == expected })
        );
        assert!(
            plan.claim_obligations.iter().any(|obligation| {
                obligation.material && obligation.binding_terms == [expected]
            })
        );
    }
    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::SymbolOwnership,
        &mut plan,
        &answer,
        &budget(),
    );

    let upper = plan
        .claim_obligations
        .iter()
        .find(|obligation| obligation.binding_terms == ["Foo/run"])
        .unwrap();
    let lower = plan
        .claim_obligations
        .iter()
        .find(|obligation| obligation.binding_terms == ["foo/run"])
        .unwrap();
    assert_eq!(upper.proof_status, PacketObligationProofStatusDto::Proven);
    assert_eq!(
        lower.proof_status,
        PacketObligationProofStatusDto::Unsupported
    );
    assert!(lower.carrier_node_ids.is_empty());

    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::SymbolOwnership,
        &answer,
        &budget(),
        &[],
        &[],
        &plan,
    );
    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
}

#[test]
fn one_proven_claim_cannot_hide_an_unproven_claim_in_the_same_file() {
    let question = "Find RuntimeService::run and CliErrorBody.";
    let shared_path = "src/runtime_service.rs";
    let mut carrier = citation("RuntimeService::run", shared_path, NodeKind::METHOD);
    carrier.evidence_edge_ids = vec![EdgeId("generic-call".to_string())];
    let false_carrier = citation("CliErrorBody", shared_path, NodeKind::STRUCT);
    let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
    let mut answer = answer(vec![carrier.clone(), false_carrier, target.clone()]);
    answer.prompt = question.to_string();
    answer.graphs.push(GraphArtifactDto::Uml {
        id: "generic-flow".to_string(),
        title: "Generic flow".to_string(),
        graph: GraphResponse {
            center_id: carrier.node_id.clone(),
            nodes: Vec::new(),
            edges: vec![GraphEdgeDto {
                id: EdgeId("generic-call".to_string()),
                source: carrier.node_id,
                target: target.node_id,
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".to_string()),
                callsite_identity: Some("test:1".to_string()),
                candidate_targets: Vec::new(),
            }],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        },
    });
    let mut plan = build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::SymbolOwnership,
        &mut plan,
        &answer,
        &budget(),
    );
    assert!(plan.claim_obligations.iter().any(|obligation| {
        obligation.proof_status == PacketObligationProofStatusDto::Proven
            && obligation
                .carrier_paths
                .iter()
                .any(|path| path == shared_path)
    }));
    assert!(plan.claim_obligations.iter().any(|obligation| {
        obligation.proof_status == PacketObligationProofStatusDto::Reported
            && obligation
                .carrier_paths
                .iter()
                .any(|path| path == shared_path)
    }));

    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::SymbolOwnership,
        &answer,
        &budget(),
        &[],
        &[],
        &plan,
    );

    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
    assert!(!material_packet_obligations_are_proven(&plan));
    assert!(
        !sufficiency
            .avoid_opening_paths
            .iter()
            .any(|path| path == shared_path),
        "a file with an unproven claim must remain openable: {sufficiency:?}"
    );
    assert!(
        sufficiency
            .open_next
            .iter()
            .any(|command| command.contains(shared_path)),
        "the unproven same-file claim must stay open-next: {sufficiency:?}"
    );
}

#[test]
fn missing_requested_claim_gets_a_material_unsupported_obligation() {
    let question = "Find RuntimeService::run and MissingWidget.";
    let mut carrier = citation(
        "RuntimeService::run",
        "src/runtime_service.rs",
        NodeKind::METHOD,
    );
    carrier.evidence_edge_ids = vec![EdgeId("generic-call".to_string())];
    let target = citation("Worker::run", "src/worker.rs", NodeKind::METHOD);
    let mut answer = answer(vec![carrier.clone(), target.clone()]);
    answer.prompt = question.to_string();
    answer.graphs.push(GraphArtifactDto::Uml {
        id: "generic-flow".to_string(),
        title: "Generic flow".to_string(),
        graph: GraphResponse {
            center_id: carrier.node_id.clone(),
            nodes: Vec::new(),
            edges: vec![GraphEdgeDto {
                id: EdgeId("generic-call".to_string()),
                source: carrier.node_id,
                target: target.node_id,
                kind: EdgeKind::CALL,
                confidence: Some(1.0),
                certainty: Some("certain".to_string()),
                callsite_identity: Some("test:1".to_string()),
                candidate_targets: Vec::new(),
            }],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        },
    });
    let mut plan = build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::SymbolOwnership,
        &mut plan,
        &answer,
        &budget(),
    );

    let missing = plan
        .claim_obligations
        .iter()
        .find(|obligation| obligation.binding_terms == ["MissingWidget"])
        .expect("material obligation for the missing requested claim");
    assert!(missing.material);
    assert_eq!(
        missing.proof_status,
        PacketObligationProofStatusDto::Unsupported
    );
    assert!(missing.carrier_node_ids.is_empty());
    assert!(!material_packet_obligations_are_proven(&plan));

    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::SymbolOwnership,
        &answer,
        &budget(),
        &[],
        &[],
        &plan,
    );
    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
    assert!(
        sufficiency
            .open_next
            .iter()
            .any(|command| command.contains("MissingWidget"))
    );
}

#[test]
fn false_shape_carrier_stays_reported_partial_and_open_next() {
    let false_carrier = citation(
        "CliErrorBody",
        "crates/example/src/indexer/runtime.rs",
        NodeKind::STRUCT,
    );
    let answer = answer(vec![false_carrier]);
    let budget = budget();
    let mut plan = indexing_entrypoint_plan();
    finalize_packet_obligation_plan(
        INDEXING_QUESTION,
        PacketTaskClassDto::ArchitectureExplanation,
        &mut plan,
        &answer,
        &budget,
    );

    assert_eq!(
        plan.claim_obligations[0].proof_status,
        PacketObligationProofStatusDto::Reported
    );
    assert_eq!(
        plan.claim_obligations[0].reason.as_deref(),
        Some("carrier_does_not_satisfy_role_contract")
    );
    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        INDEXING_QUESTION,
        PacketTaskClassDto::ArchitectureExplanation,
        &answer,
        &budget,
        &[],
        &[],
        &plan,
    );
    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
    assert!(sufficiency.avoid_opening_paths.is_empty());
    assert!(
        sufficiency
            .open_next
            .iter()
            .any(|command| command.contains("crates/example/src/indexer/runtime.rs"))
    );
}

#[test]
fn known_false_carriers_publish_no_proven_claims_and_stay_open_next() {
    let question = "Explain CliErrorBody, runtime_path, and CompilationDatabase responsibilities.";
    let false_carriers = vec![
        citation("CliErrorBody", "src/cli/errors.rs", NodeKind::STRUCT),
        citation("runtime_path", "src/runtime/config.rs", NodeKind::VARIABLE),
        citation(
            "CompilationDatabase",
            "src/store/database.rs",
            NodeKind::CLASS,
        ),
    ];
    let mut answer = answer(false_carriers.clone());
    answer.prompt = question.to_string();
    let budget = budget();
    let mut plan = build_packet_obligation_plan(question, PacketTaskClassDto::SymbolOwnership, &[]);
    finalize_packet_obligation_plan(
        question,
        PacketTaskClassDto::SymbolOwnership,
        &mut plan,
        &answer,
        &budget,
    );

    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        question,
        PacketTaskClassDto::SymbolOwnership,
        &answer,
        &budget,
        &[],
        &[],
        &plan,
    );

    assert_eq!(sufficiency.status, PacketSufficiencyStatusDto::Partial);
    assert!(
        sufficiency
            .covered_claims
            .iter()
            .all(|claim| { claim.proof_status != Some(PacketProofStatusDto::Proven) })
    );
    assert!(sufficiency.avoid_opening_paths.is_empty());
    for carrier in false_carriers {
        let path = carrier.file_path.expect("false carrier path");
        assert!(
            sufficiency
                .open_next
                .iter()
                .any(|command| command.contains(&path)),
            "{path} must remain open-next: {:?}",
            sufficiency.open_next
        );
    }
}

#[test]
fn missing_carrier_keeps_requested_path_as_open_next_candidate() {
    let queries = [PacketPlanQueryDto {
        query: "src/indexer.rs".to_string(),
        purpose: "explicit symbol probe from packet request".to_string(),
    }];
    let mut plan = build_packet_obligation_plan(
        INDEXING_QUESTION,
        PacketTaskClassDto::ArchitectureExplanation,
        &queries,
    );
    plan.claim_obligations
        .retain(|obligation| obligation.id == "indexing_entrypoint");
    let answer = answer(Vec::new());
    let budget = budget();
    finalize_packet_obligation_plan(
        INDEXING_QUESTION,
        PacketTaskClassDto::ArchitectureExplanation,
        &mut plan,
        &answer,
        &budget,
    );
    let sufficiency = build_packet_sufficiency_with_obligation_context(
        &RuntimeWorkspacePathIdentity,
        Path::new("/workspace/example"),
        INDEXING_QUESTION,
        PacketTaskClassDto::ArchitectureExplanation,
        &answer,
        &budget,
        &[],
        &[],
        &plan,
    );

    assert!(
        sufficiency
            .open_next
            .iter()
            .any(|command| command.contains("src/indexer.rs"))
    );
}
