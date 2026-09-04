use codestory_contracts::evidence_address::{
    ByteRangeV1, EvidenceAnchorV1, LineRangeV1, ProjectRelativePath,
};
use serde_json::json;

#[test]
fn evidence_addresses_reject_invalid_coordinates_and_path_authority() {
    for (start, end) in [(0, 0), (5, 4)] {
        assert!(ByteRangeV1::new(start, end).is_err());
        assert!(
            serde_json::from_value::<ByteRangeV1>(json!({"start": start, "end": end})).is_err()
        );
    }
    for (start, end) in [(0, 1), (2, 1)] {
        assert!(LineRangeV1::new(start, end).is_err());
        assert!(
            serde_json::from_value::<LineRangeV1>(json!({"start": start, "end": end})).is_err()
        );
    }
    for path in [
        "",
        "/absolute",
        "../outside",
        "src/../outside",
        "src//file",
        "./file",
        "C:/file",
        "src\\file",
        "src/\nfile",
    ] {
        assert!(ProjectRelativePath::new(path).is_err(), "{path:?}");
        assert!(serde_json::from_value::<ProjectRelativePath>(json!(path)).is_err());
    }
    let bytes = ByteRangeV1::new(0, 4).unwrap();
    assert_eq!((bytes.start(), bytes.end()), (0, 4));
    let lines = LineRangeV1::new(1, 1).unwrap();
    assert_eq!((lines.start(), lines.end()), (1, 1));
    assert_eq!(
        ProjectRelativePath::new("src/日本語 file.rs")
            .unwrap()
            .as_str(),
        "src/日本語 file.rs"
    );
}

#[test]
fn nonlexical_anchors_never_gain_invented_match_coordinates() {
    let range = json!({
        "path": "src/unit.rs", "byte_range": {"start": 4, "end": 30},
        "line_range": {"start": 2, "end": 3}, "content_digest": "a".repeat(64),
    });
    for value in [
        json!({"kind": "match", "byte_range": {"start": 4, "end": 8}, "line_range": {"start": 2, "end": 2}}),
        json!({"kind": "indexed_node", "node_id": "node:7", "source_range": range}),
        json!({"kind": "relation_occurrence", "relation_id": "edge:9", "source_range": range}),
        json!({"kind": "path_only", "path": "src/unit.rs"}),
    ] {
        let anchor: EvidenceAnchorV1 = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(anchor).unwrap(), value);
        let mut forged = value.clone();
        forged["matched_line"] = json!(1);
        assert!(serde_json::from_value::<EvidenceAnchorV1>(forged).is_err());
    }
    let mut malformed = json!({"kind": "indexed_node", "node_id": "", "source_range": range});
    assert!(serde_json::from_value::<EvidenceAnchorV1>(malformed.clone()).is_err());
    malformed["node_id"] = json!("node:7");
    malformed["source_range"]["content_digest"] = json!("missing");
    assert!(serde_json::from_value::<EvidenceAnchorV1>(malformed).is_err());
}
