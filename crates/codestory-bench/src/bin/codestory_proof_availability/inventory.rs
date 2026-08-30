use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use codestory_agent::proof_qualification_support::{RawAdmissionFailure, diagnose_raw_call_edge};
use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, ResolutionCertainty};
use codestory_store::Store;

use super::contracts::InventoryReportV1;

const ALL_ADMISSION_FAILURES: [RawAdmissionFailure; 11] = [
    RawAdmissionFailure::WrongKind,
    RawAdmissionFailure::WrongEffectiveSource,
    RawAdmissionFailure::WrongEffectiveTarget,
    RawAdmissionFailure::MissingExactResolvedTarget,
    RawAdmissionFailure::CandidateAlternativesRetained,
    RawAdmissionFailure::MissingFileNode,
    RawAdmissionFailure::MissingLine,
    RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity,
    RawAdmissionFailure::CallsiteFileMismatch,
    RawAdmissionFailure::CallsiteLineMismatch,
    RawAdmissionFailure::CallsiteRawTargetMismatch,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CertaintyBucket {
    Absent,
    Certain,
    Probable,
    Uncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ResolutionBucket {
    ExactResolved,
    UnresolvedPlaceholder,
}

#[derive(Debug)]
pub(crate) struct InventoryAnalysis {
    pub(crate) report: InventoryReportV1,
    pub(crate) certainty: BTreeMap<CertaintyBucket, u128>,
    pub(crate) resolution: BTreeMap<ResolutionBucket, u128>,
    pub(crate) rejections: BTreeMap<RawAdmissionFailure, u128>,
    pub(crate) admitted_edge_ids: Vec<EdgeId>,
}

pub(crate) fn stored_call_edges(store: &Store) -> Result<Vec<Edge>> {
    let mut edges = store
        .get_edges()
        .context("read proof availability graph inventory")?
        .into_iter()
        .filter(|edge| edge.kind == EdgeKind::CALL)
        .collect::<Vec<_>>();
    edges.sort_by_key(|edge| edge.id);
    Ok(edges)
}

pub(crate) fn analyze_store(repository_id: &str, store: &Store) -> Result<InventoryAnalysis> {
    let edges = stored_call_edges(store)?;
    let mut certainty = BTreeMap::from([
        (CertaintyBucket::Absent, 0),
        (CertaintyBucket::Certain, 0),
        (CertaintyBucket::Probable, 0),
        (CertaintyBucket::Uncertain, 0),
    ]);
    let mut resolution = BTreeMap::from([
        (ResolutionBucket::ExactResolved, 0),
        (ResolutionBucket::UnresolvedPlaceholder, 0),
    ]);
    let mut rejections = ALL_ADMISSION_FAILURES
        .into_iter()
        .map(|failure| (failure, 0))
        .collect::<BTreeMap<_, _>>();
    let mut stored_call_rows = 0_u128;
    let mut effective_endpoint_rows = 0_u128;
    let mut exact_resolved_rows = 0_u128;
    let mut unresolved_placeholder_rows = 0_u128;
    let mut admitted_edge_ids = Vec::new();

    for edge in &edges {
        checked_increment(&mut stored_call_rows)?;
        checked_increment(&mut effective_endpoint_rows)?;
        let certainty_bucket = match edge.certainty {
            None => CertaintyBucket::Absent,
            Some(ResolutionCertainty::Certain) => CertaintyBucket::Certain,
            Some(ResolutionCertainty::Probable) => CertaintyBucket::Probable,
            Some(ResolutionCertainty::Uncertain) => CertaintyBucket::Uncertain,
        };
        checked_increment(
            certainty
                .get_mut(&certainty_bucket)
                .expect("all certainty buckets are initialized"),
        )?;
        let resolution_bucket = if edge.resolved_target.is_some() {
            checked_increment(&mut exact_resolved_rows)?;
            ResolutionBucket::ExactResolved
        } else {
            checked_increment(&mut unresolved_placeholder_rows)?;
            ResolutionBucket::UnresolvedPlaceholder
        };
        checked_increment(
            resolution
                .get_mut(&resolution_bucket)
                .expect("all resolution buckets are initialized"),
        )?;

        match diagnose_raw_call_edge(edge, edge.effective_source(), edge.effective_target()) {
            Ok(admitted) => admitted_edge_ids.push(admitted.edge_id),
            Err(reason) => checked_increment(
                rejections
                    .get_mut(&reason)
                    .expect("all admission failures are initialized"),
            )?,
        }
    }

    let admitted_rows = u128::try_from(admitted_edge_ids.len())
        .context("proof_availability_inventory_count_overflow")?;
    let certainty_total = checked_total(certainty.values().copied())?;
    let resolution_total = checked_total(resolution.values().copied())?;
    let rejection_total = checked_total(rejections.values().copied())?;
    let admission_total = rejection_total
        .checked_add(admitted_rows)
        .context("proof_availability_inventory_count_overflow")?;
    if effective_endpoint_rows != stored_call_rows
        || certainty_total != stored_call_rows
        || resolution_total != stored_call_rows
        || admission_total != stored_call_rows
        || exact_resolved_rows
            .checked_add(unresolved_placeholder_rows)
            .context("proof_availability_inventory_count_overflow")?
            != stored_call_rows
    {
        bail!("proof_availability_inventory_reconciliation_failed");
    }

    Ok(InventoryAnalysis {
        report: InventoryReportV1 {
            repository_id: repository_id.to_string(),
            stored_call_rows,
            effective_endpoint_rows,
            exact_resolved_rows,
            admitted_rows,
            unresolved_placeholder_rows,
        },
        certainty,
        resolution,
        rejections,
        admitted_edge_ids,
    })
}

fn checked_increment(value: &mut u128) -> Result<()> {
    *value = value
        .checked_add(1)
        .context("proof_availability_inventory_count_overflow")?;
    Ok(())
}

fn checked_total(values: impl IntoIterator<Item = u128>) -> Result<u128> {
    values.into_iter().try_fold(0_u128, |total, value| {
        total
            .checked_add(value)
            .context("proof_availability_inventory_count_overflow")
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use anyhow::Result;
    use codestory_agent::proof_qualification_support::RawAdmissionFailure;
    use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, Node, NodeId, ResolutionCertainty};
    use codestory_store::Store;

    use super::{CertaintyBucket, ResolutionBucket, analyze_store, checked_total};

    fn lawful_call(id: i64, source: i64, raw_target: i64, target: i64) -> Edge {
        Edge {
            id: EdgeId(id),
            source: NodeId(source),
            target: NodeId(raw_target),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(10)),
            line: Some(7),
            resolved_source: None,
            resolved_target: Some(NodeId(target)),
            confidence: Some(1.0),
            certainty: Some(ResolutionCertainty::Certain),
            callsite_identity: Some(format!("10:7:1:{raw_target}")),
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
            node_ids.extend(edge.candidate_targets.iter().copied());
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

    fn rejection_count(
        counts: &BTreeMap<RawAdmissionFailure, u128>,
        reason: RawAdmissionFailure,
    ) -> u128 {
        counts.get(&reason).copied().unwrap_or_default()
    }

    #[test]
    fn inventory_reconciles_stored_call_rows_through_the_actual_admission_leaf() -> Result<()> {
        let mut edges = Vec::new();

        let mut absent = lawful_call(1, 1, 2, 3);
        absent.certainty = None;
        edges.push(absent);
        let mut probable = lawful_call(2, 1, 2, 3);
        probable.certainty = Some(ResolutionCertainty::Probable);
        edges.push(probable);
        let mut uncertain = lawful_call(3, 1, 2, 3);
        uncertain.certainty = Some(ResolutionCertainty::Uncertain);
        edges.push(uncertain);
        let mut unresolved = lawful_call(4, 1, 2, 3);
        unresolved.resolved_target = None;
        edges.push(unresolved);
        let mut candidates = lawful_call(5, 1, 2, 3);
        candidates.candidate_targets = vec![NodeId(4)];
        edges.push(candidates);
        let mut no_file = lawful_call(6, 1, 2, 3);
        no_file.file_node_id = None;
        edges.push(no_file);
        let mut no_line = lawful_call(7, 1, 2, 3);
        no_line.line = None;
        edges.push(no_line);
        let mut legacy_identity = lawful_call(8, 1, 2, 3);
        legacy_identity.callsite_identity = Some("|marker-only".to_string());
        edges.push(legacy_identity);
        let mut wrong_file = lawful_call(9, 1, 2, 3);
        wrong_file.callsite_identity = Some("11:7:1:2".to_string());
        edges.push(wrong_file);
        let mut wrong_line = lawful_call(10, 1, 2, 3);
        wrong_line.callsite_identity = Some("10:8:1:2".to_string());
        edges.push(wrong_line);
        let mut wrong_raw_target = lawful_call(11, 1, 2, 3);
        wrong_raw_target.callsite_identity = Some("10:7:1:99".to_string());
        edges.push(wrong_raw_target);
        edges.push(lawful_call(12, 1, 2, 3));
        edges.push(Edge {
            id: EdgeId(99),
            source: NodeId(1),
            target: NodeId(2),
            kind: EdgeKind::USAGE,
            ..Edge::default()
        });

        let store = store_with_edges(&edges)?;
        let analysis = analyze_store("synthetic", &store)?;

        assert_eq!(analysis.report.stored_call_rows, 12);
        assert_eq!(analysis.report.effective_endpoint_rows, 12);
        assert_eq!(analysis.report.exact_resolved_rows, 11);
        assert_eq!(analysis.report.admitted_rows, 4);
        assert_eq!(analysis.report.unresolved_placeholder_rows, 1);
        assert_eq!(analysis.certainty[&CertaintyBucket::Absent], 1);
        assert_eq!(analysis.certainty[&CertaintyBucket::Certain], 9);
        assert_eq!(analysis.certainty[&CertaintyBucket::Probable], 1);
        assert_eq!(analysis.certainty[&CertaintyBucket::Uncertain], 1);
        assert_eq!(analysis.resolution[&ResolutionBucket::ExactResolved], 11);
        assert_eq!(
            analysis.resolution[&ResolutionBucket::UnresolvedPlaceholder],
            1
        );
        for reason in [
            RawAdmissionFailure::MissingExactResolvedTarget,
            RawAdmissionFailure::CandidateAlternativesRetained,
            RawAdmissionFailure::MissingFileNode,
            RawAdmissionFailure::MissingLine,
            RawAdmissionFailure::InvalidOrLegacyCallsiteIdentity,
            RawAdmissionFailure::CallsiteFileMismatch,
            RawAdmissionFailure::CallsiteLineMismatch,
            RawAdmissionFailure::CallsiteRawTargetMismatch,
        ] {
            assert_eq!(
                rejection_count(&analysis.rejections, reason),
                1,
                "{reason:?}"
            );
        }
        for impossible in [
            RawAdmissionFailure::WrongKind,
            RawAdmissionFailure::WrongEffectiveSource,
            RawAdmissionFailure::WrongEffectiveTarget,
        ] {
            assert_eq!(rejection_count(&analysis.rejections, impossible), 0);
        }
        assert_eq!(analysis.rejections.len(), 11);
        assert_eq!(
            analysis.admitted_edge_ids,
            vec![EdgeId(1), EdgeId(2), EdgeId(3), EdgeId(12)]
        );
        Ok(())
    }

    #[test]
    fn checked_inventory_reconciliation_rejects_u128_overflow() {
        let error = checked_total([u128::MAX, 1]).expect_err("overflow must fail");
        assert_eq!(
            error.to_string(),
            "proof_availability_inventory_count_overflow"
        );
    }

    #[test]
    fn raw_inventory_sql_matches_the_rust_store_and_kernel_counts() -> Result<()> {
        let edges = vec![
            lawful_call(1, 1, 2, 3),
            {
                let mut edge = lawful_call(2, 3, 4, 5);
                edge.resolved_target = None;
                edge
            },
            {
                let mut edge = lawful_call(3, 5, 6, 7);
                edge.certainty = Some(ResolutionCertainty::Probable);
                edge
            },
        ];
        let store = store_with_edges(&edges)?;
        let analysis = analyze_store("sql-parity", &store)?;
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
            "../../../../../benchmarks/proof-availability/sql/raw-call-inventory.sql"
        ))?;
        let mut rows = statement.query([EdgeKind::CALL as i32])?;
        let mut sql = BTreeMap::new();
        while let Some(row) = rows.next()? {
            let metric: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            sql.insert(metric, u128::try_from(count)?);
        }

        assert_eq!(sql["stored_call_rows"], analysis.report.stored_call_rows);
        assert_eq!(
            sql["effective_endpoint_rows"],
            analysis.report.effective_endpoint_rows
        );
        assert_eq!(
            sql["exact_resolved_rows"],
            analysis.report.exact_resolved_rows
        );
        assert_eq!(sql["admitted_rows"], analysis.report.admitted_rows);
        assert_eq!(
            sql["unresolved_placeholder_rows"],
            analysis.report.unresolved_placeholder_rows
        );
        Ok(())
    }
}
