use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{EdgeId, EdgeKind, Node, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{
    ProofResolutionStatus, ResolutionEvidence, ResolutionEvidenceKind,
};
use codestory_indexer::{WorkspaceIndexer, rematerialize_proof_resolution_projection};
use codestory_store::{FileInfo, FileRole, IndexPublicationMode, IndexPublicationRecord, Store};
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

#[derive(Clone, Copy)]
enum RelationMutation {
    Missing,
    Wrong,
    Duplicate,
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
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum RepeatedCallGraphMutation {
    DuplicateOrdinal,
    OrdinalGap,
    ExtraEdge,
    ExtraInput,
    OpaqueIdentity,
    WrongIdentityFile,
    WrongIdentityLine,
    WrongIdentityRawTarget,
    CandidatesRetained,
    CrossSource,
    CrossTarget,
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
        RepeatedCallGraphMutation::CrossSource => {
            connection.execute(
                "UPDATE edge SET resolved_source_node_id = ?1 WHERE id = ?2",
                (calls[0].file_node_id.unwrap().0, calls[0].id.0),
            )?;
        }
        RepeatedCallGraphMutation::CrossTarget => {
            connection.execute(
                "UPDATE edge SET resolved_target_node_id = ?1 WHERE id = ?2",
                (calls[0].effective_source().0, calls[0].id.0),
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
fn repeated_call_correlation_rejects_incomplete_or_noncanonical_domains() -> anyhow::Result<()> {
    for mutation in [
        RepeatedCallGraphMutation::DuplicateOrdinal,
        RepeatedCallGraphMutation::OrdinalGap,
        RepeatedCallGraphMutation::ExtraEdge,
        RepeatedCallGraphMutation::ExtraInput,
        RepeatedCallGraphMutation::OpaqueIdentity,
        RepeatedCallGraphMutation::WrongIdentityFile,
        RepeatedCallGraphMutation::WrongIdentityLine,
        RepeatedCallGraphMutation::WrongIdentityRawTarget,
        RepeatedCallGraphMutation::CandidatesRetained,
        RepeatedCallGraphMutation::CrossSource,
        RepeatedCallGraphMutation::CrossTarget,
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
        "export const unrelated = 1;\nexport function target() {}\n",
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
    assert_eq!(empty_receipt.adapter_roster.len(), 4);

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
    assert_eq!(unsupported_receipt.adapter_roster.len(), 4);

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
            0 => artifact["resolution_input_schema_version"] = 4.into(),
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
            ["stale", "schema-v5", "adapter", "language"]
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
fn rust_macro_expansion_domains_are_never_exact() -> anyhow::Result<()> {
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
    assert_only_call_is_not_exact(&[
        (
            "src/main.rs",
            "struct Worker;\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
        ),
        (
            "src/other.rs",
            "macro_rules! duplicate { () => { impl Worker { fn target(&self) {} } }; }\nduplicate!();\n",
        ),
    ])?;
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
        "struct Worker;\nimpl Worker { fn target(&self) {} fn caller(&self) { self.target(); } }\n",
    )])?;
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
