#![cfg(feature = "benchmark-support")]

use codestory_contracts::compilation::{
    PacketAdmissionOriginV1, PacketAdmissionReceiptV1, PacketCompilationPublicationV1,
};
use codestory_contracts::evidence_address::{
    ByteRangeV1, EvidenceAnchorV1, LineRangeV1, ProjectRelativePath,
};
use codestory_contracts::graph::{Node, NodeId, NodeKind};
use codestory_contracts::packet_projection_v3::Sha256DigestV3Dto;
use codestory_runtime::benchmark_support::{WitnessSeamDescriptor, run_witness_seam};
use codestory_store::{
    CorePublicationLayout, CorePublishTransaction, CoreReadSession, FileInfo, FileRole, Store,
};
use sha2::{Digest, Sha256};

#[test]
fn witness_seam_changes_only_hydration_under_one_core_pin() {
    let temp = tempfile::tempdir().unwrap();
    let root_path = temp.path().canonicalize().unwrap();
    let root = root_path.as_path();
    std::fs::create_dir(root.join("src")).unwrap();
    let logical = root.join("codestory.db");
    let stage = CorePublicationLayout::from_storage_path(&logical)
        .unwrap()
        .create_staging_database_path()
        .unwrap();
    let store = Store::open(&stage).unwrap();
    let mut descriptors = Vec::new();
    let mut hashes = Vec::new();
    for ordinal in 0..16 {
        let relative = format!("src/unit_{ordinal}.rs");
        let source = format!(
            "{}fn value_{ordinal}() {{\n    work_{ordinal}();\n}}\n",
            "// unrelated header\n".repeat(80)
        );
        std::fs::write(root.join(&relative), &source).unwrap();
        let id = ordinal as i64 + 1;
        store
            .insert_node(&Node {
                id: NodeId(id),
                kind: NodeKind::FILE,
                serialized_name: relative.clone(),
                qualified_name: None,
                canonical_id: None,
                file_node_id: None,
                start_line: None,
                start_col: None,
                end_line: None,
                end_col: None,
            })
            .unwrap();
        store
            .insert_file(&FileInfo {
                id,
                path: root.join(&relative),
                language: "rust".into(),
                modification_time: 1,
                indexed: true,
                complete: ordinal % 2 == 0,
                line_count: 83,
                file_role: FileRole::default(),
            })
            .unwrap();
        // The declaration span contains the match even without a token
        // occurrence on the matched line. Hydration must use syntax extent.
        store
            .insert_node(&Node {
                id: NodeId(id + 100),
                kind: NodeKind::FUNCTION,
                serialized_name: format!("value_{ordinal}"),
                qualified_name: None,
                canonical_id: None,
                file_node_id: Some(NodeId(id)),
                start_line: Some(81),
                start_col: Some(1),
                end_line: Some(83),
                end_col: Some(2),
            })
            .unwrap();
        let digest = format!("{:x}", Sha256::digest(&source));
        hashes.push((id, digest.clone()));
        let start = source.find(&format!("work_{ordinal}")).unwrap();
        descriptors.push(WitnessSeamDescriptor {
            admission: PacketAdmissionReceiptV1 {
                packet_ordinal: ordinal,
                stable_identity: format!("path:{relative}"),
                score_version: "frozen-test/v1".into(),
                reserved_source_bytes: 512,
                origin: PacketAdmissionOriginV1::Retrieval,
            },
            path: Some(ProjectRelativePath::new(relative).unwrap()),
            symbol: None,
            anchor: Some(EvidenceAnchorV1::Match {
                byte_range: ByteRangeV1::new(start as u64, start as u64 + 4).unwrap(),
                line_range: LineRangeV1::new(82, 82).unwrap(),
            }),
            content_digest: Some(Sha256DigestV3Dto::new(digest).unwrap()),
        });
    }
    drop(store);
    let db = rusqlite::Connection::open(&stage).unwrap();
    for (id, hash) in hashes {
        db.execute(
            "UPDATE file SET content_hash=?1 WHERE id=?2",
            rusqlite::params![hash, id],
        )
        .unwrap();
    }
    drop(db);
    CorePublishTransaction::begin_from_stage(&logical, stage)
        .unwrap()
        .commit_rehydrate(&logical)
        .unwrap();
    let pin = CoreReadSession::pin(&logical).unwrap();
    let publication = PacketCompilationPublicationV1 {
        project_id: "fixture-project".into(),
        core_generation_id: pin.identity().generation_id.clone(),
        retrieval_generation: None,
    };
    let pair = run_witness_seam(&pin, None, root, &publication, &descriptors).unwrap();
    assert_eq!(
        pair.control_input.admissions,
        pair.addressed_input.admissions
    );
    assert_eq!(pair.control_input.admissions.len(), 16);
    assert_eq!(
        pair.control_input.publication,
        pair.addressed_input.publication
    );
    assert_eq!(pair.control_input.sources.len(), 16);
    assert_eq!(pair.addressed_input.sources.len(), 16);
    assert_eq!(
        pair.addressed_input
            .sources
            .iter()
            .filter(|source| source.parser_completeness
                == codestory_contracts::compilation::PacketParserCompletenessV1::Partial)
            .count(),
        8,
        "partial parsing does not discard verified source or assert complete coverage"
    );
    assert!(
        pair.control_input
            .sources
            .iter()
            .all(|source| source.start_line == 1 && source.end_line == 9)
    );
    assert!(pair.control_input.sources.iter().all(|source| {
        source.source.contains("unrelated header") && !source.source.contains("work_")
    }));
    assert!(
        pair.addressed_input
            .sources
            .iter()
            .all(|source| source.source.contains("work_"))
    );
    assert!(
        pair.addressed_input
            .sources
            .iter()
            .all(|source| source.source.contains("fn value_")
                && !source.source.contains("unrelated header"))
    );
    assert_eq!(pair.control.support.len(), 16);
    assert_eq!(pair.addressed.support.len(), 16);
    assert!(
        pair.addressed_input
            .sources
            .iter()
            .all(|source| source.source.len() <= 512)
    );

    for count in [0, 1, 15] {
        assert_eq!(
            run_witness_seam(&pin, None, root, &publication, &descriptors[..count])
                .unwrap()
                .addressed_input
                .admissions
                .len(),
            count
        );
    }
    let mut wrong_publication = publication.clone();
    wrong_publication.core_generation_id.push_str("-other");
    assert!(run_witness_seam(&pin, None, root, &wrong_publication, &descriptors).is_err());
    let mut changed_charge = descriptors.clone();
    changed_charge[0].admission.reserved_source_bytes = 513;
    assert!(run_witness_seam(&pin, None, root, &publication, &changed_charge).is_err());
    let mut changed_order = descriptors.clone();
    changed_order.swap(0, 1);
    assert!(run_witness_seam(&pin, None, root, &publication, &changed_order).is_err());
    let mut invalid = descriptors.clone();
    invalid[0].anchor = Some(EvidenceAnchorV1::PathOnly {
        path: invalid[0].path.clone().unwrap(),
    });
    let gaps = run_witness_seam(&pin, None, root, &publication, &invalid).unwrap();
    assert_eq!(
        gaps.control_input.admission_gaps,
        gaps.addressed_input.admission_gaps
    );
    assert_eq!(gaps.addressed_input.sources.len(), 15);
    invalid[0].path = None;
    invalid[0].anchor = None;
    invalid[0].content_digest = None;
    let missing = run_witness_seam(&pin, None, root, &publication, &invalid).unwrap();
    assert_eq!(
        missing.control_input.admission_gaps,
        missing.addressed_input.admission_gaps
    );
    assert_eq!(missing.addressed_input.admissions.len(), 16);
    std::fs::write(root.join("src/unit_0.rs"), "changed\n").unwrap();
    assert!(
        run_witness_seam(&pin, None, root, &publication, &descriptors).is_err(),
        "source replacement invalidates both arms"
    );
}
