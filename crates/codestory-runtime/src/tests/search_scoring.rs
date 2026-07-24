use super::{
    AgentHybridWeightsDto, AppController, CancellationToken, CoreNodeId, EnvGuard, FileExt,
    GroundingBudgetDto, HYBRID_RETRIEVAL_ENABLED_ENV, HashMap, HybridSearchConfig,
    IndexFreshnessStatusDto, IndexMode, Node, NodeId, NodeKind, Occurrence, OccurrenceKind,
    OpenProjectRequest, Path, PathBuf, PublicationTestAction, PublicationTestBoundary,
    RetrievalModeDto, SEMANTIC_DOC_ALIAS_MODE_ENV, SEMANTIC_DOC_MAX_TOKENS_ENV, SearchEngine,
    SearchGenerationCatalogGuard, SearchGenerationCompletion, SearchHit, SearchRepoTextMode,
    SearchRequest, SearchSymbolProjection, SemanticProjectionStats, SnapshotStore, SourceLocation,
    Storage, apply_hybrid_limits, arm_publication_test_fault,
    assert_mandatory_retrieval_unavailable, assert_no_staged_publication_artifacts,
    build_persisted_search_state_from_canonical_symbols, build_search_state, compare_search_hits,
    copy_tictactoe_workspace, current_epoch_ms, dedupe_inexact_search_hits_by_display_key,
    default_source_policy_identity, finalize_staged_semantic_docs,
    flush_pending_dense_anchor_inputs, fs, hybrid_search_config_for_request, hybrid_test_env,
    insert_semantic_fixture_nodes, llm_symbol_doc_hash, load_persisted_search_state,
    merge_search_hits_by_node_id, normalized_hybrid_weights, pending_semantic_doc_for_test,
    persisted_search_generation_names, primary_source_retention_threshold, process_env_test_lock,
    project_identity_v3, prune_search_generations, rebuild_search_state_from_storage,
    search_generation_completion_path, search_index_generation_root,
    search_index_path_for_publication, search_index_storage_path, semantic_doc_text_for_test,
    semantic_projection_republish_for_runtime, tempdir, test_index_publication,
    test_retrieval_manifest, test_sidecar_runtime_from_env, unbounded,
    write_search_generation_completion, write_semantic_fixture,
};

#[test]
fn semantic_doc_text_alias_modes_are_switchable_for_research() {
    let _lock = process_env_test_lock();
    let _budget = EnvGuard::set(SEMANTIC_DOC_MAX_TOKENS_ENV, "512");
    let no_alias = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "no_alias");
    let no_alias_doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );
    let no_alias_hash = llm_symbol_doc_hash(&no_alias_doc);
    assert!(!no_alias_doc.contains("terminal_alias:"));
    assert!(!no_alias_doc.contains("path_aliases:"));
    drop(no_alias);

    let variant = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "alias_variant");
    let variant_doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );
    let variant_hash = llm_symbol_doc_hash(&variant_doc);
    assert!(variant_doc.contains("terminal_alias: open project with storage path"));
    assert!(variant_doc.contains("owner_aliases: AppController, app controller"));
    assert!(variant_doc.contains("symbol_role: method member function"));
    assert!(!variant_doc.contains("name_aliases:"));
    assert!(!variant_doc.contains("path_aliases:"));
    assert_ne!(no_alias_hash, variant_hash);
    drop(variant);

    let current = EnvGuard::set(SEMANTIC_DOC_ALIAS_MODE_ENV, "current_alias");
    let current_doc = semantic_doc_text_for_test(
        "AppController::openProjectWithStoragePath",
        Some("codestory_runtime::AppController::openProjectWithStoragePath"),
        "crates/codestory-runtime/src/lib.rs",
        NodeKind::METHOD,
    );
    assert!(current_doc.contains("name_aliases:"));
    assert!(current_doc.contains("path_aliases:"));
    assert_ne!(variant_hash, llm_symbol_doc_hash(&current_doc));
    drop(current);
}

#[test]
fn build_search_hit_prefers_declaration_coordinates_and_filters_unknown_nodes() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(10),
                kind: NodeKind::FILE,
                serialized_name: "src/lib.rs".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(11),
                kind: NodeKind::FUNCTION,
                serialized_name: "check_winner".to_string(),
                file_node_id: Some(CoreNodeId(10)),
                start_line: Some(42),
                start_col: Some(5),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(12),
                kind: NodeKind::UNKNOWN,
                serialized_name: "check_winner".to_string(),
                file_node_id: Some(CoreNodeId(10)),
                start_line: Some(99),
                ..Default::default()
            },
        ])
        .expect("insert nodes");
    storage
        .insert_occurrences_batch(&[Occurrence {
            element_id: 11,
            kind: OccurrenceKind::REFERENCE,
            location: SourceLocation {
                file_node_id: CoreNodeId(10),
                start_line: 87,
                start_col: 9,
                end_line: 87,
                end_col: 20,
            },
        }])
        .expect("insert occurrences");

    let node_names = HashMap::from([
        (CoreNodeId(11), "check_winner".to_string()),
        (CoreNodeId(12), "check_winner".to_string()),
    ]);

    let definition_hit =
        AppController::build_search_hit(&storage, &node_names, CoreNodeId(11), 1.0)
            .expect("provenance lookup")
            .expect("definition hit");
    assert_eq!(definition_hit.file_path.as_deref(), Some("src/lib.rs"));
    assert_eq!(definition_hit.line, Some(42));

    assert!(
        AppController::build_search_hit(&storage, &node_names, CoreNodeId(12), 1.0)
            .expect("provenance lookup")
            .is_none(),
        "unknown placeholder nodes should be dropped from indexed results"
    );
}

#[test]
fn build_search_hit_fails_closed_when_structural_provenance_cannot_be_read() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[Node {
            id: CoreNodeId(13),
            kind: NodeKind::FUNCTION,
            serialized_name: "handler".to_string(),
            ..Default::default()
        }])
        .expect("insert node");
    storage
        .get_connection()
        .execute("DROP TABLE structural_text_unit", [])
        .expect("inject structural provenance read failure");

    let error = AppController::build_search_hit(
        &storage,
        &HashMap::from([(CoreNodeId(13), "handler".to_string())]),
        CoreNodeId(13),
        1.0,
    )
    .expect_err("provenance storage failures must abort indexed-symbol search");
    assert!(
        error
            .message
            .contains("Failed to load structural provenance for node 13")
    );
}

