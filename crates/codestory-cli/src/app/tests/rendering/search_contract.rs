use super::super::test_support::{sample_retrieval, test_search_hit_defaults};
use crate::app::drill::drill_search_hit_from_packet_citation;
use crate::app::rendering::{
    RepoTextOutputConfig, SearchOutputParts, build_query_resolution_output, build_search_output,
};
use crate::args::{QuerySelectorOutput, RepoTextMode};
use crate::output::render_search_markdown;
use crate::runtime;
use codestory_contracts::api::{AgentCitationDto, NodeId, NodeKind, SearchHit};
use std::collections::HashMap;
use std::path::Path;

#[test]
fn build_search_output_preserves_separate_provenance_groups() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "indexed_symbol".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: Some("src/lib.rs".to_string()),
        line: Some(10),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];
    let repo_text_hits = vec![SearchHit {
        node_id: NodeId("repo-text".to_string()),
        display_name: "README.md".to_string(),
        kind: codestory_contracts::api::NodeKind::FILE,
        file_path: Some("README.md".to_string()),
        line: Some(3),
        score: 500.0,
        origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
        match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
        resolvable: false,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "needle",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &repo_text_hits,
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: true,
        },
        explain: false,
    });

    assert_eq!(output.repo_text_mode, RepoTextMode::Auto);
    assert!(output.repo_text_enabled);
    assert_eq!(output.indexed_symbol_hits.len(), 1);
    assert_eq!(output.repo_text_hits.len(), 1);
    assert_eq!(output.indexed_symbol_hits[0].display_name, "indexed_symbol");
    assert_eq!(output.repo_text_hits[0].display_name, "README.md");
    assert_eq!(
        output.repo_text_hits[0].origin,
        codestory_contracts::api::SearchHitOrigin::TextMatch
    );
}

#[test]
fn cli_search_and_resolution_keep_structural_evidence_metadata() {
    let root = Path::new("C:/repo");
    let manifest = SearchHit {
        node_id: NodeId("cargo-package".to_string()),
        display_name: "demo".to_string(),
        kind: NodeKind::PACKAGE,
        file_path: Some("C:/repo/Cargo.toml".to_string()),
        line: Some(2),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        resolvable: true,
        evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText),
        evidence_producer: Some("structural_cargo_manifest_collector".to_string()),
        resolution_status: Some(
            codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
        ),
        eligible_for_sufficiency: Some(false),
        ..test_search_hit_defaults()
    };
    let workflow = SearchHit {
        node_id: NodeId("workflow-job".to_string()),
        display_name: "test".to_string(),
        kind: NodeKind::FUNCTION,
        file_path: Some("C:/repo/.github/workflows/ci.yml".to_string()),
        line: Some(12),
        score: 0.8,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        resolvable: true,
        evidence_tier: Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText),
        evidence_producer: Some("structural_github_actions_workflow_collector".to_string()),
        resolution_status: Some(
            codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly,
        ),
        eligible_for_sufficiency: Some(false),
        ..test_search_hit_defaults()
    };
    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "demo",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &[manifest.clone(), workflow.clone()],
        repo_text_hits: &[],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Off,
            enabled: false,
        },
        explain: false,
    });

    let search_json = serde_json::to_value(&output).expect("serialize CLI search output");
    for (index, producer) in [
        (0, "structural_cargo_manifest_collector"),
        (1, "structural_github_actions_workflow_collector"),
    ] {
        let hit = &search_json["indexed_symbol_hits"][index];
        assert_eq!(hit["evidence_tier"], "structural_text");
        assert_eq!(hit["evidence_producer"], producer);
        assert_eq!(hit["resolution_status"], "source_range_only");
        assert_eq!(hit["eligible_for_sufficiency"], false);
    }
    let markdown = render_search_markdown(root, &output);
    assert!(
        markdown.contains("evidence_tier=structural_text"),
        "{markdown}"
    );
    assert!(
        markdown.contains("resolution_status=source_range_only"),
        "{markdown}"
    );
    assert!(
        markdown.contains("eligible_for_sufficiency=false"),
        "{markdown}"
    );

    let target = runtime::ResolvedTarget {
        selector: QuerySelectorOutput::Query,
        requested: "demo".to_string(),
        file_filter: None,
        selected: manifest.clone(),
        alternatives: vec![manifest, workflow.clone()],
    };
    let resolution = build_query_resolution_output(root, &target);
    let resolution_json =
        serde_json::to_value(&resolution).expect("serialize CLI query resolution output");
    assert_eq!(
        resolution_json["resolved"]["evidence_tier"],
        "structural_text"
    );
    assert_eq!(
        resolution_json["resolved"]["resolution_status"],
        "source_range_only"
    );
    assert_eq!(
        resolution_json["resolved"]["eligible_for_sufficiency"],
        false
    );
    assert_eq!(
        resolution_json["alternatives"][0]["evidence_producer"],
        "structural_github_actions_workflow_collector"
    );

    let citation = AgentCitationDto {
        node_id: workflow.node_id.clone(),
        display_name: workflow.display_name.clone(),
        kind: workflow.kind,
        file_path: workflow.file_path.clone(),
        line: workflow.line,
        score: workflow.score,
        origin: workflow.origin,
        resolvable: workflow.resolvable,
        subgraph_id: None,
        evidence_edge_ids: Vec::new(),
        retrieval_score_breakdown: None,
        evidence_tier: workflow.evidence_tier,
        evidence_producer: workflow.evidence_producer.clone(),
        resolution_status: workflow.resolution_status,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: workflow.eligible_for_sufficiency,
    };
    let drill_hit = drill_search_hit_from_packet_citation(root, "test", &citation);
    assert_eq!(
        drill_hit.evidence_producer.as_deref(),
        Some("structural_github_actions_workflow_collector")
    );
    assert_eq!(
        drill_hit.resolution_status,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
    );
    assert_eq!(drill_hit.eligible_for_sufficiency, Some(false));
}

#[test]
fn build_search_output_marks_repo_text_why_as_diagnostic_navigation() {
    let root = Path::new("C:/repo");
    let repo_text_hits = vec![SearchHit {
        node_id: NodeId("repo-text".to_string()),
        display_name: "README.md".to_string(),
        kind: codestory_contracts::api::NodeKind::FILE,
        file_path: Some("README.md".to_string()),
        line: Some(3),
        score: 500.0,
        origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
        match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
        resolvable: false,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "needle",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &[],
        repo_text_hits: &repo_text_hits,
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: true,
        },
        explain: true,
    });

    let why = output.repo_text_hits[0].why.join("\n");
    assert!(
        why.contains("repo-text diagnostic match"),
        "repo-text why should be a diagnostic/navigation hint: {why}"
    );
    assert!(
        !why.contains("this hit is evidence"),
        "repo-text why must not present text as evidence: {why}"
    );
}
