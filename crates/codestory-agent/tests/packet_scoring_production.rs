use codestory_agent::packet_scoring::packet_citation_rank;
use codestory_contracts::api::{AgentCitationDto, NodeId, NodeKind, SearchHitOrigin};

fn citation(display_name: &str, file_path: &str) -> AgentCitationDto {
    AgentCitationDto {
        node_id: NodeId(display_name.to_string()),
        display_name: display_name.to_string(),
        kind: NodeKind::METHOD,
        file_path: Some(file_path.to_string()),
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
fn python_sources_are_not_globally_demoted_without_a_language_term() {
    let terms = vec!["request".to_string(), "flow".to_string()];
    let python = citation("handle_request", "src/routes/request.py");
    let native = citation("handle_request", "src/routes/request.rs");

    assert_eq!(
        packet_citation_rank(&python, &terms, false),
        packet_citation_rank(&native, &terms, false)
    );
}

#[test]
fn collections_path_alone_has_no_rank_authority() {
    let terms = vec!["payload".to_string()];
    let collection = citation("record", "src/collections/record.ts");
    let source = citation("record", "src/content/record.ts");

    assert_eq!(
        packet_citation_rank(&collection, &terms, false),
        packet_citation_rank(&source, &terms, false)
    );
}
