use codestory_contracts::graph::{
    AccessKind, CallableProjectionState, Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind,
    Occurrence, OccurrenceKind, SourceLocation,
};
use codestory_store::{FileInfo, Store as Storage};
use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;

const FILE_COUNT: usize = 80;
const CALLERS_PER_FILE: usize = 24;
const CALLS_PER_CALLER: usize = 6;
const TOUCHED_CALLER_COUNT: usize = 8;

struct CleanupFixture {
    storage: Storage,
    touched_file_id: i64,
    touched_caller_ids: Vec<NodeId>,
    comparison_file_id: i64,
}

fn bench_incremental_cleanup(c: &mut Criterion) {
    c.bench_function("incremental_cleanup_touched_files", |b| {
        b.iter_batched(
            || build_cleanup_storage().expect("seed cleanup benchmark storage"),
            |fixture| {
                let mut storage = fixture.storage;
                let delta = storage
                    .delete_projection_for_callers(
                        fixture.touched_file_id,
                        &fixture.touched_caller_ids,
                    )
                    .expect("delta cleanup");
                let full = storage
                    .delete_file_projection(fixture.comparison_file_id)
                    .expect("full file cleanup");
                black_box((delta, full));
            },
            BatchSize::SmallInput,
        )
    });
}

fn build_cleanup_storage() -> anyhow::Result<CleanupFixture> {
    let mut storage = Storage::new_in_memory()?;
    let mut files = Vec::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut occurrences = Vec::new();
    let mut component_access = Vec::new();
    let mut callable_states = Vec::new();
    let mut touched_file_id = None;
    let mut touched_caller_ids = Vec::new();
    let mut comparison_file_id = None;

    let mut next_node_id = 1_i64;
    let mut next_edge_id = 1_i64;

    for file_idx in 0..FILE_COUNT {
        let file_id = next_node_id;
        next_node_id += 1;

        if file_idx == 0 {
            touched_file_id = Some(file_id);
        } else if file_idx == 1 {
            comparison_file_id = Some(file_id);
        }

        files.push(FileInfo {
            id: file_id,
            path: PathBuf::from(format!("/bench/file_{file_idx}.rs")),
            language: "rust".to_string(),
            modification_time: file_idx as i64,
            indexed: true,
            complete: true,
            line_count: 400,
        });
        nodes.push(Node {
            id: NodeId(file_id),
            kind: NodeKind::FILE,
            serialized_name: format!("/bench/file_{file_idx}.rs"),
            qualified_name: None,
            canonical_id: None,
            file_node_id: None,
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(400),
            end_col: Some(1),
        });

        for caller_idx in 0..CALLERS_PER_FILE {
            let caller_id = next_node_id;
            next_node_id += 1;
            let start_line = 10 + (caller_idx as u32 * 10);
            let end_line = start_line + 5;

            if file_idx == 0 && touched_caller_ids.len() < TOUCHED_CALLER_COUNT {
                touched_caller_ids.push(NodeId(caller_id));
            }

            nodes.push(Node {
                id: NodeId(caller_id),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("caller_{file_idx}_{caller_idx}"),
                qualified_name: Some(format!("bench::file_{file_idx}::caller_{caller_idx}")),
                canonical_id: None,
                file_node_id: Some(NodeId(file_id)),
                start_line: Some(start_line),
                start_col: Some(1),
                end_line: Some(end_line),
                end_col: Some(20),
            });
            callable_states.push(CallableProjectionState {
                file_id,
                symbol_key: format!("caller::{caller_idx}"),
                node_id: NodeId(caller_id),
                signature_hash: caller_idx as i64,
                body_hash: (file_idx * 10 + caller_idx) as i64,
                start_line,
                end_line,
            });
            component_access.push((NodeId(caller_id), AccessKind::Public));
            occurrences.push(Occurrence {
                element_id: caller_id,
                kind: OccurrenceKind::DEFINITION,
                location: SourceLocation {
                    file_node_id: NodeId(file_id),
                    start_line,
                    start_col: 1,
                    end_line,
                    end_col: 20,
                },
            });

            for call_idx in 0..CALLS_PER_CALLER {
                let target_id = next_node_id;
                next_node_id += 1;
                nodes.push(Node {
                    id: NodeId(target_id),
                    kind: NodeKind::UNKNOWN,
                    serialized_name: format!("target_{call_idx}"),
                    qualified_name: None,
                    canonical_id: None,
                    file_node_id: Some(NodeId(file_id)),
                    start_line: Some(start_line + call_idx as u32),
                    start_col: Some(4),
                    end_line: Some(start_line + call_idx as u32),
                    end_col: Some(14),
                });
                edges.push(Edge {
                    id: EdgeId(next_edge_id),
                    source: NodeId(caller_id),
                    target: NodeId(target_id),
                    kind: EdgeKind::CALL,
                    file_node_id: Some(NodeId(file_id)),
                    line: Some(start_line + call_idx as u32),
                    resolved_source: Some(NodeId(caller_id)),
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: Some(format!("{file_idx}:{caller_idx}:{call_idx}")),
                    candidate_targets: Vec::new(),
                });
                next_edge_id += 1;
                occurrences.push(Occurrence {
                    element_id: caller_id,
                    kind: OccurrenceKind::REFERENCE,
                    location: SourceLocation {
                        file_node_id: NodeId(file_id),
                        start_line: start_line + call_idx as u32,
                        start_col: 4,
                        end_line: start_line + call_idx as u32,
                        end_col: 14,
                    },
                });
            }
        }
    }

    storage.insert_files_batch(&files)?;
    storage.insert_nodes_batch(&nodes)?;
    storage.insert_edges_batch(&edges)?;
    storage.insert_occurrences_batch(&occurrences)?;
    storage.insert_component_access_batch(&component_access)?;
    storage.upsert_callable_projection_states(&callable_states)?;
    Ok(CleanupFixture {
        storage,
        touched_file_id: touched_file_id.expect("first file fixture ID"),
        touched_caller_ids,
        comparison_file_id: comparison_file_id.expect("second file fixture ID"),
    })
}

criterion_group!(benches, bench_incremental_cleanup);
criterion_main!(benches);
