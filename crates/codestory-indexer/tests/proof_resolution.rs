use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{EdgeKind, NodeKind};
use codestory_contracts::proof_resolution::{
    ProofResolutionStatus, ResolutionEvidence, ResolutionEvidenceKind,
};
use codestory_indexer::{WorkspaceIndexer, rematerialize_proof_resolution_projection};
use codestory_store::{IndexPublicationMode, IndexPublicationRecord, Store};
use codestory_workspace::{BuildMode, RefreshInfo};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

fn publication(generation: u64) -> IndexPublicationRecord {
    IndexPublicationRecord {
        generation,
        generation_id: format!("generation-{generation}"),
        run_id: format!("run-{generation}"),
        mode: if generation == 1 {
            IndexPublicationMode::Full
        } else {
            IndexPublicationMode::Incremental
        },
        published_at_epoch_ms: generation as i64,
    }
}

fn index_files(
    root: &std::path::Path,
    store: &mut Store,
    files: &[(&str, &str)],
) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for (relative, source) in files {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, source)?;
        paths.push(path);
    }
    WorkspaceIndexer::new(root.to_path_buf()).run_incremental(
        store,
        &RefreshInfo {
            mode: BuildMode::Incremental,
            files_to_index: paths.clone(),
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    Ok(paths)
}

#[test]
fn typescript_and_rust_reference_calls_rematerialize_exact_facts() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            (
                "src/local.ts",
                "export function localTarget() {}\nexport function localCaller() { localTarget(); }\n",
            ),
            ("src/imported.ts", "export function importedTarget() {}\n"),
            (
                "src/defaulted.ts",
                "export default function defaultTarget() {}\n",
            ),
            (
                "src/importer.ts",
                "import { importedTarget } from './imported';\nexport function importCaller() { importedTarget(); }\n",
            ),
            (
                "src/default_importer.ts",
                "import defaultTarget from './defaulted';\nexport function defaultCaller() { defaultTarget(); }\n",
            ),
            (
                "src/lib.rs",
                "fn rust_target() {}\nstruct Worker;\nimpl Worker {\n    fn step(&self) {}\n    fn run(&self) { self.step(); rust_target(); }\n}\n",
            ),
        ],
    )?;

    let graph_before = store.get_edges()?;

    let first = rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    assert_eq!(
        store.get_edges()?,
        graph_before,
        "proof overlay mutated graph output"
    );
    store.validate_proof_resolution_publication(&publication(1))?;
    let facts = store.get_proof_resolution_facts()?;

    for target in [
        "localTarget",
        "importedTarget",
        "defaultTarget",
        "step",
        "rust_target",
    ] {
        let fact = facts
            .iter()
            .find(|fact| fact.callsite.raw_target == target)
            .unwrap_or_else(|| panic!("missing fact for {target}: {facts:#?}"));
        assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
        assert!(fact.edge_id.is_some());
        assert!(fact.target.is_some());
        assert_eq!(fact.callsite.source_sha256.len(), 64);
        assert_eq!(fact.provenance.evidence_sha256.len(), 64);
        assert_eq!(fact.provenance.parser_fingerprint.len(), 64);
        assert!(
            fact.provenance
                .parser_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
    }
    let imported = facts
        .iter()
        .find(|fact| fact.callsite.raw_target == "importedTarget")
        .expect("imported call fact");
    assert!(matches!(
        imported.evidence_chain.as_slice(),
        [ResolutionEvidence::StaticImportBinding { .. }]
    ));
    let (import, declaration) = match imported.evidence_chain.as_slice() {
        [
            ResolutionEvidence::StaticImportBinding {
                import,
                declaration,
            },
        ] => (*import, *declaration),
        other => panic!("unexpected import evidence: {other:?}"),
    };
    let import_node = store.get_node(import)?.expect("import binding node");
    assert_eq!(
        import_node.file_node_id,
        Some(codestory_contracts::graph::NodeId(
            imported.callsite.file_id.0
        ))
    );
    assert_eq!(Some(declaration), imported.target);
    assert_eq!(
        store
            .get_edges()?
            .iter()
            .filter(|edge| edge.kind == EdgeKind::IMPORT
                && edge.source == import
                && edge.resolved_target == Some(declaration))
            .count(),
        1
    );
    let inherent = facts
        .iter()
        .find(|fact| fact.callsite.raw_target == "step")
        .expect("inherent method fact");
    assert!(
        inherent
            .evidence_chain
            .iter()
            .any(|evidence| evidence.kind() == ResolutionEvidenceKind::ImplicitReceiver)
    );

    let second = rematerialize_proof_resolution_projection(&mut store, &publication(2))?;
    assert_eq!(first.fact_count, second.fact_count);
    assert_eq!(first.fact_digest, second.fact_digest);
    assert_eq!(
        second
            .funnel
            .iter()
            .map(|row| row.counts.syntax_calls)
            .sum::<u64>(),
        second.fact_count
    );
    assert!(
        second
            .funnel
            .iter()
            .all(|row| row.counts.proof_shape_admitted == 0
                && row.counts.authoritative_receipts == 0
                && row.counts.complete_proofs == 0)
    );
    Ok(())
}