#[test]
fn build_search_hit_adjusts_route_scores_by_extraction_provenance() {
    fn route_canonical_id(extraction: &str) -> String {
        format!(
            "route_endpoint:{}",
            serde_json::json!({
                "kind": "framework_route",
                "framework": "express",
                "method": "GET",
                "path": "/api/users",
                "provenance": [format!("extraction:{extraction}")],
            })
        )
    }

    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(20),
                kind: NodeKind::FILE,
                serialized_name: "src/routes.ts".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(22),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("ast_indexed")),
                start_line: Some(3),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(23),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("text_only")),
                start_line: Some(3),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(24),
                kind: NodeKind::FUNCTION,
                serialized_name: "plain_handler".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                start_line: Some(8),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(25),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("tree_sitter_query")),
                start_line: Some(4),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(26),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(20)),
                canonical_id: Some(route_canonical_id("lexical_fallback")),
                start_line: Some(5),
                ..Default::default()
            },
        ])
        .expect("insert route nodes");
    let node_names = HashMap::from([
        (CoreNodeId(22), "GET /api/users".to_string()),
        (CoreNodeId(23), "GET /api/users".to_string()),
        (CoreNodeId(24), "plain_handler".to_string()),
        (CoreNodeId(25), "GET /api/users".to_string()),
        (CoreNodeId(26), "GET /api/users".to_string()),
    ]);

    let ast = AppController::build_search_hit(&storage, &node_names, CoreNodeId(22), 1.0)
        .expect("provenance lookup")
        .expect("ast route hit");
    let text_only = AppController::build_search_hit(&storage, &node_names, CoreNodeId(23), 1.0)
        .expect("provenance lookup")
        .expect("text-only route hit");
    let normal = AppController::build_search_hit(&storage, &node_names, CoreNodeId(24), 1.0)
        .expect("provenance lookup")
        .expect("normal hit");
    let tree_sitter = AppController::build_search_hit(&storage, &node_names, CoreNodeId(25), 1.0)
        .expect("provenance lookup")
        .expect("tree-sitter route hit");
    let lexical_fallback =
        AppController::build_search_hit(&storage, &node_names, CoreNodeId(26), 1.0)
            .expect("provenance lookup")
            .expect("lexical fallback route hit");

    assert!(
        ast.score > text_only.score,
        "AST-indexed route evidence should outrank otherwise equivalent text-only route guesses"
    );
    assert!(ast.score > normal.score);
    assert!(text_only.score < normal.score);
    assert_eq!(tree_sitter.score, ast.score);
    assert_eq!(lexical_fallback.score, text_only.score);
    assert_eq!(normal.score, 1.0);
    assert_eq!(
        normal.evidence_tier,
        Some(codestory_contracts::api::PacketEvidenceTierDto::ResolvedGraph),
        "a valid missing unit remains resolved graph evidence"
    );

    let mut hits = [text_only, ast.clone()];
    hits.sort_by(|left, right| compare_search_hits("/api/users", left, right));
    assert_eq!(hits.first().map(|hit| &hit.node_id), Some(&ast.node_id));
}

#[test]
fn build_search_hit_marks_openapi_endpoints_as_diagnostic_source() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(30),
                kind: NodeKind::FILE,
                serialized_name: "openapi.json".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(31),
                kind: NodeKind::FUNCTION,
                serialized_name: "GET /api/users".to_string(),
                file_node_id: Some(CoreNodeId(30)),
                canonical_id: Some("openapi:endpoint:GET /api/users".to_string()),
                start_line: Some(7),
                ..Default::default()
            },
        ])
        .expect("insert OpenAPI nodes");
    let node_names = HashMap::from([(CoreNodeId(31), "GET /api/users".to_string())]);

    let hit = AppController::build_search_hit(&storage, &node_names, CoreNodeId(31), 1.0)
        .expect("provenance lookup")
        .expect("OpenAPI endpoint hit");

    assert_eq!(
        hit.evidence_tier,
        Some(codestory_contracts::api::PacketEvidenceTierDto::ExactSource)
    );
    assert_eq!(
        hit.resolution_status,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
    );
    assert_eq!(
        hit.evidence_producer.as_deref(),
        Some("openapi_endpoint_schema")
    );
    assert_eq!(hit.eligible_for_sufficiency, Some(false));
}

#[test]
fn build_search_hit_marks_generic_structural_collectors_as_non_sufficient() {
    let mut storage = Storage::new_in_memory().expect("storage");
    let source_hash = "a".repeat(64);
    let unit = codestory_store::StructuralTextUnit {
        node_id: CoreNodeId(41),
        file_id: 40,
        placement_id: "b".repeat(64),
        content_hash: "c".repeat(64),
        source_content_hash: source_hash.clone(),
        descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
        producer: "structural_markdown_collector".to_string(),
        evidence_tier: "structural_text".to_string(),
        resolution: "source_range_only".to_string(),
        language: "markdown".to_string(),
        kind: NodeKind::MODULE,
        start_line: 2,
        start_col: 1,
        end_line: 2,
        end_col: 4,
        file_role: codestory_store::FileRole::Source,
    };
    storage
        .projections()
        .flush_projection_batch(codestory_store::ProjectionBatch {
            files: &[codestory_store::FileInfo {
                id: 40,
                path: PathBuf::from("docs/demo.md"),
                language: "markdown".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 2,
                file_role: codestory_store::FileRole::Source,
            }],
            file_content_hashes: &[codestory_store::FileContentHash {
                file_id: 40,
                content_hash: source_hash.clone(),
            }],
            nodes: &[
                Node {
                    id: CoreNodeId(40),
                    kind: NodeKind::FILE,
                    serialized_name: "docs/demo.md".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(41),
                    kind: NodeKind::MODULE,
                    serialized_name: "demo".to_string(),
                    file_node_id: Some(CoreNodeId(40)),
                    start_line: Some(2),
                    start_col: Some(1),
                    end_line: Some(2),
                    end_col: Some(4),
                    ..Default::default()
                },
            ],
            structural_text_units: std::slice::from_ref(&unit),
            structural_text_projections: &[codestory_store::StructuralTextProjection {
                file_id: 40,
                source_content_hash: source_hash,
                descriptor_version: codestory_store::STRUCTURAL_TEXT_UNIT_DESCRIPTOR_VERSION,
                producer: "structural_markdown_collector".to_string(),
                language: "markdown".to_string(),
                file_role: codestory_store::FileRole::Source,
                unit_count: 1,
                unit_digest: codestory_store::structural_text_unit_digest(std::slice::from_ref(
                    &unit,
                )),
            }],
            structural_text_cache_writes: &[],
            edges: &[],
            occurrences: &[],
            component_access: &[],
            callable_projection_states: &[],
            file_errors: &[],
        })
        .expect("insert verified structural projection");
    let node_names = HashMap::from([(CoreNodeId(41), "demo".to_string())]);

    let hit = AppController::build_search_hit(&storage, &node_names, CoreNodeId(41), 1.0)
        .expect("provenance lookup")
        .expect("structural hit");

    assert_eq!(
        hit.evidence_tier,
        Some(codestory_contracts::api::PacketEvidenceTierDto::StructuralText)
    );
    assert_eq!(
        hit.evidence_producer.as_deref(),
        Some("structural_markdown_collector")
    );
    assert_eq!(
        hit.resolution_status,
        Some(codestory_contracts::api::PacketEvidenceResolutionDto::SourceRangeOnly)
    );
    assert_eq!(hit.eligible_for_sufficiency, Some(false));
}

