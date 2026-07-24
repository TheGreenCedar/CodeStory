use super::super::test_support::{sample_retrieval, test_search_hit_defaults};
use crate::app::rendering::{RepoTextOutputConfig, SearchOutputParts, build_search_output};
use crate::args::RepoTextMode;
use codestory_contracts::api::{NodeId, NodeKind, SearchHit, SourceOccurrenceDto};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn build_search_output_adds_stable_node_ref_when_location_is_known() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "ResolutionPass".to_string(),
        kind: codestory_contracts::api::NodeKind::STRUCT,
        file_path: Some("C:/repo/src/resolution/mod.rs".to_string()),
        line: Some(42),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "ResolutionPass",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &[],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: false,
        },
        explain: false,
    });

    assert_eq!(
        output.indexed_symbol_hits[0].node_ref.as_deref(),
        Some("src/resolution/mod.rs:42:ResolutionPass")
    );
}

#[test]
fn build_search_output_adds_occurrence_quality_and_verification_targets() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "StorageAccess".to_string(),
        kind: codestory_contracts::api::NodeKind::CLASS,
        file_path: Some("C:/repo/src/lib/StorageAccess.h".to_string()),
        line: Some(12),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: None,
        ..test_search_hit_defaults()
    }];
    let mut occurrences = HashMap::new();
    occurrences.insert(
        NodeId("1".to_string()),
        vec![
            SourceOccurrenceDto {
                element_id: "1".to_string(),
                kind: "declaration".to_string(),
                file_path: "C:/repo/src/lib/StorageAccess.h".to_string(),
                start_line: 12,
                start_col: 1,
                end_line: 12,
                end_col: 20,
            },
            SourceOccurrenceDto {
                element_id: "1".to_string(),
                kind: "definition".to_string(),
                file_path: "C:/repo/src/lib/StorageAccess.cpp".to_string(),
                start_line: 44,
                start_col: 1,
                end_line: 60,
                end_col: 1,
            },
        ],
    );

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "StorageAccess",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
        repo_text_hits: &[],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &occurrences,
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::Auto,
            enabled: false,
        },
        explain: false,
    });

    let hit = &output.indexed_symbol_hits[0];
    assert_eq!(hit.primary_occurrence_kind.as_deref(), Some("definition"));
    assert_eq!(hit.symbol_role.as_deref(), Some("definition"));
    assert!(hit.verification_targets.iter().any(|target| target.path
            == "src/lib/StorageAccess.cpp"
            && target.role == "definition"));
    assert!(
        hit.paired_refs.iter().any(|target| {
            target.path == "src/lib/StorageAccess.h" && target.role == "declaration"
        }),
        "definition hits should point back to the paired declaration"
    );
}

#[test]
fn renderer_uses_operation_bound_excerpt_after_source_mutation() {
    let temp = tempdir().expect("tempdir");
    let source = temp.path().join("src/Project.h");
    let implementation = temp.path().join("src/Project.cpp");
    fs::create_dir_all(source.parent().expect("source parent")).expect("create source parent");
    fs::write(&source, "class Project { void buildIndex(); };\n").expect("write indexed source");
    fs::write(
        &implementation,
        "#include \"Project.h\"\n\nvoid Project::buildIndex() {}\n",
    )
    .expect("write indexed implementation");
    let hit = SearchHit {
        node_id: NodeId("text-1".to_string()),
        display_name: "Project::buildIndex".to_string(),
        kind: NodeKind::FILE,
        file_path: Some(source.to_string_lossy().into_owned()),
        line: Some(1),
        score: 500.0,
        origin: codestory_contracts::api::SearchHitOrigin::TextMatch,
        match_quality: Some(codestory_contracts::api::SearchMatchQualityDto::RepoText),
        resolvable: false,
        source_excerpt: Some("class Project { void buildIndex(); };".to_string()),
        verification_targets: vec![codestory_contracts::api::SearchVerificationTargetDto {
            role: "source_text_match".to_string(),
            file_path: implementation.to_string_lossy().into_owned(),
            line: 3,
            display_name: "Project::buildIndex".to_string(),
            reason: "verified same-stem C/C++ source contains exact qualified-name text"
                .to_string(),
        }],
        ..test_search_hit_defaults()
    };

    fs::write(&source, "class Replacement {};\n").expect("mutate source after result");
    fs::write(&implementation, "void unrelated() {}\n")
        .expect("mutate implementation after result");
    let output = build_search_output(SearchOutputParts {
        project_root: temp.path(),
        query: "value",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &[],
        repo_text_hits: &[hit],
        repo_text_stats: None,
        query_assessment: None,
        search_plan: None,
        suggestions: &[],
        occurrences_by_node: &HashMap::new(),
        limit_per_source: 5,
        repo_text: RepoTextOutputConfig {
            mode: RepoTextMode::On,
            enabled: true,
        },
        explain: false,
    });

    assert_eq!(
        output.repo_text_hits[0].excerpt.as_deref(),
        Some("class Project { void buildIndex(); };")
    );
    assert_eq!(
        output.repo_text_hits[0].verification_targets[0].path,
        "src/Project.cpp"
    );
    assert_eq!(
        output.repo_text_hits[0].verification_targets[0].line, 3,
        "rendering must not relocate a pinned target from newer source bytes"
    );
}

#[test]
fn build_search_output_marks_repo_text_duplicates_of_indexed_symbols() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("symbol-1".to_string()),
        display_name: "build_snapshot_digest".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: Some("C:/repo/src/lib.rs".to_string()),
        line: Some(7),
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
        node_id: NodeId("text-1".to_string()),
        display_name: "src/lib.rs".to_string(),
        kind: codestory_contracts::api::NodeKind::FILE,
        file_path: Some("C:/repo/src/lib.rs".to_string()),
        line: Some(7),
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
        query: "snapshot digest",
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

    assert_eq!(
        output.repo_text_hits[0].duplicate_of.as_deref(),
        Some("symbol-1")
    );
}

#[test]
fn build_search_output_includes_why_when_requested() {
    let root = Path::new("C:/repo");
    let symbol_hits = vec![SearchHit {
        node_id: NodeId("1".to_string()),
        display_name: "ranked_symbol".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: Some("src/lib.rs".to_string()),
        line: Some(10),
        score: 0.9,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        source_excerpt: None,
        verification_targets: Vec::new(),
        score_breakdown: Some(codestory_contracts::api::RetrievalScoreBreakdownDto {
            lexical: 0.7,
            semantic: 0.2,
            graph: 0.1,
            total: 0.9,
            tier_cap: None,
            boosts: Vec::new(),
            dampening: Vec::new(),
            final_rank_reason: None,
            provenance: Vec::new(),
        }),
        ..test_search_hit_defaults()
    }];

    let output = build_search_output(SearchOutputParts {
        project_root: root,
        query: "ranked",
        retrieval: &sample_retrieval(),
        retrieval_shadow: None,
        freshness: None,
        symbol_hits: &symbol_hits,
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
        explain: true,
    });

    assert!(output.explain);
    assert_eq!(
        output.indexed_symbol_hits[0]
            .score_breakdown
            .as_ref()
            .map(|score| score.total),
        Some(0.9)
    );
    assert!(
        output.indexed_symbol_hits[0]
            .why
            .iter()
            .any(|why| why.contains("lexical=0.700"))
    );
}
