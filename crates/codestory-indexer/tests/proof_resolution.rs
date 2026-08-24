use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{EdgeId, EdgeKind, Node, NodeId, NodeKind, ResolutionCertainty};
use codestory_contracts::proof_resolution::{
    DependencyFileHash, FileId, ProofResolutionProjection, ProofResolutionReason,
    ProofResolutionStatus, ResolutionEvidence, ResolutionEvidenceKind,
};
use codestory_indexer::{
    WorkspaceIndexer, build_proof_resolution_funnel, rematerialize_proof_resolution_projection,
};
use codestory_store::{
    FileInfo, FileRole, IndexPublicationMode, IndexPublicationRecord, Store,
    seal_call_resolution_fact,
};
use codestory_workspace::{BuildMode, RefreshInfo};
use std::collections::{BTreeSet, HashMap};
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

#[derive(Clone, Copy, Debug)]
enum RelationMutation {
    Missing,
    Wrong,
    Duplicate,
    CandidateRetained,
    RecoveredSource,
    RecoveredMemberSource,
    RecoveredMemberTarget,
    WrongFile,
    WrongSourceKind,
    WrongTargetKind,
    WrongTargetOwnership,
}

fn mutate_relation(
    store: &mut Store,
    edge: &codestory_contracts::graph::Edge,
    mutation: RelationMutation,
) -> anyhow::Result<()> {
    match mutation {
        RelationMutation::Missing => {
            store
                .get_connection()
                .execute("DELETE FROM edge WHERE id = ?1", [edge.id.0])?;
        }
        RelationMutation::Wrong => {
            store.get_connection().execute(
                "UPDATE edge SET resolved_target_node_id = ?1 WHERE id = ?2",
                [edge.effective_source().0, edge.id.0],
            )?;
        }
        RelationMutation::Duplicate => {
            let mut duplicate = edge.clone();
            duplicate.id =
                EdgeId(8_800_000_000_000_000_000 + edge.id.0.unsigned_abs() as i64 % 1_000_000);
            store.insert_edge(&duplicate)?;
        }
        RelationMutation::CandidateRetained => {
            store.get_connection().execute(
                "UPDATE edge SET candidate_target_node_ids = ?1 WHERE id = ?2",
                (format!("[{}]", edge.effective_target().0), edge.id.0),
            )?;
        }
        RelationMutation::RecoveredSource => {
            store.get_connection().execute(
                "UPDATE edge SET resolved_source_node_id = file_node_id WHERE id = ?1",
                [edge.id.0],
            )?;
        }
        RelationMutation::RecoveredMemberSource => {
            store.get_connection().execute(
                "UPDATE edge SET source_node_id = file_node_id, resolved_source_node_id = ?1 WHERE id = ?2",
                [edge.source.0, edge.id.0],
            )?;
        }
        RelationMutation::RecoveredMemberTarget => {
            store.get_connection().execute(
                "UPDATE edge SET target_node_id = file_node_id, resolved_target_node_id = ?1 WHERE id = ?2",
                [edge.target.0, edge.id.0],
            )?;
        }
        RelationMutation::WrongFile => {
            store.get_connection().execute(
                "UPDATE edge SET file_node_id = source_node_id WHERE id = ?1",
                [edge.id.0],
            )?;
        }
        RelationMutation::WrongSourceKind => {
            store
                .get_connection()
                .execute("UPDATE node SET kind = 21 WHERE id = ?1", [edge.source.0])?;
        }
        RelationMutation::WrongTargetKind => {
            store
                .get_connection()
                .execute("UPDATE node SET kind = 21 WHERE id = ?1", [edge.target.0])?;
        }
        RelationMutation::WrongTargetOwnership => {
            store.get_connection().execute(
                "UPDATE node SET file_node_id = (SELECT file_node_id FROM node WHERE id = ?1) WHERE id = ?2",
                [edge.source.0, edge.target.0],
            )?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum RepeatedCallGraphMutation {
    DuplicateOrdinal,
    OrdinalGap,
    ExtraEdge,
    ExtraInput,
    NonCall,
    OpaqueIdentity,
    WrongIdentityFile,
    WrongIdentityLine,
    WrongIdentityRawTarget,
    CandidatesRetained,
    HeuristicSource,
    HeuristicTarget,
    CertainSource,
    CertainTarget,
    WrongRawSource,
    TwoValidRawPlaceholderGroups,
}

fn rewrite_callsite_identity(identity: &str, field: usize, value: i64) -> String {
    let (base, markers) = identity.split_once('|').unwrap_or((identity, ""));
    let mut fields = base.split(':').map(str::to_owned).collect::<Vec<_>>();
    assert_eq!(fields.len(), 4, "test fixture identity is canonical");
    fields[field] = value.to_string();
    let mut rewritten = fields.join(":");
    if !markers.is_empty() {
        rewritten.push('|');
        rewritten.push_str(markers);
    }
    rewritten
}

fn repeated_call_facts_after_graph_mutation(
    mutation: RepeatedCallGraphMutation,
) -> anyhow::Result<Vec<codestory_contracts::proof_resolution::CallResolutionFact>> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/lib.rs",
            "fn target() {}\nfn source() { target(); target(); }\n",
        )],
    )?;
    let mut calls = store
        .get_edges()?
        .into_iter()
        .filter(|edge| edge.kind == EdgeKind::CALL && edge.line == Some(2))
        .collect::<Vec<_>>();
    calls.sort_by_key(|edge| {
        edge.callsite_identity
            .as_deref()
            .and_then(|identity| identity.split('|').next())
            .and_then(|identity| identity.split(':').nth(2))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    assert_eq!(calls.len(), 2, "real repeated-call graph fixture");
    let connection = store.get_connection();
    match mutation {
        RepeatedCallGraphMutation::DuplicateOrdinal => {
            connection.execute(
                "UPDATE edge SET callsite_identity = ?1 WHERE id = ?2",
                (
                    calls[0].callsite_identity.as_deref().unwrap(),
                    calls[1].id.0,
                ),
            )?;
        }
        RepeatedCallGraphMutation::OrdinalGap => {
            let identity =
                rewrite_callsite_identity(calls[1].callsite_identity.as_deref().unwrap(), 2, 3);
            connection.execute(
                "UPDATE edge SET callsite_identity = ?1 WHERE id = ?2",
                (identity, calls[1].id.0),
            )?;
        }
        RepeatedCallGraphMutation::ExtraEdge => {
            let mut extra = calls[1].clone();
            extra.id = EdgeId(9_000_000_000_000_000_000);
            extra.callsite_identity = Some(rewrite_callsite_identity(
                extra.callsite_identity.as_deref().unwrap(),
                2,
                3,
            ));
            store.insert_edge(&extra)?;
        }
        RepeatedCallGraphMutation::ExtraInput => {
            connection.execute("DELETE FROM edge WHERE id = ?1", [calls[1].id.0])?;
        }
        RepeatedCallGraphMutation::NonCall => {
            connection.execute(
                "UPDATE edge SET kind = ?1 WHERE id = ?2",
                (EdgeKind::USAGE as i32, calls[1].id.0),
            )?;
        }
        RepeatedCallGraphMutation::OpaqueIdentity => {
            connection.execute(
                "UPDATE edge SET callsite_identity = 'opaque' WHERE id = ?1",
                [calls[1].id.0],
            )?;
        }
        RepeatedCallGraphMutation::WrongIdentityFile => {
            let identity = rewrite_callsite_identity(
                calls[0].callsite_identity.as_deref().unwrap(),
                0,
                calls[0].file_node_id.unwrap().0.wrapping_add(1),
            );
            connection.execute(
                "UPDATE edge SET callsite_identity = ?1 WHERE id = ?2",
                (identity, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::WrongIdentityLine => {
            let identity =
                rewrite_callsite_identity(calls[0].callsite_identity.as_deref().unwrap(), 1, 99);
            connection.execute(
                "UPDATE edge SET callsite_identity = ?1 WHERE id = ?2",
                (identity, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::WrongIdentityRawTarget => {
            let identity = rewrite_callsite_identity(
                calls[0].callsite_identity.as_deref().unwrap(),
                3,
                calls[0].target.0.wrapping_add(1),
            );
            connection.execute(
                "UPDATE edge SET callsite_identity = ?1 WHERE id = ?2",
                (identity, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::CandidatesRetained => {
            connection.execute(
                "UPDATE edge SET candidate_target_node_ids = ?1 WHERE id = ?2",
                (
                    format!("[{}]", calls[0].effective_target().0),
                    calls[0].id.0,
                ),
            )?;
        }
        RepeatedCallGraphMutation::HeuristicSource => {
            connection.execute(
                "UPDATE edge SET resolved_source_node_id = ?1, certainty = 'uncertain' WHERE id = ?2",
                (calls[0].file_node_id.unwrap().0, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::HeuristicTarget => {
            connection.execute(
                "UPDATE edge SET resolved_target_node_id = ?1, certainty = 'uncertain' WHERE id = ?2",
                (calls[0].effective_source().0, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::CertainSource => {
            connection.execute(
                "UPDATE edge SET resolved_source_node_id = ?1, certainty = 'certain' WHERE id = ?2",
                (calls[0].file_node_id.unwrap().0, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::CertainTarget => {
            connection.execute(
                "UPDATE edge SET resolved_target_node_id = ?1, certainty = 'certain' WHERE id = ?2",
                (calls[0].effective_source().0, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::WrongRawSource => {
            connection.execute(
                "UPDATE edge SET source_node_id = ?1, resolved_source_node_id = NULL, certainty = 'uncertain' WHERE id = ?2",
                (calls[0].file_node_id.unwrap().0, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::TwoValidRawPlaceholderGroups => {
            let mut raw = store.get_node(calls[0].target)?.unwrap();
            raw.id = NodeId(8_999_999_999_999_999_999);
            store.insert_node(&raw)?;
            for (index, call) in calls.iter().enumerate() {
                let mut duplicate = call.clone();
                duplicate.id = EdgeId(8_999_999_999_999_999_990 + index as i64);
                duplicate.target = raw.id;
                duplicate.callsite_identity = Some(rewrite_callsite_identity(
                    duplicate.callsite_identity.as_deref().unwrap(),
                    3,
                    raw.id.0,
                ));
                store.insert_edge(&duplicate)?;
            }
        }
    }
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    Ok(store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "target")
        .collect())
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
    let graph_after = store.get_edges()?;
    assert_eq!(graph_after.len(), graph_before.len());
    for before in &graph_before {
        let after = graph_after
            .iter()
            .find(|edge| edge.id == before.id)
            .expect("projection preserves every edge ID");
        assert_eq!(after.source, before.source);
        assert_eq!(after.target, before.target);
        assert_eq!(after.kind, before.kind);
        assert_eq!(after.file_node_id, before.file_node_id);
        assert_eq!(after.line, before.line);
        assert_eq!(after.callsite_identity, before.callsite_identity);
        if before.kind != EdgeKind::CALL {
            assert_eq!(after, before, "non-CALL graph output is immutable");
        }
    }
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
fn go_closed_exact_subset_authorizes_package_functions_and_concrete_receivers() -> anyhow::Result<()>
{
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("go.mod", "module example.com/proof\n\ngo 1.24\n"),
            (
                "worker.go",
                concat!(
                    "package proof\n\n",
                    "type Worker struct{}\n",
                    "type ValueWorker struct{}\n",
                    "func localTarget() {}\n",
                    "func (w *Worker) implicit() { w.step() }\n",
                    "func (w *Worker) step() {}\n",
                    "func (w ValueWorker) valueStep() {}\n",
                    "func parameter(w *Worker) { w.step() }\n",
                    "func constructed() {\n",
                    "  a := ValueWorker{}\n",
                    "  a.valueStep()\n",
                    "  b := &Worker{}\n",
                    "  b.step()\n",
                    "  c := new(Worker)\n",
                    "  c.step()\n",
                    "}\n",
                    "func sameFile() { localTarget() }\n",
                ),
            ),
            (
                "cross.go",
                "package proof\n\nfunc crossFile() { localTarget() }\n",
            ),
        ],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    store.validate_proof_resolution_publication(&publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    let exact = facts
        .iter()
        .filter(|fact| fact.provenance.language_adapter == "go")
        .filter(|fact| fact.status == ProofResolutionStatus::Exact)
        .collect::<Vec<_>>();
    assert_eq!(
        exact
            .iter()
            .filter(|fact| fact.callsite.raw_target == "localTarget")
            .count(),
        2,
        "same-file and cross-file package functions: {facts:#?}"
    );
    assert_eq!(
        exact
            .iter()
            .filter(|fact| matches!(fact.callsite.raw_target.as_str(), "step" | "valueStep"))
            .count(),
        5,
        "implicit, typed, value, pointer, and builtin-new receivers: {facts:#?}"
    );
    assert!(exact.iter().all(|fact| {
        fact.provenance.dependency_file_hashes.len() == 2
            && fact.edge_id.is_some()
            && fact.target.is_some()
    }));
    Ok(())
}

#[test]
fn python_closed_exact_subset_authorizes_s1_through_s4() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("pkg/__init__.py", ""),
            (
                "pkg/target.py",
                "def imported_target():\n    pass\n\nclass ImportedWorker:\n    def run(self):\n        pass\n",
            ),
            (
                "pkg/main.py",
                concat!(
                    "from .target import imported_target\n",
                    "from .target import ImportedWorker\n\n",
                    "def local_target():\n    pass\n\n",
                    "class Worker:\n",
                    "    def run(self):\n        pass\n",
                    "    def caller(self):\n        self.run()\n\n",
                    "def local_caller():\n    local_target()\n\n",
                    "def import_caller():\n    imported_target()\n\n",
                    "def constructed_caller():\n",
                    "    local = Worker()\n",
                    "    local.run()\n",
                    "    imported: ImportedWorker\n",
                    "    imported.run()\n",
                ),
            ),
        ],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    store.validate_proof_resolution_publication(&publication(1))?;
    let facts = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.provenance.language_adapter == "python")
        .collect::<Vec<_>>();
    for target in ["local_target", "imported_target"] {
        let fact = facts
            .iter()
            .find(|fact| fact.callsite.raw_target == target)
            .unwrap_or_else(|| panic!("missing Python fact for {target}: {facts:#?}"));
        assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    }
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.callsite.raw_target == "run"
                && fact.status == ProofResolutionStatus::Exact)
            .count(),
        3,
        "self, constructor, and explicit annotation receivers: {facts:#?}"
    );
    assert!(
        facts
            .iter()
            .all(|fact| fact.callsite.start_byte < fact.callsite.end_byte_exclusive)
    );
    Ok(())
}

#[test]
fn python_single_simple_base_authorizes_a_direct_same_class_method() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "main.py",
            concat!(
                "class Base:\n    pass\n\n",
                "class Worker(Base):\n",
                "    def target(self):\n        pass\n",
                "    def caller(self):\n        self.target()\n",
            ),
        )],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    let fact = facts
        .iter()
        .find(|fact| {
            fact.provenance.language_adapter == "python" && fact.callsite.raw_target == "target"
        })
        .unwrap_or_else(|| panic!("missing Python target fact: {facts:#?}"));
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert!(matches!(
        fact.evidence_chain.as_slice(),
        [
            ResolutionEvidence::ImplicitReceiver { .. },
            ResolutionEvidence::SameFileDeclaration { .. }
        ]
    ));
    Ok(())
}

#[test]
fn python_class_header_comments_do_not_change_direct_method_semantics() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "main.py",
            concat!(
                "class Worker(dict):  # type: ignore[type-arg]\n",
                "    def target(self):\n        return True\n",
                "    def caller(self):\n        return self.target()\n",
            ),
        )],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    let fact = facts
        .iter()
        .find(|fact| {
            fact.provenance.language_adapter == "python" && fact.callsite.raw_target == "target"
        })
        .unwrap_or_else(|| panic!("missing Python target fact: {facts:#?}"));
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert!(matches!(
        fact.evidence_chain.as_slice(),
        [
            ResolutionEvidence::ImplicitReceiver { .. },
            ResolutionEvidence::SameFileDeclaration { .. }
        ]
    ));
    Ok(())
}

fn assert_python_target_has_closed_status(
    files: &[(&str, &str)],
    raw_target: &str,
    expected: ProofResolutionStatus,
) -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(project.path(), &mut store, files)?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| {
            fact.provenance.language_adapter == "python" && fact.callsite.raw_target == raw_target
        })
        .collect::<Vec<_>>();
    assert!(
        !facts.is_empty(),
        "missing closed Python fact for {raw_target}"
    );
    assert!(
        facts.iter().any(|fact| fact.status == expected),
        "missing {expected:?} Python fact for {raw_target}: {facts:#?}"
    );
    assert!(
        facts.iter().all(|fact| {
            fact.status != ProofResolutionStatus::Exact
                && fact.reason.matches_status(fact.status)
                && fact.evidence_chain.is_empty()
        }),
        "unsupported Python family became authoritative: {facts:#?}"
    );
    Ok(())
}

#[test]
fn python_closed_statuses_distinguish_unsupported_missing_ambiguous_and_incomplete()
-> anyhow::Result<()> {
    assert_python_target_has_closed_status(
        &[("main.py", "def caller():\n    missing()\n")],
        "missing",
        ProofResolutionStatus::MissingBinding,
    )?;
    assert_python_target_has_closed_status(
        &[(
            "main.py",
            "def target():\n    pass\ndef target():\n    pass\ndef caller():\n    target()\n",
        )],
        "target",
        ProofResolutionStatus::Ambiguous,
    )?;
    assert_python_target_has_closed_status(
        &[("main.py", "def caller(:\n    target()\n")],
        "target",
        ProofResolutionStatus::IncompleteDomain,
    )?;
    assert_python_target_has_closed_status(
        &[("main.py", "def caller(target):\n    target()\n")],
        "target",
        ProofResolutionStatus::Unsupported,
    )?;
    Ok(())
}

#[test]
fn python_dispatch_and_dynamic_language_families_are_closed_unsupported() -> anyhow::Result<()> {
    for source in [
        "class Base:\n    def target(self):\n        pass\nclass Child(Base):\n    def caller(self):\n        self.target()\n",
        "class Left:\n    pass\nclass Right:\n    pass\nclass Child(Left, Right):\n    def target(self):\n        pass\n    def caller(self):\n        self.target()\n",
        "class Base:\n    def target(self):\n        pass\nclass Child(Base):\n    def caller(self):\n        super().target()\n",
        "class Worker:\n    def target(self):\n        pass\n    @classmethod\n    def caller(cls):\n        cls.target()\n",
        "class Worker:\n    @staticmethod\n    def target():\n        pass\n    def caller(self):\n        self.target()\n",
        "class Worker:\n    @property\n    def target(self):\n        return 1\n    def caller(self):\n        self.target()\n",
        "def decorate(value):\n    return value\nclass Worker:\n    @decorate\n    def target(self):\n        pass\n    def caller(self):\n        self.target()\n",
        "class Meta(type):\n    pass\nclass Worker(metaclass=Meta):\n    def target(self):\n        pass\n    def caller(self):\n        self.target()\n",
        "class Worker[T]:\n    def target(self):\n        pass\n    def caller(self):\n        self.target()\n",
        "class Worker:\n    def target(self):\n        pass\ndef caller(worker):\n    worker.target()\n",
        "class Worker:\n    def target(self):\n        pass\ndef factory():\n    return Worker()\ndef caller():\n    factory().target()\n",
        "class Worker:\n    def target(self):\n        pass\ndef caller(wrapper):\n    wrapper.worker.target()\n",
        "class Worker:\n    def target(self):\n        pass\ndef caller():\n    worker = Worker()\n    setattr(worker, 'target', lambda: None)\n    worker.target()\n",
        "def target():\n    pass\ndef caller():\n    eval('target = None')\n    target()\n",
    ] {
        assert_python_target_has_closed_status(
            &[("main.py", source)],
            "target",
            ProofResolutionStatus::Unsupported,
        )?;
    }
    for mutation in [
        "Worker.target = lambda self: None",
        "Worker.target: object = lambda self: None",
        "Worker.target += other",
        "del Worker.target",
    ] {
        let source = format!(
            "class Worker:\n    def target(self):\n        pass\n{mutation}\ndef caller():\n    worker: Worker\n    worker.target()\n"
        );
        assert_python_target_has_closed_status(
            &[("main.py", source.as_str())],
            "target",
            ProofResolutionStatus::Unsupported,
        )?;
    }
    Ok(())
}

#[test]
fn python_namespace_closure_hostiles_never_become_exact() -> anyhow::Result<()> {
    let single_file_cases = [
        concat!(
            "class Worker:\n",
            "    def target(self):\n        pass\n",
            "    target = lambda self: None\n",
            "    def caller(self):\n        self.target()\n",
        ),
        concat!(
            "class Descriptor:\n    def __get__(self, instance, owner):\n        return lambda: None\n",
            "class Worker:\n",
            "    def target(self):\n        pass\n",
            "    target = Descriptor()\n",
            "    def caller(self):\n        self.target()\n",
        ),
        concat!(
            "class Worker:\n",
            "    def target(self):\n        pass\n",
            "    del target\n",
            "    def caller(self):\n        self.target()\n",
        ),
        concat!(
            "class Worker:\n",
            "    def target(self):\n        pass\n",
            "    from other import target\n",
            "    def caller(self):\n        self.target()\n",
        ),
        concat!(
            "class Worker:\n",
            "    def target(self):\n        pass\n",
            "    def mutate(self):\n        self.target = lambda: None\n",
            "    def caller(self):\n        self.target()\n",
        ),
        concat!(
            "class Worker:\n",
            "    def target(self):\n        pass\n",
            "    def __getattribute__(self, name):\n        return object.__getattribute__(self, name)\n",
            "    def caller(self):\n        self.target()\n",
        ),
        concat!(
            "def target():\n    pass\n",
            "def caller():\n    target += other\n    target()\n",
        ),
        concat!(
            "def target():\n    pass\n",
            "def caller():\n    callback = lambda: target()\n    callback()\n",
        ),
        concat!(
            "def target():\n    pass\n",
            "def mutate():\n    global target\n    target = other\n",
            "def caller():\n    target()\n",
        ),
        concat!(
            "class Worker:\n    def target(self):\n        pass\n",
            "def caller():\n",
            "    Worker = make_factory()\n",
            "    worker = Worker()\n",
            "    worker.target()\n",
        ),
        concat!(
            "class Worker:\n    def target(self):\n        pass\n",
            "def caller():\n",
            "    worker: Worker\n",
            "    Worker = Other\n",
            "    worker.target()\n",
        ),
    ];
    for source in single_file_cases {
        assert_python_target_has_closed_status(
            &[("main.py", source)],
            "target",
            ProofResolutionStatus::Unsupported,
        )?;
    }

    assert_python_target_has_closed_status(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/target.py", "def target():\n    pass\n"),
            (
                "pkg/main.py",
                concat!(
                    "from .target import target\n",
                    "target = replacement\n",
                    "def caller():\n    target()\n",
                ),
            ),
        ],
        "target",
        ProofResolutionStatus::Unsupported,
    )?;

    Ok(())
}

#[test]
fn python_import_and_annotation_families_are_closed_non_exact() -> anyhow::Result<()> {
    for source in [
        "from external import target\ndef caller():\n    target()\n",
        "import external\ndef caller():\n    external.target()\n",
        "from external import *\ndef caller():\n    target()\n",
        "import importlib\ndef caller():\n    importlib.import_module('external').target()\n",
        "if enabled:\n    from .target import target\ndef caller():\n    target()\n",
        "try:\n    from .target import target\nexcept ImportError:\n    pass\ndef caller():\n    target()\n",
        "if TYPE_CHECKING:\n    from .target import target\ndef caller():\n    target()\n",
    ] {
        assert_python_target_has_closed_status(
            &[("main.py", source)],
            "target",
            ProofResolutionStatus::Unsupported,
        )?;
    }

    for annotation in ["'Worker'", "Worker | None", "list[Worker]", "Unknown"] {
        let source = format!(
            "class Worker:\n    def target(self):\n        pass\ndef caller():\n    worker: {annotation}\n    worker.target()\n"
        );
        assert_python_target_has_closed_status(
            &[("main.py", source.as_str())],
            "target",
            ProofResolutionStatus::Unsupported,
        )?;
    }

    assert_python_target_has_closed_status(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/reexport.py", "from .actual import target\n"),
            ("pkg/actual.py", "def target():\n    pass\n"),
            (
                "pkg/main.py",
                "from .reexport import target\ndef caller():\n    target()\n",
            ),
        ],
        "target",
        ProofResolutionStatus::Unsupported,
    )?;
    assert_python_target_has_closed_status(
        &[
            ("pkg/__init__.py", ""),
            (
                "pkg/main.py",
                "from .stubs import target\ndef caller():\n    target()\n",
            ),
            ("pkg/stubs.pyi", "def target() -> None: ...\n"),
        ],
        "target",
        ProofResolutionStatus::MissingBinding,
    )?;
    Ok(())
}

#[test]
fn python_constructor_calls_and_non_straight_line_receivers_never_gain_authority()
-> anyhow::Result<()> {
    assert_python_target_has_closed_status(
        &[(
            "main.py",
            "class Worker:\n    def target(self):\n        pass\ndef caller():\n    worker = Worker()\n    if enabled:\n        worker.target()\n",
        )],
        "target",
        ProofResolutionStatus::Unsupported,
    )?;
    assert_python_target_has_closed_status(
        &[(
            "main.py",
            "class Worker:\n    def __init__(self):\n        pass\ndef caller():\n    Worker()\n",
        )],
        "Worker",
        ProofResolutionStatus::Unsupported,
    )?;
    Ok(())
}

#[test]
fn python_unsupported_and_lexical_hostiles_receive_closed_non_exact_facts() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "main.py",
            concat!(
                "def target():\n    pass\n\n",
                "class Base:\n    def method(self):\n        pass\n\n",
                "class Child(Base):\n",
                "    @classmethod\n    def class_call(cls):\n        cls.method()\n",
                "    def inherited(self):\n        self.method()\n",
                "    def parent(self):\n        super().method()\n\n",
                "def parameter(target):\n    target()\n\n",
                "def future_assignment():\n    target()\n    target = lambda: None\n\n",
                "def destructuring():\n    target, other = values\n    target()\n\n",
                "def control_flow():\n    for target in values:\n        target()\n\n",
                "def dynamic(receiver):\n    getattr(receiver, 'method')()\n\n",
                "def chained(factory):\n    factory().method()\n\n",
                "def computed(receiver):\n    receiver['method']()\n\n",
                "def nested():\n    def inner():\n        target()\n    inner()\n",
            ),
        )],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.provenance.language_adapter == "python")
        .collect::<Vec<_>>();
    assert!(
        facts.len() >= 12,
        "every Python syntax call must be closed: {facts:#?}"
    );
    assert!(
        facts.iter().all(|fact| {
            fact.status != ProofResolutionStatus::Exact
                && fact.reason.matches_status(fact.status)
                && fact.evidence_chain.is_empty()
        }),
        "hostile Python fact became authoritative: {facts:#?}"
    );
    Ok(())
}

#[test]
fn python_calls_preserve_utf8_crlf_and_repeated_terminal_coordinates() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    let source = "def target():\r\n    pass\r\n\r\ndef caller():\r\n    note = 'é'\r\n    target(); target()\r\n";
    index_files(project.path(), &mut store, &[("main.py", source)])?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let mut facts = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.provenance.language_adapter == "python")
        .filter(|fact| fact.callsite.raw_target == "target")
        .collect::<Vec<_>>();
    facts.sort_by_key(|fact| fact.callsite.start_byte);
    assert_eq!(facts.len(), 2, "{facts:#?}");
    assert!(
        facts
            .iter()
            .all(|fact| fact.status == ProofResolutionStatus::Exact)
    );
    assert_eq!(facts[0].callsite.line, 6);
    assert_eq!(facts[0].callsite.column, 5);
    assert_eq!(facts[1].callsite.column, 15);
    assert_eq!(
        &source.as_bytes()
            [facts[0].callsite.start_byte as usize..facts[0].callsite.end_byte_exclusive as usize],
        b"target"
    );
    assert_ne!(facts[0].edge_id, facts[1].edge_id);
    Ok(())
}

#[test]
fn python_exact_fact_replay_rejects_graph_correlation_mutations() -> anyhow::Result<()> {
    for mutation in ["target", "file", "line", "candidate", "identity"] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[(
                "main.py",
                "def target():\n    pass\n\ndef caller():\n    target()\n",
            )],
        )?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("Python exact fact");
        assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
        let edge_id = fact.edge_id.expect("linked raw CALL").0;
        match mutation {
            "target" => {
                store.get_connection().execute(
                    "UPDATE edge SET resolved_target_node_id = source_node_id WHERE id = ?1",
                    [edge_id],
                )?;
            }
            "file" => {
                store.get_connection().execute(
                    "UPDATE edge SET file_node_id = target_node_id WHERE id = ?1",
                    [edge_id],
                )?;
            }
            "line" => {
                store
                    .get_connection()
                    .execute("UPDATE edge SET line = 99 WHERE id = ?1", [edge_id])?;
            }
            "candidate" => {
                store.get_connection().execute(
                    "UPDATE edge SET candidate_target_node_ids = ?1 WHERE id = ?2",
                    (format!("[{}]", fact.target.unwrap().0), edge_id),
                )?;
            }
            "identity" => {
                store.get_connection().execute(
                    "UPDATE edge SET callsite_identity = 'opaque' WHERE id = ?1",
                    [edge_id],
                )?;
            }
            _ => unreachable!(),
        }
        let error = store
            .validate_proof_resolution_publication(&publication(1))
            .expect_err("resealed Python graph mutation must reject the exact fact");
        assert!(!error.to_string().is_empty(), "{mutation}");
    }
    Ok(())
}

#[test]
fn python_exact_fact_replay_rejects_resealed_file_row_move() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    let paths = index_files(
        project.path(),
        &mut store,
        &[(
            "main.py",
            "def target():\n    pass\n\ndef caller():\n    target()\n",
        )],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let source = paths.into_iter().next().expect("Python source path");
    let moved = project.path().join("moved.py");
    fs::rename(&source, &moved)?;
    store.get_connection().execute(
        "UPDATE file SET path = ?1 WHERE language = 'python'",
        [moved.to_string_lossy().into_owned()],
    )?;
    let error = store
        .validate_proof_resolution_publication(&publication(1))
        .expect_err("resealed Python file-row move must reject the exact fact");
    assert!(
        error.to_string().contains("canonical file node id"),
        "{error}"
    );
    Ok(())
}

#[test]
fn python_relative_import_domain_fails_closed_for_missing_and_colliding_modules()
-> anyhow::Result<()> {
    for files in [
        vec![
            (
                "pkg/main.py",
                "from .target import target\ndef caller():\n    target()\n",
            ),
            ("pkg/target.py", "def target():\n    pass\n"),
        ],
        vec![
            ("pkg/__init__.py", ""),
            (
                "pkg/main.py",
                "from .target import target\ndef caller():\n    target()\n",
            ),
            ("pkg/target.py", "def target():\n    pass\n"),
            ("pkg/target/__init__.py", "def target():\n    pass\n"),
        ],
        vec![
            ("pkg/__init__.py", ""),
            (
                "pkg/main.py",
                "from target import target\ndef caller():\n    target()\n",
            ),
            ("pkg/target.py", "def target():\n    pass\n"),
        ],
        vec![
            ("pkg/__init__.py", ""),
            (
                "pkg/main.py",
                "from .sub.target import target\ndef caller():\n    target()\n",
            ),
            ("pkg/sub/target.py", "def target():\n    pass\n"),
        ],
        vec![
            ("pkg/__init__.py", ""),
            ("pkg/sub/__init__.py", ""),
            (
                "pkg/main.py",
                "from .sub.target import target\ndef caller():\n    target()\n",
            ),
            ("pkg/sub/target.py", "def target():\n    pass\n"),
            ("pkg/sub/target/__init__.py", "def target():\n    pass\n"),
        ],
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &files)?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| {
                fact.provenance.language_adapter == "python" && fact.callsite.raw_target == "target"
            })
            .expect("closed import call fact");
        assert_ne!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    }
    Ok(())
}

