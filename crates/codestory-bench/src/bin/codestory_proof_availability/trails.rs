use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use codestory_agent::proof_qualification_support::diagnose_raw_call_edge;
use codestory_contracts::graph::{Edge, EdgeId, NodeId};
use codestory_store::Store;

use super::contracts::{TrailLengthCountsV1, TrailReportV1};
use super::inventory::stored_call_edges;

const MAX_TRAIL_LENGTH: usize = 6;

#[derive(Debug, Clone, Copy)]
struct TrailEdge {
    id: EdgeId,
    target: NodeId,
}

pub(crate) fn count_store_trails(repository_id: &str, store: &Store) -> Result<TrailReportV1> {
    let edges = stored_call_edges(store)?;
    ensure_unique_edge_ids(&edges)?;

    let effective = edges.iter().collect::<Vec<_>>();
    let exact_resolved = edges
        .iter()
        .filter(|edge| edge.resolved_target.is_some())
        .collect::<Vec<_>>();
    let strictly_admitted = edges
        .iter()
        .filter(|edge| {
            diagnose_raw_call_edge(edge, edge.effective_source(), edge.effective_target()).is_ok()
        })
        .collect::<Vec<_>>();

    let effective_counts = count_edge_distinct_trails(&effective)?;
    let exact_counts = count_edge_distinct_trails(&exact_resolved)?;
    let admitted_counts = count_edge_distinct_trails(&strictly_admitted)?;
    let lengths = (0..MAX_TRAIL_LENGTH)
        .map(|index| TrailLengthCountsV1 {
            length: u8::try_from(index + 1).expect("maximum trail length fits in u8"),
            effective_endpoint: effective_counts[index],
            exact_resolved: exact_counts[index],
            strictly_admitted: admitted_counts[index],
        })
        .collect();

    Ok(TrailReportV1 {
        repository_id: repository_id.to_string(),
        lengths,
    })
}

fn ensure_unique_edge_ids(edges: &[Edge]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for edge in edges {
        if !ids.insert(edge.id) {
            bail!("proof_availability_duplicate_edge_id");
        }
    }
    Ok(())
}

fn count_edge_distinct_trails(edges: &[&Edge]) -> Result<[u128; MAX_TRAIL_LENGTH]> {
    let mut sorted = edges.to_vec();
    sorted.sort_by_key(|edge| (edge.effective_source(), edge.id));
    let mut adjacency = BTreeMap::<NodeId, Vec<TrailEdge>>::new();
    for edge in sorted {
        adjacency
            .entry(edge.effective_source())
            .or_default()
            .push(TrailEdge {
                id: edge.id,
                target: edge.effective_target(),
            });
    }

    let mut counts = [0_u128; MAX_TRAIL_LENGTH];
    let mut used = [EdgeId(0); MAX_TRAIL_LENGTH];
    for edges in adjacency.values() {
        for edge in edges {
            used[0] = edge.id;
            counts[0] = checked_add_count(counts[0], 1)?;
            extend_trails(&adjacency, edge.target, 1, &mut used, &mut counts)?;
        }
    }
    Ok(counts)
}

fn extend_trails(
    adjacency: &BTreeMap<NodeId, Vec<TrailEdge>>,
    current: NodeId,
    used_len: usize,
    used: &mut [EdgeId; MAX_TRAIL_LENGTH],
    counts: &mut [u128; MAX_TRAIL_LENGTH],
) -> Result<()> {
    if used_len == MAX_TRAIL_LENGTH {
        return Ok(());
    }
    let Some(edges) = adjacency.get(&current) else {
        return Ok(());
    };
    for edge in edges {
        if used[..used_len].contains(&edge.id) {
            continue;
        }
        used[used_len] = edge.id;
        counts[used_len] = checked_add_count(counts[used_len], 1)?;
        extend_trails(adjacency, edge.target, used_len + 1, used, counts)?;
    }
    Ok(())
}

