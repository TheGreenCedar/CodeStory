use codestory_core::{EdgeKind, ResolutionCertainty};
use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;
use tempfile::tempdir;

struct FidelityFixture {
    path: &'static str,
    source: &'static str,
}

const FIXTURES: &[FidelityFixture] = &[
    FidelityFixture {
        path: "python_fidelity_lab.py",
        source: include_str!("../../codestory-index/tests/fixtures/fidelity_lab/python_fidelity_lab.py"),
    },
    FidelityFixture {
        path: "typescript_fidelity_lab.ts",
        source: include_str!("../../codestory-index/tests/fixtures/fidelity_lab/typescript_fidelity_lab.ts"),
    },
    FidelityFixture {
        path: "javascript_fidelity_lab.js",
        source: include_str!("../../codestory-index/tests/fixtures/fidelity_lab/javascript_fidelity_lab.js"),
    },
    FidelityFixture {
        path: "java_fidelity_lab.java",
        source: include_str!("../../codestory-index/tests/fixtures/fidelity_lab/java_fidelity_lab.java"),
    },
    FidelityFixture {
        path: "cpp_fidelity_lab.cpp",
        source: include_str!("../../codestory-index/tests/fixtures/fidelity_lab/cpp_fidelity_lab.cpp"),
    },
    FidelityFixture {
        path: "c_fidelity_lab.c",
        source: include_str!("../../codestory-index/tests/fixtures/fidelity_lab/c_fidelity_lab.c"),
    },
    FidelityFixture {
        path: "rust_fidelity_lab.rs",
        source: include_str!("../../codestory-index/tests/fixtures/fidelity_lab/rust_fidelity_lab.rs"),
    },
];

fn materialize_fixtures() -> anyhow::Result<(tempfile::TempDir, Vec<PathBuf>)> {
    let dir = tempdir()?;
    let mut files = Vec::with_capacity(FIXTURES.len());
    for fixture in FIXTURES {
        let path = dir.path().join(fixture.path);
        std::fs::write(&path, fixture.source)?;
        files.push(path);
    }
    Ok((dir, files))
}

fn bench_graph_fidelity(c: &mut Criterion) {
    c.bench_function("graph_fidelity_fixture_suite", |b| {
        b.iter(|| {
            let (_dir, files_to_index) = materialize_fixtures().expect("fixture materialization");
            let root = files_to_index
                .first()
                .and_then(|path| path.parent())
                .expect("fixture root")
                .to_path_buf();

            let refresh_info = codestory_project::RefreshInfo {
                files_to_index,
                files_to_remove: Vec::new(),
            };
            let event_bus = EventBus::new();
            let mut storage = Storage::new_in_memory().expect("in-memory storage");
            let indexer = WorkspaceIndexer::new(root);
            indexer
                .run_incremental(&mut storage, &refresh_info, &event_bus, None)
                .expect("indexing");

            let edges = storage.get_edges().expect("edges");
            let call_edges = edges.iter().filter(|edge| edge.kind == EdgeKind::CALL).count();
            let uncertain_edges = edges
                .iter()
                .filter(|edge| {
                    edge.certainty
                        .or_else(|| ResolutionCertainty::from_confidence(edge.confidence))
                        .is_some_and(|certainty| matches!(certainty, ResolutionCertainty::Uncertain))
                })
                .count();

            black_box((edges.len(), call_edges, uncertain_edges));
        });
    });
}

criterion_group!(benches, bench_graph_fidelity);
criterion_main!(benches);
