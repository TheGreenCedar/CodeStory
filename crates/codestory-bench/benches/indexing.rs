use codestory_contracts::api::{IndexMode, IndexingPhaseTimings};
use codestory_contracts::events::EventBus;
use codestory_indexer::{IncrementalIndexingStats, WorkspaceIndexer};
use codestory_runtime::AppController;
use codestory_store::Store as Storage;
use codestory_workspace::{
    BuildMode, Language, LanguageSpecificSettings, LanguageStandard, RefreshExecutionPlan,
    SourceGroupSettings, WorkspaceManifest, WorkspaceSettings,
};
use criterion::measurement::WallTime;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

use codestory_bench::util;

const PARSE_HEAVY_FILE_COUNT: usize = 320;
const PARSE_HEAVY_METHODS_PER_FILE: usize = 48;
const PARSE_HEAVY_CALLSITES_PER_FILE: usize = 2;
const PARSE_HEAVY_TOUCHED_FILES: usize = 18;
const RESOLUTION_HEAVY_FILE_COUNT: usize = 180;
const RESOLUTION_HEAVY_METHODS_PER_FILE: usize = 12;
const RESOLUTION_HEAVY_CALLSITES_PER_FILE: usize = 48;
const RESOLUTION_HEAVY_TOUCHED_FILES: usize = 12;
const APP_FULL_REFRESH_FILE_COUNT: usize = 180;
const APP_FULL_REFRESH_FANOUT: usize = 6;
const APP_FULL_REFRESH_HELPERS_PER_FILE: usize = 6;

struct PersistentIndexFixture {
    _temp_dir: TempDir,
    root: PathBuf,
    storage_path: PathBuf,
    files: Vec<PathBuf>,
    touch_cursor: usize,
    label: &'static str,
}

fn open_temp_storage() -> (TempDir, Storage) {
    let storage_temp = tempfile::tempdir().expect("create benchmark storage tempdir");
    let storage_path = storage_temp.path().join("codestory.db");
    let storage = Storage::open(&storage_path).expect("open benchmark storage");
    (storage_temp, storage)
}

fn refresh_inputs_from_storage(storage: &Storage) -> codestory_workspace::RefreshInputs {
    codestory_workspace::RefreshInputs {
        stored_files: storage
            .get_files()
            .expect("list benchmark storage files")
            .into_iter()
            .map(|file| codestory_workspace::StoredFileState {
                id: file.id,
                path: file.path,
                modification_time: file.modification_time,
                indexed: file.indexed,
            })
            .collect(),
        inventory: Default::default(),
    }
}

fn bench_indexing_100_files_incremental_cold(c: &mut Criterion) {
    let file_count = 100;
    let temp_dir = util::generate_synthetic_project(file_count).unwrap();
    let root = temp_dir.path().to_path_buf();
    let files = util::collect_files_with_extension(&root, "cpp");

    c.bench_function("index_100_files_incremental_cold", |b| {
        b.iter(|| {
            let (_storage_temp, mut storage) = open_temp_storage();
            let indexer = WorkspaceIndexer::new(root.clone());
            let event_bus = EventBus::new();

            let refresh_info = RefreshExecutionPlan {
                mode: BuildMode::Incremental,
                files_to_index: files.clone(),
                files_to_remove: vec![],
                existing_file_ids: std::collections::HashMap::new(),
            };

            indexer
                .run(&mut storage, &refresh_info, &event_bus, None)
                .unwrap();
        })
    });
}

fn run_full_refresh_bench(
    root: std::path::PathBuf,
    files: Vec<std::path::PathBuf>,
) -> IncrementalIndexingStats {
    let (_storage_temp, mut storage) = open_temp_storage();
    let indexer = WorkspaceIndexer::new(root);
    let event_bus = EventBus::new();
    let plan = RefreshExecutionPlan {
        mode: BuildMode::FullRefresh,
        files_to_index: files,
        files_to_remove: Vec::new(),
        existing_file_ids: Default::default(),
    };

    indexer.run(&mut storage, &plan, &event_bus, None).unwrap()
}

fn black_box_indexing_stats(stats: &IncrementalIndexingStats) {
    black_box((
        stats.setup_existing_projection_ids_ms,
        stats.setup_seed_symbol_table_ms,
        stats.parse_index_ms,
        stats.projection_flush_ms,
        stats.flush_files_ms,
        stats.flush_nodes_ms,
        stats.flush_edges_ms,
        stats.flush_occurrences_ms,
        stats.flush_component_access_ms,
        stats.flush_callable_projection_ms,
        stats.edge_resolution_ms,
        stats.resolution_call_candidate_index_ms,
        stats.resolution_import_candidate_index_ms,
        stats.resolution_call_semantic_candidates_ms,
        stats.resolution_import_semantic_candidates_ms,
    ));
}

