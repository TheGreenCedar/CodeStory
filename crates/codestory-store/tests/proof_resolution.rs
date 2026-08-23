use codestory_contracts::graph::{Edge, EdgeId, EdgeKind, Node, NodeId, NodeKind};
use codestory_contracts::proof_resolution::{
    CallResolutionFact, CalleeForm, DependencyFileHash, EXACT_CALL_RESOLUTION_ALGORITHM,
    ExactCallsite, FileId, INTERNAL_RESOLUTION_PRODUCER, PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
    ProofResolutionAdapter, ProofResolutionFunnelCounts, ProofResolutionFunnelRow,
    ProofResolutionProjection, ProofResolutionReason, ProofResolutionStatus, ResolutionEvidence,
    ResolutionEvidenceKind, ResolutionProvenance,
};
use codestory_store::{
    FileInfo, FileRole, IndexPublicationMode, IndexPublicationRecord, Store,
    seal_call_resolution_fact,
};

fn publication() -> IndexPublicationRecord {
    IndexPublicationRecord {
        generation: 1,
        generation_id: "generation-1".to_owned(),
        run_id: "run-1".to_owned(),
        mode: IndexPublicationMode::Full,
        published_at_epoch_ms: 123,
    }
}

fn exact_fact(edge_id: EdgeId) -> CallResolutionFact {
    seal_call_resolution_fact(CallResolutionFact {
        fact_id: String::new(),
        edge_id: Some(edge_id),
        raw_edge_target: Some(NodeId(4)),
        raw_callsite_identity: Some("1:2:1:4".to_owned()),
        callsite: ExactCallsite {
            file_id: FileId(1),
            source_sha256: "a".repeat(64),
            start_byte: 24,
            end_byte_exclusive: 32,
            line: 2,
            column: 15,
            callee_form: CalleeForm::Identifier,
            raw_target: "callee".to_owned(),
        },
        caller: NodeId(2),
        target: Some(NodeId(3)),
        status: ProofResolutionStatus::Exact,
        reason: ProofResolutionReason::ExactResolution,
        evidence_chain: vec![ResolutionEvidence::SameFileDeclaration {
            declaration: NodeId(3),
        }],
        lookup_domain_complete: true,
        provenance: ResolutionProvenance {
            producer: INTERNAL_RESOLUTION_PRODUCER.to_owned(),
            fact_schema_version: PROOF_RESOLUTION_FACT_SCHEMA_VERSION,
            algorithm: EXACT_CALL_RESOLUTION_ALGORITHM.to_owned(),
            language_adapter: "rust".to_owned(),
            language_adapter_version: "rust-exact-v1".to_owned(),
            parser_fingerprint: "1".repeat(64),
            dependency_file_hashes: vec![DependencyFileHash {
                file_id: FileId(1),
                source_sha256: "a".repeat(64),
            }],
            evidence_sha256: String::new(),
        },
    })
    .expect("seal fact")
}

fn projection(facts: Vec<CallResolutionFact>) -> ProofResolutionProjection {
    ProofResolutionProjection {
        adapter_roster: vec![ProofResolutionAdapter {
            language: "rust".to_owned(),
            adapter_version: "rust-exact-v1".to_owned(),
        }],
        facts,
        funnel: vec![ProofResolutionFunnelRow {
            language: "rust".to_owned(),
            callee_form: Some(CalleeForm::Identifier),
            evidence_kind: Some(ResolutionEvidenceKind::SameFileDeclaration),
            counts: ProofResolutionFunnelCounts {
                syntax_calls: 1,
                adapter_supported: 1,
                exact: 1,
                ambiguous: 0,
                missing_binding: 0,
                incomplete_domain: 0,
                unsupported: 0,
                exact_call_linked: 1,
                proof_shape_admitted: 0,
                authoritative_receipts: 0,
                complete_proofs: 0,
            },
        }],
    }
}

