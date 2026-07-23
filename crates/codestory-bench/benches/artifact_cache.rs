use codestory_store::{IndexArtifactCacheWrite, Store};
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

const CHUNK_SIZE: usize = 24;
const ARTIFACT_BYTES: usize = 64 * 1024;

struct ArtifactCacheFixture {
    _temp: TempDir,
    store: Store,
    entries: Vec<(PathBuf, String, Vec<u8>)>,
}

fn artifact_cache_chunk_persistence(c: &mut Criterion) {
    let mut group = c.benchmark_group("artifact_cache_chunk_persistence");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(8));
    group.throughput(Throughput::Elements(CHUNK_SIZE as u64));

    group.bench_function("single_row_autocommits", |b| {
        b.iter_batched(
            build_fixture,
            |fixture| {
                for (path, cache_key, artifact_blob) in &fixture.entries {
                    fixture
                        .store
                        .upsert_index_artifact_cache(path, cache_key, artifact_blob)
                        .expect("single artifact-cache upsert");
                }
                black_box(fixture.store);
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("one_transaction_per_chunk", |b| {
        b.iter_batched(
            build_fixture,
            |fixture| {
                let writes = fixture
                    .entries
                    .iter()
                    .map(|(path, cache_key, artifact_blob)| IndexArtifactCacheWrite {
                        path,
                        cache_key,
                        artifact_blob,
                    })
                    .collect::<Vec<_>>();
                let written = fixture
                    .store
                    .upsert_index_artifact_cache_batch(&writes)
                    .expect("artifact-cache batch upsert");
                black_box((written, fixture.store));
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn build_fixture() -> ArtifactCacheFixture {
    let temp = tempfile::tempdir().expect("artifact-cache benchmark tempdir");
    let store = Store::open_build(&temp.path().join("codestory.db"))
        .expect("artifact-cache benchmark build store");
    let entries = (0..CHUNK_SIZE)
        .map(|index| {
            (
                PathBuf::from(format!("src/module_{index}.rs")),
                format!("v2:benchmark-{index}"),
                vec![index as u8; ARTIFACT_BYTES],
            )
        })
        .collect();
    ArtifactCacheFixture {
        _temp: temp,
        store,
        entries,
    }
}

criterion_group!(benches, artifact_cache_chunk_persistence);
criterion_main!(benches);