#[test]
fn python_raw_import_marker_hostiles_never_corroborate_exact_evidence() -> anyhow::Result<()> {
    for mutation in [
        "wrong_source",
        "candidate",
        "cross_file",
        "non_marker",
        "inconsistent_resolved",
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[
                ("pkg/__init__.py", ""),
                ("pkg/target.py", "def target():\n    pass\n"),
                (
                    "pkg/main.py",
                    "from .target import target\ndef caller():\n    target()\n",
                ),
            ],
        )?;
        let marker = store
            .get_edges()?
            .into_iter()
            .find(|edge| {
                edge.kind == EdgeKind::IMPORT
                    && store
                        .get_node(edge.target)
                        .ok()
                        .flatten()
                        .is_some_and(|node| node.kind == NodeKind::MODULE)
            })
            .expect("raw Python import marker edge");
        match mutation {
            "wrong_source" => {
                store.get_connection().execute(
                    "UPDATE edge SET resolved_source_node_id = target_node_id WHERE id = ?1",
                    [marker.id.0],
                )?;
            }
            "candidate" => {
                store.get_connection().execute(
                    "UPDATE edge SET candidate_target_node_ids = ?1 WHERE id = ?2",
                    (format!("[{}]", marker.target.0), marker.id.0),
                )?;
            }
            "cross_file" => {
                let target_file = store
                    .get_nodes()?
                    .into_iter()
                    .find(|node| {
                        node.kind == NodeKind::FUNCTION && node.serialized_name == "target"
                    })
                    .and_then(|node| node.file_node_id)
                    .expect("imported target file");
                store.get_connection().execute(
                    "UPDATE node SET file_node_id = ?1 WHERE id = ?2",
                    (target_file.0, marker.target.0),
                )?;
            }
            "non_marker" => {
                store.get_connection().execute(
                    "UPDATE node SET kind = ?1 WHERE id = ?2",
                    (NodeKind::FUNCTION as i32, marker.target.0),
                )?;
            }
            "inconsistent_resolved" => {
                store.get_connection().execute(
                    "UPDATE edge SET resolved_target_node_id = source_node_id WHERE id = ?1",
                    [marker.id.0],
                )?;
            }
            _ => unreachable!(),
        }
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("closed Python import fact");
        assert_ne!(
            fact.status,
            ProofResolutionStatus::Exact,
            "{mutation}: {fact:#?}"
        );
    }
    Ok(())
}

#[test]
fn python_nested_relative_import_authenticates_every_package_marker_exactly() -> anyhow::Result<()>
{
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("pkg/__init__.py", ""),
            ("pkg/sub/__init__.py", ""),
            ("pkg/sub/target.py", "def target():\n    pass\n"),
            (
                "pkg/main.py",
                "from .sub.target import target\ndef caller():\n    target()\n",
            ),
        ],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    store.validate_proof_resolution_publication(&publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| {
            fact.provenance.language_adapter == "python" && fact.callsite.raw_target == "target"
        })
        .expect("nested relative-import fact");
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");

    let files = store.get_files()?;
    let expected = [
        "pkg/__init__.py",
        "pkg/sub/__init__.py",
        "pkg/sub/target.py",
        "pkg/main.py",
    ]
    .into_iter()
    .map(|suffix| {
        FileId(
            files
                .iter()
                .find(|file| file.path.ends_with(suffix))
                .unwrap_or_else(|| panic!("missing indexed dependency {suffix}"))
                .id,
        )
    })
    .collect::<BTreeSet<_>>();
    let observed = fact
        .provenance
        .dependency_file_hashes
        .iter()
        .map(|dependency| dependency.file_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(observed, expected, "{fact:#?}");
    Ok(())
}

#[test]
fn python_nested_relative_import_store_replay_rejects_dependency_mutations() -> anyhow::Result<()> {
    for mutation in ["missing", "extra", "hash"] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[
                ("pkg/__init__.py", ""),
                ("pkg/sub/__init__.py", ""),
                ("pkg/sub/target.py", "def target():\n    pass\n"),
                ("pkg/unrelated.py", "unrelated = True\n"),
                (
                    "pkg/main.py",
                    "from .sub.target import target\ndef caller():\n    target()\n",
                ),
            ],
        )?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let receipt = store
            .get_proof_resolution_publication()?
            .expect("Python proof publication");
        let files = store.get_files()?;
        let file = |suffix: &str| {
            files
                .iter()
                .find(|file| file.path.ends_with(suffix))
                .unwrap_or_else(|| panic!("missing {suffix}"))
        };
        let intermediate = FileId(file("pkg/sub/__init__.py").id);
        let unrelated = FileId(file("pkg/unrelated.py").id);
        let unrelated_hash = store
            .get_file_content_hash(unrelated.0)?
            .expect("unrelated source hash");
        let mut facts = store.get_proof_resolution_facts()?;
        let fact = facts
            .iter_mut()
            .find(|fact| {
                fact.provenance.language_adapter == "python" && fact.callsite.raw_target == "target"
            })
            .expect("nested relative-import fact");
        match mutation {
            "missing" => fact
                .provenance
                .dependency_file_hashes
                .retain(|dependency| dependency.file_id != intermediate),
            "extra" => fact
                .provenance
                .dependency_file_hashes
                .push(DependencyFileHash {
                    file_id: unrelated,
                    source_sha256: unrelated_hash,
                }),
            "hash" => {
                fact.provenance
                    .dependency_file_hashes
                    .iter_mut()
                    .find(|dependency| dependency.file_id == intermediate)
                    .expect("intermediate package marker dependency")
                    .source_sha256 = "0".repeat(64);
            }
            _ => unreachable!(),
        }
        *fact = seal_call_resolution_fact(fact.clone())?;
        let projection = ProofResolutionProjection {
            adapter_roster: receipt.adapter_roster,
            funnel: build_proof_resolution_funnel(&facts),
            facts,
        };
        let error = store
            .replace_proof_resolution_projection(&publication(2), &projection)
            .expect_err("resealed nested Python dependency mutation must fail closed");
        assert!(
            error.to_string().contains("depend") || error.to_string().contains("source hash"),
            "{mutation}: {error}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn python_nested_relative_import_rejects_symlinked_package_marker() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    let root_marker = project.path().join("pkg/__init__.py");
    let nested_marker = project.path().join("pkg/sub/__init__.py");
    fs::create_dir_all(nested_marker.parent().unwrap())?;
    fs::write(&root_marker, "")?;
    symlink(&root_marker, &nested_marker)?;
    fs::write(
        project.path().join("pkg/sub/target.py"),
        "def target():\n    pass\n",
    )?;
    fs::write(
        project.path().join("pkg/main.py"),
        "from .sub.target import target\ndef caller():\n    target()\n",
    )?;
    WorkspaceIndexer::new(project.path().to_path_buf()).run_incremental(
        &mut store,
        &RefreshInfo {
            mode: BuildMode::Incremental,
            files_to_index: vec![
                root_marker,
                nested_marker,
                project.path().join("pkg/sub/target.py"),
                project.path().join("pkg/main.py"),
            ],
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
        .expect_err("symlinked Python package marker must fail closed");
    assert!(
        error.to_string().contains("identity collision") || error.to_string().contains("symlink"),
        "{error}"
    );
    Ok(())
}

#[test]
fn go_explicit_typed_local_receiver_is_exact_without_reassignment() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "main.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func caller() { var worker *Worker; worker.Run() }\n",
            ),
        )],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "Run")
        .expect("typed local fact");
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert!(matches!(
        fact.evidence_chain.as_slice(),
        [
            ResolutionEvidence::ExplicitReceiverType { .. },
            ResolutionEvidence::SameFileDeclaration { .. }
        ]
    ));
    Ok(())
}