fn seed_exact_graph(store: &mut Store) {
    let file = FileInfo {
        id: 1,
        path: "src/lib.rs".into(),
        language: "rust".to_owned(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 2,
        file_role: FileRole::Source,
    };
    store.insert_file(&file).expect("file");
    store
        .update_file_metadata(&file, Some(&"a".repeat(64)))
        .expect("source hash");
    for node in [
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_owned(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_owned(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(2),
            start_col: Some(1),
            end_line: Some(2),
            end_col: Some(40),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "callee".to_owned(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(1),
            start_col: Some(1),
            end_line: Some(1),
            end_col: Some(20),
            ..Default::default()
        },
        Node {
            id: NodeId(4),
            kind: NodeKind::UNKNOWN,
            serialized_name: "callee".to_owned(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(2),
            start_col: Some(15),
            end_line: Some(2),
            end_col: Some(21),
            ..Default::default()
        },
    ] {
        store.insert_node(&node).expect("node");
    }
    store
        .insert_edge(&Edge {
            id: EdgeId(7),
            source: NodeId(2),
            target: NodeId(4),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(1)),
            line: Some(2),
            resolved_target: Some(NodeId(3)),
            callsite_identity: Some("1:2:1:4".to_owned()),
            ..Default::default()
        })
        .expect("edge");
}

#[test]
fn schema_32_migration_creates_no_synthetic_proof_publication() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("codestory.db");
    let store = Store::open(&path).expect("store");

    assert_eq!(codestory_store::CURRENT_SCHEMA_VERSION, 32);
    assert_eq!(store.get_proof_resolution_publication().unwrap(), None);
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 0);
}

#[test]
fn exact_projection_round_trips_with_matching_raw_call_and_deterministic_digest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("codestory.db");
    let mut store = Store::open(&path).expect("store");
    seed_exact_graph(&mut store);
    let publication = publication();
    let projection = projection(vec![exact_fact(EdgeId(7))]);

    let first = store
        .replace_proof_resolution_projection(&publication, &projection)
        .expect("publish proof projection");
    store
        .validate_proof_resolution_publication(&publication)
        .expect("validate projection");
    let loaded = store
        .get_exact_proof_resolution_fact_by_edge(EdgeId(7))
        .expect("read exact fact")
        .expect("exact fact");

    assert_eq!(loaded, projection.facts[0]);
    assert_eq!(first.fact_count, 1);
    assert_eq!(first.fact_digest.len(), 64);
    assert_eq!(
        first,
        store.get_proof_resolution_publication().unwrap().unwrap()
    );
}

#[test]
fn exact_projection_rejects_digest_and_graph_mismatches_without_partial_rows() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("codestory.db");
    let mut store = Store::open(&path).expect("store");
    seed_exact_graph(&mut store);
    let publication = publication();
    let mut fact = exact_fact(EdgeId(7));
    fact.provenance.evidence_sha256 = "0".repeat(64);

    let error = store
        .replace_proof_resolution_projection(&publication, &projection(vec![fact]))
        .expect_err("digest mismatch must fail");
    assert!(error.to_string().contains("evidence digest"), "{error}");
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 0);
    assert_eq!(store.get_proof_resolution_publication().unwrap(), None);

    let mut endpoint_mismatch = exact_fact(EdgeId(7));
    endpoint_mismatch.target = Some(NodeId(2));
    endpoint_mismatch.evidence_chain = vec![ResolutionEvidence::SameFileDeclaration {
        declaration: NodeId(2),
    }];
    let endpoint_mismatch = seal_call_resolution_fact(endpoint_mismatch).unwrap();
    let error = store
        .replace_proof_resolution_projection(&publication, &projection(vec![endpoint_mismatch]))
        .expect_err("graph mismatch must fail");
    assert!(error.to_string().contains("ordinary CALL edge"), "{error}");
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 0);
    assert_eq!(store.get_proof_resolution_publication().unwrap(), None);

    let mut callsite_mismatch = exact_fact(EdgeId(7));
    callsite_mismatch.raw_callsite_identity = Some("1:2:2:4".to_owned());
    let callsite_mismatch = seal_call_resolution_fact(callsite_mismatch).unwrap();
    let error = store
        .replace_proof_resolution_projection(&publication, &projection(vec![callsite_mismatch]))
        .expect_err("callsite mismatch must fail");
    assert!(error.to_string().contains("callsite identity"), "{error}");
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 0);

    let mut hash_mismatch = exact_fact(EdgeId(7));
    hash_mismatch.callsite.source_sha256 = "b".repeat(64);
    hash_mismatch.provenance.dependency_file_hashes[0].source_sha256 = "b".repeat(64);
    let hash_mismatch = seal_call_resolution_fact(hash_mismatch).unwrap();
    let error = store
        .replace_proof_resolution_projection(&publication, &projection(vec![hash_mismatch]))
        .expect_err("source hash mismatch must fail");
    assert!(error.to_string().contains("source hash"), "{error}");
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 0);
}

