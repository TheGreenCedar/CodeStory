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
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
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

fn repeated_exact_fact(index: u32) -> CallResolutionFact {
    let mut fact = exact_fact(EdgeId(7 + i64::from(index)));
    fact.raw_callsite_identity = Some(format!("1:2:{}:4", index + 1));
    fact.callsite.start_byte = 24 + u64::from(index) * 10;
    fact.callsite.end_byte_exclusive = fact.callsite.start_byte + 6;
    fact.callsite.column = 15 + index * 10;
    seal_call_resolution_fact(fact).expect("seal repeated fact")
}

fn incomplete_domain_fact() -> CallResolutionFact {
    let mut fact = exact_fact(EdgeId(7));
    fact.edge_id = None;
    fact.raw_edge_target = None;
    fact.raw_callsite_identity = None;
    fact.target = None;
    fact.status = ProofResolutionStatus::IncompleteDomain;
    fact.reason = ProofResolutionReason::LookupDomainIncomplete;
    fact.evidence_chain.clear();
    fact.lookup_domain_complete = false;
    seal_call_resolution_fact(fact).expect("seal incomplete-domain fact")
}

fn projection(facts: Vec<CallResolutionFact>) -> ProofResolutionProjection {
    let exact_count = u64::try_from(facts.len()).expect("test fact count fits u64");
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
                syntax_calls: exact_count,
                adapter_supported: exact_count,
                exact: exact_count,
                ambiguous: 0,
                missing_binding: 0,
                incomplete_domain: 0,
                unsupported: 0,
                exact_call_linked: exact_count,
                proof_shape_admitted: 0,
                authoritative_receipts: 0,
                complete_proofs: 0,
            },
        }],
    }
}

fn go_projection(fact: CallResolutionFact) -> ProofResolutionProjection {
    let mut result = projection(vec![fact.clone()]);
    result.adapter_roster = vec![ProofResolutionAdapter {
        language: "go".to_owned(),
        adapter_version: "reference-v10".to_owned(),
    }];
    result.funnel[0].language = "go".to_owned();
    result.funnel[0].callee_form = Some(fact.callsite.callee_form);
    result.funnel[0].evidence_kind = fact.evidence_chain.first().map(ResolutionEvidence::kind);
    result
}

fn go_exact_fact(dependencies: &[(i64, String)]) -> CallResolutionFact {
    let mut fact = exact_fact(EdgeId(7));
    fact.provenance.language_adapter = "go".to_owned();
    fact.provenance.language_adapter_version = "reference-v10".to_owned();
    if let Some((_, source_hash)) = dependencies.iter().find(|(file_id, _)| *file_id == 1) {
        fact.callsite.source_sha256 = source_hash.clone();
    }
    fact.provenance.dependency_file_hashes = dependencies
        .iter()
        .map(|(file_id, hash)| DependencyFileHash {
            file_id: FileId(*file_id),
            source_sha256: hash.clone(),
        })
        .collect();
    seal_call_resolution_fact(fact).expect("seal Go exact fact")
}

