use super::{
    HashMap, HashSet, Path, SearchHitOrigin, SearchPlanActivePathEvidence, SearchPlanChannelDto,
    fs, graph_bridge_evidence_kind, orientation_query, same_search_file, search_plan_anchor_groups,
    search_plan_eligible, search_plan_path_is_test_or_bench, search_plan_rejected_hits,
    search_plan_runtime_call_is_speculative, search_plan_subqueries, search_plan_terms,
    search_plan_test_hit, tempdir,
};
use crate::root_rank::{CallDegrees, EntryEvidence, diversify_root_order};
use crate::search_plan::search_orientation_report;
use crate::search_terms::search_plan_query_token_closure;
use crate::symbol_query::{
    OrientationEvidence, OrientationHitEvidence, compare_search_hits_with_project_root,
};
use codestory_contracts::api::{
    EdgeId, EdgeKind, GraphEdgeDto, GraphNodeDto, GraphResponse, NodeId, NodeKind,
    SearchPlanBridgeEvidenceKindDto,
};
use codestory_contracts::api::{
    GroundingOrientationConfidenceDto, GroundingOrientationUncertaintyDto, SearchHit,
};
use codestory_contracts::graph::STRUCTURAL_COLLECTION_CANONICAL_ID_PREFIXES;

#[test]
fn broad_architecture_search_plan_terms_and_subqueries_are_bounded() {
    let query = "Explain how CodeStory's full-index path flows through CLI/runtime/workspace/indexer/store and how that supports later search, trail, and snippet commands.";
    let terms = search_plan_terms(query);
    for expected in [
        "full-index",
        "full",
        "index",
        "cli",
        "runtime",
        "workspace",
        "indexer",
        "store",
        "search",
        "trail",
        "snippet",
    ] {
        assert!(
            terms
                .extracted
                .iter()
                .any(|term| term.eq_ignore_ascii_case(expected)),
            "expected `{expected}` in extracted terms: {:?}",
            terms.extracted
        );
    }
    assert!(
        terms
            .dropped
            .iter()
            .any(|term| term.term.eq_ignore_ascii_case("explain")),
        "natural-language filler should be visible as dropped terms: {:?}",
        terms.dropped
    );
    assert!(
        orientation_query(query),
        "structure-shaped question should enter the orientation regime"
    );
    let subqueries = search_plan_subqueries(query, &terms);
    assert!(
        (3..=8).contains(&subqueries.len()),
        "subqueries should be bounded: {subqueries:#?}"
    );
    assert!(
        subqueries.iter().any(|subquery| subquery
            .channels
            .contains(&SearchPlanChannelDto::TypedSymbol)),
        "subqueries should cover typed symbol discovery: {subqueries:#?}"
    );
    assert!(
        subqueries
            .iter()
            .any(|subquery| subquery.channels.contains(&SearchPlanChannelDto::RepoText)),
        "subqueries should cover repo text discovery: {subqueries:#?}"
    );
}

#[test]
fn multi_anchor_agent_question_prioritizes_named_anchor_subquery_terms() {
    let query = "Explain how ProjectAlpha turns configuration into processing work, then how processed data is accessed by the application. Anchor the answer around ConfigGroup, WorkerRunner, and DataAccess.";
    assert!(
        orientation_query(query),
        "explain-how question should enter the orientation regime"
    );
    let terms = search_plan_terms(query);
    for expected in ["ConfigGroup", "WorkerRunner", "DataAccess"] {
        assert!(
            terms.extracted.iter().any(|term| term == expected),
            "expected named anchor `{expected}` in extracted terms: {:?}",
            terms.extracted
        );
    }

    let subqueries = search_plan_subqueries(query, &terms);
    let typed_anchor_terms = subqueries
        .iter()
        .find(|subquery| subquery.role == "typed_anchor_terms")
        .map(|subquery| subquery.query.as_str())
        .expect("typed anchor subquery");
    for expected in ["ConfigGroup", "WorkerRunner", "DataAccess"] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
            "expected named-anchor subquery for `{expected}`: {subqueries:#?}"
        );
        assert!(
            typed_anchor_terms.contains(expected),
            "typed anchor subquery should prioritize named anchors; got `{typed_anchor_terms}`"
        );
    }
}