#[test]
fn build_search_state_ignores_stale_legacy_projection_rows() {
    let mut storage = Storage::new_in_memory().expect("storage");
    storage
        .insert_nodes_batch(&[
            Node {
                id: CoreNodeId(900),
                kind: NodeKind::FILE,
                serialized_name: "src/changed.rs".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(901),
                kind: NodeKind::FUNCTION,
                serialized_name: "old_name".to_string(),
                qualified_name: Some("pkg::old_name".to_string()),
                file_node_id: Some(CoreNodeId(900)),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(910),
                kind: NodeKind::FILE,
                serialized_name: "src/untouched.rs".to_string(),
                ..Default::default()
            },
            Node {
                id: CoreNodeId(911),
                kind: NodeKind::FUNCTION,
                serialized_name: "untouched".to_string(),
                qualified_name: Some("pkg::untouched".to_string()),
                file_node_id: Some(CoreNodeId(910)),
                ..Default::default()
            },
        ])
        .expect("insert nodes");
    storage
        .insert_nodes_batch(&[Node {
            id: CoreNodeId(901),
            kind: NodeKind::FUNCTION,
            serialized_name: "renamed".to_string(),
            qualified_name: Some("pkg::renamed".to_string()),
            file_node_id: Some(CoreNodeId(900)),
            ..Default::default()
        }])
        .expect("update changed node");
    storage
        .upsert_search_symbol_projection_batch(&[SearchSymbolProjection {
            node_id: CoreNodeId(911),
            display_name: "stale_other_file".to_string(),
        }])
        .expect("seed untouched stale projection");

    let nodes = storage.get_nodes().expect("nodes");
    let result = build_search_state(None, nodes).expect("build search state from canonical nodes");
    assert_eq!(
        result.node_names.get(&CoreNodeId(901)).map(String::as_str),
        Some("pkg::renamed")
    );
    assert_eq!(
        result.node_names.get(&CoreNodeId(911)).map(String::as_str),
        Some("pkg::untouched")
    );

    let projection = storage
        .get_search_symbol_projection_batch_after(None, 10)
        .expect("projection");
    let names_by_id: HashMap<_, _> = projection
        .into_iter()
        .map(|entry| (entry.node_id, entry.display_name))
        .collect();
    assert_eq!(
        names_by_id.get(&CoreNodeId(911)).map(String::as_str),
        Some("stale_other_file")
    );
}

#[test]
fn persisted_search_build_streams_multiple_pages_through_one_writer() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("search stream tempdir");
    let storage_path = temp.path().join("codestory.db");
    let search_path = temp.path().join("search-generation");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    let nodes = (1..=4_100_i64)
        .map(|id| Node {
            id: CoreNodeId(id),
            kind: NodeKind::FUNCTION,
            serialized_name: format!("symbol_{id}"),
            qualified_name: (id % 2 == 0).then(|| format!("pkg::symbol_{id}")),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    storage
        .insert_nodes_batch(&nodes)
        .expect("insert streamed search nodes");

    let cancelled_search_path = temp.path().join("cancelled-search-generation");
    let cancel_token = CancellationToken::new();
    arm_publication_test_fault(
        PublicationTestBoundary::SearchSymbolPage,
        PublicationTestAction::Cancel,
    );
    let cancelled = match build_persisted_search_state_from_canonical_symbols(
        &mut storage,
        &cancelled_search_path,
        false,
        &test_sidecar_runtime_from_env(),
        Some(&cancel_token),
    ) {
        Err(error) => error,
        Ok(_) => panic!("cancel after the first non-final page"),
    };
    assert_eq!(cancelled.code, "cancelled");
    assert!(cancel_token.is_cancelled());
    assert!(
        !search_generation_completion_path(&cancelled_search_path).exists(),
        "cancelled page stream must not publish a completion marker"
    );
    let cancelled_engine = SearchEngine::open_existing(&cancelled_search_path)
        .expect("open uncommitted cancelled generation");
    assert_eq!(
        cancelled_engine.tantivy_doc_count(),
        0,
        "cancelled non-final page must not commit Tantivy documents"
    );
    drop(cancelled_engine);

    let result = build_persisted_search_state_from_canonical_symbols(
        &mut storage,
        &search_path,
        false,
        &test_sidecar_runtime_from_env(),
        None,
    )
    .expect("build persisted search from canonical stream");

    assert_eq!(result.search_stats.search_projection_rebuild_ms, 0);
    assert_eq!(result.search_stats.search_symbol_stream_rows, 4_100);
    assert_eq!(result.search_stats.search_symbol_stream_batches, 2);
    assert_eq!(result.search_stats.search_symbol_index_docs_written, 4_100);
    assert_eq!(result.search_stats.search_symbol_index_writer_count, 1);
    assert_eq!(result.search_stats.search_symbol_index_commit_count, 1);
    assert_eq!(result.search_stats.search_symbol_index_reload_count, 1);
    assert_eq!(result.node_names.len(), 4_100);
    assert_eq!(result.engine.full_text_doc_count(), 4_100);
    assert_eq!(
        storage
            .get_search_symbol_projection_count()
            .expect("count legacy projection"),
        0
    );
}

#[test]
fn search_requires_full_sidecars_for_exact_type_queries() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(10),
                    kind: NodeKind::FILE,
                    serialized_name: temp
                        .path()
                        .join("src")
                        .join("lib.rs")
                        .to_string_lossy()
                        .to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::STRUCT,
                    serialized_name: "AppController".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(10),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(12),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "AppController::open_project".to_string(),
                    qualified_name: Some("AppController::open_project".to_string()),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(20),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(13),
                    kind: NodeKind::UNKNOWN,
                    serialized_name: "AppController".to_string(),
                    file_node_id: Some(CoreNodeId(10)),
                    start_line: Some(30),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), db_path)
        .expect("open project");

    let error = controller
        .search(SearchRequest {
            query: "AppController".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn compare_search_hits_prefers_function_over_method_for_equal_symbol_matches() {
    let function = SearchHit {
        node_id: NodeId("function".to_string()),
        display_name: "ArtificialPlayer::min_max".to_string(),
        kind: codestory_contracts::api::NodeKind::FUNCTION,
        file_path: None,
        line: None,
        score: 184.0,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: None,
    };
    let method = SearchHit {
        node_id: NodeId("method".to_string()),
        display_name: "ArtificialPlayer::min_max".to_string(),
        kind: codestory_contracts::api::NodeKind::METHOD,
        file_path: None,
        line: None,
        score: 184.0,
        origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
        match_quality: None,
        resolvable: true,
        evidence_tier: None,
        evidence_producer: None,
        resolution_status: None,
        loss_reason: None,
        coverage_role: None,
        eligible_for_sufficiency: None,
        score_breakdown: None,
    };

    let mut hits = [method, function.clone()];
    hits.sort_by(|left, right| compare_search_hits("min_max", left, right));

    assert_eq!(hits.first().map(|hit| hit.kind), Some(function.kind));
}

#[test]
fn search_prefers_full_sidecars_for_tictactoe_queries() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "false");
    let workspace = copy_tictactoe_workspace();
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    controller
        .open_project_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open workspace");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("index fixtures");

    for query in ["check_winner", "min_max"] {
        let error = controller
            .search(SearchRequest {
                query: query.to_string(),
                repo_text: SearchRepoTextMode::Off,
                limit_per_source: 10,
                expand_search_plan: false,
                hybrid_weights: None,
                hybrid_limits: None,
            })
            .expect_err("search fixtures should require full sidecars");
        assert_mandatory_retrieval_unavailable(&error);
    }
}

