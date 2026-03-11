use codestory_api::{GroundingBudgetDto, GroundingSnapshotDto, ProjectSummary};
use codestory_app::AppController;
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_project::Project;
use codestory_storage::Storage;
use criterion::measurement::WallTime;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;
use tempfile::TempDir;

use codestory_bench::util;

const LARGE_REPO_FILE_COUNT: usize = 260;
const LARGE_REPO_FANOUT: usize = 8;
const LARGE_REPO_HELPERS_PER_FILE: usize = 8;

fn build_indexed_controller(
    file_count: usize,
    fanout: usize,
    helpers_per_file: usize,
) -> anyhow::Result<(TempDir, PathBuf, PathBuf, AppController)> {
    let temp = util::generate_grounding_project(file_count, fanout, helpers_per_file)?;
    let project_root = temp.path().to_path_buf();

    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path)?;
    let project = Project::open(project_root.clone())?;
    let refresh_info = project.full_refresh()?;
    let event_bus = EventBus::new();
    let indexer = WorkspaceIndexer::new(project_root.clone());
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;
    drop(storage);

    let controller = AppController::new();
    Ok((temp, project_root, storage_path, controller))
}

fn validate_grounding_scenario(project_root: &PathBuf, storage_path: &PathBuf) {
    let summary_controller = AppController::new();
    summary_controller
        .open_project_summary_with_storage_path(project_root.clone(), storage_path.clone())
        .expect("summary open should succeed");
    let strict = summary_controller
        .grounding_snapshot(GroundingBudgetDto::Strict)
        .expect("strict grounding snapshot should succeed");
    let balanced = summary_controller
        .grounding_snapshot(GroundingBudgetDto::Balanced)
        .expect("balanced grounding snapshot should succeed");

    assert!(
        strict.coverage.total_files >= LARGE_REPO_FILE_COUNT as u32,
        "strict snapshot should represent the generated large repo"
    );
    assert!(
        balanced.coverage.represented_symbols >= strict.coverage.represented_symbols,
        "balanced grounding should not surface fewer symbols than strict"
    );

    let full_controller = AppController::new();
    full_controller
        .open_project_with_storage_path(project_root.clone(), storage_path.clone())
        .expect("full open should succeed");
    let full_snapshot = full_controller
        .grounding_snapshot(GroundingBudgetDto::Balanced)
        .expect("full grounding snapshot should succeed");
    assert_eq!(
        full_snapshot.coverage.represented_files, balanced.coverage.represented_files,
        "summary-open and full-open balanced grounding should surface the same file coverage"
    );

    eprintln!(
        "[grounding_large_repo_validation] total_files={} strict_files={} balanced_files={} strict_symbols={} balanced_symbols={} root_symbols={}",
        balanced.coverage.total_files,
        strict.files.len(),
        balanced.files.len(),
        strict.coverage.represented_symbols,
        balanced.coverage.represented_symbols,
        balanced.root_symbols.len()
    );
}

fn configure_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group.sample_size(10);
    group
}

fn black_box_grounding_snapshot(snapshot: &GroundingSnapshotDto) {
    black_box((
        snapshot.stats.file_count,
        snapshot.stats.node_count,
        snapshot.stats.edge_count,
        snapshot.coverage.total_files,
        snapshot.coverage.represented_files,
        snapshot.coverage.represented_symbols,
        snapshot.coverage.compressed_files,
        snapshot.root_symbols.len(),
        snapshot.files.len(),
        snapshot.coverage_buckets.len(),
        snapshot.notes.len(),
        snapshot.recommended_queries.len(),
    ));
}

fn black_box_project_summary(summary: &ProjectSummary) {
    black_box((
        summary.stats.file_count,
        summary.stats.node_count,
        summary.stats.edge_count,
        summary.stats.error_count,
    ));
}

fn bench_grounding_snapshot(c: &mut Criterion) {
    let (_temp, project_root, storage_path, controller) = build_indexed_controller(
        LARGE_REPO_FILE_COUNT,
        LARGE_REPO_FANOUT,
        LARGE_REPO_HELPERS_PER_FILE,
    )
    .expect("prepare benchmark workspace");
    controller
        .open_project_summary_with_storage_path(project_root.clone(), storage_path.clone())
        .expect("project summary");

    validate_grounding_scenario(&project_root, &storage_path);

    let mut snapshot_group = configure_group(c, "grounding_snapshot_large_repo");
    snapshot_group.bench_function(BenchmarkId::new("strict", LARGE_REPO_FILE_COUNT), |b| {
        b.iter(|| {
            let snapshot = controller
                .grounding_snapshot(GroundingBudgetDto::Strict)
                .expect("grounding snapshot should succeed");
            black_box_grounding_snapshot(&snapshot);
        })
    });
    snapshot_group.bench_function(BenchmarkId::new("balanced", LARGE_REPO_FILE_COUNT), |b| {
        b.iter(|| {
            let snapshot = controller
                .grounding_snapshot(GroundingBudgetDto::Balanced)
                .expect("grounding snapshot should succeed");
            black_box_grounding_snapshot(&snapshot);
        })
    });
    snapshot_group.bench_function(
        BenchmarkId::new("summary_open_plus_balanced", LARGE_REPO_FILE_COUNT),
        |b| {
            b.iter(|| {
                controller
                    .open_project_summary_with_storage_path(
                        project_root.clone(),
                        storage_path.clone(),
                    )
                    .expect("summary open should succeed");
                let snapshot = controller
                    .grounding_snapshot(GroundingBudgetDto::Balanced)
                    .expect("grounding snapshot should succeed");
                black_box_grounding_snapshot(&snapshot);
            })
        },
    );
    snapshot_group.finish();

    let mut open_group = configure_group(c, "grounding_open_paths_large_repo");
    open_group.bench_function(
        BenchmarkId::new("open_project_summary", LARGE_REPO_FILE_COUNT),
        |b| {
            b.iter(|| {
                let controller = AppController::new();
                let summary = controller
                    .open_project_summary_with_storage_path(
                        project_root.clone(),
                        storage_path.clone(),
                    )
                    .expect("summary open should succeed");
                black_box_project_summary(&summary);
            })
        },
    );
    open_group.bench_function(
        BenchmarkId::new("open_project", LARGE_REPO_FILE_COUNT),
        |b| {
            b.iter(|| {
                let controller = AppController::new();
                let summary = controller
                    .open_project_with_storage_path(project_root.clone(), storage_path.clone())
                    .expect("project open should succeed");
                black_box_project_summary(&summary);
            })
        },
    );
    open_group.finish();
}

criterion_group!(benches, bench_grounding_snapshot);
criterion_main!(benches);
