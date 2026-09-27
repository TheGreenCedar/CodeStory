use codestory_contracts::api::{
    IndexMode, LayoutDirection, ListRootSymbolsRequest, TrailCallerScope, TrailDirection, TrailMode,
};
use codestory_runtime::AppController;
use codestory_store::Store;
use std::fs;
use tempfile::tempdir;

fn should_run_repo_scale_test() -> bool {
    matches!(
        std::env::var("CODESTORY_RUN_REPO_SCALE_TEST").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

#[test]
fn test_cli_app_indexer_smoke() -> anyhow::Result<()> {
    // This test exercises CLI -> runtime -> project/storage -> indexer lifecycle without being a benchmark.
    // We simulate the sequence of commands the user would run via CLI wrapper.
    let dir = tempdir()?;
    let root = dir.path();

    // Create a dummy workspace with 12 functions to exceed the minimum `max_nodes` clamp of 10.
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir)?;
    let mut code = String::new();
    for i in 0..12 {
        if i == 11 {
            code.push_str(&format!("fn f{i}() {{}}\n"));
        } else {
            code.push_str(&format!("fn f{i}() {{ f{}(); }}\n", i + 1));
        }
    }
    fs::write(src_dir.join("main.rs"), code)?;

    let controller = AppController::new();
    let storage_path = root.join(".cache").join("codestory.db");

    // 1. Open project
    let summary = controller
        .open_project_with_storage_path(root.to_path_buf(), storage_path.clone())
        .unwrap();
    assert_eq!(summary.stats.node_count, 0, "Should start empty");

    // 2. Index project
    let timings = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .unwrap();
    assert!(timings.parse_index_ms > 0);

    // Re-open to get refresh stats
    let summary = controller
        .open_project_with_storage_path(root.to_path_buf(), storage_path)
        .unwrap();
    assert!(summary.stats.node_count > 0);

    // 3. Resolve an indexed symbol through the graph surface. Search is sidecar-primary and
    // requires retrieval sidecars, which this lifecycle smoke intentionally does not build.
    let symbols = controller
        .list_root_symbols(ListRootSymbolsRequest { limit: Some(50) })
        .unwrap();
    assert!(!symbols.is_empty(), "Root symbols should include f0");

    let main_id = symbols
        .into_iter()
        .find(|symbol| symbol.label.contains("f0"))
        .unwrap()
        .id;

    // 4. Trail query with max_nodes = 10 to force truncation
    // This is the regression test around truncated trails not emitting fallback node IDs
    let trail = controller
        .trail_context(codestory_contracts::api::TrailConfigDto {
            root_id: main_id,
            mode: TrailMode::Neighborhood,
            target_id: None,
            depth: 15,
            direction: TrailDirection::Outgoing,
            caller_scope: TrailCallerScope::ProductionOnly,
            edge_filter: vec![],
            show_utility_calls: true,
            hide_speculative: false,
            story: false,
            node_filter: vec![],
            max_nodes: 10,
            layout_direction: LayoutDirection::Horizontal,
        })
        .unwrap();

    println!("TRAIL RESULT: {:#?}", trail.trail);

    assert!(
        trail.trail.truncated,
        "Trail should be truncated due to max_nodes=10"
    );
    assert!(
        trail.trail.omitted_edge_count > 0,
        "Should have omitted edge count > 0"
    );
    assert!(trail.trail.nodes.len() <= 10, "Should adhere to max_nodes");

    // Verify that NO edges target a node that isn't in the returned node list.
    // If they did, GUI (AppController) would synthesize a raw ID fallback node (which we're testing against!)
    let returned_node_ids: std::collections::HashSet<_> =
        trail.trail.nodes.iter().map(|n| n.id.clone()).collect();
    for edge in trail.trail.edges {
        assert!(
            returned_node_ids.contains(&edge.source),
            "Edge source {} was not in returned nodes! Bug present.",
            edge.source.0
        );
        assert!(
            returned_node_ids.contains(&edge.target),
            "Edge target {} was not in returned nodes! Bug present.",
            edge.target.0
        );
    }

    // Also explicitly verify no "UNKNOWN" fallback nodes exist
    for node in trail.trail.nodes {
        // Fallback nodes had NodeKind::UNKNOWN and lack file paths
        assert_ne!(
            node.kind,
            codestory_contracts::api::NodeKind::UNKNOWN,
            "Found UNKNOWN fallback node! Truncation issue."
        );
    }

    Ok(())
}

#[test]
#[ignore = "indexes the full codestory repo and can exhaust memory; set CODESTORY_RUN_REPO_SCALE_TEST=1 and run this test explicitly"]
fn test_repo_scale_call_resolution() -> anyhow::Result<()> {
    if !should_run_repo_scale_test() {
        println!(
            "Skipping repo-scale test; set CODESTORY_RUN_REPO_SCALE_TEST=1 to run it explicitly."
        );
        return Ok(());
    }

    // We only run this if we are running in the codestory repo so we can index ourselves
    let root_path = std::env::current_dir()?.join("../../").canonicalize()?;
    if !root_path.join("Cargo.toml").exists() {
        println!(
            "Skipping repo-scale test as we are not at workspace root: {:?}",
            root_path
        );
        return Ok(());
    }

    let controller = AppController::new();
    let cache_dir = tempdir()?;
    let storage_path = cache_dir.path().join("codestory.db");

    println!("Indexing repo root: {:?}", root_path);
    let _summary = controller
        .open_project_with_storage_path(root_path.clone(), storage_path.clone())
        .unwrap();

    // Auto-refresh should trigger full index
    let timings = controller.run_indexing_blocking(IndexMode::Full).unwrap();

    assert!(
        timings.unresolved_calls_start > 0,
        "Repo should have at least some graph-extracted unresolved CALL edges"
    );

    // Let's assert that we don't just have 0 usable call edges
    // Actually, "zero post-pass resolutions on this workspace is measurable and not confused with zero usable call graph"
    // implies that we still have CALL edges in the DB
    let storage = Store::open(&storage_path).unwrap();
    let edges = storage.get_edges().unwrap();
    let call_edges = edges
        .iter()
        .filter(|e| e.kind == codestory_contracts::graph::EdgeKind::CALL)
        .count();

    // We expect there to be thousands of call edges parsed out of the syntax tree,
    // regardless of whether the post-pass resolution managed to link them to definitions.
    assert!(
        call_edges > 1000,
        "Should have parsed a large number of direct syntax-tree CALL edges, found: {}",
        call_edges
    );

    println!(
        "Repo-scale call resolution: {} direct edges parsed, {} unresolved at start of pass, {} successfully resolved post-pass",
        call_edges, timings.unresolved_calls_start, timings.resolved_calls
    );

    Ok(())
}

/// Excluded from the measurement project copy: build output, dependency
/// caches, and version-control state are not project source and would dwarf
/// the tracked tree. `.git` is excluded by name whether it is a directory or a
/// worktree pointer file, because a copied pointer would make discovery
/// enumerate the original checkout's tracked paths against a tree that does not
/// contain them. `.github` is excluded because it carries deliberately
/// malformed CI fixtures that fail source verification by design, which would
/// abort the full refresh this measurement needs.
const MEASUREMENT_PROJECT_SKIPPED_ENTRIES: [&str; 5] =
    [".git", ".github", ".claude", "target", "node_modules"];

/// Copy the repository's source tree into a scratch root.
///
/// The measurement needs a project it may edit. Indexing the checkout in place
/// would mean mutating the working tree to produce an incremental plan.
fn copy_measurement_project(source: &std::path::Path, target: &std::path::Path) -> u64 {
    let mut copied = 0;
    fs::create_dir_all(target).expect("create measurement project directory");
    for entry in fs::read_dir(source).expect("read measurement source directory") {
        let entry = entry.expect("read measurement source entry");
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if MEASUREMENT_PROJECT_SKIPPED_ENTRIES.contains(&name.as_str()) {
            continue;
        }
        let file_type = entry.file_type().expect("read measurement entry type");
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            copied += copy_measurement_project(&entry.path(), &target.join(&name));
            continue;
        }
        fs::copy(entry.path(), target.join(&name)).expect("copy measurement project file");
        copied += 1;
    }
    copied
}