#[test]
fn go_type_and_constructor_authority_requires_closed_ast_shapes() -> anyhow::Result<()> {
    let cases = [
        (
            "nested_selector.go",
            concat!(
                "package proof\n",
                "type A struct { B B }\n",
                "type B struct{}\n",
                "func (*A) Run() {}\n",
                "func (*B) Run() {}\n",
                "func caller() { x := A{B: B{}}.B; x.Run() }\n",
            ),
        ),
        (
            "double_pointer.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "func (*A) Run() {}\n",
                "func caller(x **A) { x.Run() }\n",
            ),
        ),
        (
            "parenthesized.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "func (*A) Run() {}\n",
                "func caller(x (A)) { x.Run() }\n",
            ),
        ),
        (
            "arbitrary_expression.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "func (*A) Run() {}\n",
                "func caller() { x := A{}; y := []A{x}[0]; y.Run() }\n",
            ),
        ),
    ];
    for (path, source) in cases {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[(path, source)])?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let run = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "Run")
            .unwrap_or_else(|| panic!("missing hostile constructor fact for {path}"));
        assert_ne!(
            run.status,
            ProofResolutionStatus::Exact,
            "non-closed Go type/constructor surface became Exact for {path}: {run:#?}"
        );
    }
    Ok(())
}

#[test]
fn go_relevant_range_select_and_type_switch_bindings_poison_outer_authority() -> anyhow::Result<()>
{
    let cases = [
        (
            "range_declare.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "type B struct{}\n",
                "func (*A) Run() {}\n",
                "func (*B) Run() {}\n",
                "func caller(a *A, xs []*B) { for _, a := range xs { a.Run() } }\n",
            ),
        ),
        (
            "range_assign.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "type B struct{}\n",
                "func (*A) Run() {}\n",
                "func (*B) Run() {}\n",
                "func caller(a *A, xs []*B) { for _, a = range xs { a.Run() } }\n",
            ),
        ),
        (
            "select_receive.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "type B struct{}\n",
                "func (*A) Run() {}\n",
                "func (*B) Run() {}\n",
                "func caller(a *A, ch <-chan *B) { select { case a := <-ch: a.Run() } }\n",
            ),
        ),
        (
            "type_switch.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "type B struct{}\n",
                "func (*A) Run() {}\n",
                "func (*B) Run() {}\n",
                "func caller(a *A, value any) { switch a := value.(type) { case *B: a.Run() } }\n",
            ),
        ),
    ];
    for (path, source) in cases {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[(path, source)])?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let run = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "Run")
            .unwrap_or_else(|| panic!("missing hostile lexical fact for {path}"));
        assert_ne!(
            run.status,
            ProofResolutionStatus::Exact,
            "incomplete Go lexical closure retained outer authority for {path}: {run:#?}"
        );
    }
    Ok(())
}

#[test]
fn go_source_and_owner_closure_rejects_generated_linkname_and_owner_competitors()
-> anyhow::Result<()> {
    let leading_comments = (0..21)
        .map(|index| format!("// leading comment {index}\n"))
        .collect::<String>();
    let generated = format!(
        "{leading_comments}// Code generated by fixture. DO NOT EDIT.\npackage proof\nfunc target() {{}}\nfunc caller() {{ target() }}\n"
    );
    let cases = [
        (
            vec![("generated.go", generated.as_str())],
            "target",
            "generated marker after line 20",
        ),
        (
            vec![(
                "linkname.go",
                concat!(
                    "package proof\n",
                    "import _ \"unsafe\"\n",
                    "//go:linkname target runtime.target\n",
                    "func target() {}\n",
                    "func caller() { target() }\n",
                ),
            )],
            "target",
            "go:linkname",
        ),
        (
            vec![
                (
                    "main.go",
                    concat!(
                        "package proof\n",
                        "type Worker struct{}\n",
                        "func (*Worker) Run() {}\n",
                        "func caller(worker *Worker) { worker.Run() }\n",
                    ),
                ),
                (
                    "conditional.go",
                    "//go:build linux\n\npackage proof\nvar Worker = 1\n",
                ),
            ],
            "Run",
            "conditional owner-name competitor",
        ),
    ];
    for (files, target, label) in cases {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &files)?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == target)
            .unwrap_or_else(|| panic!("missing source-closure fact for {label}"));
        assert_ne!(
            fact.status,
            ProofResolutionStatus::Exact,
            "{label} became Exact: {fact:#?}"
        );
    }

    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "string.go",
            concat!(
                "package proof\n",
                "const marker = \"// Code generated by fixture. DO NOT EDIT.\"\n",
                "func target() {}\n",
                "func caller() { target() }\n",
            ),
        )],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let target = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("string non-marker fact");
    assert_eq!(target.status, ProofResolutionStatus::Exact, "{target:#?}");
    Ok(())
}

#[test]
fn go_unsupported_shadowed_and_open_domains_never_authorize_exact_receipts() -> anyhow::Result<()> {
    let cases = [
        (
            "shadow.go",
            concat!(
                "package proof\n",
                "func target() {}\n",
                "func caller(target func()) { target() }\n",
            ),
            "target",
        ),
        (
            "interface.go",
            concat!(
                "package proof\n",
                "type Runner interface { Run() }\n",
                "func caller(r Runner) { r.Run() }\n",
            ),
            "Run",
        ),
        (
            "embedded.go",
            concat!(
                "package proof\n",
                "type Base struct{}\n",
                "func (Base) Run() {}\n",
                "type Child struct { Base }\n",
                "func caller(c Child) { c.Run() }\n",
            ),
            "Run",
        ),
        (
            "rebound.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func caller(w *Worker) { w = &Worker{}; w.Run() }\n",
            ),
            "Run",
        ),
        (
            "pointer_promotion.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func caller(w Worker) { w.Run() }\n",
            ),
            "Run",
        ),
        (
            "captured.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func caller(w *Worker) { func() { w = &Worker{} }(); w.Run() }\n",
            ),
            "Run",
        ),
        (
            "branch_write.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func caller(w *Worker, condition bool) { if condition { w = &Worker{} }; w.Run() }\n",
            ),
            "Run",
        ),
        (
            "branch_binding.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func caller(condition bool) { if condition { w := &Worker{}; w.Run() } }\n",
            ),
            "Run",
        ),
        (
            "shadowed_new.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func caller(new func(Worker) *Worker) { w := new(Worker{}); w.Run() }\n",
            ),
            "Run",
        ),
        (
            "package_new.go",
            concat!(
                "package proof\n",
                "type Worker struct{}\n",
                "func (*Worker) Run() {}\n",
                "func new(Worker) *Worker { return &Worker{} }\n",
                "func caller() { w := new(Worker{}); w.Run() }\n",
            ),
            "Run",
        ),
        (
            "generic_receiver.go",
            concat!(
                "package proof\n",
                "type Worker[T any] struct{}\n",
                "func (*Worker[T]) Run() {}\n",
                "func caller(w *Worker[int]) { w.Run() }\n",
            ),
            "Run",
        ),
        (
            "local_const_shadow.go",
            concat!(
                "package proof\n",
                "func target() {}\n",
                "func caller() { const target = 1; target() }\n",
            ),
            "target",
        ),
        (
            "local_type_shadow.go",
            concat!(
                "package proof\n",
                "func target() {}\n",
                "func caller() { type target int; target() }\n",
            ),
            "target",
        ),
        (
            "local_function_shadow.go",
            concat!(
                "package proof\n",
                "func target() {}\n",
                "func caller() { target := func() {}; target() }\n",
            ),
            "target",
        ),
        (
            "import_shadow.go",
            concat!(
                "package proof\n",
                "import target \"example.com/other\"\n",
                "func target() {}\n",
                "func caller() { target() }\n",
            ),
            "target",
        ),
        (
            "dot_import.go",
            concat!(
                "package proof\n",
                "import . \"example.com/other\"\n",
                "func target() {}\n",
                "func caller() { target() }\n",
            ),
            "target",
        ),
    ];
    for (path, source, target) in cases {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[(path, source)])?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let matching = store
            .get_proof_resolution_facts()?
            .into_iter()
            .filter(|fact| fact.provenance.language_adapter == "go")
            .filter(|fact| fact.callsite.raw_target == target)
            .collect::<Vec<_>>();
        assert!(!matching.is_empty(), "missing Go fact for {path}");
        assert!(
            matching
                .iter()
                .all(|fact| fact.status != ProofResolutionStatus::Exact),
            "unsupported Go case became Exact for {path}: {matching:#?}"
        );
    }
    Ok(())
}

#[test]
fn go_lexical_shadowing_is_callsite_specific() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "main.go",
            concat!(
                "package proof\n",
                "func target() {}\n",
                "func caller() { target(); { target := func() {}; target() }; target() }\n",
            ),
        )],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let matching = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "target")
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 3, "{matching:#?}");
    assert_ne!(matching[0].status, ProofResolutionStatus::Unsupported);
    assert_eq!(matching[1].status, ProofResolutionStatus::Unsupported);
    assert_ne!(matching[2].status, ProofResolutionStatus::Unsupported);
    assert!(
        matching
            .iter()
            .all(|fact| fact.status != ProofResolutionStatus::Exact)
    );
    Ok(())
}

#[test]
fn go_filename_selected_callers_and_competing_declarations_are_incomplete() -> anyhow::Result<()> {
    for files in [
        vec![(
            "main_linux.go",
            "package proof\nfunc target() {}\nfunc caller() { target() }\n",
        )],
        vec![
            (
                "main.go",
                "package proof\nfunc target() {}\nfunc caller() { target() }\n",
            ),
            (
                "target_windows_amd64.go",
                "package proof\nfunc target() {}\n",
            ),
        ],
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &files)?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let target = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("target fact");
        assert_eq!(
            target.status,
            ProofResolutionStatus::IncompleteDomain,
            "{target:#?}"
        );
    }
    Ok(())
}

#[test]
fn go_package_closure_and_imports_fail_closed_without_complete_authenticated_domains()
-> anyhow::Result<()> {
    for (files, target) in [
        (
            vec![
                ("a.go", "package proof\nfunc target() {}\n"),
                (
                    "b.go",
                    "package proof\nfunc target() {}\nfunc caller() { target() }\n",
                ),
            ],
            "target",
        ),
        (
            vec![
                (
                    "a.go",
                    "package proof\nfunc target() {}\nfunc caller() { target() }\n",
                ),
                (
                    "conditional.go",
                    "//go:build linux\n\npackage proof\nfunc target() {}\n",
                ),
            ],
            "target",
        ),
        (
            vec![
                ("go.mod", "module example.com/proof\n\ngo 1.24\n"),
                ("dep/dep.go", "package dep\nfunc Target() {}\n"),
                (
                    "main.go",
                    "package proof\nimport alias \"example.com/proof/dep\"\nfunc caller() { alias.Target() }\n",
                ),
            ],
            "Target",
        ),
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &files)?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let matching = store
            .get_proof_resolution_facts()?
            .into_iter()
            .filter(|fact| fact.provenance.language_adapter == "go")
            .filter(|fact| fact.callsite.raw_target == target)
            .collect::<Vec<_>>();
        assert!(!matching.is_empty(), "missing Go closure fact for {target}");
        assert!(
            matching
                .iter()
                .all(|fact| fact.status != ProofResolutionStatus::Exact),
            "open Go package/import domain became Exact: {matching:#?}"
        );
    }
    Ok(())
}

#[test]
fn go_imported_receivers_are_incomplete_without_an_authenticated_module_domain()
-> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("go.mod", "module example.com/proof\n\ngo 1.24\n"),
            (
                "dep/dep.go",
                "package dep\ntype Worker struct{}\nfunc (*Worker) Run() {}\n",
            ),
            (
                "main.go",
                concat!(
                    "package proof\n",
                    "import dep \"example.com/proof/dep\"\n",
                    "func parameter(worker *dep.Worker) { worker.Run() }\n",
                    "func constructed() { worker := &dep.Worker{}; worker.Run() }\n",
                ),
            ),
        ],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let matching = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "Run")
        .collect::<Vec<_>>();
    assert_eq!(matching.len(), 2, "{matching:#?}");
    assert!(matching.iter().all(|fact| {
        fact.status == ProofResolutionStatus::IncompleteDomain
            && fact.reason == ProofResolutionReason::LookupDomainIncomplete
    }));
    Ok(())
}

#[test]
fn go_noncompeting_conditional_and_test_siblings_preserve_the_exact_package_domain()
-> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            (
                "main.go",
                "package proof\nfunc target() {}\nfunc caller() { target() }\n",
            ),
            (
                "conditional.go",
                "//go:build linux\n\npackage proof\nfunc unrelated() {}\n",
            ),
            ("main_test.go", "package proof_test\nfunc testOnly() {}\n"),
            ("same_package_test.go", "package proof\nfunc target() {}\n"),
        ],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let target = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| {
            fact.provenance.language_adapter == "go" && fact.callsite.raw_target == "target"
        })
        .expect("target fact");
    assert_eq!(target.status, ProofResolutionStatus::Exact, "{target:#?}");
    assert_eq!(target.provenance.dependency_file_hashes.len(), 2);
    assert!(
        target
            .provenance
            .dependency_file_hashes
            .iter()
            .all(|dependency| dependency.file_id != target.callsite.file_id
                || dependency.source_sha256 == target.callsite.source_sha256)
    );
    Ok(())
}

#[test]
fn go_missing_cache_coverage_and_parser_error_siblings_make_the_package_incomplete()
-> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "main.go",
            "package proof\nfunc target() {}\nfunc caller() { target() }\n",
        )],
    )?;
    fs::write(
        project.path().join("not_indexed.go"),
        "package proof\nfunc unrelated() {}\n",
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let missing = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("missing-coverage target fact");
    assert_eq!(missing.status, ProofResolutionStatus::IncompleteDomain);

    let parse_project = tempfile::tempdir()?;
    let mut parse_store = Store::new_in_memory()?;
    index_files(
        parse_project.path(),
        &mut parse_store,
        &[
            (
                "main.go",
                "package proof\nfunc target() {}\nfunc caller() { target() }\n",
            ),
            ("broken.go", "package proof\nfunc broken( {\n"),
        ],
    )?;
    rematerialize_proof_resolution_projection(&mut parse_store, &publication(1))?;
    let parser_error = parse_store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("parser-error target fact");
    assert_eq!(parser_error.status, ProofResolutionStatus::IncompleteDomain);
    Ok(())
}

#[test]
fn go_generated_targets_and_same_named_other_owners_stay_non_authoritative() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            (
                "types.go",
                concat!(
                    "package proof\n",
                    "type A struct{}\n",
                    "type B struct{}\n",
                    "func (*A) Run() {}\n",
                    "func (*B) Run() {}\n",
                    "func caller(a *A) { a.Run() }\n",
                ),
            ),
            (
                "generated.go",
                "// Code generated by fixture. DO NOT EDIT.\npackage proof\nfunc generatedTarget() {}\nfunc generatedCaller() { generatedTarget() }\n",
            ),
        ],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    let run = facts
        .iter()
        .find(|fact| fact.callsite.raw_target == "Run")
        .expect("owner-specific method fact");
    assert_eq!(
        run.status,
        ProofResolutionStatus::Exact,
        "the exact receiver owner must disambiguate same-spelled methods: {run:#?}"
    );
    assert!(matches!(
        run.evidence_chain.as_slice(),
        [
            ResolutionEvidence::ExplicitReceiverType { .. },
            ResolutionEvidence::SameFileDeclaration { .. }
        ]
    ));
    let generated = facts
        .iter()
        .find(|fact| fact.callsite.raw_target == "generatedTarget")
        .expect("generated target fact");
    assert_ne!(generated.status, ProofResolutionStatus::Exact);
    Ok(())
}

#[test]
fn go_repeated_callsites_and_utf8_crlf_coordinates_are_source_bound_and_distinct()
-> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    let source =
        "package proof\r\n// λλ\r\nfunc target() {}\r\nfunc caller() { target(); target() }\r\n";
    index_files(project.path(), &mut store, &[("main.go", source)])?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let mut facts = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "target")
        .collect::<Vec<_>>();
    facts.sort_by_key(|fact| fact.callsite.start_byte);
    assert_eq!(facts.len(), 2, "{facts:#?}");
    assert!(
        facts
            .iter()
            .all(|fact| fact.status == ProofResolutionStatus::Exact)
    );
    assert_ne!(facts[0].edge_id, facts[1].edge_id);
    assert_ne!(
        facts[0].raw_callsite_identity,
        facts[1].raw_callsite_identity
    );
    for fact in &facts {
        assert_eq!(
            &source.as_bytes()
                [fact.callsite.start_byte as usize..fact.callsite.end_byte_exclusive as usize],
            b"target"
        );
        assert_eq!(fact.callsite.line, 4);
    }
    assert!(facts[0].callsite.column < facts[1].callsite.column);
    Ok(())
}

#[test]
fn go_authenticated_receiver_replaces_wrong_certain_navigation_target() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "main.go",
            concat!(
                "package proof\n",
                "type A struct{}\n",
                "type B struct{}\n",
                "func (*A) Run() {}\n",
                "func (*B) Run() {}\n",
                "func caller(a *A) { a.Run() }\n",
            ),
        )],
    )?;
    let nodes = store.get_nodes()?;
    let right_target = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::METHOD && node.serialized_name.ends_with("A.Run"))
        .map(|node| node.id)
        .next()
        .expect("A.Run target");
    let wrong_target = nodes
        .iter()
        .filter(|node| node.kind == NodeKind::METHOD && node.serialized_name.ends_with("B.Run"))
        .map(|node| node.id)
        .next()
        .or_else(|| {
            nodes
                .iter()
                .filter(|node| {
                    node.kind == NodeKind::METHOD && node.serialized_name.ends_with("Run")
                })
                .nth(1)
                .map(|node| node.id)
        })
        .expect("B.Run target");
    let call = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL && edge.line == Some(6))
        .expect("receiver CALL edge");
    store.get_connection().execute(
        "UPDATE edge SET resolved_target_node_id = ?1, certainty = 1, confidence = 1.0 WHERE id = ?2",
        (wrong_target.0, call.id.0),
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "Run")
        .expect("mutated Run fact");
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert_eq!(fact.target, Some(right_target));
    assert_eq!(fact.edge_id, Some(call.id));
    let projected = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.id == call.id)
        .expect("projected receiver CALL edge");
    assert_eq!(projected.resolved_target, Some(right_target));
    assert_eq!(projected.confidence, Some(1.0));
    assert_eq!(projected.certainty, Some(ResolutionCertainty::Certain));
    assert!(projected.candidate_targets.is_empty());
    Ok(())
}

