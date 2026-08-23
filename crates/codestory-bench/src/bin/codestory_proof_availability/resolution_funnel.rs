use super::contracts::{
    ActualProductResultV1, CaseReportV1, ProjectMaterializationEvidenceV1,
    ResolutionAdapterReportV1, ResolutionFunnelCountsV1, ResolutionFunnelReportV1,
    ResolutionFunnelRowV1, ResolutionProjectionReceiptV1, StepQualificationOutcomeV1,
    resolution_funnel_conversions,
};
use anyhow::{Context, Result, bail};
use codestory_contracts::proof_resolution::{CallResolutionFact, ProofResolutionFunnelCounts};
use codestory_store::Store;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
struct ResolutionFactDimension {
    repository_id: String,
    fact_id: String,
    edge_id: i64,
    language: String,
    callee_form: Option<String>,
    primary_evidence_kind: Option<String>,
}

fn report_counts(counts: &ProofResolutionFunnelCounts) -> ResolutionFunnelCountsV1 {
    ResolutionFunnelCountsV1 {
        syntax_calls: counts.syntax_calls,
        adapter_supported: counts.adapter_supported,
        exact: counts.exact,
        ambiguous: counts.ambiguous,
        missing_binding: counts.missing_binding,
        incomplete_domain: counts.incomplete_domain,
        unsupported: counts.unsupported,
        exact_call_linked: counts.exact_call_linked,
        proof_shape_admitted: 0,
        authoritative_receipts: 0,
        complete_proofs: 0,
    }
}

fn dimension_for_fact(
    repository_id: &str,
    fact: &CallResolutionFact,
) -> Option<ResolutionFactDimension> {
    Some(ResolutionFactDimension {
        repository_id: repository_id.to_owned(),
        fact_id: fact.fact_id.clone(),
        edge_id: fact.edge_id?.0,
        language: fact.provenance.language_adapter.clone(),
        callee_form: Some(fact.callsite.callee_form.as_str().to_owned()),
        primary_evidence_kind: fact
            .evidence_chain
            .first()
            .map(|evidence| evidence.kind().as_str().to_owned()),
    })
}