#[test]
fn syntax_claims_reject_shadowing_rebinding_trait_and_generic_inference() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            (
                "src/shadow.ts",
                "function target() {}\nexport function caller(target: () => void) { target(); }\n",
            ),
            (
                "src/rebind.ts",
                "function changed() {}\nexport function caller() { let changed = () => {}; changed(); }\n",
            ),
            (
                "src/trait.rs",
                "struct Worker; trait Run { fn step(&self); } impl Run for Worker { fn step(&self) {} } fn caller<T: Run>(value: &T) { value.step(); }\n",
            ),
            (
                "src/generic.rs",
                "struct Boxed<T>(T); impl<T> Boxed<T> { fn step(&self) {} fn run(&self) { self.step(); } }\n",
            ),
        ],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    for raw_target in ["target", "changed", "step"] {
        for fact in facts
            .iter()
            .filter(|fact| fact.callsite.raw_target == raw_target)
        {
            assert_ne!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
            assert!(fact.edge_id.is_none());
            assert!(fact.target.is_none());
        }
    }
    Ok(())
}

#[test]
fn parser_claim_is_independent_of_tampered_navigation_resolution() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("src/right.ts", "export function importedTarget() {}\n"),
            ("src/wrong.ts", "export function importedTarget() {}\n"),
            (
                "src/caller.ts",
                "import { importedTarget } from './right';\nexport function caller() { importedTarget(); }\n",
            ),
        ],
    )?;
    let wrong = store
        .get_nodes()?
        .into_iter()
        .find(|node| {
            node.kind == NodeKind::FUNCTION
                && node.serialized_name == "importedTarget"
                && node
                    .file_node_id
                    .and_then(|id| store.get_node(id).ok().flatten())
                    .is_some_and(|file| file.serialized_name.ends_with("wrong.ts"))
        })
        .expect("wrong declaration");
    let call_edge = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL && edge.line == Some(2))
        .expect("raw call edge");
    store.get_connection().execute(
        "UPDATE edge SET resolved_target_node_id = ?1 WHERE id = ?2",
        [wrong.id.0, call_edge.id.0],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "importedTarget")
        .expect("call fact");
    assert_ne!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert_eq!(fact.target, None);
    assert_eq!(fact.edge_id, None);
    Ok(())
}