#[test]
fn javascript_and_typescript_name_specific_callable_bindings_are_exact() -> anyhow::Result<()> {
    for (path, source) in [
        (
            "src/module.js",
            "export const target = value => value;\nexport function caller() { target(1); }\n",
        ),
        (
            "src/module.ts",
            "type Noise = { value: number };\nclass Unrelated {}\nconst unrelated = 1;\nexport const target = (value: number) => value;\nexport const caller = () => { if (unrelated) { target(1); } };\n",
        ),
        (
            "src/module.tsx",
            "type Noise = { value: number };\nexport const target = (value: number) => value;\nexport function caller() { const unrelated = <div />; target(1); }\n",
        ),
        (
            "src/module.jsx",
            "export function target(value) { return value; }\nexport function caller() { target(<div />); }\n",
        ),
    ] {
        assert_only_call_is_exact(&[(path, source)])?;
    }
    Ok(())
}

#[test]
fn javascript_and_typescript_direct_imports_are_exact() -> anyhow::Result<()> {
    for files in [
        vec![
            (
                "src/exported.js",
                "export function target(value) { return value; }\n",
            ),
            (
                "src/importer.js",
                "import { target } from './exported';\nexport function caller() { target(1); }\n",
            ),
        ],
        vec![
            (
                "src/exported.ts",
                "export const target = (value: number) => value;\n",
            ),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(1); }\n",
            ),
        ],
        vec![
            (
                "src/exported.jsx",
                "export function target(value) { return <>{value}</>; }\n",
            ),
            (
                "src/importer.js",
                "import { target } from './exported';\nexport function caller() { target(1); }\n",
            ),
        ],
        vec![
            (
                "src/exported.tsx",
                "export const target = (value: number) => <>{value}</>;\n",
            ),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(1); }\n",
            ),
        ],
        vec![
            ("src/exported.mjs", "export default function target() {}\n"),
            (
                "src/importer.mjs",
                "import target from './exported.mjs';\nexport function caller() { target(); }\n",
            ),
        ],
    ] {
        if let Err(error) = assert_only_call_is_exact(&files) {
            panic!("direct import fixture {files:?} failed: {error:#}");
        }
    }
    Ok(())
}

#[test]
fn relative_module_resolution_uses_one_closed_language_family() -> anyhow::Result<()> {
    for files in [
        vec![
            ("src/exported.mts", "export function target() {}\n"),
            (
                "src/importer.mts",
                "import { target } from './exported.mts';\nexport function caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/exported.cts", "export function target() {}\n"),
            (
                "src/importer.cts",
                "import { target } from './exported.cts';\nexport function caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/exported.cjs", "export function target() {}\n"),
            (
                "src/importer.cjs",
                "import { target } from './exported.cjs';\nexport function caller() { target(); }\n",
            ),
        ],
        vec![
            (
                "src/a.ts",
                "import { target } from './b';\nexport function helper() {}\nexport function caller() { target(); }\n",
            ),
            (
                "src/b.ts",
                "import { helper } from './a';\nexport function target() { helper(); }\n",
            ),
        ],
    ] {
        assert_only_call_is_exact(&files)?;
    }

    for files in [
        vec![(
            "src/importer.ts",
            "import { target } from './missing';\nexport function caller() { target(); }\n",
        )],
        vec![
            ("src/exported.ts", "export function target() {}\n"),
            ("src/exported.tsx", "export function target() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/exported/index.ts", "export function target() {}\n"),
            ("src/exported/index.tsx", "export function target() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/exported.js", "export function target() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/exported.ts", "export function target() {}\n"),
            (
                "src/importer.js",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ],
        vec![
            (
                "src/exported.d.ts",
                "export declare function target(): void;\n",
            ),
            (
                "src/importer.ts",
                "import { target } from './exported.d.ts';\nexport function caller() { target(); }\n",
            ),
        ],
    ] {
        assert_only_call_is_not_exact(&files)?;
    }
    Ok(())
}

#[test]
fn supported_import_alias_still_requires_an_ordinary_resolved_call_edge() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[
        ("src/exported.ts", "export function actual() {}\n"),
        (
            "src/importer.ts",
            "import { actual as target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])
}

#[test]
fn authenticated_direct_import_projects_one_existing_call_edge_before_exact_replay()
-> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("src/right.ts", "export function importedTarget() {}\n"),
            (
                "src/caller.ts",
                "import { importedTarget } from './right';\nexport function caller() { importedTarget(); }\n",
            ),
        ],
    )?;
    let initial = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL)
        .expect("ordinary CALL edge");
    let nodes = store.get_nodes()?;
    let caller = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == "caller")
        .expect("authenticated caller")
        .id;
    let target = nodes
        .iter()
        .find(|node| {
            node.kind == NodeKind::FUNCTION
                && node.serialized_name == "importedTarget"
                && node
                    .file_node_id
                    .and_then(|id| store.get_node(id).ok().flatten())
                    .is_some_and(|file| file.serialized_name.ends_with("right.ts"))
        })
        .expect("authenticated target")
        .id;
    let wrong = caller;
    store.get_connection().execute(
        "UPDATE edge
         SET resolved_target_node_id = ?1,
             confidence = 0.5,
             certainty = 'certain',
             candidate_target_node_ids = ?2
         WHERE id = ?3",
        (wrong.0, format!("[{}]", wrong.0), initial.id.0),
    )?;
    let before = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.id == initial.id)
        .expect("tampered ordinary CALL edge");

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;

    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "importedTarget")
        .expect("direct import call fact");
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert_eq!(fact.edge_id, Some(before.id));
    assert_eq!(fact.caller, caller);
    assert_eq!(fact.target, Some(target));

    let after = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.id == before.id)
        .expect("projected CALL edge");
    assert_eq!(after.source, before.source, "raw source is immutable");
    assert_eq!(after.target, before.target, "raw target is immutable");
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.file_node_id, before.file_node_id);
    assert_eq!(after.line, before.line);
    assert_eq!(after.callsite_identity, before.callsite_identity);
    assert_eq!(after.resolved_source, Some(caller));
    assert_eq!(after.resolved_target, Some(target));
    assert_eq!(after.certainty, Some(ResolutionCertainty::Certain));
    assert_eq!(after.confidence, Some(1.0));
    assert!(after.candidate_targets.is_empty());
    Ok(())
}

#[test]
fn go_same_package_helper_projects_one_existing_call_edge_before_exact_replay() -> anyhow::Result<()>
{
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("target.go", "package proof\nfunc target() {}\n"),
            ("caller.go", "package proof\nfunc caller() { target() }\n"),
        ],
    )?;
    let nodes = store.get_nodes()?;
    let caller = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == "caller")
        .expect("authenticated Go caller")
        .id;
    let target = nodes
        .iter()
        .find(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == "target")
        .expect("authenticated Go target")
        .id;
    let before = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL)
        .expect("source-built Go CALL edge");
    store.get_connection().execute(
        "UPDATE edge
         SET resolved_target_node_id = ?1,
             confidence = 0.5,
             certainty = 'certain',
             candidate_target_node_ids = ?2
         WHERE id = ?3",
        (caller.0, format!("[{}]", caller.0), before.id.0),
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;

    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("Go helper call fact");
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert_eq!(fact.edge_id, Some(before.id));
    assert_eq!(fact.caller, caller);
    assert_eq!(fact.target, Some(target));
    let after = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.id == before.id)
        .expect("projected Go CALL edge");
    assert_eq!(after.source, before.source);
    assert_eq!(after.target, before.target);
    assert_eq!(after.kind, before.kind);
    assert_eq!(after.file_node_id, before.file_node_id);
    assert_eq!(after.line, before.line);
    assert_eq!(after.callsite_identity, before.callsite_identity);
    assert_eq!(after.resolved_source, Some(caller));
    assert_eq!(after.resolved_target, Some(target));
    assert_eq!(after.certainty, Some(ResolutionCertainty::Certain));
    assert_eq!(after.confidence, Some(1.0));
    assert!(after.candidate_targets.is_empty());
    Ok(())
}

#[test]
fn relative_module_resolution_preserves_native_case_identity() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("src/exported.ts", "export function target() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './Exported';\nexport function caller() { target(); }\n",
            ),
        ],
    )?;
    let native_spelling_exists = project.path().join("src/Exported.ts").exists();
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("target call fact");
    assert_eq!(
        fact.status == ProofResolutionStatus::Exact,
        native_spelling_exists,
        "module equality must follow native filesystem identity: {fact:#?}"
    );
    Ok(())
}

#[test]
fn javascript_and_typescript_closed_receiver_bindings_are_exact() -> anyhow::Result<()> {
    for (path, source, expected_evidence) in [
        (
            "src/implicit.js",
            "export class C { target() {} caller() { this.target(); } }\n",
            vec![
                ResolutionEvidenceKind::ImplicitReceiver,
                ResolutionEvidenceKind::SameFileDeclaration,
            ],
        ),
        (
            "src/implicit.ts",
            "export class C { target() {} constructor() { this.target(); } }\n",
            vec![
                ResolutionEvidenceKind::ImplicitReceiver,
                ResolutionEvidenceKind::SameFileDeclaration,
            ],
        ),
        (
            "src/constructor.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
            vec![
                ResolutionEvidenceKind::ConstructorBinding,
                ResolutionEvidenceKind::ExplicitReceiverType,
                ResolutionEvidenceKind::SameFileDeclaration,
            ],
        ),
        (
            "src/constructor.tsx",
            "export class C { target() {} }\nexport const caller = () => { const receiver = new C(); receiver.target(); };\n",
            vec![
                ResolutionEvidenceKind::ConstructorBinding,
                ResolutionEvidenceKind::ExplicitReceiverType,
                ResolutionEvidenceKind::SameFileDeclaration,
            ],
        ),
        (
            "src/typed.ts",
            "export class C { target() {} }\nexport function caller(receiver: C) { receiver.target(); }\n",
            vec![
                ResolutionEvidenceKind::ExplicitReceiverType,
                ResolutionEvidenceKind::SameFileDeclaration,
            ],
        ),
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[(path, source)])?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let facts = store.get_proof_resolution_facts()?;
        let fact = facts
            .iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .unwrap_or_else(|| panic!("missing receiver fact: {facts:#?}"));
        assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
        assert_eq!(
            fact.evidence_chain
                .iter()
                .map(ResolutionEvidence::kind)
                .collect::<Vec<_>>(),
            expected_evidence,
            "{fact:#?}"
        );
    }
    Ok(())
}

#[test]
fn imported_constructor_and_typed_receiver_bindings_are_exact() -> anyhow::Result<()> {
    for (files, expected_prefix) in [
        (
            vec![
                ("src/exported.js", "export class C { target() {} }\n"),
                (
                    "src/importer.js",
                    "import { C } from './exported';\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
                ),
            ],
            ResolutionEvidenceKind::ConstructorBinding,
        ),
        (
            vec![
                (
                    "src/exported.ts",
                    "export default class C { target() {} }\n",
                ),
                (
                    "src/importer.ts",
                    "import C from './exported';\nexport function caller(receiver: C) { receiver.target(); }\n",
                ),
            ],
            ResolutionEvidenceKind::ExplicitReceiverType,
        ),
        (
            vec![
                ("src/exported.ts", "export class C { target() {} }\n"),
                (
                    "src/importer.ts",
                    "import { C } from './exported';\nexport function caller(receiver: C) { receiver.target(); }\n",
                ),
            ],
            ResolutionEvidenceKind::ExplicitReceiverType,
        ),
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &files)?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let facts = store.get_proof_resolution_facts()?;
        let fact = facts
            .iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .unwrap_or_else(|| panic!("missing imported receiver fact: {facts:#?}"));
        assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
        assert_eq!(
            fact.evidence_chain.first().map(ResolutionEvidence::kind),
            Some(ResolutionEvidenceKind::StaticImportBinding),
            "{fact:#?}"
        );
        assert!(
            fact.evidence_chain
                .iter()
                .any(|evidence| evidence.kind() == expected_prefix),
            "{fact:#?}"
        );
    }
    Ok(())
}

#[test]
fn import_corroboration_downgrades_missing_wrong_and_duplicate_relations_without_rejecting_projection()
-> anyhow::Result<()> {
    for mutation in [
        RelationMutation::Missing,
        RelationMutation::Wrong,
        RelationMutation::Duplicate,
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[
                ("src/exported.ts", "export function importedTarget() {}\n"),
                (
                    "src/importer.ts",
                    "import { importedTarget } from './exported';\nexport function localTarget() {}\nexport function caller() { importedTarget(); localTarget(); }\n",
                ),
            ],
        )?;
        let target = store
            .get_nodes()?
            .into_iter()
            .find(|node| {
                node.kind == NodeKind::FUNCTION && node.serialized_name == "importedTarget"
            })
            .expect("import target");
        let relation = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::IMPORT && edge.effective_target() == target.id)
            .expect("import relation");
        mutate_relation(&mut store, &relation, mutation)?;

        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        store.validate_proof_resolution_publication(&publication(1))?;
        let facts = store.get_proof_resolution_facts()?;
        let imported = facts
            .iter()
            .find(|fact| fact.callsite.raw_target == "importedTarget")
            .expect("imported fact");
        let local = facts
            .iter()
            .find(|fact| fact.callsite.raw_target == "localTarget")
            .expect("local fact");
        assert_eq!(
            imported.status,
            ProofResolutionStatus::IncompleteDomain,
            "{imported:#?}"
        );
        assert_eq!(imported.target, None);
        assert!(imported.evidence_chain.is_empty());
        assert_eq!(local.status, ProofResolutionStatus::Exact, "{local:#?}");
    }
    Ok(())
}

#[test]
fn imported_receiver_corroboration_requires_unique_import_and_member_relations()
-> anyhow::Result<()> {
    for (relation_kind, mutation, constructor_binding) in [
        (EdgeKind::IMPORT, RelationMutation::Missing, true),
        (EdgeKind::IMPORT, RelationMutation::Wrong, false),
        (EdgeKind::IMPORT, RelationMutation::Duplicate, true),
        (EdgeKind::MEMBER, RelationMutation::Missing, false),
        (EdgeKind::MEMBER, RelationMutation::Wrong, true),
        (EdgeKind::MEMBER, RelationMutation::Duplicate, false),
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[
                ("src/exported.ts", "export class C { target() {} }\n"),
                (
                    "src/importer.ts",
                    if constructor_binding {
                        "import { C } from './exported';\nexport function caller() { const receiver = new C(); receiver.target(); }\n"
                    } else {
                        "import { C } from './exported';\nexport function caller(receiver: C) { receiver.target(); }\n"
                    },
                ),
            ],
        )?;
        let nodes = store.get_nodes()?;
        let owner = nodes
            .iter()
            .find(|node| node.kind == NodeKind::CLASS && node.serialized_name == "C")
            .expect("class owner");
        let method = nodes
            .iter()
            .find(|node| node.kind == NodeKind::METHOD && node.serialized_name.ends_with(".target"))
            .expect("target method");
        let relation = store
            .get_edges()?
            .into_iter()
            .find(|edge| match relation_kind {
                EdgeKind::IMPORT => {
                    edge.kind == EdgeKind::IMPORT && edge.effective_target() == owner.id
                }
                EdgeKind::MEMBER => {
                    edge.kind == EdgeKind::MEMBER
                        && edge.effective_source() == owner.id
                        && edge.effective_target() == method.id
                }
                _ => false,
            })
            .expect("receiver evidence relation");
        mutate_relation(&mut store, &relation, mutation)?;

        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("receiver fact");
        assert_eq!(
            fact.status,
            ProofResolutionStatus::IncompleteDomain,
            "{fact:#?}"
        );
        assert_eq!(fact.target, None);
        assert!(fact.evidence_chain.is_empty());
    }
    Ok(())
}

#[test]
fn local_and_implicit_receiver_corroboration_requires_unique_member_relations() -> anyhow::Result<()>
{
    for (source, caller_member, mutation) in [
        (
            "export class C { target() {} }\nexport function caller(receiver: C) { receiver.target(); }\n",
            false,
            RelationMutation::Missing,
        ),
        (
            "export class C { target() {} }\nexport function caller(receiver: C) { receiver.target(); }\n",
            false,
            RelationMutation::Wrong,
        ),
        (
            "export class C { target() {} }\nexport function caller(receiver: C) { receiver.target(); }\n",
            false,
            RelationMutation::Duplicate,
        ),
        (
            "export class C { target() {} caller() { this.target(); } }\n",
            true,
            RelationMutation::Missing,
        ),
        (
            "export class C { target() {} caller() { this.target(); } }\n",
            true,
            RelationMutation::Wrong,
        ),
        (
            "export class C { target() {} caller() { this.target(); } }\n",
            true,
            RelationMutation::Duplicate,
        ),
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[("src/local.ts", source)])?;
        let nodes = store.get_nodes()?;
        let owner = nodes
            .iter()
            .find(|node| node.kind == NodeKind::CLASS && node.serialized_name == "C")
            .expect("class owner");
        let member = nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::METHOD
                    && node.serialized_name.ends_with(if caller_member {
                        ".caller"
                    } else {
                        ".target"
                    })
            })
            .expect("member relation target");
        let relation = store
            .get_edges()?
            .into_iter()
            .find(|edge| {
                edge.kind == EdgeKind::MEMBER
                    && edge.effective_source() == owner.id
                    && edge.effective_target() == member.id
            })
            .expect("local member relation");
        mutate_relation(&mut store, &relation, mutation)?;

        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("receiver fact");
        assert_eq!(
            fact.status,
            ProofResolutionStatus::IncompleteDomain,
            "{fact:#?}"
        );
        assert_eq!(fact.target, None);
        assert!(fact.evidence_chain.is_empty());
    }
    Ok(())
}

#[test]
fn rust_import_qualified_and_inherent_evidence_requires_unique_graph_relations()
-> anyhow::Result<()> {
    for (family, relation_kind, mutation) in [
        ("import", EdgeKind::IMPORT, RelationMutation::Missing),
        ("import", EdgeKind::IMPORT, RelationMutation::Wrong),
        ("import", EdgeKind::IMPORT, RelationMutation::Duplicate),
        (
            "import",
            EdgeKind::IMPORT,
            RelationMutation::CandidateRetained,
        ),
        (
            "import",
            EdgeKind::IMPORT,
            RelationMutation::RecoveredSource,
        ),
        ("import", EdgeKind::IMPORT, RelationMutation::WrongFile),
        ("qualified", EdgeKind::MEMBER, RelationMutation::Missing),
        ("qualified", EdgeKind::MEMBER, RelationMutation::Wrong),
        ("qualified", EdgeKind::MEMBER, RelationMutation::Duplicate),
        (
            "qualified",
            EdgeKind::MEMBER,
            RelationMutation::CandidateRetained,
        ),
        (
            "qualified",
            EdgeKind::MEMBER,
            RelationMutation::RecoveredMemberSource,
        ),
        (
            "qualified",
            EdgeKind::MEMBER,
            RelationMutation::RecoveredMemberTarget,
        ),
        ("qualified", EdgeKind::MEMBER, RelationMutation::WrongFile),
        ("inherent", EdgeKind::MEMBER, RelationMutation::Missing),
        ("inherent", EdgeKind::MEMBER, RelationMutation::Wrong),
        ("inherent", EdgeKind::MEMBER, RelationMutation::Duplicate),
        (
            "inherent",
            EdgeKind::MEMBER,
            RelationMutation::CandidateRetained,
        ),
        (
            "inherent",
            EdgeKind::MEMBER,
            RelationMutation::RecoveredMemberSource,
        ),
        (
            "inherent",
            EdgeKind::MEMBER,
            RelationMutation::RecoveredMemberTarget,
        ),
        ("inherent", EdgeKind::MEMBER, RelationMutation::WrongFile),
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        let files = match family {
            "import" => vec![
                ("src/target.rs", "pub fn target() {}\n"),
                (
                    "src/lib.rs",
                    "mod target;\nuse crate::target::target;\nfn caller() { target(); }\n",
                ),
            ],
            "qualified" => vec![(
                "src/lib.rs",
                "mod nested { pub fn target() {} }\nfn caller() { crate::nested::target(); }\n",
            )],
            _ => vec![(
                "src/lib.rs",
                "struct Owner;\nimpl Owner { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
            )],
        };
        index_files(project.path(), &mut store, &files)?;
        let nodes = store.get_nodes()?;
        let target = nodes
            .iter()
            .find(|node| {
                matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                    && node.serialized_name.ends_with("target")
            })
            .expect("target node");
        let relation = store
            .get_edges()?
            .into_iter()
            .find(|edge| {
                edge.kind == relation_kind
                    && edge.effective_target() == target.id
                    && (family != "inherent" || edge.effective_source() != target.id)
            })
            .expect("Rust evidence relation");
        mutate_relation(&mut store, &relation, mutation)?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("target fact");
        assert_eq!(
            fact.status,
            ProofResolutionStatus::IncompleteDomain,
            "{family}: {fact:#?}"
        );
        assert!(fact.evidence_chain.is_empty(), "{family}: {fact:#?}");
    }
    Ok(())
}

