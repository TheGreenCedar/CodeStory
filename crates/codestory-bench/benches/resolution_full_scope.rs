use codestory_core::{Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind};
use codestory_index::resolution::ResolutionPass;
use codestory_storage::Storage;
use criterion::{Criterion, criterion_group, criterion_main};

const FILE_COUNT: usize = 140;
const CALLERS_PER_FILE: usize = 22;
const CALL_EDGES_PER_CALLER: usize = 8;
const IMPORT_EDGES_PER_FILE: usize = 12;

const CALL_TARGETS: &[&str] = &[
    "run", "save", "persist", "notify", "track", "compute", "helper", "build",
];
const IMPORT_TARGETS: &[&str] = &[
    "pkg::util",
    "pkg::storage::Repository",
    "pkg::network as Net",
    "pkg::core::{Engine, Runner}",
    "pkg::platform.*",
];

fn bench_resolution_full_scope(c: &mut Criterion) {
    let mut storage = build_benchmark_storage().expect("failed to seed benchmark storage");
    let resolver = ResolutionPass::new();

    c.bench_function("resolution_full_scope", |b| {
        b.iter(|| {
            reset_resolution_columns(&storage).expect("failed to reset resolution");
            resolver.run(&mut storage).expect("resolution pass failed");
        })
    });
}

fn reset_resolution_columns(storage: &Storage) -> anyhow::Result<()> {
    storage.get_connection().execute(
        "UPDATE edge
         SET resolved_source_node_id = NULL,
             resolved_target_node_id = NULL,
             confidence = NULL,
             certainty = NULL,
             candidate_target_node_ids = NULL
         WHERE kind = ?1 OR kind = ?2",
        [EdgeKind::CALL as i32, EdgeKind::IMPORT as i32],
    )?;
    Ok(())
}

fn build_benchmark_storage() -> anyhow::Result<Storage> {
    let mut storage = Storage::new_in_memory()?;
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    let mut node_id = 1_i64;
    let mut edge_id = 1_i64;

    let mut file_node_ids = Vec::with_capacity(FILE_COUNT);
    for file_idx in 0..FILE_COUNT {
        let file_id = node_id;
        node_id += 1;
        file_node_ids.push(file_id);
        nodes.push(Node {
            id: NodeId(file_id),
            kind: NodeKind::FILE,
            serialized_name: format!("/bench/file_{file_idx}.rs"),
            qualified_name: None,
            canonical_id: None,
            file_node_id: None,
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(1),
            end_col: Some(1),
        });
    }

    for (file_offset, file_node_id) in file_node_ids.iter().enumerate() {
        for (target_offset, target) in CALL_TARGETS.iter().enumerate() {
            nodes.push(Node {
                id: NodeId(node_id),
                kind: NodeKind::FUNCTION,
                serialized_name: (*target).to_string(),
                qualified_name: Some(format!("pkg{file_offset}::{}", target)),
                canonical_id: None,
                file_node_id: Some(NodeId(*file_node_id)),
                start_line: Some((target_offset + 1) as u32),
                start_col: Some(1),
                end_line: Some((target_offset + 1) as u32),
                end_col: Some(10),
            });
            node_id += 1;
        }
    }

    for module_name in [
        "pkg::util",
        "pkg::storage::Repository",
        "pkg::network",
        "pkg::core::Engine",
        "pkg::core::Runner",
        "pkg::platform",
    ] {
        nodes.push(Node {
            id: NodeId(node_id),
            kind: NodeKind::MODULE,
            serialized_name: module_name.to_string(),
            qualified_name: Some(module_name.to_string()),
            canonical_id: None,
            file_node_id: None,
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(1),
            end_col: Some(1),
        });
        node_id += 1;
    }

    for (file_offset, file_node_id) in file_node_ids.iter().enumerate() {
        let mut caller_ids = Vec::with_capacity(CALLERS_PER_FILE);
        for caller_idx in 0..CALLERS_PER_FILE {
            let caller_id = node_id;
            nodes.push(Node {
                id: NodeId(caller_id),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("caller_{caller_idx}"),
                qualified_name: Some(format!("pkg{file_offset}::caller_{caller_idx}")),
                canonical_id: None,
                file_node_id: Some(NodeId(*file_node_id)),
                start_line: Some((50 + caller_idx) as u32),
                start_col: Some(1),
                end_line: Some((50 + caller_idx) as u32),
                end_col: Some(20),
            });
            node_id += 1;
            caller_ids.push(caller_id);
        }

        for (caller_idx, caller_id) in caller_ids.iter().enumerate() {
            for edge_offset in 0..CALL_EDGES_PER_CALLER {
                let unresolved_target_name =
                    CALL_TARGETS[(caller_idx + edge_offset) % CALL_TARGETS.len()].to_string();
                let target_id = node_id;
                nodes.push(Node {
                    id: NodeId(target_id),
                    kind: NodeKind::UNKNOWN,
                    serialized_name: unresolved_target_name,
                    qualified_name: None,
                    canonical_id: None,
                    file_node_id: Some(NodeId(*file_node_id)),
                    start_line: Some((100 + edge_offset) as u32),
                    start_col: Some(1),
                    end_line: Some((100 + edge_offset) as u32),
                    end_col: Some(8),
                });
                node_id += 1;

                edges.push(Edge {
                    id: EdgeId(edge_id),
                    source: NodeId(*caller_id),
                    target: NodeId(target_id),
                    kind: EdgeKind::CALL,
                    file_node_id: Some(NodeId(*file_node_id)),
                    line: Some((100 + edge_offset) as u32),
                    resolved_source: None,
                    resolved_target: None,
                    confidence: None,
                    certainty: None,
                    callsite_identity: Some(format!("{file_offset}:{caller_idx}:{edge_offset}")),
                    candidate_targets: Vec::new(),
                });
                edge_id += 1;
            }
        }

        let import_source = caller_ids[0];
        for import_idx in 0..IMPORT_EDGES_PER_FILE {
            let import_name = IMPORT_TARGETS[import_idx % IMPORT_TARGETS.len()].to_string();
            let target_id = node_id;
            nodes.push(Node {
                id: NodeId(target_id),
                kind: NodeKind::UNKNOWN,
                serialized_name: import_name,
                qualified_name: None,
                canonical_id: None,
                file_node_id: Some(NodeId(*file_node_id)),
                start_line: Some((200 + import_idx) as u32),
                start_col: Some(1),
                end_line: Some((200 + import_idx) as u32),
                end_col: Some(12),
            });
            node_id += 1;

            edges.push(Edge {
                id: EdgeId(edge_id),
                source: NodeId(import_source),
                target: NodeId(target_id),
                kind: EdgeKind::IMPORT,
                file_node_id: Some(NodeId(*file_node_id)),
                line: Some((200 + import_idx) as u32),
                resolved_source: None,
                resolved_target: None,
                confidence: None,
                certainty: None,
                callsite_identity: None,
                candidate_targets: Vec::new(),
            });
            edge_id += 1;
        }
    }

    storage.insert_nodes_batch(&nodes)?;
    storage.insert_edges_batch(&edges)?;
    Ok(storage)
}

criterion_group!(benches, bench_resolution_full_scope);
criterion_main!(benches);
