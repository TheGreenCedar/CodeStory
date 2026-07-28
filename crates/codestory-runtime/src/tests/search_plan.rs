use super::{
    HashMap, Path, SearchHitOrigin, SearchPlanActivePathEvidence, SearchPlanChannelDto,
    apply_architecture_cross_source_coverage, architecture_coverage_for_hit,
    architecture_query_intents, fs, search_plan_anchor_groups, search_plan_eligible,
    search_plan_path_is_test_or_bench, search_plan_runtime_call_is_speculative,
    search_plan_subqueries, search_plan_terms, search_plan_test_hit, tempdir,
};

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
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");
    let subqueries = search_plan_subqueries(query, &terms, &intents);
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
fn search_plan_terms_never_name_a_symbol_the_question_did_not() {
    for query in [
        "Explain how ProjectForge turns project/source-group configuration into indexing work, then how indexed data is accessed by the application. Cite the source files that support the path.",
        "Explain how `forge exec --json` flows from the top-level CLI into the exec runtime, thread and turn start requests, and JSONL event output.",
        "Explain how Root & Runtime public writing and social surfaces connect through collections, comment auth, RSS, and the elsewhere feed.",
    ] {
        let terms = search_plan_terms(query);
        let lower = query.to_ascii_lowercase();
        for term in &terms.extracted {
            assert!(
                lower.contains(&term.to_ascii_lowercase()),
                "extracted term `{term}` was never asked for in `{query}`: {:?}",
                terms.extracted
            );
        }
        for dropped in &terms.dropped {
            assert!(
                lower.contains(&dropped.term.to_ascii_lowercase()),
                "dropped term `{}` was never asked for in `{query}`: {:?}",
                dropped.term,
                terms.dropped
            );
        }
    }
}

#[test]
fn content_flow_questions_keep_every_word_the_question_used() {
    let query = "Explain how Root & Runtime public writing and social surfaces connect through collections, comment auth, RSS, and the elsewhere feed.";
    let terms = search_plan_terms(query);
    for expected in [
        "root",
        "runtime",
        "collections",
        "comment",
        "auth",
        "elsewhere",
        "feed",
    ] {
        assert!(
            terms
                .extracted
                .iter()
                .any(|term| term.eq_ignore_ascii_case(expected)),
            "asked term `{expected}` should survive extraction: {:?}",
            terms.extracted
        );
    }
    for dropped in &terms.dropped {
        assert!(
            ["too_short", "natural_language_filler"].contains(&dropped.reason.as_str()),
            "terms may only be dropped for length or filler, got `{}` for `{}`",
            dropped.reason,
            dropped.term
        );
    }
}

#[test]
fn multi_anchor_agent_question_prioritizes_named_anchor_subquery_terms() {
    let query = "Explain how ProjectAlpha turns configuration into processing work, then how processed data is accessed by the application. Anchor the answer around ConfigGroup, WorkerRunner, and DataAccess.";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(
        intents.iter().any(|intent| intent == "orchestration"),
        "explain-how architecture question should trigger a search plan: {intents:#?}"
    );
    let terms = search_plan_terms(query);
    for expected in ["ConfigGroup", "WorkerRunner", "DataAccess"] {
        assert!(
            terms.extracted.iter().any(|term| term == expected),
            "expected named anchor `{expected}` in extracted terms: {:?}",
            terms.extracted
        );
    }

    let subqueries = search_plan_subqueries(query, &terms, &intents);
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
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    assert!(
        search_plan_eligible(query, 3, &intents),
        "drill seed-anchor queries need a plan even when the anchors produce exact symbol hits"
    );

    let same_query_without_seed_anchors = "Explain how run_index RuntimeContext::ensure_open_from_summary WorkspaceIndexer::run moves through the runtime.";
    assert!(
        !search_plan_eligible(same_query_without_seed_anchors, 3, &intents),
        "ordinary exact-symbol queries should keep the exact-hit suppression"
    );
}