pub(crate) fn checked_add_count(current: u128, addend: u128) -> Result<u128> {
    current
        .checked_add(addend)
        .context("proof_availability_trail_count_overflow")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use anyhow::Result;
    use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, Node, NodeId, ResolutionCertainty};
    use codestory_store::Store;

    use super::{checked_add_count, count_store_trails};
    use crate::proof_availability::inventory::analyze_store;

    fn call(
        id: i64,
        source: i64,
        target: i64,
        certainty: ResolutionCertainty,
        resolved: bool,
    ) -> Edge {
        Edge {
            id: EdgeId(id),
            source: NodeId(source),
            target: NodeId(target),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(10)),
            line: Some(u32::try_from(id).expect("small fixture edge id")),
            resolved_source: None,
            resolved_target: resolved.then_some(NodeId(target)),
            confidence: Some(1.0),
            certainty: Some(certainty),
            callsite_identity: Some(format!("10:{id}:1:{target}")),
            candidate_targets: Vec::new(),
        }
    }

    fn store_with_edges(edges: &[Edge]) -> Result<Store> {
        let mut node_ids = BTreeSet::new();
        for edge in edges {
            node_ids.extend([edge.source, edge.target]);
            node_ids.extend(edge.resolved_source);
            node_ids.extend(edge.resolved_target);
            node_ids.extend(edge.file_node_id);
        }
        let nodes = node_ids
            .into_iter()
            .map(|id| Node {
                id,
                serialized_name: format!("node-{}", id.0),
                ..Node::default()
            })
            .collect::<Vec<_>>();
        let mut store = Store::new_in_memory()?;
        store.insert_nodes_batch(&nodes)?;
        store.insert_edges_batch(edges)?;
        Ok(store)
    }

    fn fixture_edges() -> Vec<Edge> {
        vec![
            call(1, 1, 2, ResolutionCertainty::Certain, true),
            call(2, 2, 1, ResolutionCertainty::Certain, true),
            call(3, 1, 1, ResolutionCertainty::Certain, true),
            call(4, 1, 2, ResolutionCertainty::Certain, true),
            call(5, 2, 3, ResolutionCertainty::Probable, true),
            call(6, 3, 4, ResolutionCertainty::Certain, false),
        ]
    }

    #[test]
    fn counts_edge_distinct_trails_with_repeated_vertices_self_edges_and_parallel_edges()
    -> Result<()> {
        let store = store_with_edges(&fixture_edges())?;
        let report = count_store_trails("synthetic", &store)?;
        let actual = report
            .lengths
            .iter()
            .map(|row| {
                (
                    row.effective_endpoint,
                    row.exact_resolved,
                    row.strictly_admitted,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            actual,
            vec![
                (6, 5, 4),
                (10, 9, 7),
                (14, 12, 8),
                (12, 8, 4),
                (8, 4, 0),
                (4, 0, 0),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_static_edge_cannot_be_reused_to_extend_a_trail() -> Result<()> {
        let store = store_with_edges(&[call(1, 1, 1, ResolutionCertainty::Certain, true)])?;
        let report = count_store_trails("single-self-edge", &store)?;
        assert_eq!(report.lengths[0].effective_endpoint, 1);
        assert!(
            report.lengths[1..]
                .iter()
                .all(|row| row.effective_endpoint == 0)
        );
        Ok(())
    }

    #[test]
    fn checked_trail_accumulation_rejects_u128_overflow() {
        let error = checked_add_count(u128::MAX, 1).expect_err("overflow must fail");
        assert_eq!(error.to_string(), "proof_availability_trail_count_overflow");
    }

    #[test]
    fn connected_trail_sql_matches_rust_for_all_three_edge_sets() -> Result<()> {
        let store = store_with_edges(&fixture_edges())?;
        let analysis = analyze_store("sql-parity", &store)?;
        let rust = count_store_trails("sql-parity", &store)?;
        let connection = store.get_connection();
        connection.execute_batch(
            "DROP TABLE IF EXISTS temp.proof_admitted_edge;
             CREATE TEMP TABLE proof_admitted_edge(edge_id INTEGER PRIMARY KEY);",
        )?;
        {
            let mut insert =
                connection.prepare("INSERT INTO proof_admitted_edge(edge_id) VALUES (?1)")?;
            for edge_id in &analysis.admitted_edge_ids {
                insert.execute([edge_id.0])?;
            }
        }
        let mut statement = connection.prepare(include_str!(
            "../../../../../benchmarks/proof-availability/sql/connected-trails.sql"
        ))?;
        let mut rows = statement.query([EdgeKind::CALL as i32])?;
        let mut sql = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let edge_set: String = row.get(0)?;
            let length: u8 = row.get(1)?;
            let count: i64 = row.get(2)?;
            sql.insert((edge_set, length), u128::try_from(count)?);
        }

        for row in rust.lengths {
            assert_eq!(
                sql[&("effective_endpoint".to_string(), row.length)],
                row.effective_endpoint
            );
            assert_eq!(
                sql[&("exact_resolved".to_string(), row.length)],
                row.exact_resolved
            );
            assert_eq!(
                sql[&("strictly_admitted".to_string(), row.length)],
                row.strictly_admitted
            );
        }
        Ok(())
    }
}