#[test]
fn complete_projection_requires_cache_coverage_but_empty_and_unsupported_repositories_work()
-> anyhow::Result<()> {
    let mut empty = Store::new_in_memory()?;
    let empty_receipt = rematerialize_proof_resolution_projection(&mut empty, &publication(1))?;
    assert_eq!(empty_receipt.fact_count, 0);
    assert_eq!(empty_receipt.adapter_roster.len(), 3);

    let project = tempfile::tempdir()?;
    let mut unsupported = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut unsupported,
        &[
            ("main.go", "package main\nfunc main() {}\n"),
            ("main.py", "def main():\n    pass\n"),
        ],
    )?;
    let unsupported_receipt =
        rematerialize_proof_resolution_projection(&mut unsupported, &publication(1))?;
    assert_eq!(unsupported_receipt.fact_count, 0);
    assert_eq!(unsupported_receipt.adapter_roster.len(), 3);

    let governed_project = tempfile::tempdir()?;
    let mut governed = Store::new_in_memory()?;
    index_files(
        governed_project.path(),
        &mut governed,
        &[("src/main.ts", "export function main() {}\n")],
    )?;
    governed
        .get_connection()
        .execute("DELETE FROM index_artifact_cache", [])?;
    let error = rematerialize_proof_resolution_projection(&mut governed, &publication(1))
        .expect_err("missing governed cache coverage must fail");
    assert!(
        error.to_string().contains("cache coverage is missing"),
        "{error}"
    );

    let corrupt_project = tempfile::tempdir()?;
    let mut corrupt = Store::new_in_memory()?;
    index_files(
        corrupt_project.path(),
        &mut corrupt,
        &[("src/main.ts", "export function main() {}\n")],
    )?;
    corrupt
        .get_connection()
        .execute("UPDATE index_artifact_cache SET artifact_blob = x'00'", [])?;
    let error = rematerialize_proof_resolution_projection(&mut corrupt, &publication(1))
        .expect_err("corrupt governed cache coverage must fail");
    assert!(error.to_string().contains("cache is corrupt"), "{error}");

    let stale_project = tempfile::tempdir()?;
    let mut stale = Store::new_in_memory()?;
    index_files(
        stale_project.path(),
        &mut stale,
        &[("src/main.ts", "export function main() {}\n")],
    )?;
    stale.get_connection().execute(
        "UPDATE file SET content_hash = ?1 WHERE language = 'typescript'",
        ["f".repeat(64)],
    )?;
    let error = rematerialize_proof_resolution_projection(&mut stale, &publication(1))
        .expect_err("hash-mismatched governed cache coverage must fail");
    assert!(error.to_string().contains("hash-mismatched"), "{error}");

    let duplicate_project = tempfile::tempdir()?;
    let mut duplicate = Store::new_in_memory()?;
    index_files(
        duplicate_project.path(),
        &mut duplicate,
        &[("src/main.ts", "export function main() {}\n")],
    )?;
    duplicate.get_connection().execute(
        "INSERT INTO index_artifact_cache (file_path, cache_key, artifact_blob, updated_at_epoch_ms)
         SELECT file_path || '.duplicate', cache_key, artifact_blob, updated_at_epoch_ms
         FROM index_artifact_cache",
        [],
    )?;
    let error = rematerialize_proof_resolution_projection(&mut duplicate, &publication(1))
        .expect_err("duplicate governed cache coverage must fail");
    assert!(
        error.to_string().contains("coverage is duplicated"),
        "{error}"
    );
    Ok(())
}

#[test]
fn non_exact_reference_inputs_keep_closed_fail_closed_statuses() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            (
                "src/missing.ts",
                "export function caller() { missingBinding(); }\n",
            ),
            (
                "src/unsupported.ts",
                "export function caller() { object.dynamicCall(); }\n",
            ),
            (
                "src/incomplete.ts",
                "export function target() {}\nexport function caller() { target( ; }\n",
            ),
            (
                "src/ambiguous.ts",
                "function duplicate() {}\nfunction duplicate() {}\nexport function caller() { duplicate(); }\n",
            ),
        ],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    let status = |raw_target: &str| {
        facts
            .iter()
            .find(|fact| fact.callsite.raw_target == raw_target)
            .map(|fact| fact.status)
            .unwrap_or_else(|| panic!("missing {raw_target} fact: {facts:#?}"))
    };

    assert_eq!(
        status("missingBinding"),
        ProofResolutionStatus::MissingBinding
    );
    assert_eq!(status("dynamicCall"), ProofResolutionStatus::Unsupported);
    assert_eq!(status("target"), ProofResolutionStatus::IncompleteDomain);
    assert_eq!(status("duplicate"), ProofResolutionStatus::Ambiguous);
    Ok(())
}
