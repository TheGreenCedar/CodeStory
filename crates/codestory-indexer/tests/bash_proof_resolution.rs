use codestory_contracts::events::EventBus;
use codestory_contracts::graph::{Edge, EdgeId, EdgeKind};
use codestory_contracts::proof_resolution::{
    ProofResolutionReason, ProofResolutionStatus, ResolutionEvidence,
};
use codestory_indexer::{WorkspaceIndexer, rematerialize_proof_resolution_projection};
use codestory_store::{IndexPublicationMode, IndexPublicationRecord, Store};
use codestory_workspace::{BuildMode, RefreshInfo};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

fn publication(generation: u64) -> IndexPublicationRecord {
    IndexPublicationRecord {
        generation,
        generation_id: format!("bash-generation-{generation}"),
        run_id: format!("bash-run-{generation}"),
        mode: IndexPublicationMode::Incremental,
        published_at_epoch_ms: generation as i64,
    }
}

fn index_files(
    root: &Path,
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
fn bash_unique_prior_same_file_literal_calls_emit_exact_replayable_receipts() -> anyhow::Result<()>
{
    let cases = [
        (
            "posix_function",
            "proof.sh",
            "target() { return 0; }\ncaller() { target; }\n",
            1,
        ),
        (
            "function_keyword",
            "proof.sh",
            "function target { return 0; }\nfunction caller { target argument; }\n",
            1,
        ),
        (
            "companion_extension",
            "proof.bash",
            "target() { return 0; }\ncaller() { target; }\n",
            1,
        ),
        (
            "repeated_callsites",
            "proof.sh",
            "target() { return 0; }\ncaller() { target; target argument; }\n",
            2,
        ),
    ];

    for (name, path, source, expected_calls) in cases {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[(path, source)])?;
        let edge_count_before = store
            .get_edges()?
            .iter()
            .filter(|edge| edge.kind == EdgeKind::CALL)
            .count();

        let first = rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        store.validate_proof_resolution_publication(&publication(1))?;
        let target_facts = store
            .get_proof_resolution_facts()?
            .into_iter()
            .filter(|fact| fact.provenance.language_adapter == "bash")
            .filter(|fact| fact.callsite.raw_target == "target")
            .collect::<Vec<_>>();
        assert_eq!(
            target_facts.len(),
            expected_calls,
            "{name}: {target_facts:#?}"
        );
        let mut receipt_edges = std::collections::BTreeSet::new();
        for fact in &target_facts {
            assert_eq!(
                fact.status,
                ProofResolutionStatus::Exact,
                "{name}: {fact:#?}"
            );
            assert_eq!(fact.reason, ProofResolutionReason::ExactResolution);
            let target = fact.target.expect("Exact Bash target");
            let edge_id = fact.edge_id.expect("Exact Bash edge");
            assert!(fact.raw_edge_target.is_some());
            assert!(fact.raw_callsite_identity.is_some());
            assert!(fact.lookup_domain_complete);
            assert!(matches!(
                fact.evidence_chain.as_slice(),
                [ResolutionEvidence::SameFileDeclaration { declaration }] if *declaration == target
            ));
            assert_eq!(fact.provenance.dependency_file_hashes.len(), 1);
            assert_eq!(fact.provenance.evidence_sha256.len(), 64);
            assert_eq!(
                store.get_exact_proof_resolution_fact_by_edge(edge_id)?,
                Some(fact.clone()),
                "{name}: exact evidence must replay as the authoritative edge receipt"
            );
            assert!(receipt_edges.insert(edge_id), "{name}: receipt edge reused");
        }
        assert_eq!(
            store
                .get_edges()?
                .iter()
                .filter(|edge| edge.kind == EdgeKind::CALL)
                .count(),
            edge_count_before,
            "{name}: proof projection changed the ordinary navigation edge set"
        );
        let second = rematerialize_proof_resolution_projection(&mut store, &publication(2))?;
        assert_eq!(first.fact_count, second.fact_count, "{name}");
        assert_eq!(first.fact_digest, second.fact_digest, "{name}");
    }
    Ok(())
}

