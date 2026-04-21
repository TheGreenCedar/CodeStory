use codestory_contracts::api::{AgentHybridWeightsDto, NodeId, OpenProjectRequest, SearchRequest};
use codestory_contracts::events::EventBus;
use codestory_indexer::WorkspaceIndexer;
use codestory_runtime::AppController;
use codestory_store::Store as Storage;
use codestory_workspace::WorkspaceManifest;
use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn write_fixture(root: &Path, file_count: usize) -> anyhow::Result<()> {
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;

    for idx in 0..file_count {
        let path = src.join(format!("module_{idx}.rs"));
        let content = format!(
            r#"
pub fn handler_{idx}(user_role: &str) -> bool {{
    enforce_permission_policy_{idx}(user_role)
}}

pub fn enforce_permission_policy_{idx}(user_role: &str) -> bool {{
    matches!(user_role, "admin" | "owner")
}}

pub fn helper_{idx}() -> &'static str {{
    "ui render helper"
}}
"#
        );
        std::fs::write(path, content)?;
    }

    Ok(())
}

fn build_indexed_controller(file_count: usize) -> anyhow::Result<(TempDir, AppController)> {
    let temp = tempfile::tempdir()?;
    write_fixture(temp.path(), file_count)?;

    let storage_path = temp.path().join("codestory.db");
    let mut storage = Storage::open(&storage_path)?;
    let project = WorkspaceManifest::open(temp.path().to_path_buf())?;
    let refresh_info = project.full_refresh_execution_plan()?;
    let event_bus = EventBus::new();
    let indexer = WorkspaceIndexer::new(temp.path().to_path_buf());
    indexer.run_incremental(&mut storage, &refresh_info, &event_bus, None)?;
    drop(storage);

    // SAFETY: Criterion benches are executed in a controlled single-process context here,
    // and we set env vars before constructing the AppController/runtime that reads them.
    unsafe {
        // Benchmark uses deterministic local embeddings to avoid external model setup in CI/dev.
        std::env::set_var("CODESTORY_EMBED_RUNTIME_MODE", "hash");
        std::env::remove_var("CODESTORY_EMBED_MODEL_PATH");
    }

    let controller = AppController::new();
    controller
        .open_project(OpenProjectRequest {
            path: temp.path().to_string_lossy().to_string(),
        })
        .map_err(|error| anyhow::anyhow!("open_project failed: {:?}", error))?;

    Ok((temp, controller))
}

fn bench_ask_retrieval_latency(c: &mut Criterion) {
    let (_temp, controller) = build_indexed_controller(80).expect("prepare benchmark workspace");
    let query = SearchRequest {
        query: "permission policy checks".to_string(),
        repo_text: codestory_contracts::api::SearchRepoTextMode::Off,
        limit_per_source: 12,
        hybrid_weights: None,
        hybrid_limits: None,
    };

    c.bench_function("ask_retrieval_hybrid_latency", |b| {
        b.iter_custom(|iters| {
            let mut samples = Vec::with_capacity(iters as usize);
            for _ in 0..iters {
                let started = Instant::now();
                let _ = controller
                    .search_hybrid(
                        SearchRequest {
                            query: query.query.clone(),
                            repo_text: codestory_contracts::api::SearchRepoTextMode::Off,
                            limit_per_source: 12,
                            hybrid_weights: None,
                            hybrid_limits: None,
                        },
                        None::<NodeId>,
                        Some(12),
                        Some(AgentHybridWeightsDto {
                            lexical: Some(0.3),
                            semantic: Some(0.6),
                            graph: Some(0.1),
                        }),
                    )
                    .expect("hybrid search should succeed");
                samples.push(started.elapsed());
            }

            samples.sort_unstable();
            if !samples.is_empty() {
                let p50_idx = ((samples.len() as f64) * 0.50).floor() as usize;
                let p95_idx = ((samples.len() as f64) * 0.95).floor() as usize;
                let p50 = samples[p50_idx.min(samples.len() - 1)];
                let p95 = samples[p95_idx.min(samples.len() - 1)];
                eprintln!(
                    "[ask_retrieval_hybrid_latency] p50_ms={:.2} p95_ms={:.2} samples={}",
                    p50.as_secs_f64() * 1000.0,
                    p95.as_secs_f64() * 1000.0,
                    samples.len()
                );
            }

            samples.into_iter().sum::<Duration>()
        });
    });
}

criterion_group!(benches, bench_ask_retrieval_latency);
criterion_main!(benches);