#[test]
fn search_plan_still_runs_for_seed_anchor_drill_queries_with_exact_hits() {
    let query = "Explain how a full indexing run moves through the runtime. Seed anchors: run_index, RuntimeContext::ensure_open_from_summary, WorkspaceIndexer::run";
    assert!(
        search_plan_eligible(query, 3),
        "drill seed-anchor queries need a plan even when the anchors produce exact symbol hits"
    );

    let same_query_without_seed_anchors = "Explain how run_index RuntimeContext::ensure_open_from_summary WorkspaceIndexer::run moves through the runtime.";
    assert!(
        !search_plan_eligible(same_query_without_seed_anchors, 3),
        "ordinary exact-symbol queries should keep the exact-hit suppression"
    );
}

#[test]
fn broad_explain_how_search_plan_survives_generic_exact_hits() {
    let query = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
    // Eligibility is now the regime gate plus the exact-hit rule; a query with
    // exact hits and no seed anchors stays exact-first whatever it is about.
    assert!(
        orientation_query(query),
        "explain-how question should enter the orientation regime"
    );
    assert!(
        !search_plan_eligible(query, 7),
        "exact hits without seed anchors keep the exact-first suppression"
    );
    assert!(
        search_plan_eligible(query, 0),
        "an orientation query with no exact anchor should get a plan"
    );

    let ordinary_exact_query = "run_index RuntimeContext::ensure_open_from_summary";
    assert!(
        !orientation_query(ordinary_exact_query),
        "a bare symbol query must not enter the orientation regime"
    );
}

#[test]
fn search_plan_preserves_seed_anchor_line_exactly() {
    let query = "Explain how a full indexing run moves through the runtime. Seed anchors: run_index, run_index_once, RuntimeContext::ensure_open_from_summary, IndexService::run_indexing_blocking, AppController::run_indexing_blocking_inner, index_incremental, WorkspaceManifest::build_execution_plan, WorkspaceIndexer::run, WorkspaceIndexer::flush_projection_batch";
    let terms = search_plan_terms(query);
    let subqueries = search_plan_subqueries(query, &terms);
    for expected in [
        "run_index",
        "run_index_once",
        "RuntimeContext::ensure_open_from_summary",
        "IndexService::run_indexing_blocking",
        "AppController::run_indexing_blocking_inner",
        "index_incremental",
        "WorkspaceManifest::build_execution_plan",
        "WorkspaceIndexer::run",
        "WorkspaceIndexer::flush_projection_batch",
    ] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
            "expected exact seed-anchor subquery for `{expected}`: {subqueries:#?}"
        );
    }
}

#[test]
fn public_surface_question_keeps_short_pascal_case_named_anchor() {
    let query = "Explain how the public surfaces connect to the storage modules and the delivery pipeline. Anchor the answer around Zarq, getQuellStream, and getZarqGuard.";
    let terms = search_plan_terms(query);
    let subqueries = search_plan_subqueries(query, &terms);
    for expected in ["Zarq", "getQuellStream", "getZarqGuard"] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
            "expected named-anchor subquery for `{expected}`: {subqueries:#?}"
        );
    }
}

#[test]
fn search_plan_anchor_groups_keep_diverse_names_before_truncation() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("flow.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(&source_path, "fn placeholder() {}\n").expect("write source");
    let mut hits = (0..10)
        .map(|index| {
            search_plan_test_hit(
                &format!("cli-{index}"),
                "cli",
                &source_path,
                index + 1,
                SearchHitOrigin::IndexedSymbol,
                true,
            )
        })
        .collect::<Vec<_>>();
    hits.push(search_plan_test_hit(
        "workspace",
        "WorkspaceManifest::build_execution_plan",
        &source_path,
        20,
        SearchHitOrigin::IndexedSymbol,
        true,
    ));
    hits.push(search_plan_test_hit(
        "indexer",
        "WorkspaceIndexer::run",
        &source_path,
        21,
        SearchHitOrigin::IndexedSymbol,
        true,
    ));

    let terms = search_plan_terms(
        "Explain how the CLI runtime workspace indexer store and search flow fits together.",
    );
    let groups = search_plan_anchor_groups(
        "Explain how the CLI runtime workspace indexer store and search flow fits together.",
        &terms,
        &hits,
        &[],
        &[],
        &HashMap::new(),
        None,
    );
    let anchors = groups
        .iter()
        .map(|group| group.anchor.as_str())
        .collect::<Vec<_>>();
    assert!(
        anchors
            .iter()
            .any(|anchor| anchor.contains("WorkspaceManifest")),
        "duplicate cli anchors should not crowd out workspace anchor: {anchors:#?}"
    );
    assert!(
        anchors
            .iter()
            .any(|anchor| anchor.contains("WorkspaceIndexer")),
        "duplicate cli anchors should not crowd out indexer anchor: {anchors:#?}"
    );
}

