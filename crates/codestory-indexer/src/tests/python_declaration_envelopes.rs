use super::*;

#[test]
fn python_reference_occurrences_preserve_declarations_and_grounding_owners() -> Result<()> {
    let sources = [
        (
            "owners.py",
            "class Base:\n    pass\n\nclass Box(Base):\n    @property\n    def value(self):\n        return self._value\n\n    @classmethod\n    def create(cls, value):\n        result = cls()\n        result._value = value\n        return result\n\n    @staticmethod\n    def normalize(value):\n        return str(value).strip()\n\nclass Failure(Exception):\n    pass\n\ndef plain(value):\n    return Box.normalize(value)\n",
        ),
        (
            "mixed.py",
            "import plugins\n\ndef property(fn):\n    return fn\n\nclass LocalBase:\n    pass\n\nclass Child(LocalBase, plugins.Base):\n    @property\n    @plugins.trace('label')\n    def run(self):\n        return 1\n",
        ),
    ];
    let dir = tempdir()?;
    let paths = sources
        .iter()
        .map(|(name, source)| {
            let path = dir.path().join(name);
            std::fs::write(&path, source)?;
            Ok(path)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut store = Storage::new_in_memory()?;
    WorkspaceIndexer::new(dir.path().to_path_buf()).run_incremental(
        &mut store,
        &codestory_workspace::RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: paths.clone(),
            files_to_remove: vec![],
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    let nodes = store.get_nodes()?;
    let edges = store.get_edges()?;
    let file_ids = paths
        .iter()
        .map(|path| store.get_file_by_path(path).map(|file| file.unwrap().id))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let all_occurrences = store.get_occurrences()?;
    let node = |file: usize, name: &str, line: u32| {
        let file_id = file_ids[file];
        nodes
            .iter()
            .find(|node| {
                node.file_node_id == Some(NodeId(file_id))
                    && node_matches_name(node, name)
                    && all_occurrences
                        .iter()
                        .any(|occ| occ.element_id == node.id.0 && occ.location.start_line == line)
            })
            .unwrap_or_else(|| panic!("missing {name}:{line} in {:?}", paths[file]))
    };
    for (file, name, line) in [
        (0, "property", 5),
        (0, "classmethod", 9),
        (0, "staticmethod", 15),
        (0, "Exception", 19),
        (0, "Base", 4),
        (1, "property", 10),
        (1, "LocalBase", 9),
        (1, "plugins.Base", 9),
    ] {
        let target = node(file, name, line);
        let occurrences = store.get_occurrences_for_node(target.id)?;
        assert!(occurrences.iter().any(|occ| occ.location.start_line == line && occ.kind == OccurrenceKind::REFERENCE),
            "{name}:{line} must remain reference evidence: {occurrences:?}");
        assert!(
            !occurrences.iter().any(
                |occ| occ.location.start_line == line && occ.kind == OccurrenceKind::DEFINITION
            ),
            "{name}:{line} is not a local definition"
        );
    }
    for (file, name, line) in [
        (0, "Base", 1),
        (0, "Box", 4),
        (0, "value", 6),
        (0, "create", 10),
        (0, "normalize", 16),
        (0, "Failure", 19),
        (0, "plain", 22),
        (1, "property", 3),
        (1, "LocalBase", 6),
        (1, "Child", 9),
        (1, "run", 12),
    ] {
        let owner = node(file, name, line);
        assert!(
            store
                .get_occurrences_for_node(owner.id)?
                .iter()
                .any(
                    |occ| occ.kind == OccurrenceKind::DEFINITION && occ.location.start_line == line
                ),
            "{name}:{line} retains its real declaration; selected={owner:?}, occurrences={:?}, matching={:?}",
            store.get_occurrences_for_node(owner.id)?,
            nodes
                .iter()
                .filter(|n| n.file_node_id == owner.file_node_id && node_matches_name(n, name))
                .collect::<Vec<_>>()
        );
    }
    for (file, owner, owner_line, target, target_line, kind) in [
        (0, "Box", 4, "Base", 4, EdgeKind::INHERITANCE),
        (0, "Failure", 19, "Exception", 19, EdgeKind::INHERITANCE),
        (0, "value", 6, "property", 5, EdgeKind::CALL),
        (0, "create", 10, "classmethod", 9, EdgeKind::CALL),
        (0, "normalize", 16, "staticmethod", 15, EdgeKind::CALL),
        (1, "Child", 9, "plugins.Base", 9, EdgeKind::INHERITANCE),
        (1, "run", 12, "property", 10, EdgeKind::CALL),
    ] {
        assert!(
            edges.iter().any(|edge| edge.kind == kind
                && edge.source == node(file, owner, owner_line).id
                && edge.target == node(file, target, target_line).id),
            "{owner} -> {target} retains {kind:?}"
        );
    }
    for snapshot in [false, true] {
        if snapshot {
            store.refresh_grounding_snapshots()?;
        }
        let roots = store.get_grounding_root_symbol_candidates(100, 0)?;
        for (name, line) in [
            ("property", 5),
            ("classmethod", 9),
            ("staticmethod", 15),
            ("Exception", 19),
        ] {
            assert!(
                !roots
                    .iter()
                    .any(|root| root.node.id == node(0, name, line).id),
                "reference-only {name} is not an owner ({snapshot})"
            );
        }
        for (file, name, line) in [
            (0, "Base", 1),
            (0, "Box", 4),
            (0, "Failure", 19),
            (0, "plain", 22),
            (1, "property", 3),
            (1, "LocalBase", 6),
            (1, "Child", 9),
        ] {
            assert!(
                roots
                    .iter()
                    .any(|root| root.node.id == node(file, name, line).id),
                "real {name} remains an owner ({snapshot})"
            );
        }
    }
    Ok(())
}

#[test]
fn python_callable_declarations_join_exact_core_and_execution_body_spans() -> Result<()> {
    let cases = [
        ("plain", "def worker():\n    pass\n", 1, 2, 2, 1),
        ("async", "async def worker():\n    pass\n", 1, 2, 2, 1),
        (
            "decorated",
            "# before\n@trace\ndef worker():\n    pass\n# after\ndef sibling():\n    pass\n",
            2,
            4,
            4,
            1,
        ),
        (
            "decorated async",
            "@trace\nasync def worker():\n    pass\n",
            1,
            3,
            3,
            1,
        ),
        (
            "multiple multiline",
            "# before\n@cases(\n    'one',\n)\n# attached comment\n@trace\ndef worker():\n    pass\n# after\ndef sibling():\n    pass\n",
            2,
            8,
            8,
            1,
        ),
        (
            "method",
            "class C:\n    @trace\n    def worker(self):\n        pass\n",
            2,
            4,
            4,
            5,
        ),
        (
            "static method",
            "class C:\n    @staticmethod\n    def worker():\n        pass\n",
            2,
            4,
            4,
            5,
        ),
        (
            "class method",
            "class C:\n    @classmethod\n    async def worker(cls):\n        pass\n",
            2,
            4,
            4,
            5,
        ),
        (
            "nested",
            "def outer():\n    @trace\n    def worker():\n        pass\n    return worker\n",
            2,
            4,
            4,
            5,
        ),
        (
            "utf8",
            "@trace('é')\ndef worker(): return 'é'\n",
            1,
            2,
            2,
            1,
        ),
    ];
    for (case, source, start, end, body_start, column) in cases {
        let config = get_language_for_ext("py").expect("Python configuration");
        let indexed = index_file(Path::new("declarations.py"), source, &config, None, None)?;
        assert!(indexed.files[0].complete, "{case}");
        let workers = indexed
            .nodes
            .iter()
            .filter(|node| {
                matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                    && node_matches_name(node, "worker")
            })
            .collect::<Vec<_>>();
        assert_eq!(workers.len(), 1, "{case}: one normal declaration");
        let worker = workers[0];
        assert_eq!(
            (worker.start_line, worker.start_col, worker.end_line),
            (Some(start), Some(column), Some(end)),
            "{case}: declaration envelope"
        );
        assert_eq!(
            worker.end_col,
            Some(source.lines().nth(end as usize - 1).unwrap().len() as u32 + 1),
            "{case}: exclusive byte column"
        );
        let bodies = parsed_callable_body_spans(Path::new("declarations.py"), source)
            .expect("complete parser source");
        let joined = bodies
            .iter()
            .filter(|body| {
                Some(body.start_line) == worker.start_line
                    && Some(body.start_column) == worker.start_col
                    && Some(body.end_line) == worker.end_line
                    && Some(body.end_column) == worker.end_col
            })
            .collect::<Vec<_>>();
        assert_eq!(joined.len(), 1, "{case}: exact four-coordinate body join");
        assert_eq!(
            (joined[0].body_start_line, joined[0].body_end_line),
            (body_start, end),
            "{case}: execution body excludes declaration decorators"
        );
    }
    Ok(())
}

#[test]
fn python_decorator_arguments_keep_lexical_call_owners_and_receiver_edges() -> Result<()> {
    let source = r#"def case_values():
    return []

def factory(values):
    def decorate(fn):
        return fn
    return decorate

def target():
    return 1

@factory(case_values())
def top():
    return target()

def outer():
    @factory(case_values())
    def inner():
        return target()
    return inner()

class Receiver:
    def save(self):
        return 1

class Owner:
    @factory([])
    def worker(self, receiver: Receiver):
        return receiver.save()

def one_line(): return case_values()
"#;
    let dir = tempdir()?;
    let path = dir.path().join("owners.py");
    std::fs::write(&path, source)?;
    let mut store = Storage::new_in_memory()?;
    WorkspaceIndexer::new(dir.path().to_path_buf()).run_incremental(
        &mut store,
        &codestory_workspace::RefreshInfo {
            mode: codestory_workspace::BuildMode::Incremental,
            files_to_index: vec![path],
            files_to_remove: vec![],
            existing_file_ids: HashMap::new(),
        },
        &EventBus::new(),
        None,
    )?;
    let nodes = store.get_nodes()?;
    let edges = store.get_edges()?;
    let id = |name| {
        nodes
            .iter()
            .find(|node| {
                node_matches_name(node, name)
                    && matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
                    && node
                        .end_line
                        .zip(node.start_line)
                        .is_some_and(|(end, start)| end > start)
            })
            .unwrap_or_else(|| panic!("missing callable {name}"))
            .id
    };
    let calls = |from, to| {
        edges.iter().any(|edge| {
            edge.kind == EdgeKind::CALL
                && edge.source == id(from)
                && edge.effective_source() == id(from)
                && edge.effective_target() == id(to)
        })
    };
    let name = |id: NodeId| {
        nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| node.serialized_name.as_str())
            .unwrap_or("<missing>")
    };
    let call_diagnostic = edges.iter().filter(|edge| edge.kind == EdgeKind::CALL)
        .map(|edge| format!(
            "line={:?} raw_source={:?} ({}) resolved_source={:?} raw_target={:?} ({}) resolved_target={:?} ({})",
            edge.line, edge.source, name(edge.source), edge.resolved_source,
            edge.target, name(edge.target), edge.resolved_target, name(edge.effective_target()),
        )).collect::<Vec<_>>().join("\n");
    assert!(
        calls("outer", "case_values"),
        "decorator arguments execute in the enclosing callable:\n{call_diagnostic}"
    );
    assert!(!calls("inner", "case_values"), "{call_diagnostic}");
    assert!(!calls("top", "case_values"), "{call_diagnostic}");
    assert!(
        calls("inner", "factory"),
        "explicit decorator relation remains:\n{call_diagnostic}"
    );
    assert!(calls("top", "factory"), "{call_diagnostic}");
    assert!(calls("inner", "target"), "{call_diagnostic}");
    assert!(
        calls("worker", "save"),
        "decorated annotated receiver remains resolved:\n{call_diagnostic}"
    );
    let one_line = nodes
        .iter()
        .find(|node| {
            node_matches_name(node, "one_line")
                && matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        })
        .expect("one-line callable declaration");
    assert_eq!(one_line.start_line, one_line.end_line);
    assert!(
        edges.iter().any(|edge| edge.kind == EdgeKind::CALL
            && edge.source == one_line.id
            && edge.effective_source() == one_line.id
            && edge.effective_target() == id("case_values")),
        "{call_diagnostic}"
    );
    let owner = nodes
        .iter()
        .find(|node| node_matches_name(node, "Owner") && node.kind == NodeKind::CLASS)
        .expect("owner class")
        .id;
    assert!(edges.iter().any(|edge| edge.kind == EdgeKind::MEMBER
        && edge.source == owner
        && edge.target == id("worker")));
    Ok(())
}

#[test]
fn python_nested_same_name_declarations_keep_stable_distinct_ids() -> Result<()> {
    let before = "def one():\n    def worker():\n        pass\ndef two():\n    def worker():\n        pass\n";
    let after = "def one():\n    @trace\n    def worker():\n        pass\ndef two():\n    @trace\n    def worker():\n        pass\n";
    let config = get_language_for_ext("py").expect("Python configuration");
    let old = index_file(Path::new("nested.py"), before, &config, None, None)?;
    let new = index_file(Path::new("nested.py"), after, &config, None, None)?;
    let ids = |nodes: &[Node]| {
        nodes
            .iter()
            .filter(|node| {
                node_matches_name(node, "worker")
                    && matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
            })
            .map(|node| node.id)
            .collect::<HashSet<_>>()
    };
    assert_eq!(ids(&old.nodes).len(), 2);
    assert_eq!(
        ids(&old.nodes),
        ids(&new.nodes),
        "attached decorators preserve declaration ordering"
    );
    let bodies = parsed_callable_body_spans(Path::new("nested.py"), after).expect("parsed bodies");
    for node in new
        .nodes
        .iter()
        .filter(|node| node_matches_name(node, "worker"))
    {
        assert_eq!(
            bodies
                .iter()
                .filter(|body| Some(body.start_line) == node.start_line
                    && Some(body.start_column) == node.start_col
                    && Some(body.end_line) == node.end_line
                    && Some(body.end_column) == node.end_col)
                .count(),
            1
        );
    }
    assert!(
        parsed_callable_body_spans(Path::new("broken.py"), "@trace(\ndef worker(): pass\n")
            .is_none()
    );
    assert!(
        parsed_callable_body_spans(
            Path::new("large.py"),
            &format!(
                "@trace('{}')\ndef worker(): pass\n",
                "x".repeat(1024 * 1024)
            )
        )
        .is_none()
    );
    Ok(())
}

#[test]
fn stale_python_extraction_replaces_old_bounds_even_with_current_cached_projection() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("worker.py");
    let empty_path = dir.path().join("empty.py");
    std::fs::write(&path, "# before\n@trace\ndef worker():\n    return 1\n")?;
    std::fs::write(&empty_path, "VALUE = 1\n")?;
    let indexer = WorkspaceIndexer::new(dir.path().to_path_buf());
    let mut store = Storage::new_in_memory()?;
    let plan = codestory_workspace::RefreshInfo {
        mode: codestory_workspace::BuildMode::Incremental,
        files_to_index: vec![path.clone(), empty_path.clone()],
        files_to_remove: vec![],
        existing_file_ids: HashMap::new(),
    };
    indexer.run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    let revision = current_file_extraction_revision("python").unwrap();
    let file = store.get_file_by_path(&path)?.unwrap();
    let empty = store.get_file_by_path(&empty_path)?.unwrap();
    assert!(store.file_extraction_identity_matches(file.id, "python", revision)?);
    assert!(
        store.file_extraction_identity_matches(empty.id, "python", revision)?,
        "zero-callable Python files retain successful producer evidence"
    );
    let worker = store
        .get_nodes()?
        .into_iter()
        .find(|node| {
            node_matches_name(node, "worker")
                && matches!(node.kind, NodeKind::FUNCTION | NodeKind::METHOD)
        })
        .unwrap();
    assert_eq!(worker.start_line, Some(2));
    let stored_span = |store: &Storage| -> Result<(u32, u32, u32, u32)> {
        Ok(store.get_connection().query_row(
            "SELECT start_line, start_col, end_line, end_col FROM node WHERE id = ?1",
            [worker.id.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?)
    };
    assert_eq!(stored_span(&store)?, (2, 1, 4, 13));
    let source_hash = store.get_file_content_hash(file.id)?.unwrap();
    store.get_connection().execute(
        "UPDATE node SET start_line = 3, start_col = 1 WHERE id = ?1",
        [worker.id.0],
    )?;
    store.get_connection().execute(
        "UPDATE file_extraction_identity SET extraction_revision = 'old' WHERE file_id = ?1",
        [file.id],
    )?;
    assert_eq!(
        stored_span(&store)?,
        (3, 1, 4, 13),
        "actual old graph before repair"
    );
    assert!(!store.file_extraction_identity_matches(file.id, "python", revision)?);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let cancelled_stats =
        indexer.run_incremental(&mut store, &plan, &EventBus::new(), Some(&cancelled))?;
    assert!(!cancelled_stats.graph_projection_changed);
    assert_eq!(stored_span(&store)?, (3, 1, 4, 13));
    assert!(
        store.file_extraction_identity_matches(file.id, "python", "old")?,
        "cancelled extraction never advances the producer"
    );
    let repaired = indexer.run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    assert!(
        repaired.artifact_cache_hits > 0,
        "exercise authenticated current cache replay"
    );
    assert_eq!(stored_span(&store)?, (2, 1, 4, 13));
    assert_eq!(
        store.get_file_content_hash(file.id)?.as_deref(),
        Some(source_hash.as_str())
    );
    assert!(store.file_extraction_identity_matches(file.id, "python", revision)?);
    let reused = indexer.run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    assert_eq!(reused.source_identity_only_files, 2);
    assert!(!reused.graph_projection_changed);
    WorkspaceIndexer::new(dir.path().to_path_buf())
        .with_source_file_byte_cap(16)
        .run_incremental(&mut store, &plan, &EventBus::new(), None)?;
    assert_eq!(
        stored_span(&store)?,
        (2, 1, 4, 13),
        "incomplete retry preserves the last graph"
    );
    assert!(!store.get_file_by_path(&path)?.unwrap().complete);
    assert!(
        !store.file_extraction_identity_matches(file.id, "python", revision)?,
        "incomplete extraction cannot certify its inherited graph as current"
    );
    Ok(())
}
