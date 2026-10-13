use super::*;

#[test]
fn root_symbols_pages_cover_beyond_legacy_limit_and_are_repeatable() {
    let cache = tempfile::tempdir().unwrap();
    let project = tempdir().unwrap();
    let db = project.path().join("codestory.db");
    let mut nodes: Vec<Node> = (1..=2505)
        .rev()
        .map(|id| Node {
            id: CoreNodeId(id),
            kind: NodeKind::MODULE,
            serialized_name: format!("\"unit_{id:05}\""),
            ..Default::default()
        })
        .collect();
    nodes.push(Node {
        id: CoreNodeId(9000),
        kind: NodeKind::MODULE,
        serialized_name: "\"unit_00001\"".into(),
        ..Default::default()
    });
    Storage::open(&db)
        .unwrap()
        .insert_nodes_batch(&nodes)
        .unwrap();
    let controller = AppController::new_with_owned_cache_root(cache.path());
    controller
        .open_project(OpenProjectRequest {
            path: project.path().to_string_lossy().into(),
        })
        .unwrap();
    let mut collected = Vec::new();
    let mut fingerprint = None;
    for offset in (0..2505).step_by(200) {
        let (page, total, digest) = controller.list_root_symbols_page(offset, 200).unwrap();
        assert_eq!(total, 2505);
        if let Some(expected) = &fingerprint {
            assert_eq!(expected, &digest);
        }
        fingerprint = Some(digest.clone());
        let replay = controller.list_root_symbols_page(offset, 200).unwrap();
        assert_eq!(
            serde_json::to_value(&page).unwrap(),
            serde_json::to_value(&replay.0).unwrap()
        );
        assert_eq!(digest, replay.2);
        collected.extend(page);
    }
    assert_eq!(collected.len(), 2505);
    let ids: std::collections::HashSet<_> = collected.iter().map(|s| s.id.clone()).collect();
    assert_eq!(ids.len(), 2505);
    assert_eq!(
        collected[0].id,
        codestory_contracts::api::NodeId("1".into())
    );
    assert!(!ids.contains(&codestory_contracts::api::NodeId("9000".into())));
    assert_eq!(
        controller
            .list_root_symbols(ListRootSymbolsRequest { limit: None })
            .unwrap()
            .len(),
        300
    );
    let (last, total, same) = controller.list_root_symbols_page(2505, 200).unwrap();
    assert!(last.is_empty());
    assert_eq!(total, 2505);
    assert_eq!(Some(same), fingerprint);
}