#[test]
fn bash_closed_unsupported_and_hostile_matrix_is_canonical_nonexact() -> anyhow::Result<()> {
    struct Case {
        name: &'static str,
        source: &'static str,
        probe: &'static str,
        status: ProofResolutionStatus,
        reason: ProofResolutionReason,
    }

    let cases = [
        Case {
            name: "literal_source",
            source: "source ./other.sh\ntarget() { :; }\ncaller() { target; }\n",
            probe: "target",
            status: ProofResolutionStatus::IncompleteDomain,
            reason: ProofResolutionReason::LookupDomainIncomplete,
        },
        Case {
            name: "dot_source",
            source: ". ./other.sh\ntarget() { :; }\ncaller() { target; }\n",
            probe: "target",
            status: ProofResolutionStatus::IncompleteDomain,
            reason: ProofResolutionReason::LookupDomainIncomplete,
        },
        Case {
            name: "alias",
            source: "alias target='echo replaced'\ntarget() { :; }\ncaller() { target; }\n",
            probe: "target",
            status: ProofResolutionStatus::Unsupported,
            reason: ProofResolutionReason::UnsupportedConstruct,
        },
        Case {
            name: "eval",
            source: "target() { :; }\ncaller() { eval 'target'; target; }\n",
            probe: "target",
            status: ProofResolutionStatus::Unsupported,
            reason: ProofResolutionReason::UnsupportedConstruct,
        },
        Case {
            name: "quoted_variable_command",
            source: "target() { :; }\ncaller() { callback=target; \"$callback\"; }\n",
            probe: "\"$callback\"",
            status: ProofResolutionStatus::Unsupported,
            reason: ProofResolutionReason::UnsupportedConstruct,
        },
        Case {
            name: "expanded_variable_command",
            source: "target() { :; }\ncaller() { callback=target; ${callback}; }\n",
            probe: "${callback}",
            status: ProofResolutionStatus::Unsupported,
            reason: ProofResolutionReason::UnsupportedConstruct,
        },
        Case {
            name: "target_producing_substitution",
            source: "target() { :; }\ncaller() { $(printf target); }\n",
            probe: "$(printf target)",
            status: ProofResolutionStatus::Unsupported,
            reason: ProofResolutionReason::UnsupportedConstruct,
        },
        Case {
            name: "duplicate_definition",
            source: "target() { :; }\ntarget() { false; }\ncaller() { target; }\n",
            probe: "target",
            status: ProofResolutionStatus::Ambiguous,
            reason: ProofResolutionReason::MultipleBindings,
        },
        Case {
            name: "conditional_definition",
            source: "if test -n \"$FLAG\"; then target() { :; }; fi\ncaller() { target; }\n",
            probe: "target",
            status: ProofResolutionStatus::IncompleteDomain,
            reason: ProofResolutionReason::LookupDomainIncomplete,
        },
        Case {
            name: "external_path_command",
            source: "caller() { codestory_external_fixture; }\n",
            probe: "codestory_external_fixture",
            status: ProofResolutionStatus::MissingBinding,
            reason: ProofResolutionReason::MissingBinding,
        },
        Case {
            name: "definition_after_callsite",
            source: "caller() { target; }\ntarget() { :; }\n",
            probe: "target",
            status: ProofResolutionStatus::IncompleteDomain,
            reason: ProofResolutionReason::LookupDomainIncomplete,
        },
        Case {
            name: "parser_incomplete",
            source: "target() { :; }\ncaller() { target;\n",
            probe: "target",
            status: ProofResolutionStatus::IncompleteDomain,
            reason: ProofResolutionReason::LookupDomainIncomplete,
        },
    ];

    for case in cases {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(project.path(), &mut store, &[("proof.sh", case.source)])?;
        rematerialize_proof_resolution_projection(&mut store, &publication(1))?;
        store.validate_proof_resolution_publication(&publication(1))?;
        let facts = store
            .get_proof_resolution_facts()?
            .into_iter()
            .filter(|fact| fact.provenance.language_adapter == "bash")
            .collect::<Vec<_>>();
        assert!(!facts.is_empty(), "{} emitted no Bash facts", case.name);
        assert!(
            facts
                .iter()
                .all(|fact| fact.status != ProofResolutionStatus::Exact),
            "{} became proof-authoritative: {facts:#?}",
            case.name
        );
        let probe = facts
            .iter()
            .find(|fact| fact.callsite.raw_target == case.probe)
            .unwrap_or_else(|| panic!("{} missing probe {}: {facts:#?}", case.name, case.probe));
        assert_eq!(probe.status, case.status, "{}: {probe:#?}", case.name);
        assert_eq!(probe.reason, case.reason, "{}: {probe:#?}", case.name);
        assert!(probe.target.is_none(), "{}: {probe:#?}", case.name);
        assert!(probe.edge_id.is_none(), "{}: {probe:#?}", case.name);
        assert!(probe.raw_edge_target.is_none(), "{}: {probe:#?}", case.name);
        assert!(
            probe.raw_callsite_identity.is_none(),
            "{}: {probe:#?}",
            case.name
        );
        assert!(probe.evidence_chain.is_empty(), "{}: {probe:#?}", case.name);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum GraphMutation {
    Missing,
    Duplicate,
    OpaqueIdentity,
    WrongLine,
    WrongSource,
}

#[test]
fn bash_graph_correlation_hostile_matrix_never_authorizes_a_receipt() -> anyhow::Result<()> {
    for mutation in [
        GraphMutation::Missing,
        GraphMutation::Duplicate,
        GraphMutation::OpaqueIdentity,
        GraphMutation::WrongLine,
        GraphMutation::WrongSource,
    ] {
        let project = tempfile::tempdir()?;
        let mut store = Store::new_in_memory()?;
        index_files(
            project.path(),
            &mut store,
            &[("proof.sh", "target() { :; }\ncaller() { target; }\n")],
        )?;
        let edge = store
            .get_edges()?
            .into_iter()
            .find(|edge| edge.kind == EdgeKind::CALL && edge.line == Some(2))
            .expect("Bash target CALL edge");
        match mutation {
            GraphMutation::Missing => {
                store
                    .get_connection()
                    .execute("DELETE FROM edge WHERE id = ?1", [edge.id.0])?;
            }
            GraphMutation::Duplicate => {
                let mut duplicate: Edge = edge.clone();
                duplicate.id = EdgeId(edge.id.0.wrapping_add(1_000_000_000));
                store.insert_edge(&duplicate)?;
            }
            GraphMutation::OpaqueIdentity => {
                store.get_connection().execute(
                    "UPDATE edge SET callsite_identity = 'opaque' WHERE id = ?1",
                    [edge.id.0],
                )?;
            }
            GraphMutation::WrongLine => {
                store
                    .get_connection()
                    .execute("UPDATE edge SET line = 99 WHERE id = ?1", [edge.id.0])?;
            }
            GraphMutation::WrongSource => {
                let file = edge.file_node_id.expect("Bash edge file");
                store.get_connection().execute(
                    "UPDATE edge SET source_node_id = ?1, resolved_source_node_id = NULL WHERE id = ?2",
                    [file.0, edge.id.0],
                )?;
            }
        }

        if let Err(error) = rematerialize_proof_resolution_projection(&mut store, &publication(1)) {
            assert!(
                error.to_string().contains("CALL edge")
                    || error.to_string().contains("correlation"),
                "{mutation:?}: {error}"
            );
            assert_eq!(store.proof_resolution_fact_count()?, 0, "{mutation:?}");
            assert_eq!(
                store.get_proof_resolution_publication()?,
                None,
                "{mutation:?}"
            );
            continue;
        }
        let facts = store
            .get_proof_resolution_facts()?
            .into_iter()
            .filter(|fact| fact.provenance.language_adapter == "bash")
            .filter(|fact| fact.callsite.raw_target == "target")
            .collect::<Vec<_>>();
        assert_eq!(facts.len(), 1, "{mutation:?}: {facts:#?}");
        assert_ne!(
            facts[0].status,
            ProofResolutionStatus::Exact,
            "{mutation:?}: {:#?}",
            facts[0]
        );
        assert!(facts[0].target.is_none(), "{mutation:?}: {:#?}", facts[0]);
        assert!(facts[0].edge_id.is_none(), "{mutation:?}: {:#?}", facts[0]);
        assert!(
            facts[0].evidence_chain.is_empty(),
            "{mutation:?}: {:#?}",
            facts[0]
        );
    }
    Ok(())
}

#[test]
fn bash_semantic_cache_is_reauthenticated_from_source_before_replay() -> anyhow::Result<()> {
    let project = tempfile::tempdir()?;
    let mut store = Store::new_in_memory()?;
    index_files(
        project.path(),
        &mut store,
        &[("proof.sh", "target() { :; }\ncaller() { target; }\n")],
    )?;
    let blob = store.get_connection().query_row(
        "SELECT artifact_blob FROM index_artifact_cache",
        [],
        |row| row.get::<_, Vec<u8>>(0),
    )?;
    let mut artifact: serde_json::Value = serde_json::from_slice(&blob)?;
    let target_call = artifact["call_resolution_inputs"]
        .as_array_mut()
        .expect("Bash cache calls")
        .iter_mut()
        .find(|call| call["callsite"]["raw_target"] == "target")
        .expect("cached target call");
    target_call["binding"] = serde_json::json!({ "kind": "unsupported" });
    store.get_connection().execute(
        "UPDATE index_artifact_cache SET artifact_blob = ?1",
        [serde_json::to_vec(&artifact)?],
    )?;

    let error = rematerialize_proof_resolution_projection(&mut store, &publication(1))
        .expect_err("mutable Bash cache bytes cannot authenticate proof semantics");
    assert!(
        error.to_string().contains("semantic cache")
            || error.to_string().contains("authenticated source"),
        "{error}"
    );
    assert_eq!(store.proof_resolution_fact_count()?, 0);
    assert_eq!(store.get_proof_resolution_publication()?, None);
    Ok(())
}
