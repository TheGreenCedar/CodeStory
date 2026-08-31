//! Black-box packet generalization boundary tests (Phase 2→3).
//!
//! These encode the decontaminated planner contract: domain nouns alone must
//! not create fixed obligations, prompt words must not delete evidence, encoded
//! brand bytes must behave like ordinary tokens, and bijective renames must
//! preserve repository-derived objectives.
//!
//! On the contaminated head these tests fail. Phase 3 makes them pass by
//! deleting taxonomy/cleanup surfaces and landing `repository_evidence_plan`.

use codestory_agent::packet_obligations::build_packet_obligation_plan;
use codestory_agent::packet_plan::build_packet_plan_with_extra;
use codestory_agent::packet_scoring::{
    packet_drop_unrequested_markdown_siblings, packet_terms_contain,
};
use codestory_agent::packet_terms::{
    packet_probe_terms, packet_terms_indicate_client_send_flow,
    packet_terms_indicate_hook_cache_flow, packet_terms_indicate_mapper_configuration_plan_flow,
    packet_terms_indicate_prepared_session_adapter_flow,
};
use codestory_contracts::api::{
    AgentCitationDto, NodeId, NodeKind, PacketBudgetModeDto, SearchHitOrigin,
};

fn domain_noun_prompt(noun: &str) -> String {
    format!("Explain how the {noun} works in this repository.")
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
        let terms = packet_probe_terms(&question);
        assert!(
            !packet_terms_indicate_client_send_flow(&terms)
                && !packet_terms_indicate_hook_cache_flow(&terms)
                && !packet_terms_indicate_mapper_configuration_plan_flow(&terms),
            "domain noun `{noun}` activated a production flow classifier"
        );
        let plan = build_packet_plan_with_extra(
            &question,
            None,
            PacketBudgetModeDto::Standard,
            &[],
        );
        let obligations = build_packet_obligation_plan(&question, plan.task_class, &plan.queries);
        // Contaminated planners mint fixed stage obligations from vocabulary
        // via flow_requirements. Generic profile guards may remain until
        // Phase 3 replaces sufficiency with repository-derived objectives.
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

    // Stronger contamination probe: noun + typical holdout verbs must still
    // not select a fixed domain stage list after decontamination.
    let contaminated_prompt =
        "Explain how the client request session adapter send path works with cache hooks";
    let contaminated_terms = packet_probe_terms(contaminated_prompt);
    assert!(
        !packet_terms_indicate_client_send_flow(&contaminated_terms)
            && !packet_terms_indicate_prepared_session_adapter_flow(&contaminated_terms)
            && !packet_terms_indicate_hook_cache_flow(&contaminated_terms),
        "domain vocabulary still selects fixed flow classifiers"
    );
}

#[test]
fn prompt_words_cannot_delete_retrieved_evidence() {
    let markdown = AgentCitationDto {
        node_id: NodeId("n-md".into()),
        display_name: "README.md".into(),
        kind: NodeKind::FILE,
        file_path: Some("README.md".into()),
        line: Some(1),
        score: 1.0,
        origin: SearchHitOrigin::TextMatch,
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
        source_excerpt: Some("# readme".into()),
    };
    let code = AgentCitationDto {
        node_id: NodeId("n-code".into()),
        display_name: "format".into(),
        kind: NodeKind::METHOD,
        file_path: Some("src/format.rs".into()),
        line: Some(10),
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
        source_excerpt: Some("fn format() {}".into()),
    };
    // Contaminated cleanup deletes markdown siblings when formatting-flow
    // vocabulary appears in the prompt, even though the user never asked to
    // drop README evidence.
    let question = "Explain the runtime formatting output path";
    let terms = packet_probe_terms(question);
    let mut rows = vec![markdown, code];
    let before = rows.len();
    packet_drop_unrequested_markdown_siblings(&mut rows, &terms);
    assert_eq!(
        rows.len(),
        before,
        "prompt vocabulary alone deleted retrieved evidence"
    );
}

#[test]
fn encoded_brand_bytes_do_not_activate_domain_flow_classifiers() {
    // Contaminated code treats [115,119,114] / "swr" as a hook-cache flow.
    // Decontaminated code must treat it like any other token.
    let encoded_terms = packet_probe_terms("how does swr cache requests?");
    let ordinary_terms = packet_probe_terms("how does xyz cache requests?");
    assert_eq!(
        packet_terms_indicate_hook_cache_flow(&encoded_terms),
        packet_terms_indicate_hook_cache_flow(&ordinary_terms),
        "encoded brand token activated a domain classifier differently from an ordinary token"
    );
    // Once classifiers are deleted, both sides are false; until then this
    // pins the equivalence requirement even if both incorrectly return true.
    let _ = (
        packet_terms_indicate_client_send_flow(&encoded_terms),
        packet_terms_indicate_mapper_configuration_plan_flow(&ordinary_terms),
        packet_terms_contain(&encoded_terms, "swr"),
    );
}

#[test]
fn bijective_rename_preserves_seed_query_objectives() {
    // Until repository_evidence_plan lands, seed plans must at least treat
    // renamed explicit anchors as first-class seeds rather than domain stages.
    let original = "Trace Foo::bar in src/foo.rs calling Baz::qux";
    let renamed = "Trace Alpha::beta in src/alpha.rs calling Gamma::delta";
    let original_plan =
        build_packet_plan_with_extra(original, None, PacketBudgetModeDto::Standard, &[]);
    let renamed_plan =
        build_packet_plan_with_extra(renamed, None, PacketBudgetModeDto::Standard, &[]);
    let original_has_path = original_plan
        .queries
        .iter()
        .any(|q| q.query.contains("src/foo.rs"));
    let renamed_has_path = renamed_plan
        .queries
        .iter()
        .any(|q| q.query.contains("src/alpha.rs"));
    assert!(original_has_path && renamed_has_path);
    // Domain stage seeds must not appear for either spelling.
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
}