#[test]
fn repo_explanation_search_requires_full_sidecar_retrieval() {
    let _lock = process_env_test_lock();
    let _env = EnvGuard::set(HYBRID_RETRIEVAL_ENABLED_ENV, "false");
    let workspace = copy_tictactoe_workspace();
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    controller
        .open_project_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open workspace");
    controller
        .run_indexing_blocking(IndexMode::Full)
        .expect("index fixtures");

    let generic_error = controller
        .search_results(SearchRequest {
            query: "Explain how this repo fits together".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("generic repo explanation search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&generic_error);

    let symbol_error = controller
        .search_results(SearchRequest {
            query: "Explain how check_winner fits in this repo".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: true,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("symbol-like repo explanation search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&symbol_error);
}

#[test]
fn search_rejects_natural_language_queries_without_full_sidecars() {
    let temp = tempdir().expect("create temp dir");
    let db_path = temp.path().join("codestory.db");

    {
        let mut storage = Storage::open(&db_path).expect("open storage");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(201),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "language_parsing_pipeline".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(202),
                    kind: NodeKind::MODULE,
                    serialized_name: "parser_core".to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(203),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "runtime_workspace_indexer_store_flow".to_string(),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .expect("open project");

    let broad_query =
        "Explain how the full-index path flows through runtime workspace indexer and store";
    let error_without_plan = controller
        .search_results(SearchRequest {
            query: broad_query.to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 20,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("natural language search should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error_without_plan);

    let error_with_plan = controller
        .search_results(SearchRequest {
            query: broad_query.to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 20,
            expand_search_plan: true,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("natural language search plan should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error_with_plan);
}

#[test]
fn build_search_state_prefers_qualified_name() {
    let nodes = vec![Node {
        id: CoreNodeId(1),
        kind: NodeKind::FUNCTION,
        serialized_name: "short_name".to_string(),
        qualified_name: Some("pkg.mod.short_name".to_string()),
        ..Default::default()
    }];

    let result = build_search_state(None, nodes).expect("build search state");
    let node_names = result.node_names;
    let engine = result.engine;
    assert_eq!(
        node_names.get(&CoreNodeId(1)).map(String::as_str),
        Some("pkg.mod.short_name")
    );

    let hits = engine.search_symbol("pkg.mod");
    assert_eq!(hits.first().copied(), Some(CoreNodeId(1)));
}

#[test]
fn open_project_summary_clears_search_state() {
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
        .expect("open project with search state");
    assert!(
        controller.state.lock().search_engine.is_some(),
        "expected full open to initialize search state"
    );

    controller
        .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("open project summary");
    let state = controller.state.lock();
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

#[test]
fn run_indexing_without_runtime_refresh_keeps_search_uninitialized() {
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let state = controller.state.lock();
    assert!(!state.is_indexing);
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

#[test]
fn semantic_projection_republish_fail_and_cancel_matrix_preserves_complete_core_and_search() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let runtime = test_sidecar_runtime_from_env();
    let controller = AppController::new_with_config(runtime.clone());
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("publish complete core");
    let identity = project_identity_v3(workspace.path());
    let (publication, retrieval_publication) = {
        let mut storage = Storage::open(&storage_path).expect("open baseline core");
        let publication = storage
            .get_complete_index_publication()
            .expect("read baseline publication")
            .expect("complete baseline publication");
        let symbol_doc_count = storage
            .get_symbol_search_doc_count()
            .expect("count baseline symbol documents");
        let dense_count = storage
            .validate_dense_anchor_publication(&publication)
            .expect("validate baseline dense publication")
            .anchor_count;
        storage
            .upsert_retrieval_index_manifest(&test_retrieval_manifest(
                &identity.project_id,
                symbol_doc_count as i64,
                dense_count as i64,
            ))
            .expect("publish baseline retrieval identity");
        let retrieval = storage
            .get_retrieval_index_publication(&identity.project_id)
            .expect("read baseline retrieval identity")
            .expect("baseline retrieval publication");
        (publication, retrieval)
    };
    let search_generations = persisted_search_generation_names(&storage_path);
    for boundary in [
        PublicationTestBoundary::SemanticContextIndexes,
        PublicationTestBoundary::SemanticNodePage,
        PublicationTestBoundary::SemanticStoredDocumentPage,
        PublicationTestBoundary::SemanticEndpointRead,
        PublicationTestBoundary::ProjectionSnapshotFinalize,
        PublicationTestBoundary::ProjectionSnapshotDetail,
        PublicationTestBoundary::ProjectionManifestIdentity,
        PublicationTestBoundary::SearchBuild,
        PublicationTestBoundary::SearchSymbolPage,
        PublicationTestBoundary::SearchIndexWrite,
        PublicationTestBoundary::SearchValidation,
        PublicationTestBoundary::SearchCompletion,
        PublicationTestBoundary::CatalogLock,
        PublicationTestBoundary::MarkerCompletion,
        PublicationTestBoundary::DatabaseReplacement,
    ] {
        for action in [PublicationTestAction::Fail, PublicationTestAction::Cancel] {
            let cancel = CancellationToken::new();
            arm_publication_test_fault(boundary, action);
            let error = match semantic_projection_republish_for_runtime(
                workspace.path(),
                &storage_path,
                Some(&cancel),
                &runtime,
                controller.source_index_policy.as_ref(),
            ) {
                Err(error) => error,
                Ok(_) => panic!("faulted projection republish must not publish"),
            };
            assert_eq!(
                error.code,
                if action == PublicationTestAction::Cancel {
                    "cancelled"
                } else {
                    "internal"
                },
                "boundary={boundary:?} action={action:?}: {error:?}"
            );
            assert_eq!(
                cancel.is_cancelled(),
                action == PublicationTestAction::Cancel
            );
            assert_eq!(
                Storage::database_complete_index_publication(&storage_path)
                    .expect("read preserved publication"),
                Some(publication.clone()),
                "boundary={boundary:?} action={action:?}"
            );
            assert_eq!(
                persisted_search_generation_names(&storage_path),
                search_generations,
                "boundary={boundary:?} action={action:?}"
            );
            assert_eq!(
                Storage::open(&storage_path)
                    .expect("open preserved retrieval state")
                    .get_retrieval_index_publication(&identity.project_id)
                    .expect("read preserved retrieval state"),
                Some(retrieval_publication.clone()),
                "boundary={boundary:?} action={action:?}"
            );
            assert_no_staged_publication_artifacts(&storage_path);
        }
    }
}

#[test]
fn grounding_snapshot_from_summary_open_keeps_search_state_cold() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    {
        let state = controller.state.lock();
        assert!(
            state.search_engine.is_none(),
            "summary open plus indexing should leave search state unloaded"
        );
        assert!(
            state.node_names.is_empty(),
            "summary open plus indexing should leave node label cache empty"
        );
    }

    let snapshot = controller
        .grounding_snapshot(GroundingBudgetDto::Balanced)
        .expect("grounding snapshot");
    let retrieval = snapshot.retrieval.expect("retrieval state");
    assert_eq!(retrieval.mode, RetrievalModeDto::Symbolic);
    assert!(!retrieval.semantic_ready);
    assert_eq!(retrieval.semantic_doc_count, 0);

    let storage = Storage::open(&storage_path).expect("open indexed storage");
    assert!(
        !storage
            .get_dense_anchor_inputs_batch_after(None, 10_000)
            .expect("dense anchor inputs")
            .is_empty(),
        "core indexing should publish embedding-free dense anchor inputs"
    );
    assert!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy semantic rows")
            .is_empty(),
        "core indexing should not publish retrieval-owned embeddings"
    );

    let state = controller.state.lock();
    assert!(
        state.search_engine.is_none(),
        "grounding snapshot should not rebuild the full search engine"
    );
    assert!(
        state.node_names.is_empty(),
        "grounding snapshot should not repopulate node labels from search state"
    );
}

#[test]
fn retrieval_state_from_summary_open_keeps_search_state_cold() {
    let _env = hybrid_test_env();
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new_with_config(test_sidecar_runtime_from_env());

    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let retrieval = controller.retrieval_state().expect("retrieval state");
    assert_eq!(retrieval.mode, RetrievalModeDto::Symbolic);
    assert!(!retrieval.semantic_ready);
    assert_eq!(retrieval.semantic_doc_count, 0);

    let state = controller.state.lock();
    assert!(
        state.search_engine.is_none(),
        "retrieval_state should stay storage-backed on a cold controller"
    );
    assert!(
        state.node_names.is_empty(),
        "retrieval_state should not populate search labels on a cold controller"
    );
}

#[test]
fn completed_search_cache_load_does_not_mutate_live_semantic_rows() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    storage
        .put_index_publication(&test_index_publication(
            1,
            "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        ))
        .expect("publish identity");
    finalize_staged_semantic_docs(&mut storage, None, None, None).expect("finalize semantic rows");
    let before_legacy = storage
        .get_all_llm_symbol_docs()
        .expect("legacy semantic rows before cache load");
    let before_symbolic = storage
        .get_symbol_search_docs_batch_after(None, 10_000)
        .expect("symbolic rows before cache load");
    let before_dense = storage
        .get_dense_anchor_inputs_batch_after(None, 10_000)
        .expect("dense anchor inputs before cache load");
    assert!(before_legacy.is_empty());
    assert!(!before_symbolic.is_empty());
    assert!(!before_dense.is_empty());
    storage
        .get_connection()
        .execute_batch(
            "CREATE TRIGGER reject_live_llm_insert BEFORE INSERT ON llm_symbol_doc
             BEGIN SELECT RAISE(ABORT, 'live llm insert'); END;
             CREATE TRIGGER reject_live_llm_update BEFORE UPDATE ON llm_symbol_doc
             BEGIN SELECT RAISE(ABORT, 'live llm update'); END;
             CREATE TRIGGER reject_live_llm_delete BEFORE DELETE ON llm_symbol_doc
             BEGIN SELECT RAISE(ABORT, 'live llm delete'); END;
             CREATE TRIGGER reject_live_symbol_insert BEFORE INSERT ON symbol_search_doc
             BEGIN SELECT RAISE(ABORT, 'live symbol insert'); END;
             CREATE TRIGGER reject_live_symbol_update BEFORE UPDATE ON symbol_search_doc
             BEGIN SELECT RAISE(ABORT, 'live symbol update'); END;
             CREATE TRIGGER reject_live_symbol_delete BEFORE DELETE ON symbol_search_doc
             BEGIN SELECT RAISE(ABORT, 'live symbol delete'); END;
             CREATE TRIGGER reject_live_dense_insert BEFORE INSERT ON dense_anchor_input
             BEGIN SELECT RAISE(ABORT, 'live dense insert'); END;
             CREATE TRIGGER reject_live_dense_update BEFORE UPDATE ON dense_anchor_input
             BEGIN SELECT RAISE(ABORT, 'live dense update'); END;
             CREATE TRIGGER reject_live_dense_delete BEFORE DELETE ON dense_anchor_input
             BEGIN SELECT RAISE(ABORT, 'live dense delete'); END;",
        )
        .expect("install live semantic mutation guards");

    let result = rebuild_search_state_from_storage(&mut storage, &storage_path, None, true)
        .expect("hydrate cache without semantic persistence");

    assert!(!result.engine.semantic_index_ready());
    assert_eq!(result.engine.full_text_doc_count(), result.node_names.len());
    assert_eq!(
        storage
            .get_all_llm_symbol_docs()
            .expect("legacy semantic rows after cache load"),
        before_legacy
    );
    assert_eq!(
        storage
            .get_symbol_search_docs_batch_after(None, 10_000)
            .expect("symbolic rows after cache load"),
        before_symbolic
    );
    assert_eq!(
        storage
            .get_dense_anchor_inputs_batch_after(None, 10_000)
            .expect("dense anchor inputs after cache load"),
        before_dense
    );

    let cancel_token = CancellationToken::new();
    cancel_token.cancel();
    let error = flush_pending_dense_anchor_inputs(
        &mut storage,
        &[pending_semantic_doc_for_test(1, "cancelled")],
        "core:test-publication",
        current_epoch_ms(),
        &mut SemanticProjectionStats::default(),
        Some(&cancel_token),
    )
    .expect_err("cancelled semantic persistence must stop before DB upsert");
    assert_eq!(error.code, "cancelled");
    let error = finalize_staged_semantic_docs(&mut storage, None, None, Some(&cancel_token))
        .expect_err("cancelled semantic finalization must stop before persistence");
    assert_eq!(error.code, "cancelled");
}

#[test]
fn search_generation_path_rejects_invalid_publication_identity() {
    let publication = test_index_publication(1, "../outside");
    let error = search_index_path_for_publication(Path::new("codestory.db"), Some(&publication))
        .expect_err("path-shaped generation identity must be rejected");

    assert_eq!(error.code, "internal");
    assert!(
        error
            .message
            .contains("Invalid index publication generation id")
    );
}

#[test]
fn persisted_search_generations_do_not_overwrite_a_racing_reader() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);

    let old_publication = test_index_publication(1, "11111111-1111-4111-8111-111111111111");
    storage
        .put_index_publication(&old_publication)
        .expect("publish old core generation");
    let old_state = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("build old search generation");
    let old_path =
        search_index_path_for_publication(&storage_path, Some(&old_publication)).expect("old path");
    let same_generation_reader = SearchEngine::try_open_existing(&old_path)
        .expect("completed builder must retain only a shared generation lock");
    assert_eq!(same_generation_reader.tantivy_doc_count(), 3);
    drop(same_generation_reader);

    storage
        .insert_nodes_batch(&[Node {
            id: CoreNodeId(4),
            kind: NodeKind::FUNCTION,
            serialized_name: "gamma_generation_anchor".to_string(),
            qualified_name: Some("pkg::gamma_generation_anchor".to_string()),
            file_node_id: Some(CoreNodeId(1)),
            start_line: Some(8),
            end_line: Some(8),
            ..Default::default()
        }])
        .expect("insert new-generation symbol");
    let new_publication = test_index_publication(2, "22222222-2222-4222-8222-222222222222");
    storage
        .put_index_publication(&new_publication)
        .expect("publish new core generation");
    let new_state = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("build new search generation while old reader is live");

    assert!(
        old_state
            .engine
            .search_symbol("gamma_generation_anchor")
            .is_empty(),
        "old reader must remain bound to the old generation"
    );
    assert_eq!(
        new_state.engine.search_symbol("gamma_generation_anchor"),
        vec![CoreNodeId(4)]
    );
    let new_path =
        search_index_path_for_publication(&storage_path, Some(&new_publication)).expect("new path");
    assert!(old_path.is_dir());
    assert!(new_path.is_dir());
    assert_ne!(old_path, new_path);
}

#[test]
fn catalog_waiting_loader_reopens_core_and_search_as_one_generation() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let old_publication = test_index_publication(1, "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa");
    storage
        .publish_structural_text_unit_generation(&old_publication)
        .expect("publish old structural text identity");
    storage
        .put_index_publication(&old_publication)
        .expect("publish old identity");
    storage
        .publish_source_policy_exclusion_generation(
            &old_publication,
            "test-project",
            "test-workspace",
            default_source_policy_identity(),
            &[],
        )
        .expect("publish old source policy identity");
    drop(
        rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
            .expect("build old generation"),
    );

    let stale_storage = Storage::open(&storage_path).expect("open pre-publication reader");
    let catalog_guard =
        SearchGenerationCatalogGuard::acquire(&storage_path).expect("hold catalog for publish");
    let loader_path = storage_path.clone();
    let (started_tx, started_rx) = unbounded();
    let loader = std::thread::spawn(move || {
        let mut stale_storage = stale_storage;
        started_tx.send(()).expect("announce loader");
        load_persisted_search_state(&mut stale_storage, &loader_path)
            .expect("load post-publication generation")
    });
    started_rx.recv().expect("loader started");

    let mut staged = SnapshotStore::clone_live_to_staged(&storage_path)
        .expect("clone live database for replacement");
    staged
        .store_mut()
        .get_connection()
        .execute(
            "UPDATE node
             SET serialized_name = 'gamma_generation',
                 qualified_name = 'pkg::gamma_generation'
             WHERE id = 2",
            [],
        )
        .expect("rename symbol in staged core");
    let new_publication = test_index_publication(2, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    staged
        .store_mut()
        .publish_structural_text_unit_generation(&new_publication)
        .expect("publish staged structural text identity");
    staged
        .store_mut()
        .put_index_publication(&new_publication)
        .expect("publish staged identity");
    staged
        .store_mut()
        .publish_source_policy_exclusion_generation(
            &new_publication,
            "test-project",
            "test-workspace",
            default_source_policy_identity(),
            &[],
        )
        .expect("publish staged source policy identity");
    staged
        .publish(&storage_path)
        .expect("publish replacement core");

    let mut live = Storage::open(&storage_path).expect("open replacement core");
    let search_path = search_index_path_for_publication(&storage_path, Some(&new_publication))
        .expect("replacement search path");
    let mut built = build_persisted_search_state_from_canonical_symbols(
        &mut live,
        &search_path,
        false,
        &test_sidecar_runtime_from_env(),
        None,
    )
    .expect("build replacement search generation");
    write_search_generation_completion(
        &search_path,
        &new_publication,
        built.node_names.len(),
        built.engine.tantivy_doc_count(),
    )
    .expect("complete replacement search generation");
    built
        .engine
        .downgrade_persisted_lock_to_shared()
        .expect("share replacement generation");
    drop(catalog_guard);

    let loaded = loader.join().expect("join loader");
    assert_eq!(loaded.publication, Some(new_publication));
    assert_eq!(
        loaded.node_names.get(&CoreNodeId(2)).map(String::as_str),
        Some("pkg::gamma_generation")
    );
    assert!(
        loaded
            .engine
            .search_symbol("gamma_generation")
            .contains(&CoreNodeId(2))
    );
    drop(built);
}

#[test]
fn legacy_search_rebuild_cannot_delete_a_generation_reader() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("codestory.db");
    let legacy_path = search_index_storage_path(&storage_path);
    let mut legacy = SearchEngine::new(Some(&legacy_path)).expect("create legacy search index");
    legacy
        .index_nodes(vec![(CoreNodeId(1), "legacy_symbol".to_string())])
        .expect("index legacy symbol");
    drop(legacy);

    let publication = test_index_publication(1, "88888888-8888-4888-8888-888888888888");
    let generation_path = search_index_path_for_publication(&storage_path, Some(&publication))
        .expect("generation path");
    let mut generation =
        SearchEngine::new(Some(&generation_path)).expect("create generation search index");
    generation
        .index_nodes(vec![(CoreNodeId(2), "generation_symbol".to_string())])
        .expect("index generation symbol");

    let replacement_legacy =
        SearchEngine::new(Some(&legacy_path)).expect("rebuild independent legacy index");

    assert!(generation_path.is_dir());
    assert_eq!(
        generation.search_symbol("generation_symbol"),
        vec![CoreNodeId(2)]
    );
    drop(replacement_legacy);
    drop(generation);
}

#[test]
fn missing_corrupt_or_count_mismatched_search_generation_is_not_rebuilt_by_a_reader() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);
    let publication = test_index_publication(1, "33333333-3333-4333-8333-333333333333");
    storage
        .put_index_publication(&publication)
        .expect("publish core generation");
    let expected_path = search_index_path_for_publication(&storage_path, Some(&publication))
        .expect("expected path");
    let missing_error = match load_persisted_search_state(&mut storage, &storage_path) {
        Err(error) => error,
        Ok(_) => panic!("reader must not rebuild a missing search generation"),
    };
    assert_eq!(missing_error.code, "cache_busy");
    assert!(!expected_path.exists());

    storage = Storage::open(&storage_path).expect("reopen writer storage");
    drop(
        rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
            .expect("writer builds expected generation"),
    );
    let completion_path = search_generation_completion_path(&expected_path);
    let correct_completion = fs::read(&completion_path).expect("read completion marker");
    let mut mismatched_completion: SearchGenerationCompletion =
        serde_json::from_slice(&correct_completion).expect("decode completion marker");
    mismatched_completion.symbol_count = mismatched_completion.symbol_count.saturating_add(1);
    fs::write(
        &completion_path,
        serde_json::to_vec(&mismatched_completion).expect("encode mismatched completion"),
    )
    .expect("write mismatched completion marker");

    let mismatch_error = match load_persisted_search_state(&mut storage, &storage_path) {
        Err(error) => error,
        Ok(_) => panic!("reader must reject a count-mismatched search generation"),
    };
    assert_eq!(mismatch_error.code, "cache_busy");
    assert!(expected_path.is_dir());

    fs::write(&completion_path, correct_completion).expect("restore completion marker");
    fs::remove_dir_all(&expected_path).expect("remove built generation");
    fs::write(&expected_path, b"corrupt search generation")
        .expect("write corrupt generation artifact");

    let corrupt_error = match load_persisted_search_state(&mut storage, &storage_path) {
        Err(error) => error,
        Ok(_) => panic!("reader must not rebuild a corrupt search generation"),
    };

    assert_eq!(corrupt_error.code, "cache_busy");
    assert!(expected_path.is_file());
}

#[test]
fn search_generation_retention_keeps_active_and_one_verified_rollback() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let storage_path = temp.path().join("codestory.db");
    let ids = [
        "44444444-4444-4444-8444-444444444444",
        "55555555-5555-4555-8555-555555555555",
        "66666666-6666-4666-8666-666666666666",
        "77777777-7777-4777-8777-777777777777",
    ];
    let mut engines = Vec::new();
    for (offset, id) in ids.iter().enumerate() {
        let publication = test_index_publication(offset as u64 + 1, id);
        let path = search_index_path_for_publication(&storage_path, Some(&publication))
            .expect("generation path");
        let mut engine = SearchEngine::new(Some(&path)).expect("create search generation");
        engine
            .index_nodes(vec![(
                CoreNodeId(offset as i64 + 1),
                format!("symbol_{offset}"),
            )])
            .expect("index generation symbol");
        write_search_generation_completion(&path, &publication, 1, engine.tantivy_doc_count())
            .expect("complete search generation");
        engines.push(engine);
    }
    let active_engine = engines.pop().expect("active engine");
    let locked_old_engine = engines.remove(0);
    drop(engines);
    let malformed = search_index_generation_root(&storage_path).join("not-a-generation");
    fs::create_dir_all(&malformed).expect("create malformed generation");
    let malformed_file = search_index_generation_root(&storage_path).join("partial-generation");
    fs::write(&malformed_file, b"partial").expect("create partial generation artifact");
    let partial_publication = test_index_publication(8, "99999999-9999-4999-8999-999999999999");
    let partial_path = search_index_path_for_publication(&storage_path, Some(&partial_publication))
        .expect("partial generation path");
    let mut partial =
        SearchEngine::new(Some(&partial_path)).expect("create crash-partial generation");
    partial
        .index_nodes(vec![(CoreNodeId(99), "partial_symbol".to_string())])
        .expect("commit partial generation batch");
    drop(partial);
    let partial_lock_path = crate::search::engine::persisted_search_index_lock_path(&partial_path);

    prune_search_generations(&storage_path, ids[3]).expect("prune with locked reader");
    assert!(!malformed.exists());
    assert!(!malformed_file.exists());
    assert!(
        !partial_path.exists(),
        "structurally openable generation without completion marker is not a rollback"
    );
    assert!(
        partial_lock_path.is_file(),
        "generation lock files must remain durable after data pruning"
    );
    let first_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&partial_lock_path)
        .expect("open first durable lock handle");
    let second_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&partial_lock_path)
        .expect("open second durable lock handle");
    assert!(FileExt::try_lock_exclusive(&first_lock).expect("lock first handle"));
    assert!(
        !FileExt::try_lock_exclusive(&second_lock).expect("contend second handle"),
        "both handles must coordinate through the same durable lock file"
    );
    FileExt::unlock(&first_lock).expect("unlock first handle");
    assert!(FileExt::try_lock_exclusive(&second_lock).expect("lock second handle after release"));
    FileExt::unlock(&second_lock).expect("unlock second handle");
    assert!(
        search_index_generation_root(&storage_path)
            .join(ids[0])
            .is_dir(),
        "locked old generation must be skipped"
    );

    drop(locked_old_engine);
    prune_search_generations(&storage_path, ids[3]).expect("prune unlocked generations");
    let retained = fs::read_dir(search_index_generation_root(&storage_path))
        .expect("list retained generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 2, "active plus one rollback should remain");
    assert!(
        retained.iter().any(|entry| entry.file_name() == ids[3]),
        "active generation must remain"
    );
    drop(active_engine);
}

#[test]
fn search_without_publication_identity_uses_legacy_storage_path() {
    let _env = hybrid_test_env();
    let temp = tempdir().expect("create temp dir");
    let file_path = write_semantic_fixture(temp.path());
    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path).expect("open storage");
    insert_semantic_fixture_nodes(&mut storage, &file_path);

    let rebuilt = rebuild_search_state_from_storage(&mut storage, &storage_path, None, false)
        .expect("build legacy search index");

    assert!(search_index_storage_path(&storage_path).is_dir());
    assert!(!search_index_generation_root(&storage_path).exists());
    assert!(
        rebuilt
            .engine
            .search_symbol("beta")
            .contains(&CoreNodeId(3))
    );
}

#[test]
fn merge_search_hits_by_node_id_keeps_stronger_expanded_score() {
    let mut hits = vec![
        SearchHit {
            node_id: NodeId("primary".to_string()),
            display_name: "alpha".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(10),
            score: 0.25,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("secondary".to_string()),
            display_name: "alpha".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(20),
            score: 0.75,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
    ];

    merge_search_hits_by_node_id(
        &mut hits,
        vec![SearchHit {
            node_id: NodeId("primary".to_string()),
            display_name: "alpha".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/lib.rs".to_string()),
            line: Some(10),
            score: 250.0,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        }],
    );

    hits.sort_by(|left, right| compare_search_hits("alpha", left, right));

    assert_eq!(hits[0].node_id, NodeId("primary".to_string()));
    assert_eq!(hits[0].score, 250.0);
}

#[test]
fn primary_source_retention_keeps_short_precise_windows() {
    assert_eq!(primary_source_retention_threshold(1), 1);
    assert_eq!(primary_source_retention_threshold(3), 3);
    assert_eq!(primary_source_retention_threshold(10), 3);
    assert_eq!(primary_source_retention_threshold(50), 3);
}

#[test]
fn inexact_search_results_deduplicate_repeated_display_keys() {
    let mut hits = vec![
        SearchHit {
            node_id: NodeId("embedding-engine-id".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(178),
            score: 0.90,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("embedding-engine-id-copy".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(187),
            score: 0.80,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("other-helper".to_string()),
            display_name: "EmbeddingEngineCache::open".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(194),
            score: 0.70,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
    ];

    hits.sort_by(|left, right| {
        compare_search_hits(
            "embedding engine identity parser configuration",
            left,
            right,
        )
    });
    dedupe_inexact_search_hits_by_display_key(
        "embedding engine identity parser configuration",
        &mut hits,
    );

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].node_id, NodeId("embedding-engine-id".to_string()));
    assert_eq!(hits[1].node_id, NodeId("other-helper".to_string()));
}

#[test]
fn exact_search_results_keep_repeated_display_keys() {
    let mut hits = vec![
        SearchHit {
            node_id: NodeId("embedding-engine-id".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(178),
            score: 0.90,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
        SearchHit {
            node_id: NodeId("embedding-engine-id-copy".to_string()),
            display_name: "EMBEDDING_ENGINE_ID".to_string(),
            kind: codestory_contracts::api::NodeKind::FUNCTION,
            file_path: Some("src/search/engine.rs".to_string()),
            line: Some(187),
            score: 0.80,
            origin: codestory_contracts::api::SearchHitOrigin::IndexedSymbol,
            match_quality: None,
            resolvable: true,
            evidence_tier: None,
            evidence_producer: None,
            resolution_status: None,
            loss_reason: None,
            coverage_role: None,
            eligible_for_sufficiency: None,
            score_breakdown: None,
        },
    ];

    dedupe_inexact_search_hits_by_display_key("EMBEDDING_ENGINE_ID", &mut hits);

    assert_eq!(hits.len(), 2);
}

#[test]
fn hybrid_search_config_skips_exact_symbol_escalation_for_mixed_nl() {
    let req = SearchRequest {
        query: "how ExtensionHostManager starts".to_string(),
        repo_text: SearchRepoTextMode::Off,
        limit_per_source: 10,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    };
    let config = hybrid_search_config_for_request(&req, 10, None, true);
    assert_eq!(config.max_results, 10);
}

#[test]
fn staged_recovery_search_failure_preserves_the_marked_live_database() {
    let workspace = tempdir().expect("workspace dir");
    fs::write(
        workspace.path().join("lib.rs"),
        "pub fn value() -> i32 { 1 }\n",
    )
    .expect("write source");
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(
            workspace.path().to_path_buf(),
            storage_path.clone(),
        )
        .expect("open project");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("initial full index");
    Storage::open(&storage_path)
        .expect("open storage")
        .begin_incremental_run()
        .expect("simulate interrupted incremental");

    let search_path = search_index_generation_root(&storage_path);
    if search_path.is_dir() {
        fs::remove_dir_all(&search_path).expect("remove search directory");
    }
    fs::write(&search_path, b"not a search directory").expect("block search rebuild path");

    let error = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect_err("explicit full recovery cache rebuild must fail");

    assert!(
        error.message.contains("search"),
        "unexpected error: {error:?}"
    );
    let storage = Storage::open(&storage_path).expect("open replacement database");
    assert!(
        storage
            .has_incomplete_incremental_run()
            .expect("replacement marker")
    );
    assert!(
        storage
            .snapshots()
            .has_ready_summary()
            .expect("live summary readiness"),
        "pre-publication failure must preserve the live summary snapshot"
    );
    assert!(
        storage
            .snapshots()
            .has_ready_detail()
            .expect("live detail readiness"),
        "pre-publication failure must preserve the live detail snapshot"
    );
    storage
        .get_connection()
        .execute(
            "UPDATE incomplete_index_run
             SET started_at_epoch_ms = started_at_epoch_ms
             WHERE id = 1",
            [],
        )
        .expect("retain the fence in committed WAL state");
    let fenced_schema =
        Storage::database_schema_version(&storage_path).expect("replacement schema");
    assert_ne!(fenced_schema, codestory_store::CURRENT_SCHEMA_VERSION);
    let wal_path = storage_path.with_extension("db-wal");
    assert!(wal_path.is_file(), "fenced fixture must retain WAL state");
    let database_before = fs::read(&storage_path).expect("read fenced database before freshness");
    let wal_before = fs::read(&wal_path).expect("read fenced WAL before freshness");

    let cached = controller
        .index_freshness()
        .expect("cached recovery freshness");
    let uncached = controller
        .index_freshness_uncached()
        .expect("uncached recovery freshness");
    for freshness in [&cached, &uncached] {
        assert_eq!(freshness.status, IndexFreshnessStatusDto::Stale);
        assert_eq!(
            freshness.reason.as_deref(),
            Some("previous_incremental_run_incomplete_full_refresh_required")
        );
        assert_eq!(freshness.changed_file_count, 0);
        assert_eq!(freshness.new_file_count, 0);
        assert_eq!(freshness.removed_file_count, 0);
        assert_eq!(freshness.checked_file_count, 0);
        assert_eq!(freshness.indexed_file_count, 0);
        assert!(freshness.samples.is_empty());
    }
    assert_eq!(
        fs::read(&storage_path).expect("read fenced database after freshness"),
        database_before
    );
    assert_eq!(
        fs::read(&wal_path).expect("read fenced WAL after freshness"),
        wal_before
    );
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("schema after freshness"),
        fenced_schema
    );
    assert!(
        storage
            .has_incomplete_incremental_run()
            .expect("marker after freshness"),
        "freshness observation must preserve the durable fence"
    );

    drop(storage);
    fs::remove_file(&search_path).expect("remove search rebuild blocker");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("successful full recovery");
    assert_eq!(
        Storage::database_schema_version(&storage_path).expect("recovered schema"),
        codestory_store::CURRENT_SCHEMA_VERSION
    );
    let readable = Storage::open_read_only(&storage_path).expect("open recovered publication");
    assert!(
        !readable
            .has_incomplete_incremental_run()
            .expect("read recovered marker")
    );
    assert!(
        readable
            .get_complete_index_publication()
            .expect("read recovered publication")
            .is_some()
    );
}

#[test]
fn search_rejects_reads_while_indexing_is_active() {
    let controller = AppController::new();
    {
        let mut state = controller.state.lock();
        state.is_indexing = true;
    }

    let error = controller
        .search_results(SearchRequest {
            query: "check_winner".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("search should be blocked while indexing");

    assert_eq!(error.code, "invalid_argument");
    assert!(error.message.contains("indexing is in progress"));
}

#[test]
fn search_after_summary_open_stays_sidecar_primary_without_runtime_refresh() {
    let workspace = copy_tictactoe_workspace();
    let storage_path = workspace.path().join(".cache").join("codestory.db");
    let controller = AppController::new();

    controller
        .open_project_summary_with_storage_path(workspace.path().to_path_buf(), storage_path)
        .expect("open project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("index without runtime refresh");

    let error = controller
        .search(SearchRequest {
            query: "check_winner".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 10,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("search should require full sidecars after summary open");

    assert_mandatory_retrieval_unavailable(&error);
    let state = controller.state.lock();
    assert!(state.search_engine.is_none());
    assert!(state.node_names.is_empty());
}

#[test]
fn normalized_hybrid_weights_clamps_and_normalizes_values() {
    let fallback = HybridSearchConfig::default();
    let (lexical, semantic, graph) = normalized_hybrid_weights(
        Some(AgentHybridWeightsDto {
            lexical: Some(2.0),
            semantic: Some(-1.0),
            graph: Some(0.5),
        }),
        &fallback,
    );

    assert!((lexical - 0.666_666_7).abs() < 1e-4);
    assert!((semantic - 0.0).abs() < 1e-6);
    assert!((graph - 0.333_333_34).abs() < 1e-4);
}

#[test]
fn normalized_hybrid_weights_falls_back_when_invalid_sum() {
    let fallback = HybridSearchConfig::default();
    let (lexical, semantic, graph) = normalized_hybrid_weights(
        Some(AgentHybridWeightsDto {
            lexical: Some(0.0),
            semantic: Some(0.0),
            graph: Some(0.0),
        }),
        &fallback,
    );

    assert!((lexical - fallback.lexical_weight).abs() < 1e-6);
    assert!((semantic - fallback.semantic_weight).abs() < 1e-6);
    assert!((graph - fallback.graph_weight).abs() < 1e-6);
}

#[test]
fn hybrid_search_defaults_to_accuracy_first_semantic_profile() {
    let config = HybridSearchConfig::default();

    assert_eq!(config.max_results, 20);
    assert_eq!(config.lexical_weight, 0.0);
    assert_eq!(config.semantic_weight, 1.0);
    assert_eq!(config.graph_weight, 0.0);
    assert_eq!(config.lexical_limit, 0);
    assert_eq!(config.semantic_limit, 20);
}

#[test]
fn apply_hybrid_limits_overrides_and_caps_values() {
    let mut config = HybridSearchConfig::default();
    apply_hybrid_limits(
        Some(codestory_contracts::api::SearchHybridLimitsDto {
            lexical: Some(0),
            semantic: Some(5_000),
        }),
        &mut config,
    );

    assert_eq!(config.lexical_limit, 0);
    assert_eq!(config.semantic_limit, 1_000);
}