#[test]
fn broad_explain_how_search_plan_survives_generic_exact_hits() {
    let query = "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    assert!(
        search_plan_eligible(query, 7, &intents),
        "generic exact hits such as CLI should not suppress broad architecture search plans"
    );
    let terms = search_plan_terms(query);
    let subqueries = search_plan_subqueries(query, &terms, &intents);
    let roles = subqueries
        .iter()
        .map(|subquery| subquery.role.as_str())
        .collect::<Vec<_>>();
    // A prompt that names no identifier still gets the asked question plus its
    // own words as repo text; it does not get roles a term catalog invented.
    assert_eq!(
        roles,
        vec!["original_question", "typed_anchor_terms", "repo_text_terms"],
        "broad explain-how prompt should plan from its own words: {subqueries:#?}"
    );

    let ordinary_exact_query =
        "Explain how run_index RuntimeContext::ensure_open_from_summary moves through runtime.";
    assert!(
        !search_plan_eligible(ordinary_exact_query, 2, &intents),
        "ordinary exact-symbol explanations should still stay exact-first unless they name enough architecture surfaces"
    );
}

#[test]
fn prose_flow_questions_survive_exact_hits_whatever_domain_they_ask_about() {
    for query in [
        "Explain how a checkout moves from the storefront into payment capture, ledger posting, and receipt delivery.",
        "Explain how a full indexing run moves from the CLI into runtime orchestration, file discovery, symbol extraction, persistence, and search or snapshot refresh.",
        "Explain how a lab sample moves from intake through the assay queue into reported results.",
    ] {
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(
            !intents.is_empty(),
            "explain-how question should have architecture intent: `{query}`"
        );
        assert!(
            search_plan_eligible(query, 7, &intents),
            "a question that names no identifier should keep its plan whatever it asks about: `{query}`"
        );
    }
}

#[test]
fn questions_that_name_an_identifier_stay_exact_first() {
    for query in [
        "Explain how run_index moves work through the runtime.",
        "Explain how RuntimeContext::ensure_open_from_summary opens a stored snapshot.",
        "Explain how WorkspaceIndexer moves files into the store.",
    ] {
        let intents = architecture_query_intents(query)
            .into_iter()
            .map(|intent| intent.label().to_string())
            .collect::<Vec<_>>();
        assert!(
            !intents.is_empty(),
            "explain-how question should have architecture intent: `{query}`"
        );
        assert!(
            !search_plan_eligible(query, 2, &intents),
            "the asker named this symbol, so its exact hits answer the question: `{query}`"
        );
    }
}

#[test]
fn repo_text_identifiers_come_from_the_file_rather_than_a_noun_list() {
    let temp = tempdir().expect("create temp dir");
    let source_path = temp.path().join("src").join("queue.go");
    fs::create_dir_all(source_path.parent().expect("src parent")).expect("create src");
    fs::write(
        &source_path,
        "package queue\n\nfunc dispatch() {}\n\n\n\n// dispatch hands the job to the next worker\n",
    )
    .expect("write source");
    let symbol_hit = search_plan_test_hit(
        "symbol",
        "dispatch",
        &source_path,
        3,
        SearchHitOrigin::IndexedSymbol,
        false,
    );
    let repo_hit = search_plan_test_hit(
        "repo",
        "src/queue.go:7",
        &source_path,
        7,
        SearchHitOrigin::TextMatch,
        false,
    );
    let query = "how does a job reach the next worker";
    let terms = search_plan_terms(query);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[],
        &[repo_hit],
        &[symbol_hit],
        &HashMap::new(),
    );

    assert!(
        groups.iter().any(|group| {
            group
                .chosen_symbol
                .as_ref()
                .is_some_and(|hit| hit.display_name == "dispatch")
        }),
        "a lowercase symbol named by the file's own text should still bind: {groups:#?}"
    );
}

