use codestory_api::{IndexMode, OpenProjectRequest, TrailMode, TrailDirection, TrailCallerScope, LayoutDirection};
use codestory_app::AppController;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_cli_app_indexer_smoke() -> anyhow::Result<()> {
    // This test exercises CLI -> app -> project/storage -> indexer lifecycle without being a benchmark.
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
    
    // 1. Open project
    let summary = controller.open_project(OpenProjectRequest {
        path: root.to_string_lossy().to_string(),
    }).unwrap();
    assert_eq!(summary.stats.node_count, 0, "Should start empty");

    // 2. Index project
    let timings = controller.run_indexing_blocking(IndexMode::Full).unwrap();
    assert!(timings.parse_index_ms > 0);

    // Re-open to get refresh stats
    let summary = controller.open_project(OpenProjectRequest {
        path: root.to_string_lossy().to_string(),
    }).unwrap();
    assert!(summary.stats.node_count > 0);

    // 3. Search for a symbol
    let hits = controller.search(codestory_api::SearchRequest {
        query: "f0".to_string(),
    }).unwrap();
    assert!(!hits.is_empty(), "Search should find f0");
    
    let main_id = hits.into_iter().find(|h| h.display_name.contains("f0")).unwrap().node_id;

    // 4. Trail query with max_nodes = 10 to force truncation
    // This is the regression test around truncated trails not emitting fallback node IDs
    let trail = controller.trail_context(codestory_api::TrailConfigDto {
        root_id: main_id,
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth: 15,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: vec![],
        show_utility_calls: true,
        node_filter: vec![],
        max_nodes: 10,
        layout_direction: LayoutDirection::Horizontal,
    }).unwrap();

    println!("TRAIL RESULT: {:#?}", trail.trail);

    assert!(trail.trail.truncated, "Trail should be truncated due to max_nodes=10");
    assert!(trail.trail.omitted_edge_count > 0, "Should have omitted edge count > 0");
    assert!(trail.trail.nodes.len() <= 10, "Should adhere to max_nodes");
    
    // Verify that NO edges target a node that isn't in the returned node list.
    // If they did, GUI (AppController) would synthesize a raw ID fallback node (which we're testing against!)
    let returned_node_ids: std::collections::HashSet<_> = trail.trail.nodes.iter().map(|n| n.id.clone()).collect();
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
            codestory_api::NodeKind::UNKNOWN,
            "Found UNKNOWN fallback node! Truncation issue."
        );
    }

    Ok(())
}

#[test]
fn test_repo_scale_call_resolution() -> anyhow::Result<()> {
    // We only run this if we are running in the codestory repo so we can index ourselves
    let root_path = std::env::current_dir()?.join("../../").canonicalize()?;
    if !root_path.join("Cargo.toml").exists() {
        println!("Skipping repo-scale test as we are not at workspace root: {:?}", root_path);
        return Ok(());
    }

    let controller = AppController::new();
    let cache_dir = tempdir()?;
    let storage_path = cache_dir.path().join("codestory.db");

    println!("Indexing repo root: {:?}", root_path);
    let summary = controller.open_project_with_storage_path(root_path.clone(), storage_path.clone()).unwrap();
    
    // Auto-refresh should trigger full index
    let timings = controller.run_indexing_blocking(IndexMode::Full).unwrap();
    
    assert!(
        timings.unresolved_calls_start > 0,
        "Repo should have at least some graph-extracted unresolved CALL edges"
    );

    // Let's assert that we don't just have 0 usable call edges
    // Actually, "zero post-pass resolutions on this workspace is measurable and not confused with zero usable call graph"
    // implies that we still have CALL edges in the DB
    let mut storage = codestory_storage::Storage::open(&storage_path).unwrap();
    let edges = storage.get_edges().unwrap();
    let call_edges = edges.iter().filter(|e| e.kind == codestory_core::EdgeKind::CALL).count();
    
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