/// S7 measurement: what an incremental publication spends creating an
/// immutable generation, including the CoW stage, one-time validation,
/// generation rename, and atomic pointer replacement.
///
/// The numbers this prints are the decision input recorded in
/// `docs/architecture/indexing-pipeline.md`. Build it optimized: a `-O0` build
/// inflates the indexer far more than it inflates SQLite's copy path, which
/// would make the movement share look smaller than it is.
///
/// ```text
/// CARGO_PROFILE_DEV_OPT_LEVEL=3 CARGO_PROFILE_DEV_DEBUG=0 \
///   cargo test -p codestory-runtime --test integration \
///   incremental_publication_immutable_generation_measurement -- --ignored --nocapture
/// ```
#[test]
#[ignore = "measurement lane; build optimized and run with --ignored --nocapture"]
fn incremental_publication_immutable_generation_measurement() -> anyhow::Result<()> {
    let repo_root = std::env::current_dir()?.join("../../").canonicalize()?;
    if !repo_root.join("Cargo.toml").exists() {
        println!("Skipping measurement: not at the workspace root: {repo_root:?}");
        return Ok(());
    }

    let scratch = tempdir()?;
    let project_root = scratch.path().join("project");
    let copied = copy_measurement_project(&repo_root, &project_root);
    let storage_path = scratch.path().join("cache").join("codestory.db");

    let controller = AppController::new();
    controller
        .open_project_with_storage_path(project_root.clone(), storage_path.clone())
        .expect("open measurement project");

    let full_started = std::time::Instant::now();
    let full = controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("full measurement index");
    let full_wall_ms = full_started.elapsed().as_millis();
    let published_file_bytes =
        fs::metadata(codestory_store::resolve_core_database_path(&storage_path)?)?.len();
    println!(
        "measurement project: {copied} files copied from {}",
        repo_root.display()
    );
    println!(
        "full refresh: wall_ms={full_wall_ms} published_core_file_bytes={published_file_bytes} candidate_bytes={:?}",
        full.core_promotion.as_ref().map(|p| p.candidate_bytes)
    );

    // One changed file is the smallest non-empty immutable-generation refresh:
    // the smallest non-empty plan against the whole published core. An empty
    // plan already short-circuits, so it is not the case in question.
    let edited = project_root.join("crates/codestory-store/src/sqlite_path.rs");
    let mut edited_source = fs::read_to_string(&edited)?;

    for round in 1..=5 {
        edited_source.push_str(&format!(
            "\n#[allow(dead_code)]\npub fn s7_measurement_probe_{round}() -> u32 {{ {round} }}\n"
        ));
        fs::write(&edited, &edited_source)?;

        let incremental_started = std::time::Instant::now();
        let incremental = controller
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Incremental)
            .expect("incremental measurement index");
        let incremental_wall_ms = incremental_started.elapsed().as_millis();

        let probe = incremental
            .incremental_plan_probe
            .as_ref()
            .expect("incremental plan probe telemetry");
        let clone = incremental
            .staged_snapshot_copy
            .as_ref()
            .expect("incremental clone telemetry");
        let promotion = incremental
            .core_promotion
            .as_ref()
            .expect("incremental promotion telemetry");

        let publication_ms = u128::from(clone.copy_ms)
            + u128::from(promotion.generation_install_ms)
            + u128::from(promotion.pointer_publication_ms);

        println!(
            "incremental round {round}: files_to_index={} files_to_remove={} outcome={:?}",
            probe.files_to_index, probe.files_to_remove, probe.outcome
        );
        println!(
            "  wall_ms={incremental_wall_ms} publish_ms={:?} promotion_total_ms={}",
            incremental.publish_ms, promotion.total_ms
        );
        println!(
            "  immutable_publication_ms={publication_ms} (cow_stage={} generation_install={} pointer_publication={}) live_bytes={:?} candidate_bytes={}",
            clone.copy_ms,
            promotion.generation_install_ms,
            promotion.pointer_publication_ms,
            promotion.previous_live_bytes,
            promotion.candidate_bytes
        );
        println!(
            "  promotion_total_ms={} (lock_recovery={} candidate_validation={} previous_validation={} generation_install={} pointer_publication={} cleanup={} unattributed={})",
            promotion.total_ms,
            promotion.lock_recovery_ms,
            promotion.candidate_validation_ms,
            promotion.previous_validation_ms,
            promotion.generation_install_ms,
            promotion.pointer_publication_ms,
            promotion.cleanup_ms,
            promotion.unattributed_ms
        );
        println!(
            "  candidate_validation_ms={} indexer_projection_transactions={:?}",
            promotion.candidate_validation_ms, incremental.projection_batch_transactions
        );
        println!(
            "  generation_identity_fence={} publication_share_of_refresh={:.1}% publish_share_of_refresh={:.1}%",
            promotion.promoted_validation.as_str(),
            (publication_ms as f64 / incremental_wall_ms.max(1) as f64) * 100.0,
            (f64::from(incremental.publish_ms.unwrap_or_default())
                / incremental_wall_ms.max(1) as f64)
                * 100.0
        );
    }

    Ok(())
}
