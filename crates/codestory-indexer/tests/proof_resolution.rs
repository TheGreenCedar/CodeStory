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
                "src/module_global_reflection.ts",
                "function other() {}\nexport function moduleReflectiveTarget() {}\nObject.defineProperty(globalThis, \"moduleReflectiveTarget\", { value: other });\nexport function moduleReflectiveCaller() { moduleReflectiveTarget(); }\n",
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
        "moduleReflectiveTarget",
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
        "{called:#?}"
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
        "{called:#?}"
    );
    assert!(called[0].target.is_some(), "{called:#?}");
    assert!(called[0].edge_id.is_some(), "{called:#?}");
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
