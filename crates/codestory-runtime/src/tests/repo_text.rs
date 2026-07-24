use super::{
    AppController, CoreNodeId, FileInfo, HashMap, HashSet, Instant, Node, NodeKind, Path,
    REPO_TEXT_MAX_FILE_BYTES, REPO_TEXT_SCAN_BYTE_CAP, REPO_TEXT_SCAN_FILE_CAP,
    REPO_TEXT_SCAN_TIME_CAP_MS, RepoTextScanStatsDto, SearchHitOrigin, SearchPlanAnchorGroupDto,
    SearchPlanPromotionStatusDto, SearchRepoTextMode, SearchRequest, Storage,
    assert_mandatory_retrieval_unavailable, fs, search_plan_anchor_groups,
    search_plan_next_actions, search_plan_rejected_hits, search_plan_terms, search_plan_test_hit,
    tempdir, truncate_repo_text_hits_for_query,
};

#[test]
fn architecture_repo_text_window_preserves_coverage_surfaces() {
    let query = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
    let mut hits = vec![
        search_plan_test_hit(
            "runtime-lib",
            "crates/codestory-runtime/src/lib.rs",
            Path::new("crates/codestory-runtime/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-agent",
            "crates/codestory-runtime/src/agent/orchestrator.rs",
            Path::new("crates/codestory-runtime/src/agent/orchestrator.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "cli-runtime",
            "crates/codestory-cli/src/runtime.rs",
            Path::new("crates/codestory-cli/src/runtime.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-semantic",
            "crates/codestory-runtime/src/semantic_doc_text.rs",
            Path::new("crates/codestory-runtime/src/semantic_doc_text.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-symbol",
            "crates/codestory-runtime/src/symbol_query.rs",
            Path::new("crates/codestory-runtime/src/symbol_query.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-search",
            "crates/codestory-runtime/src/search/engine.rs",
            Path::new("crates/codestory-runtime/src/search/engine.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-search-runtime",
            "crates/codestory-runtime/src/search_runtime.rs",
            Path::new("crates/codestory-runtime/src/search_runtime.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-services",
            "crates/codestory-runtime/src/services.rs",
            Path::new("crates/codestory-runtime/src/services.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "cli-args",
            "crates/codestory-cli/src/args.rs",
            Path::new("crates/codestory-cli/src/args.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "runtime-browser",
            "crates/codestory-runtime/src/browser.rs",
            Path::new("crates/codestory-runtime/src/browser.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "indexer-lib",
            "crates/codestory-indexer/src/lib.rs",
            Path::new("crates/codestory-indexer/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "storage-impl",
            "crates/codestory-store/src/storage_impl/mod.rs",
            Path::new("crates/codestory-store/src/storage_impl/mod.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
    ];

    truncate_repo_text_hits_for_query(query, &mut hits, 10);
    let paths = hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();

    assert!(paths.contains(&"crates/codestory-runtime/src/lib.rs"));
    assert!(paths.contains(&"crates/codestory-cli/src/runtime.rs"));
    assert!(paths.contains(&"crates/codestory-runtime/src/services.rs"));
    assert!(paths.contains(&"crates/codestory-indexer/src/lib.rs"));
    assert!(paths.contains(&"crates/codestory-store/src/storage_impl/mod.rs"));
    assert_eq!(paths.len(), 10);
}

#[test]
fn architecture_repo_text_window_preserves_non_crate_source_surfaces() {
    let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
    let mut hits = vec![
        search_plan_test_hit(
            "custom-command",
            "src/lib/project/SourceGroupCustomCommand.cpp",
            Path::new("src/lib/project/SourceGroupCustomCommand.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-data",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-info",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.cpp",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.cpp",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory",
            "src/lib/project/SourceGroupFactory.cpp",
            Path::new("src/lib/project/SourceGroupFactory.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory-custom",
            "src/lib/project/SourceGroupFactoryModuleCustom.cpp",
            Path::new("src/lib/project/SourceGroupFactoryModuleCustom.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "python-empty",
            "src/lib_python/project/SourceGroupPythonEmpty.cpp",
            Path::new("src/lib_python/project/SourceGroupPythonEmpty.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory-cxx",
            "src/lib_cxx/project/SourceGroupFactoryModuleCxx.cpp",
            Path::new("src/lib_cxx/project/SourceGroupFactoryModuleCxx.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-data-h",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.h",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.h",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "wizard-info-h",
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.h",
            Path::new(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupInfoText.h",
            ),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "factory-java",
            "src/lib_java/project/SourceGroupFactoryModuleJava.cpp",
            Path::new("src/lib_java/project/SourceGroupFactoryModuleJava.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "cdb",
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
            Path::new("src/lib_cxx/project/SourceGroupCxxCdb.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "storage-access",
            "src/lib/data/storage/StorageAccess.h",
            Path::new("src/lib/data/storage/StorageAccess.h"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "storage-proxy",
            "src/lib/data/storage/StorageAccessProxy.cpp",
            Path::new("src/lib/data/storage/StorageAccessProxy.cpp"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
    ];

    truncate_repo_text_hits_for_query(query, &mut hits, 10);
    let paths = hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();

    assert!(paths.contains(&"src/lib_cxx/project/SourceGroupCxxCdb.cpp"));
    assert!(paths.contains(&"src/lib/data/storage/StorageAccess.h"));
    assert!(paths.contains(&"src/lib/data/storage/StorageAccessProxy.cpp"));
    assert_eq!(paths.len(), 10);
}

#[test]
fn search_plan_rejected_hits_exposes_repo_text_coverage_candidates() {
    let chosen = search_plan_test_hit(
        "project",
        "Project::isIndexing",
        Path::new("src/lib/project/Project.cpp"),
        92,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let anchor_groups = vec![SearchPlanAnchorGroupDto {
        anchor: "Project::isIndexing".to_string(),
        chosen_symbol: Some(chosen),
        supporting_hits: Vec::new(),
        promotion_status: SearchPlanPromotionStatusDto::TypedAnchor,
        promotion_method: None,
        caller_count: 0,
        definition_only: false,
        no_visible_callers: false,
        confidence: "high".to_string(),
        reasons: Vec::new(),
    }];
    let indexed_hits = vec![search_plan_test_hit(
        "storage-access",
        "StorageAccess::~StorageAccess",
        Path::new("src/lib/data/storage/StorageAccess.h"),
        36,
        SearchHitOrigin::IndexedSymbol,
        true,
    )];
    let repo_text_hits = vec![search_plan_test_hit(
        "source-group-cdb",
        "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
        Path::new("src/lib_cxx/project/SourceGroupCxxCdb.cpp"),
        1,
        SearchHitOrigin::TextMatch,
        false,
    )];

    let rejected = search_plan_rejected_hits(&anchor_groups, &[], &indexed_hits, &repo_text_hits);

    let repo_text = rejected
        .iter()
        .find(|hit| hit.origin == SearchHitOrigin::TextMatch)
        .expect("repo-text rejected hit should be retained for diagnostics");
    assert_eq!(
        repo_text.file_path.as_deref(),
        Some("src/lib_cxx/project/SourceGroupCxxCdb.cpp")
    );
    assert!(
        repo_text.reason.contains("source=repo_text")
            && repo_text
                .reason
                .contains("coverage_key=source_group:configuration:impl")
            && repo_text.reason.contains("coverage_score=10"),
        "repo-text rejection reason should include coverage provenance: {repo_text:#?}"
    );
}

#[test]
fn repo_text_window_does_not_diversify_non_architecture_queries() {
    let mut hits = vec![
        search_plan_test_hit(
            "first",
            "first",
            Path::new("crates/codestory-runtime/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "second",
            "second",
            Path::new("crates/codestory-indexer/src/lib.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
        search_plan_test_hit(
            "third",
            "third",
            Path::new("crates/codestory-store/src/storage_impl/mod.rs"),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ),
    ];

    truncate_repo_text_hits_for_query("run_index", &mut hits, 2);

    assert_eq!(
        hits.iter()
            .map(|hit| hit.node_id.0.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[test]
fn search_plan_repo_text_owner_identifier_does_not_promote_member_symbol() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("lib.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(
        &source_path,
        "pub struct WorkspaceIndexer;\n\nimpl WorkspaceIndexer {\n    pub fn normalize_index_path(&self) {}\n}\n\n\n\n// WorkspaceIndexer coordinates indexing flow\n",
    )
    .expect("write source");
    let member_hit = search_plan_test_hit(
        "member",
        "WorkspaceIndexer::normalize_index_path",
        &source_path,
        4,
        SearchHitOrigin::IndexedSymbol,
        false,
    );
    let repo_hit = search_plan_test_hit(
        "repo",
        "src/lib.rs:9",
        &source_path,
        9,
        SearchHitOrigin::TextMatch,
        false,
    );
    let query = "WorkspaceIndexer indexing flow";
    let terms = search_plan_terms(query);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[],
        &[repo_hit],
        &[member_hit],
        &HashMap::new(),
    );

    assert!(
        groups.iter().any(|group| {
            group.chosen_symbol.is_none()
                && matches!(
                    group.promotion_status,
                    SearchPlanPromotionStatusDto::Ambiguous
                )
        }),
        "owner-only repo-text mention should stay unbound instead of promoting to a member: {groups:#?}"
    );
}

#[test]
fn search_plan_repo_text_exact_terminal_identifier_promotes_member_symbol() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("lib.rs");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(
        &source_path,
        "pub struct WorkspaceIndexer;\n\nimpl WorkspaceIndexer {\n    pub fn normalize_index_path(&self) {}\n}\n\n\n\n// normalize_index_path normalizes storage keys before indexing\n",
    )
    .expect("write source");
    let member_hit = search_plan_test_hit(
        "member",
        "WorkspaceIndexer::normalize_index_path",
        &source_path,
        4,
        SearchHitOrigin::IndexedSymbol,
        false,
    );
    let repo_hit = search_plan_test_hit(
        "repo",
        "src/lib.rs:9",
        &source_path,
        9,
        SearchHitOrigin::TextMatch,
        false,
    );
    let query = "normalize_index_path storage keys";
    let terms = search_plan_terms(query);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[],
        &[repo_hit],
        &[member_hit],
        &HashMap::new(),
    );

    assert!(
        groups.iter().any(|group| {
            group
                .chosen_symbol
                .as_ref()
                .is_some_and(|hit| hit.display_name == "WorkspaceIndexer::normalize_index_path")
                && group.promotion_method.as_deref() == Some("same_file_exact_identifier")
        }),
        "exact terminal identifier should still promote to the matching member: {groups:#?}"
    );
    let next_actions = search_plan_next_actions(&groups);
    assert!(next_actions.iter().any(|action| {
        action.action == "snippet"
            && action.node_id.0 == "member"
            && action
                .options
                .iter()
                .any(|option| option == "function_body")
    }));
}

#[test]
fn search_results_ignores_repo_text_hits_without_full_sidecars() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let source_path = temp.path().join("src").join("lib.rs");
    std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    std::fs::write(
        &source_path,
        "fn alpha() {}\n// this explains how alpha work items flow through the runtime\n",
    )
    .expect("write source");

    {
        let mut storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 11,
                path: source_path.clone(),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 2,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::FILE,
                    serialized_name: source_path.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "alpha".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(1),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("open project");

    let error = controller
        .search_results(SearchRequest {
            query: "how does alpha work".to_string(),
            repo_text: SearchRepoTextMode::On,
            limit_per_source: 5,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("repo-text search should still require full sidecars");
    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn repo_text_auto_fallback_is_not_product_search_without_full_sidecars() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let source_path = temp.path().join("src").join("lib.rs");
    let readme_path = temp.path().join("README.md");
    std::fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    std::fs::write(&source_path, "pub fn unrelated_anchor() {}\n").expect("write source");
    std::fs::write(
        &readme_path,
        "GlobalResourceListView is a retired frontend surface mentioned in notes.\n",
    )
    .expect("write readme");

    {
        let mut storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 11,
                path: source_path.clone(),
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert source file");
        storage
            .insert_file(&FileInfo {
                id: 12,
                path: readme_path,
                language: "markdown".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert readme file");
        storage
            .insert_nodes_batch(&[
                Node {
                    id: CoreNodeId(11),
                    kind: NodeKind::FILE,
                    serialized_name: source_path.to_string_lossy().to_string(),
                    ..Default::default()
                },
                Node {
                    id: CoreNodeId(101),
                    kind: NodeKind::FUNCTION,
                    serialized_name: "unrelated_anchor".to_string(),
                    file_node_id: Some(CoreNodeId(11)),
                    start_line: Some(1),
                    ..Default::default()
                },
            ])
            .expect("insert nodes");
    }

    let controller = AppController::new();
    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
        .expect("open project");

    let error = controller
        .search_results(SearchRequest {
            query: "GlobalResourceListView".to_string(),
            repo_text: SearchRepoTextMode::Auto,
            limit_per_source: 5,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .expect_err("repo-text auto fallback should require full sidecars");
    assert_mandatory_retrieval_unavailable(&error);
}

#[test]
fn repo_text_ranking_uses_path_and_query_tokens_for_svelte_surfaces() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let rust_path = temp.path().join("src").join("commands.rs");
    let svelte_path = temp.path().join("src").join("App.svelte");
    std::fs::create_dir_all(rust_path.parent().expect("src parent")).expect("create src");
    std::fs::write(
        &rust_path,
        "pub fn get_snapshot() {}\n// invoke runtime bridge\n",
    )
    .expect("write rust");
    std::fs::write(
        &svelte_path,
        "const readSnapshot = () => invoke('get_snapshot');\n",
    )
    .expect("write svelte");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        for (id, path, language) in [(11, rust_path, "rust"), (12, svelte_path.clone(), "svelte")] {
            storage
                .insert_file(&FileInfo {
                    id,
                    path,
                    language: language.to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "readSnapshot get_snapshot App.svelte invoke",
        5,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(
        scan.hits
            .first()
            .is_some_and(|hit| hit.display_name.ends_with("App.svelte")),
        "Svelte command surface should rank first: {:#?}",
        scan.hits
    );
}

#[test]
fn repo_text_partial_matches_surface_public_page_wiring() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let page_path = temp
        .path()
        .join("src")
        .join("app")
        .join("(frontend)")
        .join("posts")
        .join("[slug]")
        .join("page.tsx");
    let social_path = temp.path().join("src").join("lib").join("social-feed.ts");
    std::fs::create_dir_all(page_path.parent().expect("page parent")).expect("create page dir");
    std::fs::create_dir_all(social_path.parent().expect("social parent"))
        .expect("create social dir");
    std::fs::write(
        &page_path,
        "import { PostComments } from './PostComments';\nexport default async function PostPage() { return <PostComments />; }\n",
    )
    .expect("write page");
    std::fs::write(
        &social_path,
        "export async function getElsewhereFeed() { return []; }\n",
    )
    .expect("write social feed");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        for (id, path, language) in [(11, page_path, "tsx"), (12, social_path, "typescript")] {
            storage
                .insert_file(&FileInfo {
                    id,
                    path,
                    language: language.to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 2,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "how posts comments auth and elsewhere feed connect to public pages",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(
        scan.hits.iter().any(|hit| hit
            .display_name
            .ends_with("src/app/(frontend)/posts/[slug]/page.tsx")),
        "natural-language repo text should surface public page wiring, not only symbols: {:#?}",
        scan.hits
    );
}

#[test]
fn repo_text_partial_match_requires_distinct_query_terms() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let page_path = temp.path().join("src").join("posts").join("page.tsx");
    std::fs::create_dir_all(page_path.parent().expect("page parent")).expect("create page dir");
    std::fs::write(&page_path, "export const posts = [];\n").expect("write page");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 11,
                path: page_path,
                language: "tsx".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert file");
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "posts comments auth",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(
        scan.hits.is_empty(),
        "one repeated term in path and file contents should not satisfy multi-concept repo-text matching: {:#?}",
        scan.hits
    );
}

#[test]
fn repo_text_scan_reports_file_cap_on_large_low_match_fixture() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let src = temp.path().join("src");
    std::fs::create_dir_all(&src).expect("create src");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        for idx in 0..(REPO_TEXT_SCAN_FILE_CAP + 3) {
            let path = src.join(format!("file_{idx}.rs"));
            std::fs::write(&path, format!("pub fn file_{idx}() {{}}\n"))
                .expect("write fixture file");
            storage
                .insert_file(&FileInfo {
                    id: idx as i64 + 1,
                    path,
                    language: "rust".to_string(),
                    modification_time: 1,
                    indexed: true,
                    complete: true,
                    line_count: 1,
                    file_role: codestory_store::FileRole::Source,
                })
                .expect("insert file");
        }
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "needle that is not present",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(scan.hits.is_empty());
    assert!(scan.stats.truncated, "{:?}", scan.stats);
    assert!(scan.stats.scanned_file_count <= REPO_TEXT_SCAN_FILE_CAP as u32);
    assert!(
        scan.stats
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("scanning") || reason.contains("ms"))
    );
    assert!(scan.stats.action.is_some());
}

#[test]
fn repo_text_scan_file_cap_sets_truncated_reason() {
    let mut stats = RepoTextScanStatsDto {
        scanned_file_count: REPO_TEXT_SCAN_FILE_CAP as u32,
        scanned_byte_count: 0,
        skipped_large_file_count: 0,
        file_cap: REPO_TEXT_SCAN_FILE_CAP as u32,
        byte_cap: REPO_TEXT_SCAN_BYTE_CAP as u32,
        time_cap_ms: REPO_TEXT_SCAN_TIME_CAP_MS as u32,
        duration_ms: 0,
        truncated: false,
        reason: None,
        action: None,
    };

    assert!(AppController::repo_text_scan_should_stop(
        &mut stats,
        &Instant::now()
    ));
    assert!(stats.truncated);
    assert!(
        stats
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("scanning 2000 files")),
        "{stats:?}"
    );
    assert!(stats.action.is_some());
}

#[test]
fn repo_text_scan_skips_large_files_before_reading_contents() {
    let temp = tempdir().expect("temp dir");
    let storage_path = temp.path().join("cache").join("codestory.db");
    std::fs::create_dir_all(storage_path.parent().expect("db parent")).expect("create db dir");
    let source_path = temp.path().join("large.rs");
    std::fs::write(
        &source_path,
        format!(
            "needle\n{}",
            "x".repeat(REPO_TEXT_MAX_FILE_BYTES as usize + 16)
        ),
    )
    .expect("write large source");

    {
        let storage = Storage::open(&storage_path).expect("open storage");
        storage
            .insert_file(&FileInfo {
                id: 1,
                path: source_path,
                language: "rust".to_string(),
                modification_time: 1,
                indexed: true,
                complete: true,
                line_count: 1,
                file_role: codestory_store::FileRole::Source,
            })
            .expect("insert file");
    }

    let storage = Storage::open(&storage_path).expect("reopen storage");
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        "needle",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan");

    assert!(scan.hits.is_empty());
    assert_eq!(scan.stats.scanned_file_count, 1);
    assert_eq!(scan.stats.scanned_byte_count, 0);
    assert_eq!(scan.stats.skipped_large_file_count, 1);
    assert!(!scan.stats.truncated);
}