#[test]
fn search_plan_ranks_active_callers_above_definition_only_anchors() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("feed.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(
        &source_path,
        "pub fn getQuellRecords() {}\npub fn getQuellStream() {}\n",
    )
    .expect("write source");
    let active = search_plan_test_hit(
        "active",
        "getQuellRecords",
        &source_path,
        1,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let definition_only = search_plan_test_hit(
        "definition",
        "getQuellStream",
        &source_path,
        2,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let query = "getQuellStream quell record stream";
    let terms = search_plan_terms(query);
    let active_path_evidence = HashMap::from([
        (
            active.node_id.clone(),
            SearchPlanActivePathEvidence {
                caller_count: 2,
                out_call_count: 1,
            },
        ),
        (
            definition_only.node_id.clone(),
            SearchPlanActivePathEvidence {
                caller_count: 0,
                out_call_count: 0,
            },
        ),
    ]);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[definition_only, active],
        &[],
        &[],
        &active_path_evidence,
        None,
    );

    assert_eq!(
        groups
            .first()
            .and_then(|group| group.chosen_symbol.as_ref())
            .map(|hit| hit.display_name.as_str()),
        Some("getQuellRecords"),
        "visible production callers should outrank a definition-only exact-name anchor: {groups:#?}"
    );
    assert!(
        groups.iter().any(|group| {
            group.anchor == "getQuellStream"
                && group.caller_count == 0
                && group.definition_only
                && group.no_visible_callers
                && group
                    .reasons
                    .iter()
                    .any(|reason| reason.contains("no visible production callers"))
        }),
        "definition-only callable anchors should be labeled: {groups:#?}"
    );
}

#[test]
fn search_plan_test_file_names_are_not_visible_production_callers() {
    for path in [
        "src/api.test.ts",
        "src/api.spec.ts",
        "src/api.test.tsx",
        "src/api.spec.jsx",
        "src/__tests__/api.ts",
    ] {
        assert!(
            search_plan_path_is_test_or_bench(path),
            "{path} should be treated as test code for active-path evidence"
        );
    }
}

#[test]
fn search_plan_speculation_policy_matches_hidden_trail_edges() {
    assert!(search_plan_runtime_call_is_speculative(
        Some(codestory_contracts::graph::ResolutionCertainty::Probable),
        Some(0.70)
    ));
    assert!(search_plan_runtime_call_is_speculative(None, Some(0.84)));
    assert!(!search_plan_runtime_call_is_speculative(
        Some(codestory_contracts::graph::ResolutionCertainty::Certain),
        Some(codestory_contracts::graph::ResolutionCertainty::CERTAIN_MIN)
    ));
}

#[test]
fn bridge_evidence_uses_collector_canonical_ids_not_display_labels() {
    fn graph(callsite_identity: Option<&str>, label: &str) -> GraphResponse {
        GraphResponse {
            center_id: NodeId("n1".to_string()),
            nodes: vec![GraphNodeDto {
                id: NodeId("n1".to_string()),
                label: label.to_string(),
                kind: NodeKind::FUNCTION,
                depth: 0,
                label_policy: None,
                badge_visible_members: None,
                badge_total_members: None,
                merged_symbol_examples: Vec::new(),
                file_path: Some("src/handler.ts".to_string()),
                qualified_name: None,
                member_access: None,
            }],
            edges: vec![GraphEdgeDto {
                id: EdgeId("e1".to_string()),
                source: NodeId("n1".to_string()),
                target: NodeId("n1".to_string()),
                kind: EdgeKind::CALL,
                confidence: None,
                certainty: None,
                callsite_identity: callsite_identity.map(str::to_string),
                candidate_targets: Vec::new(),
            }],
            truncated: false,
            omitted_edge_count: 0,
            canonical_layout: None,
        }
    }

    let structured = STRUCTURAL_COLLECTION_CANONICAL_ID_PREFIXES
        .first()
        .map(|prefix| format!("{prefix}orders"))
        .expect("at least one structural collection namespace");
    assert_eq!(
        graph_bridge_evidence_kind(&graph(Some(&structured), "run")),
        SearchPlanBridgeEvidenceKindDto::DataCollectionUsage,
        "a collector's canonical id is the evidence"
    );

    // The rendered label is what the deleted sniffs read. A node may say
    // anything; only the structured id written by the collector counts.
    assert_ne!(
        graph_bridge_evidence_kind(&graph(
            None,
            "payload collection orders route; confidence=0.9"
        )),
        SearchPlanBridgeEvidenceKindDto::DataCollectionUsage,
        "a display label must not stand in for collector evidence"
    );
}

