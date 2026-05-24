use codestory_contracts::api::{
    IndexMode, LayoutDirection, NodeId, RepoTextScanStatsDto, SearchRepoTextMode, SearchRequest,
    TrailCallerScope, TrailConfigDto, TrailDirection, TrailMode,
};
use codestory_runtime::AppController;
use criterion::measurement::WallTime;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const SMOKE_FILE_COUNT: usize = 1_000;
const LARGE_FILE_COUNT: usize = 10_000;
const FULL_FILE_COUNT: usize = 100_000;
const DEFAULT_FANOUT: usize = 8;
const HIGH_DEGREE_FANOUT: usize = 48;
const TRAIL_DEPTHS: &[u32] = &[2, 4, 6];
const CONCURRENCY_LEVELS: &[usize] = &[1, 4, 16];
const HEAVY_STRESS_GUARD: &str = "CODESTORY_ALLOW_HEAVY_STRESS";
const FULL_STRESS_GUARD: &str = "CODESTORY_ALLOW_100K_STRESS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StressScale {
    Smoke,
    Large,
    Full,
}

#[derive(Debug, Clone, Copy)]
enum StressGraphShape {
    LeafFanout,
    LayeredLeafFanout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BrowserLoopCounts {
    search_hits: usize,
    trail_nodes: usize,
    trail_edges: usize,
}

#[derive(Debug, Clone, Copy)]
struct BrowserLoopConsistency {
    workers: usize,
    expected: BrowserLoopCounts,
    min_search_hits: usize,
    max_search_hits: usize,
    min_trail_nodes: usize,
    max_trail_nodes: usize,
    min_trail_edges: usize,
    max_trail_edges: usize,
}

struct BrowserStressFixture {
    _temp: TempDir,
    controller: AppController,
    focus_id: NodeId,
}

fn stress_scale() -> StressScale {
    match std::env::var("CODESTORY_STRESS_SCALE")
        .unwrap_or_else(|_| "smoke".to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        "full" | "100k" => StressScale::Full,
        "large" | "10k" => StressScale::Large,
        _ => StressScale::Smoke,
    }
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn stress_file_counts() -> Vec<usize> {
    let heavy_allowed = env_flag(HEAVY_STRESS_GUARD);
    let full_allowed = env_flag(FULL_STRESS_GUARD);
    match (stress_scale(), heavy_allowed, full_allowed) {
        (StressScale::Smoke, _, _) => vec![SMOKE_FILE_COUNT],
        (StressScale::Large, true, _) => vec![SMOKE_FILE_COUNT, LARGE_FILE_COUNT],
        (StressScale::Large, false, _) => {
            eprintln!(
                "[browser_stress_guard] CODESTORY_STRESS_SCALE=large requires {HEAVY_STRESS_GUARD}=1; running smoke only"
            );
            vec![SMOKE_FILE_COUNT]
        }
        (StressScale::Full, true, true) => {
            vec![SMOKE_FILE_COUNT, LARGE_FILE_COUNT, FULL_FILE_COUNT]
        }
        (StressScale::Full, true, false) => {
            eprintln!(
                "[browser_stress_guard] CODESTORY_STRESS_SCALE=full requires {FULL_STRESS_GUARD}=1 for 100k; running 1k+10k"
            );
            vec![SMOKE_FILE_COUNT, LARGE_FILE_COUNT]
        }
        (StressScale::Full, false, _) => {
            eprintln!(
                "[browser_stress_guard] CODESTORY_STRESS_SCALE=full requires {HEAVY_STRESS_GUARD}=1 and {FULL_STRESS_GUARD}=1; running smoke only"
            );
            vec![SMOKE_FILE_COUNT]
        }
    }
}

fn configure_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group.sample_size(10);
    group
}

fn write_browser_stress_project(
    root: &Path,
    file_count: usize,
    fanout: usize,
    graph_shape: StressGraphShape,
) -> anyhow::Result<()> {
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;
    std::fs::write(
        root.join("Cargo.toml"),
        r#"[package]
name = "codestory-browser-stress"
version = "0.0.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )?;

    let mods = (0..file_count)
        .map(|idx| format!("pub mod module_{idx};"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(src.join("lib.rs"), format!("{mods}\n"))?;

    for idx in 0..file_count {
        let calls = (0..fanout)
            .map(|offset| {
                let target = (idx + offset + 1) % file_count.max(1);
                format!("    crate::module_{target}::browser_stress_leaf_{target}();")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let leaf_calls = match graph_shape {
            StressGraphShape::LeafFanout => String::new(),
            StressGraphShape::LayeredLeafFanout => (0..fanout)
                .map(|offset| {
                    let target = (idx + offset + 1) % file_count.max(1);
                    format!("    crate::module_{target}::browser_stress_leaf_{target}();")
                })
                .collect::<Vec<_>>()
                .join("\n"),
        };
        let policy_comment = if idx % 8 == 0 {
            "    // browser stress repo text policy marker for scan mode coverage\n"
        } else {
            ""
        };
        let content = format!(
            r#"
pub struct BrowserStressNode{idx} {{
    pub enabled: bool,
}}

pub fn browser_stress_entry_{idx}() -> bool {{
{policy_comment}{calls}
    browser_stress_leaf_{idx}()
}}

pub fn browser_stress_leaf_{idx}() -> bool {{
{leaf_calls}
    true
}}
"#
        );
        std::fs::write(src.join(format!("module_{idx}.rs")), content)?;
    }
    Ok(())
}

fn indexed_fixture(
    file_count: usize,
    fanout: usize,
    graph_shape: StressGraphShape,
) -> anyhow::Result<BrowserStressFixture> {
    let temp = tempfile::tempdir()?;
    write_browser_stress_project(temp.path(), file_count, fanout, graph_shape)?;

    // SAFETY: Criterion runs this benchmark in-process; the deterministic hash runtime is set
    // before constructing the controller that may lazily initialize retrieval state.
    unsafe {
        std::env::set_var("CODESTORY_EMBED_RUNTIME_MODE", "hash");
    }

    let controller = AppController::new();
    let storage_path = temp.path().join("codestory-stress.db");
    controller
        .open_project_summary_with_storage_path(temp.path().to_path_buf(), storage_path.clone())
        .map_err(|error| anyhow::anyhow!("open project summary failed: {:?}", error))?;
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .map_err(|error| anyhow::anyhow!("index stress fixture failed: {:?}", error))?;
    controller
        .open_project_with_storage_path(temp.path().to_path_buf(), storage_path)
        .map_err(|error| anyhow::anyhow!("open indexed project failed: {:?}", error))?;

    let focus_id = controller
        .search_results(SearchRequest {
            query: "browser_stress_entry_0".to_string(),
            repo_text: SearchRepoTextMode::Off,
            limit_per_source: 1,
            expand_search_plan: false,
            hybrid_weights: None,
            hybrid_limits: None,
        })
        .map_err(|error| anyhow::anyhow!("resolve stress focus failed: {:?}", error))?
        .indexed_symbol_hits
        .into_iter()
        .find(|hit| hit.resolvable)
        .map(|hit| hit.node_id)
        .ok_or_else(|| anyhow::anyhow!("stress focus symbol was not indexed"))?;

    Ok(BrowserStressFixture {
        _temp: temp,
        controller,
        focus_id,
    })
}

fn stress_search_request(mode: SearchRepoTextMode) -> SearchRequest {
    SearchRequest {
        query: "where is the browser stress repo text policy marker used".to_string(),
        repo_text: mode,
        limit_per_source: 12,
        expand_search_plan: false,
        hybrid_weights: None,
        hybrid_limits: None,
    }
}

fn stress_trail_request(root_id: NodeId, depth: u32) -> TrailConfigDto {
    TrailConfigDto {
        root_id,
        mode: TrailMode::Neighborhood,
        target_id: None,
        depth,
        direction: TrailDirection::Outgoing,
        caller_scope: TrailCallerScope::ProductionOnly,
        edge_filter: Vec::new(),
        show_utility_calls: true,
        hide_speculative: false,
        story: false,
        node_filter: Vec::new(),
        max_nodes: 2_000,
        layout_direction: LayoutDirection::Horizontal,
    }
}

fn log_search_validation(
    file_count: usize,
    mode: SearchRepoTextMode,
    elapsed: Duration,
    hits: usize,
    stats: Option<&RepoTextScanStatsDto>,
) {
    let repo_text_stats = stats.map(|stats| {
        serde_json::json!({
            "scanned_file_count": stats.scanned_file_count,
            "scanned_byte_count": stats.scanned_byte_count,
            "skipped_large_file_count": stats.skipped_large_file_count,
            "file_cap": stats.file_cap,
            "byte_cap": stats.byte_cap,
            "time_cap_ms": stats.time_cap_ms,
            "duration_ms": stats.duration_ms,
            "truncated": stats.truncated,
            "reason": stats.reason.as_deref(),
            "action": stats.action.as_deref(),
        })
    });
    eprintln!(
        "[browser_stress_repo_text_validation] files={file_count} mode={mode:?} elapsed_ms={:.2} hits={hits}",
        elapsed.as_secs_f64() * 1000.0
    );
    eprintln!(
        "[browser_stress_stats] {}",
        serde_json::json!({
            "lane": "repo_text",
            "files": file_count,
            "mode": format!("{mode:?}"),
            "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
            "hits": hits,
            "repo_text_stats": repo_text_stats,
        })
    );
}

fn log_trail_validation(
    file_count: usize,
    depth: u32,
    elapsed: Duration,
    nodes: usize,
    edges: usize,
    truncated: bool,
) {
    eprintln!(
        "[browser_stress_trail_validation] files={file_count} depth={depth} elapsed_ms={:.2} nodes={nodes} edges={edges} truncated={truncated}",
        elapsed.as_secs_f64() * 1000.0
    );
    eprintln!(
        "[browser_stress_stats] {}",
        serde_json::json!({
            "lane": "high_degree_trail",
            "files": file_count,
            "depth": depth,
            "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
            "nodes": nodes,
            "edges": edges,
            "truncated": truncated,
        })
    );
}

fn bench_repo_text_modes(c: &mut Criterion) {
    let mut group = configure_group(c, "browser_stress_repo_text_modes");
    for file_count in stress_file_counts() {
        let fixture = indexed_fixture(file_count, DEFAULT_FANOUT, StressGraphShape::LeafFanout)
            .expect("prepare repo-text stress fixture");
        for mode in [
            SearchRepoTextMode::Auto,
            SearchRepoTextMode::On,
            SearchRepoTextMode::Off,
        ] {
            let request = stress_search_request(mode);
            let started = Instant::now();
            let validation = fixture
                .controller
                .search_results(request.clone())
                .expect("validate repo-text stress search");
            log_search_validation(
                file_count,
                mode,
                started.elapsed(),
                validation.hits.len(),
                validation.repo_text_stats.as_ref(),
            );
            group.bench_function(BenchmarkId::new(format!("{mode:?}"), file_count), |b| {
                b.iter(|| {
                    let results = fixture
                        .controller
                        .search_results(request.clone())
                        .expect("repo-text stress search");
                    black_box((
                        results.hits.len(),
                        results
                            .repo_text_stats
                            .as_ref()
                            .map(|stats| stats.truncated),
                    ));
                })
            });
        }
    }
    group.finish();
}

fn bench_high_degree_trails(c: &mut Criterion) {
    let mut group = configure_group(c, "browser_stress_high_degree_trails");
    for file_count in [SMOKE_FILE_COUNT] {
        let fixture = indexed_fixture(
            file_count,
            HIGH_DEGREE_FANOUT,
            StressGraphShape::LayeredLeafFanout,
        )
        .expect("prepare trail stress fixture");
        let mut previous_nodes = 0;
        for &depth in TRAIL_DEPTHS {
            let request = stress_trail_request(fixture.focus_id.clone(), depth);
            let started = Instant::now();
            let validation = fixture
                .controller
                .graph_trail(request.clone())
                .expect("validate high-degree trail");
            log_trail_validation(
                file_count,
                depth,
                started.elapsed(),
                validation.nodes.len(),
                validation.edges.len(),
                validation.truncated,
            );
            assert!(
                validation.nodes.len() >= previous_nodes,
                "high-degree trail depth {depth} returned fewer nodes than the previous depth"
            );
            if previous_nodes > 0 && !validation.truncated {
                assert!(
                    validation.nodes.len() > previous_nodes,
                    "high-degree trail depth {depth} did not expand beyond the previous depth"
                );
            }
            previous_nodes = validation.nodes.len();
            group.bench_function(
                BenchmarkId::new(format!("depth_{depth}"), file_count),
                |b| {
                    b.iter(|| {
                        let graph = fixture
                            .controller
                            .graph_trail(request.clone())
                            .expect("high-degree trail");
                        black_box((graph.nodes.len(), graph.edges.len(), graph.truncated));
                    })
                },
            );
        }
    }
    group.finish();
}

fn browser_loop_counts(controller: &AppController, focus_id: &NodeId) -> BrowserLoopCounts {
    let search = controller
        .search_results(stress_search_request(SearchRepoTextMode::Off))
        .expect("browser loop baseline search");
    let trail = controller
        .graph_trail(stress_trail_request(focus_id.clone(), 2))
        .expect("browser loop baseline trail");
    BrowserLoopCounts {
        search_hits: search.hits.len(),
        trail_nodes: trail.nodes.len(),
        trail_edges: trail.edges.len(),
    }
}

fn run_concurrent_browser_loop(
    controller: AppController,
    focus_id: NodeId,
    concurrency: usize,
    label: &'static str,
    expected: BrowserLoopCounts,
) -> BrowserLoopConsistency {
    let mut handles = Vec::with_capacity(concurrency);
    for worker in 0..concurrency {
        let worker_controller = controller.clone();
        let worker_focus = focus_id.clone();
        handles.push(thread::spawn(move || {
            let search = worker_controller
                .search_results(stress_search_request(SearchRepoTextMode::Off))
                .expect("concurrent search");
            let trail = worker_controller
                .graph_trail(stress_trail_request(worker_focus, 2))
                .expect("concurrent trail");
            let counts = BrowserLoopCounts {
                search_hits: search.hits.len(),
                trail_nodes: trail.nodes.len(),
                trail_edges: trail.edges.len(),
            };
            black_box((label, worker, counts));
            counts
        }));
    }
    let mut results = Vec::with_capacity(concurrency);
    for handle in handles {
        let counts = handle.join().expect("browser stress worker");
        assert_eq!(
            counts, expected,
            "browser service proxy returned inconsistent counts at concurrency {concurrency}"
        );
        results.push(counts);
    }
    let min_search_hits = results
        .iter()
        .map(|counts| counts.search_hits)
        .min()
        .unwrap_or(expected.search_hits);
    let max_search_hits = results
        .iter()
        .map(|counts| counts.search_hits)
        .max()
        .unwrap_or(expected.search_hits);
    let min_trail_nodes = results
        .iter()
        .map(|counts| counts.trail_nodes)
        .min()
        .unwrap_or(expected.trail_nodes);
    let max_trail_nodes = results
        .iter()
        .map(|counts| counts.trail_nodes)
        .max()
        .unwrap_or(expected.trail_nodes);
    let min_trail_edges = results
        .iter()
        .map(|counts| counts.trail_edges)
        .min()
        .unwrap_or(expected.trail_edges);
    let max_trail_edges = results
        .iter()
        .map(|counts| counts.trail_edges)
        .max()
        .unwrap_or(expected.trail_edges);
    BrowserLoopConsistency {
        workers: concurrency,
        expected,
        min_search_hits,
        max_search_hits,
        min_trail_nodes,
        max_trail_nodes,
        min_trail_edges,
        max_trail_edges,
    }
}

fn log_concurrency_validation(label: &str, consistency: BrowserLoopConsistency, elapsed: Duration) {
    eprintln!(
        "[browser_stress_concurrency_validation] label={label} workers={} elapsed_ms={:.2} search_hits={} trail_nodes={} trail_edges={}",
        consistency.workers,
        elapsed.as_secs_f64() * 1000.0,
        consistency.expected.search_hits,
        consistency.expected.trail_nodes,
        consistency.expected.trail_edges,
    );
    eprintln!(
        "[browser_stress_stats] {}",
        serde_json::json!({
            "lane": "browser_service_concurrency_proxy",
            "label": label,
            "workers": consistency.workers,
            "elapsed_ms": elapsed.as_secs_f64() * 1000.0,
            "consistent": true,
            "search_hits": {
                "expected": consistency.expected.search_hits,
                "min": consistency.min_search_hits,
                "max": consistency.max_search_hits,
            },
            "trail_nodes": {
                "expected": consistency.expected.trail_nodes,
                "min": consistency.min_trail_nodes,
                "max": consistency.max_trail_nodes,
            },
            "trail_edges": {
                "expected": consistency.expected.trail_edges,
                "min": consistency.min_trail_edges,
                "max": consistency.max_trail_edges,
            },
        })
    );
}

fn bench_browser_service_concurrency_proxy(c: &mut Criterion) {
    let fixture = indexed_fixture(
        SMOKE_FILE_COUNT,
        DEFAULT_FANOUT,
        StressGraphShape::LeafFanout,
    )
    .expect("prepare concurrency fixture");
    let mut group = configure_group(c, "browser_stress_browser_service_concurrency_proxy");
    let label = "search_trail_service_proxy";
    let expected = browser_loop_counts(&fixture.controller, &fixture.focus_id);
    for &concurrency in CONCURRENCY_LEVELS {
        let started = Instant::now();
        let consistency = run_concurrent_browser_loop(
            fixture.controller.clone(),
            fixture.focus_id.clone(),
            concurrency,
            label,
            expected,
        );
        log_concurrency_validation(label, consistency, started.elapsed());
        group.bench_function(BenchmarkId::new(label, concurrency), |b| {
            b.iter_custom(|iters| {
                let mut total = Duration::ZERO;
                for _ in 0..iters {
                    let started = Instant::now();
                    run_concurrent_browser_loop(
                        fixture.controller.clone(),
                        fixture.focus_id.clone(),
                        concurrency,
                        label,
                        expected,
                    );
                    total += started.elapsed();
                }
                total
            })
        });
    }
    eprintln!(
        "[browser_stress_concurrency_note] browser_service_concurrency_proxy exercises shared browser-service work for stdio/http-shaped loads; it is not transport promotion proof."
    );
    group.finish();
}

fn print_stress_lane_configuration() {
    let mut counts = String::new();
    for (idx, count) in stress_file_counts().iter().enumerate() {
        if idx > 0 {
            counts.push(',');
        }
        let _ = write!(counts, "{count}");
    }
    eprintln!(
        "[browser_stress_config] scale={:?} file_counts={} repo_text_modes=auto,on,off trail_depths=2,4,6 concurrency=1,4,16 synthetic_only=true high_degree_files={} heavy_guard={} full_guard={}",
        stress_scale(),
        counts,
        SMOKE_FILE_COUNT,
        HEAVY_STRESS_GUARD,
        FULL_STRESS_GUARD
    );
}

fn bench_browser_stress(c: &mut Criterion) {
    print_stress_lane_configuration();
    bench_repo_text_modes(c);
    bench_high_degree_trails(c);
    bench_browser_service_concurrency_proxy(c);
}

criterion_group!(benches, bench_browser_stress);
criterion_main!(benches);