#[test]
fn rust_imported_implicit_receiver_requires_literal_import_owner_and_members() -> anyhow::Result<()>
{
    for (relation_role, mutation) in [
        ("import", RelationMutation::Missing),
        ("import", RelationMutation::Wrong),
        ("import", RelationMutation::Duplicate),
        ("import", RelationMutation::CandidateRetained),
        ("import", RelationMutation::RecoveredSource),
        ("import", RelationMutation::WrongFile),
        ("import", RelationMutation::WrongSourceKind),
        ("import", RelationMutation::WrongTargetKind),
        ("caller_member", RelationMutation::Missing),
        ("caller_member", RelationMutation::Wrong),
        ("caller_member", RelationMutation::Duplicate),
        ("caller_member", RelationMutation::CandidateRetained),
        ("caller_member", RelationMutation::RecoveredMemberSource),
        ("caller_member", RelationMutation::RecoveredMemberTarget),
        ("caller_member", RelationMutation::WrongFile),
        ("caller_member", RelationMutation::WrongSourceKind),
        ("caller_member", RelationMutation::WrongTargetKind),
        ("caller_member", RelationMutation::WrongTargetOwnership),
        ("target_member", RelationMutation::Missing),
        ("target_member", RelationMutation::Wrong),
        ("target_member", RelationMutation::Duplicate),
        ("target_member", RelationMutation::CandidateRetained),
        ("target_member", RelationMutation::RecoveredMemberSource),
        ("target_member", RelationMutation::RecoveredMemberTarget),
        ("target_member", RelationMutation::WrongFile),
        ("target_member", RelationMutation::WrongSourceKind),
        ("target_member", RelationMutation::WrongTargetKind),
        ("target_member", RelationMutation::WrongTargetOwnership),
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[
                ("src/owner.rs", "pub struct Owner;\n"),
                (
                    "src/lib.rs",
                    "mod owner;\nuse crate::owner::Owner;\nimpl Owner { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
                ),
            ],
        )?;
        let nodes = store.get_nodes()?;
        let owner = nodes
            .iter()
            .find(|node| node.kind == NodeKind::STRUCT && node.serialized_name == "Owner")
            .expect("imported owner")
            .id;
        let import = nodes
            .iter()
            .find(|node| {
                node.kind == NodeKind::MODULE
                    && node.serialized_name.ends_with(" (import)")
                    && node
                        .serialized_name
                        .trim_end_matches(" (import)")
                        .rsplit(['.', ':'])
                        .find(|part| !part.is_empty())
                        == Some("Owner")
            })
            .expect("Owner import node")
            .id;
        let caller = nodes
            .iter()
            .find(|node| node.kind == NodeKind::METHOD && node.serialized_name.ends_with("caller"))
            .expect("caller method")
            .id;
        let target = nodes
            .iter()
            .find(|node| node.kind == NodeKind::METHOD && node.serialized_name.ends_with("target"))
            .expect("target method")
            .id;
        let relation = store
            .get_edges()?
            .into_iter()
            .find(|edge| match relation_role {
                "import" => {
                    edge.kind == EdgeKind::IMPORT
                        && edge.source == import
                        && edge.effective_source() == import
                        && edge.resolved_target == Some(owner)
                        && edge.effective_target() == owner
                        && edge.candidate_targets.is_empty()
                }
                "caller_member" => {
                    edge.kind == EdgeKind::MEMBER
                        && edge.source == owner
                        && edge.target == caller
                        && edge.effective_source() == owner
                        && edge.effective_target() == caller
                        && edge.candidate_targets.is_empty()
                }
                _ => {
                    edge.kind == EdgeKind::MEMBER
                        && edge.source == owner
                        && edge.target == target
                        && edge.effective_source() == owner
                        && edge.effective_target() == target
                        && edge.candidate_targets.is_empty()
                }
            })
            .expect("imported S4 evidence relation");
        mutate_relation(&mut store, &relation, mutation)?;
        if let Err(error) = rematerialize_proof_resolution_projection(&mut store, &publication(1)) {
            assert!(
                matches!(
                    mutation,
                    RelationMutation::WrongTargetKind | RelationMutation::WrongTargetOwnership
                ),
                "unexpected imported S4 rematerialization error for {relation_role}/{mutation:?}: {error:#}"
            );
            continue;
        }
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("imported S4 fact");
        assert_eq!(
            fact.status,
            ProofResolutionStatus::IncompleteDomain,
            "{relation_role}/{mutation:?}: {fact:#?}"
        );
        assert!(
            fact.evidence_chain.is_empty(),
            "{relation_role}/{mutation:?}: {fact:#?}"
        );
    }
    Ok(())
}

#[test]
fn optional_typescript_parameters_are_not_receiver_authority() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[(
        "src/optional.ts",
        "export class C { target() {} }\nexport function caller(receiver?: C) { receiver.target(); }\n",
    )])
}

#[test]
fn receiver_class_and_mutation_domains_fail_closed() -> anyhow::Result<()> {
    for (path, source) in [
        (
            "src/derived.ts",
            "class Base { target() {} }\nexport class C extends Base { target() {} caller() { this.target(); } }\n",
        ),
        (
            "src/super.js",
            "class Base { target() {} }\nexport class C extends Base { caller() { super.target(); } }\n",
        ),
        (
            "src/duplicate.js",
            "export class C { target() {} target() {} caller() { this.target(); } }\n",
        ),
        (
            "src/nested.js",
            "export class C { target() {} caller() { function nested() { this.target(); } nested(); } }\n",
        ),
        (
            "src/let_receiver.js",
            "export class C { target() {} }\nexport function caller() { let receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/reassigned.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/prototype.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\nC.prototype.target = function () {};\n",
        ),
        (
            "src/member_write.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); receiver.target = () => {}; }\n",
        ),
        (
            "src/decorated.ts",
            "function sealed(value: unknown) { return value; }\n@sealed export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/static.js",
            "export class C { static target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/private.js",
            "export class C { #target() {} caller() { this.#target(); } }\n",
        ),
        (
            "src/computed.js",
            "export class C { ['target']() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/accessor_target.js",
            "export class C { get target() { return () => {}; } }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/generator_target.js",
            "export class C { *target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/static_caller.js",
            "export class C { static target() {} static caller() { this.target(); } }\n",
        ),
        (
            "src/shadowed_receiver.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); { const receiver = {}; receiver.target(); } }\n",
        ),
        (
            "src/factory.js",
            "export class C { target() {} }\nfunction make() { return new C(); }\nexport function caller() { const receiver = make(); receiver.target(); }\n",
        ),
        (
            "src/constructor_alias.js",
            "export class C { target() {} }\nconst Alias = C;\nexport function caller() { const receiver = new Alias(); receiver.target(); }\n",
        ),
        (
            "src/object_assign.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\nObject.assign(C.prototype, { target() {} });\n",
        ),
        (
            "src/define_property.js",
            "export class C { target() {} }\nObject.defineProperty(C.prototype, 'target', { value() {} });\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/set_prototype.js",
            "export class C { target() {} }\nObject.setPrototypeOf(C.prototype, {});\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        ),
        (
            "src/union.ts",
            "class D { target() {} }\nexport class C { target() {} }\nexport function caller(receiver: C | D) { receiver.target(); }\n",
        ),
        (
            "src/intersection.ts",
            "export class C { target() {} }\ntype Extra = { value: number };\nexport function caller(receiver: C & Extra) { receiver.target(); }\n",
        ),
        (
            "src/generic_type.ts",
            "export class C<T> { target() {} }\nexport function caller(receiver: C<number>) { receiver.target(); }\n",
        ),
        (
            "src/interface.ts",
            "interface C { target(): void }\nexport function caller(receiver: C) { receiver.target(); }\n",
        ),
        (
            "src/alias.ts",
            "export class Actual { target() {} }\ntype C = Actual;\nexport function caller(receiver: C) { receiver.target(); }\n",
        ),
        (
            "src/any.ts",
            "export function caller(receiver: any) { receiver.target(); }\n",
        ),
        (
            "src/unknown.ts",
            "export function caller(receiver: unknown) { receiver.target(); }\n",
        ),
    ] {
        assert_no_exact_target_calls(&[(path, source)])?;
    }
    assert_only_call_is_not_exact(&[
        ("src/exported.ts", "export class C { target() {} }\n"),
        (
            "src/importer.ts",
            "import { C } from './exported';\nclass C { target() {} }\nexport function caller(receiver: C) { receiver.target(); }\n",
        ),
    ])?;
    Ok(())
}

#[test]
fn receiver_mutation_domains_are_structural_and_exactly_keyed() -> anyhow::Result<()> {
    for source in [
        "class Other { target() {} }\nexport class C { target() {} }\nexport function caller() { const receiver = new C(); C = Other; receiver.target(); }\n",
        "export class C { target() {} }\nexport function caller() { const receiver = new C(); Object.defineProperty(receiver, 'target', { value() {} }); receiver.target(); }\n",
        "export class C { target() {} }\nexport function caller() { const receiver = new C(); Object.assign(receiver, { target() {} }); receiver.target(); }\n",
        "export class C { target() {} }\nexport function caller(key) { const receiver = new C(); C.prototype[key] = () => {}; receiver.target(); }\n",
        "export class C { target() {} }\nexport function caller(key) { const receiver = new C(); receiver[key] = () => {}; receiver.target(); }\n",
        "export class C { target() {} }\nexport function caller(key) { const receiver = new C(); Object.defineProperty(C.prototype, key, { value() {} }); receiver.target(); }\n",
        "export class C { target() {} }\nexport function caller() { const receiver = new C(); C.prototype = {}; receiver.target(); }\n",
        "export class C { target() {} }\nexport function caller(key) { const receiver = new C(); C[key] = {}; receiver.target(); }\n",
    ] {
        assert_no_exact_target_calls(&[("src/mutated.js", source)])?;
    }

    assert_only_call_is_exact(&[(
        "src/unrelated.js",
        "class Cat {}\nexport class C { target() {} }\nObject.defineProperty(Cat.prototype, 'retarget', { value() {} });\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
    )])?;
    assert_only_call_is_exact(&[(
        "src/sibling_mutation.js",
        "export class C { target() {} }\nexport function noisy() { const receiver = new C(); Object.assign(receiver, {}); }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
    )])?;
    Ok(())
}

#[test]
fn javascript_binding_and_method_modifiers_use_parser_tokens() -> anyhow::Result<()> {
    assert_only_call_is_exact(&[(
        "src/commented_const.js",
        "export const/*comment*/ target = () => 1;\nexport function caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/commented_var.js",
        "export function target() {}\nexport function caller() { { var/*comment*/ target = () => {}; } target(); }\n",
    )])?;
    for source in [
        "export class C { static/*comment*/ target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        "export class C { get/*comment*/ target() { return () => {}; } }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
        "export class C { */*comment*/ target() {} }\nexport function caller() { const receiver = new C(); receiver.target(); }\n",
    ] {
        assert_no_exact_target_calls(&[("src/commented_method.js", source)])?;
    }
    Ok(())
}

#[test]
fn reexports_only_poison_the_names_they_can_compete_with() -> anyhow::Result<()> {
    for exporter in [
        "export function target() {}\nexport * from './other';\n",
        "export function target() {}\nexport { target } from './other';\n",
        "export function target() {}\nexport { other as target } from './other';\n",
    ] {
        assert_only_call_is_not_exact(&[
            ("src/exported.ts", exporter),
            ("src/other.ts", "export function target() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ])?;
    }
    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "export function target() {}\nexport const unrelated = target;\nexport { other } from './other';\n",
        ),
        ("src/other.ts", "export function other() {}\n"),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "export function target() {}\nexport { target as other };\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    Ok(())
}

#[test]
fn dynamic_breakers_are_scoped_to_the_governing_lexical_domain() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/scoped.js",
            "export function target() {}\nexport function noisy() { eval('target'); target(); }\nexport function safe() { target(); }\n",
        )],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let mut target_facts = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "target")
        .collect::<Vec<_>>();
    target_facts.sort_by_key(|fact| fact.callsite.line);
    assert_eq!(target_facts.len(), 2, "{target_facts:#?}");
    assert_ne!(target_facts[0].status, ProofResolutionStatus::Exact);
    assert_eq!(target_facts[1].status, ProofResolutionStatus::Exact);

    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "export function target() { eval('local'); }\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[(
        "src/governing.js",
        "export function target() {}\nexport function caller(object) { with (object) {} target(); }\n",
    )])?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn broken_relative_import_candidate_is_incomplete_not_absent() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        )],
    )?;
    symlink("missing-target.ts", project.path().join("src/exported.ts"))?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("target call fact");
    assert_eq!(
        fact.status,
        ProofResolutionStatus::IncompleteDomain,
        "{fact:#?}"
    );
    assert_eq!(fact.target, None);
    Ok(())
}

#[test]
fn rust_same_line_repeated_calls_correlate_to_distinct_ordinary_edges() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/lib.rs",
            "fn target() {}\nfn source() { target(); target(); }\n",
        )],
    )?;

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let repeated = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "target")
        .collect::<Vec<_>>();

    assert_eq!(repeated.len(), 2, "one syntax fact per repeated callsite");
    assert!(
        repeated
            .iter()
            .all(|fact| fact.status == ProofResolutionStatus::Exact),
        "both parser-derived callsites must correlate independently: {repeated:#?}"
    );
    assert_eq!(
        repeated
            .iter()
            .filter_map(|fact| fact.edge_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        2,
        "each Exact syntax fact needs its own ordinary CALL edge"
    );
    assert_eq!(
        repeated
            .iter()
            .map(|fact| (fact.callsite.start_byte, fact.callsite.column))
            .collect::<Vec<_>>(),
        [(29, 15), (39, 25)]
    );
    Ok(())
}

#[test]
fn rust_exact_callsites_use_source_bound_utf8_byte_coordinates() -> anyhow::Result<()> {
    for source in [
        "fn target() {}\r\nfn caller() {\r\n\t/* é */ target();\r\n}",
        "fn target() {}\nfn caller() { /* 日 */\ttarget(); }",
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[("src/lib.rs", source)])?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let facts = store
            .get_proof_resolution_facts()?
            .into_iter()
            .filter(|fact| fact.callsite.raw_target == "target")
            .collect::<Vec<_>>();
        let [fact] = facts.as_slice() else {
            panic!("one target call expected: {facts:#?}");
        };
        let start = source.rfind("target();").expect("fixture call") as u64;
        let line_start = source[..start as usize]
            .rfind('\n')
            .map_or(0, |newline| newline + 1);
        let line = source[..start as usize]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count() as u32
            + 1;
        assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
        assert_eq!(fact.callsite.start_byte, start);
        assert_eq!(
            fact.callsite.end_byte_exclusive,
            start + "target".len() as u64
        );
        assert_eq!(fact.callsite.line, line);
        assert_eq!(
            fact.callsite.column,
            (start as usize - line_start + 1) as u32,
            "tree-sitter columns are UTF-8 byte columns"
        );
    }
    Ok(())
}

#[test]
fn repeated_call_correlation_rejects_incomplete_or_noncanonical_domains() -> anyhow::Result<()> {
    for mutation in [
        RepeatedCallGraphMutation::DuplicateOrdinal,
        RepeatedCallGraphMutation::OrdinalGap,
        RepeatedCallGraphMutation::ExtraEdge,
        RepeatedCallGraphMutation::ExtraInput,
        RepeatedCallGraphMutation::NonCall,
        RepeatedCallGraphMutation::OpaqueIdentity,
        RepeatedCallGraphMutation::WrongIdentityFile,
        RepeatedCallGraphMutation::WrongIdentityLine,
        RepeatedCallGraphMutation::WrongIdentityRawTarget,
        RepeatedCallGraphMutation::WrongRawSource,
        RepeatedCallGraphMutation::TwoValidRawPlaceholderGroups,
    ] {
        let facts = repeated_call_facts_after_graph_mutation(mutation)?;
        assert_eq!(facts.len(), 2, "one syntax fact per callsite: {mutation:?}");
        assert!(
            facts
                .iter()
                .all(|fact| fact.status != ProofResolutionStatus::Exact),
            "malformed complete correlation domain retained Exact for {mutation:?}: {facts:#?}"
        );
    }
    Ok(())
}

