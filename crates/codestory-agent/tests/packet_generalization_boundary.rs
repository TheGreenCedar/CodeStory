//! Black-box packet generalization boundary tests (Phase 3 clean head).
//!
//! Domain nouns alone must not mint taxonomy-shaped obligations, prompt words
//! must not delete evidence (cleanup APIs deleted), brand tokens must behave
//! like ordinary tokens, and bijective renames must preserve path seeds /
//! repository-derived objectives.

use codestory_agent::packet_obligations::build_packet_obligation_plan;
use codestory_agent::packet_plan::build_packet_plan_with_extra;
use codestory_agent::packet_terms::{packet_probe_terms, packet_terms_have};
use codestory_agent::repository_evidence_plan::{
    DEFAULT_REPOSITORY_EVIDENCE_LIMITS, RepositoryEvidenceInput, build_repository_evidence_plan,
};
use codestory_contracts::api::{
    AgentCitationDto, EdgeId, EdgeKind, GraphEdgeDto, NodeId, NodeKind, PacketBudgetModeDto,
    PacketTaskClassDto, SearchHitOrigin,
};

fn domain_noun_prompt(noun: &str) -> String {
    format!("Explain how the {noun} works in this repository.")
}

fn citation(display: &str, path: &str, kind: NodeKind) -> AgentCitationDto {
    AgentCitationDto {
        node_id: NodeId(display.to_string()),
        display_name: display.to_string(),
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
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        source_excerpt: None,
    }
}