fn incomplete_domain_projection() -> ProofResolutionProjection {
    ProofResolutionProjection {
        adapter_roster: vec![ProofResolutionAdapter {
            language: "rust".to_owned(),
            adapter_version: "rust-exact-v1".to_owned(),
        }],
        facts: vec![incomplete_domain_fact()],
        funnel: vec![ProofResolutionFunnelRow {
            language: "rust".to_owned(),
            callee_form: Some(CalleeForm::Identifier),
            evidence_kind: None,
            counts: ProofResolutionFunnelCounts {
                syntax_calls: 1,
                adapter_supported: 1,
                exact: 0,
                ambiguous: 0,
                missing_binding: 0,
                incomplete_domain: 1,
                unsupported: 0,
                exact_call_linked: 0,
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

fn seed_go_exact_graph(
    store: &mut Store,
    sibling: bool,
) -> (tempfile::TempDir, Vec<(i64, String)>) {
    seed_exact_graph(store);
    let temp = tempfile::tempdir().expect("Go exact graph tempdir");
    let package = temp.path().join("src");
    fs::create_dir_all(&package).expect("create Go exact package");
    let source_bytes = b"package proof\nfunc caller() { callee() }\n";
    let source_path = package.join("main.go");
    fs::write(&source_path, source_bytes).expect("write Go exact source");
    let source_hash = sha256_bytes(source_bytes);
    let source = FileInfo {
        id: 1,
        path: source_path,
        language: "go".to_owned(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 2,
        file_role: FileRole::Source,
    };
    store.insert_file(&source).expect("Go source file");
    store
        .update_file_metadata(&source, Some(&source_hash))
        .expect("Go source hash");
    let mut dependencies = vec![(1, source_hash)];
    if sibling {
        let sibling_bytes = b"package proof\nfunc helper() {}\n";
        let sibling_path = package.join("helper.go");
        fs::write(&sibling_path, sibling_bytes).expect("write Go exact sibling");
        let sibling_hash = sha256_bytes(sibling_bytes);
        let sibling = FileInfo {
            id: 5,
            path: sibling_path,
            language: "go".to_owned(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        };
        store.insert_file(&sibling).expect("Go sibling file");
        store
            .update_file_metadata(&sibling, Some(&sibling_hash))
            .expect("Go sibling hash");
        dependencies.push((5, sibling_hash));
    }
    (temp, dependencies)
}

fn sha256_bytes(source: &[u8]) -> String {
    format!("{:x}", Sha256::digest(source))
}

fn seed_authenticated_go_cross_file_graph(
    store: &mut Store,
    root: &Path,
    source_package: &str,
    target_package: &str,
    target_path_uses_native_alias: bool,
) -> CallResolutionFact {
    let package = root.join("package");
    fs::create_dir_all(&package).expect("create Go package");
    let source_path = package.join("main.go");
    let target_path = package.join("target.go");
    let source = format!("package {source_package}\nfunc caller() {{ callee() }}\n");
    let target = format!("package {target_package}\nfunc callee() {{}}\n");
    fs::write(&source_path, source.as_bytes()).expect("write Go source");
    fs::write(&target_path, target.as_bytes()).expect("write Go target");
    let recorded_target_path = if target_path_uses_native_alias {
        package.join("..").join("package").join("target.go")
    } else {
        target_path
    };
    let source_sha256 = sha256_bytes(source.as_bytes());
    let target_sha256 = sha256_bytes(target.as_bytes());
    for (id, path, hash) in [
        (1, source_path, source_sha256.clone()),
        (5, recorded_target_path, target_sha256.clone()),
    ] {
        let file = FileInfo {
            id,
            path,
            language: "go".to_owned(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 2,
            file_role: FileRole::Source,
        };
        store.insert_file(&file).expect("Go file");
        store
            .update_file_metadata(&file, Some(&hash))
            .expect("Go source hash");
    }
    for node in [
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "main.go".to_owned(),
            ..Default::default()
        },
        Node {
            id: NodeId(5),
            kind: NodeKind::FILE,
            serialized_name: "target.go".to_owned(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_owned(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(2),
            end_line: Some(2),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::FUNCTION,
            serialized_name: "callee".to_owned(),
            file_node_id: Some(NodeId(5)),
            start_line: Some(2),
            end_line: Some(2),
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
        store.insert_node(&node).expect("Go graph node");
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
        .expect("Go CALL edge");

    let mut fact = exact_fact(EdgeId(7));
    fact.callsite.source_sha256 = source_sha256.clone();
    fact.evidence_chain = vec![ResolutionEvidence::SamePackageDeclaration {
        declaration: NodeId(3),
    }];
    fact.provenance.dependency_file_hashes = vec![
        DependencyFileHash {
            file_id: FileId(1),
            source_sha256,
        },
        DependencyFileHash {
            file_id: FileId(5),
            source_sha256: target_sha256,
        },
    ];
    fact.provenance.language_adapter = "go".to_owned();
    fact.provenance.language_adapter_version = "reference-v10".to_owned();
    seal_call_resolution_fact(fact).expect("seal authenticated Go fact")
}

fn receiver_projection(fact: CallResolutionFact) -> ProofResolutionProjection {
    let mut result = projection(vec![fact.clone()]);
    result.adapter_roster = vec![ProofResolutionAdapter {
        language: "typescript".to_owned(),
        adapter_version: "reference-v6".to_owned(),
    }];
    result.funnel[0].language = "typescript".to_owned();
    result.funnel[0].callee_form = Some(fact.callsite.callee_form);
    result.funnel[0].evidence_kind = fact.evidence_chain.first().map(ResolutionEvidence::kind);
    result
}

fn rust_receiver_projection(fact: CallResolutionFact) -> ProofResolutionProjection {
    let mut result = receiver_projection(fact);
    result.adapter_roster = vec![ProofResolutionAdapter {
        language: "rust".to_owned(),
        adapter_version: "reference-v9".to_owned(),
    }];
    result.funnel[0].language = "rust".to_owned();
    result
}

fn local_receiver_fact(with_constructor: bool) -> CallResolutionFact {
    let mut fact = exact_fact(EdgeId(7));
    fact.raw_edge_target = Some(NodeId(4));
    fact.raw_callsite_identity = Some("1:2:1:4".to_owned());
    fact.callsite.callee_form = CalleeForm::ExplicitReceiver;
    fact.callsite.raw_target = "target".to_owned();
    fact.caller = NodeId(2);
    fact.target = Some(NodeId(4));
    fact.evidence_chain = vec![
        ResolutionEvidence::ExplicitReceiverType {
            receiver_type: NodeId(3),
        },
        ResolutionEvidence::SameFileDeclaration {
            declaration: NodeId(4),
        },
    ];
    if with_constructor {
        fact.evidence_chain.insert(
            0,
            ResolutionEvidence::ConstructorBinding {
                constructor: NodeId(3),
            },
        );
    }
    fact.provenance.language_adapter = "typescript".to_owned();
    fact.provenance.language_adapter_version = "reference-v6".to_owned();
    seal_call_resolution_fact(fact).expect("seal local receiver fact")
}

fn imported_receiver_fact(with_constructor: bool) -> CallResolutionFact {
    let mut fact = local_receiver_fact(with_constructor);
    fact.raw_edge_target = Some(NodeId(7));
    fact.raw_callsite_identity = Some("1:2:1:7".to_owned());
    fact.target = Some(NodeId(7));
    for evidence in &mut fact.evidence_chain {
        match evidence {
            ResolutionEvidence::ConstructorBinding { constructor } => *constructor = NodeId(6),
            ResolutionEvidence::ExplicitReceiverType { receiver_type } => {
                *receiver_type = NodeId(6)
            }
            ResolutionEvidence::SameFileDeclaration { declaration } => *declaration = NodeId(7),
            _ => {}
        }
    }
    fact.evidence_chain.insert(
        0,
        ResolutionEvidence::StaticImportBinding {
            import: NodeId(8),
            declaration: NodeId(6),
        },
    );
    fact.provenance
        .dependency_file_hashes
        .push(DependencyFileHash {
            file_id: FileId(5),
            source_sha256: "b".repeat(64),
        });
    seal_call_resolution_fact(fact).expect("seal imported receiver fact")
}

fn rust_imported_implicit_receiver_fact() -> CallResolutionFact {
    let mut fact = exact_fact(EdgeId(7));
    fact.callsite.callee_form = CalleeForm::ImplicitReceiver;
    fact.callsite.raw_target = "target".to_owned();
    fact.raw_edge_target = Some(NodeId(4));
    fact.raw_callsite_identity = Some("1:2:1:4".to_owned());
    fact.caller = NodeId(2);
    fact.target = Some(NodeId(4));
    fact.evidence_chain = vec![
        ResolutionEvidence::StaticImportBinding {
            import: NodeId(8),
            declaration: NodeId(6),
        },
        ResolutionEvidence::ImplicitReceiver { owner: NodeId(6) },
        ResolutionEvidence::SameFileDeclaration {
            declaration: NodeId(4),
        },
    ];
    fact.provenance.language_adapter = "rust".to_owned();
    fact.provenance.language_adapter_version = "reference-v9".to_owned();
    fact.provenance
        .dependency_file_hashes
        .push(DependencyFileHash {
            file_id: FileId(5),
            source_sha256: "b".repeat(64),
        });
    seal_call_resolution_fact(fact).expect("seal Rust imported implicit receiver fact")
}

fn seed_local_receiver_graph(store: &mut Store) {
    let file = FileInfo {
        id: 1,
        path: "src/main.ts".into(),
        language: "typescript".to_owned(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 2,
        file_role: FileRole::Source,
    };
    store.insert_file(&file).unwrap();
    store
        .update_file_metadata(&file, Some(&"a".repeat(64)))
        .unwrap();
    for node in [
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "src/main.ts".to_owned(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "caller".to_owned(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(2),
            end_line: Some(2),
            ..Default::default()
        },
        Node {
            id: NodeId(3),
            kind: NodeKind::CLASS,
            serialized_name: "C".to_owned(),
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
        Node {
            id: NodeId(4),
            kind: NodeKind::METHOD,
            serialized_name: "C.target".to_owned(),
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
    ] {
        store.insert_node(&node).unwrap();
    }
    for edge in [
        Edge {
            id: EdgeId(7),
            source: NodeId(2),
            target: NodeId(4),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(1)),
            line: Some(2),
            resolved_target: Some(NodeId(4)),
            callsite_identity: Some("1:2:1:4".to_owned()),
            ..Default::default()
        },
        Edge {
            id: EdgeId(8),
            source: NodeId(3),
            target: NodeId(4),
            kind: EdgeKind::MEMBER,
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
        Edge {
            id: EdgeId(9),
            source: NodeId(2),
            target: NodeId(3),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(1)),
            line: Some(2),
            resolved_target: Some(NodeId(3)),
            callsite_identity: Some("1:2:2:3".to_owned()),
            ..Default::default()
        },
    ] {
        store.insert_edge(&edge).unwrap();
    }
}

fn seed_imported_receiver_graph(store: &mut Store) {
    seed_local_receiver_graph(store);
    store
        .get_connection()
        .execute("DELETE FROM edge", [])
        .unwrap();
    store
        .get_connection()
        .execute("DELETE FROM node WHERE id IN (3, 4)", [])
        .unwrap();
    let target_file = FileInfo {
        id: 5,
        path: "src/other.ts".into(),
        language: "typescript".to_owned(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 2,
        file_role: FileRole::Source,
    };
    store.insert_file(&target_file).unwrap();
    store
        .update_file_metadata(&target_file, Some(&"b".repeat(64)))
        .unwrap();
    for node in [
        Node {
            id: NodeId(5),
            kind: NodeKind::FILE,
            serialized_name: "src/other.ts".to_owned(),
            ..Default::default()
        },
        Node {
            id: NodeId(6),
            kind: NodeKind::CLASS,
            serialized_name: "C".to_owned(),
            file_node_id: Some(NodeId(5)),
            ..Default::default()
        },
        Node {
            id: NodeId(7),
            kind: NodeKind::METHOD,
            serialized_name: "C.target".to_owned(),
            file_node_id: Some(NodeId(5)),
            ..Default::default()
        },
        Node {
            id: NodeId(8),
            kind: NodeKind::MODULE,
            serialized_name: "C".to_owned(),
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
    ] {
        store.insert_node(&node).unwrap();
    }
    for edge in [
        Edge {
            id: EdgeId(7),
            source: NodeId(2),
            target: NodeId(7),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(1)),
            line: Some(2),
            resolved_target: Some(NodeId(7)),
            callsite_identity: Some("1:2:1:7".to_owned()),
            ..Default::default()
        },
        Edge {
            id: EdgeId(8),
            source: NodeId(8),
            target: NodeId(6),
            kind: EdgeKind::IMPORT,
            file_node_id: Some(NodeId(1)),
            resolved_target: Some(NodeId(6)),
            ..Default::default()
        },
        Edge {
            id: EdgeId(9),
            source: NodeId(6),
            target: NodeId(7),
            kind: EdgeKind::MEMBER,
            file_node_id: Some(NodeId(5)),
            ..Default::default()
        },
        Edge {
            id: EdgeId(10),
            source: NodeId(2),
            target: NodeId(6),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(1)),
            line: Some(2),
            resolved_target: Some(NodeId(6)),
            callsite_identity: Some("1:2:2:6".to_owned()),
            ..Default::default()
        },
    ] {
        store.insert_edge(&edge).unwrap();
    }
}

fn seed_rust_imported_implicit_receiver_graph(store: &mut Store) {
    for file in [
        FileInfo {
            id: 1,
            path: "src/lib.rs".into(),
            language: "rust".to_owned(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 3,
            file_role: FileRole::Source,
        },
        FileInfo {
            id: 5,
            path: "src/owner.rs".into(),
            language: "rust".to_owned(),
            modification_time: 0,
            indexed: true,
            complete: true,
            line_count: 1,
            file_role: FileRole::Source,
        },
    ] {
        let hash = if file.id == 1 {
            "a".repeat(64)
        } else {
            "b".repeat(64)
        };
        store.insert_file(&file).expect("file");
        store
            .update_file_metadata(&file, Some(&hash))
            .expect("source hash");
    }
    for node in [
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "src/lib.rs".to_owned(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::METHOD,
            serialized_name: "Owner.caller".to_owned(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(2),
            end_line: Some(2),
            ..Default::default()
        },
        Node {
            id: NodeId(4),
            kind: NodeKind::METHOD,
            serialized_name: "Owner.target".to_owned(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(1),
            end_line: Some(1),
            ..Default::default()
        },
        Node {
            id: NodeId(5),
            kind: NodeKind::FILE,
            serialized_name: "src/owner.rs".to_owned(),
            ..Default::default()
        },
        Node {
            id: NodeId(6),
            kind: NodeKind::STRUCT,
            serialized_name: "Owner".to_owned(),
            file_node_id: Some(NodeId(5)),
            ..Default::default()
        },
        Node {
            id: NodeId(8),
            kind: NodeKind::MODULE,
            serialized_name: "Owner".to_owned(),
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
    ] {
        store.insert_node(&node).expect("node");
    }
    for edge in [
        Edge {
            id: EdgeId(7),
            source: NodeId(2),
            target: NodeId(4),
            kind: EdgeKind::CALL,
            file_node_id: Some(NodeId(1)),
            line: Some(2),
            resolved_target: Some(NodeId(4)),
            callsite_identity: Some("1:2:1:4".to_owned()),
            ..Default::default()
        },
        Edge {
            id: EdgeId(8),
            source: NodeId(8),
            target: NodeId(6),
            kind: EdgeKind::IMPORT,
            file_node_id: Some(NodeId(1)),
            resolved_target: Some(NodeId(6)),
            ..Default::default()
        },
        Edge {
            id: EdgeId(9),
            source: NodeId(6),
            target: NodeId(2),
            kind: EdgeKind::MEMBER,
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
        Edge {
            id: EdgeId(10),
            source: NodeId(6),
            target: NodeId(4),
            kind: EdgeKind::MEMBER,
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        },
    ] {
        store.insert_edge(&edge).expect("edge");
    }
}

fn seed_repeated_exact_graph(store: &mut Store, fact_count: u32) {
    seed_exact_graph(store);
    for index in 1..fact_count {
        store
            .insert_edge(&Edge {
                id: EdgeId(7 + i64::from(index)),
                source: NodeId(2),
                target: NodeId(4),
                kind: EdgeKind::CALL,
                file_node_id: Some(NodeId(1)),
                line: Some(2),
                resolved_target: Some(NodeId(3)),
                callsite_identity: Some(format!("1:2:{}:4", index + 1)),
                ..Default::default()
            })
            .expect("repeated call edge");
    }
}

fn graph_read_authorizations_for_projection(fact_count: u32) -> usize {
    let mut store = Store::new_in_memory().expect("store");
    seed_repeated_exact_graph(&mut store, fact_count);
    let graph_reads = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&graph_reads);
    store
        .get_connection()
        .authorizer(Some(move |context: AuthContext<'_>| {
            if matches!(
                context.action,
                AuthAction::Read {
                    table_name: "file" | "node" | "edge",
                    ..
                }
            ) {
                observed.fetch_add(1, Ordering::Relaxed);
            }
            Authorization::Allow
        }))
        .expect("install graph read observer");

    let facts = (0..fact_count).map(repeated_exact_fact).collect();
    store
        .replace_proof_resolution_projection(&publication(), &projection(facts))
        .expect("publish repeated exact facts");
    store
        .get_connection()
        .authorizer(None::<fn(AuthContext<'_>) -> Authorization>)
        .expect("remove graph read observer");
    graph_reads.load(Ordering::Relaxed)
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
fn go_exact_projection_requires_the_complete_package_dependency_set() {
    let mut accepted = Store::new_in_memory().unwrap();
    let (_accepted_files, accepted_dependencies) = seed_go_exact_graph(&mut accepted, true);
    accepted
        .replace_proof_resolution_projection(
            &publication(),
            &go_projection(go_exact_fact(&accepted_dependencies)),
        )
        .expect("complete Go package closure must validate");

    let mut missing = Store::new_in_memory().unwrap();
    let (_missing_files, missing_dependencies) = seed_go_exact_graph(&mut missing, true);
    missing
        .replace_proof_resolution_projection(
            &publication(),
            &go_projection(go_exact_fact(&missing_dependencies[..1])),
        )
        .expect_err("missing Go package dependency must fail closed");

    let mut extra = Store::new_in_memory().unwrap();
    let (_extra_files, mut extra_dependencies) = seed_go_exact_graph(&mut extra, true);
    let unrelated = FileInfo {
        id: 6,
        path: "other/unrelated.go".into(),
        language: "go".to_owned(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 1,
        file_role: FileRole::Source,
    };
    extra.insert_file(&unrelated).unwrap();
    extra
        .update_file_metadata(&unrelated, Some(&"c".repeat(64)))
        .unwrap();
    extra_dependencies.push((6, "c".repeat(64)));
    extra
        .replace_proof_resolution_projection(
            &publication(),
            &go_projection(go_exact_fact(&extra_dependencies)),
        )
        .expect_err("extra Go package dependency must fail closed");
}

#[test]
fn go_store_rejects_evidence_scope_swaps_and_unauthenticated_imports() {
    for mutate in [
        (|fact: &mut CallResolutionFact| {
            fact.evidence_chain = vec![ResolutionEvidence::SamePackageDeclaration {
                declaration: NodeId(3),
            }];
        }) as fn(&mut CallResolutionFact),
        |fact: &mut CallResolutionFact| {
            fact.evidence_chain = vec![ResolutionEvidence::StaticImportBinding {
                import: NodeId(4),
                declaration: NodeId(3),
            }];
        },
    ] {
        let mut store = Store::new_in_memory().unwrap();
        let (_files, dependencies) = seed_go_exact_graph(&mut store, false);
        let mut fact = go_exact_fact(&dependencies);
        mutate(&mut fact);
        let fact = seal_call_resolution_fact(fact).unwrap();
        store
            .replace_proof_resolution_projection(&publication(), &go_projection(fact))
            .expect_err("forged Go evidence must fail closed");
    }
}

#[test]
fn go_store_replay_requires_exact_package_clause_and_native_directory_identity() {
    let crossed = tempfile::tempdir().expect("crossed package tempdir");
    let mut crossed_store = Store::new_in_memory().unwrap();
    let crossed_fact = seed_authenticated_go_cross_file_graph(
        &mut crossed_store,
        crossed.path(),
        "proof",
        "other",
        false,
    );
    crossed_store
        .replace_proof_resolution_projection(&publication(), &go_projection(crossed_fact))
        .expect_err("resealed cross-package-clause Go evidence must fail closed");

    let aliased = tempfile::tempdir().expect("native alias tempdir");
    let mut aliased_store = Store::new_in_memory().unwrap();
    let aliased_fact = seed_authenticated_go_cross_file_graph(
        &mut aliased_store,
        aliased.path(),
        "proof",
        "proof",
        true,
    );
    aliased_store
        .replace_proof_resolution_projection(&publication(), &go_projection(aliased_fact))
        .expect("native aliases of one authenticated Go package directory must agree");
}

#[test]
fn go_store_replay_rejects_identifier_authority_over_a_method_target() {
    let mut store = Store::new_in_memory().unwrap();
    let (_files, dependencies) = seed_go_exact_graph(&mut store, false);
    store
        .get_connection()
        .execute("UPDATE node SET kind = 14 WHERE id = 3", [])
        .unwrap();
    let stored_kind: i32 = store
        .get_connection()
        .query_row("SELECT kind FROM node WHERE id = 3", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored_kind, NodeKind::METHOD as i32);
    store
        .replace_proof_resolution_projection(
            &publication(),
            &go_projection(go_exact_fact(&dependencies)),
        )
        .expect_err("Go identifier evidence must authorize only a FUNCTION target");
}

#[test]
fn literal_receiver_evidence_shapes_round_trip() {
    for with_constructor in [false, true] {
        let mut local = Store::new_in_memory().unwrap();
        seed_local_receiver_graph(&mut local);
        let local_fact = local_receiver_fact(with_constructor);
        local
            .replace_proof_resolution_projection(
                &publication(),
                &receiver_projection(local_fact.clone()),
            )
            .expect("local receiver evidence must validate");
        assert_eq!(
            local
                .get_exact_proof_resolution_fact_by_edge(EdgeId(7))
                .unwrap(),
            Some(local_fact)
        );

        let mut imported = Store::new_in_memory().unwrap();
        seed_imported_receiver_graph(&mut imported);
        let imported_fact = imported_receiver_fact(with_constructor);
        imported
            .replace_proof_resolution_projection(
                &publication(),
                &receiver_projection(imported_fact.clone()),
            )
            .expect("imported receiver evidence must validate");
        assert_eq!(
            imported
                .get_exact_proof_resolution_fact_by_edge(EdgeId(7))
                .unwrap(),
            Some(imported_fact)
        );
    }
}

#[test]
fn rust_imported_implicit_receiver_literal_shape_round_trips() {
    let mut store = Store::new_in_memory().unwrap();
    seed_rust_imported_implicit_receiver_graph(&mut store);
    let fact = rust_imported_implicit_receiver_fact();
    store
        .replace_proof_resolution_projection(
            &publication(),
            &rust_receiver_projection(fact.clone()),
        )
        .expect("Rust imported implicit receiver evidence must validate");
    assert_eq!(
        store
            .get_exact_proof_resolution_fact_by_edge(EdgeId(7))
            .unwrap(),
        Some(fact)
    );
}

#[test]
fn rust_imported_implicit_receiver_rejects_nonliteral_import_and_member_relations() {
    for sql in [
        "DELETE FROM edge WHERE id = 8",
        "UPDATE edge SET candidate_target_node_ids = '[6]' WHERE id = 8",
        "UPDATE edge SET resolved_source_node_id = 1 WHERE id = 8",
        "UPDATE edge SET file_node_id = 5 WHERE id = 8",
        "INSERT INTO edge SELECT 18, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence, certainty, callsite_identity, candidate_target_node_ids FROM edge WHERE id = 8",
        "DELETE FROM edge WHERE id = 9",
        "DELETE FROM edge WHERE id = 10",
        "UPDATE edge SET candidate_target_node_ids = '[2]' WHERE id = 9",
        "UPDATE edge SET source_node_id = 1, resolved_source_node_id = 6 WHERE id = 9",
        "UPDATE edge SET target_node_id = 1, resolved_target_node_id = 2 WHERE id = 9",
        "UPDATE edge SET file_node_id = 5 WHERE id = 9",
        "INSERT INTO edge SELECT 19, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence, certainty, callsite_identity, candidate_target_node_ids FROM edge WHERE id = 9",
        "UPDATE edge SET candidate_target_node_ids = '[4]' WHERE id = 10",
        "UPDATE edge SET source_node_id = 1, resolved_source_node_id = 6 WHERE id = 10",
        "UPDATE edge SET target_node_id = 1, resolved_target_node_id = 4 WHERE id = 10",
        "UPDATE edge SET file_node_id = 5 WHERE id = 10",
        "INSERT INTO edge SELECT 20, source_node_id, target_node_id, kind, file_node_id, line, resolved_source_node_id, resolved_target_node_id, confidence, certainty, callsite_identity, candidate_target_node_ids FROM edge WHERE id = 10",
        "UPDATE node SET kind = 0 WHERE id = 6",
        "UPDATE node SET kind = 13 WHERE id = 2",
        "UPDATE node SET file_node_id = 5 WHERE id = 4",
    ] {
        let mut store = Store::new_in_memory().unwrap();
        seed_rust_imported_implicit_receiver_graph(&mut store);
        store.get_connection().execute(sql, []).unwrap();
        let result = store.replace_proof_resolution_projection(
            &publication(),
            &rust_receiver_projection(rust_imported_implicit_receiver_fact()),
        );
        assert!(
            result.is_err(),
            "hostile imported S4 relation was accepted: {sql}"
        );
    }
}

#[test]
fn literal_receiver_evidence_rejects_permuted_missing_and_unrelated_graph_proof() {
    let assert_local_rejected = |mutate: fn(&mut CallResolutionFact), graph_sql: Option<&str>| {
        let mut store = Store::new_in_memory().unwrap();
        seed_local_receiver_graph(&mut store);
        if let Some(sql) = graph_sql {
            store.get_connection().execute(sql, []).unwrap();
        }
        let mut fact = local_receiver_fact(true);
        mutate(&mut fact);
        let fact = seal_call_resolution_fact(fact).unwrap();
        store
            .replace_proof_resolution_projection(&publication(), &receiver_projection(fact))
            .expect_err("mutated local receiver proof must fail closed");
    };
    assert_local_rejected(|fact| fact.evidence_chain.swap(0, 1), None);
    assert_local_rejected(
        |fact| {
            fact.evidence_chain[0] = ResolutionEvidence::ConstructorBinding {
                constructor: NodeId(4),
            };
        },
        None,
    );
    assert_local_rejected(
        |fact| {
            fact.evidence_chain.remove(1);
        },
        None,
    );
    assert_local_rejected(
        |fact| {
            fact.evidence_chain.remove(2);
        },
        None,
    );
    assert_local_rejected(
        |fact| fact.callsite.callee_form = CalleeForm::Identifier,
        None,
    );
    assert_local_rejected(|_| {}, Some("DELETE FROM edge WHERE id = 8"));
    assert_local_rejected(|_| {}, Some("UPDATE node SET kind = 13 WHERE id = 4"));

    let assert_imported_rejected = |mutate: fn(&mut CallResolutionFact),
                                    graph_sql: Option<&str>| {
        let mut store = Store::new_in_memory().unwrap();
        seed_imported_receiver_graph(&mut store);
        if let Some(sql) = graph_sql {
            store.get_connection().execute(sql, []).unwrap();
        }
        let mut fact = imported_receiver_fact(true);
        mutate(&mut fact);
        let fact = seal_call_resolution_fact(fact).unwrap();
        store
            .replace_proof_resolution_projection(&publication(), &receiver_projection(fact))
            .expect_err("mutated imported receiver proof must fail closed");
    };
    assert_imported_rejected(|fact| fact.evidence_chain.swap(0, 1), None);
    for index in [0, 2, 3] {
        assert_imported_rejected(
            match index {
                0 => |fact| {
                    fact.evidence_chain.remove(0);
                },
                1 => |fact| {
                    fact.evidence_chain.remove(1);
                },
                2 => |fact| {
                    fact.evidence_chain.remove(2);
                },
                _ => |fact| {
                    fact.evidence_chain.remove(3);
                },
            },
            None,
        );
    }
    assert_imported_rejected(
        |fact| {
            fact.evidence_chain[1] = ResolutionEvidence::ConstructorBinding {
                constructor: NodeId(7),
            };
        },
        None,
    );
    assert_imported_rejected(
        |fact| {
            fact.provenance.dependency_file_hashes.pop();
        },
        None,
    );
    assert_imported_rejected(|_| {}, Some("DELETE FROM edge WHERE id = 8"));
    assert_imported_rejected(|_| {}, Some("DELETE FROM edge WHERE id = 9"));
    assert_imported_rejected(
        |_| {},
        Some("UPDATE edge SET candidate_target_node_ids = '[6]' WHERE id = 8"),
    );
    assert_imported_rejected(
        |_| {},
        Some("UPDATE edge SET resolved_source_node_id = 1 WHERE id = 8"),
    );
    assert_imported_rejected(
        |_| {},
        Some("UPDATE edge SET file_node_id = 5 WHERE id = 8"),
    );
    assert_imported_rejected(
        |_| {},
        Some(
            "INSERT INTO edge
             SELECT 18, source_node_id, target_node_id, kind, file_node_id, line,
                    resolved_source_node_id, resolved_target_node_id, confidence, certainty,
                    callsite_identity, candidate_target_node_ids
             FROM edge WHERE id = 8",
        ),
    );
    assert_imported_rejected(
        |_| {},
        Some("UPDATE edge SET candidate_target_node_ids = '[7]' WHERE id = 9"),
    );
    assert_imported_rejected(
        |_| {},
        Some("UPDATE edge SET source_node_id = 1, resolved_source_node_id = 6 WHERE id = 9"),
    );
    assert_imported_rejected(
        |_| {},
        Some("UPDATE edge SET target_node_id = 1, resolved_target_node_id = 7 WHERE id = 9"),
    );
    assert_imported_rejected(
        |_| {},
        Some("UPDATE edge SET file_node_id = 1 WHERE id = 9"),
    );
    assert_imported_rejected(|_| {}, Some("UPDATE node SET kind = 0 WHERE id = 6"));
    assert_imported_rejected(
        |_| {},
        Some("UPDATE node SET file_node_id = 1 WHERE id = 7"),
    );
    assert_imported_rejected(|_| {}, Some("UPDATE node SET kind = 13 WHERE id = 7"));
}

#[test]
fn projection_validation_prepares_graph_reads_once_for_all_exact_facts() {
    let one_fact_reads = graph_read_authorizations_for_projection(1);
    let eight_fact_reads = graph_read_authorizations_for_projection(8);

    assert!(one_fact_reads > 0, "the graph read observer must be active");
    assert_eq!(
        eight_fact_reads, one_fact_reads,
        "graph-table reads must be prepared once per projection, not once per fact"
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

    let mut unrelated_import = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut unrelated_import);
    unrelated_import
        .insert_edge(&Edge {
            id: EdgeId(8),
            source: NodeId(2),
            target: NodeId(3),
            kind: EdgeKind::IMPORT,
            file_node_id: Some(NodeId(1)),
            resolved_target: Some(NodeId(3)),
            ..Default::default()
        })
        .unwrap();
    let mut wrong_named_import = exact_fact(EdgeId(7));
    wrong_named_import.callsite.callee_form = CalleeForm::NamedImport;
    wrong_named_import.evidence_chain = vec![ResolutionEvidence::StaticImportBinding {
        import: NodeId(2),
        declaration: NodeId(3),
    }];
    let wrong_named_import = seal_call_resolution_fact(wrong_named_import).unwrap();
    let mut wrong_named_import_projection = projection(vec![wrong_named_import]);
    wrong_named_import_projection.funnel[0].callee_form = Some(CalleeForm::NamedImport);
    wrong_named_import_projection.funnel[0].evidence_kind =
        Some(ResolutionEvidenceKind::StaticImportBinding);
    let error = unrelated_import
        .replace_proof_resolution_projection(&publication(), &wrong_named_import_projection)
        .expect_err("an unrelated source node must not authenticate an import binding");
    assert!(error.to_string().contains("StaticImportBinding"), "{error}");

    let mut cross_file_receiver = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut cross_file_receiver);
    let target_file = FileInfo {
        id: 5,
        path: "src/other.rs".into(),
        language: "rust".to_owned(),
        modification_time: 0,
        indexed: true,
        complete: true,
        line_count: 1,
        file_role: FileRole::Source,
    };
    cross_file_receiver.insert_file(&target_file).unwrap();
    cross_file_receiver
        .update_file_metadata(&target_file, Some(&"b".repeat(64)))
        .unwrap();
    cross_file_receiver
        .insert_node(&Node {
            id: NodeId(5),
            kind: NodeKind::FILE,
            serialized_name: "src/other.rs".to_owned(),
            ..Default::default()
        })
        .unwrap();
    let mut target = cross_file_receiver.get_node(NodeId(3)).unwrap().unwrap();
    cross_file_receiver
        .get_connection()
        .execute("DELETE FROM edge WHERE id = 7", [])
        .unwrap();
    target.file_node_id = Some(NodeId(5));
    target.kind = NodeKind::METHOD;
    cross_file_receiver.insert_node(&target).unwrap();
    cross_file_receiver
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
        .unwrap();
    cross_file_receiver
        .insert_node(&Node {
            id: NodeId(6),
            kind: NodeKind::STRUCT,
            serialized_name: "Owner".to_owned(),
            file_node_id: Some(NodeId(1)),
            ..Default::default()
        })
        .unwrap();
    for (id, member) in [(8, NodeId(2)), (9, NodeId(3))] {
        cross_file_receiver
            .insert_edge(&Edge {
                id: EdgeId(id),
                source: NodeId(6),
                target: member,
                kind: EdgeKind::MEMBER,
                file_node_id: Some(NodeId(1)),
                ..Default::default()
            })
            .unwrap();
    }
    let mut wrong_receiver = exact_fact(EdgeId(7));
    wrong_receiver.callsite.callee_form = CalleeForm::ImplicitReceiver;
    wrong_receiver.evidence_chain = vec![
        ResolutionEvidence::ImplicitReceiver { owner: NodeId(6) },
        ResolutionEvidence::SameFileDeclaration {
            declaration: NodeId(3),
        },
    ];
    wrong_receiver
        .provenance
        .dependency_file_hashes
        .push(DependencyFileHash {
            file_id: FileId(5),
            source_sha256: "b".repeat(64),
        });
    let wrong_receiver = seal_call_resolution_fact(wrong_receiver).unwrap();
    assert_eq!(
        wrong_receiver.provenance.dependency_file_hashes,
        vec![
            DependencyFileHash {
                file_id: FileId(1),
                source_sha256: "a".repeat(64),
            },
            DependencyFileHash {
                file_id: FileId(5),
                source_sha256: "b".repeat(64),
            },
        ]
    );
    assert_eq!(
        cross_file_receiver
            .get_node(NodeId(3))
            .unwrap()
            .unwrap()
            .file_node_id,
        Some(NodeId(5))
    );
    let mut wrong_receiver_projection = projection(vec![wrong_receiver]);
    wrong_receiver_projection.funnel[0].callee_form = Some(CalleeForm::ImplicitReceiver);
    wrong_receiver_projection.funnel[0].evidence_kind =
        Some(ResolutionEvidenceKind::ImplicitReceiver);
    let error = cross_file_receiver
        .replace_proof_resolution_projection(&publication(), &wrong_receiver_projection)
        .expect_err("implicit receiver evidence cannot claim a cross-file same-file declaration");
    assert!(error.to_string().contains("ImplicitReceiver"), "{error}");

    let mut wrong_funnel = projection(vec![exact_fact(EdgeId(7))]);
    wrong_funnel.funnel[0].counts.syntax_calls = 2;
    let error = store
        .replace_proof_resolution_projection(&publication(), &wrong_funnel)
        .expect_err("funnel mismatch must fail");
    assert!(error.to_string().contains("funnel"), "{error}");
}

#[test]
fn resealed_raw_callsite_and_incomplete_dependency_mutations_are_rejected() {
    let mut wrong_placeholder = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut wrong_placeholder);
    let mut placeholder = wrong_placeholder.get_node(NodeId(4)).unwrap().unwrap();
    placeholder.serialized_name = "other".to_owned();
    wrong_placeholder.insert_node(&placeholder).unwrap();
    let error = wrong_placeholder
        .replace_proof_resolution_projection(
            &publication(),
            &projection(vec![exact_fact(EdgeId(7))]),
        )
        .expect_err("raw placeholder spelling mismatch must fail");
    assert!(error.to_string().contains("placeholder"), "{error}");

    let mut incomplete = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut incomplete);
    incomplete
        .get_connection()
        .execute("UPDATE file SET complete = 0 WHERE id = 1", [])
        .unwrap();
    let error = incomplete
        .replace_proof_resolution_projection(
            &publication(),
            &projection(vec![exact_fact(EdgeId(7))]),
        )
        .expect_err("incomplete dependency file must fail");
    assert!(error.to_string().contains("indexed-complete"), "{error}");

    let mut inconsistent_file = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut inconsistent_file);
    inconsistent_file
        .get_connection()
        .execute(
            "UPDATE edge SET callsite_identity = '9:2:1:4' WHERE id = 7",
            [],
        )
        .unwrap();
    let mut fact = exact_fact(EdgeId(7));
    fact.raw_callsite_identity = Some("9:2:1:4".to_owned());
    let fact = seal_call_resolution_fact(fact).unwrap();
    let error = inconsistent_file
        .replace_proof_resolution_projection(&publication(), &projection(vec![fact]))
        .expect_err("callsite identity file must match the exact syntax span");
    assert!(error.to_string().contains("callsite identity"), "{error}");
}

#[test]
fn incomplete_domain_fact_allows_hashed_indexed_parser_incomplete_source() {
    let mut store = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut store);
    store
        .get_connection()
        .execute("UPDATE file SET complete = 0 WHERE id = 1", [])
        .unwrap();

    let receipt = store
        .replace_proof_resolution_projection(&publication(), &incomplete_domain_projection())
        .expect("parser incompleteness is a fact status, not publication corruption");

    assert_eq!(receipt.fact_count, 1);
    let facts = store.get_proof_resolution_facts().unwrap();
    assert_eq!(facts[0].status, ProofResolutionStatus::IncompleteDomain);
    assert_eq!(facts[0].provenance.dependency_file_hashes.len(), 1);
}

#[test]
fn incomplete_domain_fact_still_rejects_unindexed_missing_or_mismatched_source_identity() {
    let mut unindexed = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut unindexed);
    unindexed
        .get_connection()
        .execute("UPDATE file SET indexed = 0 WHERE id = 1", [])
        .unwrap();
    let error = unindexed
        .replace_proof_resolution_projection(&publication(), &incomplete_domain_projection())
        .expect_err("non-Exact cannot authenticate an unindexed source");
    assert!(error.to_string().contains("indexed-complete"), "{error}");

    let mut missing_hash = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut missing_hash);
    missing_hash
        .get_connection()
        .execute("UPDATE file SET content_hash = NULL WHERE id = 1", [])
        .unwrap();
    let error = missing_hash
        .replace_proof_resolution_projection(&publication(), &incomplete_domain_projection())
        .expect_err("non-Exact source hash absence is integrity corruption");
    assert!(error.to_string().contains("source hash"), "{error}");

    let mut mismatched_hash = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut mismatched_hash);
    mismatched_hash
        .get_connection()
        .execute(
            "UPDATE file SET content_hash = ?1 WHERE id = 1",
            ["b".repeat(64)],
        )
        .unwrap();
    let error = mismatched_hash
        .replace_proof_resolution_projection(&publication(), &incomplete_domain_projection())
        .expect_err("non-Exact source hash mismatch is integrity corruption");
    assert!(error.to_string().contains("source hash"), "{error}");
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

#[test]
fn incremental_fence_invalidates_proof_overlay_before_graph_mutation() {
    let mut store = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut store);
    store
        .replace_proof_resolution_projection(
            &publication(),
            &projection(vec![exact_fact(EdgeId(7))]),
        )
        .unwrap();

    store.begin_incremental_run().unwrap();

    assert_eq!(store.get_proof_resolution_publication().unwrap(), None);
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 0);
    store
        .get_connection()
        .execute("DELETE FROM edge WHERE id = 7", [])
        .expect("the staged graph may mutate after proof invalidation");
}

#[test]
fn failed_incremental_fence_preserves_the_complete_proof_overlay() {
    let mut store = Store::new_in_memory().unwrap();
    seed_exact_graph(&mut store);
    let proof = store
        .replace_proof_resolution_projection(
            &publication(),
            &projection(vec![exact_fact(EdgeId(7))]),
        )
        .unwrap();
    store
        .get_connection()
        .execute_batch(
            "CREATE TRIGGER fail_incomplete_begin
             BEFORE INSERT ON incomplete_index_run
             BEGIN SELECT RAISE(ABORT, 'forced marker insert failure'); END;",
        )
        .unwrap();

    store.begin_incremental_run().expect_err("fence must fail");

    assert_eq!(
        store.get_proof_resolution_publication().unwrap(),
        Some(proof)
    );
    assert_eq!(store.proof_resolution_fact_count().unwrap(), 1);
    store
        .validate_proof_resolution_publication(&publication())
        .expect("the prior proof overlay survived rollback");
}