#[test]
fn search_plan_preserves_seed_anchor_line_exactly() {
    let query = "Explain how a full indexing run moves through the runtime. Seed anchors: run_index, run_index_once, RuntimeContext::ensure_open_from_summary, IndexService::run_indexing_blocking, AppController::run_indexing_blocking_inner, index_incremental, WorkspaceManifest::build_execution_plan, WorkspaceIndexer::run, WorkspaceIndexer::flush_projection_batch";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let terms = search_plan_terms(query);
    let subqueries = search_plan_subqueries(query, &terms, &intents);
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
    let query = "Explain how public writing/social surfaces connect to content collections, comment auth, and the elsewhere feed. Anchor the answer around Notes, getElsewhereFeed, and getCommentAuth.";
    let intents = architecture_query_intents(query)
        .into_iter()
        .map(|intent| intent.label().to_string())
        .collect::<Vec<_>>();
    assert!(!intents.is_empty(), "query should have architecture intent");

    let terms = search_plan_terms(query);
    let subqueries = search_plan_subqueries(query, &terms, &intents);
    for expected in ["Notes", "getElsewhereFeed", "getCommentAuth"] {
        assert!(
            subqueries
                .iter()
                .any(|subquery| subquery.role == "named_anchor" && subquery.query == expected),
            "expected named-anchor subquery for `{expected}`: {subqueries:#?}"
        );
    }
}

