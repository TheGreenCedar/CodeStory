use super::{
    AppController, CoreNodeId, FileInfo, HashMap, HashSet, Instant, Node, NodeKind,
    REPO_TEXT_MAX_FILE_BYTES, REPO_TEXT_SCAN_BYTE_CAP, REPO_TEXT_SCAN_FILE_CAP,
    REPO_TEXT_SCAN_TIME_CAP_MS, RepoTextScanStatsDto, SearchHitOrigin,
    SearchPlanPromotionStatusDto, SearchRepoTextMode, SearchRequest, SourceIndexPolicy, Storage,
    assert_mandatory_retrieval_unavailable, fs, search_plan_anchor_groups,
    search_plan_next_actions, search_plan_terms, search_plan_test_hit, tempdir,
};
use crate::repo_text::{RepoTextFileReadOutcome, read_repo_text_file};

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
        None,
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
        None,
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
        &SourceIndexPolicy::default(),
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
        &SourceIndexPolicy::default(),
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
        &SourceIndexPolicy::default(),
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
        &SourceIndexPolicy::default(),
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
        &SourceIndexPolicy::default(),
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

    let widened_policy = SourceIndexPolicy::oversized(REPO_TEXT_MAX_FILE_BYTES + 1_024);
    let scan = AppController::collect_repo_text_hits(
        &storage,
        Some(temp.path()),
        &widened_policy,
        "needle",
        10,
        &HashSet::new(),
    )
    .expect("repo text scan with widened source policy");

    assert_eq!(
        scan.hits.len(),
        1,
        "widened admitted source must be searchable"
    );
    assert_eq!(scan.stats.skipped_large_file_count, 0);
}

#[test]
fn repo_text_file_read_is_bounded_by_remaining_aggregate_budget() {
    let temp = tempdir().expect("temp dir");
    let source_path = temp.path().join("grew-after-metadata.rs");
    fs::write(&source_path, "x".repeat(4_096)).expect("write source");

    let result = read_repo_text_file(source_path.to_string_lossy().as_ref(), 8_192, 31);

    assert_eq!(result.outcome, RepoTextFileReadOutcome::ByteBudgetExceeded);
    assert_eq!(result.bytes_read, 0);
}

#[test]
fn repo_text_file_read_charges_invalid_utf8() {
    let temp = tempdir().expect("temp dir");
    let source_path = temp.path().join("invalid.rs");
    fs::write(&source_path, vec![0xff; 19]).expect("write invalid source");

    let result = read_repo_text_file(source_path.to_string_lossy().as_ref(), 64, 31);

    assert_eq!(result.outcome, RepoTextFileReadOutcome::Unreadable);
    assert_eq!(result.bytes_read, 19);
}