fn apply_qualification_stages(
    rows: &mut [ResolutionFunnelRowV1],
    facts: &[ResolutionFactDimension],
    admitted_edges: &BTreeSet<(String, i64)>,
    authoritative_edges: &BTreeSet<(String, i64)>,
    complete_edges: &BTreeSet<(String, i64)>,
) -> Result<()> {
    if !authoritative_edges.is_subset(admitted_edges)
        || !complete_edges.is_subset(authoritative_edges)
    {
        bail!("proof_availability_resolution_funnel_stages_not_nested")
    }
    let mut fact_by_edge = BTreeMap::new();
    for fact in facts {
        let key = (fact.repository_id.clone(), fact.edge_id);
        if fact_by_edge.insert(key, fact).is_some() {
            bail!("proof_availability_resolution_funnel_edge_not_unique")
        }
    }
    for edge in admitted_edges
        .iter()
        .chain(authoritative_edges)
        .chain(complete_edges)
    {
        if !fact_by_edge.contains_key(edge) {
            bail!("proof_availability_resolution_funnel_fact_missing")
        }
    }
    for row in rows.iter_mut() {
        row.counts.proof_shape_admitted = 0;
        row.counts.authoritative_receipts = 0;
        row.counts.complete_proofs = 0;
    }
    let row_index = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            (
                (
                    row.repository_id.clone(),
                    row.language.clone(),
                    row.callee_form.clone(),
                    row.primary_evidence_kind.clone(),
                ),
                index,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let increment = |rows: &mut [ResolutionFunnelRowV1],
                     edges: &BTreeSet<(String, i64)>,
                     stage: fn(&mut ResolutionFunnelCountsV1) -> &mut u64|
     -> Result<()> {
        let mut seen_facts = BTreeSet::new();
        for edge in edges {
            let fact = fact_by_edge[edge];
            if !seen_facts.insert((fact.repository_id.as_str(), fact.fact_id.as_str())) {
                continue;
            }
            let key = (
                fact.repository_id.clone(),
                fact.language.clone(),
                fact.callee_form.clone(),
                fact.primary_evidence_kind.clone(),
            );
            let row = row_index
                .get(&key)
                .and_then(|index| rows.get_mut(*index))
                .ok_or_else(|| {
                    anyhow::anyhow!("proof_availability_resolution_funnel_row_missing")
                })?;
            let count = stage(&mut row.counts);
            *count = count
                .checked_add(1)
                .context("proof_availability_resolution_funnel_count_overflow")?;
        }
        Ok(())
    };
    increment(rows, admitted_edges, |counts| {
        &mut counts.proof_shape_admitted
    })?;
    increment(rows, authoritative_edges, |counts| {
        &mut counts.authoritative_receipts
    })?;
    increment(rows, complete_edges, |counts| &mut counts.complete_proofs)?;
    for row in rows {
        row.conversions = resolution_funnel_conversions(&row.counts)?;
    }
    Ok(())
}

pub(crate) fn build_repository_resolution_funnel(
    repository_id: &str,
    expected: &ProjectMaterializationEvidenceV1,
    store: &Store,
    cases: &[CaseReportV1],
) -> Result<ResolutionFunnelReportV1> {
    if expected.repository_id != repository_id
        || cases.iter().any(|case| case.repository_id != repository_id)
    {
        bail!("proof_availability_resolution_funnel_repository_mismatch")
    }
    let core = store
        .get_complete_index_publication()?
        .ok_or_else(|| anyhow::anyhow!("proof_availability_core_publication_missing"))?;
    if core.generation != expected.core_generation
        || core.generation_id != expected.identity.core_generation_id
        || core.run_id != expected.identity.core_run_id
    {
        bail!("proof_availability_resolution_funnel_core_mismatch")
    }
    let manifest = store
        .validate_proof_resolution_publication(&core)
        .context("validate proof resolution publication for qualification funnel")?;
    let facts = store.get_proof_resolution_facts()?;
    let dimensions = facts
        .iter()
        .filter_map(|fact| dimension_for_fact(repository_id, fact))
        .collect::<Vec<_>>();
    let mut rows = manifest
        .funnel
        .iter()
        .map(|row| {
            let counts = report_counts(&row.counts);
            Ok(ResolutionFunnelRowV1 {
                repository_id: repository_id.to_owned(),
                language: row.language.clone(),
                callee_form: row.callee_form.map(|form| form.as_str().to_owned()),
                primary_evidence_kind: row.evidence_kind.map(|kind| kind.as_str().to_owned()),
                conversions: resolution_funnel_conversions(&counts)?,
                counts,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut admitted = BTreeSet::new();
    let mut authoritative = BTreeSet::new();
    let mut complete = BTreeSet::new();
    for case in cases {
        for step in &case.proof_trace.steps {
            if let StepQualificationOutcomeV1::Admitted { edge_ids } = &step.outcome {
                admitted.extend(
                    edge_ids
                        .iter()
                        .map(|edge_id| (repository_id.to_owned(), *edge_id)),
                );
            }
        }
        let case_authoritative = case
            .product_disposition
            .authoritative_receipts
            .iter()
            .map(|receipt| (repository_id.to_owned(), receipt.edge_id))
            .collect::<BTreeSet<_>>();
        authoritative.extend(case_authoritative.iter().cloned());
        if matches!(
            case.product_disposition.actual,
            ActualProductResultV1::ContractProven { .. }
        ) && case.evaluable_facts()?.contract_proven_supported
        {
            complete.extend(case_authoritative);
        }
    }
    apply_qualification_stages(&mut rows, &dimensions, &admitted, &authoritative, &complete)?;
    Ok(ResolutionFunnelReportV1 {
        projections: vec![ResolutionProjectionReceiptV1 {
            repository_id: repository_id.to_owned(),
            core_generation_id: manifest.core_generation_id,
            core_run_id: manifest.core_run_id,
            core_published_at_epoch_ms: manifest.published_at_epoch_ms,
            fact_schema_version: manifest.fact_schema_version,
            fact_count: manifest.fact_count,
            fact_digest: manifest.fact_digest,
            adapter_roster: manifest
                .adapter_roster
                .into_iter()
                .map(|adapter| ResolutionAdapterReportV1 {
                    language: adapter.language,
                    adapter_version: adapter.adapter_version,
                })
                .collect(),
        }],
        rows,
    })
}

#[cfg(test)]
mod tests {
    use super::super::contracts::{
        EnvironmentIdentityV1, MaterializationFreshnessV1, ResolutionFunnelConversionsV1,
        ResolutionFunnelCountsV1, ResolutionFunnelRowV1,
    };
    use super::*;
    use codestory_contracts::api::IndexMode;
    use std::collections::BTreeSet;
    use std::fs;

    fn row(
        repository_id: &str,
        language: &str,
        callee_form: &str,
        evidence: &str,
    ) -> ResolutionFunnelRowV1 {
        ResolutionFunnelRowV1 {
            repository_id: repository_id.into(),
            language: language.into(),
            callee_form: Some(callee_form.into()),
            primary_evidence_kind: Some(evidence.into()),
            counts: ResolutionFunnelCountsV1 {
                syntax_calls: 1,
                adapter_supported: 1,
                exact: 1,
                exact_call_linked: 1,
                ..ResolutionFunnelCountsV1::default()
            },
            conversions: ResolutionFunnelConversionsV1::default(),
        }
    }

    #[test]
    fn qualification_stages_count_each_fact_once_in_its_primary_dimension() {
        let mut rows = vec![
            row(
                "typescript-repository",
                "typescript",
                "named_import",
                "static_import_binding",
            ),
            row(
                "rust-repository",
                "rust",
                "implicit_receiver",
                "implicit_receiver",
            ),
        ];
        let facts = vec![
            ResolutionFactDimension {
                repository_id: "typescript-repository".into(),
                fact_id: "typescript-fact".into(),
                edge_id: 11,
                language: "typescript".into(),
                callee_form: Some("named_import".into()),
                primary_evidence_kind: Some("static_import_binding".into()),
            },
            ResolutionFactDimension {
                repository_id: "rust-repository".into(),
                fact_id: "rust-fact".into(),
                edge_id: 22,
                language: "rust".into(),
                callee_form: Some("implicit_receiver".into()),
                primary_evidence_kind: Some("implicit_receiver".into()),
            },
        ];
        let admitted = BTreeSet::from([
            ("typescript-repository".into(), 11),
            ("rust-repository".into(), 22),
        ]);
        let authoritative = BTreeSet::from([("typescript-repository".into(), 11)]);
        let complete = BTreeSet::from([("typescript-repository".into(), 11)]);

        apply_qualification_stages(&mut rows, &facts, &admitted, &authoritative, &complete)
            .expect("qualified funnel");
        assert_eq!(rows[0].counts.proof_shape_admitted, 1);
        assert_eq!(rows[0].counts.authoritative_receipts, 1);
        assert_eq!(rows[0].counts.complete_proofs, 1);
        assert_eq!(rows[1].counts.proof_shape_admitted, 1);
        assert_eq!(rows[1].counts.authoritative_receipts, 0);
        assert_eq!(rows[1].counts.complete_proofs, 0);
        assert_eq!(
            rows[0]
                .conversions
                .complete_proofs_per_authoritative_receipts_milli,
            1000
        );
    }

    #[test]
    fn qualification_stages_reject_non_nested_or_unpublished_edge_sets() {
        let mut rows = vec![row(
            "typescript-repository",
            "typescript",
            "identifier",
            "same_file_declaration",
        )];
        let facts = vec![ResolutionFactDimension {
            repository_id: "typescript-repository".into(),
            fact_id: "typescript-fact".into(),
            edge_id: 11,
            language: "typescript".into(),
            callee_form: Some("identifier".into()),
            primary_evidence_kind: Some("same_file_declaration".into()),
        }];
        assert!(
            apply_qualification_stages(
                &mut rows,
                &facts,
                &BTreeSet::new(),
                &BTreeSet::from([("typescript-repository".into(), 11)]),
                &BTreeSet::new(),
            )
            .is_err()
        );
        assert!(
            apply_qualification_stages(
                &mut rows,
                &facts,
                &BTreeSet::from([("typescript-repository".into(), 99)]),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn authenticated_store_funnel_preserves_language_callee_and_primary_evidence_dimensions() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("src")).unwrap();
        fs::write(
            project.path().join("src/exported.ts"),
            "export function importedTarget() {}\n",
        )
        .unwrap();
        fs::write(
            project.path().join("src/importer.ts"),
            "import { importedTarget } from './exported';\nexport function caller() { importedTarget(); }\n",
        )
        .unwrap();
        fs::write(
            project.path().join("src/lib.rs"),
            "struct Worker; impl Worker { fn step(&self) {} fn run(&self) { self.step(); } }\n",
        )
        .unwrap();
        let cache = tempfile::tempdir().unwrap();
        let database = project.path().join("core.sqlite3");
        let runtime = super::super::materialize::core_only_runtime(project.path(), cache.path());
        runtime
            .project_service()
            .open_project_summary_with_storage_path(project.path().to_path_buf(), database.clone())
            .unwrap();
        runtime
            .index_service()
            .run_indexing_blocking_without_runtime_refresh(IndexMode::Full)
            .unwrap();
        drop(runtime);
        let store = Store::open_observational(&database).unwrap();
        let core = store.get_complete_index_publication().unwrap().unwrap();
        let expected = ProjectMaterializationEvidenceV1 {
            repository_id: "fixture".into(),
            source_head: "a".repeat(40),
            source_tree: "b".repeat(64),
            store_schema: "codestory-store/v32".into(),
            file_count: 3,
            node_count: 0,
            edge_count: 0,
            freshness: MaterializationFreshnessV1::Fresh,
            database_sha256: "c".repeat(64),
            core_generation: core.generation,
            identity: EnvironmentIdentityV1 {
                project_id: "project-fixture".into(),
                core_generation_id: core.generation_id,
                core_run_id: core.run_id,
            },
        };
        let report = build_repository_resolution_funnel("fixture", &expected, &store, &[])
            .expect("authenticated resolution funnel");
        assert!(report.rows.iter().any(|row| {
            row.language == "typescript"
                && row.callee_form.as_deref() == Some("named_import")
                && row.primary_evidence_kind.as_deref() == Some("static_import_binding")
        }));
        assert!(report.rows.iter().any(|row| {
            row.language == "rust"
                && row.callee_form.as_deref() == Some("implicit_receiver")
                && row.primary_evidence_kind.as_deref() == Some("implicit_receiver")
        }));
        assert!(report.rows.iter().all(|row| {
            row.counts.proof_shape_admitted == 0
                && row.counts.authoritative_receipts == 0
                && row.counts.complete_proofs == 0
        }));
    }
}