#[test]
fn repeated_call_projection_replaces_only_heuristic_resolution_metadata() -> anyhow::Result<()> {
    for mutation in [
        RepeatedCallGraphMutation::CandidatesRetained,
        RepeatedCallGraphMutation::HeuristicSource,
        RepeatedCallGraphMutation::HeuristicTarget,
        RepeatedCallGraphMutation::CertainSource,
        RepeatedCallGraphMutation::CertainTarget,
    ] {
        let facts = repeated_call_facts_after_graph_mutation(mutation)?;
        assert_eq!(facts.len(), 2, "one syntax fact per callsite: {mutation:?}");
        assert!(
            facts
                .iter()
                .all(|fact| fact.status == ProofResolutionStatus::Exact),
            "authenticated repeated calls did not replace heuristic metadata for {mutation:?}: {facts:#?}"
        );
        assert_eq!(
            facts
                .iter()
                .filter_map(|fact| fact.edge_id)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            2,
            "one stored edge must authorize one repeated call: {facts:#?}"
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum RawProjectionHostile {
    WrongSameNamedDirect,
    MalformedSameFilePlaceholder,
    CrossFilePlaceholder,
    NonPlaceholderKind,
    WrongRawCaller,
    WrongEdgeFile,
    WrongEdgeLine,
}

#[test]
fn raw_call_projection_hostiles_leave_the_edge_unchanged_and_fact_non_exact() -> anyhow::Result<()>
{
    for hostile in [
        RawProjectionHostile::WrongSameNamedDirect,
        RawProjectionHostile::MalformedSameFilePlaceholder,
        RawProjectionHostile::CrossFilePlaceholder,
        RawProjectionHostile::NonPlaceholderKind,
        RawProjectionHostile::WrongRawCaller,
        RawProjectionHostile::WrongEdgeFile,
        RawProjectionHostile::WrongEdgeLine,
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[
                ("src/lib.rs", "fn target() {}\nfn caller() { target(); }\n"),
                ("src/other.rs", "fn unrelated() {}\n"),
            ],
        )?;
        let call = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::CALL)
            .expect("source-built CALL edge");
        let raw = store.get_node(call.target)?.expect("raw CALL target");
        assert_eq!(raw.kind, NodeKind::UNKNOWN, "closed placeholder fixture");
        store.get_connection().execute(
            "UPDATE edge
             SET resolved_target_node_id = NULL,
                 confidence = 0.5,
                 certainty = 'uncertain',
                 candidate_target_node_ids = '[]'
             WHERE id = ?1",
            [call.id.0],
        )?;
        let other_file = store
            .get_nodes()?
            .into_iter()
            .find(|node| node.kind == NodeKind::FILE && node.serialized_name.ends_with("other.rs"))
            .expect("other file")
            .id;
        match hostile {
            RawProjectionHostile::WrongSameNamedDirect => {
                store.get_connection().execute(
                    "UPDATE node SET kind = ?1 WHERE id = ?2",
                    (NodeKind::FUNCTION as i32, raw.id.0),
                )?;
            }
            RawProjectionHostile::MalformedSameFilePlaceholder => {
                store
                    .get_connection()
                    .execute("UPDATE node SET start_line = 99 WHERE id = ?1", [raw.id.0])?;
            }
            RawProjectionHostile::CrossFilePlaceholder => {
                store.get_connection().execute(
                    "UPDATE node SET file_node_id = ?1 WHERE id = ?2",
                    (other_file.0, raw.id.0),
                )?;
            }
            RawProjectionHostile::NonPlaceholderKind => {
                store.get_connection().execute(
                    "UPDATE node SET kind = ?1 WHERE id = ?2",
                    (NodeKind::VARIABLE as i32, raw.id.0),
                )?;
            }
            RawProjectionHostile::WrongRawCaller => {
                store.get_connection().execute(
                    "UPDATE edge SET source_node_id = file_node_id, resolved_source_node_id = NULL WHERE id = ?1",
                    [call.id.0],
                )?;
            }
            RawProjectionHostile::WrongEdgeFile => {
                store.get_connection().execute(
                    "UPDATE edge SET file_node_id = ?1 WHERE id = ?2",
                    (other_file.0, call.id.0),
                )?;
            }
            RawProjectionHostile::WrongEdgeLine => {
                store
                    .get_connection()
                    .execute("UPDATE edge SET line = 99 WHERE id = ?1", [call.id.0])?;
            }
        }
        let before = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.id == call.id)
            .expect("mutated raw edge");
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let after = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.id == call.id)
            .expect("retained raw edge");
        assert_eq!(after, before, "rejected hostile was mutated: {hostile:?}");
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("target fact");
        assert_ne!(
            fact.status,
            ProofResolutionStatus::Exact,
            "{hostile:?}: {fact:#?}"
        );
    }
    Ok(())
}

#[test]
fn noncanonical_callsite_spellings_leave_the_edge_unchanged_and_fact_non_exact()
-> anyhow::Result<()> {
    for case in 0..15 {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[("src/lib.rs", "fn target() {}\nfn caller() { target(); }\n")],
        )?;
        let call = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::CALL)
            .expect("source-built CALL edge");
        let canonical = call
            .callsite_identity
            .as_deref()
            .expect("canonical source-built identity");
        let fields = canonical
            .split('|')
            .next()
            .unwrap()
            .split(':')
            .collect::<Vec<_>>();
        let [file, line, discriminator, raw] = fields.as_slice() else {
            panic!("source-built identity has four fields: {canonical}");
        };
        let rewritten = match case {
            0 => format!("0{file}:{line}:{discriminator}:{raw}"),
            1 => format!("+{file}:{line}:{discriminator}:{raw}"),
            2 => format!("{file}:0{line}:{discriminator}:{raw}"),
            3 => format!("{file}:{line}:+{discriminator}:{raw}"),
            4 => format!("{file}:{line}:0:{raw}"),
            5 => format!("{file}:{line}:0{discriminator}:{raw}"),
            6 => format!("{file}:{line}:4294967296:{raw}"),
            7 => format!("{file}:{line}:{discriminator}:+{raw}"),
            8 => format!("{file}:{line}:{discriminator}:0{raw}"),
            9 => format!("{file}:{line}:{discriminator}:9223372036854775808"),
            10 => format!("{file}:{line}:{discriminator}"),
            11 => format!("{file}:{line}:{discriminator}:{raw}:5"),
            12 => "|marker".to_owned(),
            13 => "opaque".to_owned(),
            14 => format!("{file}:{line}:{discriminator}:{raw}|"),
            _ => unreachable!(),
        };
        store.get_connection().execute(
            "UPDATE edge SET callsite_identity = ?1 WHERE id = ?2",
            (&rewritten, call.id.0),
        )?;
        let before = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.id == call.id)
            .expect("mutated raw edge");
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        let after = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.id == call.id)
            .expect("retained raw edge");
        assert_eq!(
            after, before,
            "noncanonical identity was mutated: {rewritten}"
        );
        let fact = store
            .get_proof_resolution_facts()?
            .into_iter()
            .find(|fact| fact.callsite.raw_target == "target")
            .expect("target fact");
        assert_ne!(
            fact.status,
            ProofResolutionStatus::Exact,
            "{rewritten}: {fact:#?}"
        );
    }
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

fn assert_only_call_is_not_exact(files: &[(&str, &str)]) -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(project.path(), &mut store, files)?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let called = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == "target")
        .collect::<Vec<_>>();
    assert_eq!(called.len(), 1, "unexpected target calls: {called:#?}");
    assert_ne!(
        called[0].status,
        ProofResolutionStatus::Exact,
        "{files:?}: {called:#?}"
    );
    assert_eq!(called[0].target, None, "{called:#?}");
    assert_eq!(called[0].edge_id, None, "{called:#?}");
    Ok(())
}

fn assert_only_call_is_exact(files: &[(&str, &str)]) -> anyhow::Result<()> {
    assert_call_named_is_exact(files, "target")
}

fn assert_call_named_is_exact(files: &[(&str, &str)], raw_target: &str) -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(project.path(), &mut store, files)?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let called = store
        .get_proof_resolution_facts()?
        .into_iter()
        .filter(|fact| fact.callsite.raw_target == raw_target)
        .collect::<Vec<_>>();
    assert_eq!(called.len(), 1, "unexpected target calls: {called:#?}");
    assert_eq!(
        called[0].status,
        ProofResolutionStatus::Exact,
        "{files:?}: {called:#?}"
    );
    assert!(called[0].target.is_some(), "{called:#?}");
    assert!(called[0].edge_id.is_some(), "{called:#?}");
    Ok(())
}

fn assert_no_exact_calls(files: &[(&str, &str)]) -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(project.path(), &mut store, files)?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    assert!(
        facts
            .iter()
            .all(|fact| fact.status != ProofResolutionStatus::Exact),
        "{files:?}: {facts:#?}"
    );
    Ok(())
}

fn assert_no_exact_target_calls(files: &[(&str, &str)]) -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(project.path(), &mut store, files)?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    assert!(
        facts.iter().all(|fact| {
            fact.callsite.raw_target != "target" || fact.status != ProofResolutionStatus::Exact
        }),
        "{files:?}: {facts:#?}"
    );
    Ok(())
}

fn rust_fact_after_forcing_call_target(
    files: &[(&str, &str)],
    raw_target: &str,
) -> anyhow::Result<codestory_contracts::proof_resolution::CallResolutionFact> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(project.path(), &mut store, files)?;
    let nodes = store.get_nodes()?;
    let target_name = if raw_target == "alias" {
        "target"
    } else {
        raw_target
    };
    let target = nodes
        .iter()
        .find(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && node.serialized_name.ends_with(target_name)
                && !node.serialized_name.ends_with("caller")
        })
        .unwrap_or_else(|| panic!("forced target node for {files:?}: {nodes:#?}"));
    let caller = nodes
        .iter()
        .find(|node| {
            matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                && node.serialized_name.ends_with("caller")
        })
        .expect("forced caller node");
    let call = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL)
        .expect("forced ordinary CALL edge");
    store.get_connection().execute(
        "UPDATE edge
         SET resolved_source_node_id = ?1,
             resolved_target_node_id = ?2,
             candidate_target_node_ids = '[]'
         WHERE id = ?3",
        (caller.id.0, target.id.0, call.id.0),
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    Ok(store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == raw_target)
        .expect("forced call fact"))
}

#[test]
fn javascript_typescript_unsupported_matrix_never_authorizes() -> anyhow::Result<()> {
    for (path, source) in [
        (
            "src/script.js",
            "function target() {}\nfunction caller() { target(); }\n",
        ),
        (
            "src/commonjs.cjs",
            "const target = require('./target');\nfunction caller() { target(); }\n",
        ),
        (
            "src/function_expression.js",
            "export const target = function () {};\nexport function caller() { target(); }\n",
        ),
        (
            "src/alias.js",
            "const actual = () => {};\nexport const target = actual;\nexport function caller() { target(); }\n",
        ),
        (
            "src/object.js",
            "const object = { target() {} };\nexport function caller() { object.target(); }\n",
        ),
        (
            "src/optional.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver?.target(); }\n",
        ),
        (
            "src/computed.js",
            "export class C { target() {} }\nexport function caller() { const receiver = new C(); receiver['target'](); }\n",
        ),
        (
            "src/optional_identifier.js",
            "export function target() {}\nexport function caller() { target?.(); }\n",
        ),
        (
            "src/tagged.js",
            "export function target() {}\nexport function caller() { target`value`; }\n",
        ),
        (
            "src/call.js",
            "export function target() {}\nexport function caller() { target.call(null); }\n",
        ),
        (
            "src/bind.js",
            "export function target() {}\nexport function caller() { target.bind(null)(); }\n",
        ),
        (
            "src/dynamic_import.js",
            "export async function caller() { const target = await import('./target.js'); target(); }\n",
        ),
        (
            "src/type_only.ts",
            "import type { target } from './target';\nexport function caller() { target(); }\n",
        ),
        (
            "src/namespace.ts",
            "import * as ns from './target';\nexport function caller() { ns.target(); }\n",
        ),
        (
            "src/package.ts",
            "import { target } from 'package';\nexport function caller() { target(); }\n",
        ),
        (
            "src/nested.ts",
            "export function target() {}\nexport function outer() { function caller() { target(); } caller(); }\n",
        ),
        (
            "src/parameter_initializer.ts",
            "export function target() {}\nexport function caller(value = target()) {}\n",
        ),
        (
            "src/relevant_using.ts",
            "export function target() {}\nusing target = resource;\nexport function caller() { target(); }\n",
        ),
        (
            "src/accessor.js",
            "export function target() {}\nexport class C { get value() { target(); return 1; } }\n",
        ),
        (
            "src/field.js",
            "export function target() {}\nexport class C { value = target(); }\n",
        ),
        (
            "src/static.js",
            "export function target() {}\nexport class C { static { target(); } }\n",
        ),
        (
            "src/new.js",
            "export class C {}\nexport function caller() { new C(); }\n",
        ),
    ] {
        assert_no_exact_calls(&[(path, source)])?;
    }
    Ok(())
}