#[test]
fn resealed_but_semantically_false_evidence_and_funnel_are_rejected() {
    let mut store = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut store);

    let mut wrong_same_file = exact_fact(EdgeId(7));
    wrong_same_file.evidence_chain = vec![ResolutionEvidence::SameFileDeclaration {
        declaration: NodeId(2),
    }];
    let wrong_same_file = seal_call_resolution_fact(wrong_same_file).unwrap();
    let error = store
        .replace_proof_resolution_projection(&publication(), &projection(vec![wrong_same_file]))
        .expect_err("resealed false same-file evidence must fail");
    assert!(error.to_string().contains("semantic validator"), "{error}");

    let mut unused_variant = exact_fact(EdgeId(7));
    unused_variant.evidence_chain = vec![ResolutionEvidence::ConstructorBinding {
        constructor: NodeId(3),
    }];
    let unused_variant = seal_call_resolution_fact(unused_variant).unwrap();
    let error = store
        .replace_proof_resolution_projection(&publication(), &projection(vec![unused_variant]))
        .expect_err("unused evidence variants must fail closed");
    assert!(error.to_string().contains("semantic validator"), "{error}");

    let mut wrong_import = exact_fact(EdgeId(7));
    wrong_import.callsite.callee_form = CalleeForm::NamedImport;
    wrong_import.evidence_chain = vec![ResolutionEvidence::StaticImportBinding {
        import: NodeId(2),
        declaration: NodeId(3),
    }];
    let wrong_import = seal_call_resolution_fact(wrong_import).unwrap();
    let error = store
        .replace_proof_resolution_projection(&publication(), &projection(vec![wrong_import]))
        .expect_err("unrelated import evidence must fail");
    assert!(error.to_string().contains("StaticImportBinding"), "{error}");

    let mut wrong_funnel = projection(vec![exact_fact(EdgeId(7))]);
    wrong_funnel.funnel[0].counts.syntax_calls = 2;
    let error = store
        .replace_proof_resolution_projection(&publication(), &wrong_funnel)
        .expect_err("funnel mismatch must fail");
    assert!(error.to_string().contains("funnel"), "{error}");
}

#[test]
fn publication_digest_authenticates_roster_and_funnel() {
    let mut store = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut store);
    let publication = publication();
    store
        .replace_proof_resolution_projection(&publication, &projection(vec![exact_fact(EdgeId(7))]))
        .unwrap();
    store
        .get_connection()
        .execute(
            "UPDATE proof_resolution_publication SET funnel_json = '[]' WHERE id = 1",
            [],
        )
        .unwrap();
    let error = store
        .validate_proof_resolution_publication(&publication)
        .expect_err("resealed publication metadata mutation must fail");
    assert!(error.to_string().contains("digest"), "{error}");
}

#[test]
fn failed_replacement_and_stale_validation_preserve_the_previous_complete_publication() {
    let mut store = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut store);
    let first_publication = publication();
    let first = store
        .replace_proof_resolution_projection(
            &first_publication,
            &projection(vec![exact_fact(EdgeId(7))]),
        )
        .unwrap();

    let second_publication = IndexPublicationRecord {
        generation: 2,
        generation_id: "generation-2".to_owned(),
        run_id: "run-2".to_owned(),
        mode: IndexPublicationMode::Incremental,
        published_at_epoch_ms: 456,
    };
    let mut corrupt = exact_fact(EdgeId(7));
    corrupt.provenance.evidence_sha256 = "0".repeat(64);
    store
        .replace_proof_resolution_projection(&second_publication, &projection(vec![corrupt]))
        .expect_err("failed staged replacement");

    assert_eq!(
        store.get_proof_resolution_publication().unwrap(),
        Some(first.clone())
    );
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 1);
    let stale = store
        .validate_proof_resolution_publication(&second_publication)
        .expect_err("stale proof publication must be unavailable");
    assert!(stale.to_string().contains("does not match"), "{stale}");
    store
        .validate_proof_resolution_publication(&first_publication)
        .unwrap();
}