#[test]
fn architecture_cross_source_coverage_promotes_concrete_role_representatives() {
    let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
    let mut indexed_hits = vec![
        search_plan_test_hit(
            "persistent-h",
            "StorageAccess",
            Path::new("src/lib/data/storage/PersistentStorage.h"),
            17,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "generic-indexer",
            "Indexer",
            Path::new("src/lib/data/indexer/Indexer.h"),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "persistent-cpp",
            "PersistentStorage::PersistentStorage",
            Path::new("src/lib/data/storage/PersistentStorage.cpp"),
            32,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "project",
            "Project::isIndexing",
            Path::new("src/lib/project/Project.cpp"),
            92,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
    ];
    for index in 0..6 {
        indexed_hits.push(search_plan_test_hit(
            &format!("generic-indexer-{index}"),
            "Indexer",
            Path::new(&format!("src/lib/data/indexer/Indexer{index}.h")),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let mut indexed_candidates = indexed_hits.clone();
    indexed_candidates.push(search_plan_test_hit(
        "storage-access-h",
        "StorageAccess::~StorageAccess",
        Path::new("src/lib/data/storage/StorageAccess.h"),
        36,
        SearchHitOrigin::IndexedSymbol,
        true,
    ));

    let mut repo_text_hits = vec![search_plan_test_hit(
        "cdb-h",
        "src/lib_cxx/project/SourceGroupCxxCdb.h",
        Path::new("src/lib_cxx/project/SourceGroupCxxCdb.h"),
        1,
        SearchHitOrigin::TextMatch,
        false,
    )];
    for index in 0..9 {
        repo_text_hits.push(search_plan_test_hit(
            &format!("wizard-{index}"),
            "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData.cpp",
            Path::new(&format!(
                "src/lib_gui/qt/project_wizard/content/QtProjectWizardContentSourceGroupData{index}.cpp"
            )),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }
    let mut repo_text_candidates = repo_text_hits.clone();
    repo_text_candidates.push(search_plan_test_hit(
        "indexer-java",
        "src/lib_java/data/indexer/IndexerJava.cpp",
        Path::new("src/lib_java/data/indexer/IndexerJava.cpp"),
        15,
        SearchHitOrigin::TextMatch,
        false,
    ));

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let indexed_paths = indexed_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();

    for expected in [
        "src/lib/project/Project.cpp",
        "src/lib/data/storage/PersistentStorage.cpp",
        "src/lib/data/storage/PersistentStorage.h",
        "src/lib/data/storage/StorageAccess.h",
    ] {
        assert!(
            indexed_paths.contains(&expected),
            "expected indexed path `{expected}` in {indexed_paths:#?}"
        );
    }
    for expected in [
        "src/lib_cxx/project/SourceGroupCxxCdb.h",
        "src/lib_java/data/indexer/IndexerJava.cpp",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected repo-text path `{expected}` in {repo_text_paths:#?}"
        );
    }
    assert_eq!(indexed_hits.len(), 10);
    assert_eq!(repo_text_hits.len(), 10);
}

#[test]
fn architecture_cross_source_coverage_uses_replacement_budget_for_actual_admissions() {
    let query = "Explain how Sourcetrail turns project/source-group configuration into indexing work, then how indexed data is accessed by the application.";
    let mut indexed_hits = Vec::new();
    let indexed_candidates = Vec::new();
    let mut repo_text_hits = (0..10)
        .map(|index| {
            search_plan_test_hit(
                &format!("generic-source-group-{index}"),
                &format!("src/lib/project/SourceGroupGeneric{index}.cpp"),
                Path::new(&format!("src/lib/project/SourceGroupGeneric{index}.cpp")),
                1,
                SearchHitOrigin::TextMatch,
                false,
            )
        })
        .collect::<Vec<_>>();
    let mut repo_text_candidates = repo_text_hits.clone();
    for (id, path) in [
        (
            "source-group-cdb-h",
            "src/lib_cxx/project/SourceGroupCxxCdb.h",
        ),
        (
            "source-group-cdb-cpp",
            "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
        ),
        (
            "indexer-command-cxx-cpp",
            "src/lib_cxx/data/indexer/IndexerCommandCxx.cpp",
        ),
        (
            "indexer-command-cxx-h",
            "src/lib_cxx/data/indexer/IndexerCommandCxx.h",
        ),
        ("indexer-java", "src/lib_java/data/indexer/IndexerJava.cpp"),
        (
            "storage-proxy",
            "src/lib/data/storage/StorageAccessProxy.cpp",
        ),
    ] {
        repo_text_candidates.push(search_plan_test_hit(
            id,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "src/lib_cxx/project/SourceGroupCxxCdb.cpp",
        "src/lib_java/data/indexer/IndexerJava.cpp",
        "src/lib/data/storage/StorageAccessProxy.cpp",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected high-coverage late candidate `{expected}` in {repo_text_paths:#?}"
        );
    }
    assert_eq!(repo_text_paths.len(), 10);
}

#[test]
fn architecture_coverage_promotes_exec_flow_source_surfaces() {
    let expected = [
        (
            "codex-rs/cli/src/main.rs",
            "cli:top_level_entrypoint:impl",
            8,
        ),
        (
            "codex-rs/exec/src/main.rs",
            "exec:binary_entrypoint:impl",
            9,
        ),
        ("codex-rs/exec/src/cli.rs", "exec:cli_options:impl", 10),
        ("codex-rs/exec/src/lib.rs", "exec:runtime:impl", 9),
        ("codex-rs/exec/src/exec_events.rs", "exec:events:impl", 9),
        (
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
            "exec:jsonl_event_processor:impl",
            9,
        ),
        (
            "codex-rs/exec/src/event_processor.rs",
            "exec:event_processor:impl",
            8,
        ),
    ];

    for (path, expected_key, expected_score) in expected {
        let hit = search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        );
        let coverage = architecture_coverage_for_hit(&hit)
            .unwrap_or_else(|| panic!("expected coverage for {path}"));
        assert_eq!(coverage.key, expected_key);
        assert_eq!(coverage.score, expected_score);
    }
}

#[test]
fn architecture_coverage_promotes_payload_content_flow_surfaces() {
    let expected = [
        ("src/payload.config.ts", "payload:config:impl", 9),
        (
            "src/collections/Posts.ts",
            "payload:posts_collection:impl",
            10,
        ),
        (
            "src/collections/Comments.ts",
            "payload:comments_collection:impl",
            10,
        ),
        (
            "src/app/(frontend)/posts/[slug]/comments/route.ts",
            "comments:submission_route:impl",
            10,
        ),
        ("src/app/feed.xml/route.ts", "feed:rss_route:impl", 10),
        ("src/lib/payload.ts", "payload:client:impl", 10),
        (
            "src/lib/content-data/post-content.ts",
            "content:post_data:impl",
            10,
        ),
        (
            "src/lib/content-data/comment-content.ts",
            "content:comment_data:impl",
            10,
        ),
    ];

    for (path, expected_key, expected_score) in expected {
        let hit = search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        );
        let coverage = architecture_coverage_for_hit(&hit)
            .unwrap_or_else(|| panic!("expected coverage for {path}"));
        assert_eq!(coverage.key, expected_key);
        assert_eq!(coverage.score, expected_score);
    }
}

#[test]
fn architecture_cross_source_coverage_admits_late_payload_content_surfaces() {
    let query = "Explain how Root & Runtime public writing and social surfaces connect through Payload collections, post rendering, comment auth/submission, RSS, and the Elsewhere feed.";
    let mut indexed_hits = Vec::new();
    let indexed_candidates = Vec::new();
    let mut repo_text_hits = (0..10)
        .map(|index| {
            search_plan_test_hit(
                &format!("generic-payload-{index}"),
                &format!("src/app/(payload)/admin/importMap{index}.js"),
                Path::new(&format!("src/app/(payload)/admin/importMap{index}.js")),
                1,
                SearchHitOrigin::TextMatch,
                false,
            )
        })
        .collect::<Vec<_>>();
    let mut repo_text_candidates = repo_text_hits.clone();
    for path in [
        "src/collections/Posts.ts",
        "src/collections/Comments.ts",
        "src/app/(frontend)/posts/[slug]/comments/route.ts",
        "src/app/feed.xml/route.ts",
        "src/lib/payload.ts",
        "src/lib/content-data/post-content.ts",
        "src/lib/content-data/comment-content.ts",
    ] {
        repo_text_candidates.push(search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "src/collections/Posts.ts",
        "src/collections/Comments.ts",
        "src/app/(frontend)/posts/[slug]/comments/route.ts",
        "src/app/feed.xml/route.ts",
        "src/lib/payload.ts",
        "src/lib/content-data/post-content.ts",
        "src/lib/content-data/comment-content.ts",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected late Payload content surface `{expected}` in {repo_text_paths:#?}"
        );
    }
    assert_eq!(repo_text_paths.len(), 10);
}

#[test]
fn architecture_cross_source_coverage_admits_late_exec_flow_surfaces() {
    let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime and JSONL event output.";
    let mut indexed_hits = vec![search_plan_test_hit(
        "exec-cli",
        "Cli",
        Path::new("codex-rs/exec/src/cli.rs"),
        14,
        SearchHitOrigin::IndexedSymbol,
        true,
    )];
    for index in 0..9 {
        indexed_hits.push(search_plan_test_hit(
            &format!("generic-cli-{index}"),
            "Cli",
            Path::new(&format!("codex-rs/generic-{index}/src/cli.rs")),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let indexed_candidates = indexed_hits.clone();

    let mut repo_text_hits = vec![search_plan_test_hit(
        "exec-events",
        "codex-rs/exec/src/exec_events.rs",
        Path::new("codex-rs/exec/src/exec_events.rs"),
        8,
        SearchHitOrigin::TextMatch,
        false,
    )];
    for index in 0..9 {
        repo_text_hits.push(search_plan_test_hit(
            &format!("generic-client-{index}"),
            &format!("codex-rs/generic-{index}/src/client.rs"),
            Path::new(&format!("codex-rs/generic-{index}/src/client.rs")),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }
    let mut repo_text_candidates = repo_text_hits.clone();
    for path in [
        "codex-rs/cli/src/main.rs",
        "codex-rs/exec/src/main.rs",
        "codex-rs/exec/src/lib.rs",
    ] {
        repo_text_candidates.push(search_plan_test_hit(
            path,
            path,
            Path::new(path),
            1,
            SearchHitOrigin::TextMatch,
            false,
        ));
    }

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &repo_text_candidates,
        10,
    );

    let repo_text_paths = repo_text_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "codex-rs/exec/src/exec_events.rs",
        "codex-rs/cli/src/main.rs",
        "codex-rs/exec/src/main.rs",
        "codex-rs/exec/src/lib.rs",
    ] {
        assert!(
            repo_text_paths.contains(&expected),
            "expected exec-flow surface `{expected}` in {repo_text_paths:#?}"
        );
    }
}

#[test]
fn architecture_cross_source_coverage_admits_late_indexed_exec_flow_surfaces() {
    let query = "Explain how codex exec --json flows from the top-level CLI into the exec runtime and JSONL event output.";
    let mut indexed_hits = vec![
        search_plan_test_hit(
            "cli-main",
            "Subcommand::Exec",
            Path::new("codex-rs/cli/src/main.rs"),
            120,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
        search_plan_test_hit(
            "exec-lib",
            "run_exec_session",
            Path::new("codex-rs/exec/src/lib.rs"),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ),
    ];
    for index in 0..8 {
        indexed_hits.push(search_plan_test_hit(
            &format!("app-server-noise-{index}"),
            "CommandExec",
            Path::new(&format!(
                "codex-rs/app-server-protocol/src/protocol/v2/noise_{index}.rs"
            )),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let mut indexed_candidates = indexed_hits.clone();
    for (id, name, path) in [
        ("exec-cli", "Cli", "codex-rs/exec/src/cli.rs"),
        ("exec-main", "clap::Parser", "codex-rs/exec/src/main.rs"),
        (
            "exec-jsonl",
            "EventProcessorWithJsonOutput::emit",
            "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
        ),
        (
            "exec-events",
            "codex_protocol::models::WebSearchAction",
            "codex-rs/exec/src/exec_events.rs",
        ),
    ] {
        indexed_candidates.push(search_plan_test_hit(
            id,
            name,
            Path::new(path),
            1,
            SearchHitOrigin::IndexedSymbol,
            true,
        ));
    }
    let mut repo_text_hits = Vec::new();

    apply_architecture_cross_source_coverage(
        query,
        &mut indexed_hits,
        &mut repo_text_hits,
        &indexed_candidates,
        &[],
        10,
    );

    let indexed_paths = indexed_hits
        .iter()
        .filter_map(|hit| hit.file_path.as_deref())
        .collect::<Vec<_>>();
    for expected in [
        "codex-rs/exec/src/cli.rs",
        "codex-rs/exec/src/main.rs",
        "codex-rs/exec/src/event_processor_with_jsonl_output.rs",
        "codex-rs/exec/src/exec_events.rs",
    ] {
        assert!(
            indexed_paths.contains(&expected),
            "expected late indexed exec-flow surface `{expected}` in {indexed_paths:#?}"
        );
    }
    assert_eq!(indexed_paths.len(), 10);
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
        "pub fn getLatestSocialEntries() {}\npub fn getElsewhereFeed() {}\n",
    )
    .expect("write source");
    let active = search_plan_test_hit(
        "active",
        "getLatestSocialEntries",
        &source_path,
        1,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let definition_only = search_plan_test_hit(
        "definition",
        "getElsewhereFeed",
        &source_path,
        2,
        SearchHitOrigin::IndexedSymbol,
        true,
    );
    let query = "getElsewhereFeed latest social feed";
    let terms = search_plan_terms(query);
    let active_path_evidence = HashMap::from([
        (
            active.node_id.clone(),
            SearchPlanActivePathEvidence { caller_count: 2 },
        ),
        (
            definition_only.node_id.clone(),
            SearchPlanActivePathEvidence { caller_count: 0 },
        ),
    ]);

    let groups = search_plan_anchor_groups(
        query,
        &terms,
        &[definition_only, active],
        &[],
        &[],
        &active_path_evidence,
    );

    assert_eq!(
        groups
            .first()
            .and_then(|group| group.chosen_symbol.as_ref())
            .map(|hit| hit.display_name.as_str()),
        Some("getLatestSocialEntries"),
        "visible production callers should outrank a definition-only exact-name anchor: {groups:#?}"
    );
    assert!(
        groups.iter().any(|group| {
            group.anchor == "getElsewhereFeed"
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
