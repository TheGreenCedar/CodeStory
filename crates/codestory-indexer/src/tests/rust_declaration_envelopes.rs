use super::*;

#[test]
fn rust_callable_declarations_keep_attached_metadata_and_execution_ownership() -> Result<()> {
    type Bounds = (u32, u32, u32, u32);
    type Declaration = (&'static str, Bounds, Option<(u32, u32)>);
    struct Case {
        name: &'static str,
        before: &'static str,
        source: &'static str,
        declarations: &'static [Declaration],
        calls: &'static [(&'static str, u32, &'static str, Option<u32>)],
    }

    // Blank baseline lines keep callable tokens fixed, isolating the source envelope
    // from unrelated line movement while requiring IDs and call ownership to survive.
    let cases = [
        Case {
            name: "actual test declaration",
            before: "\nfn native_definitions_follow_nested_declarators() {\n    assert_definition_matrix(&[\"c\", \"cpp\"], NATIVE_FORMS);\n}\n",
            source: "#[test]\nfn native_definitions_follow_nested_declarators() {\n    assert_definition_matrix(&[\"c\", \"cpp\"], NATIVE_FORMS);\n}\n",
            declarations: &[(
                "native_definitions_follow_nested_declarators",
                (1, 1, 4, 2),
                Some((2, 4)),
            )],
            calls: &[(
                "native_definitions_follow_nested_declarators",
                1,
                "assert_definition_matrix",
                None,
            )],
        },
        Case {
            name: "stacked multiline cases and platform gates",
            before: "// outside\n\n\n\n// between attached attributes\n\n\npub async fn worker() {\n    leaf();\n}\nfn leaf() {}\n",
            source: "// outside\n#[test_case(\n    7,\n)]\n// between attached attributes\n#[cfg(all(unix, target_pointer_width = \"64\"))]\n#[cfg_attr(windows, ignore)]\npub async fn worker() {\n    leaf();\n}\nfn leaf() {}\n",
            declarations: &[
                ("worker", (2, 1, 10, 2), Some((8, 10))),
                ("leaf", (11, 1, 11, 13), Some((11, 11))),
            ],
            calls: &[("worker", 2, "leaf", Some(11))],
        },
        Case {
            name: "siblings comments inner attributes and intervening items",
            before: "#![allow(unused)]\n// preceding\n\n/* between attribute and function */\n\n// between attribute and function\nfn first() {}\n\nconst FLAG: bool = true;\n// unrelated preceding\nfn second() {\n    #![allow(unused)]\n    leaf();\n}\nfn leaf() {}\n",
            source: "#![allow(unused)]\n// preceding\n#[test]\n/* between attribute and function */\n#[cfg(unix)]\n// between attribute and function\nfn first() {}\n#[allow(dead_code)]\nconst FLAG: bool = true;\n// unrelated preceding\nfn second() {\n    #![allow(unused)]\n    leaf();\n}\nfn leaf() {}\n",
            declarations: &[
                ("first", (3, 1, 7, 14), Some((7, 7))),
                ("second", (11, 1, 14, 2), Some((11, 14))),
                ("leaf", (15, 1, 15, 13), Some((15, 15))),
            ],
            calls: &[("second", 11, "leaf", Some(15))],
        },
        Case {
            name: "same-line indented attributes and UTF-8 byte endpoint",
            before: "\n    fn worker() -> &'static str { \"é\" }\n",
            source: "    #[test_case(\"é\")] #[cfg(unix)]\n    fn worker() -> &'static str { \"é\" }\n",
            declarations: &[("worker", (1, 5, 2, 41), Some((2, 2)))],
            calls: &[],
        },
        Case {
            name: "attributed bodyless signature and default body",
            before: "trait Ops {\n\n    fn pending(&self);\n\n    fn worker(&self) {\n        leaf();\n    }\n}\nfn leaf() {}\n",
            source: "trait Ops {\n    #[cfg(unix)]\n    fn pending(&self);\n    #[test_case(7)]\n    fn worker(&self) {\n        leaf();\n    }\n}\nfn leaf() {}\n",
            declarations: &[
                ("pending", (2, 5, 3, 23), None),
                ("worker", (4, 5, 7, 6), Some((5, 7))),
                ("leaf", (9, 1, 9, 13), Some((9, 9))),
            ],
            calls: &[("worker", 4, "leaf", Some(9))],
        },
        Case {
            name: "nested and impl workers keep distinct owners",
            before: "fn leaf() {}\nfn outer() {\n\n    fn worker() {\n        leaf();\n    }\n    worker();\n}\nstruct Owner;\nimpl Owner {\n\n\n    fn worker(&self) {\n        leaf();\n    }\n}\n",
            source: "fn leaf() {}\nfn outer() {\n    #[cfg(any())]\n    fn worker() {\n        leaf();\n    }\n    worker();\n}\nstruct Owner;\nimpl Owner {\n    #[test_case(\"é\")]\n    #[cfg_attr(unix, ignore)]\n    fn worker(&self) {\n        leaf();\n    }\n}\n",
            declarations: &[
                ("leaf", (1, 1, 1, 13), Some((1, 1))),
                ("outer", (2, 1, 8, 2), Some((2, 8))),
                ("worker", (3, 5, 6, 6), Some((4, 6))),
                ("worker", (11, 5, 15, 6), Some((13, 15))),
            ],
            calls: &[
                ("outer", 2, "worker", Some(3)),
                ("worker", 3, "leaf", Some(1)),
                ("worker", 11, "leaf", Some(1)),
            ],
        },
    ];

    let path = Path::new("declarations.rs");
    let config = get_language_for_ext("rs").expect("Rust configuration");
    for case in cases {
        let before = index_file(path, case.before, &config, None, None)?;
        let indexed = index_file(path, case.source, &config, None, None)?;
        assert!(before.files[0].complete, "{}: baseline", case.name);
        assert!(
            indexed.files[0].complete,
            "{}: attributed source",
            case.name
        );
        let callable_ids = |result: &IndexResult| {
            result
                .nodes
                .iter()
                .filter(|node| matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD))
                .map(|node| node.id)
                .collect::<HashSet<_>>()
        };
        let ids = callable_ids(&indexed);
        assert_eq!(ids.len(), case.declarations.len(), "{}", case.name);
        assert_eq!(ids, callable_ids(&before), "{}: stable IDs", case.name);

        let bodies = parsed_callable_body_spans(path, case.source)
            .unwrap_or_else(|| panic!("{}: complete parser source", case.name));
        assert_eq!(
            bodies.len(),
            case.declarations
                .iter()
                .filter(|row| row.2.is_some())
                .count(),
            "{}: bodyless declarations cannot gain bodies",
            case.name
        );
        let mut declarations = HashMap::new();
        for &(name, bounds, body) in case.declarations {
            let nodes = indexed
                .nodes
                .iter()
                .filter(|node| {
                    matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                        && node_matches_name(node, name)
                        && node.end_line == Some(bounds.2)
                })
                .collect::<Vec<_>>();
            assert_eq!(nodes.len(), 1, "{}: {name} ending {}", case.name, bounds.2);
            let node = nodes[0];
            assert_eq!(
                (node.start_line, node.start_col, node.end_line, node.end_col),
                (
                    Some(bounds.0),
                    Some(bounds.1),
                    Some(bounds.2),
                    Some(bounds.3)
                ),
                "{}: {name} attached declaration envelope",
                case.name
            );
            declarations.insert((name, bounds.0), node.id);
            let joined = bodies
                .iter()
                .filter(|span| {
                    (
                        span.start_line,
                        span.start_column,
                        span.end_line,
                        span.end_column,
                    ) == bounds
                })
                .collect::<Vec<_>>();
            if let Some(expected_body) = body {
                assert_eq!(joined.len(), 1, "{}: {name} exact body join", case.name);
                assert_eq!(
                    (joined[0].body_start_line, joined[0].body_end_line),
                    expected_body,
                    "{}: {name} execution body excludes outer metadata",
                    case.name
                );
            } else {
                assert!(
                    joined.is_empty(),
                    "{}: {name} has no execution body",
                    case.name
                );
            }
        }

        let calls = |result: &IndexResult| {
            result
                .edges
                .iter()
                .filter(|edge| edge.kind == EdgeKind::CALL)
                .map(|edge| (edge.effective_source(), edge.effective_target()))
                .collect::<HashSet<_>>()
        };
        let actual_calls = calls(&indexed);
        let baseline_calls = calls(&before);
        assert_eq!(
            actual_calls, baseline_calls,
            "{}: call identities",
            case.name
        );
        assert_eq!(
            actual_calls.len(),
            case.calls.len(),
            "{}: call count",
            case.name
        );
        for &(caller, start, callee, callee_start) in case.calls {
            let caller = declarations[&(caller, start)];
            if let Some(callee_start) = callee_start {
                let declaration = declarations[&(callee, callee_start)];
                let targets = baseline_calls
                    .iter()
                    .filter(|(source, _)| *source == caller)
                    .map(|(_, target)| *target)
                    .collect::<Vec<_>>();
                assert_eq!(targets.len(), 1, "{}: baseline caller target", case.name);
                let target = targets[0];
                if target != declaration {
                    // index_file may retain an unresolved reference; do not require
                    // new resolution, or accept a different same-name declaration.
                    let reference = before.nodes.iter().find(|node| node.id == target).unwrap();
                    assert_eq!(reference.kind, NodeKind::UNKNOWN, "{}", case.name);
                    assert!(node_matches_name(reference, callee), "{}", case.name);
                }
                assert!(
                    actual_calls.contains(&(caller, target)),
                    "{}: exact declaration or unchanged unresolved target",
                    case.name
                );
                continue;
            }
            assert!(
                actual_calls.iter().any(|(source, target)| {
                    *source == caller
                        && indexed
                            .nodes
                            .iter()
                            .any(|node| node.id == *target && node_matches_name(node, callee))
                }),
                "{}: owner {caller:?} must call {callee}; actual={actual_calls:?}",
                case.name
            );
        }
    }
    Ok(())
}

