use super::*;

fn flush_extraction_fixture(
    store: &mut Storage,
    start_line: u32,
    hash: &str,
    revision: Option<&str>,
    complete: bool,
) -> Result<ProjectionFlushBreakdown, StorageError> {
    let files = [FileInfo {
        id: 1,
        path: PathBuf::from("fixture.py"),
        language: "python".into(),
        modification_time: 0,
        indexed: true,
        complete,
        line_count: 20,
        file_role: FileRole::Test,
    }];
    let hashes = [FileContentHash {
        file_id: 1,
        content_hash: hash.to_string(),
    }];
    let identities = revision
        .into_iter()
        .map(|revision| FileExtractionIdentity {
            file_id: 1,
            language: "python".into(),
            source_content_hash: hash.to_string(),
            extraction_revision: revision.to_string(),
        })
        .collect::<Vec<_>>();
    let nodes = [
        Node {
            id: NodeId(1),
            kind: NodeKind::FILE,
            serialized_name: "fixture.py".into(),
            ..Default::default()
        },
        Node {
            id: NodeId(2),
            kind: NodeKind::FUNCTION,
            serialized_name: "worker".into(),
            file_node_id: Some(NodeId(1)),
            start_line: Some(start_line),
            start_col: Some(1),
            end_line: Some(15),
            end_col: Some(5),
            ..Default::default()
        },
    ];
    store.flush_projection_batch(ProjectionBatch {
        files: &files,
        file_content_hashes: &hashes,
        file_extraction_identities: &identities,
        nodes: &nodes,
        structural_text_units: &[],
        structural_text_projections: &[],
        structural_text_cache_writes: &[],
        edges: &[],
        occurrences: &[],
        component_access: &[],
        callable_projection_states: &[],
        file_errors: &[],
    })
}

#[test]
fn extraction_identity_commits_with_graph_and_hash_or_rolls_back() -> Result<(), StorageError> {
    let mut store = Storage::new_in_memory()?;
    let old_hash = "a".repeat(64);
    let new_hash = "b".repeat(64);
    flush_extraction_fixture(&mut store, 14, &old_hash, Some("old"), true)?;
    assert!(store.file_extraction_identity_matches(1, "python", "old")?);
    store.conn.execute_batch("CREATE TRIGGER reject_extraction_identity BEFORE INSERT ON file_extraction_identity BEGIN SELECT RAISE(ABORT, 'reject extraction identity'); END")?;
    assert!(flush_extraction_fixture(&mut store, 7, &new_hash, Some("current"), true).is_err());
    assert_eq!(store.get_node(NodeId(2))?.unwrap().start_line, Some(14));
    assert_eq!(
        store.get_file_content_hash(1)?.as_deref(),
        Some(old_hash.as_str())
    );
    assert!(store.file_extraction_identity_matches(1, "python", "old")?);
    assert!(!store.file_extraction_identity_matches(1, "python", "current")?);
    store
        .conn
        .execute_batch("DROP TRIGGER reject_extraction_identity")?;
    flush_extraction_fixture(&mut store, 7, &new_hash, Some("current"), true)?;
    assert_eq!(store.get_node(NodeId(2))?.unwrap().start_line, Some(7));
    assert_eq!(
        store.get_file_content_hash(1)?.as_deref(),
        Some(new_hash.as_str())
    );
    assert!(store.file_extraction_identity_matches(1, "python", "current")?);
    assert!(
        store
            .files_with_extraction_revision_mismatch("python", "current")?
            .is_empty()
    );
    store.delete_file_projection(1)?;
    assert!(!store.file_extraction_identity_matches(1, "python", "current")?);
    Ok(())
}

#[test]
fn incomplete_extraction_cannot_certify_an_inherited_graph() -> Result<(), StorageError> {
    let mut store = Storage::new_in_memory()?;
    let hash = "a".repeat(64);
    flush_extraction_fixture(&mut store, 14, &hash, Some("old"), true)?;
    assert!(flush_extraction_fixture(&mut store, 7, &hash, Some("current"), false).is_err());
    assert_eq!(store.get_node(NodeId(2))?.unwrap().start_line, Some(14));
    assert!(store.file_extraction_identity_matches(1, "python", "old")?);
    assert_eq!(
        store.files_with_extraction_revision_mismatch("python", "current")?,
        vec![1]
    );
    Ok(())
}

#[test]
fn extraction_schema_migration_never_creates_legacy_producer_evidence() -> Result<(), StorageError>
{
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("legacy.db");
    {
        let mut store = Storage::open(&path)?;
        flush_extraction_fixture(&mut store, 14, &"a".repeat(64), None, true)?;
        store
            .conn
            .execute_batch("DROP TABLE file_extraction_identity")?;
        store.set_schema_version(36)?;
    }
    let bytes = std::fs::read(&path).unwrap();
    assert!(Storage::open_freshness_observational(&path).is_err());
    assert_eq!(Storage::database_schema_version_observational(&path)?, 36);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let store = Storage::open(&path)?;
    assert_eq!(store.schema_version()?, CURRENT_SCHEMA_VERSION);
    assert_eq!(store.get_node(NodeId(2))?.unwrap().start_line, Some(14));
    assert_eq!(
        store.files_with_extraction_revision_mismatch("python", "current")?,
        vec![1]
    );
    assert!(!store.file_extraction_identity_matches(1, "python", "current")?);
    Ok(())
}