fn black_box_phase_timings(timings: &IndexingPhaseTimings) {
    black_box((
        timings.parse_index_ms,
        timings.projection_flush_ms,
        timings.edge_resolution_ms,
        timings.error_flush_ms,
        timings.cleanup_ms,
        timings.deferred_indexes_ms,
        timings.summary_snapshot_ms,
        timings.detail_snapshot_ms,
        timings.publish_ms,
    ));
}

fn log_indexing_shape(label: &str, stats: &IncrementalIndexingStats) {
    let parse_ms = stats.parse_index_ms;
    let flush_ms = stats.projection_flush_ms
        + stats.flush_files_ms
        + stats.flush_nodes_ms
        + stats.flush_edges_ms
        + stats.flush_occurrences_ms
        + stats.flush_component_access_ms
        + stats.flush_callable_projection_ms;
    let resolution_ms = stats.edge_resolution_ms
        + stats.resolution_call_candidate_index_ms
        + stats.resolution_import_candidate_index_ms
        + stats.resolution_call_semantic_candidates_ms
        + stats.resolution_import_semantic_candidates_ms;
    let total_ms = stats.setup_existing_projection_ids_ms
        + stats.setup_seed_symbol_table_ms
        + parse_ms
        + flush_ms
        + resolution_ms;

    eprintln!(
        "[indexing_large_repo_validation] scenario={label} total_ms={} parse_ms={} flush_ms={} resolution_ms={} call_candidate_ms={} import_candidate_ms={} call_semantic_ms={} import_semantic_ms={}",
        total_ms,
        parse_ms,
        flush_ms,
        resolution_ms,
        stats.resolution_call_candidate_index_ms,
        stats.resolution_import_candidate_index_ms,
        stats.resolution_call_semantic_candidates_ms,
        stats.resolution_import_semantic_candidates_ms
    );
}

fn log_incremental_shape(
    label: &str,
    touched_files: usize,
    planned_files: usize,
    stats: &IncrementalIndexingStats,
) {
    let parse_ms = stats.parse_index_ms;
    let flush_ms = stats.projection_flush_ms
        + stats.flush_files_ms
        + stats.flush_nodes_ms
        + stats.flush_edges_ms
        + stats.flush_occurrences_ms
        + stats.flush_component_access_ms
        + stats.flush_callable_projection_ms;
    let resolution_ms = stats.edge_resolution_ms
        + stats.resolution_call_candidate_index_ms
        + stats.resolution_import_candidate_index_ms
        + stats.resolution_call_semantic_candidates_ms
        + stats.resolution_import_semantic_candidates_ms;

    eprintln!(
        "[indexing_incremental_validation] scenario={label} touched_files={} planned_files={} parse_ms={} flush_ms={} resolution_ms={} resolved_calls={} resolved_imports={}",
        touched_files,
        planned_files,
        parse_ms,
        flush_ms,
        resolution_ms,
        stats.resolved_calls,
        stats.resolved_imports
    );
}

fn run_app_full_refresh_bench(root: PathBuf) -> IndexingPhaseTimings {
    let storage_path = root.join("codestory-bench.sqlite");
    let controller = AppController::new();
    controller
        .open_project_summary_with_storage_path(root, storage_path)
        .expect("open benchmark project summary");
    controller
        .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
        .expect("run app full refresh benchmark")
}

fn cxx_benchmark_manifest(root: PathBuf) -> WorkspaceManifest {
    WorkspaceManifest::from_parts(
        WorkspaceSettings {
            name: "benchmark".to_string(),
            version: 1,
            source_groups: vec![SourceGroupSettings {
                id: Uuid::new_v4(),
                language: Language::Cxx,
                standard: LanguageStandard::Default,
                source_paths: vec![root.clone()],
                exclude_patterns: Vec::new(),
                include_paths: Vec::new(),
                defines: Default::default(),
                language_specific: LanguageSpecificSettings::Cxx {
                    cdb_path: Some(root.join("compile_commands.json")),
                    header_paths: Vec::new(),
                    precompiled_header: None,
                },
            }],
        },
        root.join("codestory_project.json"),
    )
}

fn build_persistent_index_fixture(
    label: &'static str,
    file_count: usize,
    methods_per_file: usize,
    callsites_per_file: usize,
) -> anyhow::Result<PersistentIndexFixture> {
    let temp_dir =
        util::generate_repo_scale_project(file_count, methods_per_file, callsites_per_file)?;
    util::generate_compile_commands(temp_dir.path(), file_count)?;

    let root = temp_dir.path().to_path_buf();
    let files = util::collect_files_with_extension(&root, "cpp");
    let storage_path = root.join("codestory.db");
    let mut storage = Storage::open(&storage_path)?;
    let indexer = WorkspaceIndexer::new(root.clone());
    let event_bus = EventBus::new();
    let plan = RefreshExecutionPlan {
        mode: BuildMode::FullRefresh,
        files_to_index: files.clone(),
        files_to_remove: Vec::new(),
        existing_file_ids: Default::default(),
    };
    indexer.run(&mut storage, &plan, &event_bus, None)?;

    Ok(PersistentIndexFixture {
        _temp_dir: temp_dir,
        root,
        storage_path,
        files,
        touch_cursor: 0,
        label,
    })
}

