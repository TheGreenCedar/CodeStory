use codestory_events::EventBus;
use codestory_index::WorkspaceIndexer;
use codestory_storage::Storage;
use criterion::{Criterion, criterion_group, criterion_main};

use codestory_bench::util;

fn bench_indexing_100_files(c: &mut Criterion) {
    let file_count = 100;
    let temp_dir = util::generate_synthetic_project(file_count).unwrap();
    let root = temp_dir.path().to_path_buf();
    let files = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "cpp"))
        .collect::<Vec<_>>();

    c.bench_function("index_100_files", |b| {
        b.iter(|| {
            let mut storage = Storage::new_in_memory().unwrap();
            let indexer = WorkspaceIndexer::new(root.clone());
            let event_bus = EventBus::new();

            let refresh_info = codestory_project::RefreshInfo {
                files_to_index: files.clone(),
                files_to_remove: vec![],
            };

            indexer
                .run_incremental(&mut storage, &refresh_info, &event_bus, None)
                .unwrap();
        })
    });
}

criterion_group!(benches, bench_indexing_100_files);
criterion_main!(benches);