#[test]
fn search_file_identity_groups_aliases_without_folding_unix_case() {
    let temp = tempdir().expect("project");
    let file = temp.path().join("routes.rs");
    fs::write(&file, "pub fn routes() {}\n").expect("write source");
    let spelled = search_plan_test_hit(
        "direct",
        "routes",
        &file,
        1,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let aliased = search_plan_test_hit(
        "aliased",
        "routes",
        &temp.path().join(".").join("routes.rs"),
        3,
        SearchHitOrigin::TextMatch,
        false,
    );
    assert!(
        same_search_file(&spelled, &aliased),
        "distinct spellings of one existing file are the same search file"
    );

    let upper = search_plan_test_hit(
        "upper",
        "routes",
        &temp.path().join("Missing.rs"),
        1,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let lower = search_plan_test_hit(
        "lower",
        "routes",
        &temp.path().join("missing.rs"),
        1,
        SearchHitOrigin::TextMatch,
        false,
    );
    assert_eq!(
        same_search_file(&upper, &lower),
        cfg!(windows),
        "missing paths keep platform lexical identity: Unix case-sensitive, Windows case-insensitive"
    );
}

fn orientation_hit(
    id: &str,
    display_name: &str,
    relative_path: &str,
    origin: SearchHitOrigin,
) -> SearchHit {
    search_plan_test_hit(id, display_name, Path::new(relative_path), 1, origin, true)
}

fn hit_evidence(
    entry: EntryEvidence,
    helper_like: bool,
    degrees: CallDegrees,
    structural_rank: u8,
    subsystem: &str,
) -> OrientationHitEvidence {
    OrientationHitEvidence {
        entry,
        helper_like,
        degrees,
        structural_rank,
        subsystem: subsystem.to_string(),
    }
}

fn order_by_orientation(
    query: &str,
    hits: &mut [SearchHit],
    evidence: Option<&OrientationEvidence>,
) -> Vec<String> {
    hits.sort_by(|left, right| {
        compare_search_hits_with_project_root(None, query, left, right, evidence)
    });
    hits.iter()
        .map(|hit| hit.display_name.clone())
        .collect::<Vec<_>>()
}

#[test]
fn orientation_query_ranks_entry_evidence_above_leaf_aliases_when_both_exist() {
    let query = "explain how the subsystems connect end to end";
    let mut hits = vec![
        orientation_hit(
            "alias",
            "zqLeafAlias",
            "src/alias.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "entry",
            "aaBootQuell",
            "src/boot.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
    ];
    let mut evidence = OrientationEvidence::default();
    evidence.insert(
        hits[0].node_id.clone(),
        hit_evidence(
            EntryEvidence::None,
            false,
            CallDegrees::default(),
            1,
            "ts:src",
        ),
    );
    evidence.insert(
        hits[1].node_id.clone(),
        hit_evidence(
            EntryEvidence::TopologicalRoot,
            false,
            CallDegrees {
                production_in_calls: 0,
                out_calls: 5,
            },
            1,
            "ts:src",
        ),
    );

    assert_eq!(
        order_by_orientation(query, &mut hits, Some(&evidence)),
        ["aaBootQuell", "zqLeafAlias"]
    );
}

#[test]
fn orientation_ranking_never_promotes_a_test_or_vendor_hit_above_production() {
    let query = "explain how the subsystems connect end to end";
    let mut hits = vec![
        orientation_hit(
            "production",
            "zzProductionRoot",
            "src/thing.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "test",
            "aaTestedRoot",
            "tests/thing.test.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
    ];
    let mut evidence = OrientationEvidence::default();
    evidence.insert(
        hits[0].node_id.clone(),
        hit_evidence(
            EntryEvidence::None,
            false,
            CallDegrees::default(),
            1,
            "ts:src",
        ),
    );
    // The test-owned hit carries far stronger graph evidence and still must not
    // climb past the production hit.
    evidence.insert(
        hits[1].node_id.clone(),
        hit_evidence(
            EntryEvidence::TopologicalRoot,
            false,
            CallDegrees {
                production_in_calls: 40,
                out_calls: 40,
            },
            0,
            "ts:tests",
        ),
    );

    assert_eq!(
        order_by_orientation(query, &mut hits, Some(&evidence)),
        ["zzProductionRoot", "aaTestedRoot"]
    );
}

#[test]
fn exact_identifier_query_ordering_is_unchanged_by_orientation_ranking() {
    let query = "zqExactAnchor";
    let build = || {
        vec![
            orientation_hit(
                "other",
                "zqOther",
                "src/other.ts",
                SearchHitOrigin::IndexedSymbol,
            ),
            orientation_hit(
                "exact",
                "zqExactAnchor",
                "src/deep/nested/exact.ts",
                SearchHitOrigin::IndexedSymbol,
            ),
        ]
    };
    let seeded = build();
    let mut evidence = OrientationEvidence::default();
    evidence.insert(
        seeded[0].node_id.clone(),
        hit_evidence(
            EntryEvidence::TopologicalRoot,
            false,
            CallDegrees {
                production_in_calls: 9,
                out_calls: 9,
            },
            1,
            "ts:src",
        ),
    );
    evidence.insert(
        seeded[1].node_id.clone(),
        hit_evidence(
            EntryEvidence::None,
            true,
            CallDegrees::default(),
            3,
            "ts:src/deep",
        ),
    );

    let mut without = build();
    let mut with_evidence = build();
    assert_eq!(
        order_by_orientation(query, &mut with_evidence, Some(&evidence)),
        order_by_orientation(query, &mut without, None),
        "exactness must stay above every orientation field"
    );
    assert_eq!(
        with_evidence.first().map(|hit| hit.display_name.as_str()),
        Some("zqExactAnchor")
    );
}

#[test]
fn ordering_reduces_to_the_lexical_comparator_when_graph_evidence_is_absent() {
    let query = "explain how the subsystems connect end to end";
    let build = || {
        vec![
            orientation_hit("one", "zqAlpha", "src/a.ts", SearchHitOrigin::IndexedSymbol),
            orientation_hit("two", "zqBeta", "src/b.ts", SearchHitOrigin::IndexedSymbol),
            orientation_hit(
                "three",
                "zqGamma",
                "src/c.ts",
                SearchHitOrigin::IndexedSymbol,
            ),
        ]
    };
    // The same window built twice: once ranked with an evidence map that holds
    // no call degrees, once ranked out of regime. The orders must agree.
    let mut evidence = OrientationEvidence::default();
    for (index, hit) in build().into_iter().enumerate() {
        evidence.insert(
            hit.node_id.clone(),
            hit_evidence(
                EntryEvidence::None,
                false,
                CallDegrees::default(),
                1,
                &format!("ts:src/{index}"),
            ),
        );
    }

    let mut edge_free = build();
    let mut lexical = build();
    assert_eq!(
        order_by_orientation(query, &mut edge_free, Some(&evidence)),
        order_by_orientation(query, &mut lexical, None)
    );

    let report = search_orientation_report(&evidence, 3, &edge_free);
    assert!(
        report
            .uncertainty
            .contains(&GroundingOrientationUncertaintyDto::GraphSignalThin)
    );
    assert!(
        report
            .uncertainty
            .contains(&GroundingOrientationUncertaintyDto::LexicalFallback)
    );
    assert_eq!(report.confidence, GroundingOrientationConfidenceDto::Weak);
}

#[test]
fn smaller_limit_results_are_an_exact_prefix_of_larger_limit_results_for_one_candidate_set() {
    let query = "explain how the subsystems connect end to end";
    let candidates = vec![
        orientation_hit("a", "zqOne", "src/a.ts", SearchHitOrigin::IndexedSymbol),
        orientation_hit("b", "zqOne", "src/b.ts", SearchHitOrigin::IndexedSymbol),
        orientation_hit("c", "zqTwo", "src/b.ts", SearchHitOrigin::IndexedSymbol),
        orientation_hit("d", "zqThree", "src/c.ts", SearchHitOrigin::IndexedSymbol),
    ];
    let mut evidence = OrientationEvidence::default();
    for (index, hit) in candidates.iter().enumerate() {
        evidence.insert(
            hit.node_id.clone(),
            hit_evidence(
                EntryEvidence::None,
                false,
                CallDegrees {
                    production_in_calls: index as u32,
                    out_calls: 0,
                },
                1,
                hit.file_path.as_deref().unwrap_or_default(),
            ),
        );
    }

    // Re-run the whole ordering pipeline per limit rather than slicing one
    // result, so a stage that consulted the limit would break the prefix.
    let run = |limit: usize| {
        let mut hits = candidates.clone();
        hits.sort_by(|left, right| {
            compare_search_hits_with_project_root(None, query, left, right, Some(&evidence))
        });
        let mut ordered = diversify_root_order(
            hits,
            |_| false,
            |hit| {
                (
                    hit.file_path.clone().unwrap_or_default(),
                    hit.display_name.clone(),
                )
            },
        );
        ordered.truncate(limit);
        ordered
            .into_iter()
            .map(|hit| hit.node_id.0)
            .collect::<Vec<_>>()
    };

    let full = run(candidates.len());
    for smaller in 0..=candidates.len() {
        assert_eq!(
            run(smaller),
            full[..smaller],
            "the order changed with the limit at {smaller}"
        );
    }
}

#[test]
fn a_candidate_the_graph_window_did_not_reach_still_ranks_on_its_own_structure() {
    let query = "explain how the subsystems connect end to end";
    let shallow = orientation_hit(
        "shallow",
        "zqAlpha",
        "src/main.ts",
        SearchHitOrigin::IndexedSymbol,
    );
    let deep = orientation_hit(
        "deep",
        "zqBeta",
        "src/deep/nested/leaf.ts",
        SearchHitOrigin::IndexedSymbol,
    );
    // Neither candidate carries call degrees: the window reached one and simply
    // did not measure the other. Path-tier evidence is free, so both still carry
    // a real structural rank, and structure decides.
    let mut evidence = OrientationEvidence::default();
    evidence.insert(
        deep.node_id.clone(),
        hit_evidence(
            EntryEvidence::None,
            false,
            CallDegrees::default(),
            2,
            "ts:src/deep",
        ),
    );
    evidence.insert(
        shallow.node_id.clone(),
        hit_evidence(
            EntryEvidence::None,
            false,
            CallDegrees::default(),
            1,
            "ts:src",
        ),
    );

    let mut hits = vec![deep, shallow];
    let order = order_by_orientation(query, &mut hits, Some(&evidence));
    assert_eq!(
        order.first().map(String::as_str),
        Some("zqAlpha"),
        "a shallower source-root candidate should outrank a deep leaf: {order:?}"
    );
}

#[test]
fn orientation_reports_the_candidates_the_graph_walk_reached_not_the_list_it_scanned() {
    let selected = vec![orientation_hit(
        "reached",
        "zqAlpha",
        "src/main.ts",
        SearchHitOrigin::IndexedSymbol,
    )];
    let mut evidence = OrientationEvidence::default();
    assert!(evidence.claim_graph_slot(1), "first slot is available");
    assert!(!evidence.claim_graph_slot(1), "the window is spent");
    evidence.insert(
        selected[0].node_id.clone(),
        hit_evidence(
            EntryEvidence::TopologicalRoot,
            false,
            CallDegrees {
                production_in_calls: 0,
                out_calls: 4,
            },
            1,
            "ts:src",
        ),
    );

    // Twelve candidates were ordered but only one carries measured graph
    // evidence; reporting twelve would overstate parser/graph coverage.
    let report = search_orientation_report(&evidence, 12, &selected);
    assert_eq!(report.evaluated_root_candidates, 1);
    assert_eq!(report.total_root_candidates, 12);
    assert!(
        report
            .uncertainty
            .contains(&GroundingOrientationUncertaintyDto::BoundedCandidateWindow),
        "an unreached candidate must be reported: {report:#?}"
    );
}

#[test]
fn subsystem_diversification_represents_distinct_production_subsystems_within_the_limit() {
    let hits = vec![
        orientation_hit(
            "a1",
            "zqAlpha",
            "src/alpha/one.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "a2",
            "zqBeta",
            "src/alpha/two.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "b1",
            "zqGamma",
            "src/beta/one.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
    ];
    let subsystem_of = |path: &str| {
        path.rsplit_once('/')
            .map(|(dir, _)| dir.to_string())
            .unwrap_or_else(|| path.to_string())
    };
    let ordered = diversify_root_order(
        hits,
        |_| false,
        |hit| {
            (
                subsystem_of(hit.file_path.as_deref().unwrap_or_default()),
                hit.display_name.clone(),
            )
        },
    );
    let first_two = ordered
        .iter()
        .take(2)
        .map(|hit| subsystem_of(hit.file_path.as_deref().unwrap_or_default()))
        .collect::<HashSet<_>>();
    assert_eq!(
        first_two.len(),
        2,
        "the first two slots should cover two subsystems: {ordered:#?}"
    );
}

#[test]
fn search_plan_subqueries_contain_only_tokens_from_the_query_closure() {
    for query in [
        "explain how the quell pipeline connects to the zarq store end to end",
        "architecture overview of the vorbex subsystem and its modules",
        "Explain how the flow works. Seed anchors: QuellRunner, zarq_store::open",
        "Explain how components connect. Anchor the answer around VorbexGate, QuellSink.",
    ] {
        let terms = search_plan_terms(query);
        let closure = search_plan_query_token_closure(query);
        for subquery in search_plan_subqueries(query, &terms) {
            if subquery.role == "original_question" || subquery.role == "named_anchor" {
                continue;
            }
            for token in subquery.query.split_whitespace() {
                assert!(
                    closure.contains(&token.to_ascii_lowercase()),
                    "subquery role `{}` injected `{token}`, which the query never supplied: {closure:?}",
                    subquery.role
                );
            }
        }
    }
}

#[test]
fn rejected_hit_reasons_report_typed_evidence_not_coverage_keys() {
    let rejected_hit = orientation_hit(
        "rejected",
        "zqRejected",
        "src/thing.ts",
        SearchHitOrigin::IndexedSymbol,
    );
    let mut evidence = OrientationEvidence::default();
    evidence.insert(
        rejected_hit.node_id.clone(),
        hit_evidence(
            EntryEvidence::LanguageMain,
            false,
            CallDegrees {
                production_in_calls: 4,
                out_calls: 1,
            },
            1,
            "ts:src",
        ),
    );

    let rejected = search_plan_rejected_hits(
        &[],
        &[],
        &[rejected_hit],
        &[],
        Some(&evidence),
        &HashSet::new(),
    );
    let reason = &rejected.first().expect("one rejected hit").reason;
    assert!(reason.contains("entry=language_main"), "{reason}");
    assert!(reason.contains("production_callers=2"), "{reason}");
    assert!(reason.contains("subsystem_represented=false"), "{reason}");
    assert!(!reason.contains("coverage_key"), "{reason}");
}

#[test]
fn duplicate_name_diversity_and_non_primary_deprioritization_are_preserved() {
    let query = "explain how the modules connect end to end";
    let mut hits = vec![
        orientation_hit(
            "vendor",
            "aaShared",
            "vendor/lib/a.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "prod-1",
            "zzShared",
            "src/one.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "prod-2",
            "zzDistinct",
            "src/two.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
    ];
    let mut evidence = OrientationEvidence::default();
    for hit in &hits {
        evidence.insert(
            hit.node_id.clone(),
            hit_evidence(
                EntryEvidence::None,
                false,
                CallDegrees::default(),
                1,
                "ts:src",
            ),
        );
    }
    let order = order_by_orientation(query, &mut hits, Some(&evidence));
    assert_eq!(
        order.last().map(String::as_str),
        Some("aaShared"),
        "vendor hits must stay demoted under orientation ranking: {order:?}"
    );

    // Three candidates share one surface and two of them share a name, so a
    // diversification that ignored names would leave the duplicate second.
    let repeated = vec![
        orientation_hit(
            "dup-1",
            "zzShared",
            "src/one.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "dup-2",
            "zzShared",
            "src/two.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
        orientation_hit(
            "distinct",
            "zzDistinct",
            "src/three.ts",
            SearchHitOrigin::IndexedSymbol,
        ),
    ];
    let diversified = diversify_root_order(
        repeated,
        |_| false,
        |hit| ("one-surface".to_string(), hit.display_name.clone()),
    );
    let names = diversified
        .iter()
        .map(|hit| hit.display_name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["zzShared", "zzDistinct", "zzShared"],
        "a novel name should precede a repeat of an already-emitted name"
    );
    assert_eq!(
        names.iter().take(2).collect::<HashSet<_>>().len(),
        2,
        "the first two slots should carry distinct names: {names:?}"
    );
}