#[test]
fn stale_rust_extraction_replaces_old_macro_duplicates_even_with_current_cached_projection()
-> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("worker.rs");
    let empty_path = dir.path().join("empty.rs");
    std::fs::write(
        &path,
        "// before\n#[test]\nfn worker() {\n    println!(\"Hello\");\n}\n#[derive(Debug)]\npub struct Record { pub value: u8 }\n",
    )?;
    std::fs::write(&empty_path, "const VALUE: u8 = 1;\n")?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let mut store = Storage::new_in_memory()?;
    let plan = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path.clone(), empty_path.clone()],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    indexer.run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    let revision = current_file_extraction_revision("rust").unwrap();
    let file = store.get_file_by_path(&path)?.unwrap();
    let empty = store.get_file_by_path(&empty_path)?.unwrap();
    assert!(store.file_extraction_identity_matches(file.id, "rust", revision)?);
    assert!(
        store.file_extraction_identity_matches(empty.id, "rust", revision)?,
        "zero-callable Rust files retain successful producer evidence"
    );
    let worker = store
        .get_nodes()?
        .into_iter()
        .find(|node| {
            node_matches_name(node, "worker")
                && matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        })
        .unwrap();
    let stored_span = |store: &Storage| -> Result<(u32, u32, u32, u32)> {
        Ok(store.get_connection().query_row(
            "SELECT start_line, start_col, end_line, end_col FROM node WHERE id = ?1",
            [worker.id.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?)
    };
    assert_eq!(stored_span(&store)?, (2, 1, 5, 2));
    let source_hash = store.get_file_content_hash(file.id)?.unwrap();
    let record = store
        .get_nodes()?
        .into_iter()
        .find(|node| node.kind == NodeKind::STRUCT && node_matches_name(node, "Record"))
        .unwrap();
    // Actual file-owned derived rows must be removed even when the current
    // parser artifact and graph projection are already equal to the refresh.
    store.get_connection().execute(
        "INSERT INTO symbol_search_doc (
             node_id, file_node_id, kind, display_name, start_line, doc_text,
             doc_version, doc_hash, policy_version, source_provenance, updated_at_epoch_ms
         ) SELECT id, file_node_id, kind, serialized_name, start_line,
             'v5 expanded type input', 10, 'v5-sentinel', 'graph_first_v3', 'extracted', 1
           FROM node WHERE id = ?1",
        [record.id.0],
    )?;
    store.get_connection().execute(
        "INSERT INTO dense_anchor_input (
             node_id, file_node_id, kind, display_name, start_line, end_line, file_role,
             source_provenance, document_text, document_hash, selection_reason,
             policy_version, source_identity, updated_at_epoch_ms
         ) SELECT id, file_node_id, kind, serialized_name, start_line, end_line, 'source',
             'extracted', 'v5 expanded type input', 'v5-sentinel', 'public_api',
             'graph_first_v3', 'core:old-v5', 1 FROM node WHERE id = ?1",
        [record.id.0],
    )?;
    assert_eq!(
        store
            .get_symbol_search_docs_for_node_ids(&[record.id])?
            .len(),
        1
    );
    assert_eq!(
        store.get_dense_anchor_inputs_batch_after(None, 10)?.len(),
        1
    );
    let legacy_key = format!("{}:worker#0", WorkspaceIndexer::file_identity_path(&path));
    let phantom_key = format!("{}:worker#1", WorkspaceIndexer::file_identity_path(&path));
    store.get_connection().execute(
        "UPDATE node SET canonical_id = ?1 WHERE id = ?2",
        (&legacy_key, worker.id.0),
    )?;
    store.insert_node(&Node {
        id: NodeId(generate_id(&phantom_key)),
        kind: NodeKind::FUNCTION,
        serialized_name: "worker".to_string(),
        qualified_name: Some("worker".to_string()),
        canonical_id: Some(phantom_key),
        file_node_id: Some(NodeId(file.id)),
        start_line: Some(3),
        start_col: Some(4),
        end_line: Some(3),
        end_col: Some(10),
    })?;
    let legacy_revision = "canonical-reference-occurrences-v1;rust-declaration-source-v5";
    assert_ne!(
        revision, legacy_revision,
        "the previous Rust producer must be retired"
    );
    store.get_connection().execute(
        "UPDATE file_extraction_identity SET extraction_revision = ?1 WHERE file_id = ?2",
        (legacy_revision, file.id),
    )?;
    let workers = |store: &Storage| -> Result<Vec<Node>> {
        Ok(store
            .get_nodes()?
            .into_iter()
            .filter(|node| node.kind == NodeKind::FUNCTION && node.serialized_name == "worker")
            .collect())
    };
    assert_eq!(
        workers(&store)?.len(),
        2,
        "actual stale full and name-only graph"
    );
    assert!(!store.file_extraction_identity_matches(file.id, "rust", revision)?);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let cancelled_stats =
        indexer.run_incremental(&mut store, &plan, &EventBus::new(), Some(&cancelled))?;
    assert!(!cancelled_stats.graph_projection_changed);
    assert_eq!(stored_span(&store)?, (2, 1, 5, 2));
    assert_eq!(
        workers(&store)?.len(),
        2,
        "cancelled refresh preserves the old graph"
    );
    assert!(
        store.file_extraction_identity_matches(file.id, "rust", legacy_revision)?,
        "cancelled extraction never advances the producer"
    );
    assert_eq!(
        store
            .get_symbol_search_docs_for_node_ids(&[record.id])?
            .len(),
        1
    );
    assert_eq!(
        store.get_dense_anchor_inputs_batch_after(None, 10)?.len(),
        1
    );
    let repaired = indexer.run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    assert_eq!(
        repaired.artifact_cache_hits, 2,
        "both files replay authenticated current cached projections"
    );
    assert!(repaired.graph_projection_changed);
    assert!(
        store
            .get_symbol_search_docs_for_node_ids(&[record.id])?
            .is_empty()
    );
    assert!(
        store
            .get_dense_anchor_inputs_batch_after(None, 10)?
            .is_empty()
    );
    assert_eq!(store.get_node(record.id)?.unwrap().start_line, Some(6));
    assert_eq!(stored_span(&store)?, (2, 1, 5, 2));
    let repaired_workers = workers(&store)?;
    assert_eq!(
        repaired_workers.len(),
        1,
        "refresh removes the phantom definition"
    );
    assert!(
        repaired_workers[0]
            .canonical_id
            .as_deref()
            .unwrap()
            .starts_with(RUST_ORDINAL_CANONICAL_ID_PREFIX)
    );
    assert_eq!(
        store.get_file_content_hash(file.id)?.as_deref(),
        Some(source_hash.as_str())
    );
    assert!(store.file_extraction_identity_matches(file.id, "rust", revision)?);
    let reused = indexer.run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    assert_eq!(reused.source_identity_only_files, 2);
    assert!(!reused.graph_projection_changed);
    WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(16)
        .run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    assert_eq!(
        stored_span(&store)?,
        (2, 1, 5, 2),
        "incomplete retry preserves the last graph"
    );
    assert!(!store.get_file_by_path(&path)?.unwrap().complete);
    assert!(
        !store.file_extraction_identity_matches(file.id, "rust", revision)?,
        "incomplete extraction cannot certify its inherited graph as current"
    );
    Ok(())
}