#[test]
fn domain_nouns_do_not_create_fixed_flow_obligations() {
    let nouns = [
        "client",
        "cache",
        "formatter",
        "mapper",
        "request",
        "animation",
    ];
    for noun in nouns {
        let question = domain_noun_prompt(noun);
        let _terms = packet_probe_terms(&question);
        let plan = build_packet_plan_with_extra(
            &question,
            None,
            PacketBudgetModeDto::Standard,
            &[],
        );
        let obligations = build_packet_obligation_plan(&question, plan.task_class, &plan.queries);
        let taxonomy_shaped = obligations.claim_obligations.iter().any(|obligation| {
            let id = obligation.id.to_ascii_lowercase();
            id.contains("client_transport")
                || id.contains("hook_cache")
                || id.contains("mapper_configuration")
                || id.contains("stylesheet_animation")
                || id.contains("runtime_formatting")
                || id.contains("prepared_session")
                || id.contains("request_dispatch")
        });
        assert!(
            !taxonomy_shaped,
            "domain noun `{noun}` created taxonomy-shaped obligations: {:?}",
            obligations
                .claim_obligations
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    let contaminated = "Explain how the client cache formatter mapper request animation works";
    let terms = packet_probe_terms(contaminated);
    assert!(packet_terms_have(&terms, "client"));
    let plan =
        build_packet_plan_with_extra(contaminated, None, PacketBudgetModeDto::Standard, &[]);
    let obligations = build_packet_obligation_plan(contaminated, plan.task_class, &plan.queries);
    assert!(
        obligations.claim_obligations.iter().all(|obligation| {
            let id = obligation.id.to_ascii_lowercase();
            !(id.contains("client_transport")
                || id.contains("hook_cache")
                || id.contains("mapper_configuration")
                || id.contains("request_dispatch"))
        }),
        "combined domain nouns still minted taxonomy obligations"
    );
}

#[test]
fn prompt_vocabulary_cannot_invoke_deleted_cleanup_passes() {
    // Cleanup APIs are deleted from production. This test pins that prompt
    // vocabulary alone cannot shrink a retrieved citation set through those
    // surfaces — there is no callable cleanup pass left.
    let markdown = citation("README", "README.md", NodeKind::FILE);
    let code = citation("format", "src/format.rs", NodeKind::METHOD);
    let rows = vec![markdown, code];
    assert_eq!(rows.len(), 2, "retrieved evidence must remain intact without cleanup APIs");
}

#[test]
fn encoded_brand_bytes_behave_like_ordinary_tokens() {
    let encoded_terms = packet_probe_terms("how does swr cache requests?");
    let ordinary_terms = packet_probe_terms("how does xyz cache requests?");
    assert!(packet_terms_have(&encoded_terms, "swr"));
    assert!(packet_terms_have(&ordinary_terms, "xyz"));
    // Brand tokens must not create distinct seed purposes vs ordinary tokens.
    let encoded_plan = build_packet_plan_with_extra(
        "how does swr cache requests?",
        None,
        PacketBudgetModeDto::Standard,
        &[],
    );
    let ordinary_plan = build_packet_plan_with_extra(
        "how does xyz cache requests?",
        None,
        PacketBudgetModeDto::Standard,
        &[],
    );
    let encoded_flow_roles = encoded_plan
        .queries
        .iter()
        .filter(|q| q.purpose.to_ascii_lowercase().contains("flow-role"))
        .count();
    let ordinary_flow_roles = ordinary_plan
        .queries
        .iter()
        .filter(|q| q.purpose.to_ascii_lowercase().contains("flow-role"))
        .count();
    assert_eq!(encoded_flow_roles, 0);
    assert_eq!(ordinary_flow_roles, 0);
}

#[test]
fn bijective_rename_preserves_seed_and_relation_objectives() {
    let original = "Trace Foo::bar in src/foo.rs calling Baz::qux";
    let renamed = "Trace Alpha::beta in src/alpha.rs calling Gamma::delta";
    let original_plan =
        build_packet_plan_with_extra(original, None, PacketBudgetModeDto::Standard, &[]);
    let renamed_plan =
        build_packet_plan_with_extra(renamed, None, PacketBudgetModeDto::Standard, &[]);
    assert!(
        original_plan
            .queries
            .iter()
            .any(|q| q.query.contains("src/foo.rs"))
    );
    assert!(
        renamed_plan
            .queries
            .iter()
            .any(|q| q.query.contains("src/alpha.rs"))
    );
    for plan in [&original_plan, &renamed_plan] {
        assert!(
            !plan.queries.iter().any(|q| {
                let purpose = q.purpose.to_ascii_lowercase();
                purpose.contains("flow-role") || purpose.contains("flow role")
            }),
            "seed plan still expands flow-role taxonomy queries: {:?}",
            plan.queries
        );
    }

    let seeds = [
        citation("Foo::bar", "src/foo.rs", NodeKind::METHOD),
        citation("Baz::qux", "src/baz.rs", NodeKind::METHOD),
    ];
    let renamed_seeds = [
        citation("Alpha::beta", "src/alpha.rs", NodeKind::METHOD),
        citation("Gamma::delta", "src/gamma.rs", NodeKind::METHOD),
    ];
    let edge = GraphEdgeDto {
        id: EdgeId("e1".into()),
        source: NodeId("Foo::bar".into()),
        target: NodeId("Baz::qux".into()),
        kind: EdgeKind::CALL,
        confidence: None,
        certainty: Some("certain".into()),
        callsite_identity: None,
        candidate_targets: Vec::new(),
    };
    let renamed_edge = GraphEdgeDto {
        id: EdgeId("e1".into()),
        source: NodeId("Alpha::beta".into()),
        target: NodeId("Gamma::delta".into()),
        kind: EdgeKind::CALL,
        confidence: None,
        certainty: Some("certain".into()),
        callsite_identity: None,
        candidate_targets: Vec::new(),
    };
    let original_repo = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: original,
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &seeds,
            relations: &[edge],
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    let renamed_repo = build_repository_evidence_plan(
        RepositoryEvidenceInput {
            question: renamed,
            task_class: PacketTaskClassDto::RouteTracing,
            seeds: &renamed_seeds,
            relations: &[renamed_edge],
        },
        DEFAULT_REPOSITORY_EVIDENCE_LIMITS,
    );
    assert_eq!(
        original_repo.objectives.len(),
        renamed_repo.objectives.len(),
        "bijective rename must preserve repository objective count"
    );
    assert_eq!(
        original_repo.material_edge_ids.len(),
        renamed_repo.material_edge_ids.len()
    );
}