#[test]
fn typescript_exact_rejects_module_writes_shadows_and_unsupported_namespace_reflection()
-> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[(
        "src/reassigned.ts",
        "function target() {}\nfunction other() {}\ntarget = other;\nexport function caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/catch_shadow.ts",
        "function target() {}\nexport function caller() { try {} catch (target) { target(); } }\n",
    )])?;
    assert_only_call_is_not_exact(&[
        (
            "src/exported.ts",
            "export function target() {}\nfunction other() {}\ntarget = other;\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[(
        "src/nested_function.ts",
        "function target() {}\nexport function caller() { function target() {} target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/outer_shadow.ts",
        "function target() {}\nexport function outer(target: () => void) { function caller() { target(); } caller(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/top_level_value.ts",
        "function target() {}\nfunction other() {}\nlet target = other;\nexport function caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/destructuring_write.ts",
        "function target() {}\nfunction other() {}\n[target] = [other];\nexport function caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[
        (
            "src/dynamic_exporter.ts",
            "export function target() {}\neval('target');\n",
        ),
        (
            "src/dynamic_importer.ts",
            "import { target } from './dynamic_exporter';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[(
        "src/for_of_write.ts",
        "function target() {}\nexport function caller() { for (target of [() => {}]) {} target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/for_in_destructure.ts",
        "function target() {}\nexport function caller() { for ([target] in { 0: [() => {}] }) {} target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/logical_assignment.ts",
        "function target() {}\nfunction other() {}\nexport function caller() { target ||= other; target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/update.ts",
        "function target() {}\nexport function caller() { target++; target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/loop_binding.ts",
        "function target() {}\nexport function caller() { for (const [target] of [[() => {}]]) { target(); } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/arrow_parameter.ts",
        "function target() {}\nexport function caller() { const invoke = target => target(); invoke(() => {}); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/named_function_expression.ts",
        "function target() {}\nexport function caller() { const invoke = function target() { target(); }; invoke(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/enum_binding.ts",
        "function target() {}\nenum target { Value }\nexport function caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[
        (
            "src/exported_loop.ts",
            "export function target() {}\nfor (target of [() => {}]) {}\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported_loop';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[
        ("src/exported.ts", "export function target() {}\n"),
        ("src/other.ts", "export const value = 1;\n"),
        (
            "src/import_collision.ts",
            "import { target } from './exported';\nimport * as target from './other';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[
        ("src/namespace_exported.ts", "export function target() {}\n"),
        (
            "src/namespace_importer.ts",
            "import * as namespace from './namespace_exported';\nfunction other() {}\nObject.defineProperty(namespace, \"target\", { value: other });\nexport function caller() { namespace.target(); }\n",
        ),
    ])?;
    Ok(())
}

#[test]
fn typescript_script_calls_are_never_exact() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[(
        "src/local.ts",
        "function target() {}\nfunction caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/direct.tsx",
        "function target() {}\nfunction caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[
        (
            "src/main.ts",
            "function target() {}\nfunction caller() { target(); }\n",
        ),
        ("src/unrelated.ts", "function unrelated() {}\n"),
    ])?;
    assert_only_call_is_not_exact(&[
        (
            "src/mixed.ts",
            "function target() {}\nfunction caller() { target(); }\n",
        ),
        ("src/mutation.js", "target = () => {};\n"),
    ])?;
    assert_only_call_is_not_exact(&[(
        "src/reflective.ts",
        "function target() {}\nfunction other() {}\nObject.defineProperty(globalThis, \"target\", { value: other });\nfunction caller() { target(); }\n",
    )])?;
    Ok(())
}

#[test]
fn typescript_direct_exports_are_classified_from_closed_syntax() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[
        (
            "src/exported.ts",
            "export default async function target() {}\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[
        ("src/exported.ts", "export default function* target() {}\n"),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[
        ("src/exported.ts", "export default function* target() {}\n"),
        (
            "src/importer.ts",
            "import target from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[
        ("src/exported.ts", "export function* target() {}\n"),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[
        (
            "src/exported.ts",
            "export default /* comment */ async function target() {}\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "export default async function target() {}\n",
        ),
        (
            "src/importer.ts",
            "import target from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "export default /* comment */ async function target() {}\n",
        ),
        (
            "src/importer.ts",
            "import target from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_exact(&[
        ("src/exported.ts", "export async function target() {}\n"),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    for unsupported_export in [
        "export default function () {}\n",
        "export default function first() {}\nexport default function second() {}\n",
        "export declare function target(): void;\n",
        "type target = () => void;\nexport type { target };\n",
        "export { target } from './actual';\n",
        "export function target(): void;\nexport function target() {}\n",
    ] {
        assert_only_call_is_not_exact(&[
            ("src/exported.ts", unsupported_export),
            ("src/actual.ts", "export function target() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ])?;
    }
    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "export const unrelated = 1;\nexport function target() {}\n",
        ),
        ("src/actual.ts", "export function target() {}\n"),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    Ok(())
}

#[test]
fn typescript_direct_export_requires_a_unique_module_value_binding() -> anyhow::Result<()> {
    for competing_binding in [
        "function other() {}\nvar target = other;\n",
        "function other() {}\nlet target = other;\n",
        "function other() {}\nconst target = other;\n",
        "function other() {}\nconst [target] = [other];\n",
        "class target {}\n",
        "enum target { Value }\n",
        "namespace target {}\n",
        "import { other as target } from './other';\n",
        "function target() {}\n",
        "declare function target(): void;\n",
        "function target(): void;\n",
    ] {
        let exporter = format!("export function target() {{}}\n{competing_binding}");
        assert_only_call_is_not_exact(&[
            ("src/exported.ts", exporter.as_str()),
            ("src/other.ts", "export function other() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ])?;
    }
    assert_only_call_is_not_exact(&[
        (
            "src/exported.ts",
            "export default async function target() {}\nfunction other() {}\nvar target = other;\n",
        ),
        (
            "src/importer.ts",
            "import target from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_exact(&[
        ("src/exported.ts", "export async function target() {}\n"),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "export default async function target() {}\n",
        ),
        (
            "src/importer.ts",
            "import target from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;
    Ok(())
}

#[test]
fn typescript_module_closure_is_name_specific() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[
        ("src/exported.ts", "export function target() {}\n"),
        (
            "src/importer.ts",
            "import { target } from './exported';\nimport target = Other.other;\nexport function caller() { target(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[
        (
            "src/exported.ts",
            "import target = Other.other;\nexport function target() {}\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;

    for unrelated_root in [
        "namespace Other {}\n",
        "module Other {}\n",
        "const unrelated = 1;\n",
        "using resource = acquire;\n",
        "class Other {}\n",
        "enum Other { Value }\n",
        "declare function other(): void;\n",
        "function other(): void;\n",
        "export { other } from './other';\n",
        "Object.defineProperty(globalThis, 'unrelated', { value: 1 });\n",
    ] {
        let importer = format!(
            "import {{ target }} from './exported';\n{unrelated_root}export function caller() {{ target(); }}\n"
        );
        assert_only_call_is_exact(&[
            ("src/exported.ts", "export function target() {}\n"),
            ("src/other.ts", "export function other() {}\n"),
            ("src/importer.ts", importer.as_str()),
        ])?;

        let exporter = format!("export function target() {{}}\n{unrelated_root}");
        assert_only_call_is_exact(&[
            ("src/exported.ts", exporter.as_str()),
            ("src/other.ts", "export function other() {}\n"),
            (
                "src/importer.ts",
                "import { target } from './exported';\nexport function caller() { target(); }\n",
            ),
        ])?;
    }

    assert_only_call_is_not_exact(&[
        (
            "src/exported.ts",
            "export function target() {}\nexport = target;\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\nexport function caller() { target(); }\n",
        ),
    ])?;

    assert_only_call_is_exact(&[
        (
            "src/exported.ts",
            "/* inert */\n;\nexport async function target() {}\n",
        ),
        (
            "src/importer.ts",
            "import { target } from './exported';\n// inert\n;\nexport function caller() { target(); }\n",
        ),
    ])?;
    Ok(())
}

#[test]
fn rust_exact_rejects_lexical_module_and_inherent_lookup_ambiguity() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[(
        "src/parameter.rs",
        "fn target() {}\nfn caller(target: fn()) { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/inner_const.rs",
        "fn target() {}\nfn caller() { const target: fn() = || {}; target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/nested_module.rs",
        "fn target() {}\nmod m { fn caller() { target(); } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/duplicate_impl.rs",
        "struct Worker;\nimpl Worker { fn target(&self) {} }\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/inner_function.rs",
        "fn target() {}\nfn caller() { fn target() {} target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/hoisted_inner_function.rs",
        "fn target() {}\nfn caller() { target(); fn target() {} }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/block_use.rs",
        "fn target() {}\nmod other { pub fn value() {} }\nfn caller() { use crate::other::value as target; target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/unit_struct.rs",
        "fn target() {}\nfn caller() { struct target; target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/static_value.rs",
        "fn original() {}\nstatic target: fn() = original;\nfn target() {}\nfn caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/for_pattern.rs",
        "fn target() {}\nfn caller() { for target in [|| {}] { target(); } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/if_let_pattern.rs",
        "fn target() {}\nfn caller() { if let Some(target) = Some(|| {}) { target(); } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/while_let_pattern.rs",
        "fn target() {}\nfn caller() { while let Some(target) = Some(|| {}) { target(); break; } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/match_pattern.rs",
        "fn target() {}\nfn caller() { match Some(|| {}) { Some(target) => target(), None => {} } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/closure_pattern.rs",
        "fn target() {}\nfn caller() { let invoke = |target: fn()| target(); invoke(|| {}); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/compound_write.rs",
        "fn target() {}\nfn other() {}\nfn caller() { target += other; target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/const_generic.rs",
        "fn target() {}\nfn caller<const target: usize>() { target(); }\n",
    )])?;
    for source in [
        "fn target() {}\nfn caller() { let (target,) = (|| {},); target(); }\n",
        "fn target() {}\nstruct Pair { target: fn() }\nfn caller() { let Pair { target } = Pair { target: || {} }; target(); }\n",
        "fn target() {}\nfn caller() { let [target] = [|| {}]; target(); }\n",
        "fn target() {}\nfn caller(value: Option<fn()>) { if let Some(target @ _) = value { target(); } }\n",
        "fn target() {}\nfn caller(value: &fn()) { let ref target = value; target(); }\n",
    ] {
        assert_only_call_is_not_exact(&[("src/pattern.rs", source)])?;
    }
    Ok(())
}

#[test]
fn rust_closed_exact_subset_authorizes_supported_calls() -> anyhow::Result<()> {
    for files in [
        vec![(
            "src/lib.rs",
            "mod nested {\nfn target() {}\nfn caller() { target(); }\n}\n",
        )],
        vec![
            ("src/target.rs", "pub fn target() {}\n"),
            (
                "src/lib.rs",
                "mod target;\nuse crate::target::target;\nfn caller() { target(); }\n",
            ),
        ],
        vec![(
            "src/lib.rs",
            "mod nested {\npub fn target() {}\n}\nfn caller() { self::nested::target(); }\n",
        )],
        vec![(
            "src/lib.rs",
            "enum Owner { Value }\nimpl Owner {\nfn target(&self) {}\nfn caller(&self) { self.target(); }\n}\n",
        )],
        vec![(
            "src/lib.rs",
            "struct Owner;\nimpl Owner {\nfn target() {}\nfn caller() { Self::target(); }\n}\n",
        )],
        vec![(
            "src/lib.rs",
            "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller(value: &mut Owner) { value.target(); }\n",
        )],
        vec![(
            "src/lib.rs",
            "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller() { let value = Owner; value.target(); }\n",
        )],
    ] {
        assert_only_call_is_exact(&files)?;
    }
    Ok(())
}

#[test]
fn rust_direct_uses_and_anchored_module_paths_authorize_closed_bindings() -> anyhow::Result<()> {
    for (files, raw_target) in [
        (
            vec![
                ("src/target.rs", "pub fn target() {}\npub fn other() {}\n"),
                (
                    "src/lib.rs",
                    "mod target;\nuse crate::target::{other, target};\nfn caller() { target(); }\n",
                ),
            ],
            "target",
        ),
        (
            vec![(
                "src/lib.rs",
                "mod nested { pub fn target() {} }\nfn caller() { crate::nested::target(); }\n",
            )],
            "target",
        ),
    ] {
        assert_call_named_is_exact(&files, raw_target)?;
    }
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            (
                "src/lib.rs",
                "mod a;\nuse crate::a::b::target;\nfn caller() { target(); }\n",
            ),
            ("src/a.rs", "pub mod b;\n"),
            ("src/a/b.rs", "pub fn target() {}\n"),
        ],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("deep direct import fact");
    assert_eq!(
        fact.status,
        ProofResolutionStatus::IncompleteDomain,
        "file-backed module paths without literal MEMBER edges remain non-exact: {fact:#?}"
    );
    assert!(fact.evidence_chain.is_empty(), "{fact:#?}");
    Ok(())
}

#[test]
fn rust_inherent_receiver_and_constructor_subset_authorizes_closed_bindings() -> anyhow::Result<()>
{
    for files in [
        vec![(
            "src/lib.rs",
            "struct Owner;\nimpl Owner { fn target() {} }\nfn caller() { Owner::target(); }\n",
        )],
        vec![(
            "src/lib.rs",
            "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller() { let value: &Owner = &Owner; value.target(); }\n",
        )],
        vec![(
            "src/lib.rs",
            "struct Owner;\nimpl Owner { fn build() -> Self { Self } fn target(&self) {} }\nfn caller() { let value = Owner::build(); value.target(); }\n",
        )],
        vec![(
            "src/lib.rs",
            "struct Owner;\nimpl Owner { fn build() -> Owner { Owner } fn target(&self) {} }\nfn caller() { let value = Owner::build(); value.target(); }\n",
        )],
        vec![
            (
                "src/owner.rs",
                "pub struct Owner;\nimpl Owner { pub fn target(&self) {} }\n",
            ),
            (
                "src/lib.rs",
                "mod owner;\nuse crate::owner::Owner;\nfn caller(value: &Owner) { value.target(); }\n",
            ),
        ],
        vec![
            (
                "src/owner.rs",
                "pub struct Owner;\nimpl Owner { pub fn target() {} }\n",
            ),
            (
                "src/lib.rs",
                "mod owner;\nuse crate::owner::Owner;\nfn caller() { Owner::target(); }\n",
            ),
        ],
        vec![
            ("src/owner.rs", "pub struct Owner;\n"),
            (
                "src/lib.rs",
                "mod owner;\nuse crate::owner::Owner;\nimpl Owner { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
            ),
        ],
    ] {
        assert_only_call_is_exact(&files)?;
    }
    Ok(())
}

#[test]
fn rust_projection_requires_exact_syntax_but_not_exact_navigation_metadata() -> anyhow::Result<()> {
    assert_no_exact_calls(&[
        ("src/target.rs", "pub fn target() {}\n"),
        (
            "src/lib.rs",
            "mod target;\nuse crate::target::target as alias;\nfn caller() { alias(); }\n",
        ),
    ])?;
    assert_only_call_is_not_exact(&[(
        "src/lib.rs",
        "pub fn target() {}\nmod nested { fn caller() { super::target(); } }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/lib.rs",
        "struct Owner { value: usize }\nimpl Owner { fn target(&self) {} }\nfn caller() { let value = Owner { value: 1 }; value.target(); }\n",
    )])?;
    for source in [
        "fn target() {}\nfn caller() { target(); let target: fn() = || {}; }\n",
        "fn target() {}\nfn caller() { { let target: fn() = || {}; let _ = target; } target(); }\n",
    ] {
        assert_only_call_is_exact(&[("src/lib.rs", source)])?;
    }
    assert_only_call_is_not_exact(&[(
        "src/lib.rs",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nmod other { pub struct Owner; }\nfn caller() { use crate::other::Owner; let value: Owner = Owner; value.target(); }\n",
    )])?;
    Ok(())
}

#[test]
fn rust_source_closure_rejects_unsupported_surfaces_before_graph_correlation() -> anyhow::Result<()>
{
    for source in [
        "struct Owner;\nimpl Owner { fn target(&self) {} fn caller() { self.target(); } }\n",
        "struct Owner;\nimpl Owner { fn target() {} fn caller() { Self::Nested::target(); } }\n",
        "struct T;\nimpl T { fn target(&self) {} }\nfn caller<T>(value: T) { value.target(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller() { struct Owner; let value: Owner = Owner; value.target(); }\n",
        "struct Owner<T = ()>;\nimpl Owner { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        "struct Owner { value: usize }\nimpl Owner { fn target(&self) {} }\nfn caller() { let value = Owner { value: 1 }; value.target(); }\n",
    ] {
        let fact = rust_fact_after_forcing_call_target(&[("src/lib.rs", source)], "target")?;
        assert_eq!(fact.status, ProofResolutionStatus::Unsupported, "{fact:#?}");
        assert!(fact.evidence_chain.is_empty(), "{fact:#?}");
    }
    let renamed = rust_fact_after_forcing_call_target(
        &[
            ("src/target.rs", "pub fn target() {}\n"),
            (
                "src/lib.rs",
                "mod target;\nuse crate::target::target as alias;\nfn caller() { alias(); }\n",
            ),
        ],
        "alias",
    )?;
    assert_eq!(
        renamed.status,
        ProofResolutionStatus::Unsupported,
        "{renamed:#?}"
    );
    let parent = rust_fact_after_forcing_call_target(
        &[(
            "src/lib.rs",
            "pub fn target() {}\nmod nested { fn caller() { super::target(); } }\n",
        )],
        "target",
    )?;
    assert_eq!(
        parent.status,
        ProofResolutionStatus::Unsupported,
        "{parent:#?}"
    );

    for source in [
        "struct Owner;\nimpl Owner { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        "struct Owner;\nimpl Owner { fn target() {} fn caller() { Self::target(); } }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller(value: &Owner) { value.target(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller() { let value = Owner; value.target(); }\n",
    ] {
        assert_only_call_is_exact(&[("src/lib.rs", source)])?;
    }
    Ok(())
}

#[test]
fn rust_module_and_import_closure_matrix_stays_fail_closed() -> anyhow::Result<()> {
    for files in [
        vec![(
            "src/lib.rs",
            "mod missing;\nuse crate::missing::target;\nfn caller() { target(); }\n",
        )],
        vec![
            ("src/foo.rs", "pub fn target() {}\n"),
            ("src/foo/mod.rs", "pub fn target() {}\n"),
            (
                "src/lib.rs",
                "mod foo;\nuse crate::foo::target;\nfn caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/actual.rs", "pub fn target() {}\n"),
            (
                "src/lib.rs",
                "#[path = \"actual.rs\"] mod selected;\nuse crate::selected::target;\nfn caller() { target(); }\n",
            ),
        ],
        vec![(
            "src/lib.rs",
            "use external::target;\nfn caller() { target(); }\n",
        )],
        vec![
            ("src/foo.rs", "pub fn target() {}\n"),
            (
                "src/lib.rs",
                "mod foo;\nuse crate::foo::*;\nfn caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/foo.rs", "pub fn target() {}\n"),
            (
                "src/lib.rs",
                "mod foo;\npub use crate::foo::target;\nfn caller() { target(); }\n",
            ),
        ],
        vec![
            ("src/foo.rs", "fn target() {}\n"),
            (
                "src/lib.rs",
                "mod foo;\nuse crate::foo::target;\nfn caller() { target(); }\n",
            ),
        ],
    ] {
        assert_no_exact_target_calls(&files)?;
    }
    assert_only_call_is_not_exact(&[(
        "src/lib.rs",
        "fn target() {}\nfn target() {}\nfn caller() { target(); }\n",
    )])?;
    assert_no_exact_target_calls(&[
        ("src/lib.rs", "mod target;\n"),
        ("src/target.rs", "pub fn target() {}\n"),
        (
            "src/orphan.rs",
            "use crate::target::target;\nfn caller() { target(); }\n",
        ),
    ])?;
    assert_no_exact_target_calls(&[
        (
            "src/lib.rs",
            "mod target;\nuse crate::target::target;\nfn caller() { target(); }\n",
        ),
        ("src/target.rs", "pub fn target() {}\nstruct target;\n"),
    ])?;
    Ok(())
}

#[test]
fn rust_inherent_and_receiver_unsupported_matrix_stays_fail_closed() -> anyhow::Result<()> {
    for source in [
        "struct Owner;\nimpl Owner where Owner: Sized { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        "struct Owner;\ntrait Trait { fn target(); }\nimpl Trait for Owner { fn target() {} }\nfn caller() { <Owner as Trait>::target(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller() { Owner::target(); }\n",
        "struct Owner;\nstruct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller(value: &Owner) { value.target(); }\n",
        "struct Owner;\ntype Alias = Owner;\nimpl Owner { fn target(&self) {} }\nfn caller(value: Alias) { value.target(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller(value: Box<Owner>) { value.target(); }\n",
        "trait Trait { fn target(&self); }\nfn caller(value: &dyn Trait) { value.target(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nstruct Holder { value: Owner }\nfn caller(holder: Holder) { holder.value.target(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn make() -> Owner { Owner }\nfn caller() { make().target(); }\n",
        "struct Owner(u8);\nimpl Owner { fn target(&self) {} }\nfn caller() { let value = Owner(1); value.target(); }\n",
        "struct Owner;\nimpl Owner { fn build() -> Result<Self, ()> { Ok(Self) } fn target(&self) {} }\nfn caller() { let value = Owner::build(); value.target(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} }\nfn caller() { let value = Owner; value = Owner; value.target(); }\n",
        "fn target<T>() {}\nfn caller() { target::<u8>(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} fn caller(&self) { self.target(); } }\nimpl Owner { generated!(); }\n",
        "struct Owner;\nimpl Owner { fn target(&self) {} fn caller(&self) { self.target(); } }\nimpl Owner where Owner: Sized { fn other(&self) {} }\n",
    ] {
        assert_no_exact_target_calls(&[("src/lib.rs", source)])?;
    }
    Ok(())
}

#[test]
fn rust_relevant_domain_poison_does_not_escape_its_module_or_callable() -> anyhow::Result<()> {
    for source in [
        "fn target() {}\nfn caller() { target(); }\nmod unrelated { generated!(); }\n",
        "fn target() {}\nfn caller() { target(); }\nfn unrelated() { generated!(); }\n",
        "fn target() {}\nfn caller() { fn unrelated() { generated!(); } target(); }\n",
        "struct Owner;\nimpl Owner {\nfn target(&self) {}\nfn caller(&self) { self.target(); }\n}\nmod unrelated { #[custom] fn hidden() {} }\n",
    ] {
        assert_only_call_is_exact(&[("src/lib.rs", source)])?;
    }
    Ok(())
}

#[test]
fn certain_navigation_resolution_is_replaced_by_authenticated_exact_evidence() -> anyhow::Result<()>
{
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[
            ("src/right.ts", "export function importedTarget() {}\n"),
            ("src/wrong.ts", "export function wrongTarget() {}\n"),
            (
                "src/caller.ts",
                "import { importedTarget } from './right';\nexport function caller() { importedTarget(); }\n",
            ),
        ],
    )?;
    let nodes = store.get_nodes()?;
    let right = nodes
        .iter()
        .find(|node| {
            node.kind == NodeKind::FUNCTION
                && node.serialized_name == "importedTarget"
                && node
                    .file_node_id
                    .and_then(|id| store.get_node(id).ok().flatten())
                    .is_some_and(|file| file.serialized_name.ends_with("right.ts"))
        })
        .expect("authenticated declaration")
        .id;
    let wrong = nodes
        .into_iter()
        .find(|node| {
            node.kind == NodeKind::FUNCTION
                && node.serialized_name == "wrongTarget"
                && node
                    .file_node_id
                    .and_then(|id| store.get_node(id).ok().flatten())
                    .is_some_and(|file| file.serialized_name.ends_with("wrong.ts"))
        })
        .expect("wrong declaration")
        .id;
    let call_edge = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.kind == EdgeKind::CALL && edge.line == Some(2))
        .expect("raw call edge");
    store.get_connection().execute(
        "UPDATE edge SET resolved_target_node_id = ?1, certainty = 'certain' WHERE id = ?2",
        [wrong.0, call_edge.id.0],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "importedTarget")
        .expect("call fact");
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    assert_eq!(fact.target, Some(right));
    assert_eq!(fact.edge_id, Some(call_edge.id));
    let edge = store
        .get_edges()?
        .into_iter()
        .find(|edge| edge.id == call_edge.id)
        .expect("projected CALL edge");
    assert_eq!(edge.resolved_target, Some(right));
    assert_eq!(edge.confidence, Some(1.0));
    assert_eq!(edge.certainty, Some(ResolutionCertainty::Certain));
    assert!(edge.candidate_targets.is_empty());
    Ok(())
}

#[test]
fn complete_projection_requires_cache_coverage_but_empty_and_unsupported_repositories_work()
-> anyhow::Result<()> {
    let mut empty = Store::new_in_memory()?;
    let empty_receipt = rematerialize_proof_resolution_projection(&mut empty, &publication(1))?;
    assert_eq!(empty_receipt.fact_count, 0);
    assert_eq!(empty_receipt.adapter_roster.len(), 6);

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
    assert_eq!(unsupported_receipt.adapter_roster.len(), 6);

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
         SELECT (SELECT path FROM file WHERE language = 'typescript'),
                cache_key, artifact_blob, updated_at_epoch_ms
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

#[cfg(unix)]
#[test]
fn complete_projection_rejects_native_path_identity_collisions() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let first = project.path().join("src/first.ts");
    fs::create_dir_all(first.parent().unwrap())?;
    fs::write(
        &first,
        "export function target() {}\nexport function caller() { target(); }\n",
    )?;
    let alias = project.path().join("src/alias.ts");
    fs::hard_link(&first, &alias)?;
    let mut store = Store::new_in_memory()?;
    WorkspaceIndexer::new(project.path().to_path_buf()).run_incremental(
        &mut store,
        &RefreshInfo {
            mode: BuildMode::Incremental,
            files_to_index: vec![first, alias],
            files_to_remove: Vec::new(),
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;

    let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
        .expect_err("one native file identity cannot prove two indexed modules");
    assert!(error.to_string().contains("identity collision"), "{error}");
    Ok(())
}

#[cfg(unix)]
#[test]
fn complete_projection_rejects_unavailable_native_path_identity() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/main.ts",
            "export function target() {}\nexport function caller() { target(); }\n",
        )],
    )?;
    store.get_connection().execute(
        "UPDATE file SET path = CAST(x'626164002e7473' AS TEXT) WHERE language = 'typescript'",
        [],
    )?;

    let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
        .expect_err("native identity failure must not publish a complete-domain fact");
    assert!(error.to_string().contains("native identity"), "{error}");
    Ok(())
}

#[test]
fn complete_projection_rejects_parser_completeness_mismatch() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/main.ts",
            "export function target() {}\nexport function caller() { target(); }\n",
        )],
    )?;
    let artifact_blob = store.get_connection().query_row(
        "SELECT artifact_blob FROM index_artifact_cache",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let mut artifact: serde_json::Value = serde_json::from_slice(&artifact_blob)?;
    artifact["resolution_file"]["complete"] = serde_json::Value::Bool(false);
    store.get_connection().execute(
        "UPDATE index_artifact_cache SET artifact_blob = ?1",
        [serde_json::to_vec(&artifact)?],
    )?;

    let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
        .expect_err("parser completeness disagreement must invalidate cache coverage");
    assert!(error.to_string().contains("stale"), "{error}");
    Ok(())
}

#[test]
fn complete_projection_rejects_an_attacker_supplied_parser_fingerprint() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/main.ts",
            "export function target() {}\nexport function caller() { target(); }\n",
        )],
    )?;
    let artifact_blob = store.get_connection().query_row(
        "SELECT artifact_blob FROM index_artifact_cache",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let mut artifact: serde_json::Value = serde_json::from_slice(&artifact_blob)?;
    let attacker_fingerprint = "f".repeat(64);
    artifact["resolution_file"]["parser_fingerprint"] =
        serde_json::Value::String(attacker_fingerprint.clone());
    for call in artifact["call_resolution_inputs"]
        .as_array_mut()
        .expect("call inputs")
    {
        call["parser_fingerprint"] = serde_json::Value::String(attacker_fingerprint.clone());
    }
    store.get_connection().execute(
        "UPDATE index_artifact_cache SET artifact_blob = ?1",
        [serde_json::to_vec(&artifact)?],
    )?;

    let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
        .expect_err("attacker-supplied parser identity must not authenticate itself");
    assert!(error.to_string().contains("fingerprint"), "{error}");
    Ok(())
}

#[test]
fn complete_projection_rejects_cache_schema_adapter_and_language_mismatch() -> anyhow::Result<()> {
    for mutation in 0..4 {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[(
                "src/main.ts",
                "export function target() {}\nexport function caller() { target(); }\n",
            )],
        )?;
        let artifact_blob = store.get_connection().query_row(
            "SELECT artifact_blob FROM index_artifact_cache",
            [],
            |row| row.get::<_, Vec<u8>>(0),
        )?;
        let mut artifact: serde_json::Value = serde_json::from_slice(&artifact_blob)?;
        match mutation {
            0 => artifact["resolution_input_schema_version"] = 5.into(),
            1 => artifact["resolution_file"]["adapter_version"] = "reference-v5".into(),
            2 => artifact["call_resolution_inputs"][0]["adapter_version"] = "reference-v5".into(),
            3 => artifact["resolution_file"]["language"] = "javascript".into(),
            _ => unreachable!(),
        }
        store.get_connection().execute(
            "UPDATE index_artifact_cache SET artifact_blob = ?1",
            [serde_json::to_vec(&artifact)?],
        )?;
        let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
            .expect_err("cache provenance mismatch must reject the complete projection");
        let message = error.to_string();
        assert!(
            ["stale", "schema-v12", "adapter", "language"]
                .iter()
                .any(|needle| message.contains(needle)),
            "{mutation}: {error}"
        );
    }
    Ok(())
}

#[test]
fn unrelated_relative_cache_suffix_cannot_impersonate_an_indexed_absolute_path()
-> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/index.ts",
            "export function target() {}\nexport function caller() { target(); }\n",
        )],
    )?;
    store.get_connection().execute(
        "UPDATE index_artifact_cache SET file_path = 'other/src/index.ts'",
        [],
    )?;

    let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
        .expect_err("a different component path must not impersonate the indexed file");
    assert!(error.to_string().contains("native path"), "{error}");
    Ok(())
}

#[test]
fn rust_macro_expansion_poison_is_scoped_to_relevant_domains() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[(
        "src/callable.rs",
        "macro_rules! shadow { ($name:ident) => { let $name: fn() = || {}; }; }\nfn target() {}\nfn caller() { shadow!(target); target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/root.rs",
        "macro_rules! duplicate { ($name:ident) => { fn $name() {} }; }\nduplicate!(target);\nfn target() {}\nfn caller() { target(); }\n",
    )])?;
    assert_only_call_is_not_exact(&[(
        "src/impl.rs",
        "macro_rules! duplicate { () => { fn target(&self) {} }; }\nstruct Worker;\nimpl Worker { duplicate!(); fn target(&self) {} fn caller(&self) { self.target(); } }\n",
    )])?;
    assert_only_call_is_exact(&[
        (
            "src/main.rs",
            "struct Worker;\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        ),
        (
            "src/other.rs",
            "macro_rules! duplicate { () => { impl Worker { fn target(&self) {} } }; }\nduplicate!();\n",
        ),
    ])?;
    assert_only_call_is_exact(&[(
        "src/expression.rs",
        "fn target() {}\nfn caller() -> String { target(); format!(\"done\") }\n",
    )])?;
    assert_only_call_is_exact(&[(
        "src/disjoint.rs",
        "macro_rules! local_only { () => { let unrelated = 1; }; }\nfn target() {}\nfn caller() { { local_only!(); } target(); }\n",
    )])?;
    Ok(())
}

#[test]
fn rust_module_closure_ignores_unrelated_closed_expression_macros() -> anyhow::Result<()> {
    for source in [
        "const LABEL: &str = concat!(\"open\", \"\");\nfn target() {}\nfn caller() { target(); }\n",
        "static PACKAGE: &str = env!(\"CARGO_PKG_NAME\");\nfn target() {}\nfn caller() { target(); }\n",
        "const LABEL: &str = { let _ = format_args!(\"{}\", \"note\"); \"note\" };\nfn target() {}\nfn caller() { target(); }\n",
        "enum Marker { Value = line!() }\nfn target() {}\nfn caller() { target(); }\n",
        "#[cfg(any())]\nconst UNRELATED: usize = 0;\nfn target() {}\nfn caller() { target(); }\n",
    ] {
        assert_only_call_is_exact(&[("src/lib.rs", source)])?;
    }

    for source in [
        "include!(\"generated.rs\");\nfn target() {}\nfn caller() { target(); }\n",
        "thread_local! { static MARKER: usize = 0; }\nfn target() {}\nfn caller() { target(); }\n",
        "macro_rules! generated { () => { fn target() {} }; }\ngenerated!();\nfn target() {}\nfn caller() { target(); }\n",
        "macro_rules! foreign_target { () => { fn target(); }; }\nunsafe extern \"C\" { foreign_target!(); }\nfn target() {}\nfn caller() { target(); }\n",
        "#![cfg(any())]\nfn target() {}\nfn caller() { target(); }\n",
        "#[cfg(any())]\nfn target() {}\nfn target() {}\nfn caller() { target(); }\n",
        "fn target() {}\n#[cfg(any())]\nfn caller() { target(); }\n",
    ] {
        assert_only_call_is_not_exact(&[("src/lib.rs", source)])?;
    }
    Ok(())
}

#[test]
fn rust_closed_expression_macros_only_relax_bare_same_file_calls() -> anyhow::Result<()> {
    assert_only_call_is_exact(&[(
        "src/lib.rs",
        "const LABEL: &str = concat!(\"open\", \"\");\nfn target() {}\nfn caller() { target(); }\n",
    )])?;

    for source in [
        "const LABEL: &str = concat!(\"open\", \"\");\nstruct Worker;\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        "const LABEL: &str = concat!(\"open\", \"\");\nstruct Worker;\nimpl Worker { fn target() {} }\nfn caller() { Worker::target(); }\n",
        "const LABEL: &str = concat!(\"open\", \"\");\nstruct Worker;\nimpl Worker { fn target(&self) {} }\nfn caller() { let value: Worker = Worker; value.target(); }\n",
    ] {
        assert_only_call_is_not_exact(&[("src/lib.rs", source)])?;
    }
    Ok(())
}

#[test]
fn rust_identifier_local_closure_keeps_glob_import_domains_incomplete() -> anyhow::Result<()> {
    assert_only_call_is_not_exact(&[(
        "src/lib.rs",
        "use crate::*;\nfn target() {}\nfn caller() { target(); }\n",
    )])
}

#[test]
fn proof_resolution_roster_tracks_the_current_adapter_version() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[("src/lib.rs", "fn target() {}\nfn caller() { target(); }\n")],
    )?;
    let receipt = rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    assert_eq!(
        receipt
            .adapter_roster
            .iter()
            .find(|adapter| adapter.language == "rust")
            .map(|adapter| adapter.adapter_version.as_str()),
        Some("reference-v14")
    );
    for fact in store.get_proof_resolution_facts()? {
        assert!(receipt.adapter_roster.iter().any(|adapter| {
            adapter.language == fact.provenance.language_adapter
                && adapter.adapter_version == fact.provenance.language_adapter_version
        }));
    }
    Ok(())
}

#[test]
fn rust_attribute_domains_are_never_exact() -> anyhow::Result<()> {
    for source in [
        "#[unresolved_attribute_macro]\nfn target() {}\nfn caller() { target(); }\n",
        "#[cfg(any())]\nfn target() {}\nfn caller() { target(); }\n",
        "fn target() {}\n#[unresolved_attribute_macro]\nfn caller() { target(); }\n",
        "#![cfg(any())]\nfn target() {}\nfn caller() { target(); }\n",
        "#[unresolved_attribute_macro]\nstruct Marker;\nfn target() {}\nfn caller() { target(); }\n",
        "fn target() {}\nfn caller() { #![cfg(any())] target(); }\n",
    ] {
        assert_only_call_is_not_exact(&[("src/lib.rs", source)])?;
    }
    for source in [
        "struct Worker;\n#[unresolved_attribute_macro]\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        "struct Worker;\nimpl Worker { #[cfg(any())] fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        "struct Worker;\nimpl Worker { fn target(&self) {} #[unresolved_attribute_macro] fn caller(&self) { self.target(); } }\n",
        "struct Worker;\nimpl Worker { #[unresolved_attribute_macro] const MARKER: usize = 0; fn target(&self) {} fn caller(&self) { self.target(); } }\n",
    ] {
        assert_only_call_is_not_exact(&[("src/lib.rs", source)])?;
    }

    assert_only_call_is_exact(&[("src/lib.rs", "fn target() {}\nfn caller() { target(); }\n")])?;
    assert_only_call_is_exact(&[(
        "src/lib.rs",
        "#[derive(Debug)]\nstruct Unrelated;\n#[cfg(test)]\nmod tests {}\nfn target() {}\nfn caller() { target(); }\n",
    )])?;
    assert_only_call_is_exact(&[(
        "src/lib.rs",
        "fn target() {}\nfn caller() { #[cfg(any())] {} target(); }\n",
    )])?;
    assert_only_call_is_exact(&[(
        "src/lib.rs",
        "#![allow(dead_code)]\nfn target() {}\nfn caller() { target(); }\n",
    )])?;
    assert_only_call_is_exact(&[(
        "src/lib.rs",
        "struct Worker;\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
    )])?;
    Ok(())
}

#[test]
fn rust_bounded_outer_metadata_preserves_unrelated_bindings() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/lib.rs",
            "#[derive(Debug)]\n#[serde(rename = \"serde_helper\")]\nstruct SerdeHelper;\n#[derive(Debug)]\n#[error(\"error_helper\")]\nstruct ErrorHelper;\n#[inline]\nfn inline_helper() {}\n#[cfg_attr(test, derive(Debug))]\nstruct Report;\nfn target() {}\nfn caller() { target(); SerdeHelper(); ErrorHelper(); inline_helper(); Report(); }\n",
        )],
    )?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let facts = store.get_proof_resolution_facts()?;
    let fact = |name| {
        facts
            .iter()
            .find(|fact| fact.callsite.raw_target == name)
            .expect("one direct call")
    };
    assert_eq!(fact("target").status, ProofResolutionStatus::Exact);
    for name in ["SerdeHelper", "ErrorHelper", "inline_helper", "Report"] {
        assert_eq!(
            fact(name).status,
            ProofResolutionStatus::IncompleteDomain,
            "the attributed item's own name remains incomplete: {name}"
        );
    }

    for source in [
        "#[unresolved_attribute_macro]\nfn helper() {}\nfn target() {}\nfn caller() { target(); }\n",
        "#[cfg_attr(test, unresolved_attribute_macro)]\nfn helper() {}\nfn target() {}\nfn caller() { target(); }\n",
        "#[serde(rename = \"helper\")]\nfn helper() {}\nfn target() {}\nfn caller() { target(); }\n",
        "#[error(\"helper\")]\nfn helper() {}\nfn target() {}\nfn caller() { target(); }\n",
        "#[serde(rename = \"helper\")]\nstruct Helper;\nfn target() {}\nfn caller() { target(); }\n",
        "#[cfg_attr(test, derive(Debug))]\nfn helper() {}\nfn target() {}\nfn caller() { target(); }\n",
    ] {
        assert_only_call_is_not_exact(&[("src/lib.rs", source)])?;
    }
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
                "export function target() {}\neval('target');\nexport function caller() { target(); }\n",
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