fn run_incremental_touched_subset(
    fixture: &mut PersistentIndexFixture,
    touched_files: usize,
) -> (usize, IncrementalIndexingStats) {
    let touched = util::append_benchmark_markers(
        &fixture.files,
        fixture.touch_cursor,
        touched_files,
        fixture.label,
    )
    .expect("touch benchmark files");
    fixture.touch_cursor = (fixture.touch_cursor + touched.len()) % fixture.files.len().max(1);

    let mut storage = Storage::open(&fixture.storage_path).expect("open benchmark storage");
    let project = cxx_benchmark_manifest(fixture.root.clone());
    let plan = project
        .build_execution_plan(&refresh_inputs_from_storage(&storage))
        .expect("build incremental plan");
    let planned_files = plan.files_to_index.len();
    assert!(
        planned_files > 0,
        "touched subset benchmark should enqueue at least one file"
    );

    let indexer = WorkspaceIndexer::new(fixture.root.clone());
    let event_bus = EventBus::new();
    let stats = indexer
        .run(&mut storage, &plan, &event_bus, None)
        .expect("run incremental benchmark");
    (planned_files, stats)
}

fn validate_incremental_fixture(fixture: &mut PersistentIndexFixture, touched_files: usize) {
    let storage = Storage::open(&fixture.storage_path).expect("open seeded benchmark storage");
    let project = cxx_benchmark_manifest(fixture.root.clone());
    let idle_plan = project
        .build_execution_plan(&refresh_inputs_from_storage(&storage))
        .expect("build idle incremental plan");
    assert_eq!(
        idle_plan.files_to_index.len(),
        0,
        "seeded fixture should start with no pending incremental work"
    );
    drop(storage);

    let (planned_files, stats) = run_incremental_touched_subset(fixture, touched_files);
    log_incremental_shape(fixture.label, touched_files, planned_files, &stats);
}

fn configure_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group.sample_size(10);
    group
}

fn bench_indexing_large_repo_shapes(c: &mut Criterion) {
    let parse_heavy_temp = util::generate_repo_scale_project(
        PARSE_HEAVY_FILE_COUNT,
        PARSE_HEAVY_METHODS_PER_FILE,
        PARSE_HEAVY_CALLSITES_PER_FILE,
    )
    .unwrap();
    util::generate_compile_commands(parse_heavy_temp.path(), PARSE_HEAVY_FILE_COUNT).unwrap();
    let parse_heavy_root = parse_heavy_temp.path().to_path_buf();
    let parse_heavy_files = util::collect_files_with_extension(&parse_heavy_root, "cpp");

    let resolution_heavy_temp = util::generate_repo_scale_project(
        RESOLUTION_HEAVY_FILE_COUNT,
        RESOLUTION_HEAVY_METHODS_PER_FILE,
        RESOLUTION_HEAVY_CALLSITES_PER_FILE,
    )
    .unwrap();
    util::generate_compile_commands(resolution_heavy_temp.path(), RESOLUTION_HEAVY_FILE_COUNT)
        .unwrap();
    let resolution_heavy_root = resolution_heavy_temp.path().to_path_buf();
    let resolution_heavy_files = util::collect_files_with_extension(&resolution_heavy_root, "cpp");

    let parse_validation =
        run_full_refresh_bench(parse_heavy_root.clone(), parse_heavy_files.clone());
    let resolution_validation = run_full_refresh_bench(
        resolution_heavy_root.clone(),
        resolution_heavy_files.clone(),
    );
    log_indexing_shape("parse_heavy_full_refresh", &parse_validation);
    log_indexing_shape("resolution_heavy_full_refresh", &resolution_validation);
    assert!(
        parse_validation.parse_index_ms > 0,
        "parse-heavy benchmark fixture should exercise parsing"
    );
    assert!(
        resolution_validation.edge_resolution_ms > 0,
        "resolution-heavy benchmark fixture should exercise the resolution stage"
    );
    assert!(
        resolution_validation.resolution_call_candidate_index_ms > 0
            || resolution_validation.resolution_call_semantic_candidates_ms > 0
            || resolution_validation.resolution_import_candidate_index_ms > 0
            || resolution_validation.resolution_import_semantic_candidates_ms > 0,
        "resolution-heavy benchmark fixture should exercise candidate or semantic-resolution work"
    );

    let mut group = configure_group(c, "index_large_repo_shapes");

    group.bench_function(
        BenchmarkId::new("parse_heavy_full_refresh", PARSE_HEAVY_FILE_COUNT),
        |b| {
            b.iter(|| {
                let stats =
                    run_full_refresh_bench(parse_heavy_root.clone(), parse_heavy_files.clone());
                black_box_indexing_stats(&stats);
            })
        },
    );

    group.bench_function(
        BenchmarkId::new("resolution_heavy_full_refresh", RESOLUTION_HEAVY_FILE_COUNT),
        |b| {
            b.iter(|| {
                let stats = run_full_refresh_bench(
                    resolution_heavy_root.clone(),
                    resolution_heavy_files.clone(),
                );
                black_box_indexing_stats(&stats);
            })
        },
    );

    group.finish();
}

fn bench_indexing_large_repo_incremental_touched(c: &mut Criterion) {
    let mut parse_fixture = build_persistent_index_fixture(
        "parse_heavy_touched_subset",
        PARSE_HEAVY_FILE_COUNT,
        PARSE_HEAVY_METHODS_PER_FILE,
        PARSE_HEAVY_CALLSITES_PER_FILE,
    )
    .unwrap();
    let mut resolution_fixture = build_persistent_index_fixture(
        "resolution_heavy_touched_subset",
        RESOLUTION_HEAVY_FILE_COUNT,
        RESOLUTION_HEAVY_METHODS_PER_FILE,
        RESOLUTION_HEAVY_CALLSITES_PER_FILE,
    )
    .unwrap();

    validate_incremental_fixture(&mut parse_fixture, PARSE_HEAVY_TOUCHED_FILES);
    validate_incremental_fixture(&mut resolution_fixture, RESOLUTION_HEAVY_TOUCHED_FILES);

    let mut group = configure_group(c, "index_large_repo_incremental_touched_subset");
    group.bench_function(
        BenchmarkId::new("parse_heavy", PARSE_HEAVY_TOUCHED_FILES),
        |b| {
            b.iter(|| {
                let (planned_files, stats) =
                    run_incremental_touched_subset(&mut parse_fixture, PARSE_HEAVY_TOUCHED_FILES);
                black_box(planned_files);
                black_box_indexing_stats(&stats);
            })
        },
    );
    group.bench_function(
        BenchmarkId::new("resolution_heavy", RESOLUTION_HEAVY_TOUCHED_FILES),
        |b| {
            b.iter(|| {
                let (planned_files, stats) = run_incremental_touched_subset(
                    &mut resolution_fixture,
                    RESOLUTION_HEAVY_TOUCHED_FILES,
                );
                black_box(planned_files);
                black_box_indexing_stats(&stats);
            })
        },
    );
    group.finish();
}

fn bench_indexing_app_full_refresh_publish(c: &mut Criterion) {
    let temp = util::generate_grounding_project(
        APP_FULL_REFRESH_FILE_COUNT,
        APP_FULL_REFRESH_FANOUT,
        APP_FULL_REFRESH_HELPERS_PER_FILE,
    )
    .expect("prepare app full refresh benchmark workspace");
    let root = temp.path().to_path_buf();

    let validation = run_app_full_refresh_bench(root.clone());
    eprintln!(
        "[indexing_app_full_refresh_validation] parse_ms={} flush_ms={} resolve_ms={} deferred_indexes_ms={} summary_snapshot_ms={} publish_ms={}",
        validation.parse_index_ms,
        validation.projection_flush_ms,
        validation.edge_resolution_ms,
        validation.deferred_indexes_ms.unwrap_or(0),
        validation.summary_snapshot_ms.unwrap_or(0),
        validation.publish_ms.unwrap_or(0)
    );
    assert!(
        validation.deferred_indexes_ms.is_some(),
        "app full-refresh benchmark should expose deferred index timing"
    );
    assert!(
        validation.summary_snapshot_ms.is_some(),
        "app full-refresh benchmark should expose summary snapshot timing"
    );
    assert!(
        validation.publish_ms.is_some(),
        "app full-refresh benchmark should expose staged publish timing"
    );

    let mut group = configure_group(c, "index_app_full_refresh_publish");
    group.bench_function(
        BenchmarkId::new("staged_full_refresh", APP_FULL_REFRESH_FILE_COUNT),
        |b| {
            b.iter(|| {
                let timings = run_app_full_refresh_bench(root.clone());
                black_box_phase_timings(&timings);
            })
        },
    );
    group.finish();
}

criterion_group!(
    benches,
    bench_indexing_100_files_incremental_cold,
    bench_indexing_large_repo_shapes,
    bench_indexing_large_repo_incremental_touched,
    bench_indexing_app_full_refresh_publish
);
criterion_main!(benches);