#[test]
fn parser_incomplete_governed_source_publishes_incomplete_domain_fact() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[(
            "src/vite-shaped.ts",
            "export function target() {}\nexport function caller() { target(); }\n<",
        )],
    )?;
    let indexed_file = store
        .get_files()?
        .into_iter()
        .find(|file| file.language == "typescript")
        .expect("typescript file");
    assert!(indexed_file.indexed);
    assert!(
        !indexed_file.complete,
        "fixture must exercise parser incompleteness"
    );

    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;

    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == "target")
        .expect("target call fact");
    assert_eq!(fact.status, ProofResolutionStatus::IncompleteDomain);
    assert_eq!(fact.target, None);
    assert_eq!(fact.edge_id, None);
    assert!(fact.evidence_chain.is_empty());
    assert_eq!(fact.provenance.dependency_file_hashes.len(), 1);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum ExactDependencyMutation {
    Caller,
    SameFileDeclaration,
    StaticImportBinding,
    StaticImportDeclaration,
    ImplicitReceiverOwner,
    ImplicitReceiverDeclaration,
}

#[derive(Clone, Copy, Debug)]
enum MutatedDependencyEligibility {
    IndexedIncomplete,
    UnindexedComplete,
    UnsupportedComplete,
    MissingOwnership,
}

fn assert_exact_dependency_mutation_downgrades(
    mutation: ExactDependencyMutation,
    eligibility: MutatedDependencyEligibility,
) -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    let (mut files, target) = match mutation {
        ExactDependencyMutation::Caller | ExactDependencyMutation::SameFileDeclaration => (
            vec![(
                "src/local.ts",
                "export function target() {}\nexport function caller() { target(); }\n",
            )],
            "target",
        ),
        ExactDependencyMutation::StaticImportBinding
        | ExactDependencyMutation::StaticImportDeclaration => (
            vec![
                ("src/exported.ts", "export function target() {}\n"),
                (
                    "src/importer.ts",
                    "import { target } from './exported';\nexport function caller() { target(); }\n",
                ),
            ],
            "target",
        ),
        ExactDependencyMutation::ImplicitReceiverOwner
        | ExactDependencyMutation::ImplicitReceiverDeclaration => (
            vec![(
                "src/lib.rs",
                "struct Worker;\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
            )],
            "target",
        ),
    };
    if matches!(eligibility, MutatedDependencyEligibility::IndexedIncomplete) {
        files.push(("src/ineligible.ts", "<"));
    }
    index_files(project.path(), &mut store, &files)?;
    rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
    let fact = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == target)
        .expect("exact fixture fact");
    assert_eq!(fact.status, ProofResolutionStatus::Exact, "{fact:#?}");
    let node_id = match mutation {
        ExactDependencyMutation::Caller => fact.caller,
        ExactDependencyMutation::SameFileDeclaration => fact
            .evidence_chain
            .iter()
            .find_map(|evidence| match evidence {
                ResolutionEvidence::SameFileDeclaration { declaration } => Some(*declaration),
                _ => None,
            })
            .expect("same-file declaration"),
        ExactDependencyMutation::StaticImportBinding => fact
            .evidence_chain
            .iter()
            .find_map(|evidence| match evidence {
                ResolutionEvidence::StaticImportBinding { import, .. } => Some(*import),
                _ => None,
            })
            .expect("static import binding"),
        ExactDependencyMutation::StaticImportDeclaration => fact
            .evidence_chain
            .iter()
            .find_map(|evidence| match evidence {
                ResolutionEvidence::StaticImportBinding { declaration, .. } => Some(*declaration),
                _ => None,
            })
            .expect("static import declaration"),
        ExactDependencyMutation::ImplicitReceiverOwner => fact
            .evidence_chain
            .iter()
            .find_map(|evidence| match evidence {
                ResolutionEvidence::ImplicitReceiver { owner } => Some(*owner),
                _ => None,
            })
            .expect("implicit receiver owner"),
        ExactDependencyMutation::ImplicitReceiverDeclaration => fact
            .evidence_chain
            .iter()
            .find_map(|evidence| match evidence {
                ResolutionEvidence::SameFileDeclaration { declaration } => Some(*declaration),
                _ => None,
            })
            .expect("implicit receiver declaration"),
    };
    let mut node = store.get_node(node_id)?.expect("mutated evidence node");
    if matches!(eligibility, MutatedDependencyEligibility::MissingOwnership) {
        node.file_node_id = None;
    } else {
        let dependency_file_id =
            if matches!(eligibility, MutatedDependencyEligibility::IndexedIncomplete) {
                let dependency_file = store
                    .get_files()?
                    .into_iter()
                    .find(|file| file.path.ends_with("src/ineligible.ts"))
                    .expect("indexed incomplete dependency file");
                assert!(dependency_file.indexed);
                assert!(!dependency_file.complete);
                NodeId(dependency_file.id)
            } else {
                let dependency_file_id = NodeId(9_000_001);
                let (language, indexed) = match eligibility {
                    MutatedDependencyEligibility::UnindexedComplete => ("typescript", false),
                    MutatedDependencyEligibility::UnsupportedComplete => ("text", true),
                    MutatedDependencyEligibility::IndexedIncomplete
                    | MutatedDependencyEligibility::MissingOwnership => unreachable!(),
                };
                let dependency_file = FileInfo {
                    id: dependency_file_id.0,
                    path: project.path().join("src/ineligible-dependency.txt"),
                    language: language.to_owned(),
                    modification_time: 0,
                    indexed,
                    complete: true,
                    line_count: 1,
                    file_role: FileRole::Source,
                };
                store.insert_file(&dependency_file)?;
                store.update_file_metadata(&dependency_file, Some(&"d".repeat(64)))?;
                store.insert_node(&Node {
                    id: dependency_file_id,
                    kind: NodeKind::FILE,
                    serialized_name: dependency_file.path.display().to_string(),
                    ..Default::default()
                })?;
                dependency_file_id
            };
        node.file_node_id = Some(dependency_file_id);
    }
    store.insert_node(&node)?;

    if matches!(mutation, ExactDependencyMutation::Caller) {
        let error = rematerialize_proof_resolution_projection(&mut store, &publication(2))
            .expect_err("caller ownership mismatch is graph/cache integrity corruption");
        assert!(error.to_string().contains("caller"), "{error}");
        return Ok(());
    }
    rematerialize_proof_resolution_projection(&mut store, &publication(2))?;
    let downgraded = store
        .get_proof_resolution_facts()?
        .into_iter()
        .find(|fact| fact.callsite.raw_target == target)
        .expect("downgraded fixture fact");
    assert_eq!(
        downgraded.status,
        ProofResolutionStatus::IncompleteDomain,
        "{mutation:?} {eligibility:?}: {downgraded:#?}"
    );
    assert_eq!(downgraded.target, None);
    assert_eq!(downgraded.edge_id, None);
    assert!(downgraded.evidence_chain.is_empty());
    Ok(())
}

#[test]
fn exact_dependency_domains_require_complete_governed_ownership() -> anyhow::Result<()> {
    let mutations = [
        ExactDependencyMutation::Caller,
        ExactDependencyMutation::SameFileDeclaration,
        ExactDependencyMutation::StaticImportBinding,
        ExactDependencyMutation::StaticImportDeclaration,
        ExactDependencyMutation::ImplicitReceiverOwner,
        ExactDependencyMutation::ImplicitReceiverDeclaration,
    ];
    let eligibility_classes = [
        MutatedDependencyEligibility::IndexedIncomplete,
        MutatedDependencyEligibility::UnindexedComplete,
        MutatedDependencyEligibility::UnsupportedComplete,
        MutatedDependencyEligibility::MissingOwnership,
    ];
    for mutation in mutations {
        for eligibility in eligibility_classes {
            assert_exact_dependency_mutation_downgrades(mutation, eligibility)?;
        }
    }
    Ok(())
}
